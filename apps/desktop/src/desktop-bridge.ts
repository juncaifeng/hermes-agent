import { invoke } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import type { HermesSelectPathsOptions } from '@/global'

/**
 * Hermes Desktop — Tauri v2 bridge shim.
 *
 * Electron provided `window.hermesDesktop` via `contextBridge` in
 * `electron/preload.ts`. Under Tauri the renderer runs directly in WebView2
 * (no preload), so we install the same object here, mapping every method onto
 * `@tauri-apps/api/core.invoke` (IPC round-trip) or `@tauri-apps/api/event.listen`
 * (main → renderer events). Method names/signatures are preserved so the ~84
 * call sites under `src/` stay untouched.
 *
 * Implemented commands so far (see `src-tauri/src/*.rs`):
 *   api · getVersion · getConnection · readDir · readFileText · readFileDataUrl
 *   writeTextFile · openExternal · connection-config · bootstrap · terminal
 *   clipboard · dialogs · fs · logs · link-title · plugin-root · project-dir
 */

/** Subscribe to a main→renderer event; returns an unsubscribe fn (matches the
 *  Electron `on*` contract). */
function on<T>(channel: string, callback: (payload: T) => void): () => void {
  let unlisten: UnlistenFn | undefined
  void listen<T>(channel, (event) => callback(event.payload)).then((fn) => {
    unlisten = fn
  })
  return () => unlisten?.()
}

// The bridge is only meaningful inside a Tauri runtime. In a plain browser
// (vite dev without Tauri, vitest) we leave `hermesDesktop` absent so the
// existing test mocks keep working.
const isTauri =
  typeof window !== 'undefined' &&
  ('__TAURI_INTERNALS__' in window || '__TAURI__' in window)

if (isTauri) {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const desktop = {
    // ---- backend / connection ----
    getConnection: (profile?: string | null) => invoke<any>('get_connection', { profile }),
    revalidateConnection: () => invoke<any>('revalidate_connection'),
    touchBackend: (profile?: string | null) => invoke<boolean>('touch_backend', { profile }),
    getGatewayWsUrl: (profile?: null | string) => invoke<any>('get_gateway_ws_url', { profile }),
    getConnectionConfig: (profile?: string | null) => invoke<any>('get_connection_config', { profile }),
    saveConnectionConfig: (payload?: unknown) => invoke<any>('save_connection_config', { payload }),
    applyConnectionConfig: (payload?: unknown) => invoke<any>('apply_connection_config', { payload }),
    testConnectionConfig: (payload?: unknown) => invoke<any>('test_connection_config', { payload }),
    probeConnectionConfig: (rawUrl: string) => invoke<any>('probe_connection_config', { rawUrl }),
    // OAuth / cloud / ssh / profile — staged (migration ordering 9): these
    // flows need RFC 8252 browser OAuth + keychain + live cloud services and
    // are NOT ported to Rust yet. Return explicit safe shapes (never reject)
    // so the renderer's field reads degrade gracefully instead of silently
    // resolving `undefined`.
    oauthLoginConnectionConfig: () =>
      Promise.resolve({ connected: false, error: 'OAuth login is not supported in this build yet.' }),
    oauthLogoutConnectionConfig: () => Promise.resolve({ ok: true }),
    sshConfigHosts: () => Promise.resolve({ hosts: [] }),
    sshResolveHost: () => Promise.resolve({ hostname: null, identityFile: null, port: null, user: null }),
    cloud: {
      status: () =>
        Promise.resolve({ signedIn: false, error: 'Cloud sign-in is not supported in this build yet.' }),
      login: () =>
        Promise.resolve({ signedIn: false, error: 'Cloud sign-in is not supported in this build yet.' }),
      logout: () => Promise.resolve({ ok: true }),
      discover: () =>
        Promise.resolve({ agents: [], needsOrgSelection: false, error: 'Cloud discovery is not supported in this build yet.' }),
      agentSignIn: () =>
        Promise.resolve({ connected: false, error: 'Cloud agent sign-in is not supported in this build yet.' }),
    },
    profile: {
      get: () => Promise.resolve(null),
      set: () => Promise.resolve(),
    },
    api: <T>(request: unknown) => invoke<T>('api', { request }),
    notify: () => Promise.resolve({ ok: true }),
    // Mic access is granted by the OS on the WebView2 process at first use;
    // Electron's preflight gate is unnecessary on Tauri.
    requestMicrophoneAccess: () => Promise.resolve(true),

    // ---- filesystem / preview ----
    readFileDataUrl: (filePath: string) => invoke<string>('read_file_data_url', { filePath }),
    readFileDataUrlForAttach: (filePath: string) => invoke<string>('read_file_data_url', { filePath }),
    dataUrlReadMax: {
      // Ship default from store/data-url-read-max.ts (16 MB). The Tauri
      // build keeps the built-in cap; set is a no-op that reports the default.
      get: () => Promise.resolve({ defaultMaxMb: 16, maxBytes: 16 * 1024 * 1024, maxMb: 16 }),
      set: () => Promise.resolve({ defaultMaxMb: 16, maxBytes: 16 * 1024 * 1024, maxMb: 16 }),
    },
    readFileText: (filePath: string) => invoke<any>('read_file_text', { filePath }),
    readDir: (dirPath: string) => invoke<any>('read_dir', { dirPath }),
    writeTextFile: (filePath: string, content: string) =>
      invoke<any>('write_text_file', { filePath, content }),
    selectPaths: (options?: HermesSelectPathsOptions) =>
      invoke<string[]>('select_paths', { options }),
    selectSavePath: (options?: {
      defaultPath?: string
      filters?: Array<{ extensions: string[]; name: string }>
      title?: string
    }) => invoke<string | null>('select_save_path', { options }),
    openExternal: (url: string) => invoke<void>('open_external', { url }),

    // ---- version / about ----
    getVersion: () => invoke<any>('get_version'),
    getRemoteDisplayReason: () => invoke<any>('get_remote_display_reason'),

    // ---- events (main → renderer) ----
    onClosePreviewRequested: (cb: () => void) => on('hermes:close-preview-requested', cb),
    onOpenFolderRequested: (cb: () => void) => on('hermes:open-folder-requested', cb),
    onOpenUpdatesRequested: (cb: () => void) => on('hermes:open-updates', cb),
    onDeepLink: (cb: (payload: unknown) => void) => on('hermes:deep-link', cb),
    signalDeepLinkReady: () => Promise.resolve({ ok: true }),
    onWindowStateChanged: (cb: (payload: unknown) => void) => on('hermes:window-state-changed', cb),
    onFocusSession: (cb: (sessionId: string) => void) => on('hermes:focus-session', cb),
    onNotificationAction: (cb: (payload: unknown) => void) => on('hermes:notification-action', cb),
    onPreviewFileChanged: (cb: (payload: unknown) => void) => on('hermes:preview-file-changed', cb),
    onBackendExit: (cb: (payload: unknown) => void) => on('hermes:backend-exit', cb),
    onConnectionApplied: (cb: () => void) => on('hermes:connection:applied', cb),
    onPowerResume: (cb: () => void) => on('hermes:power-resume', cb),
    onBatteryChanged: (cb: (onBattery: boolean) => void) => on('hermes:power-battery', cb),
    onBootProgress: (cb: (payload: unknown) => void) => on('hermes:boot-progress', cb),
    onBootstrapEvent: (cb: (payload: unknown) => void) => on('hermes:bootstrap:event', cb),
    onFoundInPage: (cb: (result: unknown) => void) => on('hermes:found-in-page', cb),

    // ---- still-to-port groups (kept as stubs so TypeScript stays happy) ----
    getBootProgress: () => invoke<any>('get_boot_progress'),
    getBootstrapState: () => invoke<any>('get_bootstrap_state'),
    continueBootstrapLocal: () => invoke<any>('continue_bootstrap_local'),
    resetBootstrap: () => invoke<any>('reset_bootstrap'),
    repairBootstrap: () => invoke<any>('repair_bootstrap'),
    cancelBootstrap: () => invoke<any>('cancel_bootstrap'),
    claimAmbientCue: () => Promise.resolve({ ok: true }),
    // Null = "no opinion": the renderer falls back to its own localPreviewTarget
    // classification. MUST NOT echo the raw string — it's truthy, so the caller
    // would skip the fallback and treat a bare path string as a PreviewTarget
    // (tabLabelFor then crashes on .split of undefined).
    normalizePreviewTarget: () => Promise.resolve(null),
    watchPreviewFile: () => Promise.resolve({ ok: true }),
    watchDirectory: () => Promise.resolve({ ok: true }),
    stopPreviewFileWatch: () => Promise.resolve({ ok: true }),
    setActiveWork: (work?: unknown) => invoke<void>('set_active_work', { work }),
    setTitleBarTheme: () => Promise.resolve({ ok: true }),
    setNativeTheme: () => Promise.resolve({ ok: true }),
    setTranslucency: () => Promise.resolve({ ok: true }),
    setKeepAwake: () => Promise.resolve({ ok: true }),
    setPreviewShortcutActive: () => Promise.resolve({ ok: true }),
    openPreviewInBrowser: (url: string) => invoke<void>('open_preview_in_browser', { url }),
    fetchLinkTitle: (url: string) => invoke<any>('fetch_link_title', { url }),
    sanitizeWorkspaceCwd: (cwd: string) => Promise.resolve(cwd),
    settings: {
      getDefaultProjectDir: () => invoke<any>('settings_get_default_project_dir'),
      setDefaultProjectDir: (dir?: null | string) =>
        invoke<{ dir: string | null }>('settings_set_default_project_dir', { dir }),
      pickDefaultProjectDir: () => invoke<any>('settings_pick_default_project_dir'),
    },
    zoom: {
      get: () => Promise.resolve(1),
      setPercent: () => Promise.resolve({ ok: true }),
      onChanged: (cb: (payload: unknown) => void) => on('hermes:zoom:changed', cb),
    },
    revealLogs: () => invoke<void>('reveal_logs'),
    getRecentLogs: () => invoke<any>('get_recent_logs'),
    gitRoot: (startPath: string) => invoke<any>('git_root', { startPath }),
    revealPath: (path: string) => invoke<void>('reveal_path', { path }),
    openDir: (path: string) => invoke<void>('open_dir', { path }),
    // Quick screenshot: enumerate capturable windows / capture one (or
    // 'screen' for the full virtual screen) as a base64 PNG.
    listWindows: () => invoke<any[]>('list_windows'),
    captureWindow: (id: string) => invoke<string>('capture_window', { id }),
    focusMainWindow: () => invoke<void>('focus_main_window'),
    // Screenshot management: dir stats / guarded cleanup / git-exclude /
    // floating capture button.
    screenshotDirStats: (projectDir?: string | null) =>
      invoke<any[]>('screenshot_dir_stats', { projectDir: projectDir ?? null }),
    clearScreenshotDir: (path: string) => invoke<any>('clear_screenshot_dir', { path }),
    excludePathFromGit: (repoPath: string, relEntry: string) =>
      invoke<string>('exclude_path_from_git', { repoPath, relEntry }),
    setScreenshotOverlayEnabled: (enabled: boolean) =>
      invoke<void>('set_screenshot_overlay_enabled', { enabled }),
    moveScreenshotOverlayBy: (dx: number, dy: number) =>
      invoke<void>('move_screenshot_overlay_by', { dx, dy }),
    saveScreenshotOverlayPosition: () => invoke<void>('save_screenshot_overlay_position'),
    triggerQuickScreenshot: () => invoke<void>('trigger_quick_screenshot'),
    onQuickScreenshot: (cb: () => void) => on('hermes:quick-screenshot', cb),
    desktopPluginsRoot: () => invoke<string | null>('desktop_plugins_root'),
    renamePath: (path: string, name: string) => invoke<any>('rename_path', { path, name }),
    trashPath: (path: string) => invoke<void>('trash_path', { path }),
    writeClipboard: (text: string) =>
      invoke<boolean>('write_clipboard', { text }).then(() => true),
    readClipboard: () => invoke<string>('read_clipboard'),
    saveImageFromUrl: () => Promise.resolve({ ok: true }),
    // Bytes cross the IPC as base64 — a JSON number-array of a multi-MB
    // screenshot is ~7x fatter and slower to (de)serialize. `dir` (absolute)
    // overrides the default app-data composer-images target (project mode);
    // `name` preserves the caller's original file name (upstream attach flow).
    saveImageBuffer: (data: ArrayBuffer | Uint8Array, ext: string, dir?: string, name?: string) => {
      const bytes = data instanceof Uint8Array ? data : new Uint8Array(data)
      let binary = ''
      const CHUNK = 0x8000

      for (let i = 0; i < bytes.length; i += CHUNK) {
        binary += String.fromCharCode(...bytes.subarray(i, i + CHUNK))
      }

      return invoke<string>('save_image_buffer', {
        dataBase64: btoa(binary),
        ext,
        dir: dir ?? null,
        name: name ?? null
      })
    },
    saveClipboardImage: () => Promise.resolve({ ok: true }),
    getPathForFile: () => '',
    openSessionWindow: (sessionId: string, opts?: unknown) =>
      invoke<string>('open_session_window', { sessionId, opts }),
    openWindow: () => invoke<string>('open_window'),
    wakeIndicator: {
      getState: () => Promise.resolve({ active: false }),
      setState: () => Promise.resolve({ ok: true }),
      onState: (cb: (payload: unknown) => void) => on('hermes:wake-indicator:state', cb),
    },
    petOverlay: {
      open: () => Promise.resolve({ ok: true }),
      close: () => Promise.resolve({ ok: true }),
      setBounds: () => Promise.resolve({ ok: true }),
      setIgnoreMouse: () => Promise.resolve({ ok: true }),
      setFocusable: () => Promise.resolve({ ok: true }),
      pushState: () => Promise.resolve({ ok: true }),
      control: () => Promise.resolve({ ok: true }),
      onState: (cb: (payload: unknown) => void) => on('hermes:pet-overlay:state', cb),
      onControl: (cb: (payload: unknown) => void) => on('hermes:pet-overlay:control', cb),
    },
    quickEntry: {
      getSettings: () => Promise.resolve({}),
      setSettings: () => Promise.resolve({ ok: true }),
      submit: () => Promise.resolve({ ok: true }),
      dismiss: () => Promise.resolve({ ok: true }),
      pushState: () => Promise.resolve({ ok: true }),
      onState: (cb: (payload: unknown) => void) => on('hermes:quick-entry:state', cb),
      onSubmit: (cb: (payload: unknown) => void) => on('hermes:quick-entry:submit', cb),
      onShown: (cb: () => void) => on('hermes:quick-entry:shown', cb),
    },
    git: {
      worktreeList: (repoPath: string) => invoke<any>('git_worktree_list', { repoPath }),
      worktreeAdd: (repoPath: string, options?: unknown) =>
        invoke<any>('git_worktree_add', { repoPath, options }),
      worktreeRemove: (repoPath: string, worktreePath: string, options?: unknown) =>
        invoke<any>('git_worktree_remove', { repoPath, worktreePath, options }),
      branchSwitch: (repoPath: string, branch: string) =>
        invoke<any>('git_branch_switch', { repoPath, branch }),
      branchList: (repoPath: string) => invoke<any>('git_branch_list', { repoPath }),
      baseBranchList: (repoPath: string) => invoke<any>('git_base_branch_list', { repoPath }),
      repoStatus: (repoPath: string) => invoke<any>('git_repo_status', { repoPath }),
      fileDiff: (repoPath: string, filePath: string) =>
        invoke<any>('git_file_diff', { repoPath, filePath }),
      scanRepos: (roots: string[], options?: unknown) =>
        invoke<any>('git_scan_repos', { roots, options }),
      review: {
        list: (repoPath: string, scope: string, baseRef?: null | string) =>
          invoke<any>('git_review_list', { repoPath, scope, baseRef }),
        diff: (repoPath: string, filePath: string, scope: string, baseRef?: null | string, staged?: boolean) =>
          invoke<any>('git_review_diff', { repoPath, filePath, scope, baseRef, staged }),
        stage: (repoPath: string, filePath?: null | string) =>
          invoke<any>('git_review_stage', { repoPath, filePath }),
        unstage: (repoPath: string, filePath?: null | string) =>
          invoke<any>('git_review_unstage', { repoPath, filePath }),
        revert: (repoPath: string, filePath?: null | string) =>
          invoke<any>('git_review_revert', { repoPath, filePath }),
        revParse: (repoPath: string, ref?: null | string) =>
          invoke<any>('git_review_rev_parse', { repoPath, reference: ref }),
        commit: (repoPath: string, message: string, push: boolean) =>
          invoke<any>('git_review_commit', { repoPath, message, push }),
        commitContext: (repoPath: string) =>
          invoke<any>('git_review_commit_context', { repoPath }),
        push: (repoPath: string) => invoke<any>('git_review_push', { repoPath }),
        shipInfo: (repoPath: string) => invoke<any>('git_review_ship_info', { repoPath }),
        createPr: (repoPath: string) => invoke<any>('git_review_create_pr', { repoPath }),
      },
    },
    terminal: {
      start: (payload?: unknown) => invoke<any>('terminal_start', { payload }),
      write: (id: string, data: string) => invoke<any>('terminal_write', { id, data }),
      resize: (id: string, size?: { cols: number; rows: number }) =>
        invoke<any>('terminal_resize', { id, cols: size?.cols, rows: size?.rows }),
      cwd: (id: string) => invoke<any>('terminal_cwd', { id }),
      dispose: (id: string) => invoke<any>('terminal_dispose', { id }),
      onData: (id: string, cb: (payload: string) => void) => on(`hermes:terminal:${id}:data`, cb),
      onExit: (id: string, cb: (payload: unknown) => void) => on(`hermes:terminal:${id}:exit`, cb),
    },
    uninstall: {
      summary: () => Promise.resolve({ sizeBytes: 0, reason: null, targetPaths: [] }),
      run: () => Promise.resolve({ ok: true }),
    },
    updates: {
      check: () => Promise.resolve({ available: false, version: null }),
      apply: () => Promise.resolve({ ok: true }),
      getBranch: () => Promise.resolve('stable'),
      setBranch: () => Promise.resolve({ ok: true }),
      onProgress: (cb: (payload: unknown) => void) => on('hermes:updates:progress', cb),
    },
    themes: {
      fetchMarketplace: () => Promise.resolve({ themes: [] }),
      searchMarketplace: () => Promise.resolve({ themes: [] }),
    },
    findInPage: () => Promise.resolve({ ok: true }),
    stopFindInPage: () => Promise.resolve({ ok: true }),
  }

  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  ;(window as any).hermesDesktop = desktop
}

export {}