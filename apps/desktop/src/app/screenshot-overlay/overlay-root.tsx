import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'

import { ErrorBoundary } from '@/components/error-boundary'
import { ThemeProvider } from '@/themes/context'

import { ScreenshotOverlayApp } from './screenshot-overlay-app'

/**
 * Boot the quick-screenshot overlay window (`?win=screenshot-overlay`). Same
 * bundle/atom graph as the main app, but a minimal transparent surface — the
 * pattern the pet overlay established (see pet-overlay/overlay-root.tsx).
 */
export function mountScreenshotOverlay(): void {
  const style = document.createElement('style')
  style.textContent = 'html,body,#root{background:transparent !important;}'
  document.head.appendChild(style)

  const root = document.getElementById('root')

  if (!root) {
    return
  }

  createRoot(root).render(
    <StrictMode>
      <ErrorBoundary label="screenshot-overlay">
        <ThemeProvider>
          <ScreenshotOverlayApp />
        </ThemeProvider>
      </ErrorBoundary>
    </StrictMode>
  )
}
