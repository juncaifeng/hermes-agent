// @vitest-environment jsdom
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

const getPendingSkillWrites = vi.fn()
const approvePendingSkillWrite = vi.fn()
const rejectPendingSkillWrite = vi.fn()
const getPendingSkillWriteDiff = vi.fn()

vi.mock('@/hermes', () => ({
  approvePendingSkillWrite: (id: string) => approvePendingSkillWrite(id),
  getHermesConfigRecord: () => Promise.resolve({}),
  getPendingSkillWriteDiff: (id: string) => getPendingSkillWriteDiff(id),
  getPendingSkillWrites: () => getPendingSkillWrites(),
  rejectPendingSkillWrite: (id: string) => rejectPendingSkillWrite(id),
  saveHermesConfig: () => Promise.resolve({ ok: true }),
  setApiRequestProfile: () => {}
}))

let gateOn = true

vi.mock('../hooks/use-config-record', () => ({
  useHermesConfigRecord: () => ({ data: { skills: { write_approval: gateOn } } })
}))

const notify = vi.fn()
const notifyError = vi.fn()

vi.mock('@/store/notifications', () => ({
  notify: (input: unknown) => notify(input),
  notifyError: (err: unknown, title: unknown) => notifyError(err, title)
}))

function pendingItem(overrides: Record<string, unknown> = {}) {
  return {
    action: 'create',
    created_at: 1_786_000_000,
    gist: "create 'alpha' — A staged skill. (1 KB)",
    id: 'abc12345',
    name: 'alpha',
    origin: 'background_review',
    ...overrides
  }
}

async function renderSection() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  const { PendingSkillsSection } = await import('./pending-section')
  let result: ReturnType<typeof render>

  await act(async () => {
    result = render(
      <QueryClientProvider client={client}>
        <PendingSkillsSection />
      </QueryClientProvider>
    )
  })

  return result!
}

beforeEach(() => {
  gateOn = true
  getPendingSkillWrites.mockResolvedValue([pendingItem()])
  approvePendingSkillWrite.mockResolvedValue({ success: true })
  rejectPendingSkillWrite.mockResolvedValue({ id: 'abc12345', ok: true })
  getPendingSkillWriteDiff.mockResolvedValue({
    ...pendingItem(),
    diff: '--- a/SKILL.md\n+++ b/SKILL.md\n+new',
    file_path: 'SKILL.md'
  })
})

afterEach(() => {
  cleanup()
  vi.clearAllMocks()
})

describe('PendingSkillsSection', () => {
  it('renders nothing when the approval gate is off', async () => {
    gateOn = false

    const { container } = await renderSection()

    expect(container.textContent).toBe('')
    expect(getPendingSkillWrites).not.toHaveBeenCalled()
  })

  it('renders nothing when the queue is empty', async () => {
    getPendingSkillWrites.mockResolvedValue([])

    const { container } = await renderSection()

    await waitFor(() => expect(getPendingSkillWrites).toHaveBeenCalled())
    expect(container.textContent).toBe('')
  })

  it('lists each staged write with action badge, name, gist and origin', async () => {
    await renderSection()

    expect(await screen.findByText('alpha')).toBeTruthy()
    expect(screen.getByText('new')).toBeTruthy()
    expect(screen.getByText("create 'alpha' — A staged skill. (1 KB)")).toBeTruthy()
    expect(screen.getByText('auto-review')).toBeTruthy()
    expect(screen.getByRole('button', { name: 'Approve' })).toBeTruthy()
    expect(screen.getByRole('button', { name: 'Reject' })).toBeTruthy()
  })

  it('approves a staged write and reports success', async () => {
    await renderSection()
    await screen.findByText('alpha')

    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Approve' }))
    })

    await waitFor(() => expect(approvePendingSkillWrite).toHaveBeenCalledWith('abc12345'))
    expect(notify).toHaveBeenCalledWith(
      expect.objectContaining({ kind: 'success', message: 'Approved alpha — applies to new sessions.' })
    )
  })

  it('rejects a staged write without applying it', async () => {
    await renderSection()
    await screen.findByText('alpha')

    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Reject' }))
    })

    await waitFor(() => expect(rejectPendingSkillWrite).toHaveBeenCalledWith('abc12345'))
    expect(approvePendingSkillWrite).not.toHaveBeenCalled()
    expect(notify).toHaveBeenCalledWith(
      expect.objectContaining({ kind: 'info', message: 'Rejected alpha. Nothing was written.' })
    )
  })

  it('lazily loads the diff on expand', async () => {
    await renderSection()
    await screen.findByText('alpha')

    expect(getPendingSkillWriteDiff).not.toHaveBeenCalled()

    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Show diff' }))
    })

    expect(await screen.findByText(/\+new/)).toBeTruthy()
    expect(getPendingSkillWriteDiff).toHaveBeenCalledWith('abc12345')
  })
})
