#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use tauri::Manager;

mod bootstrap;
mod commands;
mod connection_config;
mod desktop_misc;
mod gateway;
mod tls_proxy;
mod git;
mod terminal;
mod windows;

/// Hermes Desktop — Tauri v2 main process entry.
///
/// Creates the main window, registers the core plugins, and exposes the
/// `window.hermesDesktop` bridge commands (see `docs/tauri-migration-map.md`
/// and `commands.rs`). The command surface grows incrementally.
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_store::Builder::new().build())
        .manage(commands::BackendState::default())
        .manage(terminal::TerminalState::default())
        .manage(gateway::GatewayState::default())
        .setup(|app| {
            // Spawn the managed headless gateway (`hermes serve`) and, once it
            // announces its ephemeral port, rewrite `BackendState` so the
            // renderer connects to the real target.
            let backend = app.state::<commands::BackendState>().inner().clone();
            gateway::spawn_gateway(app.handle().clone(), backend);

            #[cfg(debug_assertions)]
            {
                if let Some(window) = app.get_webview_window("main") {
                    window.open_devtools();
                }
            }

            Ok(())
        })
        .on_window_event(|app, event| {
            if let tauri::WindowEvent::Destroyed = event {
                // Tear down the managed gateway so it doesn't leak as an orphan.
                let gw = app.state::<gateway::GatewayState>();
                gateway::stop_gateway(&gw);
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::api,
            commands::get_version,
            commands::get_connection,
            commands::get_boot_progress,
            commands::touch_backend,
            commands::revalidate_connection,
            commands::get_remote_display_reason,
            commands::set_active_work,
            commands::get_gateway_ws_url,
            connection_config::get_connection_config,
            connection_config::save_connection_config,
            connection_config::apply_connection_config,
            connection_config::test_connection_config,
            connection_config::probe_connection_config,
            bootstrap::get_bootstrap_state,
            bootstrap::continue_bootstrap_local,
            bootstrap::reset_bootstrap,
            bootstrap::repair_bootstrap,
            bootstrap::cancel_bootstrap,
            commands::read_dir,
            commands::read_file_text,
            commands::read_file_data_url,
            commands::write_text_file,
            commands::open_external,
            // -- clipboard / dialogs / fs / logs / misc (desktop_misc.rs) --
            desktop_misc::read_clipboard,
            desktop_misc::write_clipboard,
            desktop_misc::select_paths,
            desktop_misc::select_save_path,
            desktop_misc::trash_path,
            desktop_misc::rename_path,
            desktop_misc::open_dir,
            desktop_misc::reveal_path,
            desktop_misc::open_preview_in_browser,
            desktop_misc::get_recent_logs,
            desktop_misc::reveal_logs,
            desktop_misc::desktop_plugins_root,
            desktop_misc::fetch_link_title,
            desktop_misc::settings_get_default_project_dir,
            desktop_misc::settings_set_default_project_dir,
            desktop_misc::settings_pick_default_project_dir,
            // -- git (port of electron/git-*.ts) --
            git::git_root,
            git::scan_repos,
            git::worktree_list,
            git::git_worktree_add,
            git::worktree_remove,
            git::branch_list,
            git::base_branch_list,
            git::branch_switch,
            git::repo_status,
            git::file_diff,
            git::git_review_list,
            git::git_review_diff,
            git::git_review_stage,
            git::git_review_unstage,
            git::git_review_revert,
            git::git_review_rev_parse,
            git::git_review_commit,
            git::git_review_commit_context,
            git::git_review_push,
            git::git_review_ship_info,
            git::git_review_create_pr,
            // -- windows (port of electron/session-windows.ts) --
            windows::open_session_window,
            windows::open_window,
            // -- terminal (node-pty via sidecar/terminal-sidecar.cjs) --
            terminal::terminal_start,
            terminal::terminal_write,
            terminal::terminal_resize,
            terminal::terminal_cwd,
            terminal::terminal_dispose,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
