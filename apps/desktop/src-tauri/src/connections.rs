//! v2 multi-connection registry — port of Electron `connection-registry.ts`
//! plus the `hermes:connections:*` IPC handlers (migration ordering 7).
//!
//! The registry is a named list of agent SOURCES (local runtime, remote
//! gateways, Hermes Cloud instances, SSH hosts) persisted together in
//! `{app_config_dir}/connections.json`. File shape is byte-compatible with
//! the Electron original (camelCase JSON, same fields), so an Electron
//! install's registry can be adopted as-is.
//!
//! Ported in this increment:
//!   - storage: load with mtime cache, one-shot v1 `connection.json`
//!     migration, corrupt-file sidecar preservation, malformed-entry
//!     quarantine, v1→v2 drift reconciliation
//!   - CRUD: list / save / remove / set-primary / set-launch-mode /
//!     set-last-used, with the label/id uniqueness rules and the dial-fields
//!     changed detection that drives `saved` vs `updated` broadcasts
//!   - dial path: `get_connection_for` + `get_gateway_ws_url_for` so a
//!     registry-scoped secondary resolves through its SOURCE connection
//!     (without these, exposing the registry would make a remote click
//!     silently dial the LOCAL backend — gateway.ts falls back to
//!     `getConnection` when `getConnectionFor` is absent)
//!   - `api` request routing by `connectionId` (commands.rs) so REST calls
//!     owned by a remote gateway stop leaking to the local backend
//!   - test: HTTP `/api/status` probe per connection (local + token remotes)
//!   - update fan-out: remote rows POST their backend's `/api/hermes/update`
//!
//! Deliberately NOT ported (staged gaps, documented for honesty):
//!   - safeStorage secret encryption: tokens/header values are stored in
//!     PLAINTEXT, exactly like the existing v1 `connection.json` handling in
//!     connection_config.rs. `list()` reports `secureTokenStorage:false` so
//!     the renderer offers the plain-text opt-in UI instead of pretending.
//!   - pooled registry backends / SSH tunnels / managed SSH update
//!     lifecycle: there is no SSH transport in the Tauri build yet, so SSH
//!     sources register, render and test as unsupported instead of dialing.
//!   - the WebSocket leg of Test (Electron probes HTTP + WS; the renderer's
//!     own dial surfaces a blocked WS immediately, so the HTTP probe alone
//!     is an acceptable interim).
//!   - OAuth remote sources: descriptor/dial fails with an explicit error
//!     (ticket minting needs the RFC 8252 flow, migration ordering 9).

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime};

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::commands::{build_connection, to_ws_scheme, BackendState};

// ---------------------------------------------------------------------------
// Persisted file shape (camelCase, byte-compatible with Electron)
// ---------------------------------------------------------------------------

pub const REGISTRY_VERSION: u32 = 2;
pub const LOCAL_CONNECTION_ID: &str = "local";
const LABEL_MAX: usize = 64;
const QUARANTINE_CAP: usize = 20;
/// install_id cache TTLs (positive / negative) — mirrors Electron.
const INSTALL_ID_TTL: Duration = Duration::from_secs(5 * 60);
const INSTALL_ID_NEGATIVE_TTL: Duration = Duration::from_secs(60);

#[derive(Serialize, Deserialize, Clone, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct RegistryConnection {
    pub id: String,
    /// "cloud" | "local" | "remote" | "ssh".
    pub kind: String,
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_mode: Option<String>,
    /// Plaintext string in this build (see module docs); an Electron-written
    /// safeStorage envelope object round-trips verbatim (undecryptable, but
    /// never data loss).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<Value>,
    /// Extra gateway headers. Values are plaintext strings in this build; an
    /// Electron-written safeStorage envelope `{encoding,value}` round-trips
    /// verbatim (undecryptable, but never data loss).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub headers: Option<Map<String, Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub org: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remote_hermes_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remote_profile: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct QuarantinedEntry {
    pub reason: String,
    /// The raw user-data entry preserved verbatim.
    pub entry: Value,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase", default)]
pub struct ConnectionRegistry {
    pub version: u32,
    pub primary: String,
    pub launch_mode: String,
    pub last_used: String,
    pub connections: Vec<RegistryConnection>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quarantined: Option<Vec<QuarantinedEntry>>,
}

impl Default for ConnectionRegistry {
    fn default() -> Self {
        Self {
            version: REGISTRY_VERSION,
            primary: LOCAL_CONNECTION_ID.to_string(),
            launch_mode: "primary".to_string(),
            last_used: LOCAL_CONNECTION_ID.to_string(),
            connections: vec![local_entry("This device")],
            quarantined: None,
        }
    }
}

/// Managed state: mtime-cached registry + last-known backend identities.
#[derive(Default)]
pub struct RegistryState {
    cache: std::sync::Mutex<Option<(Option<SystemTime>, ConnectionRegistry)>>,
    install_ids: std::sync::Mutex<HashMap<String, (Option<String>, Instant)>>,
}

// ---------------------------------------------------------------------------
// Pure helpers (ports of connection-registry.ts)
// ---------------------------------------------------------------------------

fn local_entry(label: &str) -> RegistryConnection {
    RegistryConnection {
        id: LOCAL_CONNECTION_ID.to_string(),
        kind: "local".to_string(),
        label: label.to_string(),
        ..Default::default()
    }
}

fn label_key(label: &str) -> String {
    label.trim().to_lowercase()
}

fn unique_label(candidate: &str, taken: &[String]) -> String {
    let used: std::collections::HashSet<String> =
        taken.iter().map(|l| label_key(l)).collect();
    // Reserve room for a collision suffix so the suffixed form stays in-bounds.
    let base: String = candidate.trim().chars().take(LABEL_MAX - 4).collect();
    if !base.is_empty() && !used.contains(&label_key(&base)) {
        return base;
    }
    for n in 2.. {
        let suffixed = format!("{base} {n}");
        if !used.contains(&label_key(&suffixed)) {
            return suffixed;
        }
    }
    unreachable!("suffix loop always terminates")
}

fn label_slug(label: &str) -> String {
    let lowered = label.trim().to_lowercase();
    let mut slug = String::new();
    let mut prev_dash = false;
    for ch in lowered.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch);
            prev_dash = false;
        } else if !prev_dash && !slug.is_empty() {
            slug.push('-');
            prev_dash = true;
        }
    }
    let slug: String = slug.trim_matches('-').chars().take(48).collect();
    if slug.is_empty() {
        "connection".to_string()
    } else {
        slug
    }
}

fn connection_id_for_label(label: &str, taken: &[String]) -> String {
    let used: std::collections::HashSet<&str> = taken.iter().map(|s| s.as_str()).collect();
    let base = label_slug(label);
    if !used.contains(base.as_str()) && base != LOCAL_CONNECTION_ID {
        return base;
    }
    for n in 2.. {
        let candidate = format!("{base}-{n}");
        if !used.contains(candidate.as_str()) && candidate != LOCAL_CONNECTION_ID {
            return candidate;
        }
    }
    unreachable!("suffix loop always terminates")
}

fn norm_auth_mode(mode: Option<&str>) -> String {
    if mode == Some("oauth") {
        "oauth".to_string()
    } else {
        "token".to_string()
    }
}

fn mode_is_remote_like(mode: &str) -> bool {
    mode == "remote" || mode == "cloud"
}

fn host_label_from_base_url(base_url: &str) -> Option<String> {
    let parsed = url::Url::parse(base_url.trim()).ok()?;
    let host = parsed.host_str()?.to_string();
    if host.is_empty() {
        return None;
    }
    match parsed.port() {
        Some(port) if port != 80 && port != 443 => Some(format!("{host}:{port}")),
        _ => Some(host),
    }
}

/// RFC 7230 token characters, per Electron `REMOTE_HEADER_NAME_RE`.
fn valid_header_name(name: &str) -> bool {
    !name.is_empty()
        && name.chars().all(|c| {
            c.is_ascii_alphanumeric()
                || matches!(
                    c,
                    '!' | '#' | '$' | '%' | '&' | '\'' | '*' | '+' | '.' | '^' | '_' | '`' | '|' | '~' | '-'
                )
        })
}

fn forbidden_header_name(name: &str) -> bool {
    matches!(
        name.trim().to_lowercase().as_str(),
        "authorization"
            | "connection"
            | "content-length"
            | "content-type"
            | "cookie"
            | "host"
            | "origin"
            | "referer"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
            | "x-hermes-session-token"
    )
}

/// Header value usable for dialing: a plain string, or a `{value}` envelope's
/// inner plaintext (safeStorage envelopes from an Electron-written file have
/// no usable value here and yield None).
fn usable_header_value(value: &Value) -> Option<String> {
    if let Some(s) = value.as_str() {
        let trimmed = s.trim();
        return (!trimmed.is_empty()).then(|| trimmed.to_string());
    }
    if let Some(obj) = value.as_object() {
        let encoding = obj.get("encoding").and_then(|e| e.as_str()).unwrap_or("plain");
        if encoding == "plain" {
            if let Some(inner) = obj.get("value").and_then(|v| v.as_str()) {
                let trimmed = inner.trim();
                if !trimmed.is_empty() {
                    return Some(trimmed.to_string());
                }
            }
        }
    }
    None
}

/// `pub(crate)` wrapper for commands.rs (api routing flattens stored headers).
pub(crate) fn usable_header_value_pub(value: &Value) -> Option<String> {
    usable_header_value(value)
}

/// Validate + normalize ssh fields. The editor posts ONE composite host field
/// (`user@host:port`), so a host string is parsed for user/port exactly like
/// Electron `normalizeSshConfig`; explicit `user`/`port` fields win.
fn normalize_ssh_fields(
    host: Option<&str>,
    user: Option<&str>,
    port: Option<i64>,
    key_path: Option<&str>,
    remote_hermes_path: Option<&str>,
    remote_profile: Option<&str>,
) -> Option<RegistryConnection> {
    // Tolerate a pasted command: "ssh root@box" → "root@box" (leading prefix
    // only — an interior "ssh " is part of a hostname).
    let trimmed_host = host.unwrap_or_default().trim();
    let host_owned;
    let host_str = if let Some(rest) = trimmed_host
        .strip_prefix("ssh ")
        .or_else(|| trimmed_host.strip_prefix("SSH "))
    {
        host_owned = rest.trim().to_string();
        host_owned.as_str()
    } else {
        trimmed_host
    };
    let mut host = host_str.to_string();
    if host.is_empty() {
        return None;
    }

    let mut parsed_user: Option<String> = None;
    let mut parsed_port: Option<i64> = None;
    if let Some(at) = host.find('@') {
        if at > 0 {
            parsed_user = Some(host[..at].to_string());
            host = host[at + 1..].to_string();
        }
    }
    // `[v6host]:port` / `host:port` (a single colon; a bare v6 literal has
    // several colons and is left alone).
    if let Some(stripped) = host
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
    {
        host = stripped.to_string();
    } else if host.matches(':').count() == 1 {
        // End the split borrow before reassigning `host`.
        let (name, raw_port) = {
            let mut parts = host.splitn(2, ':');
            (
                parts.next().unwrap_or("").to_string(),
                parts.next().map(str::to_string),
            )
        };
        if let Some(raw_port) = raw_port {
            if !raw_port.is_empty() && raw_port.chars().all(|c| c.is_ascii_digit()) {
                host = name;
                parsed_port = raw_port.parse::<i64>().ok();
            }
        }
    }
    if host.is_empty() {
        return None;
    }

    let mut out = RegistryConnection {
        kind: "ssh".to_string(),
        host: Some(host),
        ..Default::default()
    };
    let user = user.map(str::trim).filter(|u| !u.is_empty()).map(str::to_string);
    if let Some(user) = user.or(parsed_user) {
        out.user = Some(user);
    }
    let port = port.filter(|p| *p > 0 && *p <= 65535 && *p != 22).or(parsed_port);
    if let Some(port) = port.filter(|p| *p > 0 && *p <= 65535 && *p != 22) {
        out.port = Some(port);
    }
    if let Some(key_path) = key_path.map(str::trim).filter(|s| !s.is_empty()) {
        out.key_path = Some(key_path.to_string());
    }
    if let Some(path) = remote_hermes_path.map(str::trim).filter(|s| !s.is_empty()) {
        out.remote_hermes_path = Some(path.to_string());
    }
    if let Some(profile) = remote_profile.map(str::trim).filter(|s| !s.is_empty()) {
        // Same identifier rule as the Electron original (lowercase profile
        // names); anything else keeps the same-name fallback behavior.
        let valid = profile.len() <= 64
            && profile
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
            && profile
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-');
        if valid {
            out.remote_profile = Some(profile.to_string());
        }
    }
    Some(out)
}

/// Strict URL normalization for SAVE payloads: unlike the lenient v1 helper,
/// a value that still fails to parse after the `http://` default is an error
/// (Electron's normalizeRemoteBaseUrl throws on bad input).
fn normalize_remote_base_url_strict(raw: &str) -> Result<String, String> {
    let normalized = crate::connection_config::normalize_remote_base_url_pub(raw);
    if normalized.is_empty() {
        return Err("Remote connections need a gateway URL.".to_string());
    }
    if url::Url::parse(&normalized).is_err() {
        return Err(format!("\"{raw}\" is not a valid gateway URL."));
    }
    Ok(normalized)
}

/// Fields that decide whether pooled backends / sockets must be recycled
/// after a save (dial material), vs a cosmetic label rename.
fn dial_fields_changed(before: &RegistryConnection, after: &RegistryConnection) -> bool {
    if before.kind != after.kind {
        return true;
    }
    let eq_str = |a: &Option<String>, b: &Option<String>| a == b;
    !(eq_str(&before.url, &after.url)
        && before.auth_mode == after.auth_mode
        && eq_str(&before.org, &after.org)
        && eq_str(&before.host, &after.host)
        && eq_str(&before.user, &after.user)
        && before.port == after.port
        && eq_str(&before.key_path, &after.key_path)
        && eq_str(&before.remote_hermes_path, &after.remote_hermes_path)
        && eq_str(&before.remote_profile, &after.remote_profile)
        && before.token == after.token
        && before.headers == after.headers)
}

// ---------------------------------------------------------------------------
// Load-time normalization + quarantine
// ---------------------------------------------------------------------------

fn quarantine_reasonable_label(entry: &Value) -> String {
    entry
        .as_object()
        .and_then(|o| o.get("label"))
        .and_then(|l| l.as_str())
        .unwrap_or("")
        .to_string()
}

/// Coerce arbitrary parsed JSON into a valid registry (port of
/// `normalizeRegistry`): quarantine malformed entries instead of dropping
/// them, dedupe labels/ids defensively, guarantee the local entry, retarget
/// primary/lastUsed at existing entries.
fn normalize_registry(raw: Value) -> ConnectionRegistry {
    let empty = ConnectionRegistry::default();
    let Some(parsed) = raw.as_object() else {
        return empty;
    };

    let mut quarantined: Vec<QuarantinedEntry> = Vec::new();
    let mut quarantine = |reason: &str, entry: Value| {
        if quarantined.len() < QUARANTINE_CAP {
            quarantined.push(QuarantinedEntry {
                reason: reason.to_string(),
                entry,
            });
        }
    };

    // Entries quarantined by a previous load are user data too — carry them
    // through every subsequent normalize/write cycle.
    if let Some(previous) = parsed.get("quarantined").and_then(|q| q.as_array()) {
        for item in previous {
            if item.as_object().is_some_and(|o| o.contains_key("entry")) {
                quarantine(
                    item.get("reason").and_then(|r| r.as_str()).unwrap_or("unknown"),
                    item.get("entry").cloned().unwrap_or(Value::Null),
                );
            }
        }
    }

    let mut seen_labels: Vec<String> = Vec::new();
    let mut seen_ids: Vec<String> = Vec::new();
    let mut connections: Vec<RegistryConnection> = Vec::new();

    if let Some(items) = parsed.get("connections").and_then(|c| c.as_array()) {
        for item in items {
            if item.is_null() || item.is_boolean() {
                continue;
            }
            let Some(entry_obj) = item.as_object() else {
                quarantine("entry-malformed", item.clone());
                continue;
            };
            let kind = entry_obj.get("kind").and_then(|k| k.as_str()).unwrap_or("");
            if !matches!(kind, "local" | "remote" | "cloud" | "ssh") {
                quarantine("entry-unrecognized-kind", item.clone());
                continue;
            }

            let mut label = entry_obj
                .get("label")
                .and_then(|l| l.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            if label.is_empty() {
                let fallback = match kind {
                    "ssh" => entry_obj
                        .get("host")
                        .and_then(|h| h.as_str())
                        .unwrap_or("ssh")
                        .to_string(),
                    _ => entry_obj
                        .get("url")
                        .and_then(|u| u.as_str())
                        .and_then(host_label_from_base_url)
                        .unwrap_or_else(|| kind.to_string()),
                };
                label = fallback;
            }
            label = unique_label(&label, &seen_labels);

            let mut id = if kind == "local" {
                LOCAL_CONNECTION_ID.to_string()
            } else {
                entry_obj
                    .get("id")
                    .and_then(|i| i.as_str())
                    .unwrap_or("")
                    .trim()
                    .to_string()
            };
            if id.is_empty() || (seen_ids.contains(&id) && kind != "local") {
                id = connection_id_for_label(&label, &seen_ids);
            }
            if seen_ids.contains(&id) {
                continue; // second 'local' entry — first one wins
            }

            let mut clean = RegistryConnection {
                id: id.clone(),
                kind: kind.to_string(),
                label: label.clone(),
                ..Default::default()
            };

            if kind == "remote" || kind == "cloud" {
                let url = entry_obj
                    .get("url")
                    .and_then(|u| u.as_str())
                    .unwrap_or("")
                    .trim()
                    .to_string();
                if url.is_empty() {
                    quarantine("entry-missing-url", item.clone());
                    continue;
                }
                clean.url = Some(url);
                clean.auth_mode = Some(norm_auth_mode(
                    entry_obj.get("authMode").and_then(|a| a.as_str()),
                ));
                if let Some(token) = entry_obj.get("token") {
                    clean.token = Some(token.clone());
                }
                if let Some(headers) = entry_obj.get("headers") {
                    if let Some(map) = headers.as_object() {
                        let mut stored = Map::new();
                        for (name, value) in map {
                            let name = name.trim();
                            if valid_header_name(name) && !forbidden_header_name(name) {
                                stored.insert(name.to_string(), value.clone());
                            }
                        }
                        if !stored.is_empty() {
                            clean.headers = Some(stored);
                        }
                    }
                }
                let org = entry_obj
                    .get("org")
                    .and_then(|o| o.as_str())
                    .unwrap_or("")
                    .trim()
                    .to_string();
                if kind == "cloud" && !org.is_empty() {
                    clean.org = Some(org);
                }
            } else if kind == "ssh" {
                let get_str = |key: &str| entry_obj.get(key).and_then(|v| v.as_str());
                let port = entry_obj
                    .get("port")
                    .and_then(|p| p.as_i64())
                    .or_else(|| {
                        entry_obj
                            .get("port")
                            .and_then(|p| p.as_str())
                            .and_then(|s| s.trim().parse::<i64>().ok())
                    });
                let Some(ssh) = normalize_ssh_fields(
                    get_str("host"),
                    get_str("user"),
                    port,
                    get_str("keyPath"),
                    get_str("remoteHermesPath"),
                    get_str("remoteProfile"),
                ) else {
                    quarantine("entry-missing-ssh-host", item.clone());
                    continue;
                };
                clean.host = ssh.host;
                clean.user = ssh.user;
                clean.port = ssh.port;
                clean.key_path = ssh.key_path;
                clean.remote_hermes_path = ssh.remote_hermes_path;
                clean.remote_profile = ssh.remote_profile;
            }

            seen_labels.push(label);
            seen_ids.push(id);
            connections.push(clean);
        }
    }

    if !connections.iter().any(|c| c.kind == "local") {
        connections.insert(0, local_entry("This device"));
    }

    let stored_primary = parsed
        .get("primary")
        .and_then(|p| p.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let primary = if connections.iter().any(|c| c.id == stored_primary) {
        stored_primary
    } else {
        LOCAL_CONNECTION_ID.to_string()
    };
    let stored_last_used = parsed
        .get("lastUsed")
        .and_then(|l| l.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let last_used = if connections.iter().any(|c| c.id == stored_last_used) {
        stored_last_used
    } else {
        primary.clone()
    };
    let launch_mode = if parsed.get("launchMode").and_then(|l| l.as_str()) == Some("last-used") {
        "last-used"
    } else {
        "primary"
    };

    let mut registry = ConnectionRegistry {
        version: REGISTRY_VERSION,
        primary,
        launch_mode: launch_mode.to_string(),
        last_used,
        connections,
        quarantined: None,
    };
    if !quarantined.is_empty() {
        registry.quarantined = Some(quarantined);
    }
    registry
}

// ---------------------------------------------------------------------------
// v1 migration + drift reconciliation
// ---------------------------------------------------------------------------

/// One-time import of the v1 `connection.json` (global mode + remote block +
/// per-profile overrides) when `connections.json` does not exist yet. The v1
/// file is NOT modified. Tauri's v1 config never persisted SSH routes, so
/// only remote/cloud blocks import.
fn migrate_v1_to_registry(app: &AppHandle) -> ConnectionRegistry {
    let config = crate::connection_config::read_connection_config_pub(app);
    let mut registry = ConnectionRegistry::default();
    let mut by_url: Vec<String> = Vec::new();

    let mut add_remote_like = |url: &str, auth_mode: Option<&str>, token: Option<&str>, org: Option<&str>, kind: &str, registry: &mut ConnectionRegistry| {
        let url = url.trim();
        if url.is_empty() || by_url.iter().any(|u| u == url) {
            return;
        }
        let label = unique_label(
            host_label_from_base_url(url)
                .as_deref()
                .unwrap_or(if kind == "cloud" { "Hermes Cloud" } else { "Remote gateway" }),
            &registry.connections.iter().map(|c| c.label.clone()).collect::<Vec<_>>(),
        );
        let mut entry = RegistryConnection {
            id: connection_id_for_label(
                &label,
                &registry.connections.iter().map(|c| c.id.clone()).collect::<Vec<_>>(),
            ),
            kind: kind.to_string(),
            label,
            url: Some(url.to_string()),
            auth_mode: Some(norm_auth_mode(auth_mode)),
            ..Default::default()
        };
        if let Some(token) = token.filter(|t| !t.trim().is_empty()) {
            entry.token = Some(Value::String(token.trim().to_string()));
        }
        if kind == "cloud" {
            if let Some(org) = org.filter(|o| !o.trim().is_empty()) {
                entry.org = Some(org.trim().to_string());
            }
        }
        by_url.push(url.to_string());
        registry.connections.push(entry);
    };

    // Global connection → an entry + the primary designation.
    if mode_is_remote_like(&config.mode) {
        let kind = if config.mode == "cloud" { "cloud" } else { "remote" };
        let before = registry.connections.len();
        add_remote_like(
            &config.remote.url,
            Some(&config.remote.auth_mode),
            config.remote.token.as_deref(),
            Some(&config.remote.org),
            kind,
            &mut registry,
        );
        if registry.connections.len() > before {
            registry.primary = registry.connections[before].id.clone();
        }
    }

    // Per-profile overrides → additional registered sources (deduped by URL).
    let mut blocks: Vec<crate::connection_config::RemoteBlock> = Vec::new();
    for (_, block) in config.profiles.iter() {
        if mode_is_remote_like(&block.mode) {
            blocks.push(block.clone());
        }
    }
    for block in blocks {
        add_remote_like(
            &block.url,
            Some(&block.auth_mode),
            block.token.as_deref(),
            Some(&block.org),
            if block.mode == "cloud" { "cloud" } else { "remote" },
            &mut registry,
        );
    }

    if mode_is_remote_like(&config.mode) && registry.primary == LOCAL_CONNECTION_ID {
        // The global remote failed to import (empty URL) — keep local default.
        registry.last_used = registry.primary.clone();
    } else {
        registry.last_used = registry.primary.clone();
    }
    registry
}

/// Heal a registry that never learned about the v1 global route it serves
/// (port of `reconcileRegistryDrift`, remote-like arm only — Tauri's v1
/// config has no SSH routes to heal). Narrow by design: only when the v1
/// global route has NO matching registry entry at all.
fn reconcile_registry_drift(
    app: &AppHandle,
    registry: &ConnectionRegistry,
) -> Option<ConnectionRegistry> {
    let config = crate::connection_config::read_connection_config_pub(app);
    if !mode_is_remote_like(&config.mode) {
        return None;
    }
    let url = crate::connection_config::normalize_remote_base_url_pub(&config.remote.url);
    if url.trim().is_empty() {
        return None;
    }
    let already_registered = registry.connections.iter().any(|c| {
        (c.kind == "remote" || c.kind == "cloud")
            && c.url
                .as_deref()
                .map(|u| crate::connection_config::normalize_remote_base_url_pub(u) == url)
                .unwrap_or(false)
    });
    if already_registered {
        return None;
    }

    let kind = if config.mode == "cloud" { "cloud" } else { "remote" };
    let label = unique_label(
        host_label_from_base_url(&url)
            .as_deref()
            .unwrap_or(if kind == "cloud" { "Hermes Cloud" } else { "Remote gateway" }),
        &registry.connections.iter().map(|c| c.label.clone()).collect::<Vec<_>>(),
    );
    let mut entry = RegistryConnection {
        id: connection_id_for_label(
            &label,
            &registry.connections.iter().map(|c| c.id.clone()).collect::<Vec<_>>(),
        ),
        kind: kind.to_string(),
        label,
        url: Some(url),
        auth_mode: Some(norm_auth_mode(Some(&config.remote.auth_mode))),
        ..Default::default()
    };
    if let Some(token) = config.remote.token.as_deref().filter(|t| !t.trim().is_empty()) {
        entry.token = Some(Value::String(token.trim().to_string()));
    }
    if kind == "cloud" && !config.remote.org.trim().is_empty() {
        entry.org = Some(config.remote.org.trim().to_string());
    }

    let mut next = registry.clone();
    next.primary = entry.id.clone();
    next.last_used = entry.id.clone();
    // Insert-or-replace by id (id is fresh here, so this is a push).
    next.connections.push(entry);
    Some(next)
}

// ---------------------------------------------------------------------------
// Storage (mtime-cached read, best-effort write)
// ---------------------------------------------------------------------------

fn registry_path(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_config_dir()
        .map_err(|e| format!("app config dir unavailable: {e}"))?;
    Ok(dir.join("connections.json"))
}

fn file_mtime(path: &PathBuf) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// Copy an unparseable connections.json aside (once per corruption event) so
/// a later registry write can never destroy the only copy of the user's saved
/// connections. Best effort.
fn preserve_corrupt_sidecar(path: &PathBuf) {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return;
    };
    if raw.trim().is_empty() {
        return;
    }
    let stamp = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let sidecar = path.with_extension(format!("json.corrupt-{stamp}"));
    if !sidecar.exists() {
        let _ = std::fs::write(&sidecar, &raw);
    }
    eprintln!(
        "[connections] connections.json could not be parsed; preserved the original at {} and continuing with a local-only registry",
        sidecar.display()
    );
}

fn write_registry_inner(app: &AppHandle, registry: &ConnectionRegistry) -> Result<(), String> {
    let path = registry_path(app)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("create config dir: {e}"))?;
    }
    let raw = serde_json::to_string_pretty(registry).map_err(|e| e.to_string())?;
    // Plaintext secrets at rest (staged gap — see module docs): at minimum do
    // not inherit a permissive umask on unix.
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true).mode(0o600);
        let mut f = opts.open(&path).map_err(|e| format!("open connections.json: {e}"))?;
        f.write_all(raw.as_bytes())
            .map_err(|e| format!("write connections.json: {e}"))?;
        return Ok(());
    }
    #[cfg(not(unix))]
    std::fs::write(&path, raw).map_err(|e| format!("write connections.json: {e}"))?;
    Ok(())
}

/// Read the registry (mtime-cached). First run migrates from v1
/// `connection.json`; a corrupt file degrades to local-only after preserving
/// a sidecar; drift against the v1 global route heals once.
fn read_registry(app: &AppHandle, state: &RegistryState) -> ConnectionRegistry {
    let path = registry_path(app).unwrap_or_default();
    let mtime = file_mtime(&path);

    if let Ok(guard) = state.cache.lock() {
        if let Some((cached_mtime, cached)) = guard.as_ref() {
            if *cached_mtime == mtime {
                return cached.clone();
            }
        }
    }

    let registry = match mtime {
        None => {
            // First run on this build: import the v1 config, then persist.
            let migrated = migrate_v1_to_registry(app);
            if write_registry_inner(app, &migrated).is_ok() {
                let fresh_mtime = file_mtime(&path);
                if let Ok(mut guard) = state.cache.lock() {
                    *guard = Some((fresh_mtime, migrated.clone()));
                }
            } else {
                if let Ok(mut guard) = state.cache.lock() {
                    *guard = Some((None, migrated.clone()));
                }
            }
            return migrated;
        }
        Some(_) => match std::fs::read_to_string(&path)
            .ok()
            .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
        {
            Some(raw) => normalize_registry(raw),
            None => {
                preserve_corrupt_sidecar(&path);
                normalize_registry(Value::Null)
            }
        },
    };

    // Heal v1 → v2 drift once, persisting the repair.
    if let Some(healed) = reconcile_registry_drift(app, &registry) {
        if write_registry_inner(app, &healed).is_ok() {
            let fresh_mtime = file_mtime(&path);
            if let Ok(mut guard) = state.cache.lock() {
                *guard = Some((fresh_mtime, healed.clone()));
            }
            return healed;
        }
        return healed;
    }

    if let Ok(mut guard) = state.cache.lock() {
        *guard = Some((mtime, registry.clone()));
    }
    registry
}

fn write_registry(app: &AppHandle, state: &RegistryState, registry: ConnectionRegistry) -> Result<(), String> {
    write_registry_inner(app, &registry)?;
    let path = registry_path(app).unwrap_or_default();
    let mtime = file_mtime(&path);
    if let Ok(mut guard) = state.cache.lock() {
        *guard = Some((mtime, registry));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Sanitized renderer views (secrets never cross the IPC boundary)
// ---------------------------------------------------------------------------

fn remember_install_id(state: &RegistryState, connection_id: &str, status: &Value) -> Option<String> {
    let id = status
        .get("install_id")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    if let Ok(mut guard) = state.install_ids.lock() {
        guard.insert(connection_id.to_string(), (id.clone(), Instant::now()));
    }
    id
}

fn cached_install_id(state: &RegistryState, connection_id: &str) -> Option<String> {
    let guard = state.install_ids.lock().ok()?;
    let (id, ts) = guard.get(connection_id)?;
    let ttl = if id.is_some() { INSTALL_ID_TTL } else { INSTALL_ID_NEGATIVE_TTL };
    (ts.elapsed() < ttl).then(|| id.clone()).flatten()
}

fn sanitize_entry(state: &RegistryState, entry: &RegistryConnection) -> Value {
    let token = entry_token(entry);
    let token_set = !token.is_empty();
    let token_preview = if token_set {
        let shown: String = token.trim().chars().take(8).collect();
        Some(format!("{shown}…"))
    } else {
        None
    };
    let header_names: Vec<String> = entry
        .headers
        .as_ref()
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default();
    let mut out = json!({
        "id": entry.id,
        "kind": entry.kind,
        "label": entry.label,
        "tokenSet": token_set,
        "tokenPreview": token_preview,
        "headerNames": header_names,
    });
    let obj = out.as_object_mut().expect("literal object");
    for (field, value) in [
        ("url", entry.url.clone().map(Value::String)),
        ("authMode", entry.auth_mode.clone().map(Value::String)),
        ("org", entry.org.clone().map(Value::String)),
        ("host", entry.host.clone().map(Value::String)),
        ("user", entry.user.clone().map(Value::String)),
        ("port", entry.port.map(|p| json!(p))),
        ("keyPath", entry.key_path.clone().map(Value::String)),
        ("remoteHermesPath", entry.remote_hermes_path.clone().map(Value::String)),
        ("remoteProfile", entry.remote_profile.clone().map(Value::String)),
    ] {
        if let Some(value) = value {
            obj.insert(field.to_string(), value);
        }
    }
    if let Some(install_id) = cached_install_id(state, &entry.id) {
        obj.insert("installId".to_string(), json!(install_id));
    }
    out
}

fn sanitize_registry(state: &RegistryState, registry: &ConnectionRegistry) -> Value {
    let quarantined: Vec<Value> = registry
        .quarantined
        .as_ref()
        .map(|list| {
            list.iter()
                .map(|q| {
                    json!({
                        "reason": q.reason,
                        "label": quarantine_reasonable_label(&q.entry),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    json!({
        "version": registry.version,
        "primary": registry.primary,
        "launchMode": registry.launch_mode,
        "lastUsed": registry.last_used,
        // Plaintext-at-rest build: no OS-keychain encryption, so the renderer
        // offers the plain-text token opt-in instead of pretending.
        "secureTokenStorage": false,
        "connections": registry
            .connections
            .iter()
            .map(|entry| sanitize_entry(state, entry))
            .collect::<Vec<_>>(),
        "quarantined": quarantined,
    })
}

fn broadcast_connections_changed(app: &AppHandle, connection_id: &str, reason: &str) {
    let _ = app.emit(
        "hermes:connections:changed",
        json!({ "connectionId": connection_id, "reason": reason }),
    );
}

// ---------------------------------------------------------------------------
// Save / remove normalization (port of normalizeConnectionInput + merge)
// ---------------------------------------------------------------------------

fn payload_str(payload: &Value, key: &str) -> Option<String> {
    payload
        .get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

fn payload_port(payload: &Value) -> Option<i64> {
    payload.get("port").and_then(|p| {
        p.as_i64().or_else(|| {
            p.as_str()
                .and_then(|s| s.trim().parse::<i64>().ok())
        })
    })
}

/// Validate + normalize a save payload into a `RegistryConnection`.
/// Mirrors Electron `normalizeConnectionInput` (user-facing error messages).
fn normalize_connection_input(
    payload: &Value,
    registry: &ConnectionRegistry,
) -> Result<RegistryConnection, String> {
    let label = payload_str(payload, "label")
        .unwrap_or_default()
        .trim()
        .to_string();
    if label.is_empty() {
        return Err(
            "Every connection needs a name. Give this instance a device name (e.g. \"Homelab\", \"Work laptop\")."
                .to_string(),
        );
    }
    if label.chars().count() > LABEL_MAX {
        return Err(format!("Connection name is too long (max {LABEL_MAX} characters)."));
    }

    let input_id = payload_str(payload, "id").unwrap_or_default();
    let key = label_key(&label);
    if let Some(collision) = registry
        .connections
        .iter()
        .find(|c| label_key(&c.label) == key && c.id != input_id)
    {
        return Err(format!(
            "A connection named \"{}\" already exists. Connection names must be unique.",
            collision.label
        ));
    }

    let kind = payload_str(payload, "kind").unwrap_or_default();
    if kind == "local" {
        // The local entry is managed by the app; only its label is editable.
        return Ok(local_entry(&label));
    }
    if input_id == LOCAL_CONNECTION_ID {
        return Err("The id \"local\" is reserved for the local connection.".to_string());
    }

    let id = if !input_id.is_empty() {
        input_id.clone()
    } else {
        connection_id_for_label(
            &label,
            &registry
                .connections
                .iter()
                .map(|c| c.id.clone())
                .collect::<Vec<_>>(),
        )
    };

    if kind == "ssh" {
        let ssh = normalize_ssh_fields(
            payload_str(payload, "host").as_deref(),
            payload_str(payload, "user").as_deref(),
            payload_port(payload),
            payload_str(payload, "keyPath").as_deref(),
            payload_str(payload, "remoteHermesPath").as_deref(),
            payload_str(payload, "remoteProfile").as_deref(),
        )
        .ok_or("SSH connections need a host.")?;

        // Duplicate prevention: same user@host:port + remote profile.
        let ssh_key = |c: &RegistryConnection| {
            format!(
                "{}@{}:{}::{}",
                c.user.as_deref().unwrap_or("").to_lowercase(),
                c.host.as_deref().unwrap_or("").to_lowercase(),
                c.port.unwrap_or(22),
                c.remote_profile.as_deref().unwrap_or("").trim()
            )
        };
        let target_key = ssh_key(&ssh);
        if let Some(dupe) = registry
            .connections
            .iter()
            .find(|c| c.kind == "ssh" && c.id != id && ssh_key(c) == target_key)
        {
            return Err(format!(
                "A connection to this SSH host already exists (\"{}\").",
                dupe.label
            ));
        }

        let mut entry = RegistryConnection {
            id,
            kind: "ssh".to_string(),
            label,
            ..Default::default()
        };
        entry.host = ssh.host;
        entry.user = ssh.user;
        entry.port = ssh.port;
        entry.key_path = ssh.key_path;
        entry.remote_hermes_path = ssh.remote_hermes_path;
        entry.remote_profile = ssh.remote_profile;
        return Ok(entry);
    }

    if kind == "remote" || kind == "cloud" {
        let url = normalize_remote_base_url_strict(
            payload_str(payload, "url").as_deref().unwrap_or(""),
        )?;

        // Duplicate prevention on the normalized URL across remote/cloud.
        let url_key = |value: &str| {
            value
                .trim()
                .trim_end_matches('/')
                .to_lowercase()
        };
        if let Some(dupe) = registry.connections.iter().find(|c| {
            (c.kind == "remote" || c.kind == "cloud")
                && c.id != id
                && c.url
                    .as_deref()
                    .map(|u| url_key(u) == url_key(&url))
                    .unwrap_or(false)
        }) {
            return Err(format!(
                "A connection to this gateway URL already exists (\"{}\").",
                dupe.label
            ));
        }

        let auth_mode = norm_auth_mode(payload_str(payload, "authMode").as_deref());
        let mut entry = RegistryConnection {
            id,
            kind: kind.clone(),
            label,
            url: Some(url),
            auth_mode: Some(auth_mode.clone()),
            ..Default::default()
        };

        // A token is only meaningful for token-auth remotes; dropping it here
        // is what clears the stale secret when an entry switches to oauth.
        if kind == "remote" && auth_mode == "token" {
            if let Some(token) = payload.get("token") {
                entry.token = Some(token.clone());
            }
        }

        if let Some(headers) = payload.get("headers").and_then(|h| h.as_object()) {
            let mut stored = Map::new();
            for (name, value) in headers {
                let name = name.trim();
                if !valid_header_name(name) || forbidden_header_name(name) {
                    continue;
                }
                // null → keep the stored secret for this name; a non-empty
                // string replaces it; an empty string drops it.
                if value.is_null() {
                    if let Some(existing) = registry
                        .connections
                        .iter()
                        .find(|c| c.id == entry.id || c.id == input_id)
                        .and_then(|c| c.headers.as_ref())
                        .and_then(|h| h.get(name))
                    {
                        stored.insert(name.to_string(), existing.clone());
                    }
                } else if let Some(plain) = usable_header_value(value) {
                    stored.insert(name.to_string(), Value::String(plain));
                }
            }
            if !stored.is_empty() {
                entry.headers = Some(stored);
            }
        }

        let org = payload_str(payload, "org").unwrap_or_default().trim().to_string();
        if kind == "cloud" && !org.is_empty() {
            entry.org = Some(org);
        }
        return Ok(entry);
    }

    Err(format!("Unknown connection kind: {kind}"))
}

/// Merge a (possibly partial) edit payload over the stored entry so fields
/// the editor doesn't carry survive a save (port of `mergeConnectionInput`).
fn merge_connection_input(
    payload: &Value,
    existing: Option<&RegistryConnection>,
) -> Value {
    let Some(existing) = existing else {
        return payload.clone();
    };
    let kind = payload_str(payload, "kind").unwrap_or_default();
    if existing.kind != kind {
        return payload.clone();
    }

    let mut merged = payload.clone();
    let obj = merged.as_object_mut().expect("save payload is an object");
    let inherit_str = |obj: &mut Map<String, Value>, field: &str, stored: &Option<String>| {
        if !obj.contains_key(field) {
            if let Some(value) = stored {
                obj.insert(field.to_string(), Value::String(value.clone()));
            }
        }
    };
    inherit_str(obj, "url", &existing.url);
    inherit_str(obj, "authMode", &existing.auth_mode);
    inherit_str(obj, "org", &existing.org);
    inherit_str(obj, "host", &existing.host);
    inherit_str(obj, "keyPath", &existing.key_path);
    inherit_str(obj, "remoteHermesPath", &existing.remote_hermes_path);
    inherit_str(obj, "remoteProfile", &existing.remote_profile);
    if !obj.contains_key("headers") {
        if let Some(headers) = &existing.headers {
            obj.insert("headers".to_string(), Value::Object(headers.clone()));
        }
    }
    // ssh user/port: when the payload carries a NEW composite host string,
    // the host string is authoritative — stored user/port are NOT inherited.
    let has_host = payload_str(payload, "host")
        .map(|h| !h.trim().is_empty())
        .unwrap_or(false);
    if !has_host {
        if let Some(user) = &existing.user {
            obj.entry("user".to_string())
                .or_insert_with(|| Value::String(user.clone()));
        }
        if let Some(port) = existing.port {
            obj.entry("port".to_string()).or_insert_with(|| json!(port));
        }
    }
    merged
}

/// `hermes:connections:save` logic: merge → validate → upsert → broadcast
/// (`updated` for dial-material edits, `saved` otherwise).
fn save_registry_connection(
    app: &AppHandle,
    state: &RegistryState,
    payload: &Value,
) -> Result<RegistryConnection, String> {
    let registry = read_registry(app, state);
    let input_id = payload_str(payload, "id").unwrap_or_default();
    let existing = if input_id.is_empty() {
        None
    } else {
        registry.connections.iter().find(|c| c.id == input_id)
    };

    // Token handling: an incoming plaintext token replaces; an absent token
    // field inherits the stored one on edit (merge below carries it).
    let incoming_token = payload_str(payload, "token").map(|t| t.trim().to_string());
    let mut with_token = payload.clone();
    let payload_obj = with_token
        .as_object_mut()
        .ok_or_else(|| "save payload must be an object".to_string())?;
    if let Some(token) = &incoming_token {
        payload_obj.insert("token".to_string(), Value::String(token.clone()));
    } else if let Some(existing_token) = existing.and_then(|c| c.token.as_ref()) {
        payload_obj.insert("token".to_string(), existing_token.clone());
    }

    let merged = merge_connection_input(&with_token, existing);
    let entry = normalize_connection_input(&merged, &registry)?;

    // Token-auth remotes must actually have a token to be dialable.
    if entry.kind == "remote"
        && entry.auth_mode.as_deref() != Some("oauth")
        && entry
            .token
            .as_ref()
            .and_then(|t| t.as_str())
            .map(|t| t.trim().is_empty())
            .unwrap_or(true)
    {
        return Err("Remote gateway session token is required.".to_string());
    }

    let dial_changed = existing.is_some_and(|before| dial_fields_changed(before, &entry));

    let mut next = registry.clone();
    match next.connections.iter().position(|c| c.id == entry.id) {
        Some(idx) => next.connections[idx] = entry.clone(),
        None => next.connections.push(entry.clone()),
    }
    write_registry(app, state, next)?;

    if dial_changed {
        // No pooled registry backends exist in this build (SSH transport is
        // staged), so there is nothing to stop server-side; the broadcast
        // still tells renderers to dispose + re-dial their secondaries.
        broadcast_connections_changed(&app.clone(), &entry.id, "updated");
    } else {
        broadcast_connections_changed(&app.clone(), &entry.id, "saved");
    }
    Ok(entry)
}

// ---------------------------------------------------------------------------
// Remote HTTP helpers (registered-connection trust level: LAN/loopback hosts
// are the feature, but schemes stay http/https and bodies stay size-capped)
// ---------------------------------------------------------------------------

fn remote_request_builder(
    url: &str,
    method: &str,
    timeout_ms: u64,
) -> Result<reqwest::RequestBuilder, String> {
    let parsed = url::Url::parse(url).map_err(|_| format!("invalid gateway URL: {url}"))?;
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return Err(format!("unsupported gateway URL scheme: {}", parsed.scheme()));
    }
    let client = reqwest::Client::builder()
        .timeout(Duration::from_millis(timeout_ms))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| e.to_string())?;
    let builder = match method {
        "POST" => client.post(url),
        "PUT" => client.put(url),
        "PATCH" => client.patch(url),
        "DELETE" => client.delete(url),
        _ => client.get(url),
    };
    Ok(builder.header("User-Agent", "hermes-desktop"))
}

fn apply_remote_auth(
    builder: reqwest::RequestBuilder,
    token: &str,
    headers: Option<&Map<String, Value>>,
) -> reqwest::RequestBuilder {
    let mut builder = builder;
    if !token.is_empty() {
        builder = builder.header("X-Hermes-Session-Token", token);
    }
    if let Some(headers) = headers {
        for (name, value) in headers {
            if let Some(value) = usable_header_value(value) {
                builder = builder.header(name.as_str(), value);
            }
        }
    }
    builder
}

async fn fetch_remote_json(
    url: &str,
    method: &str,
    token: &str,
    headers: Option<&Map<String, Value>>,
    timeout_ms: u64,
) -> Result<Value, String> {
    let builder = remote_request_builder(url, method, timeout_ms)?;
    let builder = apply_remote_auth(builder, token, headers);
    let resp = builder.send().await.map_err(|e| e.to_string())?;
    let status = resp.status();
    let bytes = resp.bytes().await.map_err(|e| e.to_string())?;
    if bytes.len() > 512 * 1024 {
        return Err("gateway response exceeded 512 KiB limit".to_string());
    }
    if !status.is_success() {
        return Err(format!(
            "gateway {}: {}",
            status.as_u16(),
            String::from_utf8_lossy(&bytes)
        ));
    }
    serde_json::from_slice(&bytes)
        .map_err(|_| format!("gateway returned invalid JSON: {}", String::from_utf8_lossy(&bytes)))
}

/// Plaintext token for dialing (envelope values from Electron-written files
/// are undecryptable here and yield empty).
pub(crate) fn entry_token(entry: &RegistryConnection) -> String {
    entry
        .token
        .as_ref()
        .and_then(|t| t.as_str())
        .unwrap_or("")
        .trim()
        .to_string()
}

fn entry_ws_url(base_url: &str, token: &str) -> String {
    let ws_base = to_ws_scheme(base_url);
    if token.is_empty() {
        format!("{ws_base}/api/ws")
    } else {
        format!("{ws_base}/api/ws?token={token}")
    }
}

fn find_entry<'a>(
    registry: &'a ConnectionRegistry,
    id: &str,
) -> Result<&'a RegistryConnection, String> {
    registry
        .connections
        .iter()
        .find(|c| c.id == id)
        .ok_or_else(|| format!("No connection with id \"{id}\"."))
}

// ---------------------------------------------------------------------------
// API-routing support (commands::api connectionId arm)
// ---------------------------------------------------------------------------

/// Resolve the (base_url, token, headers) a connection-scoped `api` request
/// must target. `None` for local/empty (caller keeps the primary backend).
/// Errors for unknown ids, SSH sources (no transport) and OAuth remotes
/// (needs the RFC 8252 flow).
pub(crate) fn api_target(
    app: &AppHandle,
    state: &RegistryState,
    connection_id: &str,
) -> Result<Option<(String, String, Option<Map<String, Value>>)>, String> {
    let registry = read_registry(app, state);
    let entry = find_entry(&registry, connection_id)?;
    match entry.kind.as_str() {
        "remote" | "cloud" => {
            let auth_mode = entry.auth_mode.as_deref().unwrap_or("token");
            if auth_mode == "oauth" {
                return Err(
                    "OAuth remote gateways are not supported in this build yet.".to_string(),
                );
            }
            let token = entry_token(entry);
            if token.is_empty() {
                return Err(format!(
                    "Connection \"{}\" has no saved session token. Edit the connection and paste one.",
                    entry.label
                ));
            }
            let base = entry
                .url
                .as_deref()
                .map(|u| u.trim_end_matches('/').to_string())
                .unwrap_or_default();
            if base.is_empty() {
                return Err(format!("Connection \"{}\" has no URL.", entry.label));
            }
            Ok(Some((base, token, entry.headers.clone())))
        }
        "ssh" => Err(
            "SSH sources are not available in this build yet — the Tauri desktop has no SSH transport."
                .to_string(),
        ),
        _ => Ok(None), // local: caller keeps the primary backend
    }
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/// `hermes:connections:list` — sanitized registry snapshot.
#[tauri::command]
pub fn connections_list(
    app: AppHandle,
    state: State<'_, RegistryState>,
) -> Result<Value, String> {
    let registry = read_registry(&app, &state);
    if let Some(quarantined) = &registry.quarantined {
        eprintln!(
            "[connections] {} malformed registry entries quarantined (kept under \"quarantined\" in connections.json); healthy connections loaded normally.",
            quarantined.len()
        );
    }
    Ok(sanitize_registry(&state, &registry))
}

/// `hermes:connections:save` — create or edit a registry connection.
#[tauri::command]
pub fn connections_save(
    app: AppHandle,
    state: State<'_, RegistryState>,
    payload: Value,
) -> Result<Value, String> {
    let entry = save_registry_connection(&app, &state, &payload)?;
    let registry = read_registry(&app, &state);
    Ok(json!({
        "ok": true,
        "connection": sanitize_entry(&state, &entry),
        "registry": sanitize_registry(&state, &registry),
    }))
}

/// `hermes:connections:remove` — remove a connection (local is not
/// removable; removing the primary retargets to local).
#[tauri::command]
pub fn connections_remove(
    app: AppHandle,
    state: State<'_, RegistryState>,
    id: String,
) -> Result<Value, String> {
    let key = id.trim().to_string();
    let registry = read_registry(&app, &state);
    let target = find_entry(&registry, &key).cloned()?;
    if target.kind == "local" {
        return Err("The local connection cannot be removed.".to_string());
    }

    let mut next = registry.clone();
    next.connections.retain(|c| c.id != key);
    if next.primary == key {
        next.primary = LOCAL_CONNECTION_ID.to_string();
    }
    if next.last_used == key {
        next.last_used = next.primary.clone();
    }
    write_registry(&app, &state, next.clone())?;
    // No pooled registry backends exist in this build; the broadcast makes
    // renderers dispose their secondaries scoped to the removed source.
    broadcast_connections_changed(&app, &key, "removed");
    Ok(json!({ "ok": true, "registry": sanitize_registry(&state, &next) }))
}

/// `hermes:connections:set-primary` — point the window/primary designation
/// at another registered connection.
#[tauri::command]
pub fn connections_set_primary(
    app: AppHandle,
    state: State<'_, RegistryState>,
    id: String,
) -> Result<Value, String> {
    let key = id.trim().to_string();
    let mut registry = read_registry(&app, &state);
    find_entry(&registry, &key)?;
    registry.primary = key;
    write_registry(&app, &state, registry.clone())?;
    Ok(json!({ "ok": true, "registry": sanitize_registry(&state, &registry) }))
}

/// `hermes:connections:set-launch-mode` — 'primary' | 'last-used'.
#[tauri::command]
pub fn connections_set_launch_mode(
    app: AppHandle,
    state: State<'_, RegistryState>,
    mode: String,
) -> Result<Value, String> {
    if mode != "last-used" && mode != "primary" {
        return Err(format!("Unknown connection launch mode \"{mode}\"."));
    }
    let mut registry = read_registry(&app, &state);
    registry.launch_mode = mode;
    write_registry(&app, &state, registry.clone())?;
    Ok(json!({ "ok": true, "registry": sanitize_registry(&state, &registry) }))
}

/// `hermes:connections:set-last-used` — remember the last source the
/// Sessions workspace successfully opened.
#[tauri::command]
pub fn connections_set_last_used(
    app: AppHandle,
    state: State<'_, RegistryState>,
    id: String,
) -> Result<Value, String> {
    let key = id.trim().to_string();
    let mut registry = read_registry(&app, &state);
    find_entry(&registry, &key)?;
    registry.last_used = key;
    write_registry(&app, &state, registry.clone())?;
    Ok(json!({ "ok": true, "registry": sanitize_registry(&state, &registry) }))
}

/// `hermes:connections:test` — reachability probe. HTTP `/api/status` leg
/// only (the renderer's own WS dial surfaces a blocked WebSocket
/// immediately; the dedicated WS probe leg is a staged gap).
#[tauri::command]
pub async fn connections_test(
    app: AppHandle,
    state: State<'_, RegistryState>,
    id: String,
) -> Result<Value, String> {
    let key = id.trim().to_string();
    let registry = read_registry(&app, &state);
    let entry = find_entry(&registry, &key)?.clone();

    match entry.kind.as_str() {
        "ssh" => Ok(json!({
            "ok": false,
            "reachable": false,
            "sshError": "unsupported-platform",
            "error": "SSH transport is not available in this build yet.",
        })),
        "local" => {
            let backend = app.state::<BackendState>();
            let base = backend
                .wait_ready(Duration::from_secs(crate::commands::BACKEND_READY_TIMEOUT_SECS));
            let token = backend.token_snapshot();
            match fetch_remote_json(&format!("{base}/api/status"), "GET", &token, None, 8_000).await {
                Ok(status) => {
                    remember_install_id(&state, &key, &status);
                    Ok(json!({
                        "ok": true,
                        "reachable": true,
                        "baseUrl": base,
                        "version": status.get("version").and_then(|v| v.as_str()),
                    }))
                }
                Err(error) => Ok(json!({
                    "ok": false,
                    "reachable": false,
                    "baseUrl": base,
                    "error": error,
                })),
            }
        }
        _ => {
            let auth_mode = entry.auth_mode.as_deref().unwrap_or("token");
            if auth_mode == "oauth" {
                return Ok(json!({
                    "ok": false,
                    "reachable": false,
                    "error": "OAuth remote gateways are not supported in this build yet; test a token-auth connection instead.",
                }));
            }
            let base = entry
                .url
                .as_deref()
                .map(|u| u.trim_end_matches('/').to_string())
                .unwrap_or_default();
            let token = entry_token(&entry);
            if token.is_empty() {
                return Ok(json!({
                    "ok": false,
                    "reachable": false,
                    "error": "This connection has no saved session token. Edit the connection and paste one.",
                }));
            }
            match fetch_remote_json(
                &format!("{base}/api/status"),
                "GET",
                &token,
                entry.headers.as_ref(),
                8_000,
            )
            .await
            {
                Ok(status) => {
                    remember_install_id(&state, &key, &status);
                    Ok(json!({
                        "ok": true,
                        "reachable": true,
                        "baseUrl": base,
                        "version": status.get("version").and_then(|v| v.as_str()),
                    }))
                }
                Err(error) => Ok(json!({
                    "ok": false,
                    "reachable": false,
                    "baseUrl": base,
                    "error": error,
                })),
            }
        }
    }
}

/// `hermes:connections:update-managed` — transactional Desktop-managed SSH
/// update lifecycle. There is no SSH transport in this build, so every
/// request is refused with the full result shape the renderer expects.
#[tauri::command]
pub fn connections_update_managed(
    app: AppHandle,
    state: State<'_, RegistryState>,
    id: String,
) -> Result<Value, String> {
    let key = id.trim().to_string();
    let registry = read_registry(&app, &state);
    let _entry = find_entry(&registry, &key)?;
    let correlation_id = {
        use rand::Rng;
        let mut buf = [0u8; 16];
        rand::rng().fill_bytes(&mut buf);
        buf.iter().map(|b| format!("{b:02x}")).collect::<String>()
    };
    Ok(json!({
        "connectionId": key,
        "correlationId": correlation_id,
        "ok": false,
        "updateOk": false,
        "restoreOk": false,
        "outcome": "refused",
        "exitCode": null,
        "receipt": null,
        "scopes": [],
        "error": "SSH update lifecycle is not available in this build yet — the Tauri desktop has no SSH transport.",
        "message": "SSH update lifecycle is not available in this build yet.",
    }))
}

/// `hermes:connections:update-all` — fan the update out to every eligible
/// registered connection. Cloud rows are skipped (platform-managed); local
/// is installer-managed in this build; SSH is refused (no transport); remote
/// rows POST their backend's own `/api/hermes/update`.
#[tauri::command]
pub async fn connections_update_all(
    app: AppHandle,
    state: State<'_, RegistryState>,
    options: Option<Value>,
) -> Result<Value, String> {
    let registry = read_registry(&app, &state);
    let exclude: Vec<String> = options
        .as_ref()
        .and_then(|o| o.get("excludeIds"))
        .and_then(|e| e.as_array())
        .map(|list| {
            list.iter()
                .filter_map(|id| id.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();

    let mut results: Vec<Value> = Vec::new();
    for connection in &registry.connections {
        if exclude.contains(&connection.id) {
            continue;
        }
        let mut row_obj = Map::new();
        row_obj.insert("connectionId".into(), json!(connection.id));
        row_obj.insert("label".into(), json!(connection.label));
        row_obj.insert("kind".into(), json!(connection.kind));
        match connection.kind.as_str() {
            "cloud" => {
                row_obj.insert("ok".into(), json!(false));
                row_obj.insert("skipped".into(), json!(true));
                row_obj.insert("reason".into(), json!("cloud-managed"));
            }
            "ssh" => {
                row_obj.insert("ok".into(), json!(false));
                row_obj.insert(
                    "error".into(),
                    json!("SSH update lifecycle is not available in this build yet."),
                );
            }
            "local" => {
                // The bundled runtime ships inside the MSI and updates with
                // the installer; driving a backend self-update against it
                // could corrupt the packaged deployment.
                row_obj.insert("ok".into(), json!(false));
                row_obj.insert("skipped".into(), json!(true));
                row_obj.insert("reason".into(), json!("installer-managed"));
                row_obj.insert(
                    "detail".into(),
                    json!("This desktop build updates its bundled runtime through its installer."),
                );
            }
            _ => {
                // remote
                let auth_mode = connection.auth_mode.as_deref().unwrap_or("token");
                if auth_mode == "oauth" {
                    row_obj.insert("ok".into(), json!(false));
                    row_obj.insert(
                        "error".into(),
                        json!("OAuth remote gateways are not supported in this build yet."),
                    );
                } else {
                    let token = entry_token(connection);
                    let url = connection
                        .url
                        .as_deref()
                        .map(|u| format!("{}/api/hermes/update", u.trim_end_matches('/')))
                        .unwrap_or_default();
                    if token.is_empty() || url.is_empty() {
                        row_obj.insert("ok".into(), json!(false));
                        row_obj.insert(
                            "error".into(),
                            json!("This connection has no saved session token. Edit the connection and paste one."),
                        );
                    } else {
                        match fetch_remote_json(
                            &url,
                            "POST",
                            &token,
                            connection.headers.as_ref(),
                            15_000,
                        )
                        .await
                        {
                            Ok(body) => {
                                if body.get("ok") == Some(&json!(false)) {
                                    row_obj.insert("ok".into(), json!(false));
                                    row_obj.insert("skipped".into(), json!(true));
                                    row_obj.insert(
                                        "reason".into(),
                                        body.get("error").cloned().unwrap_or(json!("backend-refused")),
                                    );
                                    if let Some(detail) = body.get("message") {
                                        row_obj.insert("detail".into(), detail.clone());
                                    }
                                } else {
                                    row_obj.insert("ok".into(), json!(true));
                                    row_obj.insert(
                                        "detail".into(),
                                        body
                                            .get("message")
                                            .cloned()
                                            .unwrap_or(json!("update started")),
                                    );
                                }
                            }
                            Err(error) => {
                                row_obj.insert("ok".into(), json!(false));
                                row_obj.insert("error".into(), json!(error));
                            }
                        }
                    }
                }
            }
        }
        results.push(Value::Object(row_obj));
    }

    Ok(json!({ "ok": true, "results": results }))
}

/// `hermes:getConnectionFor` — registry-scoped backend descriptor for
/// (connectionId, profile). The dial path for multi-source switching: a
/// remote source resolves to ITS url/token; local delegates to the managed
/// backend; SSH fails loudly (never silently falls back to local, which is
/// what an absent bridge method would cause in gateway.ts).
#[tauri::command]
pub fn get_connection_for(
    app: AppHandle,
    registry_state: State<'_, RegistryState>,
    backend_state: State<'_, BackendState>,
    payload: Value,
) -> Result<Value, String> {
    let connection_id = payload
        .get("connectionId")
        .and_then(|c| c.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let profile = payload
        .get("profile")
        .and_then(|p| p.as_str())
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty());

    if connection_id.is_empty() || connection_id == LOCAL_CONNECTION_ID {
        let _ = backend_state.wait_ready(Duration::from_secs(crate::commands::BACKEND_READY_TIMEOUT_SECS));
        let conn = build_connection(&backend_state, profile)?;
        // Serialize the additive identity fields (camelCase) onto the
        // ConnectionInfo JSON so registry bookkeeping names the source.
        let mut value = serde_json::to_value(&conn).map_err(|e| e.to_string())?;
        if let Some(obj) = value.as_object_mut() {
            obj.insert("connectionId".into(), json!(LOCAL_CONNECTION_ID));
            obj.insert("registryScoped".into(), json!(true));
        }
        return Ok(value);
    }

    let registry = read_registry(&app, &registry_state);
    let entry = find_entry(&registry, &connection_id)?;
    match entry.kind.as_str() {
        "ssh" => Err(
            "SSH sources are not available in this build yet — the Tauri desktop has no SSH transport."
                .to_string(),
        ),
        _ => {
            let auth_mode = entry.auth_mode.as_deref().unwrap_or("token");
            let base = entry
                .url
                .as_deref()
                .map(|u| u.trim_end_matches('/').to_string())
                .unwrap_or_default();
            if base.is_empty() {
                return Err(format!("Connection \"{}\" has no URL.", entry.label));
            }
            let token = if auth_mode == "oauth" {
                // No ticket minting without the RFC 8252 flow: the renderer's
                // resolveGatewayWsUrl throws its explicit "cannot refresh
                // OAuth WebSocket tickets" error on dial — loud, not silent.
                String::new()
            } else {
                let token = entry_token(entry);
                if token.is_empty() {
                    return Err(format!(
                        "Connection \"{}\" has no saved session token. Edit the connection and paste one.",
                        entry.label
                    ));
                }
                token
            };
            let ws_url = entry_ws_url(&base, &token);
            Ok(json!({
                "baseUrl": base,
                "token": token,
                "wsUrl": ws_url,
                "mode": "remote",
                "remoteKind": if entry.kind == "cloud" { "cloud" } else { "url" },
                "remoteHost": host_label_from_base_url(&base),
                "authMode": auth_mode,
                "source": "settings",
                "logs": [],
                "profile": profile,
                "connectionId": entry.id,
                "registryScoped": true,
                "isFullscreen": false,
                "nativeOverlayWidth": 0,
            }))
        }
    }
}

/// `hermes:gatewayWsUrlFor` — registry-scoped fresh WS URL (same result
/// contract as get_gateway_ws_url).
#[tauri::command]
pub fn get_gateway_ws_url_for(
    app: AppHandle,
    registry_state: State<'_, RegistryState>,
    backend_state: State<'_, BackendState>,
    payload: Value,
) -> Result<Value, String> {
    let connection_id = payload
        .get("connectionId")
        .and_then(|c| c.as_str())
        .unwrap_or("")
        .trim()
        .to_string();

    if connection_id.is_empty() || connection_id == LOCAL_CONNECTION_ID {
        let _ = backend_state.wait_ready(Duration::from_secs(crate::commands::BACKEND_READY_TIMEOUT_SECS));
        let conn = build_connection(&backend_state, None)?;
        return Ok(json!({ "ok": true, "wsUrl": conn.ws_url, "baseUrl": conn.base_url }));
    }

    let registry = read_registry(&app, &registry_state);
    let entry = find_entry(&registry, &connection_id)?;
    match entry.kind.as_str() {
        "ssh" => Err(
            "SSH sources are not available in this build yet — the Tauri desktop has no SSH transport."
                .to_string(),
        ),
        _ => {
            let auth_mode = entry.auth_mode.as_deref().unwrap_or("token");
            if auth_mode == "oauth" {
                return Ok(json!({
                    "ok": false,
                    "error": "OAuth remote gateways are not supported in this build yet.",
                }));
            }
            let base = entry
                .url
                .as_deref()
                .map(|u| u.trim_end_matches('/').to_string())
                .unwrap_or_default();
            let token = entry_token(entry);
            if base.is_empty() || token.is_empty() {
                return Ok(json!({
                    "ok": false,
                    "error": format!("Connection \"{}\" has no usable URL + session token.", entry.label),
                }));
            }
            Ok(json!({
                "ok": true,
                "wsUrl": entry_ws_url(&base, &token),
                "baseUrl": base,
            }))
        }
    }
}
