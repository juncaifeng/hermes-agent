import { useRef } from 'react'

import { Codicon } from '@/components/ui/codicon'

/**
 * The floating quick-capture button: a 48px always-on-top circle. Drag to
 * move (position persists via the shell), click to open the capture picker in
 * the main window. No I18n/gateway here — the overlay is a single glyph.
 */
export function ScreenshotOverlayApp() {
  // Pointer deltas arrive in CSS px; the window moves in physical px.
  const drag = useRef<{ lastX: number; lastY: number; moved: boolean } | null>(null)

  const onPointerDown = (event: React.PointerEvent<HTMLButtonElement>) => {
    event.currentTarget.setPointerCapture(event.pointerId)
    drag.current = { lastX: event.clientX, lastY: event.clientY, moved: false }
  }

  const onPointerMove = (event: React.PointerEvent<HTMLButtonElement>) => {
    const state = drag.current
    const bridge = window.hermesDesktop

    if (!state || !bridge?.moveScreenshotOverlayBy) {
      return
    }

    const ratio = window.devicePixelRatio || 1
    const dx = (event.clientX - state.lastX) * ratio
    const dy = (event.clientY - state.lastY) * ratio

    if (!state.moved && Math.abs(dx) + Math.abs(dy) < 3 * ratio) {
      return
    }

    state.moved = true
    state.lastX = event.clientX
    state.lastY = event.clientY
    void bridge.moveScreenshotOverlayBy(dx, dy).catch(() => undefined)
  }

  const onPointerUp = () => {
    const state = drag.current
    drag.current = null

    if (!state) {
      return
    }

    const bridge = window.hermesDesktop

    if (state.moved) {
      void bridge?.saveScreenshotOverlayPosition?.().catch(() => undefined)
    } else {
      void bridge?.triggerQuickScreenshot?.().catch(() => undefined)
    }
  }

  return (
    <div className="flex h-screen w-screen items-center justify-center overflow-hidden">
      <button
        aria-label="Quick screenshot"
        className="flex size-12 cursor-pointer items-center justify-center rounded-full border border-(--ui-stroke-secondary) bg-(--ui-bg-elevated) text-(--ui-text-secondary) shadow-lg transition-colors hover:bg-(--ui-control-hover-background) hover:text-foreground"
        onPointerDown={onPointerDown}
        onPointerMove={onPointerMove}
        onPointerUp={onPointerUp}
        type="button"
      >
        <Codicon name="device-camera" size="1.1rem" />
      </button>
    </div>
  )
}
