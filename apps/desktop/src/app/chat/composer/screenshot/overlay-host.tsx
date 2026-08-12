import { useStore } from '@nanostores/react'

import { useI18n } from '@/i18n'
import { $overlayScreenshotShot } from '@/store/screenshots'

import { requestComposerInsert } from '../focus'

import { ScreenshotAnnotator } from './annotator'
import { type Annotation, imageMarkNoteLines } from './annotation-model'
import { attachScreenshotToMain, nextMainScreenshotNumber } from './quick-capture'

/**
 * Host for the floating-button quick capture: the overlay click produces a
 * full-screen shot (parked in $overlayScreenshotShot by the sync module);
 * this dialog annotates it, then the image + its numbered note lines
 * ("图N:M …") land in the main composer. Mounted once in the main window
 * (contrib/wiring).
 */
export function OverlayScreenshotHost() {
  const { t } = useI18n()
  const shot = useStore($overlayScreenshotShot)

  if (!shot) {
    return null
  }

  const close = () => $overlayScreenshotShot.set(null)

  const finish = async (blob: Blob, annotations: Annotation[]) => {
    close()

    // Number BEFORE attaching — afterwards the new image is already counted.
    const lines = imageMarkNoteLines(nextMainScreenshotNumber(), annotations, (n, m) =>
      t.composer.screenshot.noteLabel(n, m)
    ).join('\n')
    const attached = await attachScreenshotToMain(blob)

    if (attached && lines) {
      requestComposerInsert(lines, { mode: 'block', target: 'main' })
    }
  }

  return (
    <ScreenshotAnnotator imageDataUrl={shot} onCancel={close} onDone={(blob, marks) => void finish(blob, marks)} open />
  )
}
