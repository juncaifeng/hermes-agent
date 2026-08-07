/**
 * Browser dev-mode boot test (no Tauri / no Electron).
 *
 * Playwright cannot attach to Tauri's WebView2, so we test the renderer in a
 * plain Chromium against the Vite dev server, injecting a mock
 * `window.hermesDesktop` bridge + mock WebSocket. This validates the frontend
 * boot chain (use-gateway-boot) reaches the chat composer without the
 * "connecting to gateway" / "Desktop IPC bridge is unavailable" failure path.
 *
 * Prerequisite: `npm run dev:renderer` (Vite on http://127.0.0.1:5174).
 * Run:
 *   npx playwright test e2e/browser-boot-mock.spec.ts --reporter=list
 */
import { expect, test } from '@playwright/test'

const DEV_URL = process.env.DEV_URL || 'http://127.0.0.1:5174/'

// ─── Mock hermesDesktop bridge ──────────────────────────────────────────
// Injected before any app code runs. Covers the methods the boot path calls.
const MOCK_BRIDGE = `
(() => {
  const conn = {
    baseUrl: 'http://127.0.0.1:9999',
    isFullscreen: false,
    mode: 'local',
    authMode: 'token',
    nativeOverlayWidth: 0,
    source: 'local',
    token: 'mock-token',
    wsUrl: 'ws://127.0.0.1:9999/ws',
    logs: [],
    profile: 'default',
    windowButtonPosition: null,
  }

  // Monkey-patch WebSocket so gateway.connect() "opens" immediately and
  // replies to any JSON-RPC request with a null result.
  window.WebSocket = class MockWebSocket {
    static OPEN = 1
    static CONNECTING = 0
    static CLOSED = 3
    static CLOSING = 2
    readyState = 0
    url = ''
    onopen = null
    onmessage = null
    onclose = null
    onerror = null
    constructor(url) {
      this.url = url
      setTimeout(() => {
        this.readyState = 1
        this.onopen?.(new Event('open'))
      }, 0)
    }
    send(data) {
      try {
        const frame = JSON.parse(data)
        if (frame && frame.id !== undefined && frame.id !== null) {
          setTimeout(() => {
            this.onmessage?.({
              data: JSON.stringify({ id: frame.id, result: null }),
            })
          }, 0)
        }
      } catch {}
    }
    close() {
      this.readyState = 3
      this.onclose?.(new Event('close'))
    }
    addEventListener(type, cb) {
      if (type === 'open') this.onopen = cb
      if (type === 'message') this.onmessage = cb
      if (type === 'close') this.onclose = cb
    }
    removeEventListener() {}
  }

  // API router: serve minimal config / sessions so boot's refresh calls land.
  const apiHandler = async (request) => {
    const p = request?.path || ''
    if (p.includes('/api/config/defaults')) {
      return { display: {}, agent: {}, voice: {}, stt: {}, terminal: {} }
    }
    if (p === '/api/config' || p.startsWith('/api/config?')) {
      return { display: { personality: '' }, agent: {}, voice: {}, stt: {}, terminal: {} }
    }
    if (p.includes('/api/profiles/sessions/sidebar')) {
      return { recents: { profiles_truncated: false, sessions: [] }, cron: { sessions: [] }, messaging: { sessions: [] } }
    }
    if (p.includes('/api/cron/jobs')) {
      return { jobs: [] }
    }
    if (p.includes('/api/messaging/sessions')) {
      return { sessions: [] }
    }
    if (p === '/api/logs' || p.startsWith('/api/logs?')) {
      return { logs: [] }
    }
    if (p.includes('/api/models')) {
      return { models: [{ id: 'mock-model', name: 'Mock Model', provider: 'mock' }] }
    }
    if (p === '/api/health' || p === '/api/version') {
      return { ok: true, version: '0.17.0' }
    }
    return {}
  }

  window.hermesDesktop = {
    getConnection: async () => ({ ...conn }),
    revalidateConnection: async () => ({ ok: true, rebuilt: false }),
    touchBackend: async () => ({ ok: true }),
    getGatewayWsUrl: async () => ({ ok: true, wsUrl: conn.wsUrl }),
    getConnectionConfig: async () => ({
      envOverride: false, mode: 'local', profile: null, remoteAuthMode: 'token',
      remoteOauthConnected: false, remoteTokenPreview: null, remoteTokenSet: false,
      remoteUrl: '', cloudOrg: '', sshHost: '', sshUser: '', sshPort: null,
      sshKeyPath: '', sshRemoteHermesPath: '', sshRemoteProfile: '',
    }),
    saveConnectionConfig: async (x) => x,
    applyConnectionConfig: async (x) => x,
    testConnectionConfig: async () => ({ ok: true, baseUrl: conn.baseUrl }),
    probeConnectionConfig: async () => ({ baseUrl: conn.baseUrl, reachable: true, authMode: 'token', providers: [], version: '0.17.0', error: null }),
    oauthLoginConnectionConfig: async () => ({ ok: true, baseUrl: conn.baseUrl, connected: true }),
    oauthLogoutConnectionConfig: async () => ({ ok: true, connected: false }),
    sshConfigHosts: async () => ({ hosts: [] }),
    sshResolveHost: async () => ({ hostname: null, identityFile: null, port: null, user: null }),
    cloud: {
      status: async () => ({ portalBaseUrl: '', signedIn: false }),
      login: async () => ({ portalBaseUrl: '', signedIn: true, ok: true }),
      logout: async () => ({ portalBaseUrl: '', signedIn: false, ok: true }),
      discover: async () => ({ needsOrgSelection: false, agents: [], org: null }),
      agentSignIn: async () => ({ baseUrl: conn.baseUrl, connected: true }),
    },
    profile: {
      get: async () => ({ profile: 'default' }),
      set: async (name) => ({ profile: name }),
    },
    api: apiHandler,
    notify: async () => true,
    requestMicrophoneAccess: async () => true,
    readFileDataUrl: async () => '',
    readFileText: async (p) => ({ path: p, text: '' }),
    selectPaths: async () => [],
    writeClipboard: async () => true,
    readClipboard: async () => '',
    saveImageFromUrl: async () => true,
    saveImageBuffer: async () => '',
    saveClipboardImage: async () => '',
    getPathForFile: () => '/mock/file',
    normalizePreviewTarget: async (t) => ({ kind: 'file', label: t, source: t, url: t }),
    watchPreviewFile: async () => '<mock-watch>',
    stopPreviewFileWatch: async () => true,
    openExternal: async () => undefined,
    fetchLinkTitle: async () => 'Mock',
    sanitizeWorkspaceCwd: async (cwd) => ({ cwd: cwd || 'C:\\\\mock', sanitized: false }),
    settings: {
      getDefaultProjectDir: async () => ({ defaultLabel: 'Mock', dir: null, resolvedCwd: 'C:\\\\mock' }),
      pickDefaultProjectDir: async () => ({ canceled: true, dir: null }),
      setDefaultProjectDir: async (dir) => ({ dir }),
    },
    revealLogs: async () => ({ ok: true, path: 'C:\\\\mock\\\\log.txt' }),
    getRecentLogs: async () => ({ path: 'C:\\\\mock\\\\log.txt', lines: [] }),
    readDir: async () => ({ entries: [] }),
    gitRoot: async () => null,
    git: {
      worktreeList: async () => [],
      worktreeAdd: async () => { throw new Error('not supported') },
      worktreeRemove: async () => { throw new Error('not supported') },
      branchSwitch: async () => { throw new Error('not supported') },
      branchList: async () => [],
      baseBranchList: async () => [],
      repoStatus: async () => null,
      fileDiff: async () => '',
      review: {
        list: async () => ({ files: [], base: null }),
        diff: async () => '',
        stage: async () => ({ ok: true }),
        unstage: async () => ({ ok: true }),
        revert: async () => ({ ok: true }),
        revParse: async () => null,
        commit: async () => ({ ok: true }),
        commitContext: async () => ({ diff: '', recent: '' }),
        push: async () => ({ ok: true }),
        shipInfo: async () => ({ ghReady: false, pr: null }),
        createPr: async () => ({ url: '' }),
      },
      scanRepos: async () => [],
    },
    terminal: {
      cwd: async () => null,
      dispose: async () => true,
      onData: () => () => undefined,
      onExit: () => () => undefined,
      resize: async () => true,
      start: async () => ({ cwd: 'C:\\\\mock', id: 'mock-term', shell: 'cmd' }),
      write: async () => true,
    },
    onClosePreviewRequested: () => () => undefined,
    onOpenFolderRequested: () => () => undefined,
    onDeepLink: () => () => undefined,
    signalDeepLinkReady: async () => ({ ok: true }),
    onWindowStateChanged: () => () => undefined,
    onFocusSession: () => () => undefined,
    onNotificationAction: () => () => undefined,
    onPreviewFileChanged: () => () => undefined,
    onBackendExit: () => () => undefined,
    onConnectionApplied: () => () => undefined,
    onPowerResume: () => () => undefined,
    getOnBattery: async () => false,
    onBatteryChanged: () => () => undefined,
    onBootProgress: () => () => undefined,
    getBootProgress: async () => ({ error: null, fakeMode: false, message: '', phase: 'renderer.boot', progress: 0, running: false, timestamp: 0 }),
    getBootstrapState: async () => ({ active: false, manifest: null, stages: {}, error: null, log: [], startedAt: null, completedAt: null, setupChoice: null, unsupportedPlatform: null }),
    continueBootstrapLocal: async () => ({ ok: true }),
    resetBootstrap: async () => ({ ok: true }),
    repairBootstrap: async () => ({ ok: true }),
    cancelBootstrap: async () => ({ ok: true, cancelled: true }),
    onBootstrapEvent: () => () => undefined,
    getVersion: async () => ({ appVersion: '0.17.0', electronVersion: 'mock', nodeVersion: '22', platform: 'win32', hermesRoot: 'C:\\\\mock' }),
    updates: {
      check: async () => ({ supported: true }),
      apply: async () => ({ ok: true }),
      getBranch: async () => ({ branch: 'main' }),
      setBranch: async () => ({ branch: 'main' }),
      onProgress: () => () => undefined,
    },
    uninstall: {
      summary: async () => ({ hermes_home: '', agent_installed: false, gui_installed: false, source_built_artifacts: [], packaged_app_paths: [], userdata_dir: '', userdata_exists: false, platform: 'win32' }),
      run: async () => ({ ok: true }),
    },
    themes: {
      fetchMarketplace: async () => { throw new Error('not supported') },
      searchMarketplace: async () => [],
    },
    findInPage: async () => ({ count: 0 }),
    stopFindInPage: async () => undefined,
    onFoundInPage: () => () => undefined,
  }
})()
`

// ─── Test ───────────────────────────────────────────────────────────────
test('dev renderer boots to chat composer with mock bridge', async ({ page }) => {
  await page.addInitScript(MOCK_BRIDGE)

  const consoleErrors: string[] = []
  page.on('console', (msg) => {
    if (msg.type() === 'error') {
      consoleErrors.push(msg.text())
    }
  })

  await page.goto(DEV_URL, { waitUntil: 'domcontentloaded' })

  // Wait for the composer to appear (boot progressed past the connecting overlay).
  await page.waitForSelector('textarea, [contenteditable="true"]', {
    state: 'attached',
    timeout: 60_000,
  })

  // Give boot a moment to complete (config + sessions refresh).
  await page.waitForTimeout(3000)

  const visible = ((await page.evaluate(() => document.body.innerText)) ?? '').toLowerCase()

  const failureIndicators = [
    'connecting to gateway',
    'could not connect to hermes gateway',
    'desktop boot failed',
    'lost connection to the hermes gateway',
    'boot failed',
    'connection settings',
    'desktop ipc bridge is unavailable',
  ]
  const failures = failureIndicators.filter((s) => visible.includes(s))

  // The composer should be interactive (enabled) once the gateway is open.
  const composerEnabled = await page
    .locator('textarea, [contenteditable="true"]')
    .first()
    .isEnabled()
    .catch(() => false)

  console.log(`\n[RESULT] composer enabled: ${composerEnabled ? 'YES' : 'NO'}`)
  console.log(`[RESULT] failure indicators present: ${failures.length > 0 ? failures.join(', ') : 'none'}`)
  console.log(`[RESULT] console errors(${consoleErrors.length}): ${consoleErrors.slice(0, 5).join(' | ') || 'none'}`)

  await page.screenshot({ path: 'playwright-dev-boot.png', fullPage: false })

  expect(composerEnabled).toBe(true)
  expect(failures).toEqual([])
})