//! Hermes Desktop — Git command layer (Tauri).
//!
//! Ports the Electron `git-*` IPC handlers by shelling out to the system `git`
//! binary and parsing its text output (mirroring `simple-git`'s structured
//! reads). Reads degrade to null/empty on a non-repo / remote backend; mutations
//! reject so the renderer can toast.
//!
//! Note on rename resolution: `git` reports renames as `old => new`; we resolve
//! to the NEW path so diff/stage target the real file.

use std::path::Path;

const UNTRACKED_LINE_MAX_BYTES: u64 = 1024 * 1024;
const REVIEW_FILE_CAP: usize = 2_000;
const COMMIT_CONTEXT_DIFF_MAX_CHARS: usize = 120_000;
const COMMIT_CONTEXT_UNTRACKED_MAX: usize = 80;

// ---------------------------------------------------------------------------
// git execution helper
// ---------------------------------------------------------------------------

struct GitOut {
    code: i32,
    stdout: String,
    stderr: String,
}

fn git_run(cwd: &str, args: &[&str]) -> GitOut {
    match std::process::Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
    {
        Ok(out) => GitOut {
            code: out.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        },
        Err(_) => GitOut {
            code: -1,
            stdout: String::new(),
            stderr: "git executable not found on PATH".to_string(),
        },
    }
}

/// `git <args>` in `cwd`; returns trimmed stdout on success, Err on failure.
fn git(cwd: &str, args: &[&str]) -> Result<String, String> {
    let out = git_run(cwd, args);
    if let Some(code) = out.code.checked_sub(0) {
        let _ = code;
    }
    if out.code != 0 {
        return Err(if out.stderr.trim().is_empty() {
            format!("git exited {}", out.code)
        } else {
            out.stderr.trim().to_string()
        });
    }
    Ok(out.stdout)
}

/// Best-effort `git <args>`; returns trimmed stdout or "" on any failure.
fn git_quiet(cwd: &str, args: &[&str]) -> String {
    let out = git_run(cwd, args);
    if out.code != 0 {
        return String::new();
    }
    out.stdout.trim().to_string()
}

// ---------------------------------------------------------------------------
// Parsing helpers
// ---------------------------------------------------------------------------

/// `git for-each-ref` line → (name, timestamp); empty name skipped.
fn parse_ref_lines(out: &str) -> Vec<String> {
    out.lines()
        .map(|l| l.split('\t').next().unwrap_or("").trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

#[derive(Clone)]
struct WorktreeRaw {
    path: String,
    branch: Option<String>,
    detached: bool,
    locked: bool,
}

/// Parse `git worktree list --porcelain`. First record is the main worktree.
fn parse_worktrees(out: &str) -> Vec<WorktreeRaw> {
    let mut trees: Vec<WorktreeRaw> = Vec::new();
    let mut cur: Option<WorktreeRaw> = None;
    for line in out.lines() {
        if let Some(rest) = line.strip_prefix("worktree ") {
            if let Some(c) = cur.take() {
                trees.push(c);
            }
            cur = Some(WorktreeRaw {
                path: rest.trim().to_string(),
                branch: None,
                detached: false,
                locked: false,
            });
        } else if let Some(cur) = cur.as_mut() {
            if let Some(rest) = line.strip_prefix("branch ") {
                cur.branch = Some(rest.trim().trim_start_matches("refs/heads/").to_string());
            } else if line == "detached" {
                cur.detached = true;
            } else if line.starts_with("locked") {
                cur.locked = true;
            }
        }
    }
    if let Some(c) = cur {
        trees.push(c);
    }
    trees
}

fn sanitize_branch(name: &str) -> String {
    name.trim()
        .replace(|c: char| c.is_whitespace(), "-")
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '-' || *c == '.' || *c == '/' || *c == '_')
        .collect::<String>()
}

/// line count of a file (newline bytes +1 for a final unterminated line); 0 for
/// binary (NUL byte) or oversized files. Mirrors Electron's untrackedInsertions.
fn untracked_insertions(base: &str, rel: &str) -> u64 {
    let full = Path::new(base).join(rel);
    let Ok(meta) = std::fs::metadata(&full) else {
        return 0;
    };
    if !meta.is_file() || meta.len() > UNTRACKED_LINE_MAX_BYTES {
        return 0;
    }
    let Ok(buf) = std::fs::read(&full) else {
        return 0;
    };
    if buf.contains(&0) {
        return 0;
    }
    let mut lines = 0u64;
    for &b in &buf {
        if b == 10 {
            lines += 1;
        }
    }
    if !buf.is_empty() && buf[buf.len() - 1] != 10 {
        lines += 1;
    }
    lines
}

/// Resolve repo's default branch NAME, preferring origin/HEAD then common trunks.
fn default_branch_name(cwd: &str) -> Option<String> {
    let head = git_quiet(cwd, &["revparse", "--abbrev-ref", "origin/HEAD"]);
    if !head.is_empty() && head != "origin/HEAD" {
        return Some(head.trim_start_matches("origin/").to_string());
    }
    for ref_ in [
        "refs/heads/main",
        "refs/heads/master",
        "refs/remotes/origin/main",
        "refs/remotes/origin/master",
    ] {
        let out = git_quiet(cwd, &["rev-parse", "--verify", "--quiet", ref_]);
        if !out.is_empty() {
            return Some(
                ref_.trim_start_matches("refs/heads/")
                    .trim_start_matches("refs/remotes/origin/")
                    .to_string(),
            );
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/// `hermes:fs:gitRoot` — walk up from `start_path` to find a `.git` dir.
#[tauri::command]
pub fn git_root(start_path: String) -> Result<Option<String>, String> {
    let mut dir = std::path::PathBuf::from(&start_path);
    if let Ok(meta) = std::fs::metadata(&dir) {
        if !meta.is_dir() {
            dir.pop();
        }
    }
    for _ in 0..50 {
        if dir.join(".git").exists() {
            return Ok(Some(dir.to_string_lossy().into_owned()));
        }
        if !dir.pop() {
            break;
        }
    }
    Ok(None)
}

/// `hermes:git:scanRepos` — walk bounded roots for git repos (fs only).
#[tauri::command]
pub fn scan_repos(
    roots: Vec<String>,
    options: Option<ScanReposOptions>,
) -> Result<Vec<RepoHit>, String> {
    let opts = options.unwrap_or_default();
    if opts.enabled == Some(false) {
        return Ok(Vec::new());
    }
    let max_depth = opts.max_depth.unwrap_or(3).max(0) as usize;
    let home = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_default();
    let search_roots: Vec<std::path::PathBuf> = if roots.is_empty() {
        vec![std::path::PathBuf::from(&home)]
    } else {
        roots.iter().map(|r| expand_home(r, &home)).collect()
    };

    let mut found: Vec<RepoHit> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

    for root in search_roots {
        walk_scan(&root, 0, max_depth, &mut found, &mut seen);
    }

    Ok(found)
}

#[derive(serde::Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ScanReposOptions {
    pub max_depth: Option<i32>,
    pub enabled: Option<bool>,
    #[serde(default)]
    #[allow(dead_code)]
    pub exclude_paths: Option<Vec<String>>,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoHit {
    root: String,
    label: String,
}

fn expand_home(raw: &str, home: &str) -> std::path::PathBuf {
    let trimmed = raw.trim();
    if trimmed == "~" {
        return std::path::PathBuf::from(home);
    }
    if let Some(rest) = trimmed
        .strip_prefix("~/")
        .or_else(|| trimmed.strip_prefix("~\\"))
    {
        return std::path::PathBuf::from(home).join(rest);
    }
    let p = std::path::PathBuf::from(trimmed);
    if p.is_absolute() {
        p
    } else {
        std::path::PathBuf::from(home).join(p)
    }
}

const JUNK_DIRS: [&str; 6] = [
    "Applications",
    "Library",
    "node_modules",
    "site-packages",
    "vendor",
    "venv",
];

fn walk_scan(
    dir: &std::path::Path,
    depth: usize,
    max_depth: usize,
    found: &mut Vec<RepoHit>,
    seen: &mut std::collections::HashSet<String>,
) {
    if depth > max_depth {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut has_git = false;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name == ".git" && entry.path().is_dir() {
            has_git = true;
            break;
        }
    }
    if has_git {
        let abs = dir.to_string_lossy().into_owned();
        let key = abs.to_lowercase();
        if seen.insert(key) {
            let label = dir
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| abs.clone());
            found.push(RepoHit { root: abs, label });
        }
        return;
    }
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || JUNK_DIRS.contains(&name.as_str()) {
            continue;
        }
        if entry.path().is_dir() {
            walk_scan(&entry.path(), depth + 1, max_depth, found, seen);
        }
    }
}

/// `hermes:git:worktreeList`
#[tauri::command]
pub fn worktree_list(repo_path: String) -> Result<Vec<WorktreeEntry>, String> {
    let out = git_quiet(&repo_path, &["worktree", "list", "--porcelain"]);
    if out.is_empty() {
        return Ok(Vec::new());
    }
    Ok(parse_worktrees(&out)
        .into_iter()
        .enumerate()
        .map(|(i, t)| WorktreeEntry {
            path: t.path,
            branch: t.branch,
            is_main: i == 0,
            detached: t.detached,
            locked: t.locked,
        })
        .collect())
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeEntry {
    path: String,
    branch: Option<String>,
    is_main: bool,
    detached: bool,
    locked: bool,
}

/// `hermes:git:branchList` — local branches, most-recently-committed first.
#[tauri::command]
pub fn branch_list(repo_path: String) -> Result<Vec<BranchEntry>, String> {
    let out = git_quiet(
        &repo_path,
        &[
            "for-each-ref",
            "--format=%(refname:short)",
            "--sort=-committerdate",
            "refs/heads",
        ],
    );
    if out.is_empty() {
        return Ok(Vec::new());
    }
    let trees = worktree_list(repo_path.clone()).unwrap_or_default();
    let path_by_branch: std::collections::HashMap<&str, &str> = trees
        .iter()
        .filter_map(|t| t.branch.as_deref().map(|b| (b, t.path.as_str())))
        .collect();
    let trunk = default_branch_name(&repo_path);
    Ok(parse_ref_lines(&out)
        .into_iter()
        .map(|name| {
            let checked_out = path_by_branch.contains_key(name.as_str());
            BranchEntry {
                name: name.clone(),
                checked_out,
                is_default: trunk.as_deref() == Some(name.as_str()),
                worktree_path: path_by_branch.get(name.as_str()).map(|s| s.to_string()),
            }
        })
        .collect())
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BranchEntry {
    name: String,
    checked_out: bool,
    is_default: bool,
    worktree_path: Option<String>,
}

/// `hermes:git:baseBranchList` — local heads + remote-tracking refs.
#[tauri::command]
pub fn base_branch_list(repo_path: String) -> Result<Vec<BaseBranchEntry>, String> {
    let out = git_quiet(
        &repo_path,
        &[
            "for-each-ref",
            "--format=%(refname:short)",
            "--sort=-committerdate",
            "refs/heads",
            "refs/remotes",
        ],
    );
    if out.is_empty() {
        return Ok(Vec::new());
    }
    let remote_default = git_quiet(
        &repo_path,
        &[
            "symbolic-ref",
            "--quiet",
            "--short",
            "refs/remotes/origin/HEAD",
        ],
    );
    let local_default = default_branch_name(&repo_path);
    Ok(parse_ref_lines(&out)
        .into_iter()
        .map(|name| {
            let is_default = (!remote_default.is_empty() && name == remote_default)
                || (remote_default.is_empty() && local_default.as_deref() == Some(name.as_str()));
            BaseBranchEntry {
                name: name.clone(),
                is_remote: name.starts_with("origin/"),
                is_default,
            }
        })
        .collect())
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BaseBranchEntry {
    name: String,
    is_remote: bool,
    is_default: bool,
}

/// `hermes:git:branchSwitch`
#[tauri::command]
pub fn branch_switch(repo_path: String, branch: String) -> Result<serde_json::Value, String> {
    let target = sanitize_branch(&branch);
    if target.is_empty() {
        return Err("Branch name is required.".to_string());
    }
    git(&repo_path, &["switch", &target])?;
    Ok(serde_json::json!({ "branch": target }))
}

/// `hermes:git:repoStatus` — compact working-tree status for the coding rail.
#[tauri::command]
pub fn repo_status(repo_path: String) -> Result<Option<RepoStatus>, String> {
    let path = std::path::Path::new(&repo_path);
    if !path.is_dir() {
        return Ok(None);
    }
    let out = git_quiet(
        &repo_path,
        &[
            "status",
            "--porcelain=v2",
            "--branch",
            "--untracked-files=normal",
        ],
    );
    if out.is_empty() {
        return Ok(None);
    }

    let mut branch: Option<String> = None;
    let mut ahead = 0i64;
    let mut behind = 0i64;
    let mut files: Vec<RepoStatusFile> = Vec::new();
    let mut untracked: Vec<String> = Vec::new();

    for line in out.lines() {
        if let Some(rest) = line.strip_prefix("# branch.head ") {
            branch = Some(rest.trim().to_string());
        } else if let Some(rest) = line.strip_prefix("# branch.ab ") {
            let parts: Vec<&str> = rest.trim().split_whitespace().collect();
            if let Some(a) = parts
                .get(0)
                .and_then(|s| s.strip_prefix('+'))
                .and_then(|n| n.parse().ok())
            {
                ahead = a;
            }
            if let Some(b) = parts
                .get(1)
                .and_then(|s| s.strip_prefix('-'))
                .and_then(|n| n.parse().ok())
            {
                behind = b;
            }
        } else if let Some(rest) = line.strip_prefix("? ") {
            untracked.push(rest.trim().to_string());
        } else if line.starts_with("1 ") || line.starts_with("2 ") {
            // 1 <XY> <sub> <mH> <mI> <mW> <hH> <hI> <path>
            let cols: Vec<&str> = line.split_ascii_whitespace().collect();
            if cols.len() >= 3 {
                let xy = cols[1].as_bytes();
                let x = xy.get(0).copied().unwrap_or(b'.') as char;
                let y = xy.get(1).copied().unwrap_or(b'.') as char;
                let path = if cols[0] == "2" {
                    cols.get(9).copied().unwrap_or("")
                } else {
                    cols.get(8).copied().unwrap_or("")
                };
                files.push(RepoStatusFile {
                    path: path.to_string(),
                    staged: x != '.',
                    unstaged: y != '.',
                    untracked: false,
                    conflicted: x == 'U' || y == 'U',
                });
            }
        } else if line.starts_with("u ") {
            // u <XY> <sub> <m1> <m2> <m3> <mW> <h1> <h2> <h3> <path>
            let cols: Vec<&str> = line.split_ascii_whitespace().collect();
            if let Some(p) = cols.get(10) {
                files.push(RepoStatusFile {
                    path: p.to_string(),
                    staged: false,
                    unstaged: false,
                    untracked: false,
                    conflicted: true,
                });
            }
        }
    }

    let detached = branch.is_none();
    let staged = files.iter().filter(|f| f.staged).count();
    let unstaged = files.iter().filter(|f| f.unstaged).count();
    let conflicted = files.iter().filter(|f| f.conflicted).count();

    // +/- vs HEAD (tracked only).
    let (added, removed) = diff_numstat(&repo_path);

    // Fold top-level untracked file insertions into `added` (like Electron).
    let mut added_total = added;
    for u in untracked.iter().take(500) {
        added_total += untracked_insertions(&repo_path, u);
    }
    files.truncate(200);

    Ok(Some(RepoStatus {
        branch,
        default_branch: default_branch_name(&repo_path),
        detached,
        ahead,
        behind,
        staged,
        unstaged,
        untracked: untracked.len(),
        conflicted,
        changed: files.len(),
        added: added_total,
        removed,
        files,
    }))
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoStatusFile {
    path: String,
    staged: bool,
    unstaged: bool,
    untracked: bool,
    conflicted: bool,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoStatus {
    branch: Option<String>,
    default_branch: Option<String>,
    detached: bool,
    ahead: i64,
    behind: i64,
    staged: usize,
    unstaged: usize,
    untracked: usize,
    conflicted: usize,
    changed: usize,
    added: u64,
    removed: u64,
    files: Vec<RepoStatusFile>,
}

/// Sum insertions/deletions from `git diff HEAD --numstat`.
fn diff_numstat(cwd: &str) -> (u64, u64) {
    let out = git_quiet(cwd, &["diff", "HEAD", "--numstat"]);
    let mut added = 0u64;
    let mut removed = 0u64;
    for line in out.lines() {
        let cols: Vec<&str> = line.split('\t').collect();
        if cols.len() < 3 {
            continue;
        }
        if let Ok(a) = cols[0].parse::<u64>() {
            added += a;
        }
        if let Ok(r) = cols[1].parse::<u64>() {
            removed += r;
        }
    }
    (added, removed)
}

/// `hermes:git:fileDiff` — working-tree-vs-HEAD diff for one file.
#[tauri::command]
pub fn file_diff(repo_path: String, file_path: String) -> Result<String, String> {
    let head = git_quiet(&repo_path, &["diff", "HEAD", "--", &file_path]);
    if !head.is_empty() {
        return Ok(head);
    }
    // No tracked changes vs HEAD. Synthesize an all-add diff only for a file git
    // doesn't know yet (untracked).
    let status = git_quiet(&repo_path, &["status", "--porcelain", "--", &file_path]);
    if !status.starts_with("??") {
        return Ok(String::new());
    }
    let out = git_run(
        &repo_path,
        &["diff", "--no-index", "--", "/dev/null", &file_path],
    );
    Ok(out.stdout)
}

/// `hermes:git:worktreeRemove`
#[tauri::command]
pub fn worktree_remove(
    repo_path: String,
    worktree_path: String,
    options: Option<WorktreeRemoveOptions>,
) -> Result<serde_json::Value, String> {
    let root = main_root(&repo_path).unwrap_or(repo_path.clone());
    let mut args = vec!["worktree", "remove"];
    if options.and_then(|o| o.force).unwrap_or(false) {
        args.push("--force");
    }
    args.push(&worktree_path);
    git(&root, &args)?;
    Ok(serde_json::json!({ "removed": worktree_path }))
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeRemoveOptions {
    force: Option<bool>,
}

fn main_root(cwd: &str) -> Option<String> {
    let list = worktree_list(cwd.to_string()).ok()?;
    list.into_iter().find(|t| t.is_main).map(|t| t.path)
}

// ---------------------------------------------------------------------------
// Vec<String> -based git helpers (for dynamically-built argument lists)
// ---------------------------------------------------------------------------

fn git_args(cwd: &str, args: &[String]) -> Result<String, String> {
    let strs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    git(cwd, &strs)
}

fn git_quiet_args(cwd: &str, args: &[String]) -> String {
    let strs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    git_quiet(cwd, &strs)
}

/// Resolve simple-git's `old => new` / `dir/{old => new}/f` rename to the NEW path.
fn resolve_rename(raw: &str) -> String {
    let path = raw.trim();
    if !path.contains(" => ") {
        return path.to_string();
    }
    if let (Some(open), Some(arrow), Some(close)) =
        (path.find('{'), path.find(" => "), path.find('}'))
    {
        if open < arrow && arrow < close {
            let prefix = &path[..open];
            let to = &path[arrow + 4..close];
            let suffix = &path[close + 1..];
            let mut s = format!("{prefix}{to}{suffix}");
            while s.contains("//") {
                s = s.replace("//", "/");
            }
            return s;
        }
    }
    path.split(" => ")
        .last()
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

/// `git diff --numstat <args…>` → per-resolved-path (added, removed) counts.
fn diff_numstat_files(cwd: &str, args: &[String]) -> Vec<(String, u64, u64)> {
    let mut full = vec!["diff".to_string(), "--numstat".to_string()];
    full.extend_from_slice(args);
    let out = git_quiet_args(cwd, &full);
    let mut res = Vec::new();
    for line in out.lines() {
        let cols: Vec<&str> = line.split('\t').collect();
        if cols.len() < 3 {
            continue;
        }
        let added = cols[0].parse::<u64>().unwrap_or(0);
        let removed = cols[1].parse::<u64>().unwrap_or(0);
        res.push((resolve_rename(cols[2]), added, removed));
    }
    res
}

/// `git diff <args…>` → raw stdout (empty on failure).
fn diff_quiet(cwd: &str, args: &[String]) -> String {
    let mut full = vec!["diff".to_string()];
    full.extend_from_slice(args);
    git_quiet_args(cwd, &full)
}

/// Merge-base of HEAD against the remote default branch (origin/HEAD), falling
/// back to common trunk refs. Mirrors `branchBase`.
fn branch_base(cwd: &str) -> Option<String> {
    let mut candidates: Vec<String> = Vec::new();
    let head = git_quiet(cwd, &["revparse", "--abbrev-ref", "origin/HEAD"]);
    if !head.is_empty() {
        candidates.push(head);
    }
    for trunk in ["origin/main", "origin/master", "main", "master"] {
        candidates.push(trunk.to_string());
    }
    for ref_ in candidates {
        let out = git_quiet(cwd, &["merge-base", "HEAD", &ref_]);
        if !out.is_empty() {
            return Some(out);
        }
    }
    None
}

fn current_branch(cwd: &str) -> Option<String> {
    let out = git_quiet(cwd, &["symbolic-ref", "--short", "HEAD"]);
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

fn has_upstream(cwd: &str) -> bool {
    let out = git_run(
        cwd,
        &["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"],
    );
    out.code == 0 && !out.stdout.trim().is_empty()
}

// ---------------------------------------------------------------------------
// Review commands (port of electron/git-review-ops.ts)
// ---------------------------------------------------------------------------

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewFile {
    path: String,
    added: u64,
    removed: u64,
    status: String,
    staged: bool,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewList {
    files: Vec<ReviewFile>,
    base: Option<String>,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewShipInfo {
    gh_ready: bool,
    pr: Option<ReviewPr>,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewPr {
    url: String,
    state: String,
    number: i64,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitContext {
    diff: String,
    recent: String,
}

struct StatusEntry {
    path: String,
    index: char,
    working: char,
    untracked: bool,
}

fn parse_status_v2(line: &str) -> Option<StatusEntry> {
    if let Some(rest) = line.strip_prefix("? ") {
        return Some(StatusEntry {
            path: rest.trim().to_string(),
            index: '?',
            working: '?',
            untracked: true,
        });
    }
    if line.starts_with("1 ") || line.starts_with("2 ") {
        let cols: Vec<&str> = line.split_ascii_whitespace().collect();
        if cols.len() < 3 {
            return None;
        }
        let index = cols[1].chars().next().unwrap_or(' ');
        let working = cols[1].chars().nth(1).unwrap_or(' ');
        let path = if cols[0] == "2" {
            cols.get(9).copied().unwrap_or("")
        } else {
            cols.get(8).copied().unwrap_or("")
        };
        return Some(StatusEntry {
            path: resolve_rename(path),
            index,
            working,
            untracked: false,
        });
    }
    if line.starts_with("u ") {
        let cols: Vec<&str> = line.split_ascii_whitespace().collect();
        if let Some(p) = cols.get(10) {
            return Some(StatusEntry {
                path: p.to_string(),
                index: 'U',
                working: 'U',
                untracked: false,
            });
        }
    }
    None
}

fn status_letter(e: &StatusEntry) -> String {
    if e.untracked {
        return "?".to_string();
    }
    let code = if e.index != ' ' { e.index } else { e.working };
    let c = if code == ' ' { 'M' } else { code };
    c.to_uppercase().to_string()
}

fn fill_untracked_counts(cwd: &str, files: &mut [ReviewFile]) {
    for f in files.iter_mut() {
        if f.status == "?" && f.added == 0 && f.removed == 0 {
            f.added = untracked_insertions(cwd, &f.path);
        }
    }
}

fn review_list_uncommitted(cwd: &str) -> ReviewList {
    let status_out = git_quiet(
        cwd,
        &["status", "--porcelain=v2", "--untracked-files=normal"],
    );
    let staged = diff_numstat_files(cwd, &["--cached".to_string()]);
    let unstaged = diff_numstat_files(cwd, &[]);
    let staged_by: std::collections::HashMap<String, (u64, u64)> =
        staged.into_iter().map(|(p, a, r)| (p, (a, r))).collect();
    let unstaged_by: std::collections::HashMap<String, (u64, u64)> =
        unstaged.into_iter().map(|(p, a, r)| (p, (a, r))).collect();

    let mut files = Vec::new();
    for line in status_out.lines() {
        let Some(entry) = parse_status_v2(line) else {
            continue;
        };
        let sc = staged_by.get(&entry.path).copied().unwrap_or((0, 0));
        let uc = unstaged_by.get(&entry.path).copied().unwrap_or((0, 0));
        let staged = entry.index != ' ' && entry.index != '?';
        let status = status_letter(&entry);
        files.push(ReviewFile {
            path: entry.path,
            added: sc.0 + uc.0,
            removed: sc.1 + uc.1,
            status,
            staged,
        });
    }
    files.truncate(REVIEW_FILE_CAP);
    files.sort_by(|a, b| a.path.cmp(&b.path));
    fill_untracked_counts(cwd, &mut files);
    ReviewList { files, base: None }
}

/// `git.review.list` — changed files per scope (uncommitted / branch / lastTurn).
#[tauri::command]
pub fn git_review_list(
    repo_path: String,
    scope: String,
    base_ref: Option<String>,
) -> Result<ReviewList, String> {
    let cwd = repo_path;

    if scope == "branch" || scope == "lastTurn" {
        let b = if scope == "branch" {
            branch_base(&cwd)
        } else {
            base_ref.clone()
        };
        let Some(b) = b else {
            return Ok(ReviewList {
                files: Vec::new(),
                base: None,
            });
        };
        let range = if scope == "branch" {
            format!("{b}...HEAD")
        } else {
            b.clone()
        };
        let mut files: Vec<ReviewFile> = diff_numstat_files(&cwd, &[range])
            .into_iter()
            .map(|(path, added, removed)| ReviewFile {
                path,
                added,
                removed,
                status: "M".to_string(),
                staged: false,
            })
            .collect();
        files.truncate(REVIEW_FILE_CAP);

        if scope == "lastTurn" && files.len() < REVIEW_FILE_CAP {
            let status_out = git_quiet(
                &cwd,
                &["status", "--porcelain=v2", "--untracked-files=normal"],
            );
            let mut known: std::collections::HashSet<String> =
                files.iter().map(|f| f.path.clone()).collect();
            for line in status_out.lines() {
                if files.len() >= REVIEW_FILE_CAP {
                    break;
                }
                if let Some(rest) = line.strip_prefix("? ") {
                    let p = rest.trim().to_string();
                    if known.insert(p.clone()) {
                        files.push(ReviewFile {
                            path: p,
                            added: 0,
                            removed: 0,
                            status: "?".to_string(),
                            staged: false,
                        });
                    }
                }
            }
        }

        files.sort_by(|a, b| a.path.cmp(&b.path));
        fill_untracked_counts(&cwd, &mut files);
        return Ok(ReviewList {
            files,
            base: Some(b),
        });
    }

    Ok(review_list_uncommitted(&cwd))
}

/// `git.review.diff` — per-file unified diff for a scope / staged flag.
#[tauri::command]
pub fn git_review_diff(
    repo_path: String,
    file_path: String,
    scope: String,
    base_ref: Option<String>,
    staged: Option<bool>,
) -> Result<String, String> {
    let cwd = repo_path;

    if scope == "branch" {
        return Ok(branch_base(&cwd)
            .map(|b| {
                diff_quiet(
                    &cwd,
                    &[format!("{b}...HEAD"), "--".into(), file_path.clone()],
                )
            })
            .unwrap_or_default());
    }
    if scope == "lastTurn" {
        return Ok(base_ref
            .map(|b| diff_quiet(&cwd, &[b, "--".into(), file_path.clone()]))
            .unwrap_or_default());
    }
    if staged.unwrap_or(false) {
        return Ok(diff_quiet(
            &cwd,
            &["--cached".into(), "--".into(), file_path],
        ));
    }

    let worktree = diff_quiet(&cwd, &["--".into(), file_path.clone()]);
    if !worktree.trim().is_empty() {
        return Ok(worktree);
    }
    // Untracked file: synthesize an all-add diff (exits non-zero by design).
    let out = git_run(&cwd, &["diff", "--no-index", "--", "/dev/null", &file_path]);
    Ok(out.stdout)
}

#[tauri::command]
pub fn git_review_stage(
    repo_path: String,
    file_path: Option<String>,
) -> Result<serde_json::Value, String> {
    if let Some(fp) = file_path {
        git(&repo_path, &["add", "--", &fp])?;
    } else {
        git(&repo_path, &["add", "-A"])?;
    }
    Ok(serde_json::json!({ "ok": true }))
}

#[tauri::command]
pub fn git_review_unstage(
    repo_path: String,
    file_path: Option<String>,
) -> Result<serde_json::Value, String> {
    if let Some(fp) = file_path {
        git(&repo_path, &["reset", "-q", "HEAD", "--", &fp])?;
    } else {
        git(&repo_path, &["reset", "-q", "HEAD"])?;
    }
    Ok(serde_json::json!({ "ok": true }))
}

#[tauri::command]
pub fn git_review_revert(
    repo_path: String,
    file_path: Option<String>,
) -> Result<serde_json::Value, String> {
    if let Some(fp) = file_path {
        let _ = git_quiet(&repo_path, &["checkout", "HEAD", "--", &fp]);
        let _ = git_quiet(&repo_path, &["clean", "-fd", "--", &fp]);
    } else {
        let _ = git_quiet(&repo_path, &["checkout", "HEAD", "--", "."]);
        let _ = git_quiet(&repo_path, &["clean", "-fd"]);
    }
    Ok(serde_json::json!({ "ok": true }))
}

#[tauri::command]
pub fn git_review_rev_parse(
    repo_path: String,
    reference: Option<String>,
) -> Result<Option<String>, String> {
    let ref_name = reference.unwrap_or_else(|| "HEAD".to_string());
    let out = git_run(&repo_path, &["rev-parse", &ref_name]);
    if out.code != 0 {
        return Ok(None);
    }
    let trimmed = out.stdout.trim().to_string();
    Ok(if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    })
}

fn review_push_impl(cwd: &str) -> Result<(), String> {
    if has_upstream(cwd) {
        git(cwd, &["push"])?;
    } else if let Some(branch) = current_branch(cwd) {
        git_args(cwd, &["push".into(), "-u".into(), "origin".into(), branch])?;
    }
    Ok(())
}

#[tauri::command]
pub fn git_review_commit(
    repo_path: String,
    message: String,
    push: bool,
) -> Result<serde_json::Value, String> {
    let staged = !git_quiet(&repo_path, &["diff", "--cached", "--name-only"]).is_empty();
    if !staged {
        git(&repo_path, &["add", "-A"])?;
    }
    git(&repo_path, &["commit", "-m", &message])?;
    if push {
        review_push_impl(&repo_path)?;
    }
    Ok(serde_json::json!({ "ok": true }))
}

fn cap_text(text: &str, max_chars: usize, label: &str) -> String {
    if text.len() <= max_chars {
        return text.to_string();
    }
    format!(
        "{}\n# {label}: {} chars omitted\n",
        &text[..max_chars],
        text.len() - max_chars
    )
}

/// `git.review.commitContext` — diff (staged-or-all) + recent commit subjects.
#[tauri::command]
pub fn git_review_commit_context(repo_path: String) -> Result<CommitContext, String> {
    let cwd = repo_path;
    let status_out = git_quiet(&cwd, &["status", "--porcelain=v2", "--untracked-files=all"]);

    let staged = parse_status_v2_lines(&status_out)
        .iter()
        .any(|e| e.index != ' ' && e.index != '?');
    let mut diff = if staged {
        diff_quiet(&cwd, &["--cached".to_string()])
    } else {
        diff_quiet(&cwd, &["HEAD".to_string()])
    };
    diff = cap_text(
        &diff,
        COMMIT_CONTEXT_DIFF_MAX_CHARS,
        "diff truncated for commit-message generation",
    );

    let untracked: Vec<String> = parse_status_v2_lines(&status_out)
        .into_iter()
        .filter(|e| e.untracked)
        .map(|e| e.path)
        .collect();
    if !untracked.is_empty() {
        let visible: Vec<&str> = untracked
            .iter()
            .take(COMMIT_CONTEXT_UNTRACKED_MAX)
            .map(|s| s.as_str())
            .collect();
        let omitted = untracked.len() - visible.len();
        let mut note = format!(
            "\n# New (untracked) files:\n{}",
            visible
                .iter()
                .map(|p| format!("#   {p}"))
                .collect::<Vec<_>>()
                .join("\n")
        );
        if omitted > 0 {
            note.push_str(&format!("\n#   ... {omitted} more omitted"));
        }
        note.push('\n');
        diff = if diff.is_empty() {
            note
        } else {
            format!("{diff}{note}")
        };
    }

    let recent = git_quiet(&cwd, &["log", "-n", "10", "--pretty=format:%s"])
        .trim()
        .to_string();
    Ok(CommitContext { diff, recent })
}

fn parse_status_v2_lines(out: &str) -> Vec<StatusEntry> {
    out.lines().filter_map(parse_status_v2).collect()
}

#[tauri::command]
pub fn git_review_push(repo_path: String) -> Result<serde_json::Value, String> {
    review_push_impl(&repo_path)?;
    Ok(serde_json::json!({ "ok": true }))
}

/// `gh` invocation helper (mirrors Electron's runGh).
struct GhOut {
    ok: bool,
    stdout: String,
}

fn gh_run(cwd: &str, args: &[&str]) -> GhOut {
    match std::process::Command::new("gh")
        .args(args)
        .current_dir(cwd)
        .output()
    {
        Ok(o) => GhOut {
            ok: o.status.success(),
            stdout: String::from_utf8_lossy(&o.stdout).into_owned(),
        },
        Err(_) => GhOut {
            ok: false,
            stdout: String::new(),
        },
    }
}

#[tauri::command]
pub fn git_review_ship_info(repo_path: String) -> Result<ReviewShipInfo, String> {
    let auth = gh_run(&repo_path, &["auth", "status"]);
    if !auth.ok {
        return Ok(ReviewShipInfo {
            gh_ready: false,
            pr: None,
        });
    }
    let view = gh_run(&repo_path, &["pr", "view", "--json", "url,state,number"]);
    if !view.ok {
        return Ok(ReviewShipInfo {
            gh_ready: true,
            pr: None,
        });
    }
    match serde_json::from_str::<serde_json::Value>(&view.stdout) {
        Ok(v) => {
            let pr = v.get("url").and_then(|u| u.as_str()).map(|url| ReviewPr {
                url: url.to_string(),
                state: v
                    .get("state")
                    .and_then(|s| s.as_str())
                    .unwrap_or("")
                    .to_string(),
                number: v.get("number").and_then(|n| n.as_i64()).unwrap_or(0),
            });
            Ok(ReviewShipInfo { gh_ready: true, pr })
        }
        Err(_) => Ok(ReviewShipInfo {
            gh_ready: true,
            pr: None,
        }),
    }
}

/// `git.review.createPr` — push the branch, then `gh pr create --fill`.
#[tauri::command]
pub fn git_review_create_pr(repo_path: String) -> Result<serde_json::Value, String> {
    let _ = review_push_impl(&repo_path);
    let created = gh_run(&repo_path, &["pr", "create", "--fill"]);
    if !created.ok {
        return Err("gh pr create failed (is gh installed and authenticated?)".into());
    }
    let url = created
        .stdout
        .trim()
        .split('\n')
        .filter(|s| !s.trim().is_empty())
        .next_back()
        .unwrap_or("")
        .to_string();
    Ok(serde_json::json!({ "url": url }))
}

// ---------------------------------------------------------------------------
// Worktree add (port of electron/git-worktree-ops.ts)
// ---------------------------------------------------------------------------

#[derive(serde::Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeAddOptions {
    name: Option<String>,
    branch: Option<String>,
    base: Option<String>,
    existing_branch: Option<String>,
}

fn slugify_work(name: &str) -> String {
    let mut slug: String = name
        .trim()
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect();
    while slug.starts_with('-') {
        slug.remove(0);
    }
    while slug.ends_with('-') {
        slug.pop();
    }
    slug.truncate(40);
    if slug.is_empty() {
        "work".to_string()
    } else {
        slug
    }
}

fn unique_worktree_dir(root: &str, name: &str) -> String {
    let mut n = 1usize;
    loop {
        let dir = Path::new(root).join(".worktrees").join(if n == 1 {
            name.to_string()
        } else {
            format!("{name}-{n}")
        });
        if !dir.exists() {
            return dir.to_string_lossy().into_owned();
        }
        n += 1;
    }
}

fn ensure_git_repo(dir: &str) -> Result<(), String> {
    let inside = git_quiet(dir, &["rev-parse", "--is-inside-work-tree"]);
    let mut needs_root = false;
    if inside == "true" {
        if git_quiet(dir, &["rev-parse", "--verify", "HEAD"]).is_empty() {
            needs_root = true;
        }
    } else {
        git(dir, &["init"])?;
        needs_root = true;
    }
    if needs_root {
        git(
            dir,
            &[
                "-c",
                "user.email=hermes@localhost",
                "-c",
                "user.name=Hermes",
                "commit",
                "--allow-empty",
                "-m",
                "Initial commit",
            ],
        )?;
    }
    Ok(())
}

fn add_existing_branch_worktree(root: &str, name: &str) -> Result<serde_json::Value, String> {
    let branch = sanitize_branch(name);
    if branch.is_empty() {
        return Err("Branch name is required.".to_string());
    }
    if Some(branch.as_str()) == default_branch_name(root).as_deref() {
        git(root, &["switch", &branch])?;
        return Ok(serde_json::json!({ "path": root, "branch": branch, "repoRoot": root }));
    }
    let dir = unique_worktree_dir(root, &slugify_work(&branch));
    git_args(
        root,
        &["worktree".into(), "add".into(), dir.clone(), branch.clone()],
    )?;
    Ok(serde_json::json!({ "path": dir, "branch": branch, "repoRoot": root }))
}

/// `git.worktreeAdd` — create a fresh worktree (or check out an existing branch).
#[tauri::command]
pub fn git_worktree_add(
    repo_path: String,
    options: Option<WorktreeAddOptions>,
) -> Result<serde_json::Value, String> {
    let opts = options.unwrap_or_default();

    if let Some(existing) = opts.existing_branch {
        return add_existing_branch_worktree(&repo_path, &existing);
    }

    ensure_git_repo(&repo_path)?;
    let root = main_root(&repo_path).unwrap_or(repo_path.clone());

    let slug = slugify_work(&opts.name.unwrap_or_else(|| format!("work-{:x}", now_ts())));
    let branch = sanitize_branch(&opts.branch.unwrap_or_default());
    let branch = if branch.is_empty() {
        format!("hermes/{slug}")
    } else {
        branch
    };
    let dir = unique_worktree_dir(&root, &slug);

    let mut args: Vec<String> = vec![
        "worktree".into(),
        "add".into(),
        "-b".into(),
        branch.clone(),
        dir.clone(),
    ];
    if let Some(base) = opts.base {
        if let Some(remote) = base.strip_prefix("origin/") {
            let _ = git_quiet(&root, &["fetch", "origin", remote]);
            args.push("--no-track".into());
        }
        args.push(base);
    }

    match git_args(&root, &args) {
        Ok(_) => {}
        Err(err) if err.to_lowercase().contains("already exists") => {
            git_args(
                &root,
                &["worktree".into(), "add".into(), dir.clone(), branch.clone()],
            )?;
        }
        Err(err) => return Err(err),
    }

    Ok(serde_json::json!({ "path": dir, "branch": branch, "repoRoot": root }))
}

fn now_ts() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
