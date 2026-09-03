//! Desktop misc commands (migration of the remaining Electron IPC handlers):
//! clipboard (arboard), file dialogs (tauri-plugin-dialog), fs ops, log
//! tailing, link-title fetch, plugin root, default project dir.
//!
//! Migration-ordering note: these were the tail of the 126-method map. Most
//! were one-shot utility calls with no shared state, so each lands as a plain
//! command. Window-only concepts (petOverlay/quickEntry/updates/uninstall/
//! theme marketplace/wakeIndicator/zoom) are intentionally NOT ported — they
//! are desktop-window features with no WebView2 equivalent and are surfaced as
//! safe no-op shapes in `desktop-bridge.ts`.

use std::path::{Path, PathBuf};

use serde::Serialize;
use tauri::{AppHandle, Manager};

fn hermes_home() -> PathBuf {
    std::env::var("HERMES_HOME")
        .map(PathBuf::from)
        .ok()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| {
            // Match Python `hermes_constants._get_platform_default_hermes_home()`:
            // %LOCALAPPDATA%\hermes on Windows, ~/.hermes elsewhere. The Rust
            // side must resolve the SAME default or reveal_logs / get_recent_logs
            // open an empty directory while the backend writes to the real one.
            if cfg!(windows) {
                let base = std::env::var("LOCALAPPDATA")
                    .map(PathBuf::from)
                    .unwrap_or_else(|_| {
                        std::env::var("USERPROFILE")
                            .map(|p| PathBuf::from(p).join("AppData").join("Local"))
                            .unwrap_or_default()
                    });
                base.join("hermes")
            } else {
                let base = std::env::var("HOME").unwrap_or_default();
                PathBuf::from(base).join(".hermes")
            }
        })
}

fn app_config_file(app: &AppHandle, name: &str) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_config_dir()
        .map_err(|e| format!("app config dir unavailable: {e}"))?;
    Ok(dir.join(name))
}

/// Read the last `n` lines of a file (best-effort, for the boot-failure log
/// panel and gateway-settings log tail).
fn read_tail(path: &Path, n: usize) -> Vec<String> {
    let Ok(content) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    content
        .lines()
        .rev()
        .take(n)
        .map(|s| s.to_string())
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect()
}

// ---------------------------------------------------------------------------
// Clipboard
// ---------------------------------------------------------------------------

/// `hermes:clipboard:read` — current clipboard text.
#[tauri::command]
pub fn read_clipboard() -> Result<String, String> {
    let mut cb = arboard::Clipboard::new().map_err(|e| format!("clipboard init: {e}"))?;
    cb.get_text().map_err(|e| format!("clipboard read: {e}"))
}

/// `hermes:clipboard:write` — replace clipboard text.
#[tauri::command]
pub fn write_clipboard(text: String) -> Result<(), String> {
    let mut cb = arboard::Clipboard::new().map_err(|e| format!("clipboard init: {e}"))?;
    cb.set_text(text)
        .map_err(|e| format!("clipboard write: {e}"))
}

// ---------------------------------------------------------------------------
// File dialogs (tauri-plugin-dialog)
// ---------------------------------------------------------------------------

/// `hermes:dialog:selectPaths` — file/dir picker. Returns the chosen paths
/// (empty on cancel), matching the `Promise<string[]>` bridge contract.
#[tauri::command]
pub fn select_paths(
    app: AppHandle,
    options: Option<SelectPathsOptions>,
) -> Result<Vec<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    let opts = options.unwrap_or_default();
    if opts.directories.unwrap_or(false) {
        let picked = match opts.multiple.unwrap_or(false) {
            true => app.dialog().file().blocking_pick_folders(),
            false => app.dialog().file().blocking_pick_folder().map(|p| vec![p]),
        };
        return Ok(picked
            .unwrap_or_default()
            .into_iter()
            .map(|f| f.to_string())
            .collect());
    }
    let mut builder = app.dialog().file();
    if let Some(dp) = opts.default_path.as_deref() {
        builder = builder.set_directory(dp);
    }
    if let Some(title) = opts.title.as_deref() {
        builder = builder.set_title(title);
    }
    for f in opts.filters.unwrap_or_default() {
        let exts: Vec<&str> = f.extensions.iter().map(|s| s.as_str()).collect();
        builder = builder.add_filter(f.name, &exts);
    }
    let picked = match opts.multiple.unwrap_or(false) {
        true => builder.blocking_pick_files(),
        false => builder.blocking_pick_file().map(|p| vec![p]),
    };
    Ok(picked
        .unwrap_or_default()
        .into_iter()
        .map(|f| f.to_string())
        .collect())
}

/// Options for [`select_paths`] (mirror of `HermesSelectPathsOptions`).
#[derive(serde::Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SelectPathsOptions {
    directories: Option<bool>,
    multiple: Option<bool>,
    default_path: Option<String>,
    title: Option<String>,
    filters: Option<Vec<SelectPathFilter>>,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectPathFilter {
    extensions: Vec<String>,
    name: String,
}

/// `hermes:dialog:selectSavePath` — save-file picker. Returns the chosen path
/// or `null` on cancel (`Promise<string | null>` bridge contract). Accepts the
/// renderer's save options (title / defaultPath / filters).
#[tauri::command]
pub fn select_save_path(
    app: AppHandle,
    options: Option<SelectPathsOptions>,
) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    let opts = options.unwrap_or_default();
    let mut builder = app.dialog().file();
    if let Some(dp) = opts.default_path.as_deref() {
        builder = builder.set_file_name(dp);
    }
    if let Some(title) = opts.title.as_deref() {
        builder = builder.set_title(title);
    }
    for f in opts.filters.unwrap_or_default() {
        let exts: Vec<&str> = f.extensions.iter().map(|s| s.as_str()).collect();
        builder = builder.add_filter(f.name, &exts);
    }
    Ok(builder.blocking_save_file().map(|p| p.to_string()))
}

// ---------------------------------------------------------------------------
// Filesystem ops
// ---------------------------------------------------------------------------

/// `hermes:fs:trash` — move a file/folder to the OS trash (recycle bin).
#[tauri::command]
pub fn trash_path(path: String) -> Result<(), String> {
    trash::delete(&path).map_err(|e| format!("trash: {e}"))
}

/// `hermes:fs:rename` — rename a file/folder, returning `{ path }` (the
/// bridge contract). Rejects unsafe names (path separators, `.`/`..`, absolute
/// paths) like Electron's guard.
#[tauri::command]
pub fn rename_path(path: String, name: String) -> Result<serde_json::Value, String> {
    if name.is_empty()
        || name.contains(['/', '\\', ':'])
        || name == "."
        || name == ".."
        || Path::new(&name).is_absolute()
    {
        return Err(format!("unsafe rename target: {name:?}"));
    }
    let src = PathBuf::from(&path);
    let parent = src
        .parent()
        .ok_or_else(|| "rename target has no parent directory".to_string())?;
    let dst = parent.join(&name);
    std::fs::rename(&src, &dst).map_err(|e| format!("rename: {e}"))?;
    Ok(serde_json::json!({ "path": dst.to_string_lossy().to_string() }))
}

/// `hermes:fs:openDir` — open a folder in the OS file manager.
#[tauri::command]
pub fn open_dir(path: String) -> Result<(), String> {
    open::that_detached(&path).map_err(|e| format!("open dir: {e}"))
}

/// `hermes:fs:reveal` — reveal a file in the OS file manager.
#[tauri::command]
pub fn reveal_path(path: String) -> Result<(), String> {
    open::that_detached(&path).map_err(|e| format!("reveal: {e}"))
}

/// `hermes:saveImageBuffer` — persist a composer image (paste / drop /
/// quick-screenshot) and return its path. Port of Electron's
/// `writeComposerImage`; the renderer sends the bytes base64-encoded (JSON
/// arrays of a multi-MB screenshot are needlessly fat). `dir` overrides the
/// default `<app_data>/composer-images/` target (project screenshot mode:
/// `<workspace>/.hermes/screenshots/`, resolved renderer-side).
#[tauri::command]
pub fn save_image_buffer(
    app: AppHandle,
    data_base64: String,
    ext: String,
    dir: Option<String>,
    name: Option<String>,
) -> Result<String, String> {
    use base64::Engine as _;

    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data_base64.as_bytes())
        .map_err(|e| format!("saveImageBuffer: bad base64: {e}"))?;
    if bytes.is_empty() {
        return Err("saveImageBuffer: empty image".to_string());
    }

    // Same sanitization as Electron: `.[a-z0-9]{1,5}` or fall back to .png.
    let raw = ext.trim().to_lowercase();
    let bare = raw.strip_prefix('.').unwrap_or(&raw);
    let safe_ext = if !bare.is_empty() && bare.len() <= 5 && bare.chars().all(|c| c.is_ascii_alphanumeric()) {
        format!(".{bare}")
    } else {
        ".png".to_string()
    };

    let dir = match dir {
        Some(d) if !d.trim().is_empty() => {
            let d = PathBuf::from(d);
            if !d.is_absolute() {
                return Err("saveImageBuffer: dir must be absolute".to_string());
            }
            d
        }
        _ => app
            .path()
            .app_data_dir()
            .map_err(|e| format!("app data dir unavailable: {e}"))?
            .join("composer-images"),
    };
    std::fs::create_dir_all(&dir).map_err(|e| format!("create composer-images dir: {e}"))?;

    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let suffix: u32 = rand::random();
    // Preferred file name from the caller (upstream feature: preserve the
    // original File name on attach). Sanitized to a bare file name — path
    // separators and traversal are stripped, extension forced to `safe_ext`.
    let base = name
        .as_deref()
        .map(|n| {
            n.rsplit(['/', '\\']).next().unwrap_or(n)
        })
        .map(|n| n.trim())
        .filter(|n| !n.is_empty() && *n != "." && *n != "..")
        .and_then(|n| Path::new(n).file_stem().map(|s| s.to_string_lossy().into_owned()))
        .filter(|s| !s.is_empty());
    let file_name = match base {
        Some(b) => format!("{b}_{millis}_{suffix:08x}{safe_ext}"),
        None => format!("composer_{millis}_{suffix:08x}{safe_ext}"),
    };
    let path = dir.join(file_name);
    std::fs::write(&path, &bytes).map_err(|e| format!("write composer image: {e}"))?;

    Ok(path.to_string_lossy().to_string())
}

/// `hermes:preview:openInBrowser` — open a preview URL in the default browser.
#[tauri::command]
pub fn open_preview_in_browser(url: String) -> Result<(), String> {
    open::that_detached(&url).map_err(|e| format!("open browser: {e}"))
}

// ---------------------------------------------------------------------------
// Logs
// ---------------------------------------------------------------------------

/// `hermes:logs:recent` — tail of the local agent log (for the boot-failure
/// overlay's error panel).
#[tauri::command]
pub fn get_recent_logs() -> Result<serde_json::Value, String> {
    let lines = read_tail(&hermes_home().join("logs").join("agent.log"), 40);
    Ok(serde_json::json!({ "lines": lines }))
}

/// `hermes:logs:reveal` — open the hermes log directory in the file manager.
#[tauri::command]
pub fn reveal_logs() -> Result<(), String> {
    let dir = hermes_home().join("logs");
    let _ = std::fs::create_dir_all(&dir);
    open::that_detached(&dir).map_err(|e| format!("reveal logs: {e}"))
}

// ---------------------------------------------------------------------------
// Misc
// ---------------------------------------------------------------------------

/// `hermes:pluginsRoot` — the desktop-scoped plugin directory (created on
/// demand). Mirrors Electron's userData/plugins; kept separate from the
/// backend's HERMES_HOME/plugins so a remote backend can't redirect it.
#[tauri::command]
pub fn desktop_plugins_root(app: AppHandle) -> Result<Option<String>, String> {
    let dir = app_config_file(&app, "plugins")?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("create plugins dir: {e}"))?;
    Ok(Some(dir.to_string_lossy().to_string()))
}

/// `hermes:fetchLinkTitle` — fetch `<title>` from a URL. Returns the bare
/// title string (empty on failure), matching the `Promise<string>` contract
/// (`external-link.tsx` calls `.replace()` on the value). http/https only, no
/// redirects, 8s timeout, 512 KiB body cap, loopback/private-host reject.
#[tauri::command]
pub async fn fetch_link_title(url: String) -> Result<String, String> {
    let parsed = url::Url::parse(&url).map_err(|_| "invalid link URL".to_string())?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(format!("unsupported link URL scheme: {}", parsed.scheme()));
    }
    if is_loopback_or_private_host(parsed.host_str().unwrap_or("")) {
        return Err("link URL points at a local/private host".to_string());
    }
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(8))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| e.to_string())?;
    let resp = client
        .get(&url)
        .header("User-Agent", "hermes-desktop")
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if let Some(len) = resp.content_length() {
        if len > 512 * 1024 {
            return Err("link page exceeded 512 KiB limit".to_string());
        }
    }
    let body = resp.bytes().await.map_err(|e| e.to_string())?;
    if body.len() > 512 * 1024 {
        return Err("link page exceeded 512 KiB limit".to_string());
    }
    let html = String::from_utf8_lossy(&body);
    let title = regex::Regex::new(r"(?is)<title[^>]*>(.*?)</title>")
        .map_err(|e| e.to_string())?
        .captures(&html)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().trim().to_string())
        .unwrap_or_default();
    Ok(title)
}

/// Reject loopback/private hosts (literal IPs — including IPv4-mapped IPv6 —
/// and `localhost`), matching the renderer's `external-link.tsx` guard so a
/// linked page can't be used as an SSRF probe against the local backend.
/// Also rejects Windows-style numeric host forms (`127.1`, `0x7f000001`)
/// that system resolvers fold back onto loopback.
pub(crate) fn is_loopback_or_private_host(host: &str) -> bool {
    let host = host
        .trim_start_matches('[')
        .trim_end_matches(']')
        .trim_end_matches('.');
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    // Pure-numeric hosts (e.g. `2130706433`) are parsed as IPv4 by reqwest;
    // reject them so the octet checks below stay meaningful.
    if !host.is_empty() && host.chars().all(|c| c.is_ascii_digit()) {
        return true;
    }
    // Windows numeric shorthand / hex forms (`127.1`, `0x7f000001`) don't
    // parse as IpAddr but system resolvers treat them as IPv4; try to fold
    // them onto a plain dotted quad before the parse check.
    if host.contains('.') || host.starts_with("0x") || host.starts_with("0X") {
        if let Some(v4) = fold_numeric_host(host) {
            return is_private_ipv4(&v4);
        }
    }
    let Ok(ip) = host.parse::<std::net::IpAddr>() else {
        return false; // hostname — the renderer already gates link targets
    };
    // Normalize IPv4-mapped IPv6 (`::ffff:127.0.0.1`) onto the IPv4 rules.
    match ip {
        std::net::IpAddr::V4(v4) => is_private_ipv4(&v4),
        std::net::IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_private_ipv4(&v4);
            }
            v6.is_loopback() || v6.is_unique_local() || v6.is_unicast_link_local()
        }
    }
}

/// Fold Windows-style numeric host forms onto an Ipv4Addr when possible,
/// using inet_aton semantics: `127.1` → 127.0.0.1, `127.0.1` → 127.0.0.1,
/// `0x7f000001` / `0177.0.0.1` octets, single 32-bit ints.
fn fold_numeric_host(host: &str) -> Option<std::net::Ipv4Addr> {
    let parse_part = |s: &str| -> Option<u32> {
        let s = s.trim();
        if s.is_empty() {
            return None;
        }
        if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
            u32::from_str_radix(hex, 16).ok()
        } else if s.len() > 1 && s.starts_with('0') && s.bytes().all(|b| b.is_ascii_digit()) {
            u32::from_str_radix(s, 8).ok()
        } else {
            s.parse::<u32>().ok()
        }
    };
    let parts: Vec<&str> = host.split('.').collect();
    if parts.is_empty() || parts.len() > 4 {
        return None;
    }
    let vals: Vec<u32> = parts.iter().map(|p| parse_part(p)).collect::<Option<_>>()?;
    match vals.len() {
        1 => std::net::Ipv4Addr::from(vals[0]).into(),
        2 => Some(std::net::Ipv4Addr::new(
            vals[0] as u8,
            (vals[1] >> 16) as u8,
            (vals[1] >> 8) as u8,
            vals[1] as u8,
        )),
        3 => Some(std::net::Ipv4Addr::new(
            vals[0] as u8,
            vals[1] as u8,
            (vals[2] >> 8) as u8,
            vals[2] as u8,
        )),
        4 => {
            if vals.iter().any(|&v| v > 255) {
                return None;
            }
            Some(std::net::Ipv4Addr::new(
                vals[0] as u8,
                vals[1] as u8,
                vals[2] as u8,
                vals[3] as u8,
            ))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folds_windows_numeric_host_forms_onto_loopback() {
        // inet_aton forms that Windows resolvers fold back onto 127/8.
        assert!(is_loopback_or_private_host("127.1"));
        assert!(is_loopback_or_private_host("127.0.1"));
        assert!(is_loopback_or_private_host("0x7f000001"));
        assert!(is_loopback_or_private_host("0177.0.0.1"));
        assert!(is_loopback_or_private_host("2130706433"));
        assert!(is_loopback_or_private_host("127.65535"));
        assert!(is_loopback_or_private_host("::ffff:127.0.0.1"));
    }

    #[test]
    fn rejects_loopback_and_private_hosts() {
        assert!(is_loopback_or_private_host("localhost"));
        assert!(is_loopback_or_private_host("127.0.0.1"));
        assert!(is_loopback_or_private_host("10.1.2.3"));
        assert!(is_loopback_or_private_host("192.168.1.1"));
        assert!(is_loopback_or_private_host("172.16.0.1"));
        assert!(is_loopback_or_private_host("169.254.0.1"));
        assert!(is_loopback_or_private_host("100.64.0.1"));
        assert!(is_loopback_or_private_host("0.0.0.0"));
    }

    #[test]
    fn allows_public_hosts() {
        assert!(!is_loopback_or_private_host("8.8.8.8"));
        assert!(!is_loopback_or_private_host("1.1.1.1"));
        assert!(!is_loopback_or_private_host("example.com"));
        assert!(!is_loopback_or_private_host("2606:4700::1111"));
    }

    #[test]
    fn fold_numeric_host_semantics() {
        assert_eq!(
            fold_numeric_host("127.1"),
            Some(std::net::Ipv4Addr::new(127, 0, 0, 1))
        );
        assert_eq!(
            fold_numeric_host("127.0.1"),
            Some(std::net::Ipv4Addr::new(127, 0, 0, 1))
        );
        assert_eq!(
            fold_numeric_host("0x7f000001"),
            Some(std::net::Ipv4Addr::new(127, 0, 0, 1))
        );
        assert_eq!(
            fold_numeric_host("127.65535"),
            Some(std::net::Ipv4Addr::new(127, 0, 255, 255))
        );
        assert_eq!(
            fold_numeric_host("8.8.8.8"),
            Some(std::net::Ipv4Addr::new(8, 8, 8, 8))
        );
        assert_eq!(fold_numeric_host("example.com"), None);
    }
}

fn is_private_ipv4(v4: &std::net::Ipv4Addr) -> bool {
    let o = v4.octets();
    v4.is_loopback()
        || o[0] == 10 // RFC1918 10/8
        || (o[0] == 172 && (16..=31).contains(&o[1])) // RFC1918 172.16/12
        || (o[0] == 192 && o[1] == 168) // RFC1918 192.168/16
        || (o[0] == 169 && o[1] == 254) // link-local 169.254/16
        || o[0] == 0
        || (o[0] == 100 && (64..=127).contains(&o[1])) // CGNAT 100.64/10
}

// ---------------------------------------------------------------------------
// Default project dir (persisted to app config; mirror of Electron store)
// ---------------------------------------------------------------------------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DefaultProjectDirResult {
    default_label: String,
    dir: Option<String>,
    resolved_cwd: String,
}

#[tauri::command]
pub fn settings_get_default_project_dir(app: AppHandle) -> Result<DefaultProjectDirResult, String> {
    let file = app_config_file(&app, "default-project-dir.txt")?;
    let dir = std::fs::read_to_string(&file)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let cwd = std::env::current_dir()
        .map(|d| d.to_string_lossy().to_string())
        .unwrap_or_default();
    Ok(DefaultProjectDirResult {
        default_label: dir
            .as_deref()
            .map(|_| "Custom".to_string())
            .unwrap_or_else(|| "Default".to_string()),
        dir,
        resolved_cwd: cwd,
    })
}

#[tauri::command]
pub fn settings_set_default_project_dir(
    app: AppHandle,
    dir: Option<String>,
) -> Result<serde_json::Value, String> {
    let file = app_config_file(&app, "default-project-dir.txt")?;
    match dir {
        Some(dir) if !dir.trim().is_empty() => {
            if let Some(parent) = file.parent() {
                std::fs::create_dir_all(parent).map_err(|e| format!("create config dir: {e}"))?;
            }
            std::fs::write(&file, dir.trim()).map_err(|e| format!("write default dir: {e}"))?;
            Ok(serde_json::json!({ "dir": dir.trim() }))
        }
        _ => {
            let _ = std::fs::remove_file(&file);
            Ok(serde_json::json!({ "dir": null }))
        }
    }
}

#[tauri::command]
pub fn settings_pick_default_project_dir(app: AppHandle) -> Result<serde_json::Value, String> {
    use tauri_plugin_dialog::DialogExt;
    let picked = app.dialog().file().blocking_pick_folder();
    match picked {
        Some(path) => Ok(serde_json::json!({ "canceled": false, "dir": path.to_string() })),
        None => Ok(serde_json::json!({ "canceled": true, "dir": null })),
    }
}
