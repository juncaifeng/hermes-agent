import { describe, expect, it } from 'vitest'

import {
  type Annotation,
  badgeAnchor,
  badgeLabel,
  imageMarkNoteLines,
  imageNoteNumber,
  nextAnnotationId,
  removeAnnotation,
  setAnnotationDescription,
  shapeIsMeaningful
} from './annotation-model'

function ann(id: number, description = ''): Annotation {
  return { description, id, shape: { kind: 'rect', x0: 0, x1: 10, y0: 0, y1: 10 } }
}

// Stand-in for the i18n noteLabel — mirrors the zh format.
const zhLabel = (n: number, m: number) => `图${n}:${m} `

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
  it('assigns ids one past the current max', () => {
    expect(nextAnnotationId([ann(1), ann(2)])).toBe(3)
    expect(nextAnnotationId([])).toBe(1)
  })

  it('removes a single mark by id', () => {
    expect(removeAnnotation([ann(1), ann(2), ann(3)], 2).map(a => a.id)).toEqual([1, 3])
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

describe('imageNoteNumber', () => {
  it('numbers by existing image count: empty draft → 1, one image → 2', () => {
    expect(imageNoteNumber(0)).toBe(1)
    expect(imageNoteNumber(1)).toBe(2)
  })

  it('never goes below 1 on garbage input', () => {
    expect(imageNoteNumber(-5)).toBe(1)
  })
})

describe('imageMarkNoteLines', () => {
  it('first image with two described marks → 图1:1 / 图1:2', () => {
    const lines = imageMarkNoteLines(imageNoteNumber(0), [ann(1, 'first'), ann(2, 'second')], zhLabel)
    expect(lines).toEqual(['图1:1 first', '图1:2 second'])
  })

  it('with one image already in the draft → 图2:1', () => {
    const lines = imageMarkNoteLines(imageNoteNumber(1), [ann(1, 'again')], zhLabel)
    expect(lines).toEqual(['图2:1 again'])
  })

  it('skips marks with empty descriptions', () => {
    const lines = imageMarkNoteLines(1, [ann(1, ''), ann(2, 'kept'), ann(3, '  ')], zhLabel)
    expect(lines).toEqual(['图1:2 kept'])
  })

  it('renumbers after a middle deletion so badges and lines stay in lockstep', () => {
    const list = removeAnnotation([ann(1, 'first'), ann(2, 'second'), ann(3, 'third')], 2)
    expect(imageMarkNoteLines(1, list, zhLabel)).toEqual(['图1:1 first', '图1:2 third'])
  })

  it('yields no lines when every description is blank (image still attaches)', () => {
    expect(imageMarkNoteLines(1, [ann(1), ann(2)], zhLabel)).toEqual([])
  })
})
