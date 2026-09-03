//! Updates + uninstall for the Tauri desktop (migration ordering 8).
//!
//! Deployment model difference vs Electron, honored throughout: the Tauri
//! build is an MSI-installed app whose Hermes runtime is BUNDLED as an MSI
//! resource (see gateway.rs / build-backend.ps1). There is no git source
//! tree, no venv and no detached updater script to drive, so Electron's
//! git-based client updater does not apply:
//!
//!   - `updates.check` reports `supported: false, reason: 'installer-managed'`
//!     — the renderer's existing install-method handling surfaces a clean
//!     "this install updates through its installer" state instead of the
//!     git update UI (store/updates.ts `reportInstallMethodWarning`).
//!   - BACKEND-side updates are unaffected: the renderer's
//!     `checkBackendUpdates` goes through the `api` proxy, and
//!     `connections.updateAll` POSTs each remote's `/api/hermes/update`
//!     (connections.rs). Only the desktop client itself is installer-managed.
//!   - `uninstall.summary` is REAL: an inventory of the paths this
//!     deployment actually owns (HERMES_HOME data, app config dir, install
//!     dir, bundled backend presence).
//!   - `uninstall.run` stops the managed gateway, then deletes the Hermes
//!     data directory for data modes; the app bundle itself belongs to the
//!     MSI and is removed through the system uninstaller (returned as an
//!     explicit instruction, never a half-deleted install).

use std::path::PathBuf;

use serde_json::{json, Value};
use tauri::{AppHandle, Manager};

/// Hermes data home — stdlib-only mirror of
/// `hermes_startup_watchdog._process_hermes_home` (env override, platform
/// default).
fn hermes_home() -> PathBuf {
    if let Some(val) = std::env::var("HERMES_HOME").ok().filter(|v| !v.trim().is_empty()) {
        return PathBuf::from(val);
    }
    if cfg!(windows) {
        let base = std::env::var("LOCALAPPDATA")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                let home = std::env::var("USERPROFILE")
                    .or_else(|_| std::env::var("HOME"))
                    .map(PathBuf::from)
                    .unwrap_or_default();
                home.join("AppData").join("Local")
            });
        return base.join("hermes");
    }
    let home = std::env::var("HOME").map(PathBuf::from).unwrap_or_default();
    home.join(".hermes")
}

fn branch_config_path(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_config_dir()
        .map_err(|e| format!("app config dir unavailable: {e}"))?;
    Ok(dir.join("update-branch.json"))
}

fn read_branch(app: &AppHandle) -> String {
    let path = branch_config_path(app).unwrap_or_default();
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
        .and_then(|v| {
            v.get("branch")
                .and_then(|b| b.as_str())
                .map(|s| s.to_string())
        })
        .filter(|b| b == "stable" || b == "nightly")
        .unwrap_or_else(|| "stable".to_string())
}

/// `hermes:updates:check` — client updates are installer-managed in this
/// deployment; report the explicit unsupported reason so the renderer shows
/// its install-method state instead of a fake "up to date".
#[tauri::command]
pub fn updates_check() -> Result<Value, String> {
    Ok(json!({
        "supported": false,
        "reason": "installer-managed",
        "message": "This desktop build ships its Hermes runtime inside its installer. Install a new build to update the app and its bundled backend.",
        "fetchedAt": std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0),
    }))
}

/// `hermes:updates:apply` — no in-place client update path exists for an
/// MSI install; fail with the explicit reason instead of pretending.
#[tauri::command]
pub fn updates_apply() -> Result<Value, String> {
    Ok(json!({
        "ok": false,
        "error": "installer-managed",
        "message": "This desktop build updates through its installer — download and run the new MSI to update the app and its bundled backend.",
    }))
}

/// `hermes:updates:branch:get` — persisted branch preference.
#[tauri::command]
pub fn updates_get_branch(app: AppHandle) -> Result<Value, String> {
    Ok(json!({ "branch": read_branch(&app) }))
}

/// `hermes:updates:branch:set` — persist the branch preference ('stable' |
/// 'nightly'; anything else falls back to 'stable').
#[tauri::command]
pub fn updates_set_branch(app: AppHandle, name: String) -> Result<Value, String> {
    let branch = if name.trim() == "nightly" {
        "nightly"
    } else {
        "stable"
    };
    let path = branch_config_path(&app)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("create config dir: {e}"))?;
    }
    std::fs::write(&path, serde_json::to_string_pretty(&json!({ "branch": branch })).map_err(|e| e.to_string())?)
        .map_err(|e| format!("write update-branch.json: {e}"))?;
    Ok(json!({ "branch": branch }))
}

/// `hermes:uninstall:summary` — inventory of the paths this deployment
/// actually owns (the renderer renders its uninstall options from this).
#[tauri::command]
pub fn uninstall_summary(app: AppHandle) -> Result<Value, String> {
    let home = hermes_home();
    let config_dir = app
        .path()
        .app_config_dir()
        .map_err(|e| format!("app config dir unavailable: {e}"))?;

    // Bundled backend presence (MSI resource layout — see gateway.rs).
    let bundled_backend = app
        .path()
        .resource_dir()
        .ok()
        .map(|res| {
            let exe = if cfg!(windows) { "python.exe" } else { "python" };
            res.join("hermes-backend").join("python").join(exe).is_file()
        })
        .unwrap_or(false);

    let install_dir = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|p| p.to_path_buf()));

    Ok(json!({
        "hermes_home": home.to_string_lossy(),
        "agent_installed": bundled_backend,
        "gui_installed": true,
        "source_built_artifacts": [],
        "packaged_app_paths": install_dir
            .map(|dir| vec![dir.to_string_lossy().into_owned()])
            .unwrap_or_default(),
        "userdata_dir": config_dir.to_string_lossy(),
        "userdata_exists": config_dir.is_dir(),
        "platform": std::env::consts::OS,
        "running_app_path": std::env::current_exe()
            .map(|exe| exe.to_string_lossy().into_owned())
            .ok(),
        "probe": "tauri",
    }))
}

/// Best-effort recursive delete that tolerates locked/missing entries (an
/// uninstall must never fail halfway over one locked file).
fn remove_dir_best_effort(path: &PathBuf) {
    if !path.is_dir() {
        return;
    }
    match std::fs::remove_dir_all(path) {
        Ok(()) => {}
        Err(err) => eprintln!("[uninstall] could not remove {}: {err}", path.display()),
    }
}

/// `hermes:uninstall:run` — mode semantics for an MSI deployment:
///   - 'gui'  : the app bundle belongs to the MSI uninstaller — instruct.
///   - 'lite' : stop the backend + remove the Hermes DATA home, keep the app.
///   - 'full' : 'lite' + instruct the user to run the system uninstaller for
///              the app bundle.
/// The bundled backend itself is MSI-owned in every mode and is never
/// half-deleted from the install dir.
#[tauri::command]
pub fn uninstall_run(app: AppHandle, mode: Option<String>) -> Result<Value, String> {
    let mode = mode.unwrap_or_default().trim().to_string();
    match mode.as_str() {
        "gui" => Ok(json!({
            "ok": true,
            "mode": "gui",
            "message": "The desktop app is installed by its MSI. Remove it through Windows Settings → Apps → Hermes (or by running the installer again), which also removes the bundled backend.",
        })),
        "lite" | "full" => {
            // Stop the managed gateway first: a live python.exe holds
            // mandatory locks over the data tree on Windows.
            let gw = app.state::<crate::gateway::GatewayState>();
            crate::gateway::stop_gateway(&gw);

            let home = hermes_home();
            let config_dir = app.path().app_config_dir().ok();
            remove_dir_best_effort(&home);
            if let Some(config_dir) = config_dir {
                remove_dir_best_effort(&config_dir);
            }

            if mode == "full" {
                Ok(json!({
                    "ok": true,
                    "mode": "full",
                    "message": "Hermes data has been removed. Finish the uninstall through Windows Settings → Apps → Hermes (or by running the MSI installer again) to remove the app and its bundled backend.",
                }))
            } else {
                Ok(json!({
                    "ok": true,
                    "mode": "lite",
                    "message": "Hermes data (sessions, profiles, state) has been removed. The desktop app and its bundled backend are untouched.",
                }))
            }
        }
        _ => Ok(json!({
            "ok": false,
            "error": "invalid-mode",
            "message": "Unknown uninstall mode. Use 'gui', 'lite' or 'full'.",
        })),
    }
}
