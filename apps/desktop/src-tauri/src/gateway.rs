//! Hermes Desktop — managed headless gateway (`hermes serve`) process.
//!
//! On app startup we spawn the Python gateway. Resolution order:
//!   0. bundled backend (MSI resource `<resource_dir>/hermes-backend/` — a
//!      python-build-standalone runtime + `launch.py` shim produced by
//!      `scripts/build-backend.ps1`, also picked up from the repo layout in
//!      `tauri dev`)
//!   1. `HERMES_DESKTOP_HERMES` env override
//!   2. `%USERPROFILE%\.local\bin\hermes.exe` (uv tool install location)
//!   3. `hermes(.exe)` on `PATH`
//!   4. first-run bootstrap via `uv tool install hermes-agent`
//!
//! The gateway prints a `HERMES_BACKEND_READY port=<N>` (or legacy
//! `HERMES_DASHBOARD_READY port=<N>`) sentinel on stdout once uvicorn binds
//! its ephemeral socket; we parse that line and rewrite the shared
//! [`BackendState`] so the renderer's `getConnection` resolves the real port.
//! This mirrors the Electron `electron/backend-ready.ts` flow.
//!
//! If no runnable backend can be found, the gateway is left unmanaged and
//! `BackendState` keeps its default `http://127.0.0.1:8080` target (a user who
//! starts a backend themselves still connects).

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;

use rand::Rng;
use tauri::{AppHandle, Emitter, Manager};

use crate::commands::{BackendState, BootProgress, DEFAULT_BACKEND_BASE};

// `base64::Engine` trait brings `encode` into scope for URL_SAFE_NO_PAD.
use base64::Engine as _;

/// Managed child-process handle for the gateway, so the app can tear it down
/// on exit instead of leaking an orphaned `hermes serve`.
#[derive(Default)]
pub struct GatewayState {
    child: Mutex<Option<Child>>,
}

/// Cold-start tolerant deadline for the port-announcement sentinel — the clock
/// starts before the backend has even bound its socket (matches the Electron
/// `DEFAULT_PORT_ANNOUNCE_TIMEOUT_MS`).
const PORT_ANNOUNCE_TIMEOUT_MS: u64 = 90_000;

/// How to launch the gateway: the program plus the argv that must precede the
/// `serve` subcommand. For a hermes CLI executable `pre_args` is empty; for the
/// bundled python-build-standalone backend it is the `launch.py` shim path.
struct HermesLaunch {
    program: PathBuf,
    pre_args: Vec<String>,
}

impl HermesLaunch {
    fn exe(path: PathBuf) -> Self {
        Self { program: path, pre_args: Vec::new() }
    }
}

/// Bundled backend (scripts/build-backend.ps1 output): a relocatable CPython
/// runtime under `<base>/python/` plus a `launch.py` shim. Checked both in the
/// MSI resource dir and, for `tauri dev`, the repo checkout layout.
fn resolve_bundled_backend(app: &AppHandle) -> Option<HermesLaunch> {
    let mut bases: Vec<PathBuf> = Vec::new();
    // Installed deployment: resource dir mapped from
    // tauri.conf.json `resources["../backend-dist/hermes-backend"]`.
    if let Ok(res_dir) = app.path().resource_dir() {
        bases.push(res_dir.join("hermes-backend"));
    }
    // Dev: repo layout, so `tauri dev` picks a locally built bundle without
    // installing the MSI. Harmless in release builds (path won't exist).
    bases.push(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("backend-dist")
            .join("hermes-backend"),
    );

    let exe = if cfg!(windows) { "python.exe" } else { "python" };
    for base in bases {
        let python = base.join("python").join(exe);
        let shim = base.join("launch.py");
        if python.is_file() && shim.is_file() {
            return Some(HermesLaunch {
                program: python,
                pre_args: vec![shim.to_string_lossy().into_owned()],
            });
        }
    }
    None
}

/// Locate a runnable `hermes` backend:
///   0. bundled backend (MSI resource / dev repo layout)
///   1. `HERMES_DESKTOP_HERMES` env override
///   2. `%USERPROFILE%\.local\bin\hermes.exe` (uv tool install location)
///   3. `hermes(.exe)` on `PATH`
fn resolve_hermes_binary(app: &AppHandle) -> Option<HermesLaunch> {
    if let Some(launch) = resolve_bundled_backend(app) {
        return Some(launch);
    }
    if let Ok(override_path) = std::env::var("HERMES_DESKTOP_HERMES") {
        let pb = PathBuf::from(&override_path);
        if pb.is_file() {
            return Some(HermesLaunch::exe(pb));
        }
    }

    if let Some(home) = std::env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
    {
        let pb = home.join(".local").join("bin").join("hermes.exe");
        if pb.is_file() {
            return Some(HermesLaunch::exe(pb));
        }
    }

    let exe = if cfg!(windows) {
        "hermes.exe"
    } else {
        "hermes"
    };
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            let candidate = dir.join(exe);
            if candidate.is_file() {
                return Some(HermesLaunch::exe(candidate));
            }
        }
    }

    None
}

/// Locate a runnable `uv` executable: PATH first, then common installer
/// locations (uv standalone installs to `~/.local/bin`, cargo-binstall to
/// `~/.cargo/bin`, WinGet links to `%LOCALAPPDATA%\Microsoft\WinGet\Links`).
fn resolve_uv() -> Option<PathBuf> {
    let exe = if cfg!(windows) { "uv.exe" } else { "uv" };
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            let candidate = dir.join(exe);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    let home = std::env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(PathBuf::from))?;
    for sub in [
        ".local/bin",
        ".cargo/bin",
        "AppData/Local/Microsoft/WinGet/Links",
    ] {
        let candidate = home.join(sub).join(exe);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// First-run bootstrap: install the hermes CLI from PyPI via `uv tool
/// install hermes-agent`, then resolve the freshly-installed binary. Broadcasts
/// `backend.installing` boot progress while it runs. Returns None when uv is
/// missing or the install fails (the caller emits the failure state).
///
/// The default index (pypi.org) is tried first; if it fails — pypi.org is
/// frequently unreachable from CN networks — we retry once against the TUNA
/// mirror, which is the pragmatic default for Chinese users.
fn bootstrap_backend(app: &AppHandle) -> Option<HermesLaunch> {
    let _ = app.emit("hermes:boot-progress", BootProgress::installing());
    let uv = resolve_uv()?;
    eprintln!("[gateway] bootstrapping backend via {uv:?} (uv tool install hermes-agent)");
    if run_uv_tool_install(&uv, None) {
        return resolve_hermes_binary(app);
    }
    eprintln!("[gateway] default PyPI index failed; retrying via TUNA mirror");
    if run_uv_tool_install(&uv, Some("https://pypi.tuna.tsinghua.edu.cn/simple")) {
        return resolve_hermes_binary(app);
    }
    eprintln!("[gateway] uv tool install hermes-agent failed on both indexes");
    None
}

fn run_uv_tool_install(uv: &Path, index: Option<&str>) -> bool {
    let mut cmd = Command::new(uv);
    cmd.args(["tool", "install", "--force", "hermes-agent"])
        .env("HERMES_DESKTOP", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    if let Some(idx) = index {
        cmd.env("UV_DEFAULT_INDEX", idx);
    }
    cmd.status().map(|s| s.success()).unwrap_or(false)
}
fn parse_ready_port(line: &str) -> Option<u16> {
    let line = line.trim();
    let eq = line.find("port=")?;
    let prefix = line[..eq].trim();
    if !prefix.ends_with("HERMES_BACKEND_READY") && !prefix.ends_with("HERMES_DASHBOARD_READY") {
        return None;
    }
    let port_str = line[eq + "port=".len()..].split_whitespace().next()?;
    port_str.parse::<u16>().ok()
}

/// Generate a random dashboard session token (urlsafe base64, 32 bytes). We
/// pin it as `HERMES_DASHBOARD_SESSION_TOKEN` on the spawned backend so
/// `web_server._resolve_session_token` adopts it as its `_SESSION_TOKEN`; the
/// renderer then authenticates `/api/ws` with the same value (see
/// `get_connection`). Mirrors Electron's spawn-token flow.
fn generate_session_token() -> String {
    let mut buf = [0u8; 32];
    rand::rng().fill_bytes(&mut buf);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(buf)
}

/// Spawn the managed gateway and, on a background thread, wait for its
/// port-announcement sentinel before rewriting [`BackendState`]. Best-effort:
/// any failure (missing binary, spawn error, timeout) only logs, marks the
/// state ready with the default target (so the renderer's boot resolves), and
/// leaves the default backend target untouched.
///
/// The whole resolve → bootstrap → spawn sequence runs on a background thread:
/// the first-run `uv tool install hermes-agent` can block for 1–3 minutes, and
/// must never freeze the Tauri setup/UI thread. Boot progress is broadcast to
/// the renderer as `hermes:boot-progress` events (`backend.starting` →
/// `backend.installing` → `backend.ready` / `backend.failed`).
pub fn spawn_gateway(app: AppHandle, state: BackendState) {
    std::thread::spawn(move || {
        // GatewayState is managed by Tauri ('static storage); grab it inside
        // the thread so the setup closure's borrow doesn't escape.
        let gw = app.state::<GatewayState>();
        let Some(launch) = resolve_hermes_binary(&app).or_else(|| bootstrap_backend(&app)) else {
            eprintln!("[gateway] hermes executable not found; leaving default backend target");
            state.mark_ready(DEFAULT_BACKEND_BASE.to_string(), String::new());
            let _ = app.emit(
                "hermes:boot-progress",
                BootProgress::error(
                    "hermes executable not found and first-run install failed — install uv, then run: uv tool install hermes-agent",
                ),
            );
            return;
        };
        let program = launch.program.clone();

        // Pin the session token the backend will serve; get_connection returns it
        // to the renderer for /api/ws auth.
        let token = generate_session_token();

        let mut child = {
            let mut cmd = Command::new(&launch.program);
            cmd.args(&launch.pre_args)
                .args(["serve", "--host", "127.0.0.1", "--port", "0"])
                .env("HERMES_DESKTOP", "1")
                .env("HERMES_SERVE_HEADLESS", "1")
                .env("HERMES_DASHBOARD_SESSION_TOKEN", &token)
                // Bundled-backend niceties: UTF-8 stdio regardless of the
                // system codepage, and no read-only-site-packages bytecode
                // writes (also set in launch.py for direct invocations).
                .env("PYTHONUTF8", "1")
                .env("PYTHONDONTWRITEBYTECODE", "1")
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            // Detach the backend from our console so a console-window kill
            // doesn't take it down, and vice versa.
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt as _;
                const CREATE_NO_WINDOW: u32 = 0x0800_0000;
                cmd.creation_flags(CREATE_NO_WINDOW);
            }
            match cmd.spawn() {
                Ok(child) => child,
                Err(err) => {
                    eprintln!("[gateway] failed to spawn {program:?}: {err}");
                    state.mark_ready(DEFAULT_BACKEND_BASE.to_string(), String::new());
                    let _ = app.emit(
                        "hermes:boot-progress",
                        BootProgress::error(format!("failed to start the Hermes backend: {err}")),
                    );
                    return;
                }
            }
        };

        let stdout = child.stdout.take();
        let stderr = child.stderr.take();

        // If the window was destroyed while the first-run bootstrap ran
        // (1–3 min), `stop_gateway` already ran on a child we haven't stored
        // yet — killing now avoids leaking the backend as an orphan.
        if app.get_webview_window("main").is_none() {
            eprintln!("[gateway] window gone during bootstrap; killing backend");
            let _ = child.kill();
            let _ = child.wait();
            return;
        }

        if let Ok(mut guard) = gw.child.lock() {
            *guard = Some(child);
        }

        let Some(stdout) = stdout else {
            eprintln!("[gateway] stdout unavailable for {program:?}");
            return;
        };

        let app_for_reader = app.clone();

        // Forward backend stderr to our own stderr without inheriting the
        // child's handle — on Windows an inherited (redirected-file) stderr
        // blocks the child process, so we must pipe and drain it instead.
        if let Some(stderr) = stderr {
            std::thread::spawn(move || {
                use std::io::BufRead as _;
                let reader = std::io::BufReader::new(stderr);
                for line in reader.lines() {
                    let Ok(line) = line else { break };
                    eprintln!("[backend] {line}");
                }
            });
        }

        std::thread::spawn(move || {
            let reader = BufReader::new(stdout);
            let deadline = std::time::Instant::now()
                + std::time::Duration::from_millis(PORT_ANNOUNCE_TIMEOUT_MS);
            let mut announced = false;

            for line in reader.lines() {
                if std::time::Instant::now() > deadline {
                    eprintln!("[gateway] timed out waiting for port announcement");
                    break;
                }
                let Ok(line) = line else { break };

                if let Some(port) = parse_ready_port(&line) {
                    let url = format!("http://127.0.0.1:{port}");
                    state.mark_ready(url.clone(), token);
                    let _ = app_for_reader.emit("hermes:boot-progress", BootProgress::ready());
                    eprintln!("[gateway] backend ready -> {url}");
                    announced = true;
                    break;
                }
            }

            if !announced {
                // Timeout or the stream ended without the sentinel: release any
                // `get_connection` waiter against the default target so boot
                // resolves instead of hanging; the renderer surfaces the failure.
                state.mark_ready(DEFAULT_BACKEND_BASE.to_string(), String::new());
                let _ = app_for_reader.emit(
                    "hermes:boot-progress",
                    BootProgress::error("the Hermes backend did not become ready in time"),
                );
            }
        });
    });
}

/// Tear down the managed gateway child (best-effort; no-op if already gone).
pub fn stop_gateway(gw: &GatewayState) {
    if let Ok(mut guard) = gw.child.lock() {
        if let Some(mut child) = guard.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}
