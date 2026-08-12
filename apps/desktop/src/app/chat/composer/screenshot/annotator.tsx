import { useCallback, useEffect, useRef, useState } from 'react'

import { Button } from '@/components/ui/button'
import { Codicon } from '@/components/ui/codicon'
import { Dialog, DialogContent, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { Input } from '@/components/ui/input'
import { Tip } from '@/components/ui/tooltip'
import { useI18n } from '@/i18n'
import { notifyError } from '@/store/notifications'

import {
  type Annotation,
  type AnnotationShape,
  type AnnotationTool,
  badgeAnchor,
  badgeLabel,
  nextAnnotationId,
  removeAnnotation,
  setAnnotationDescription,
  shapeIsMeaningful
} from './annotation-model'

const STROKE = '#f43f5e'

// Stroke/badge geometry scales with the image — a 3px ring reads fine on a
// laptop screenshot but vanishes on a 4K full-screen capture.
function scaleFor(width: number, height: number): number {
  return Math.max(1, Math.max(width, height) / 900)
}

function drawShape(ctx: CanvasRenderingContext2D, shape: AnnotationShape, s: number) {
  ctx.strokeStyle = STROKE
  ctx.lineWidth = 2.5 * s
  ctx.lineJoin = 'round'
  ctx.lineCap = 'round'

  if (shape.kind === 'rect') {
    ctx.strokeRect(shape.x0, shape.y0, shape.x1 - shape.x0, shape.y1 - shape.y0)

    return
  }

  if (shape.kind === 'arrow') {
    const { x0, y0, x1, y1 } = shape
    const angle = Math.atan2(y1 - y0, x1 - x0)
    const head = 12 * s

    ctx.beginPath()
    ctx.moveTo(x0, y0)
    ctx.lineTo(x1, y1)
    ctx.moveTo(x1, y1)
    ctx.lineTo(x1 - head * Math.cos(angle - Math.PI / 6), y1 - head * Math.sin(angle - Math.PI / 6))
    ctx.moveTo(x1, y1)
    ctx.lineTo(x1 - head * Math.cos(angle + Math.PI / 6), y1 - head * Math.sin(angle + Math.PI / 6))
    ctx.stroke()

    return
  }

  const [first, ...rest] = shape.points

  if (!first) {
    return
  }

  ctx.beginPath()
  ctx.moveTo(first.x, first.y)

  for (const p of rest) {
    ctx.lineTo(p.x, p.y)
  }

  ctx.stroke()
}

function drawBadge(ctx: CanvasRenderingContext2D, shape: AnnotationShape, label: string, s: number) {
  const r = 12 * s
  const anchor = badgeAnchor(shape)
  // Sit just above-left of the mark, clamped into the image.
  const cx = Math.max(r, anchor.x - r * 0.6)
  const cy = Math.max(r, anchor.y - r * 0.6)

  ctx.beginPath()
  ctx.arc(cx, cy, r, 0, Math.PI * 2)
  ctx.fillStyle = STROKE
  ctx.fill()
  ctx.lineWidth = 1.5 * s
  ctx.strokeStyle = '#ffffff'
  ctx.stroke()

  ctx.fillStyle = '#ffffff'
  ctx.font = `700 ${13 * s}px system-ui, sans-serif`
  ctx.textAlign = 'center'
  ctx.textBaseline = 'middle'
  ctx.fillText(label, cx, cy + 0.5 * s)
}

export interface ScreenshotAnnotatorProps {
  imageDataUrl: string
  onCancel: () => void
  /** The marks as drawn (with their sidebar descriptions); the caller owns
   *  image numbering and text formatting. */
  onDone: (blob: Blob, annotations: Annotation[]) => void
  open: boolean
}

// PDF-review-style annotation: draw marks (rect/arrow/pen) directly on the
// screenshot; each mark auto-numbers (①②③ painted on the image) and gets a
// note field in the sidebar. Done flattens image + marks + badges to one PNG.
export function ScreenshotAnnotator({ imageDataUrl, onCancel, onDone, open }: ScreenshotAnnotatorProps) {
  const { t } = useI18n()
  const copy = t.composer.screenshot

  const [image, setImage] = useState<HTMLImageElement | null>(null)
  const [tool, setTool] = useState<AnnotationTool>('rect')
  const [annotations, setAnnotations] = useState<Annotation[]>([])
  const [draft, setDraft] = useState<AnnotationShape | null>(null)
  const [finishing, setFinishing] = useState(false)

  const canvasRef = useRef<HTMLCanvasElement | null>(null)
  const drawing = useRef(false)

  // Reset per screenshot: a new capture starts with a clean mark list.
  useEffect(() => {
    if (!open) {
      return
    }

    setAnnotations([])
    setDraft(null)
    setTool('rect')
    setImage(null)

    const img = new Image()
    img.onload = () => setImage(img)
    img.onerror = () => notifyError(new Error('image decode failed'), copy.failedTitle)
    img.src = imageDataUrl
  }, [copy.failedTitle, imageDataUrl, open])

  const redraw = useCallback(() => {
    const canvas = canvasRef.current

    if (!canvas || !image) {
      return
    }

    const w = image.naturalWidth
    const h = image.naturalHeight

    if (canvas.width !== w || canvas.height !== h) {
      canvas.width = w
      canvas.height = h
    }

    const ctx = canvas.getContext('2d')

    if (!ctx) {
      return
    }

    const s = scaleFor(w, h)

    ctx.clearRect(0, 0, w, h)
    ctx.drawImage(image, 0, 0)
    annotations.forEach((a, i) => {
      drawShape(ctx, a.shape, s)
      drawBadge(ctx, a.shape, badgeLabel(i), s)
    })

    if (draft) {
      drawShape(ctx, draft, s)
    }
  }, [annotations, draft, image])

  useEffect(redraw, [redraw])

  const toImagePoint = (event: React.PointerEvent<HTMLCanvasElement>) => {
    const canvas = canvasRef.current!
    const rect = canvas.getBoundingClientRect()

    return {
      x: ((event.clientX - rect.left) / rect.width) * canvas.width,
      y: ((event.clientY - rect.top) / rect.height) * canvas.height
    }
  }

  const onPointerDown = (event: React.PointerEvent<HTMLCanvasElement>) => {
    if (!image) {
      return
    }

    event.currentTarget.setPointerCapture(event.pointerId)
    drawing.current = true
    const p = toImagePoint(event)

    setDraft(tool === 'pen' ? { kind: 'pen', points: [p] } : { kind: tool, x0: p.x, y0: p.y, x1: p.x, y1: p.y })
  }

  const onPointerMove = (event: React.PointerEvent<HTMLCanvasElement>) => {
    if (!drawing.current) {
      return
    }

    const p = toImagePoint(event)

    setDraft(current => {
      if (!current) {
        return current
      }

      if (current.kind === 'pen') {
        return { kind: 'pen', points: [...current.points, p] }
      }

      return { ...current, x1: p.x, y1: p.y }
    })
  }

  const onPointerUp = () => {
    drawing.current = false

    setDraft(current => {
      if (current && shapeIsMeaningful(current)) {
        setAnnotations(list => [...list, { description: '', id: nextAnnotationId(list), shape: current }])
      }

      return null
    })
  }

  const finish = () => {
    const canvas = canvasRef.current

    if (!canvas || finishing) {
      return
    }

    setFinishing(true)
    canvas.toBlob(blob => {
      setFinishing(false)

      if (!blob) {
        notifyError(new Error('canvas export failed'), copy.failedTitle)

        return
      }

      onDone(blob, annotations)
    }, 'image/png')
  }

  const tools: { icon: string; kind: AnnotationTool; label: string }[] = [
    { icon: 'square', kind: 'rect', label: copy.toolRect },
    { icon: 'arrow-right', kind: 'arrow', label: copy.toolArrow },
    { icon: 'edit', kind: 'pen', label: copy.toolPen }
  ]

  return (
    <Dialog onOpenChange={next => (next ? undefined : onCancel())} open={open}>
      <DialogContent className="flex max-h-[90vh] max-w-5xl flex-col gap-3">
        <DialogHeader>
          <DialogTitle>{copy.annotateTitle}</DialogTitle>
        </DialogHeader>
        <p className="text-[length:var(--conversation-caption-font-size)] text-(--ui-text-tertiary)">
          {copy.annotateHint}
        </p>

        <div className="flex min-h-0 flex-1 gap-3">
          <div className="flex min-w-0 flex-1 flex-col gap-2">
            <div className="flex items-center gap-1">
              {tools.map(item => (
                <Tip key={item.kind} label={item.label}>
                  <Button
                    aria-label={item.label}
                    aria-pressed={tool === item.kind}
                    className={tool === item.kind ? 'bg-(--ui-control-active-background) text-foreground' : ''}
                    onClick={() => setTool(item.kind)}
                    size="icon"
                    variant="ghost"
                  >
                    <Codicon name={item.icon} size="0.9rem" />
                  </Button>
                </Tip>
              ))}
            </div>

            <div className="min-h-0 flex-1 overflow-auto rounded-md border border-(--ui-stroke-tertiary) bg-(--ui-bg-secondary)">
              {image ? (
                <canvas
                  className="block h-auto max-h-[62vh] w-auto max-w-full cursor-crosshair touch-none"
                  onPointerDown={onPointerDown}
                  onPointerMove={onPointerMove}
                  onPointerUp={onPointerUp}
                  ref={canvasRef}
                />
              ) : (
                <div className="flex h-40 items-center justify-center text-[length:var(--conversation-caption-font-size)] text-(--ui-text-tertiary)">
                  {t.common.loading}
                </div>
              )}
            </div>
          </div>

          <aside className="flex w-60 shrink-0 flex-col gap-2 overflow-y-auto">
            <div className="flex items-center justify-between">
              <span className="text-[length:var(--conversation-caption-font-size)] font-medium text-(--ui-text-secondary)">
                {copy.notesTitle}
              </span>
              <Button
                disabled={annotations.length === 0}
                onClick={() => setAnnotations(list => list.slice(0, -1))}
                size="xs"
                variant="ghost"
              >
                {copy.undo}
              </Button>
            </div>

            {annotations.length === 0 ? (
              <p className="text-[length:var(--conversation-caption-font-size)] text-(--ui-text-quaternary)">
                {copy.notesEmpty}
              </p>
            ) : (
              annotations.map((a, i) => (
                <div className="flex items-center gap-1.5" key={a.id}>
                  <span aria-hidden className="shrink-0 text-sm">
                    {badgeLabel(i)}
                  </span>
                  <Input
                    aria-label={copy.notePlaceholder(badgeLabel(i))}
                    onChange={event => setAnnotations(list => setAnnotationDescription(list, a.id, event.target.value))}
                    placeholder={copy.notePlaceholder(badgeLabel(i))}
                    size="sm"
                    value={a.description}
                  />
                  <Tip label={t.common.delete}>
                    <Button
                      aria-label={`${t.common.delete} ${badgeLabel(i)}`}
                      className="shrink-0 text-muted-foreground hover:text-destructive"
                      onClick={() => setAnnotations(list => removeAnnotation(list, a.id))}
                      size="icon"
                      variant="ghost"
                    >
                      <Codicon name="close" size="0.8rem" />
                    </Button>
                  </Tip>
                </div>
              ))
            )}
          </aside>
        </div>

        <div className="flex items-center justify-end gap-2">
          <Button onClick={onCancel} variant="ghost">
            {t.common.cancel}
          </Button>
          <Button disabled={!image || finishing} onClick={finish}>
            {copy.insert}
          </Button>
        </div>
      </DialogContent>
    </Dialog>
  )
}
