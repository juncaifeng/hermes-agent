import { attachmentId, pathLabel } from '@/lib/chat-runtime'
import { addComposerAttachment, type ComposerAttachment, mainComposerScope } from '@/store/composer'
import { $currentCwd } from '@/store/session'
import { $screenshotDirMode } from '@/store/screenshots'

import { attachmentPreviewDataUrl } from '../../hooks/use-composer-actions'
import { requestComposerFocus } from '../focus'

import { imageNoteNumber } from './annotation-model'
import { resolveScreenshotDir } from './save-dir'

// Shared attach/numbering half of the quick-screenshot flows (overlay host +
// any future global capture surface). The capture trigger itself lives in
// store/screenshot-overlay-sync.ts (kept import-light for main.tsx).

/** Persist a finished screenshot and attach it to the main composer. Mirrors
 *  use-composer-actions.attachImageBlob (dir routing + preview thumbnail). */
export async function attachScreenshotToMain(blob: Blob): Promise<boolean> {
  const bridge = window.hermesDesktop

  if (!bridge?.saveImageBuffer) {
    return false
  }

  const dir = resolveScreenshotDir($screenshotDirMode.get(), $currentCwd.get())
  const data = new Uint8Array(await blob.arrayBuffer())
  const savedPath = await bridge.saveImageBuffer(data, '.png', dir)

  if (!savedPath) {
    return false
  }

  const base: ComposerAttachment = {
    id: attachmentId('image', savedPath),
    kind: 'image',
    label: pathLabel(savedPath),
    detail: savedPath,
    path: savedPath
  }

  addComposerAttachment(base)
  requestComposerFocus('main')

  try {
    const previewUrl = await attachmentPreviewDataUrl(savedPath)

    if (previewUrl) {
      mainComposerScope.add({ ...base, previewUrl })
    }
  } catch {
    // The thumbnail is best-effort; the attachment itself is already usable.
  }

  return true
}

/** Image attachments already in the main composer draft — the numbering base
 *  for the next screenshot's 图N note. */
export function mainComposerImageCount(): number {
  return mainComposerScope.$attachments.get().filter(a => a.kind === 'image').length
}

/** The number the NEXT inserted screenshot gets: existing images + 1. Read it
 *  BEFORE attaching — afterwards the new image is already counted. */
export function nextMainScreenshotNumber(): number {
  return imageNoteNumber(mainComposerImageCount())
}
