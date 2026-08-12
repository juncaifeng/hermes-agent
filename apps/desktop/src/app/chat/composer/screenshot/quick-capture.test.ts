// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { mainComposerScope } from '@/store/composer'
import { $screenshotDirMode } from '@/store/screenshots'
import { $currentCwd } from '@/store/session'

const saveImageBuffer = vi.fn()

function installBridge() {
  ;(window as unknown as { hermesDesktop: unknown }).hermesDesktop = {
    readFileDataUrl: vi.fn(async () => 'data:image/png;base64,preview'),
    saveImageBuffer
  }
}

beforeEach(() => {
  mainComposerScope.clear()
  $screenshotDirMode.set('app_data')
  $currentCwd.set('')
  saveImageBuffer.mockResolvedValue('/appdata/composer-images/shot.png')
  installBridge()
})

afterEach(() => {
  vi.clearAllMocks()
  delete (window as unknown as { hermesDesktop?: unknown }).hermesDesktop
  localStorage.clear()
})

describe('attachScreenshotToMain', () => {
  it('saves to app-data by default and attaches the image to the main composer', async () => {
    const { attachScreenshotToMain } = await import('./quick-capture')

    const ok = await attachScreenshotToMain(new Blob(['png'], { type: 'image/png' }))

    expect(ok).toBe(true)
    expect(saveImageBuffer).toHaveBeenCalledWith(expect.any(Uint8Array), '.png', undefined)
    const attachments = mainComposerScope.$attachments.get()
    expect(attachments.some(a => a.kind === 'image' && a.path === '/appdata/composer-images/shot.png')).toBe(true)
  })

  it('routes project mode into <workspace>/.hermes/screenshots', async () => {
    $screenshotDirMode.set('project')
    $currentCwd.set('/repo')
    saveImageBuffer.mockResolvedValue('/repo/.hermes/screenshots/shot.png')

    const { attachScreenshotToMain } = await import('./quick-capture')

    await attachScreenshotToMain(new Blob(['png'], { type: 'image/png' }))

    expect(saveImageBuffer).toHaveBeenCalledWith(expect.any(Uint8Array), '.png', '/repo/.hermes/screenshots')
  })

  it('project mode without a workspace falls back to app-data (undefined dir)', async () => {
    $screenshotDirMode.set('project')
    $currentCwd.set('')

    const { attachScreenshotToMain } = await import('./quick-capture')

    await attachScreenshotToMain(new Blob(['png'], { type: 'image/png' }))

    expect(saveImageBuffer).toHaveBeenCalledWith(expect.any(Uint8Array), '.png', undefined)
  })

  it('returns false when the save fails — no attachment, no crash', async () => {
    saveImageBuffer.mockResolvedValue('')

    const { attachScreenshotToMain } = await import('./quick-capture')

    expect(await attachScreenshotToMain(new Blob(['png'], { type: 'image/png' }))).toBe(false)
    expect(mainComposerScope.$attachments.get()).toEqual([])
  })
})

describe('nextMainScreenshotNumber', () => {
  it('counts only image attachments, then adds one', async () => {
    const { nextMainScreenshotNumber } = await import('./quick-capture')

    expect(nextMainScreenshotNumber()).toBe(1)

    mainComposerScope.add({ id: 'f1', kind: 'file', label: 'notes.txt' })
    expect(nextMainScreenshotNumber()).toBe(1)

    mainComposerScope.add({ id: 'i1', kind: 'image', label: 'a.png' })
    expect(nextMainScreenshotNumber()).toBe(2)

    mainComposerScope.add({ id: 'i2', kind: 'image', label: 'b.png' })
    expect(nextMainScreenshotNumber()).toBe(3)
  })
})
