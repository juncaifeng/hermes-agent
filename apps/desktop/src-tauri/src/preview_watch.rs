//! Preview file/directory watching (port of Electron's `watchPreviewFile` /
//! `watchDirectory` / `stopPreviewFileWatch`, main.ts ~6160).
//!
//! Same contract as the Electron original:
//!   - watch the PARENT directory of the target file (some editors replace
//!     files atomically, which a direct file watch misses),
//!   - filter events to the target's file name,
//!   - debounce 120ms (reset on every matching event),
//!   - only fire when the target still exists,
//!   - emit `hermes:preview-file-changed` `{id, path, url}` to the renderer.
//!
//! The watcher registry is per-app; stopping a watch drops the notify
//! watcher and ends its dispatcher thread.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{channel, RecvTimeoutError, Sender};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use notify::{RecursiveMode, Watcher};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter};

const PREVIEW_WATCH_DEBOUNCE_MS: Duration = Duration::from_millis(120);

struct WatchEntry {
    /// Dropping the notify watcher stops OS events; keep it owned here.
    _watcher: notify::RecommendedWatcher,
    /// Signals the dispatcher thread to stand down (watch stop).
    stop: Sender<()>,
}

#[derive(Default)]
pub struct PreviewWatchState {
    watchers: Mutex<HashMap<String, WatchEntry>>,
}

/// Percent-encode a path into a `file://` URL (mirrors Node's
/// `pathToFileURL` closely enough for the renderer's preview reload: encode
/// everything outside the URL-unreserved set except the path separators).
fn path_to_file_url(path: &std::path::Path) -> String {
    let mut out = String::from("file:///");
    for ch in path.to_string_lossy().chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '.' | '_' | '~' | '/' | ':' | '\\') {
            if ch == '\\' {
                out.push('/');
            } else {
                out.push(ch);
            }
        } else {
            let mut buf = [0u8; 4];
            for b in ch.encode_utf8(&mut buf).as_bytes() {
                out.push_str(&format!("%{b:02X}"));
            }
        }
    }
    out
}

/// Convert a preview target (file:// URL or plain path) to a filesystem
/// path. Errors when the URL is not a local file URL.
fn file_path_from_preview_target(raw: &str) -> Result<PathBuf, String> {
    let value = raw.trim();
    if let Some(rest) = value.strip_prefix("file://") {
        // Strip an authority component (file://localhost/x → /x).
        let path = if let Some(stripped) = rest.strip_prefix("localhost/") {
            format!("/{stripped}")
        } else if rest.starts_with('/') {
            rest.to_string()
        } else {
            return Err(format!("not a local file URL: {value}"));
        };
        // Percent-decode.
        let decoded = percent_decode(&path).ok_or_else(|| format!("invalid file URL: {value}"))?;
        // Windows drive letters arrive as /C:/… (Node keeps the leading slash
        // off; match that shape for downstream basename/dirname logic).
        let trimmed = if cfg!(windows) {
            decoded.trim_start_matches('/').to_string()
        } else {
            decoded
        };
        return Ok(PathBuf::from(trimmed));
    }
    if value.is_empty() {
        return Err("Preview target is required.".to_string());
    }
    Ok(PathBuf::from(value))
}

fn percent_decode(input: &str) -> Option<String> {
    let bytes = input.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if i + 3 > bytes.len() {
                return None;
            }
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok()?;
            let value = u8::from_str_radix(hex, 16).ok()?;
            out.push(value);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

fn emit_preview_changed(app: &AppHandle, id: &str, path: &std::path::Path) {
    let _ = app.emit(
        "hermes:preview-file-changed",
        json!({
            "id": id,
            "path": path.to_string_lossy(),
            "url": path_to_file_url(path),
        }),
    );
}

/// Debounced dispatcher: waits for the first matching event, then keeps
/// extending the quiet window on every further event (Electron resets its
/// timer the same way), and fires once the stream is quiet.
fn spawn_dispatcher(
    app: AppHandle,
    id: String,
    target: PathBuf,
    target_name: Option<String>,
    events: std::sync::mpsc::Receiver<Option<PathBuf>>,
    stop: std::sync::mpsc::Receiver<()>,
) {
    std::thread::spawn(move || {
        loop {
            // Wait for either a stop signal or the first event.
            let first = loop {
                match events.try_recv() {
                    Ok(ev) => break ev,
                    Err(_) => {}
                }
                match stop.recv_timeout(Duration::from_millis(200)) {
                    Ok(()) => return,
                    Err(RecvTimeoutError::Timeout) => continue,
                    Err(RecvTimeoutError::Disconnected) => return,
                }
            };

            // File watches filter to the target name (an empty name passes,
            // mirroring Electron's `changedName &&` guard).
            if let Some(name) = &target_name {
                let matches = first
                    .as_ref()
                    .map(|p| {
                        p.file_name()
                            .map(|n| n == name.as_str())
                            .unwrap_or(true)
                    })
                    .unwrap_or(true);
                if !matches {
                    continue;
                }
            }

            // Debounce: extend while matching events keep arriving.
            let mut deadline = Instant::now() + PREVIEW_WATCH_DEBOUNCE_MS;
            loop {
                let now = Instant::now();
                if now >= deadline {
                    break;
                }
                match events.recv_timeout(deadline - now) {
                    Ok(Some(ev)) => {
                        if let Some(name) = &target_name {
                            let matches = ev
                                .file_name()
                                .map(|n| n == name.as_str())
                                .unwrap_or(true);
                            if !matches {
                                continue;
                            }
                        }
                        deadline = Instant::now() + PREVIEW_WATCH_DEBOUNCE_MS;
                    }
                    Ok(None) => {
                        deadline = Instant::now() + PREVIEW_WATCH_DEBOUNCE_MS;
                    }
                    Err(RecvTimeoutError::Timeout) => break,
                    Err(RecvTimeoutError::Disconnected) => return,
                }
            }

            // Only fire when the target still exists (deleted-then-recreated
            // races resolve on the recreate event).
            if target.exists() {
                emit_preview_changed(&app, &id, &target);
            }
        }
    });
}

fn register_watch(
    app: &AppHandle,
    state: &tauri::State<'_, PreviewWatchState>,
    watch_dir: PathBuf,
    target: PathBuf,
    target_name: Option<String>,
) -> Result<Value, String> {
    if !watch_dir.is_dir() {
        return Err(format!("Not a directory: {}", watch_dir.display()));
    }

    let (event_tx, event_rx) = channel::<Option<PathBuf>>();
    let mut watcher = notify::recommended_watcher(move |res: Result<notify::Event, _>| {
        if let Ok(event) = res {
            for path in event.paths {
                let _ = event_tx.send(Some(path));
            }
        }
    })
    .map_err(|e| format!("could not create watcher: {e}"))?;
    watcher
        .watch(&watch_dir, RecursiveMode::NonRecursive)
        .map_err(|e| format!("could not watch {}: {e}", watch_dir.display()))?;

    let id: String = {
        use rand::Rng;
        let mut buf = [0u8; 9];
        rand::rng().fill_bytes(&mut buf);
        buf.iter().map(|b| format!("{b:02x}")).collect::<String>()
    };
    let (stop_tx, stop_rx) = channel::<()>();
    spawn_dispatcher(app.clone(), id.clone(), target.clone(), target_name, event_rx, stop_rx);

    if let Ok(mut guard) = state.watchers.lock() {
        guard.insert(
            id.clone(),
            WatchEntry {
                _watcher: watcher,
                stop: stop_tx,
            },
        );
    }
    Ok(json!({ "id": id, "path": target.to_string_lossy() }))
}

/// `hermes:preview-watch:watch` — watch a preview FILE (its parent dir,
/// filtered to the file's name).
#[tauri::command]
pub fn watch_preview_file(
    app: AppHandle,
    state: tauri::State<'_, PreviewWatchState>,
    url: String,
) -> Result<Value, String> {
    let file_path = file_path_from_preview_target(&url)?;
    if !file_path.is_file() {
        return Err(format!("Not a file: {}", file_path.display()));
    }
    let watch_dir = file_path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    let target_name = file_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned());
    register_watch(&app, &state, watch_dir, file_path.clone(), target_name)
}

/// `hermes:preview-watch:watch-dir` — watch a DIRECTORY for entry churn
/// (the disk-plugin door's "new plugin folder" signal).
#[tauri::command]
pub fn watch_directory(
    app: AppHandle,
    state: tauri::State<'_, PreviewWatchState>,
    dir: String,
) -> Result<Value, String> {
    let watch_dir = PathBuf::from(dir.trim());
    if !watch_dir.is_dir() {
        return Err(format!("Not a directory: {}", watch_dir.display()));
    }
    register_watch(&app, &state, watch_dir.clone(), watch_dir.clone(), None)
}

/// `hermes:preview-watch:stop` — stop one watch by id.
#[tauri::command]
pub fn stop_preview_file_watch(
    state: tauri::State<'_, PreviewWatchState>,
    id: String,
) -> Result<bool, String> {
    let entry = state
        .watchers
        .lock()
        .ok()
        .and_then(|mut guard| guard.remove(&id));
    match entry {
        Some(entry) => {
            let _ = entry.stop.send(());
            Ok(true)
        }
        None => Ok(false),
    }
}

/// Close every watch (main window destroyed).
pub fn close_preview_watchers(state: &PreviewWatchState) {
    if let Ok(mut guard) = state.watchers.lock() {
        for (_, entry) in guard.drain() {
            let _ = entry.stop.send(());
        }
    }
}
