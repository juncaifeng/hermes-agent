//! Hermes Desktop — window management (multi-window / session pop-outs).
//!
//! Ports the Electron secondary "session windows" (one extra OS window per chat)
//! and full "instance" windows (⌘⇧N / New Window). The renderer reads the
//! `?win=secondary` query flag (before the `#`) to suppress install/onboarding
//! overlays and the global session sidebar; the `#/{sessionId}` hash route is
//! consumed by the HashRouter. `watch=1` marks a spectator window.
//!
//! See `apps/desktop/electron/session-windows.ts` for the original logic.

use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

const SESSION_WINDOW_MIN_WIDTH: f64 = 420.0;
const SESSION_WINDOW_MIN_HEIGHT: f64 = 620.0;
const INSTANCE_WINDOW_WIDTH: f64 = 1200.0;
const INSTANCE_WINDOW_HEIGHT: f64 = 800.0;

/// Percent-encode a path segment (used for the `#/{sessionId}` hash route).
fn encode_route_segment(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for b in raw.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Build the renderer URL for a window. In dev we point at the Vite dev server;
/// in production we load the bundled asset protocol origin (`http://tauri.localhost`).
fn build_window_url(app: &AppHandle, session_id: Option<&str>, watch: bool) -> String {
    let query = match (session_id.is_some(), watch) {
        (true, true) => "?win=secondary&watch=1",
        (true, false) => "?win=secondary",
        _ => "",
    };
    let route = session_id
        .map(|s| format!("#/{}", encode_route_segment(s)))
        .unwrap_or_default();

    // `dev_url` is always present in tauri.conf.json, so gate on the build
    // profile (mirroring `generate_context!`'s dev flag) instead of the config
    // value: a production build that consults `dev_url` would load
    // `http://localhost:5174` for every session/instance window and die when no
    // dev server is running. Production always uses the embedded origin.
    let base = if cfg!(debug_assertions) {
        app.config()
            .build
            .dev_url
            .clone()
            .map(|u| u.to_string())
            .unwrap_or_else(|| "http://tauri.localhost".to_string())
    } else {
        "http://tauri.localhost".to_string()
    };
    let base = base.trim_end_matches('/').to_string();
    format!("{base}/{query}{route}")
}

/// `hermes:openSessionWindow` — open (or focus) a secondary window for one chat.
#[tauri::command]
pub fn open_session_window(
    app: AppHandle,
    session_id: String,
    opts: Option<SessionWindowOptions>,
) -> Result<String, String> {
    let key = session_id.trim().to_string();
    if key.is_empty() {
        return Err("sessionId is required.".into());
    }

    let label = format!("session-{}", slug_label(&key));
    // Focus-or-create: never duplicate a window for the same chat.
    if let Some(win) = app.get_webview_window(&label) {
        let _ = win.show();
        let _ = win.set_focus();
        return Ok(label);
    }

    let watch = opts.map(|o| o.watch).unwrap_or(false);
    let url = WebviewUrl::External(build_window_url(&app, Some(&key), watch).parse().unwrap());

    let win = WebviewWindowBuilder::new(&app, &label, url)
        .title("Hermes")
        .inner_size(SESSION_WINDOW_MIN_WIDTH, SESSION_WINDOW_MIN_HEIGHT)
        .min_inner_size(SESSION_WINDOW_MIN_WIDTH, SESSION_WINDOW_MIN_HEIGHT)
        .resizable(true)
        .build()
        .map_err(|e| e.to_string())?;

    Ok(win.label().to_string())
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionWindowOptions {
    #[serde(default)]
    watch: bool,
}

/// `hermes:openWindow` — open a full app "instance" window (⌘⇧N / New Window).
#[tauri::command]
pub fn open_window(app: AppHandle) -> Result<String, String> {
    let label = format!("instance-{}", instance_counter());
    let url = WebviewUrl::External(build_window_url(&app, None, false).parse().unwrap());

    let win = WebviewWindowBuilder::new(&app, &label, url)
        .title("Hermes")
        .inner_size(INSTANCE_WINDOW_WIDTH, INSTANCE_WINDOW_HEIGHT)
        .resizable(true)
        .build()
        .map_err(|e| e.to_string())?;

    Ok(win.label().to_string())
}

/// Short, filesystem-safe label from an arbitrary session id.
fn slug_label(session_id: &str) -> String {
    let mut slug: String = session_id
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect();
    while slug.starts_with('-') {
        slug.remove(0);
    }
    while slug.ends_with('-') {
        slug.pop();
    }
    slug.truncate(48);
    if slug.is_empty() {
        "win".into()
    } else {
        slug
    }
}

/// Monotonic counter for instance-window labels (per process).
fn instance_counter() -> usize {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    COUNTER.fetch_add(1, Ordering::Relaxed) + 1
}
