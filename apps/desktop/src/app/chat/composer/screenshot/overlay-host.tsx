import { useStore } from '@nanostores/react'

import { useI18n } from '@/i18n'
import { $overlayScreenshotShot } from '@/store/screenshots'

import { requestComposerInsert } from '../focus'

import { ScreenshotAnnotator } from './annotator'
import { imageNoteText } from './annotation-model'
import { attachScreenshotToMain, nextMainScreenshotNumber } from './quick-capture'

/**
 * Host for the floating-button quick capture: the overlay click produces a
 * full-screen shot (parked in $overlayScreenshotShot by the sync module);
 * this dialog annotates it, then the image + its numbered note land in the
 * main composer. Mounted once in the main window (contrib/wiring).
 */
export function OverlayScreenshotHost() {
  const { t } = useI18n()
  const shot = useStore($overlayScreenshotShot)

  if (!shot) {
    return null
  }

  const close = () => $overlayScreenshotShot.set(null)

  const finish = async (blob: Blob, description: string) => {
    close()

    // Number BEFORE attaching — afterwards the new image is already counted.
    const text = imageNoteText(t.composer.screenshot.noteLabel(nextMainScreenshotNumber()), description)
    const attached = await attachScreenshotToMain(blob)

    if (attached && text) {
      requestComposerInsert(text, { mode: 'block', target: 'main' })
    }
  }

  return (
    <ScreenshotAnnotator imageDataUrl={shot} onCancel={close} onDone={(blob, desc) => void finish(blob, desc)} open />
  )
}
