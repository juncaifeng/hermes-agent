//! Connection-config persistence + probing (migration of Electron
//! `connection-config.ts` / `main.ts` `hermes:connection-config:*` handlers).
//!
//! Scope of this increment (migration ordering 1):
//!   - `get_connection_config` / `save_connection_config` — persist the
//!     desktop's connection choice to `{app_config_dir}/connection.json`
//!     (per-profile remote overrides included).
//!   - `probe_connection_config` — public `/api/status` probe of a remote
//!     gateway URL to decide OAuth vs session-token auth (no credentials sent).
//!   - `apply_connection_config` — persist + emit `hermes:connection:applied`
//!     so the renderer re-homes its connection. The full Electron backend
//!     re-home state machine (rehomePrimaryConnection / ssh bootstrap) is NOT
//!     ported here; the renderer owns the reconnect once notified.
//!   - `test_connection_config` — reachability probe (same as probe for the
//!     remote path; SSH mode returns an explicit "not supported yet" error).
//!
//! Not ported in this increment: OAuth login/logout flows (RFC 8252 +
//! keychain), cloud discovery. Those land with migration ordering 9.

use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};

// ---------------------------------------------------------------------------
// Persisted file shape (mirrors Electron readDesktopConnectionConfig)
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct RemoteBlock {
    /// The connection mode this block describes ("local"/"remote"/"cloud"/"ssh").
    #[serde(default)]
    pub(crate) mode: String,
    #[serde(default)]
    pub(crate) url: String,
    /// "oauth" | "token" (default token for backward compat).
    #[serde(default)]
    pub(crate) auth_mode: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) token: Option<String>,
    /// Cloud org slug for `cloud` mode connections.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub(crate) org: String,
}

#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionConfigFile {
    /// "local" | "remote" | "cloud" | "ssh"
    #[serde(default)]
    pub(crate) mode: String,
    #[serde(default)]
    pub(crate) remote: RemoteBlock,
    /// Per-profile remote overrides (profile key → RemoteBlock).
    #[serde(default)]
    pub(crate) profiles: HashMap<String, RemoteBlock>,
}

/// `pub(crate)` read access for connections.rs (v1→v2 registry migration and
/// drift reconciliation read the v1 config).
pub(crate) fn read_connection_config_pub(app: &AppHandle) -> ConnectionConfigFile {
    read_connection_config(app)
}

/// `pub(crate)` URL normalization for connections.rs.
pub(crate) fn normalize_remote_base_url_pub(raw: &str) -> String {
    normalize_remote_base_url(raw)
}

// ---------------------------------------------------------------------------
// Renderer-facing shapes (global.d.ts)
// ---------------------------------------------------------------------------

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct DesktopConnectionConfig {
    env_override: bool,
    mode: String,
    profile: Option<String>,
    remote_auth_mode: String,
    remote_oauth_connected: bool,
    remote_token_preview: Option<String>,
    remote_token_set: bool,
    remote_url: String,
    cloud_org: String,
    ssh_host: String,
    ssh_user: String,
    ssh_port: Option<i32>,
    ssh_key_path: String,
    ssh_remote_hermes_path: String,
    ssh_remote_profile: String,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct DesktopConnectionProbeResult {
    base_url: String,
    reachable: bool,
    auth_mode: String,
    providers: Vec<serde_json::Value>,
    version: Option<String>,
    error: Option<String>,
}

// ---------------------------------------------------------------------------
// File helpers
// ---------------------------------------------------------------------------

fn connection_config_path(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_config_dir()
        .map_err(|e| format!("app config dir unavailable: {e}"))?;
    Ok(dir.join("connection.json"))
}

fn read_connection_config(app: &AppHandle) -> ConnectionConfigFile {
    let path = connection_config_path(app).unwrap_or_default();
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|raw| serde_json::from_str::<ConnectionConfigFile>(&raw).ok())
        .unwrap_or_default()
}

fn write_connection_config(app: &AppHandle, config: &ConnectionConfigFile) -> Result<(), String> {
    let path = connection_config_path(app)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("create config dir: {e}"))?;
    }
    let raw = serde_json::to_string_pretty(config).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        // The file carries a session token: create with owner-rw only (the
        // Electron original stores this encrypted; plaintext here is a staged
        // migration gap tracked as a follow-up, but at minimum don't inherit a
        // permissive umask). Atomic mode at create avoids a write-then-chmod
        // window; propagate errors instead of swallowing them.
        use std::os::unix::fs::OpenOptionsExt;
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true).mode(0o600);
        use std::io::Write;
        let mut f = opts
            .open(&path)
            .map_err(|e| format!("open connection.json: {e}"))?;
        f.write_all(raw.as_bytes())
            .map_err(|e| format!("write connection.json: {e}"))?;
        return Ok(());
    }
    #[cfg(not(unix))]
    std::fs::write(&path, raw).map_err(|e| format!("write connection.json: {e}"))?;
    Ok(())
}

/// Normalize a remote base URL, mirroring Electron `normalizeRemoteBaseUrl`
/// (connection-config.ts): prefix `http://` when the value has no `scheme://`,
/// keep scheme/host/port/path, trim trailing slashes, drop query/fragment.
/// The pathname is preserved — a gateway deployed at `/hermes` must keep that
/// prefix or reconnect/probe hit the wrong endpoint.
fn normalize_remote_base_url(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    // "localhost:9119" / tailnet IPs would otherwise parse `host:` as the
    // scheme; only a real `scheme://` prefix opts out of the http:// default.
    let candidate = if trimmed.contains("://") {
        trimmed.to_string()
    } else {
        format!("http://{trimmed}")
    };
    match url::Url::parse(&candidate) {
        Ok(parsed) => {
            // `host_str()` drops the brackets on IPv6 literals (`::1`), which
            // would produce an invalid `http://::1:9119`; re-add them.
            let host = match parsed.host() {
                Some(url::Host::Ipv6(addr)) => format!("[{addr}]"),
                Some(host) => host.to_string(),
                None => String::new(),
            };
            let mut base = format!("{}://{host}", parsed.scheme());
            if let Some(port) = parsed.port() {
                base = format!("{base}:{port}");
            }
            let path = parsed.path().trim_end_matches('/');
            if !path.is_empty() {
                base = format!("{base}{path}");
            }
            base
        }
        Err(_) => trimmed.to_string(),
    }
}

fn token_preview(token: &str) -> Option<String> {
    if token.is_empty() {
        return None;
    }
    let shown = token.chars().take(8).collect::<String>();
    Some(format!("{shown}…"))
}

/// Best-effort sanitize into the renderer-facing shape (simplified vs the
/// Electron original: no oauth-session liveness check, no ssh normalization).
fn sanitize(config: &ConnectionConfigFile, profile: Option<String>) -> DesktopConnectionConfig {
    let key = profile.as_deref().unwrap_or("");
    let scoped = if key.is_empty() {
        None
    } else {
        config.profiles.get(key)
    };
    let block = scoped.unwrap_or(&config.remote);

    let saved_mode = scoped
        .map(|b| b.mode.as_str())
        .unwrap_or(config.mode.as_str());
    let mode = match saved_mode {
        "ssh" => "ssh",
        "remote" | "cloud" => saved_mode,
        _ => "local",
    };
    let token = block.token.as_deref().unwrap_or("");
    let auth_mode = if block.auth_mode == "oauth" {
        "oauth"
    } else {
        "token"
    };
    let remote_url = block.url.trim_end_matches('/').to_string();

    DesktopConnectionConfig {
        env_override: false,
        mode: mode.to_string(),
        profile,
        remote_auth_mode: auth_mode.to_string(),
        remote_oauth_connected: false,
        remote_token_preview: token_preview(token),
        remote_token_set: !token.is_empty(),
        remote_url,
        cloud_org: if mode == "cloud" {
            block.org.clone()
        } else {
            String::new()
        },
        ssh_host: String::new(),
        ssh_user: String::new(),
        ssh_port: None,
        ssh_key_path: String::new(),
        ssh_remote_hermes_path: String::new(),
        ssh_remote_profile: String::new(),
    }
}

/// Coerce an incoming save/apply payload into the persisted shape. Simplified
/// vs Electron coerceDesktopConnectionConfig: cloud→other clears the block;
/// authMode defaults to token; remoteUrl normalized.
fn coerce(payload: &serde_json::Value, existing: &ConnectionConfigFile) -> ConnectionConfigFile {
    let mut next = existing.clone();
    let mode_raw = payload
        .get("mode")
        .and_then(|m| m.as_str())
        .unwrap_or("local");
    let mode = match mode_raw {
        "ssh" => "ssh",
        "remote" | "cloud" => mode_raw,
        _ => "local",
    };
    let profile = payload
        .get("profile")
        .and_then(|p| p.as_str())
        .filter(|p| !p.is_empty());

    // The block being edited: a per-profile entry or the global remote block.
    let key = profile.unwrap_or("");
    let existing_block = if key.is_empty() {
        next.remote.clone()
    } else {
        next.profiles.get(key).cloned().unwrap_or_default()
    };
    // Leaving a cloud connection unselects it: don't inherit its url/org/token.
    let existing_mode = if key.is_empty() {
        next.mode.as_str()
    } else {
        next.profiles
            .get(key)
            .map(|b| b.mode.as_str())
            .unwrap_or("")
    };
    let leaving_cloud = existing_mode == "cloud" && mode != "cloud";
    let base_block = if leaving_cloud {
        RemoteBlock::default()
    } else {
        existing_block
    };

    let remote_url = normalize_remote_base_url(
        payload
            .get("remoteUrl")
            .and_then(|u| u.as_str())
            .unwrap_or(&base_block.url),
    );
    let auth_mode = payload
        .get("remoteAuthMode")
        .and_then(|a| a.as_str())
        .filter(|a| *a == "oauth" || *a == "token")
        .unwrap_or(if base_block.auth_mode == "oauth" {
            "oauth"
        } else {
            "token"
        });
    let cloud_org = if mode == "cloud" {
        payload
            .get("cloudOrg")
            .and_then(|o| o.as_str())
            .unwrap_or(&base_block.org)
            .to_string()
    } else {
        String::new()
    };
    let incoming_token = payload
        .get("remoteToken")
        .and_then(|t| t.as_str())
        .map(|t| t.trim().to_string())
        .unwrap_or_default();
    // Empty input token means "keep the saved one" (the renderer never sends
    // back the full token; it sends remoteToken only when the user typed one).
    let token = if incoming_token.is_empty() {
        base_block.token.clone()
    } else {
        Some(incoming_token)
    };

    let block = RemoteBlock {
        mode: mode.to_string(),
        url: remote_url,
        auth_mode: auth_mode.to_string(),
        token,
        org: cloud_org,
    };

    if key.is_empty() {
        next.mode = mode.to_string();
        next.remote = block;
    } else {
        next.profiles.insert(key.to_string(), block);
    }
    next
}

// ---------------------------------------------------------------------------
// Probe
// ---------------------------------------------------------------------------

async fn fetch_json_timeout(url: &str, timeout_ms: u64) -> Result<serde_json::Value, String> {
    // Explicit http/https scheme allow-list (reqwest's feature set already
    // restricts schemes, but be explicit so a future feature change cannot
    // silently widen the probe surface).
    let parsed = url::Url::parse(url).map_err(|_| "invalid probe URL".to_string())?;
    let scheme = parsed.scheme().to_string();
    if scheme != "http" && scheme != "https" {
        return Err(format!("unsupported probe URL scheme: {scheme}"));
    }
    // Reject loopback/private hosts so a probe cannot be pointed at the local
    // backend or other internal services (same guard as fetch_link_title).
    if crate::desktop_misc::is_loopback_or_private_host(parsed.host_str().unwrap_or("")) {
        return Err("probe URL points at a local/private host".to_string());
    }
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_millis(timeout_ms))
        // Never follow redirects: a public URL must not bounce the probe onto
        // loopback/link-local during a reachability check (SSRF tightening;
        // a gateway's /api/status never needs a redirect).
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| e.to_string())?;
    let resp = client
        .get(url)
        .header("User-Agent", "hermes-desktop")
        .send()
        .await
        .map_err(|e| e.to_string())?;
    // Cap the response body at 256 KiB: reject up-front when Content-Length
    // advertises more, and verify again after download (the 8s timeout is the
    // streaming guard for chunked responses without a length).
    if let Some(len) = resp.content_length() {
        if len > 256 * 1024 {
            return Err("probe response exceeded 256 KiB limit".to_string());
        }
    }
    let body = resp.bytes().await.map_err(|e| e.to_string())?;
    if body.len() > 256 * 1024 {
        return Err("probe response exceeded 256 KiB limit".to_string());
    }
    serde_json::from_slice(&body).map_err(|e| e.to_string())
}

async fn probe_remote_auth_mode(raw_url: &str) -> DesktopConnectionProbeResult {
    let base_url = normalize_remote_base_url(raw_url);
    let status_url = format!("{base_url}/api/status");

    let status = match fetch_json_timeout(&status_url, 8_000).await {
        Ok(status) => status,
        Err(err) => {
            return DesktopConnectionProbeResult {
                base_url,
                reachable: false,
                auth_mode: "unknown".to_string(),
                providers: Vec::new(),
                version: None,
                error: Some(err),
            }
        }
    };

    // auth_required: true → OAuth gate engaged; false → loopback token auth.
    let auth_required = status
        .get("auth_required")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let auth_mode = if auth_required { "oauth" } else { "token" };

    let mut providers: Vec<serde_json::Value> = Vec::new();
    if auth_required {
        if let Ok(body) = fetch_json_timeout(&format!("{base_url}/api/auth/providers"), 8_000).await
        {
            if let Some(list) = body.get("providers").and_then(|p| p.as_array()) {
                providers = list
                    .iter()
                    .filter_map(|p| {
                        let name = p.get("name").and_then(|n| n.as_str()).unwrap_or("");
                        if name.is_empty() {
                            return None;
                        }
                        Some(serde_json::json!({
                            "name": name,
                            "displayName": p.get("display_name").and_then(|d| d.as_str()).unwrap_or(name),
                            "supportsPassword": p.get("supports_password").and_then(|s| s.as_bool()).unwrap_or(false),
                        }))
                    })
                    .collect();
            }
        }
    }

    DesktopConnectionProbeResult {
        base_url,
        reachable: true,
        auth_mode: auth_mode.to_string(),
        providers,
        version: status
            .get("version")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        error: None,
    }
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/// `hermes:connectionConfig:get` — current saved config for the (optional)
/// profile scope.
#[tauri::command]
pub fn get_connection_config(
    app: AppHandle,
    profile: Option<String>,
) -> Result<DesktopConnectionConfig, String> {
    let config = read_connection_config(&app);
    Ok(sanitize(&config, profile))
}

/// `hermes:connectionConfig:save` — persist the connection choice.
#[tauri::command]
pub fn save_connection_config(
    app: AppHandle,
    payload: serde_json::Value,
) -> Result<DesktopConnectionConfig, String> {
    let existing = read_connection_config(&app);
    let next = coerce(&payload, &existing);
    write_connection_config(&app, &next)?;
    let profile = payload
        .get("profile")
        .and_then(|p| p.as_str())
        .map(|s| s.to_string());
    Ok(sanitize(&next, profile))
}

/// `hermes:connectionConfig:apply` — persist + notify the renderer to
/// re-home. (Full backend re-home state machine not ported in this
/// increment; the renderer's existing reconnect path reacts to the event.)
#[tauri::command]
pub fn apply_connection_config(
    app: AppHandle,
    payload: serde_json::Value,
) -> Result<DesktopConnectionConfig, String> {
    let existing = read_connection_config(&app);
    let next = coerce(&payload, &existing);
    write_connection_config(&app, &next)?;
    // Broadcast the *sanitized* config (never the raw payload: the original
    // may carry the full `remoteToken`, and every window shares the frontend).
    let profile = payload
        .get("profile")
        .and_then(|p| p.as_str())
        .map(|s| s.to_string());
    let sanitized = sanitize(&next, profile.clone());
    let _ = app.emit("hermes:connection:applied", &sanitized);
    Ok(sanitized)
}

/// `hermes:connectionConfig:test` — reachability/auth-mode probe of the
/// payload's remote URL (SSH mode: explicit unsupported error for now).
#[tauri::command]
pub async fn test_connection_config(
    payload: serde_json::Value,
) -> Result<DesktopConnectionProbeResult, String> {
    let mode = payload.get("mode").and_then(|m| m.as_str()).unwrap_or("");
    if mode == "ssh" {
        return Err("SSH connection testing is not supported in this build yet.".to_string());
    }
    let raw_url = payload
        .get("remoteUrl")
        .and_then(|u| u.as_str())
        .unwrap_or("");
    if raw_url.trim().is_empty() {
        return Err("remoteUrl is required to test a connection.".to_string());
    }
    Ok(probe_remote_auth_mode(raw_url).await)
}

/// `hermes:connectionConfig:probe` — public /api/status probe of a remote
/// gateway URL (auth-mode discovery, no credentials sent).
#[tauri::command]
pub async fn probe_connection_config(
    raw_url: String,
) -> Result<DesktopConnectionProbeResult, String> {
    if raw_url.trim().is_empty() {
        return Err("URL is required.".to_string());
    }
    Ok(probe_remote_auth_mode(&raw_url).await)
}
