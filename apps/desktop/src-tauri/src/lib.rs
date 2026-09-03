#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use tauri::Manager;

mod asset_server;
mod bootstrap;
mod capture;
mod commands;
mod connection_config;
mod connections;
mod desktop_misc;
mod gateway;
mod preview_watch;
mod screenshots;
mod ssh_config;
mod tls_proxy;
mod git;
mod terminal;
mod updates;
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
        .manage(asset_server::AssetBase::default())
        .manage(connections::RegistryState::default())
        .manage(preview_watch::PreviewWatchState::default())
        .setup(|app| {
            // Production: serve the renderer over loopback HTTP (see
            // asset_server.rs) BEFORE the window is created, so the window can
            // load with a loopback origin and its WebSocket upgrades carry an
            // Origin any stock `hermes serve` whitelists.
            #[cfg(not(debug_assertions))]
            match asset_server::resolve_dist_dir(&app.handle()) {
                Some(dist) => match asset_server::start(dist) {
                    Ok(port) => app
                        .state::<asset_server::AssetBase>()
                        .set(format!("http://127.0.0.1:{port}")),
                    Err(err) => eprintln!(
                        "[assets] loopback server failed: {err}; falling back to asset protocol"
                    ),
                },
                None => eprintln!("[assets] dist directory not found; falling back to asset protocol"),
            }

            // The main window is created in code (not tauri.conf.json) so its
            // URL follows the active renderer origin: Vite dev server in dev,
            // loopback HTTP in production, asset protocol as fallback.
            let renderer_url: url::Url = asset_server::base_url(&app.handle())
                .parse()
                .unwrap_or_else(|_| "http://tauri.localhost".parse().expect("valid URL"));
            tauri::WebviewWindowBuilder::new(app, "main", tauri::WebviewUrl::External(renderer_url))
                .title("Hermes")
                .inner_size(1200.0, 800.0)
                .resizable(true)
                .center()
                .build()?;

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
        .on_window_event(|window, event| {
            // Tear down the managed gateway when the MAIN window is destroyed —
            // but not for secondary/session pop-outs: closing a spectate window
            // must not kill the backend the main window is still talking to.
            if window.label() == "main" {
                if let tauri::WindowEvent::Destroyed = event {
                    let gw = window.state::<gateway::GatewayState>();
                    gateway::stop_gateway(&gw);
                    // No orphan preview watchers past the window they serve.
                    preview_watch::close_preview_watchers(
                        &window.state::<preview_watch::PreviewWatchState>(),
                    );
                    // No orphan floating screenshot button.
                    screenshots::on_main_window_destroyed(window.app_handle());
                }
            }
        })
        // NOTE: the production renderer loads from http://127.0.0.1 (a REMOTE
        // ACL context), where custom commands are denied unless an app
        // permission allows them. Every command registered here must be
        // mirrored into permissions/loopback-commands.json.
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
            // -- v2 multi-connection registry (port of electron/connection-registry.ts) --
            connections::connections_list,
            connections::connections_save,
            connections::connections_remove,
            connections::connections_set_primary,
            connections::connections_set_launch_mode,
            connections::connections_set_last_used,
            connections::connections_test,
            connections::connections_update_managed,
            connections::connections_update_all,
            connections::get_connection_for,
            connections::get_gateway_ws_url_for,
            // -- updates + uninstall (updates.rs; client updates are installer-managed) --
            updates::updates_check,
            updates::updates_apply,
            updates::updates_get_branch,
            updates::updates_set_branch,
            updates::uninstall_summary,
            updates::uninstall_run,
            // -- ssh config (port of electron/ssh-config.ts) --
            ssh_config::ssh_config_hosts,
            ssh_config::ssh_resolve_host,
            // -- preview file watching (notify crate) --
            preview_watch::watch_preview_file,
            preview_watch::watch_directory,
            preview_watch::stop_preview_file_watch,
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
            desktop_misc::save_image_buffer,
            desktop_misc::open_preview_in_browser,
            // -- quick screenshot (composer window capture → annotate → attach) --
            capture::list_windows,
            capture::capture_window,
            // -- screenshot management (dirs / cleanup / git-exclude / overlay) --
            screenshots::screenshot_dir_stats,
            screenshots::clear_screenshot_dir,
            screenshots::exclude_path_from_git,
            screenshots::set_screenshot_overlay_enabled,
            screenshots::move_screenshot_overlay_by,
            screenshots::save_screenshot_overlay_position,
            screenshots::trigger_quick_screenshot,
            screenshots::focus_main_window,
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
