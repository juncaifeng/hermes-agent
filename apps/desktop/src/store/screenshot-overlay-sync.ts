// Screenshot-overlay sync: keep the floating quick-capture button window in
// step with the persisted preference, and route its one-click capture into a
// full-screen shot parked for annotation in the main window. Main-window
// only — auxiliary roots (the overlay itself, quick entry, …) also evaluate
// the store graph, so gate on the `win` param.
//
// Imported for its side effect from main.tsx (same pattern as store/power).
// Kept deliberately light: the annotate/attach half lives in
// chat/composer/screenshot/quick-capture.ts (loaded by the host, not here).

import { translateNow } from '@/i18n'
import { notifyError } from '@/store/notifications'

import { $overlayScreenshotShot, $screenshotOverlayEnabled } from './screenshots'

const isMainWindow = typeof window !== 'undefined' && !new URLSearchParams(window.location.search).get('win')

// Overlay click = capture the whole screen immediately (the composer button
// keeps the window picker). The annotator host opens on the parked shot.
async function captureForAnnotation(): Promise<void> {
  const bridge = window.hermesDesktop

  if (!bridge?.captureWindow) {
    return
  }

  try {
    // 'screen' self-hides the overlay for the exposure (Rust side). The main
    // window is raised only AFTER the capture — surfacing it earlier would
    // put Hermes itself in front of whatever the user wanted to shoot.
    const base64 = await bridge.captureWindow('screen')

    await bridge.focusMainWindow?.().catch(() => undefined)

    $overlayScreenshotShot.set(`data:image/png;base64,${base64}`)
  } catch (err) {
    notifyError(err, translateNow('composer.screenshot.failedTitle'))
  }
}

if (isMainWindow) {
  const bridge = () => window.hermesDesktop

  // Apply on load (restore an enabled overlay across restarts) and on every
  // toggle. The Rust side is idempotent (focus-or-create / close-if-present).
  const apply = (enabled: boolean) => {
    void bridge()
      ?.setScreenshotOverlayEnabled?.(enabled)
      ?.catch(() => undefined)
  }

  apply($screenshotOverlayEnabled.get())
  $screenshotOverlayEnabled.listen(apply)

  bridge()?.onQuickScreenshot?.(() => void captureForAnnotation())
}
