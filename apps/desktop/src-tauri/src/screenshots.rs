//! Screenshot management: directory stats, guarded cleanup, git-exclude
//! writes, and the quick-screenshot overlay window.
//!
//! Everything here is about the two MANAGED screenshot locations:
//!   * `<app_data>/composer-images/`  (default, always)
//!   * `<workspace>/.hermes/screenshots/`  (opt-in project mode)
//!
//! The cleanup command is the dangerous one, so it whitelists: only those
//! two directory shapes are ever emptied, anything else is refused.

use std::path::{Path, PathBuf};

use serde::Serialize;
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

pub(crate) const OVERLAY_LABEL: &str = "screenshot-overlay";

/// Project-mode screenshot dir relative to the workspace root.
pub(crate) const PROJECT_SCREENSHOTS_REL: &str = ".hermes/screenshots";

fn composer_images_dir(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(app
        .path()
        .app_data_dir()
        .map_err(|e| format!("app data dir unavailable: {e}"))?
        .join("composer-images"))
}

fn normalize(path: &Path) -> PathBuf {
    // Case-insensitive, separator-insensitive comparison base for the
    // whitelist (Windows is the only platform with capture today, but keep
    // the comparison cheap everywhere).
    path.to_string_lossy().replace('\\', "/").trim_end_matches('/').into()
}

/// The project-mode half of the cleanup whitelist: `<something>/.hermes/screenshots`.
fn has_project_screenshots_suffix(path: &Path) -> bool {
    let names: Vec<String> = path
        .components()
        .map(|c| c.as_os_str().to_string_lossy().to_lowercase())
        .collect();
    let n = names.len();
    // At least one component before the suffix pair — ".hermes/screenshots"
    // alone is not a managed location.
    n >= 3 && names[n - 2] == ".hermes" && names[n - 1] == "screenshots"
}

fn is_managed_screenshot_dir(app: &AppHandle, path: &Path) -> bool {
    if let Ok(app_data) = composer_images_dir(app) {
        if normalize(path) == normalize(&app_data) {
            return true;
        }
    }

    has_project_screenshots_suffix(path)
}

fn dir_stats(path: &Path) -> (bool, u64, u64) {
    let Ok(entries) = std::fs::read_dir(path) else {
        return (false, 0, 0);
    };
    let mut files = 0u64;
    let mut bytes = 0u64;
    for entry in entries.flatten() {
        // Flat by construction (composer writes files directly inside).
        if entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
            files += 1;
            bytes += entry.metadata().map(|m| m.len()).unwrap_or(0);
        }
    }
    (true, files, bytes)
}

#[derive(Serialize)]
pub struct ScreenshotDirStat {
    pub kind: String,
    pub path: String,
    pub exists: bool,
    pub files: u64,
    pub bytes: u64,
}

/// `hermes:screenshotDirStats` — size/count for BOTH managed locations so the
/// settings page can show (and clean) each. `project_dir` is the renderer's
/// current workspace root; absent → the project row is omitted.
#[tauri::command]
pub fn screenshot_dir_stats(
    app: AppHandle,
    project_dir: Option<String>,
) -> Result<Vec<ScreenshotDirStat>, String> {
    let app_data = composer_images_dir(&app)?;
    let mut out = Vec::new();

    let (exists, files, bytes) = dir_stats(&app_data);
    out.push(ScreenshotDirStat {
        kind: "app_data".to_string(),
        path: app_data.to_string_lossy().to_string(),
        exists,
        files,
        bytes,
    });

    if let Some(project) = project_dir.filter(|p| !p.trim().is_empty()) {
        let dir = PathBuf::from(project).join(PROJECT_SCREENSHOTS_REL);
        let (exists, files, bytes) = dir_stats(&dir);
        out.push(ScreenshotDirStat {
            kind: "project".to_string(),
            path: dir.to_string_lossy().to_string(),
            exists,
            files,
            bytes,
        });
    }

    Ok(out)
}

#[derive(Serialize)]
pub struct ScreenshotCleanResult {
    pub path: String,
    pub removed_files: u64,
    pub freed_bytes: u64,
}

/// `hermes:clearScreenshotDir` — empty ONE managed screenshot directory
/// (files only; the directory itself stays). Refuses any path that is not a
/// managed location — this is a delete operation behind a confirm dialog.
#[tauri::command]
pub fn clear_screenshot_dir(app: AppHandle, path: String) -> Result<ScreenshotCleanResult, String> {
    let dir = PathBuf::from(&path);
    if !is_managed_screenshot_dir(&app, &dir) {
        return Err(format!("refusing to clean unmanaged path: {path}"));
    }

    let mut removed_files = 0u64;
    let mut freed_bytes = 0u64;
    let entries = std::fs::read_dir(&dir).map_err(|e| format!("read dir: {e}"))?;
    for entry in entries.flatten() {
        let p = entry.path();
        if entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
            freed_bytes += entry.metadata().map(|m| m.len()).unwrap_or(0);
            std::fs::remove_file(&p).map_err(|e| format!("remove {}: {e}", p.display()))?;
            removed_files += 1;
        }
    }

    Ok(ScreenshotCleanResult {
        path,
        removed_files,
        freed_bytes,
    })
}

// ---------------------------------------------------------------------------
// git exclude (.git/info/exclude — never the user's .gitignore)
// ---------------------------------------------------------------------------

/// Pure core: return the new info/exclude content with `entry` appended, or
/// None when it is already listed (line-exact match, so "screenshots" and
/// ".hermes/screenshots/" stay distinct, matching git's own semantics).
fn exclude_entry(existing: &str, entry: &str) -> Option<String> {
    if existing.lines().any(|line| line.trim() == entry) {
        return None;
    }
    let mut next = existing.to_string();
    if !next.is_empty() && !next.ends_with('\n') {
        next.push('\n');
    }
    next.push_str(entry);
    next.push('\n');
    Some(next)
}

/// Resolve the COMMON git dir (the one whose info/exclude git actually reads)
/// for a repo or linked worktree checkout.
fn resolve_common_git_dir(repo_path: &Path) -> Option<PathBuf> {
    let dotgit = repo_path.join(".git");
    if dotgit.is_dir() {
        return Some(dotgit);
    }
    // Linked worktree: .git is a file "gitdir: <path>", and that gitdir's
    // `commondir` file points at the shared one.
    let content = std::fs::read_to_string(dotgit).ok()?;
    let gitdir_raw = content.strip_prefix("gitdir:")?.trim();
    let gitdir = if Path::new(gitdir_raw).is_absolute() {
        PathBuf::from(gitdir_raw)
    } else {
        repo_path.join(gitdir_raw)
    };
    if let Ok(common) = std::fs::read_to_string(gitdir.join("commondir")) {
        let common = common.trim();
        let common_path = if Path::new(common).is_absolute() {
            PathBuf::from(common)
        } else {
            gitdir.join(common)
        };
        return Some(common_path);
    }
    Some(gitdir)
}

/// `hermes:excludePathFromGit` — idempotently append a repo-local exclude
/// entry (`.git/info/exclude`). Returns "added" or "already-present".
#[tauri::command]
pub fn exclude_path_from_git(repo_path: String, rel_entry: String) -> Result<String, String> {
    let repo = PathBuf::from(&repo_path);
    let git_dir = resolve_common_git_dir(&repo)
        .ok_or_else(|| format!("not a git repository: {repo_path}"))?;

    let info = git_dir.join("info");
    std::fs::create_dir_all(&info).map_err(|e| format!("create git info dir: {e}"))?;
    let exclude_file = info.join("exclude");
    let existing = std::fs::read_to_string(&exclude_file).unwrap_or_default();

    match exclude_entry(&existing, &rel_entry) {
        None => Ok("already-present".to_string()),
        Some(next) => {
            std::fs::write(&exclude_file, next).map_err(|e| format!("write info/exclude: {e}"))?;
            Ok("added".to_string())
        }
    }
}

// ---------------------------------------------------------------------------
// Overlay window (quick-screenshot floating button)
// ---------------------------------------------------------------------------

fn position_file(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(app
        .path()
        .app_config_dir()
        .map_err(|e| format!("app config dir unavailable: {e}"))?
        .join("screenshot-overlay.json"))
}

fn load_overlay_position(app: &AppHandle) -> Option<(f64, f64)> {
    let path = position_file(app).ok()?;
    let raw: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
    let x = raw.get("x")?.as_f64()?;
    let y = raw.get("y")?.as_f64()?;
    Some((x, y))
}

fn save_overlay_position(app: &AppHandle, x: f64, y: f64) -> Result<(), String> {
    let path = position_file(app)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("create config dir: {e}"))?;
    }
    std::fs::write(path, serde_json::json!({ "x": x, "y": y }).to_string())
        .map_err(|e| format!("persist overlay position: {e}"))
}

/// `hermes:setScreenshotOverlayEnabled` — create/destroy the floating button.
///
/// Async + run_on_main_thread: building a webview from inside a sync IPC
/// handler deadlocks WebView2 (the 0.18.5 lesson, same as open_session_window).
#[tauri::command]
pub async fn set_screenshot_overlay_enabled(app: AppHandle, enabled: bool) -> Result<(), String> {
    let app_for_build = app.clone();
    app.run_on_main_thread(move || {
        if !enabled {
            if let Some(win) = app_for_build.get_webview_window(OVERLAY_LABEL) {
                let _ = win.close();
            }
            return;
        }

        if app_for_build.get_webview_window(OVERLAY_LABEL).is_some() {
            return;
        }

        let base = crate::asset_server::base_url(&app_for_build);
        let url = format!("{}/?win=screenshot-overlay", base.trim_end_matches('/'));
        let parsed = match url.parse() {
            Ok(parsed) => parsed,
            Err(err) => {
                eprintln!("[screenshot-overlay] bad url {url:?}: {err}");
                return;
            }
        };

        let mut builder = WebviewWindowBuilder::new(&app_for_build, OVERLAY_LABEL, WebviewUrl::External(parsed))
            .title("Hermes Screenshot")
            .inner_size(56.0, 56.0)
            .resizable(false)
            .maximizable(false)
            .minimizable(false)
            .decorations(false)
            .transparent(true)
            .shadow(false)
            .always_on_top(true)
            .skip_taskbar(true)
            .focused(false);

        if let Some((x, y)) = load_overlay_position(&app_for_build) {
            builder = builder.position(x, y);
        } else if let Ok(Some(monitor)) = app_for_build.primary_monitor() {
            // First-run default: bottom-right of the primary monitor, clear of
            // the usual taskbar strip.
            let size = monitor.size();
            builder = builder.position(
                (size.width as f64 - 88.0).max(0.0),
                (size.height as f64 - 152.0).max(0.0),
            );
        }

        if let Err(err) = builder.build() {
            eprintln!("[screenshot-overlay] failed to build: {err}");
        }
    })
    .map_err(|e| e.to_string())
}

/// `hermes:moveScreenshotOverlayBy` — relative drag move (the overlay renders
/// a frameless circle, so dragging is manual pointer math in the renderer).
#[tauri::command]
pub fn move_screenshot_overlay_by(app: AppHandle, dx: f64, dy: f64) -> Result<(), String> {
    let Some(win) = app.get_webview_window(OVERLAY_LABEL) else {
        return Ok(());
    };
    let pos = win.outer_position().map_err(|e| e.to_string())?;
    win.set_position(tauri::PhysicalPosition::new(
        pos.x as f64 + dx,
        pos.y as f64 + dy,
    ))
    .map_err(|e| e.to_string())
}

/// `hermes:saveScreenshotOverlayPosition` — persist the drop point.
#[tauri::command]
pub fn save_screenshot_overlay_position(app: AppHandle) -> Result<(), String> {
    let Some(win) = app.get_webview_window(OVERLAY_LABEL) else {
        return Ok(());
    };
    let pos = win.outer_position().map_err(|e| e.to_string())?;
    save_overlay_position(&app, pos.x as f64, pos.y as f64)
}

/// `hermes:triggerQuickScreenshot` — overlay click: tell the main window's
/// renderer to capture the screen and open the annotator.
///
/// Deliberately does NOT raise the main window first: the capture must
/// happen while the user's screen still shows whatever they were looking at
/// — surfacing Hermes before the exposure would screenshot Hermes itself
/// (the renderer calls `focus_main_window` AFTER the capture completes).
#[tauri::command]
pub fn trigger_quick_screenshot(app: AppHandle) -> Result<(), String> {
    use tauri::Emitter;

    let Some(main) = app.get_webview_window("main") else {
        return Err("main window is not available".to_string());
    };
    main.emit("hermes:quick-screenshot", ())
        .map_err(|e| format!("notify main window: {e}"))
}

/// `hermes:focusMainWindow` — unminimize + show + focus the main window.
/// Called by the renderer once a quick-capture exposure is done and the
/// annotator is about to open.
#[tauri::command]
pub fn focus_main_window(app: AppHandle) -> Result<(), String> {
    let Some(main) = app.get_webview_window("main") else {
        return Err("main window is not available".to_string());
    };
    let _ = main.unminimize();
    let _ = main.show();
    let _ = main.set_focus();
    Ok(())
}

/// Hide the overlay before a full-screen capture, restore after (called from
/// capture.rs so the timing lives in one place).
pub(crate) fn with_overlay_hidden<T>(app: &AppHandle, capture: impl FnOnce() -> T) -> T {
    let overlay = app.get_webview_window(OVERLAY_LABEL).filter(|w| w.is_visible().unwrap_or(false));
    if let Some(win) = &overlay {
        let _ = win.hide();
        // Give the compositor a beat to actually drop the window — hide() is
        // posted, not synchronous.
        std::thread::sleep(std::time::Duration::from_millis(150));
    }
    let result = capture();
    if let Some(win) = &overlay {
        let _ = win.show();
    }
    result
}

/// Close the overlay when the main window goes away (no orphan floaters).
pub(crate) fn on_main_window_destroyed(app: &AppHandle) {
    if let Some(win) = app.get_webview_window(OVERLAY_LABEL) {
        let _ = win.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exclude_entry_appends_once() {
        let first = exclude_entry("# default\nnode_modules/\n", ".hermes/screenshots/")
            .expect("first append adds the entry");
        assert!(first.ends_with(".hermes/screenshots/\n"));
        assert!(exclude_entry(&first, ".hermes/screenshots/").is_none());
    }

    #[test]
    fn exclude_entry_matches_whole_lines() {
        // A different-but-overlapping entry must not suppress the append.
        let next = exclude_entry(".hermes/\n", ".hermes/screenshots/");
        assert!(next.is_some());
    }

    #[test]
    fn exclude_entry_handles_missing_trailing_newline_and_empty_file() {
        assert_eq!(exclude_entry("node_modules/", "x/"), Some("node_modules/\nx/\n".to_string()));
        assert_eq!(exclude_entry("", "x/"), Some("x/\n".to_string()));
    }

    #[test]
    fn whitelist_accepts_only_managed_shapes() {
        let accepts = |p: &str| has_project_screenshots_suffix(Path::new(p));
        assert!(accepts("C:/repo/.hermes/screenshots"));
        assert!(accepts("/home/u/proj/.hermes/screenshots/"));
        assert!(accepts("C:\\repo\\.hermes\\screenshots"));
        assert!(!accepts("C:/repo/.hermes/other"));
        assert!(!accepts("C:/Users/u"));
        // The bare suffix pair with nothing before it is not a workspace.
        assert!(!accepts(".hermes/screenshots"));
        assert!(!accepts("screenshots"));
    }
}
