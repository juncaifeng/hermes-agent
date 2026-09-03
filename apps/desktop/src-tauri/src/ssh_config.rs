//! OpenSSH client-config reading (port of electron/ssh-config.ts +
//! the `hermes:ssh-config:*` handlers).
//!
//! - `ssh_config_hosts`: `Host` aliases for the settings UI's suggestions,
//!   with read-only `Include` traversal (depth ≤ 8, no glob expansion —
//!   Electron's production path passes no globSync either).
//! - `ssh_resolve_host`: delegates to `ssh -G <host>` (the authoritative
//!   resolver — it applies the full config semantics we deliberately do not
//!   reimplement) and parses hostname/user/port/identityfile from its output.

use std::collections::HashSet;
use std::io::Read;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

fn home_dir() -> PathBuf {
    std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .map(PathBuf::from)
        .unwrap_or_default()
}

fn parse_ssh_config_hosts(text: &str) -> Vec<String> {
    let mut hosts: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for raw_line in text.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some(rest) = strip_ci_prefix(line, "host") else {
            continue;
        };
        for pattern in rest.split_whitespace() {
            if pattern.is_empty()
                || pattern.contains('*')
                || pattern.contains('?')
                || pattern.starts_with('!')
            {
                continue;
            }
            if seen.insert(pattern.to_string()) {
                hosts.push(pattern.to_string());
            }
        }
    }
    hosts
}

fn parse_ssh_config_includes(text: &str) -> Vec<String> {
    let mut includes: Vec<String> = Vec::new();
    for raw_line in text.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some(rest) = strip_ci_prefix(line, "include") else {
            continue;
        };
        for token in rest.split_whitespace() {
            if !token.is_empty() {
                includes.push(token.to_string());
            }
        }
    }
    includes
}

/// Case-insensitive `key ` prefix stripper ("Host foo" / "host foo" → "foo").
fn strip_ci_prefix<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let (head, rest) = line.split_once(' ')?;
    if head.eq_ignore_ascii_case(key) {
        Some(rest)
    } else {
        None
    }
}

fn read_file_lossy(path: &Path) -> Option<String> {
    let mut buf = String::new();
    std::fs::File::open(path)
        .and_then(|mut f| f.read_to_string(&mut buf))
        .ok()?;
    // Best-effort Latin-1 tolerance for hand-edited configs: replace the
    // replacement chars rather than failing the whole walk.
    Some(buf)
}

fn collect_ssh_config_hosts() -> Vec<String> {
    let home = home_dir();
    let root = home.join(".ssh").join("config");
    let ssh_dir = home.join(".ssh");

    let mut out: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut visited: HashSet<PathBuf> = HashSet::new();

    fn walk(
        path: &Path,
        ssh_dir: &Path,
        home: &Path,
        depth: usize,
        seen: &mut HashSet<String>,
        visited: &mut HashSet<PathBuf>,
        out: &mut Vec<String>,
    ) {
        if depth > 8 || !visited.insert(path.to_path_buf()) {
            return;
        }
        let Some(text) = read_file_lossy(path) else {
            return;
        };
        for host in parse_ssh_config_hosts(&text) {
            if seen.insert(host.clone()) {
                out.push(host);
            }
        }
        for token in parse_ssh_config_includes(&text) {
            let target: PathBuf = if let Some(rest) = token.strip_prefix("~/") {
                home.join(rest)
            } else if Path::new(&token).is_absolute() {
                PathBuf::from(&token)
            } else {
                ssh_dir.join(&token)
            };
            walk(&target, ssh_dir, home, depth + 1, seen, visited, out);
        }
    }

    walk(&root, &ssh_dir, &home, 0, &mut seen, &mut visited, &mut out);
    out
}

fn parse_ssh_g_output(text: &str) -> Value {
    let mut hostname: Option<String> = None;
    let mut user: Option<String> = None;
    let mut port: Option<i64> = None;
    let mut identity_file: Option<String> = None;
    for raw_line in text.lines() {
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }
        let Some((key, value)) = line.split_once(' ') else {
            continue;
        };
        let value = value.trim();
        match key.to_lowercase().as_str() {
            "hostname" if hostname.is_none() => hostname = Some(value.to_string()),
            "user" if user.is_none() => user = Some(value.to_string()),
            "port" if port.is_none() => {
                port = value.parse::<i64>().ok();
            }
            "identityfile" if identity_file.is_none() => identity_file = Some(value.to_string()),
            _ => {}
        }
    }
    json!({
        "hostname": hostname,
        "identityFile": identity_file,
        "port": port,
        "user": user,
    })
}

/// `hermes:ssh-config:hosts` — Host aliases from the user's OpenSSH config.
#[tauri::command]
pub fn ssh_config_hosts() -> Result<Value, String> {
    Ok(json!({ "hosts": collect_ssh_config_hosts() }))
}

/// `hermes:ssh-config:resolve` — authoritative resolution via `ssh -G`
/// (10s timeout, mirrors Electron). Runs on a blocking thread so the async
/// IPC never stalls on the child process.
#[tauri::command]
pub async fn ssh_resolve_host(host: String) -> Result<Value, String> {
    let value = host.trim().to_string();
    if value.is_empty() {
        return Err("SSH host is required.".to_string());
    }

    tauri::async_runtime::spawn_blocking(move || {
        let ssh: PathBuf = if cfg!(windows) {
            let system_root =
                std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_string());
            PathBuf::from(system_root)
                .join("System32")
                .join("OpenSSH")
                .join("ssh.exe")
        } else {
            PathBuf::from("ssh")
        };

        let mut child = std::process::Command::new(&ssh)
            .args(["-G", "--", &value])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| format!("could not run ssh -G (is OpenSSH installed?): {e}"))?;

        // Bound the wait: a wedged ssh (hung network config, agent prompt)
        // must not hold the resolve forever.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    let mut stdout = String::new();
                    if let Some(mut pipe) = child.stdout.take() {
                        let _ = pipe.read_to_string(&mut stdout);
                    }
                    let mut stderr = String::new();
                    if let Some(mut pipe) = child.stderr.take() {
                        let _ = pipe.read_to_string(&mut stderr);
                    }
                    if !status.success() {
                        let detail = if stderr.trim().is_empty() {
                            format!("exit code {}", status.code().unwrap_or(-1))
                        } else {
                            stderr.trim().to_string()
                        };
                        return Err(format!("ssh -G failed: {detail}"));
                    }
                    return Ok(parse_ssh_g_output(&stdout));
                }
                Ok(None) => {
                    if std::time::Instant::now() > deadline {
                        let _ = child.kill();
                        return Err("SSH config resolution timed out.".to_string());
                    }
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                Err(err) => return Err(format!("ssh -G wait failed: {err}")),
            }
        }
    })
    .await
    .map_err(|e| format!("ssh resolve task failed: {e}"))?
}
