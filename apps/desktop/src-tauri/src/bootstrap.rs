//! First-launch bootstrap surface (migration of Electron `bootstrap-runner.ts`).
//!
//! In the Electron app this runs the "install Hermes" wizard when the CLI is
//! missing. In this Tauri build the backend is provided by the operator's own
//! `hermes` CLI (uv tool install), so bootstrap is never required: every
//! command returns the renderer's `EMPTY_STATE` (`active: false`), which makes
//! the `DesktopInstallOverlay` stay hidden and keeps `boot-failure-overlay`'s
//! reset/repair buttons harmless. Events are still wired (`hermes:bootstrap:event`)
//! so future ported flows can emit without a bridge change.

use std::collections::HashMap;

use serde::Serialize;

/// Renderer-facing snapshot (global.d.ts `DesktopBootstrapState`). Always the
/// "nothing to do" shape on Tauri.
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct DesktopBootstrapState {
    active: bool,
    manifest: Option<serde_json::Value>,
    stages: HashMap<String, serde_json::Value>,
    error: Option<String>,
    log: Vec<serde_json::Value>,
    started_at: Option<i64>,
    completed_at: Option<i64>,
    setup_choice: Option<serde_json::Value>,
    unsupported_platform: Option<serde_json::Value>,
}

fn empty_state() -> DesktopBootstrapState {
    DesktopBootstrapState {
        active: false,
        manifest: None,
        stages: HashMap::new(),
        error: None,
        log: Vec::new(),
        started_at: None,
        completed_at: None,
        setup_choice: None,
        unsupported_platform: None,
    }
}

/// `hermes:bootstrap:state` — initial snapshot on overlay mount.
#[tauri::command]
pub fn get_bootstrap_state() -> DesktopBootstrapState {
    empty_state()
}

/// `hermes:bootstrap:continueLocal` — no-op (no bootstrap flow on Tauri).
#[tauri::command]
pub fn continue_bootstrap_local() -> DesktopBootstrapState {
    empty_state()
}

/// `hermes:bootstrap:reset` — no-op.
#[tauri::command]
pub fn reset_bootstrap() -> DesktopBootstrapState {
    empty_state()
}

/// `hermes:bootstrap:repair` — no-op (the backend is operator-managed).
#[tauri::command]
pub fn repair_bootstrap() -> DesktopBootstrapState {
    empty_state()
}

/// `hermes:bootstrap:cancel` — no-op.
#[tauri::command]
pub fn cancel_bootstrap() -> DesktopBootstrapState {
    empty_state()
}
