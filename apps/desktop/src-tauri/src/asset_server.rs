//! Loopback static-asset server (Hermes Desktop).
//!
//! In production the renderer is served over plain HTTP on 127.0.0.1 instead
//! of Tauri's `tauri://localhost` custom protocol. WebView2 then stamps every
//! WebSocket upgrade with `Origin: http://127.0.0.1:<port>` — a loopback name
//! `hermes serve` has always whitelisted — instead of
//! `http://tauri.localhost`, which backends without the tauri.localhost patch
//! reject (403 origin_mismatch). This lets the packaged app talk to any stock
//! backend (uv-tool installs, older runtimes), not only patched ones.
//!
//! Side effect: the page is no longer treated as a secure remote origin, so
//! WebView2's Mixed Content Autoupgrade stops rewriting `ws://127.0.0.1` into
//! `wss://` — the (currently unused) TLS proxy in `tls_proxy.rs` stays
//! unnecessary on the local-backend path.
//!
//! The server serves ONLY the bundled renderer (dist/) on a random loopback
//! port: no secrets, no API surface, no writes. Any local process could fetch
//! the UI assets, which are public by definition (shipped in the installer).

use std::fs::File;
use std::path::PathBuf;
use std::sync::Mutex;

use tauri::{AppHandle, Manager};

/// Managed state holding the renderer base URL (`http://127.0.0.1:<port>`)
/// once the loopback server has started. Window code reads it so secondary
/// and instance windows land on the same origin as the main window.
#[derive(Default)]
pub struct AssetBase {
    base: Mutex<Option<String>>,
}

impl AssetBase {
    pub fn set(&self, url: String) {
        if let Ok(mut guard) = self.base.lock() {
            *guard = Some(url);
        }
    }

    pub fn get(&self) -> Option<String> {
        self.base.lock().ok().and_then(|guard| guard.clone())
    }
}

/// The base URL every renderer window should load.
///
/// Dev builds keep pointing at the Vite dev server. Production prefers the
/// loopback asset server and falls back to the asset-protocol origin when the
/// server could not start (dist missing / bind failed) — the window still
/// loads, and the renderer surfaces its own boot error if an unpatched
/// backend rejects the tauri origin.
pub fn base_url(app: &AppHandle) -> String {
    if cfg!(debug_assertions) {
        app.config()
            .build
            .dev_url
            .clone()
            .map(|u| u.to_string())
            .unwrap_or_else(|| "http://localhost:5174".to_string())
    } else {
        app.try_state::<AssetBase>()
            .and_then(|state| state.get())
            .unwrap_or_else(|| "http://tauri.localhost".to_string())
    }
}

/// Locate the renderer bundle (dist/) shipped alongside the app.
///
/// The MSI lays it out as `<install dir>/dist/` via `bundle.resources`, and
/// `resource_dir()` resolves to that install dir on Windows. For a bare
/// `cargo build` exe the same lookup falls through to the exe directory, so
/// copying `dist/` next to the exe reproduces the installed layout.
pub fn resolve_dist_dir(app: &AppHandle) -> Option<PathBuf> {
    if let Ok(resource_dir) = app.path().resource_dir() {
        let candidate = resource_dir.join("dist");
        if candidate.is_dir() {
            return Some(candidate);
        }
    }
    let exe_dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    let candidate = exe_dir.join("dist");
    candidate.is_dir().then_some(candidate)
}

/// Bind a static-file server for `dist_dir` on a random loopback port and
/// return the bound port. The accept loop runs on a detached thread for the
/// lifetime of the process; each connection is served on its own thread so a
/// slow client cannot stall asset delivery during page load.
pub fn start(dist_dir: PathBuf) -> Result<u16, String> {
    let server =
        tiny_http::Server::http(("127.0.0.1", 0)).map_err(|e| format!("asset server bind: {e}"))?;
    let port = match server.server_addr() {
        tiny_http::ListenAddr::IP(addr) => addr.port(),
        #[cfg(unix)]
        other => return Err(format!("unexpected asset server addr: {other:?}")),
    };
    eprintln!(
        "[assets] serving {} on http://127.0.0.1:{port}",
        dist_dir.display()
    );
    std::thread::spawn(move || {
        for request in server.incoming_requests() {
            let root = dist_dir.clone();
            std::thread::spawn(move || handle_request(root, request));
        }
    });
    Ok(port)
}

fn handle_request(root: PathBuf, request: tiny_http::Request) {
    let method = request.method().clone();
    if method != tiny_http::Method::Get && method != tiny_http::Method::Head {
        let _ = request.respond(tiny_http::Response::empty(405));
        return;
    }

    let raw_url = request.url().to_string();
    let path_part = raw_url.split(['?', '#']).next().unwrap_or("/");
    let Some(rel) = sanitize_path(path_part) else {
        let _ = request.respond(tiny_http::Response::empty(403));
        return;
    };

    let mut path = root.join(&rel);
    if path.is_dir() {
        path.push("index.html");
    }
    if !path.is_file() && rel.extension().is_none() {
        // SPA fallback: client-side routes have no file on disk.
        path = root.join("index.html");
    }
    if !path.is_file() {
        let _ = request.respond(tiny_http::Response::empty(404));
        return;
    }

    let mime = mime_guess::from_path(&path).first_or_octet_stream();
    let content_type = content_type_with_charset(mime);

    if method == tiny_http::Method::Head {
        let mut response = tiny_http::Response::empty(200);
        response.add_header(header("Content-Type", &content_type));
        let _ = request.respond(response);
        return;
    }

    match File::open(&path) {
        Ok(file) => {
            let mut response = tiny_http::Response::from_file(file);
            response.add_header(header("Content-Type", &content_type));
            let _ = request.respond(response);
        }
        Err(_) => {
            let _ = request.respond(tiny_http::Response::empty(404));
        }
    }
}

fn header(name: &str, value: &str) -> tiny_http::Header {
    // Header names/values here are static ASCII plus mime strings — never
    // user input — so construction cannot fail.
    tiny_http::Header::from_bytes(name.as_bytes(), value.as_bytes())
        .expect("static header must be valid")
}

fn content_type_with_charset(mime: mime_guess::Mime) -> String {
    let base = mime.to_string();
    let needs_charset = mime.type_() == mime_guess::mime::TEXT
        || base == "application/javascript"
        || base == "application/json"
        || base == "image/svg+xml";
    if needs_charset {
        format!("{base}; charset=utf-8")
    } else {
        base
    }
}

/// Map a raw URL path to a safe relative path under the dist root, or `None`
/// for traversal attempts. Percent-decoding happens before the segment walk
/// so `%2e%2e` and encoded separators cannot smuggle an escape.
fn sanitize_path(raw: &str) -> Option<PathBuf> {
    let decoded = percent_decode(raw)?;
    let mut rel = PathBuf::new();
    for segment in decoded.split('/') {
        match segment {
            "" | "." => continue,
            ".." => return None,
            s if s.contains('\\') || s.contains('\0') => return None,
            s => rel.push(s),
        }
    }
    Some(rel)
}

fn percent_decode(input: &str) -> Option<String> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = input.get(i + 1..i + 3)?;
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
