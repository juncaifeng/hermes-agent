import { useCallback, useState } from 'react'

import { Button } from '@/components/ui/button'
import { Codicon } from '@/components/ui/codicon'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { Tip } from '@/components/ui/tooltip'
import type { HermesWindowInfo } from '@/global'
import { useI18n } from '@/i18n'
import { triggerHaptic } from '@/lib/haptics'
import { notifyError } from '@/store/notifications'

import { GHOST_ICON_BTN } from '../controls'
import { useComposerScope } from '../scope'

import { ScreenshotAnnotator } from './annotator'
import { type Annotation, imageMarkNoteLines, imageNoteNumber } from './annotation-model'
import { groupWindowsByProcess, UNKNOWN_PROCESS } from './window-list'

interface ScreenshotButtonProps {
  disabled?: boolean
  onAttachImageBlob?: (blob: Blob) => Promise<boolean | void> | boolean | void
  onInsertText: (text: string) => void
}

type Stage = 'closed' | 'picking' | 'capturing' | 'annotating'

// Composer quick screenshot: pick a running window (or the whole screen) →
// capture via the Rust bridge → annotate → insert as a normal image
// attachment, with the image's numbered note (图N) appended to the draft.
export function ScreenshotButton({ disabled, onAttachImageBlob, onInsertText }: ScreenshotButtonProps) {
  const { t } = useI18n()
  const copy = t.composer.screenshot
  const scope = useComposerScope()

  const [stage, setStage] = useState<Stage>('closed')
  const [windows, setWindows] = useState<HermesWindowInfo[] | null>(null)
  const [shot, setShot] = useState<string | null>(null)

  const bridge = typeof window === 'undefined' ? undefined : window.hermesDesktop
  // Older shells / non-Windows builds lack the capture commands — hide the
  // entry point entirely rather than offering a button that always errors.
  const available = Boolean(bridge?.listWindows && bridge?.captureWindow && onAttachImageBlob)

  const openPicker = useCallback(async () => {
    triggerHaptic('open')
    setStage('picking')
    setWindows(null)

    try {
      setWindows(await window.hermesDesktop!.listWindows!())
    } catch (err) {
      notifyError(err, copy.failedTitle)
      setStage('closed')
    }
  }, [copy.failedTitle])

  if (!available) {
    return null
  }

  const close = () => {
    setStage('closed')
    setShot(null)
  }

  const pick = async (id: string) => {
    setStage('capturing')

    try {
      const base64 = await bridge!.captureWindow!(id)
      setShot(`data:image/png;base64,${base64}`)
      setStage('annotating')
    } catch (err) {
      // Minimized-window / per-app capture failures land here with the Rust
      // error text; reopen the picker so another target is one click away.
      notifyError(err, copy.failedTitle)
      setStage('picking')
    }
  }

  const finish = async (blob: Blob, annotations: Annotation[]) => {
    close()

    // Number BEFORE attaching — afterwards the new image is already counted.
    // One "图N:M …" line per described mark; nothing when all are blank.
    const imageCount = scope.attachments.$attachments.get().filter(a => a.kind === 'image').length
    const lines = imageMarkNoteLines(imageNoteNumber(imageCount), annotations, (n, m) => copy.noteLabel(n, m)).join(
      '\n'
    )
    const attached = await onAttachImageBlob!(blob)

    // Only annotate the draft when the image actually landed — a numbered
    // note with no screenshot would read as a dangling reference.
    if (attached !== false && lines) {
      onInsertText(lines)
    }
  }

  const groups = groupWindowsByProcess(windows ?? [])

  return (
    <>
      <Popover
        onOpenChange={next => {
          if (next) {
            void openPicker()
          } else if (stage === 'picking' || stage === 'capturing') {
            setStage('closed')
          }
        }}
        open={stage === 'picking' || stage === 'capturing'}
      >
        <Tip label={copy.button}>
          <PopoverTrigger asChild>
            <Button
              aria-label={copy.button}
              className={GHOST_ICON_BTN}
              disabled={disabled}
              size="icon"
              type="button"
              variant="ghost"
            >
              <Codicon name="device-camera" size="0.875rem" />
            </Button>
          </PopoverTrigger>
        </Tip>
        <PopoverContent align="end" className="max-h-80 w-80 overflow-y-auto p-1.5" side="top">
          <div className="px-2 pb-1 pt-1 text-[0.625rem] font-semibold uppercase tracking-wider text-(--ui-text-tertiary)">
            {copy.pickWindow}
          </div>

          <PickerRow disabled={stage === 'capturing'} label={copy.entireScreen} onSelect={() => void pick('screen')} />

          {windows === null ? (
            <div className="px-2 py-2 text-[length:var(--conversation-caption-font-size)] text-(--ui-text-tertiary)">
              {copy.loadingWindows}
            </div>
          ) : groups.length === 0 ? (
            <div className="px-2 py-2 text-[length:var(--conversation-caption-font-size)] text-(--ui-text-tertiary)">
              {copy.noWindows}
            </div>
          ) : (
            groups.map(group => (
              <div key={group.process || UNKNOWN_PROCESS}>
                <div className="mt-1 border-t border-(--ui-stroke-tertiary) px-2 pb-0.5 pt-1.5 text-[0.625rem] font-medium text-(--ui-text-tertiary)">
                  {group.process || '—'}
                </div>
                {group.windows.map(win => (
                  <PickerRow
                    disabled={stage === 'capturing'}
                    key={win.id}
                    label={win.title}
                    onSelect={() => void pick(win.id)}
                  />
                ))}
              </div>
            ))
          )}
        </PopoverContent>
      </Popover>

      {shot && (
        <ScreenshotAnnotator
          imageDataUrl={shot}
          onCancel={close}
          onDone={(blob, notesText) => void finish(blob, notesText)}
          open={stage === 'annotating'}
        />
      )}
    </>
  )
}

function PickerRow({ disabled, label, onSelect }: { disabled?: boolean; label: string; onSelect: () => void }) {
  return (
    <button
      className="flex w-full cursor-pointer items-center rounded-md px-2 py-1.5 text-left text-[length:var(--conversation-tool-font-size)] transition-colors hover:bg-(--ui-control-hover-background) disabled:opacity-50"
      disabled={disabled}
      onClick={onSelect}
      type="button"
    >
      <span className="min-w-0 flex-1 truncate">{label}</span>
    </button>
  )
}
