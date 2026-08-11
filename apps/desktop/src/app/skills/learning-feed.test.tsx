// @vitest-environment jsdom
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

const getSkillsUsage = vi.fn()

vi.mock('@/hermes', () => ({
  getHermesConfigRecord: () => Promise.resolve({}),
  getSkillsUsage: () => getSkillsUsage(),
  saveHermesConfig: () => Promise.resolve({ ok: true }),
  setApiRequestProfile: () => {}
}))

function usageRow(overrides: Record<string, unknown> = {}) {
  return {
    created_at: '2026-07-01T00:00:00+00:00',
    last_activity_at: '2026-08-01T00:00:00+00:00',
    name: 'skill',
    patch_count: 0,
    state: 'active',
    use_count: 1,
    ...overrides
  }
}

async function renderFeed() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  const { LearningFeedSection } = await import('./learning-feed')
  let result: ReturnType<typeof render>

  await act(async () => {
    result = render(
      <QueryClientProvider client={client}>
        <LearningFeedSection />
      </QueryClientProvider>
    )
  })

  return result!
}

beforeEach(() => {
  getSkillsUsage.mockResolvedValue([
    // Never patched but more recent — must sort AFTER a patched row.
    usageRow({ last_activity_at: '2026-08-10T00:00:00+00:00', name: 'recent-unpatched' }),
    usageRow({ last_activity_at: '2026-08-05T00:00:00+00:00', name: 'patched-skill', patch_count: 2 }),
    // No activity and no creation record → nothing to report, filtered out.
    usageRow({ created_at: null, last_activity_at: null, name: 'untouched' })
  ])
})

afterEach(() => {
  cleanup()
  vi.clearAllMocks()
})

describe('LearningFeedSection', () => {
  it('renders nothing when there is no activity at all', async () => {
    getSkillsUsage.mockResolvedValue([])

    const { container } = await renderFeed()

    await waitFor(() => expect(getSkillsUsage).toHaveBeenCalled())
    expect(container.textContent).toBe('')
  })

  it('stays collapsed until toggled, then lists activity rows', async () => {
    await renderFeed()

    const toggle = await screen.findByRole('button', { name: /Learning activity/ })
    expect(screen.queryByText('patched-skill')).toBeNull()

    await act(async () => {
      fireEvent.click(toggle)
    })

    expect(await screen.findByText('patched-skill')).toBeTruthy()
    expect(screen.getByText('recent-unpatched')).toBeTruthy()
    expect(screen.queryByText('untouched')).toBeNull()
  })

  it('sorts patched skills ahead of merely-recent ones and shows state', async () => {
    getSkillsUsage.mockResolvedValue([
      usageRow({ last_activity_at: '2026-08-10T00:00:00+00:00', name: 'recent-unpatched' }),
      usageRow({
        last_activity_at: '2026-08-05T00:00:00+00:00',
        name: 'patched-skill',
        patch_count: 2,
        state: 'stale'
      })
    ])

    const { container } = await renderFeed()
    const toggle = await screen.findByRole('button', { name: /Learning activity/ })

    await act(async () => {
      fireEvent.click(toggle)
    })

    await screen.findByText('patched-skill')
    const text = container.textContent ?? ''

    expect(text.indexOf('patched-skill')).toBeGreaterThanOrEqual(0)
    expect(text.indexOf('patched-skill')).toBeLessThan(text.indexOf('recent-unpatched'))
    expect(screen.getByText('2 patches')).toBeTruthy()
    expect(screen.getByText('stale')).toBeTruthy()
  })
})
