// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

const setScreenshotOverlayEnabled = vi.fn()
const captureWindow = vi.fn()
const notifyError = vi.fn()
let quickScreenshotHandler: (() => void) | null = null

vi.mock('@/store/notifications', () => ({
  notify: vi.fn(),
  notifyError: (err: unknown, title: unknown) => notifyError(err, title)
}))

type ScreenshotsStore = typeof import('./screenshots')

let store: ScreenshotsStore

function installBridge() {
  ;(window as unknown as { hermesDesktop: unknown }).hermesDesktop = {
    captureWindow,
    onQuickScreenshot: (cb: () => void) => {
      quickScreenshotHandler = cb

      return () => {}
    },
    setScreenshotOverlayEnabled
  }
}

beforeEach(async () => {
  // The sync module applies once at import; reset the registry so each test's
  // import re-runs the side effect against the fresh bridge mock. The store
  // module must come through the SAME reset registry, or the test reads
  // different atom instances than the module mutates.
  vi.resetModules()
  store = await import('./screenshots')
  quickScreenshotHandler = null
  store.$screenshotOverlayEnabled.set(false)
  store.$overlayScreenshotShot.set(null)
  captureWindow.mockResolvedValue('QUJD')
  installBridge()
})

afterEach(() => {
  vi.clearAllMocks()
  delete (window as unknown as { hermesDesktop?: unknown }).hermesDesktop
  localStorage.clear()
})

describe('screenshot-overlay sync', () => {
  it('applies the persisted toggle to the shell and follows later flips', async () => {
    store.$screenshotOverlayEnabled.set(true)
    await import('./screenshot-overlay-sync')

    // Applied on import (restore after restart)…
    expect(setScreenshotOverlayEnabled).toHaveBeenCalledWith(true)

    store.$screenshotOverlayEnabled.set(false)
    expect(setScreenshotOverlayEnabled).toHaveBeenCalledWith(false)
  })

  it('one overlay click captures the full screen and parks it for annotation', async () => {
    await import('./screenshot-overlay-sync')

    expect(quickScreenshotHandler).not.toBeNull()

    quickScreenshotHandler!()

    // The picker is skipped entirely: straight to a full-screen capture…
    await vi.waitFor(() => expect(captureWindow).toHaveBeenCalledWith('screen'))
    // …and the shot is parked for the annotator host.
    await vi.waitFor(() => expect(store.$overlayScreenshotShot.get()).toBe('data:image/png;base64,QUJD'))
  })

  it('a failed capture toasts and opens nothing', async () => {
    captureWindow.mockRejectedValue(new Error('the window may be minimized'))
    await import('./screenshot-overlay-sync')

    quickScreenshotHandler!()

    await vi.waitFor(() => expect(notifyError).toHaveBeenCalled())
    expect(store.$overlayScreenshotShot.get()).toBeNull()
  })
})
