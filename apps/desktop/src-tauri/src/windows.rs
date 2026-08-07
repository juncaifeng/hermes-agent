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
/// in production at the loopback asset server (see `asset_server.rs`) so every
/// window shares the main window's loopback origin.
fn build_window_url(app: &AppHandle, session_id: Option<&str>, watch: bool) -> String {
    let query = match (session_id.is_some(), watch) {
        (true, true) => "?win=secondary&watch=1",
        (true, false) => "?win=secondary",
        _ => "",
    };
    let route = session_id
        .map(|s| format!("#/{}", encode_route_segment(s)))
        .unwrap_or_default();

    // Dev/prod origin selection lives in `asset_server::base_url` (devUrl in
    // dev, loopback HTTP in production, asset protocol as fallback) so the
    // main window and all pop-outs stay on one origin.
    let base = crate::asset_server::base_url(app);
    let base = base.trim_end_matches('/').to_string();
    format!("{base}/{query}{route}")
}

/// `hermes:openSessionWindow` — open (or focus) a secondary window for one chat.
///
/// MUST be an async command: sync commands run on the main thread, and
/// building a second webview while the main thread sits inside a WebView2
/// IPC handler deadlocks (the new window is stuck on about:blank and its
/// navigator never initializes). The async handler runs on a worker thread
/// and posts creation back to the main thread — the same conditions the
/// main window was created under.
#[tauri::command]
pub async fn open_session_window(
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
    let existing = app.get_webview_window(&label);
    if let Some(win) = existing {
        let _ = win.show();
        let _ = win.set_focus();
        return Ok(label);
    }

    let watch = opts.map(|o| o.watch).unwrap_or(false);
    let url = build_window_url(&app, Some(&key), watch);

    let label_for_build = label.clone();
    let app_for_build = app.clone();
    app.run_on_main_thread(move || {
        let parsed = match url.parse() {
            Ok(parsed) => parsed,
            Err(err) => {
                eprintln!("[windows] bad session window url {url:?}: {err}");
                return;
            }
        };
        let win_label = label_for_build.clone();
        let result = WebviewWindowBuilder::new(&app_for_build, &label_for_build, WebviewUrl::External(parsed))
            .title("Hermes")
            .inner_size(SESSION_WINDOW_MIN_WIDTH, SESSION_WINDOW_MIN_HEIGHT)
            .min_inner_size(SESSION_WINDOW_MIN_WIDTH, SESSION_WINDOW_MIN_HEIGHT)
            .resizable(true)
            .build();
        if let Err(err) = result {
            eprintln!("[windows] failed to build session window {win_label}: {err}");
        }
    })
    .map_err(|e| e.to_string())?;

    Ok(label)
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionWindowOptions {
    #[serde(default)]
    watch: bool,
}

/// `hermes:openWindow` — open a full app "instance" window (⌘⇧N / New Window).
/// Async for the same re-entrancy reason as `open_session_window`.
#[tauri::command]
pub async fn open_window(app: AppHandle) -> Result<String, String> {
    let label = format!("instance-{}", instance_counter());
    let url = build_window_url(&app, None, false);

    // Same re-entrancy rule as open_session_window: never build a webview
    // from inside an IPC handler (main-thread deadlock, about:blank window).
    let label_for_build = label.clone();
    let app_for_build = app.clone();
    app.run_on_main_thread(move || {
        let parsed = match url.parse() {
            Ok(parsed) => parsed,
            Err(err) => {
                eprintln!("[windows] bad instance window url {url:?}: {err}");
                return;
            }
        };
        let win_label = label_for_build.clone();
        let result = WebviewWindowBuilder::new(&app_for_build, &label_for_build, WebviewUrl::External(parsed))
            .title("Hermes")
            .inner_size(INSTANCE_WINDOW_WIDTH, INSTANCE_WINDOW_HEIGHT)
            .resizable(true)
            .build();
        if let Err(err) = result {
            eprintln!("[windows] failed to build instance window {win_label}: {err}");
        }
    })
    .map_err(|e| e.to_string())?;

    Ok(label)
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
