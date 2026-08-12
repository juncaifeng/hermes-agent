import { describe, expect, it } from 'vitest'

import {
  type Annotation,
  annotationsDraftText,
  badgeAnchor,
  badgeLabel,
  nextAnnotationId,
  removeAnnotation,
  setAnnotationDescription,
  shapeIsMeaningful
} from './annotation-model'

function ann(id: number, description = ''): Annotation {
  return { description, id, shape: { kind: 'rect', x0: 0, x1: 10, y0: 0, y1: 10 } }
}

describe('badgeLabel', () => {
  it('uses circled digits for the first twenty marks', () => {
    expect(badgeLabel(0)).toBe('①')
    expect(badgeLabel(2)).toBe('③')
    expect(badgeLabel(19)).toBe('⑳')
  })

  it('falls back to a plain number past twenty', () => {
    expect(badgeLabel(20)).toBe('21)')
  })
})

describe('list operations', () => {
  it('assigns ids one past the current max (ids only key the live list, so a freed id may be reused)', () => {
    expect(nextAnnotationId([ann(1), ann(2)])).toBe(3)
    expect(nextAnnotationId([ann(1)])).toBe(2)
    expect(nextAnnotationId([])).toBe(1)
  })

  it('removes a single mark by id', () => {
    const list = [ann(1), ann(2), ann(3)]
    expect(removeAnnotation(list, 2).map(a => a.id)).toEqual([1, 3])
  })

  it('updates only the targeted description', () => {
    const list = setAnnotationDescription([ann(1), ann(2)], 2, 'fixed')
    expect(list[0].description).toBe('')
    expect(list[1].description).toBe('fixed')
  })
})

describe('shapeIsMeaningful', () => {
  it('rejects click-sized drags and single-point pen strokes', () => {
    expect(shapeIsMeaningful({ kind: 'rect', x0: 5, y0: 5, x1: 6, y1: 6 })).toBe(false)
    expect(shapeIsMeaningful({ kind: 'arrow', x0: 0, y0: 0, x1: 2, y1: 2 })).toBe(false)
    expect(shapeIsMeaningful({ kind: 'pen', points: [{ x: 1, y: 1 }] })).toBe(false)
  })

  it('accepts real drags and multi-point strokes', () => {
    expect(shapeIsMeaningful({ kind: 'rect', x0: 0, y0: 0, x1: 30, y1: 30 })).toBe(true)
    expect(
      shapeIsMeaningful({
        kind: 'pen',
        points: [
          { x: 1, y: 1 },
          { x: 2, y: 2 }
        ]
      })
    ).toBe(true)
  })
})

describe('badgeAnchor', () => {
  it('anchors rects/arrows at their top-left corner regardless of drag direction', () => {
    expect(badgeAnchor({ kind: 'rect', x0: 50, y0: 60, x1: 10, y1: 20 })).toEqual({ x: 10, y: 20 })
  })

  it('anchors pen strokes at their first point', () => {
    expect(
      badgeAnchor({
        kind: 'pen',
        points: [
          { x: 7, y: 9 },
          { x: 1, y: 1 }
        ]
      })
    ).toEqual({ x: 7, y: 9 })
  })
})

describe('annotationsDraftText', () => {
  it('numbers notes by display position and skips undescribed marks', () => {
    const list = [ann(1, 'first'), ann(2, ''), ann(3, '  padded  ')]
    expect(annotationsDraftText(list)).toBe('① first\n③ padded')
  })

  it('renumbers after a middle deletion so image and notes stay in lockstep', () => {
    const list = removeAnnotation([ann(1, 'first'), ann(2, 'second'), ann(3, 'third')], 2)
    expect(annotationsDraftText(list)).toBe('① first\n② third')
  })

  it('is empty when nothing is described', () => {
    expect(annotationsDraftText([ann(1), ann(2)])).toBe('')
    expect(annotationsDraftText([])).toBe('')
  })
})
