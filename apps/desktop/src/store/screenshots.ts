import { atom } from 'nanostores'

import { Codecs, persistentAtom } from '@/lib/persisted'

// Quick-screenshot preferences — desktop-local (NOT config.yaml): where
// composer images land, and whether the floating capture button shows.
// See docs/screenshot-mgmt-proposal.md.

export type ScreenshotDirMode = 'app_data' | 'project'

const dirModeCodec = Codecs.json<ScreenshotDirMode>(value => (value === 'project' ? 'project' : 'app_data'))

/** Where quick-screenshot/pasted images are written. Default app-data. */
export const $screenshotDirMode = persistentAtom('hermes.desktop.screenshots.dirMode', 'app_data', dirModeCodec)

/** Project mode only: also add `.hermes/screenshots/` to the workspace's
 *  `.git/info/exclude`. Applied on toggle to the CURRENT workspace. */
export const $screenshotGitExclude = persistentAtom('hermes.desktop.screenshots.gitExclude', false, Codecs.bool)

/** Floating quick-screenshot button (secondary always-on-top window). Off by
 *  default; synced to the shell by store/screenshot-overlay-sync. */
export const $screenshotOverlayEnabled = persistentAtom('hermes.desktop.screenshots.overlay', false, Codecs.bool)

/** A full-screen capture from the overlay button, waiting for annotation in
 *  the main window (data URL). Null = no pending shot. */
export const $overlayScreenshotShot = atom<null | string>(null)
