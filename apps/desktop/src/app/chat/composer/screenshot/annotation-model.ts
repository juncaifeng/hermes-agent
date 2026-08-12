/**
 * Pure model for the quick-screenshot annotation editor: mark shapes plus the
 * per-IMAGE note numbering. Numbering moved from per-mark badges (①②③ drawn
 * on the image) to per-image labels ("图N:") — each inserted screenshot gets
 * N = the composer's existing image-attachment count + 1, so consecutive
 * captures read 图1/图2/图3 in the draft. No DOM here; the canvas renderer in
 * annotator.tsx consumes the shapes.
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
  /** Stable identity (React key / deletion target). */
  id: number
  shape: AnnotationShape
}

export type AnnotationTool = AnnotationShape['kind']

export function nextAnnotationId(list: Annotation[]): number {
  return list.reduce((max, a) => Math.max(max, a.id), 0) + 1
}

/** A shape is worth committing only past a minimal drag — a click is a slip. */
export function shapeIsMeaningful(shape: AnnotationShape): boolean {
  if (shape.kind === 'pen') {
    return shape.points.length >= 2
  }

  return Math.abs(shape.x1 - shape.x0) >= 4 && Math.abs(shape.y1 - shape.y0) >= 4
}

/** The inserted image's number: existing image attachments + 1. */
export function imageNoteNumber(existingImageCount: number): number {
  return Math.max(0, existingImageCount) + 1
}

/**
 * Draft text for one screenshot's note: `<prefix><description>` (the caller's
 * i18n prefix already carries the separator — "图1:" / "Image 1: "). An empty
 * description yields no text at all (image still attaches).
 */
export function imageNoteText(prefix: string, description: string): string {
  const text = description.trim()

  return text ? `${prefix}${text}` : ''
}
