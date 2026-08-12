import { describe, expect, it } from 'vitest'

import { resolveScreenshotDir } from './save-dir'

describe('resolveScreenshotDir', () => {
  it('app-data mode never overrides the directory', () => {
    expect(resolveScreenshotDir('app_data', '/repo')).toBeUndefined()
    expect(resolveScreenshotDir('app_data', null)).toBeUndefined()
  })

  it('project mode targets <workspace>/.hermes/screenshots', () => {
    expect(resolveScreenshotDir('project', '/repo')).toBe('/repo/.hermes/screenshots')
    expect(resolveScreenshotDir('project', 'C:\\repo\\')).toBe('C:/repo/.hermes/screenshots')
  })

  it('project mode without a workspace falls back to app-data (undefined)', () => {
    expect(resolveScreenshotDir('project', null)).toBeUndefined()
    expect(resolveScreenshotDir('project', undefined)).toBeUndefined()
    expect(resolveScreenshotDir('project', '  ')).toBeUndefined()
  })
})
