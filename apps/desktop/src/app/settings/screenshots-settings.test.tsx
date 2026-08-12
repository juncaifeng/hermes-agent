// @vitest-environment jsdom
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest'

import { $screenshotDirMode, $screenshotGitExclude, $screenshotOverlayEnabled } from '@/store/screenshots'
import { $currentCwd } from '@/store/session'

const screenshotDirStats = vi.fn()
const clearScreenshotDir = vi.fn()
const excludePathFromGit = vi.fn()
const setScreenshotOverlayEnabled = vi.fn()

const notify = vi.fn()
const notifyError = vi.fn()

vi.mock('@/store/notifications', () => ({
  notify: (input: unknown) => notify(input),
  notifyError: (err: unknown, title: unknown) => notifyError(err, title)
}))

// Radix Select scrolls items into view on open; jsdom lacks these.
beforeAll(() => {
  Element.prototype.scrollIntoView = vi.fn()
  Element.prototype.hasPointerCapture = vi.fn(() => false)
  Element.prototype.releasePointerCapture = vi.fn()
})

async function renderSettings() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  const { ScreenshotsSettings } = await import('./screenshots-settings')

  await act(async () => {
    render(
      <QueryClientProvider client={client}>
        <ScreenshotsSettings />
      </QueryClientProvider>
    )
  })
}

beforeEach(() => {
  $screenshotDirMode.set('app_data')
  $screenshotGitExclude.set(false)
  $screenshotOverlayEnabled.set(false)
  $currentCwd.set('/repo')
  ;(window as unknown as { hermesDesktop: unknown }).hermesDesktop = {
    clearScreenshotDir,
    excludePathFromGit,
    screenshotDirStats,
    setScreenshotOverlayEnabled
  }
  screenshotDirStats.mockResolvedValue([
    { bytes: 2048, exists: true, files: 2, kind: 'app_data', path: '/appdata/composer-images' },
    { bytes: 1024, exists: true, files: 1, kind: 'project', path: '/repo/.hermes/screenshots' }
  ])
  clearScreenshotDir.mockImplementation(async (path: string) => ({
    freed_bytes: path.includes('appdata') ? 2048 : 1024,
    path,
    removed_files: path.includes('appdata') ? 2 : 1
  }))
  excludePathFromGit.mockResolvedValue('added')
})

afterEach(() => {
  cleanup()
  vi.clearAllMocks()
  vi.restoreAllMocks()
  delete (window as unknown as { hermesDesktop?: unknown }).hermesDesktop
  localStorage.clear()
})

describe('ScreenshotsSettings', () => {
  it('lists both managed directories with file counts', async () => {
    await renderSettings()

    expect(await screen.findByText('/appdata/composer-images')).toBeTruthy()
    expect(screen.getByText('/repo/.hermes/screenshots')).toBeTruthy()
    expect(screen.getByText('2 files · 2 KB')).toBeTruthy()
    expect(screen.getByText('1 file · 1 KB')).toBeTruthy()
    expect(screenshotDirStats).toHaveBeenCalledWith('/repo')
  })

  it('shows the git-exclude toggle only in project mode', async () => {
    await renderSettings()
    await screen.findByText('/appdata/composer-images')

    expect(screen.queryByRole('switch', { name: 'Exclude from git' })).toBeNull()

    // Switch to project mode via the Select.
    fireEvent.click(screen.getByRole('combobox'))
    await act(async () => {
      fireEvent.click(await screen.findByRole('option', { name: 'Current project (.hermes/screenshots)' }))
    })

    expect($screenshotDirMode.get()).toBe('project')
    expect(await screen.findByRole('switch', { name: 'Exclude from git' })).toBeTruthy()
  })

  it('writes the git exclude for the current workspace when toggled on', async () => {
    $screenshotDirMode.set('project')
    await renderSettings()

    await act(async () => {
      fireEvent.click(await screen.findByRole('switch', { name: 'Exclude from git' }))
    })

    await waitFor(() => expect(excludePathFromGit).toHaveBeenCalledWith('/repo', '.hermes/screenshots/'))
    expect(notify).toHaveBeenCalledWith(
      expect.objectContaining({ kind: 'success', message: 'Added to .git/info/exclude.' })
    )
  })

  it('cleans every listed directory after confirmation, and reports the freed space', async () => {
    const confirmSpy = vi.spyOn(window, 'confirm').mockReturnValue(true)

    await renderSettings()
    await screen.findByText('/appdata/composer-images')

    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: /Clean up/ }))
    })

    expect(confirmSpy).toHaveBeenCalledWith(expect.stringContaining('no longer load'))
    await waitFor(() => expect(clearScreenshotDir).toHaveBeenCalledTimes(2))
    expect(clearScreenshotDir).toHaveBeenCalledWith('/appdata/composer-images')
    expect(clearScreenshotDir).toHaveBeenCalledWith('/repo/.hermes/screenshots')
    expect(notify).toHaveBeenCalledWith(
      expect.objectContaining({ kind: 'success', message: 'Removed 3 files, freed 3 KB.' })
    )
  })

  it('does not clean when the confirmation is declined', async () => {
    vi.spyOn(window, 'confirm').mockReturnValue(false)

    await renderSettings()
    await screen.findByText('/appdata/composer-images')

    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: /Clean up/ }))
    })

    expect(clearScreenshotDir).not.toHaveBeenCalled()
  })

  it('persists the floating-button toggle to the shared preference', async () => {
    await renderSettings()

    await act(async () => {
      fireEvent.click(await screen.findByRole('switch', { name: 'Floating screenshot button' }))
    })

    expect($screenshotOverlayEnabled.get()).toBe(true)
  })
})
