import { describe, expect, it } from 'vitest'

import { localPreviewTarget } from './local-preview'

// Path-shape contracts that broke the preview read on Windows (os error 123):
// file:// URLs decoded with the URL-grammar leading slash, and backslash
// drive-absolute paths misclassified as relative and joined onto the cwd.
describe('localPreviewTarget path shapes', () => {
  it('strips the leading slash from drive-letter file:// URLs', () => {
    const target = localPreviewTarget('file:///E:/repo/docs/a.md')
    expect(target?.kind).toBe('file')
    expect(target?.path).toBe('E:/repo/docs/a.md')
  })

  it('does not join backslash drive-absolute paths onto the cwd', () => {
    const target = localPreviewTarget(String.raw`E:\repo\docs\a.md`, 'E:\\repo')
    expect(target?.path).toBe(String.raw`E:\repo\docs\a.md`)
  })

  it('does not join forward-slash drive-absolute paths onto the cwd', () => {
    const target = localPreviewTarget('E:/repo/docs/a.md', 'E:\\repo')
    expect(target?.path).toBe('E:/repo/docs/a.md')
  })

  it('does not join UNC paths onto the cwd', () => {
    const target = localPreviewTarget(String.raw`\\server\share\a.md`, 'E:\\repo')
    expect(target?.path).toBe(String.raw`\\server\share\a.md`)
  })

  it('joins genuinely relative paths onto the cwd', () => {
    const target = localPreviewTarget('docs/a.md', 'E:\\repo')
    expect(target?.path).toBe('E:\\repo/docs/a.md')
  })

  it('passes http(s) targets through as url previews', () => {
    const target = localPreviewTarget('https://example.com/x')
    expect(target).toEqual({ kind: 'url', label: 'x', source: 'https://example.com/x', url: 'https://example.com/x' })
  })
})
