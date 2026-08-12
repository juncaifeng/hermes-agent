/**
 * Pure model for the quick-screenshot annotation editor: shapes, automatic
 * per-mark numbering (①②③ badges painted onto the image), and the numbered
 * notes text that lands in the composer draft on insert. No DOM here — the
 * canvas renderer in annotator.tsx consumes these structures.
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
  /** Stable identity (monotonic) — NOT the display number; the badge number
   *  is the 1-based position in the list so deleting a mark renumbers the
   *  rest, keeping image and sidebar in lockstep. */
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

/**
 * The numbered notes appended to the composer draft on insert — one "① …"
 * line per described mark, in badge order. Empty descriptions are skipped
 * (a mark with no note still shows on the image; it just gets no line).
 */
export function annotationsDraftText(list: Annotation[]): string {
  return list
    .map((a, i) => ({ label: badgeLabel(i), text: a.description.trim() }))
    .filter(a => a.text.length > 0)
    .map(a => `${a.label} ${a.text}`)
    .join('\n')
}
