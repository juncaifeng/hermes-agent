// @vitest-environment jsdom
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { act, cleanup, render, waitFor } from '@testing-library/react'
import { MemoryRouter } from 'react-router'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

const getPendingSkillWrites = vi.fn()

vi.mock('@/hermes', () => ({
  getHermesConfigRecord: () => Promise.resolve({}),
  getPendingSkillWrites: () => getPendingSkillWrites(),
  saveHermesConfig: () => Promise.resolve({ ok: true }),
  setApiRequestProfile: () => {}
}))

let gateOn = true

vi.mock('../hooks/use-config-record', () => ({
  useHermesConfigRecord: () => ({ data: { skills: { write_approval: gateOn } } })
}))

const notify = vi.fn()

vi.mock('@/store/notifications', () => ({
  notify: (input: unknown) => notify(input),
  notifyError: vi.fn()
}))

import { $pendingSkillWriteCount } from './store'

async function renderWatcher(client: QueryClient) {
  // Late import so the mocks are in place.
  const { useSkillsPendingWatcher } = await import('./pending-watcher')

  function Probe() {
    useSkillsPendingWatcher()

    return null
  }

  await act(async () => {
    render(
      <QueryClientProvider client={client}>
        <MemoryRouter>
          <Probe />
        </MemoryRouter>
      </QueryClientProvider>
    )
  })
}

const ITEM = {
  action: 'create',
  created_at: 1_786_000_000,
  gist: "create 'alpha'",
  id: 'abc12345',
  name: 'alpha',
  origin: 'background_review'
}

beforeEach(() => {
  gateOn = true
  $pendingSkillWriteCount.set(0)
  getPendingSkillWrites.mockResolvedValue([])
})

afterEach(() => {
  cleanup()
  vi.clearAllMocks()
  $pendingSkillWriteCount.set(0)
})

describe('useSkillsPendingWatcher', () => {
  it('does not poll while the gate is off', async () => {
    gateOn = false

    await renderWatcher(new QueryClient({ defaultOptions: { queries: { retry: false } } }))

    expect(getPendingSkillWrites).not.toHaveBeenCalled()
    expect($pendingSkillWriteCount.get()).toBe(0)
    expect(notify).not.toHaveBeenCalled()
  })

  it('publishes the count without toasting on the first read', async () => {
    // Opening the app with an already-nonempty queue is not news.
    getPendingSkillWrites.mockResolvedValue([ITEM, ITEM])

    await renderWatcher(new QueryClient({ defaultOptions: { queries: { retry: false } } }))

    await waitFor(() => expect($pendingSkillWriteCount.get()).toBe(2))
    expect(notify).not.toHaveBeenCalled()
  })

  it('toasts once on the 0 → N rising edge, not on every poll', async () => {
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } })

    await renderWatcher(client)
    await waitFor(() => expect(getPendingSkillWrites).toHaveBeenCalledTimes(1))
    expect(notify).not.toHaveBeenCalled()

    // The background review stages a write → next poll sees the queue grow.
    getPendingSkillWrites.mockResolvedValue([ITEM, ITEM])

    await act(async () => {
      await client.invalidateQueries()
    })

    await waitFor(() =>
      expect(notify).toHaveBeenCalledWith(
        expect.objectContaining({ kind: 'info', message: '2 skill updates waiting for approval' })
      )
    )
    expect($pendingSkillWriteCount.get()).toBe(2)

    // A later poll with the same count must NOT toast again.
    await act(async () => {
      await client.invalidateQueries()
    })

    await waitFor(() => expect(getPendingSkillWrites).toHaveBeenCalledTimes(3))
    expect(notify).toHaveBeenCalledTimes(1)
  })
})
