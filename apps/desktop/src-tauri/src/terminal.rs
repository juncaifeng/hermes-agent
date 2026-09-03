//! Hermes Desktop — terminal (node-pty via a Node sidecar).
//!
//! node-pty is a native Node addon, so it cannot run inside the Rust process.
//! Instead we keep a single headless Node child — `sidecar/terminal-sidecar.cjs`
//! — that owns every pty session, and bridge it here over a JSON-lines protocol
//! on stdin/stdout. The renderer-facing surface is identical to the old
//! Electron `hermes:terminal:*` IPC:
//!
//!   terminal.start   → returns `{ cwd, id, shell }`
//!   terminal.write   → forward keystrokes to a session
//!   terminal.resize  → resize a session's cols/rows
//!   terminal.cwd     → best-effort live cwd of a session's shell
//!   terminal.dispose → kill a session
//!   `hermes:terminal:<id>:data` / `:exit` → emitted to the renderer
//!
//! Events are emitted by the sidecar actor thread using the `AppHandle`; the
//! renderer subscribes via `@tauri-apps/api/event.listen` (see the bridge).

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::mpsc;
use tauri::{AppHandle, Emitter};

const SIDECAR_SCRIPT: &str = "sidecar/terminal-sidecar.cjs";

// ---------------------------------------------------------------------------
// Actor control protocol
// ---------------------------------------------------------------------------

enum ActorMsg {
    Start {
        id: String,
        payload: StartPayload,
        reply: mpsc::SyncSender<Result<StartedInfo, String>>,
    },
    Write {
        id: String,
        data: String,
    },
    Resize {
        id: String,
        cols: u16,
        rows: u16,
    },
    Cwd {
        id: String,
        reply: mpsc::SyncSender<Option<String>>,
    },
    Dispose {
        id: String,
    },
}

#[derive(Deserialize, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct StartPayload {
    cwd: Option<String>,
    cols: Option<u16>,
    rows: Option<u16>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StartedInfo {
    cwd: Option<String>,
    id: String,
    shell: String,
}

/// Shared per-app terminal state. The sidecar actor (and its stdin) is spawned
/// lazily on the first `terminal.start`, so this is cheap when unused.
#[derive(Default)]
pub struct TerminalState {
    inner: std::sync::Mutex<Option<mpsc::Sender<ActorMsg>>>,
}

impl TerminalState {
    fn sender(&self, app: AppHandle) -> Result<mpsc::Sender<ActorMsg>, String> {
        let mut guard = self
            .inner
            .lock()
            .map_err(|_| "terminal state poisoned".to_string())?;
        if let Some(tx) = guard.as_ref() {
            return Ok(tx.clone());
        }
        let tx = spawn_actor(app.clone())?;
        guard.replace(tx.clone());
        Ok(tx)
    }
}

// ---------------------------------------------------------------------------
// Sidecar process lifecycle
// ---------------------------------------------------------------------------

fn find_on_path(name: &str) -> Option<String> {
    let path_env = std::env::var("PATH").unwrap_or_default();
    for dir in path_env.split(';') {
        if dir.is_empty() {
            continue;
        }
        let candidate = std::path::Path::new(dir).join(name);
        if candidate.is_file() {
            return Some(candidate.to_string_lossy().into_owned());
        }
    }
    None
}

fn resolve_node() -> Option<String> {
    for var in ["HERMES_NODE", "NODE"] {
        if let Ok(n) = std::env::var(var) {
            let n = n.trim();
            if !n.is_empty() {
                return Some(n.to_string());
            }
        }
    }
    find_on_path(if cfg!(windows) { "node.exe" } else { "node" })
}

fn sidecar_script_path() -> String {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(SIDECAR_SCRIPT)
        .to_string_lossy()
        .into_owned()
}

/// The sidecar runs with its cwd set to the desktop app root (the parent of
/// `src-tauri`) so its bare `require('node-pty')` resolves against the app's
/// `node_modules`.
fn sidecar_working_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap_or(std::path::Path::new("."))
        .to_path_buf()
}

fn spawn_actor(app: AppHandle) -> Result<mpsc::Sender<ActorMsg>, String> {
    let node = resolve_node().ok_or_else(|| {
        "Could not locate a node executable to run the terminal sidecar".to_string()
    })?;
    let script = sidecar_script_path();
    let cwd = sidecar_working_dir();

    // Debug: write to file so we can see it even if stderr is swallowed
    let script_exists = std::path::Path::new(&script).exists();
    let cwd_exists = cwd.exists();
    let debug_info = format!(
        "node={:?}\nscript={:?}\nscript_exists={}\ncwd={:?}\ncwd_exists={}\n",
        node, script, script_exists, cwd, cwd_exists
    );
    let _ = std::fs::write(r"E:\git\hermes-agent\terminal_debug.txt", &debug_info);
    eprintln!("[terminal] spawn_actor node={node:?} script={script:?} cwd={cwd:?}");

    let mut child = Command::new(&node)
        .arg(&script)
        .current_dir(&cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| {
            let err_detail = format!(
                "spawn error: {}, kind={:?}, raw_os={:?}",
                e, e.kind(), e.raw_os_error()
            );
            let _ = std::fs::write(r"E:\git\hermes-agent\terminal_spawn_error.txt", &err_detail);
            eprintln!("[terminal] {}", err_detail);
            format!("failed to spawn terminal sidecar: {e} (kind={:?}, raw_os={:?})", e.kind(), e.raw_os_error())
        })?;

    let stdin = child
        .stdin
        .take()
        .ok_or("terminal sidecar stdin unavailable")?;
    let stdout = child
        .stdout
        .take()
        .ok_or("terminal sidecar stdout unavailable")?;

    // Drain the sidecar's stderr (its diagnostic log channel) so it never
    // blocks on a full pipe, and forward each line to this process's stderr so
    // the sidecar's INFO/DEBUG/ERROR logs surface in the dev console / logs.
    if let Some(stderr) = child.stderr.take() {
        std::thread::spawn(move || drain_sidecar_stderr(stderr));
    }

    let (tx, rx) = mpsc::channel::<ActorMsg>();
    std::thread::spawn(move || actor_loop(app, child, stdin, stdout, rx));
    Ok(tx)
}

fn drain_sidecar_stderr(stderr: std::process::ChildStderr) {
    use std::io::{BufRead, BufReader};
    let mut reader = BufReader::new(stderr);
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) => eprint!("[terminal-sidecar] {line}"),
        }
    }
}

// ---------------------------------------------------------------------------
// Actor loop
// ---------------------------------------------------------------------------

fn actor_loop(
    app: AppHandle,
    mut child: Child,
    mut stdin: ChildStdin,
    stdout: ChildStdout,
    rx: mpsc::Receiver<ActorMsg>,
) {
    // Pending RPCs awaiting a response from the sidecar, keyed by session id.
    // Shared between this loop (which inserts them) and the reader thread
    // (which resolves them).
    let pending_start: std::sync::Arc<
        std::sync::Mutex<HashMap<String, mpsc::SyncSender<Result<StartedInfo, String>>>>,
    > = std::sync::Arc::new(std::sync::Mutex::new(HashMap::new()));
    let pending_cwd: std::sync::Arc<
        std::sync::Mutex<HashMap<String, mpsc::SyncSender<Option<String>>>>,
    > = std::sync::Arc::new(std::sync::Mutex::new(HashMap::new()));

    // Reader thread: forwards sidecar stdout events to the renderer or resolves
    // a pending RPC. Owns `stdout`; the main loop owns `stdin`.
    let reader_app = app.clone();
    let reader_start = pending_start.clone();
    let reader_cwd = pending_cwd.clone();
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    let Ok(v): Result<serde_json::Value, _> = serde_json::from_str(trimmed) else {
                        continue;
                    };
                    let ty = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
                    let id = v
                        .get("id")
                        .and_then(|i| i.as_str())
                        .unwrap_or("")
                        .to_string();
                    match ty {
                        "started" => {
                            if let Some(sender) =
                                reader_start.lock().ok().and_then(|mut m| m.remove(&id))
                            {
                                let shell = v
                                    .get("shell")
                                    .and_then(|s| s.as_str())
                                    .unwrap_or("")
                                    .to_string();
                                let cwd =
                                    v.get("cwd").and_then(|c| c.as_str()).map(|s| s.to_string());
                                let _ = sender.send(Ok(StartedInfo { cwd, id, shell }));
                            }
                        }
                        "error" => {
                            if let Some(sender) =
                                reader_start.lock().ok().and_then(|mut m| m.remove(&id))
                            {
                                let msg = v
                                    .get("message")
                                    .and_then(|m| m.as_str())
                                    .unwrap_or("sidecar error")
                                    .to_string();
                                let _ = sender.send(Err(msg));
                            }
                        }
                        "cwd" => {
                            if let Some(sender) =
                                reader_cwd.lock().ok().and_then(|mut m| m.remove(&id))
                            {
                                let c =
                                    v.get("cwd").and_then(|c| c.as_str()).map(|s| s.to_string());
                                let _ = sender.send(c);
                            }
                        }
                        "data" => {
                            let data = v
                                .get("data")
                                .and_then(|d| d.as_str())
                                .unwrap_or("")
                                .to_string();
                            let event = format!("hermes:terminal:{id}:data");
                            let _ = reader_app.emit(&event, data);
                        }
                        "exit" => {
                            let code = v.get("code").and_then(|c| c.as_i64()).unwrap_or(0);
                            let signal = v
                                .get("signal")
                                .and_then(|s| s.as_str())
                                .map(|s| s.to_string());
                            let event = format!("hermes:terminal:{id}:exit");
                            let _ = reader_app.emit(
                                &event,
                                serde_json::json!({ "code": code, "signal": signal }),
                            );
                        }
                        _ => {}
                    }
                }
            }
        }
    });

    loop {
        match rx.recv() {
            Err(_) => break,
            Ok(msg) => match msg {
                ActorMsg::Start { id, payload, reply } => {
                    if let Ok(mut m) = pending_start.lock() {
                        m.insert(id.clone(), reply);
                    }
                    let obj = serde_json::json!({ "type": "start", "id": id, "payload": payload });
                    let _ = writeln!(stdin, "{obj}").and_then(|_| stdin.flush());
                }
                ActorMsg::Write { id, data } => {
                    let obj = serde_json::json!({ "type": "write", "id": id, "data": data });
                    let _ = writeln!(stdin, "{obj}").and_then(|_| stdin.flush());
                }
                ActorMsg::Resize { id, cols, rows } => {
                    let obj = serde_json::json!({ "type": "resize", "id": id, "cols": cols, "rows": rows });
                    let _ = writeln!(stdin, "{obj}").and_then(|_| stdin.flush());
                }
                ActorMsg::Cwd { id, reply } => {
                    if let Ok(mut m) = pending_cwd.lock() {
                        m.insert(id.clone(), reply);
                    }
                    let obj = serde_json::json!({ "type": "cwd", "id": id });
                    let _ = writeln!(stdin, "{obj}").and_then(|_| stdin.flush());
                }
                ActorMsg::Dispose { id } => {
                    let obj = serde_json::json!({ "type": "dispose", "id": id });
                    let _ = writeln!(stdin, "{obj}").and_then(|_| stdin.flush());
                }
            },
        }
    }

    let _ = writeln!(stdin, "{{\"type\":\"shutdown\"}}").and_then(|_| stdin.flush());
    let _ = child.kill();
}

// ---------------------------------------------------------------------------
// Tauri commands
// ---------------------------------------------------------------------------

fn gen_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    format!("term-{:x}-{n}", ms)
}

/// `hermes:terminal:start` — start an embedded shell session.
#[tauri::command]
pub async fn terminal_start(
    state: tauri::State<'_, TerminalState>,
    app: AppHandle,
    payload: Option<StartPayload>,
) -> Result<StartedInfo, String> {
    let tx = state.sender(app)?;
    let id = gen_id();
    let (reply_tx, reply_rx) = mpsc::sync_channel::<Result<StartedInfo, String>>(1);
    tx.send(ActorMsg::Start {
        id: id.clone(),
        payload: payload.unwrap_or_default(),
        reply: reply_tx,
    })
    .map_err(|_| "terminal sidecar is not running".to_string())?;

    let recv: Result<Result<StartedInfo, String>, mpsc::RecvError> =
        tauri::async_runtime::spawn_blocking(move || reply_rx.recv())
            .await
            .map_err(|_| "terminal request task failed".to_string())?;
    let mut info = recv.map_err(|_| "terminal sidecar closed".to_string())??;
    info.id = id;
    Ok(info)
}

/// `hermes:terminal:write` — forward input to a session.
#[tauri::command]
pub fn terminal_write(
    state: tauri::State<'_, TerminalState>,
    app: AppHandle,
    id: String,
    data: String,
) -> Result<bool, String> {
    let tx = state.sender(app)?;
    tx.send(ActorMsg::Write { id, data })
        .map_err(|_| "terminal sidecar is not running".to_string())?;
    Ok(true)
}

/// `hermes:terminal:resize` — resize a session.
#[tauri::command]
pub fn terminal_resize(
    state: tauri::State<'_, TerminalState>,
    app: AppHandle,
    id: String,
    cols: u16,
    rows: u16,
) -> Result<bool, String> {
    let tx = state.sender(app)?;
    tx.send(ActorMsg::Resize { id, cols, rows })
        .map_err(|_| "terminal sidecar is not running".to_string())?;
    Ok(true)
}

/// `hermes:terminal:cwd` — best-effort live cwd of a session's shell.
#[tauri::command]
pub async fn terminal_cwd(
    state: tauri::State<'_, TerminalState>,
    app: AppHandle,
    id: String,
) -> Result<Option<String>, String> {
    let tx = state.sender(app)?;
    let (reply_tx, reply_rx) = mpsc::sync_channel::<Option<String>>(1);
    tx.send(ActorMsg::Cwd {
        id,
        reply: reply_tx,
    })
    .map_err(|_| "terminal sidecar is not running".to_string())?;

    let recv: Result<Option<String>, mpsc::RecvError> =
        tauri::async_runtime::spawn_blocking(move || reply_rx.recv())
            .await
            .map_err(|_| "terminal request task failed".to_string())?;
    recv.map_err(|_| "terminal sidecar closed".to_string())
}

/// `hermes:terminal:dispose` — kill a session.
#[tauri::command]
pub fn terminal_dispose(
    state: tauri::State<'_, TerminalState>,
    app: AppHandle,
    id: String,
) -> Result<bool, String> {
    let tx = state.sender(app)?;
    tx.send(ActorMsg::Dispose { id })
        .map_err(|_| "terminal sidecar is not running".to_string())?;
    Ok(true)
}
