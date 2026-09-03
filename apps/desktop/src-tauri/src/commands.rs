//! Hermes Desktop — Tauri v2 command layer.
//!
//! Implements a first, working subset of the Electron IPC surface that the
//! `window.hermesDesktop` bridge targets (see `docs/tauri-migration-map.md`).
//! Each `#[tauri::command]` here maps 1:1 to a bridge method; the bridge lives
//! in `../../src/desktop-bridge.ts` and routes through `@tauri-apps/api/core.invoke`.
//!
//! Focus of this increment (migration ordering 0/1/4):
//!   - `hermes:api` — generic REST proxy to `hermes serve` (largest flat surface)
//!   - `hermes:version` — build metadata
//!   - `hermes:connection` — resolve the (local) backend descriptor
//!   - basic FS reads (readDir / readFileText / readFileDataUrl / writeTextFile)
//!   - `hermes:openExternal`
//!
//! The rest of the 126-channel surface is added in later increments.

use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::Manager;

/// `*_state` constants mirror the Electron `serveBackendArgs` defaults.
pub const DEFAULT_BACKEND_BASE: &str = "http://127.0.0.1:8080";
const DEFAULT_FETCH_TIMEOUT_MS: u64 = 20_000;

/// How long `get_connection` blocks waiting for the managed gateway's
/// port-announcement sentinel before giving up and returning the default
/// target. Mirrors the Electron `DEFAULT_PORT_ANNOUNCE_TIMEOUT_MS` (90s) —
/// cold starts on a slow disk / aggressive AV can take 30-60s before uvicorn
/// binds, so a tight deadline would kill a healthy-but-starting backend.
/// Cold-start tolerant deadline for the managed gateway to announce its
/// ephemeral port (mirrors Electron's `BACKEND_READY_TIMEOUT_SECS`).
pub(crate) const BACKEND_READY_TIMEOUT_SECS: u64 = 90;

// ---------------------------------------------------------------------------
// Shared connection state
// ---------------------------------------------------------------------------

/// Live backend connection descriptor, managed by the main process and read by
/// commands that need to reach the backend (the `api` proxy, the gateway WS
/// URL, …). The renderer never sends its token; commands attach it here.
///
/// `base_url` is a `Mutex` because the gateway spawn thread (`gateway.rs`)
/// rewrites it to the ephemeral port the backend announces at boot. `Clone`
/// lets the spawn thread own a copy of the shared handle.
#[derive(Clone)]
pub struct BackendState {
    base_url: Arc<Mutex<String>>,
    token: Arc<Mutex<Option<String>>>,
    /// Set by `gateway.rs` once the spawned `hermes serve` announces its
    /// ephemeral port; `get_connection` blocks on this so the renderer never
    /// dials the default `:8080` target before the backend is actually up
    /// (mirrors Electron's `ensureBackend` semantics).
    ready: Arc<(Mutex<bool>, Condvar)>,
}

impl BackendState {
    /// Adopt the dashboard session token the managed backend serves. The
    /// backend pins `_SESSION_TOKEN` to the `HERMES_DASHBOARD_SESSION_TOKEN`
    /// spawn env (see `gateway.rs`), so this matches what `/api/ws` accepts.
    pub fn set_token(&self, token: String) {
        if let Ok(mut guard) = self.token.lock() {
            *guard = if token.is_empty() { None } else { Some(token) };
        }
    }

    /// Record the announced base URL + auth token and wake any `get_connection`
    /// caller blocked in [`BackendState::wait_ready`]. Called by the gateway
    /// spawn thread on success; also called with the default target on
    /// failure/timeout so the renderer's boot resolves instead of hanging.
    pub fn mark_ready(&self, url: String, token: String) {
        if let Ok(mut guard) = self.base_url.lock() {
            *guard = url;
        }
        self.set_token(token);
        let (ready_lock, cvar) = &*self.ready;
        if let Ok(mut ready) = ready_lock.lock() {
            *ready = true;
            cvar.notify_all();
        }
    }

    /// Block until [`BackendState::mark_ready`] (or `timeout` elapses), then
    /// return the current base URL (trimmed of a trailing slash).
    pub fn wait_ready(&self, timeout: Duration) -> String {
        let (ready_lock, cvar) = &*self.ready;
        let ready = ready_lock.lock().unwrap_or_else(|e| e.into_inner());
        if !*ready {
            let _ = cvar
                .wait_timeout(ready, timeout)
                .unwrap_or_else(|e| e.into_inner());
        }
        self.base_url
            .lock()
            .map(|g| g.trim_end_matches('/').to_string())
            .unwrap_or_else(|_| DEFAULT_BACKEND_BASE.to_string())
    }

    /// Whether the managed gateway has announced its port (or a failure path
    /// marked the state ready with the default target).
    pub fn is_ready(&self) -> bool {
        self.ready.0.lock().map(|g| *g).unwrap_or(false)
    }

    /// Current session token (empty when unset) — read access for sibling
    /// modules (connections.rs local-route probes).
    pub(crate) fn token_snapshot(&self) -> String {
        self.token
            .lock()
            .map(|g| g.clone().unwrap_or_default())
            .unwrap_or_default()
    }
}

impl Default for BackendState {
    fn default() -> Self {
        Self {
            base_url: Arc::new(Mutex::new(DEFAULT_BACKEND_BASE.to_string())),
            token: Arc::new(Mutex::new(
                std::env::var("HERMES_DASHBOARD_SESSION_TOKEN").ok(),
            )),
            ready: Arc::new((Mutex::new(false), Condvar::new())),
        }
    }
}

// ---------------------------------------------------------------------------
// Request / response DTOs (subset of global.d.ts)
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiRequest {
    path: String,
    #[serde(default)]
    method: Option<String>,
    #[serde(default)]
    body: Option<serde_json::Value>,
    /// Single-file multipart upload (mutually exclusive with `body`).
    #[serde(default)]
    upload: Option<UploadPayload>,
    #[serde(default)]
    timeout_ms: Option<u64>,
    /// Route to a specific profile's backend; ignored for the initial local
    /// increment (resolves to the shared backend).
    #[serde(default)]
    #[allow(dead_code)]
    profile: Option<String>,
    /// Route this REST call to a specific REGISTERED gateway connection (v2
    /// registry). Omit / '' / 'local' keeps the local primary backend path;
    /// a remote id resolves through the owning connection's base URL + auth
    /// (see connections.rs `api_target`).
    #[serde(default)]
    connection_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UploadPayload {
    filename: String,
    #[serde(default)]
    content_type: Option<String>,
    /// JS `Uint8Array` / `ArrayBuffer` → Rust `Vec<u8>` via Tauri invoke.
    bytes: Vec<u8>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionInfo {
    pub(crate) base_url: String,
    pub(crate) token: String,
    pub(crate) ws_url: String,
    mode: String,
    auth_mode: String,
    source: String,
    logs: Vec<String>,
    profile: Option<String>,
    is_fullscreen: bool,
    native_overlay_width: i32,
    window_button_position: Option<serde_json::Value>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionInfo {
    app_version: String,
    platform: String,
    hermes_root: String,
}

/// Main-process boot progress snapshot, matching the renderer's
/// `DesktopBootProgress` (src/global.d.ts). Emitted as `hermes:boot-progress`
/// events by `gateway.rs` and pulled via `get_boot_progress`.
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct BootProgress {
    error: Option<String>,
    fake_mode: bool,
    message: String,
    phase: String,
    progress: u8,
    running: bool,
    timestamp: i64,
}

impl BootProgress {
    pub fn starting() -> Self {
        Self {
            error: None,
            fake_mode: false,
            message: "Waiting to start Hermes backend".to_string(),
            phase: "backend.starting".to_string(),
            progress: 20,
            running: true,
            timestamp: now_ms(),
        }
    }

    pub fn installing() -> Self {
        Self {
            error: None,
            fake_mode: false,
            message: "Installing Hermes backend (first run)".to_string(),
            phase: "backend.installing".to_string(),
            progress: 30,
            running: true,
            timestamp: now_ms(),
        }
    }

    pub fn ready() -> Self {
        Self {
            error: None,
            fake_mode: false,
            message: "Backend ready".to_string(),
            phase: "backend.ready".to_string(),
            progress: 90,
            running: false,
            timestamp: now_ms(),
        }
    }

    pub fn error(message: impl Into<String>) -> Self {
        let message = message.into();
        Self {
            error: Some(message.clone()),
            fake_mode: false,
            message,
            phase: "backend.failed".to_string(),
            progress: 95,
            running: false,
            timestamp: now_ms(),
        }
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Derive the boot snapshot from the gateway state: `backend.starting` until
/// the managed backend announces its port, then `backend.ready` at 90% (the
/// renderer owns the last stretch of its own boot progress).
fn boot_progress_snapshot(state: &BackendState) -> BootProgress {
    if state.is_ready() {
        BootProgress::ready()
    } else {
        BootProgress::starting()
    }
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/// `hermes:api` — generic REST proxy to the connected backend.
#[tauri::command]
pub async fn api(
    app: tauri::AppHandle,
    state: tauri::State<'_, BackendState>,
    request: ApiRequest,
) -> Result<serde_json::Value, String> {
    let client = reqwest::Client::new();

    // v2 registry routing: a connection-scoped request (cron jobs and their
    // run sessions live in the OWNING gateway's state.db) resolves through
    // the registered connection instead of the local primary.
    let mut remote_headers: Option<std::collections::HashMap<String, String>> = None;
    let (base, token) = match request
        .connection_id
        .as_deref()
        .map(str::trim)
        .filter(|c| !c.is_empty() && *c != "local")
    {
        Some(connection_id) => {
            let registry = app.state::<crate::connections::RegistryState>();
            match crate::connections::api_target(&app, &registry, connection_id)? {
                Some((base, token, headers)) => {
                    let mut flat = std::collections::HashMap::new();
                    if let Some(headers) = &headers {
                        for (name, value) in headers {
                            if let Some(value) = crate::connections::usable_header_value_pub(value) {
                                flat.insert(name.clone(), value);
                            }
                        }
                    }
                    remote_headers = (!flat.is_empty()).then_some(flat);
                    (base, token)
                }
                // api_target only returns None for local — handled above.
                None => (
                    state
                        .base_url
                        .lock()
                        .map(|g| g.trim_end_matches('/').to_string())
                        .map_err(|e| format!("backend state lock: {e}"))?,
                    state.token_snapshot(),
                ),
            }
        }
        None => (
            state
                .base_url
                .lock()
                .map(|g| g.trim_end_matches('/').to_string())
                .map_err(|e| format!("backend state lock: {e}"))?,
            state.token_snapshot(),
        ),
    };
    let url = format!("{base}/{}", request.path.trim_start_matches('/'));

    let timeout_ms = request.timeout_ms.unwrap_or(DEFAULT_FETCH_TIMEOUT_MS);
    let method = request
        .method
        .clone()
        .unwrap_or_else(|| "GET".to_string())
        .to_uppercase();

    let mut builder = match method.as_str() {
        "POST" => client.post(&url),
        "PUT" => client.put(&url),
        "PATCH" => client.patch(&url),
        "DELETE" => client.delete(&url),
        _ => client.get(&url),
    }
    .timeout(std::time::Duration::from_millis(timeout_ms));

    // Token-mode auth header (oauth/cookie support lands in a later increment).
    if !token.is_empty() {
        builder = builder.header("X-Hermes-Session-Token", &token);
    }
    if let Some(headers) = &remote_headers {
        for (name, value) in headers {
            builder = builder.header(name.as_str(), value);
        }
    }

    let response = if let Some(upload) = request.upload {
        let part = reqwest::multipart::Part::bytes(upload.bytes)
            .file_name(upload.filename)
            .mime_str(
                upload
                    .content_type
                    .as_deref()
                    .unwrap_or("application/octet-stream"),
            )
            .map_err(|e| e.to_string())?;
        let form = reqwest::multipart::Form::new().part("file", part);
        builder.multipart(form).send().await
    } else if let Some(body) = request.body {
        builder.json(&body).send().await
    } else {
        builder.send().await
    }
    .map_err(|e| e.to_string())?;

    let status = response.status();
    let bytes = response.bytes().await.map_err(|e| e.to_string())?;

    if !status.is_success() {
        return Err(format!(
            "backend {}: {}",
            status.as_u16(),
            String::from_utf8_lossy(&bytes)
        ));
    }

    serde_json::from_slice(&bytes).map_err(|_| {
        // Fall back to wrapping the raw text so the renderer can surface it.
        serde_json::json!({ "raw": String::from_utf8_lossy(&bytes) }).to_string()
    })
}

/// `hermes:version` — build metadata shown in Settings → About.
#[tauri::command]
pub fn get_version(app: tauri::AppHandle) -> Result<VersionInfo, String> {
    Ok(VersionInfo {
        app_version: app.package_info().version.to_string(),
        platform: std::env::consts::OS.to_string(),
        hermes_root: std::env::var("HERMES_HOME").unwrap_or_default(),
    })
}

/// `hermes:connection` — resolve the local backend descriptor. The initial
/// increment returns the default local `hermes serve` target; remote/cloud
/// resolution lands with the connection-config increment.
///
/// Blocks (up to [`BACKEND_READY_TIMEOUT_SECS`]) for the managed gateway to
/// announce its ephemeral port, mirroring Electron's `ensureBackend` so the
/// renderer's WebSocket dial targets a live backend instead of the default
/// `:8080` placeholder.
#[tauri::command]
pub fn get_connection(
    state: tauri::State<'_, BackendState>,
    profile: Option<String>,
) -> Result<ConnectionInfo, String> {
    let _ = state.wait_ready(Duration::from_secs(BACKEND_READY_TIMEOUT_SECS));
    build_connection(&state, profile)
}

/// `hermes:gatewayWsUrl` — non-blocking snapshot of the backend WS URL (the
/// boot flow already gates on `get_connection`; this feeds the renderer's WS
/// client mint without a second readiness wait).
#[tauri::command]
pub fn get_gateway_ws_url(
    state: tauri::State<'_, BackendState>,
    profile: Option<String>,
) -> Result<serde_json::Value, String> {
    let conn = build_connection(&state, profile)?;
    Ok(serde_json::json!({
        "ok": true,
        "wsUrl": conn.ws_url,
        "baseUrl": conn.base_url,
    }))
}

/// Shared ConnectionInfo builder (no readiness wait — callers decide whether
/// to block first). `pub(crate)` so connections.rs can reuse it for the
/// registry's local route.
pub(crate) fn build_connection(
    state: &BackendState,
    profile: Option<String>,
) -> Result<ConnectionInfo, String> {
    let base = state
        .base_url
        .lock()
        .map(|g| g.trim_end_matches('/').to_string())
        .map_err(|e| format!("backend state lock: {e}"))?;
    let token = state
        .token
        .lock()
        .map(|g| g.clone().unwrap_or_default())
        .map_err(|e| format!("backend state lock: {e}"))?;
    // hermes serve's WebSocket endpoint is `/api/ws`; in loopback mode it
    // authenticates via `?token=<session token>` (web_server.py
    // `_ws_auth_reason`). Electron's equivalent:
    // `ws://127.0.0.1:<port>/api/ws?token=<urlencoded token>`. The token is
    // urlsafe base64 ([A-Za-z0-9_-]) so no percent-encoding is required.
    let ws_base = to_ws_scheme(&base);
    let ws_url = if token.is_empty() {
        format!("{ws_base}/api/ws")
    } else {
        format!("{ws_base}/api/ws?token={token}")
    };
    Ok(ConnectionInfo {
        base_url: base,
        token,
        ws_url,
        mode: "local".to_string(),
        auth_mode: "token".to_string(),
        source: "local".to_string(),
        logs: Vec::new(),
        profile,
        is_fullscreen: false,
        native_overlay_width: 0,
        window_button_position: None,
    })
}

/// `hermes:bootProgress` — pull-snapshot of the main-process boot progress
/// (the renderer merges it on mount via `applyDesktopBootProgress`).
#[tauri::command]
pub fn get_boot_progress(state: tauri::State<'_, BackendState>) -> BootProgress {
    boot_progress_snapshot(&state)
}

/// `hermes:touchBackend` — keepalive: confirm the managed backend is up.
/// Mirrors Electron's backend liveness touch; the renderer calls this on a
/// keepalive timer and after profile switches.
#[tauri::command]
pub fn touch_backend(
    state: tauri::State<'_, BackendState>,
    profile: Option<String>,
) -> Result<bool, String> {
    let _ = profile;
    Ok(state.is_ready())
}

/// `hermes:revalidateConnection` — non-blocking connection snapshot used by
/// the renderer's reconnect path (never waits for readiness).
#[tauri::command]
pub fn revalidate_connection(
    state: tauri::State<'_, BackendState>,
    profile: Option<String>,
) -> Result<Option<ConnectionInfo>, String> {
    Ok(Some(build_connection(&state, profile)?))
}

/// `hermes:getRemoteDisplayReason` — Tauri builds never render on a remote
/// display; always null.
#[tauri::command]
pub fn get_remote_display_reason() -> Result<Option<String>, String> {
    Ok(None)
}

/// `hermes:setActiveWork` — report in-flight work to the main process (the
/// Electron quit-guard parity seam). No-op on Tauri; kept for bridge-signature
/// parity so the renderer's call sites stay identical.
#[tauri::command]
pub fn set_active_work(work: Option<serde_json::Value>) -> Result<(), String> {
    let _ = work;
    Ok(())
}

// ---------------------------------------------------------------------------
// Basic filesystem reads (mapped from `hermes:fs:*` / `hermes:readFile*`)
// ---------------------------------------------------------------------------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DirEntry {
    name: String,
    path: String,
    is_directory: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadDirResult {
    entries: Vec<DirEntry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

/// `hermes:fs:readDir`
#[tauri::command]
pub fn read_dir(dir_path: String) -> Result<ReadDirResult, String> {
    let dir = std::path::Path::new(&dir_path);
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Ok(ReadDirResult {
            entries: Vec::new(),
            error: Some(format!("cannot read directory: {dir_path}")),
        });
    };

    let mut entries = Vec::new();
    for entry in rd.flatten() {
        let ftype = entry.file_type().ok();
        entries.push(DirEntry {
            name: entry.file_name().to_string_lossy().into_owned(),
            path: entry.path().to_string_lossy().into_owned(),
            is_directory: ftype.map(|t| t.is_dir()).unwrap_or(false),
        });
    }
    entries.sort_by(|a, b| {
        b.is_directory
            .cmp(&a.is_directory)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(ReadDirResult {
        entries,
        error: None,
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadFileTextResult {
    path: String,
    text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    mime_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    language: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    byte_size: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    binary: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    truncated: Option<bool>,
}

/// `hermes:readFileText`
#[tauri::command]
pub fn read_file_text(file_path: String) -> Result<ReadFileTextResult, String> {
    let data = std::fs::read(&file_path).map_err(|e| e.to_string())?;
    let byte_size = data.len() as u64;
    let text = match String::from_utf8(data) {
        Ok(t) => t,
        Err(_) => {
            return Ok(ReadFileTextResult {
                path: file_path,
                text: String::new(),
                mime_type: Some("application/octet-stream".into()),
                language: None,
                byte_size: Some(byte_size),
                binary: Some(true),
                truncated: None,
            })
        }
    };
    Ok(ReadFileTextResult {
        path: file_path,
        text,
        mime_type: None,
        language: None,
        byte_size: Some(byte_size),
        binary: Some(false),
        truncated: None,
    })
}

/// `hermes:readFileDataUrl` — base64 data URL for previews / attachments.
#[tauri::command]
pub fn read_file_data_url(file_path: String) -> Result<String, String> {
    let data = std::fs::read(&file_path).map_err(|e| e.to_string())?;
    let mime = guess_mime(&file_path);
    Ok(format!("data:{mime};base64,{}", base64_encode(&data)))
}

/// `hermes:fs:writeText` — write a small UTF-8 text file (parent must exist).
#[tauri::command]
pub fn write_text_file(file_path: String, content: String) -> Result<serde_json::Value, String> {
    std::fs::write(&file_path, content).map_err(|e| e.to_string())?;
    Ok(serde_json::json!({ "path": file_path }))
}

/// `hermes:openExternal` — open a URL in the system default browser.
#[tauri::command]
pub fn open_external(url: String) -> Result<(), String> {
    open::that_detached(&url).map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn base64_encode(bytes: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// Convert an `http://`/`https://` base URL to its `ws://`/`wss://` form so
/// the renderer can hand it straight to `JsonRpcGatewayClient.connect()`.
/// `pub(crate)` for connections.rs (registry remote ws urls).
pub(crate) fn to_ws_scheme(base: &str) -> String {
    if base.starts_with("https://") {
        format!("wss://{}", &base["https://".len()..])
    } else if base.starts_with("http://") {
        format!("ws://{}", &base["http://".len()..])
    } else {
        base.to_string()
    }
}

fn guess_mime(path: &str) -> &'static str {
    let ext = PathBuf::from(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "pdf" => "application/pdf",
        "txt" | "md" | "log" => "text/plain",
        "json" => "application/json",
        "html" | "htm" => "text/html",
        "css" => "text/css",
        "js" | "mjs" | "ts" | "tsx" | "jsx" => "text/javascript",
        _ => "application/octet-stream",
    }
}
