/**
 * Pure model for the quick-screenshot annotation editor: mark shapes with
 * per-mark numbering (①②③ badges painted onto the image) and one description
 * per mark, plus the per-IMAGE note numbering. The draft text combines both:
 * the inserted image gets N = the composer's existing image-attachment count
 * + 1, and each described mark gets one line "图N:M …" (M = the mark's
 * position in the image). No DOM here; the canvas renderer in annotator.tsx
 * consumes these structures.
 */

export interface AnnotationPoint {
  x: number
  y: number
}

export type AnnotationShape =
  | { kind: 'rect'; x0: number; x1: number; y0: number; y1: number }
  | { kind: 'arrow'; x0: number; x1: number; y0: number; y1: number }
  | { kind: 'pen'; points: AnnotationPoint[] }

export interface Annotation {
  /** Stable identity (React key / deletion target) — NOT the display number;
   *  the badge number is the 1-based position in the list so deleting a mark
   *  renumbers the rest, keeping image and sidebar in lockstep. */
  id: number
  description: string
  shape: AnnotationShape
}

export type AnnotationTool = AnnotationShape['kind']

// ①..⑳ cover any realistic annotation count; beyond that fall back to "n)".
const CIRCLED = '①②③④⑤⑥⑦⑧⑨⑩⑪⑫⑬⑭⑮⑯⑰⑱⑲⑳'

export function badgeLabel(index: number): string {
  return index >= 0 && index < CIRCLED.length ? CIRCLED[index] : `${index + 1})`
}

export function nextAnnotationId(list: Annotation[]): number {
  return list.reduce((max, a) => Math.max(max, a.id), 0) + 1
}

export function removeAnnotation(list: Annotation[], id: number): Annotation[] {
  return list.filter(a => a.id !== id)
}

export function setAnnotationDescription(list: Annotation[], id: number, description: string): Annotation[] {
  return list.map(a => (a.id === id ? { ...a, description } : a))
}

/** A shape is worth committing only past a minimal drag — a click is a slip. */
export function shapeIsMeaningful(shape: AnnotationShape): boolean {
  if (shape.kind === 'pen') {
    return shape.points.length >= 2
  }

  return Math.abs(shape.x1 - shape.x0) >= 4 && Math.abs(shape.y1 - shape.y0) >= 4
}

/** Badge anchor on the image: top-left of the mark, nudged up-left. */
export function badgeAnchor(shape: AnnotationShape): AnnotationPoint {
  if (shape.kind === 'pen') {
    return shape.points[0] ?? { x: 0, y: 0 }
  }

  return { x: Math.min(shape.x0, shape.x1), y: Math.min(shape.y0, shape.y1) }
}

/** The inserted image's number: existing image attachments + 1. */
export function imageNoteNumber(existingImageCount: number): number {
  return Math.max(0, existingImageCount) + 1
}

/**
 * Draft lines for one screenshot: one "图N:M …" line per mark with a
 * non-empty description, in mark order (M = 1-based position in the image,
 * matching the badge drawn on it). `label` is the caller's i18n prefix
 * (separator included). All-empty descriptions yield no lines — the image
 * still attaches on its own.
 */
export function imageMarkNoteLines(
  imageNumber: number,
  annotations: Annotation[],
  label: (imageNumber: number, mark: number) => string
): string[] {
  return annotations
    .map((a, i) => ({ prefix: label(imageNumber, i + 1), text: a.description.trim() }))
    .filter(a => a.text.length > 0)
    .map(a => `${a.prefix}${a.text}`)
}
