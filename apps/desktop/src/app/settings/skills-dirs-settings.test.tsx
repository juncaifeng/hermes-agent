// @vitest-environment jsdom
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

const getHermesConfigRecord = vi.fn()
const saveHermesConfig = vi.fn()
const getStatus = vi.fn()

vi.mock('@/hermes', () => ({
  getHermesConfigRecord: () => getHermesConfigRecord(),
  saveHermesConfig: (config: unknown) => saveHermesConfig(config),
  getStatus: () => getStatus(),
  setApiRequestProfile: () => {}
}))

const readDesktopDir = vi.fn()
const selectDesktopPaths = vi.fn()
const isDesktopFsRemoteMode = vi.fn()

vi.mock('@/lib/desktop-fs', () => ({
  isDesktopFsRemoteMode: () => isDesktopFsRemoteMode(),
  readDesktopDir: (path: string) => readDesktopDir(path),
  selectDesktopPaths: (options: unknown) => selectDesktopPaths(options)
}))

const notify = vi.fn()
const notifyError = vi.fn()

vi.mock('@/store/notifications', () => ({
  notify: (input: unknown) => notify(input),
  notifyError: (err: unknown, title: unknown) => notifyError(err, title)
}))

const openDir = vi.fn()

const HOME = '/home/u'
const HERMES_HOME = `${HOME}/.hermes`
const BUILTIN_DIR = `${HERMES_HOME}/skills`

function configWith(dirs: string[] | undefined) {
  return {
    model: { provider: 'nous' },
    skills: dirs === undefined ? undefined : { external_dirs: dirs, template_vars: { team: 'a' } }
  }
}

async function renderSettings() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  const { SkillsDirsSettings } = await import('./skills-dirs-settings')
  const onConfigSaved = vi.fn()
  let result: ReturnType<typeof render>

  await act(async () => {
    result = render(
      <QueryClientProvider client={client}>
        <SkillsDirsSettings onConfigSaved={onConfigSaved} />
      </QueryClientProvider>
    )
  })

  return { onConfigSaved, result: result! }
}

beforeEach(() => {
  getStatus.mockResolvedValue({ hermes_home: HERMES_HOME })
  getHermesConfigRecord.mockResolvedValue(configWith(['/ext/a', '/ext/b']))
  saveHermesConfig.mockResolvedValue({ ok: true })
  readDesktopDir.mockResolvedValue({ entries: [] })
  selectDesktopPaths.mockResolvedValue([])
  isDesktopFsRemoteMode.mockReturnValue(false)
  openDir.mockResolvedValue({ ok: true })
  ;(window as unknown as { hermesDesktop: unknown }).hermesDesktop = { openDir }
})

afterEach(() => {
  cleanup()
  vi.clearAllMocks()
  delete (window as unknown as { hermesDesktop?: unknown }).hermesDesktop
})

describe('SkillsDirsSettings', () => {
  it('lists the built-in directory first and every configured external directory', async () => {
    await renderSettings()

    // Built-in row: home-collapsed display path + badge, and no remove button.
    expect(await screen.findByText('~/.hermes/skills')).toBeTruthy()
    expect(screen.getByText('Built-in (writable)')).toBeTruthy()
    expect(screen.getByText('/ext/a')).toBeTruthy()
    expect(screen.getByText('/ext/b')).toBeTruthy()
    expect(screen.queryByRole('button', { name: `Remove ${BUILTIN_DIR}` })).toBeNull()
    expect(screen.getByRole('button', { name: 'Remove /ext/a' })).toBeTruthy()
  })

  it('shows the empty state when no external directories are configured', async () => {
    getHermesConfigRecord.mockResolvedValue(configWith(undefined))

    await renderSettings()

    expect(await screen.findByText('No additional directories configured.')).toBeTruthy()
  })

  it('adds a picked directory, preserving sibling skills config keys', async () => {
    selectDesktopPaths.mockResolvedValue(['/ext/c'])
    const { onConfigSaved } = await renderSettings()

    await screen.findByText('/ext/a')

    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Add directory…' }))
    })

    await waitFor(() => expect(saveHermesConfig).toHaveBeenCalled())
    expect(selectDesktopPaths).toHaveBeenCalledWith(expect.objectContaining({ directories: true }))
    expect(saveHermesConfig).toHaveBeenCalledWith(configWith(['/ext/a', '/ext/b', '/ext/c']))
    expect(onConfigSaved).toHaveBeenCalled()
  })

  it('rejects a duplicate pick without saving', async () => {
    selectDesktopPaths.mockResolvedValue(['/ext/a/'])

    await renderSettings()
    await screen.findByText('/ext/a')

    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Add directory…' }))
    })

    await waitFor(() =>
      expect(notify).toHaveBeenCalledWith(
        expect.objectContaining({ kind: 'warning', message: 'That directory is already in the list.' })
      )
    )
    expect(saveHermesConfig).not.toHaveBeenCalled()
  })

  it('rejects a pick that resolves to the built-in directory', async () => {
    selectDesktopPaths.mockResolvedValue([BUILTIN_DIR])

    await renderSettings()
    await screen.findByText('/ext/a')

    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Add directory…' }))
    })

    await waitFor(() =>
      expect(notify).toHaveBeenCalledWith(
        expect.objectContaining({
          kind: 'warning',
          message: 'That directory resolves to the built-in skills directory.'
        })
      )
    )
    expect(saveHermesConfig).not.toHaveBeenCalled()
  })

  it('warns and does not save when the picked directory cannot be read', async () => {
    selectDesktopPaths.mockResolvedValue(['/gone'])
    readDesktopDir.mockImplementation(async (path: string) =>
      path === '/gone' ? { entries: [], error: 'ENOENT' } : { entries: [] }
    )

    await renderSettings()
    await screen.findByText('/ext/a')

    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Add directory…' }))
    })

    await waitFor(() =>
      expect(notify).toHaveBeenCalledWith(expect.objectContaining({ kind: 'warning', message: 'Directory not found.' }))
    )
    expect(saveHermesConfig).not.toHaveBeenCalled()
  })

  it('removes a directory (config only) when its remove button is clicked', async () => {
    await renderSettings()
    await screen.findByText('/ext/a')

    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Remove /ext/a' }))
    })

    await waitFor(() => expect(saveHermesConfig).toHaveBeenCalledWith(configWith(['/ext/b'])))
  })

  it('flags unreadable directories and cleans them all up on demand', async () => {
    readDesktopDir.mockImplementation(async (path: string) =>
      path === '/ext/b' ? { entries: [], error: 'ENOENT' } : { entries: [] }
    )

    await renderSettings()
    await screen.findByText('/ext/a')

    // The missing row is flagged, the healthy row is not, and the cleanup
    // action appears in the section heading.
    expect(await screen.findByText('Missing')).toBeTruthy()
    const cleanupButton = await screen.findByRole('button', { name: /Remove missing directories/ })

    await act(async () => {
      fireEvent.click(cleanupButton)
    })

    await waitFor(() => expect(saveHermesConfig).toHaveBeenCalledWith(configWith(['/ext/a'])))
  })

  it('opens a directory via the desktop bridge', async () => {
    await renderSettings()
    await screen.findByText('/ext/a')

    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Open /ext/a' }))
    })

    expect(openDir).toHaveBeenCalledWith('/ext/a')
  })
})
