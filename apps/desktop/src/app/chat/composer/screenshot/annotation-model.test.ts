import { describe, expect, it } from 'vitest'

import {
  type Annotation,
  imageNoteNumber,
  imageNoteText,
  nextAnnotationId,
  shapeIsMeaningful
} from './annotation-model'

function ann(id: number): Annotation {
  return { id, shape: { kind: 'rect', x0: 0, x1: 10, y0: 0, y1: 10 } }
}

describe('nextAnnotationId', () => {
  it('assigns ids one past the current max', () => {
    expect(nextAnnotationId([ann(1), ann(2)])).toBe(3)
    expect(nextAnnotationId([ann(1)])).toBe(2)
    expect(nextAnnotationId([])).toBe(1)
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

describe('imageNoteNumber', () => {
  it('numbers by existing image count: empty draft → 1, one image → 2', () => {
    expect(imageNoteNumber(0)).toBe(1)
    expect(imageNoteNumber(1)).toBe(2)
    expect(imageNoteNumber(3)).toBe(4)
  })

  it('never goes below 1 on garbage input', () => {
    expect(imageNoteNumber(-5)).toBe(1)
  })
})

describe('imageNoteText', () => {
  it('joins the localized prefix with the trimmed description', () => {
    expect(imageNoteText('图1:', '  看这里  ')).toBe('图1:看这里')
    expect(imageNoteText('Image 2: ', 'check this')).toBe('Image 2: check this')
  })

  it('yields no text for an empty description (image still attaches)', () => {
    expect(imageNoteText('图1:', '')).toBe('')
    expect(imageNoteText('图1:', '   ')).toBe('')
  })
})
