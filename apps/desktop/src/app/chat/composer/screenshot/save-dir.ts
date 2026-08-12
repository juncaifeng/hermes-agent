/**
 * Where a composer image (screenshot / paste / drop) is written. Pure —
 * attachImageBlob and the settings page both resolve through this.
 *
 * Project mode with no workspace (detached session, no cwd) falls back to
 * app-data — writing nowhere is never an option.
 */

import type { ScreenshotDirMode } from '@/store/screenshots'

export const PROJECT_SCREENSHOTS_REL = '.hermes/screenshots'

export function resolveScreenshotDir(mode: ScreenshotDirMode, cwd: null | string | undefined): string | undefined {
  if (mode !== 'project') {
    return undefined
  }

  // Normalize to forward slashes: the string doubles as UI display text, and
  // the backend (PathBuf) accepts either separator.
  const base = (cwd ?? '').trim().replace(/\\/g, '/').replace(/\/+$/, '')

  return base ? `${base}/${PROJECT_SCREENSHOTS_REL}` : undefined
}
