import { useQuery } from '@tanstack/react-query'
import { useMemo, useState } from 'react'

import { Button } from '@/components/ui/button'
import { Codicon } from '@/components/ui/codicon'
import { getSkillsUsage } from '@/hermes'
import { useI18n } from '@/i18n'
import { relativeTime } from '@/lib/time'
import type { SkillUsageRow } from '@/types/hermes'

import { PanelPill } from '../overlays/panel'

import { SKILLS_USAGE_QUERY_KEY } from './store'

const FEED_LIMIT = 8

function feedTime(row: SkillUsageRow): string {
  return row.last_activity_at ?? row.created_at ?? ''
}

// "Recently learned / patched" strip for the Skills tab. Purely a read
// surface over the curator sidecar (/api/skills/usage) — rollback lives with
// `hermes curator` (phase 2 may add a button). Collapsed by default so the
// capability list keeps the page.
export function LearningFeedSection() {
  const { t } = useI18n()
  const f = t.skills.feed
  const [open, setOpen] = useState(false)

  const { data } = useQuery({ queryFn: getSkillsUsage, queryKey: SKILLS_USAGE_QUERY_KEY })

  const rows = useMemo(() => {
    // Patched skills first (a patch is the strongest "Hermes changed
    // something" signal), then by most recent activity. Skills with neither
    // activity nor a creation record have nothing to report.
    return (data ?? [])
      .filter(row => row.last_activity_at || row.created_at)
      .sort((a, b) => Number(b.patch_count > 0) - Number(a.patch_count > 0) || feedTime(b).localeCompare(feedTime(a)))
      .slice(0, FEED_LIMIT)
  }, [data])

  if (!data || rows.length === 0) {
    return null
  }

  return (
    <section className="shrink-0 border-b border-(--ui-stroke-tertiary) px-3 py-1.5">
      <Button
        aria-expanded={open}
        className="h-6 gap-1.5 px-1 text-[length:var(--conversation-caption-font-size)]"
        onClick={() => setOpen(on => !on)}
        size="sm"
        variant="ghost"
      >
        <Codicon name={open ? 'chevron-down' : 'chevron-right'} size="0.75rem" />
        {f.title}
        <span className="text-(--ui-text-quaternary)">{rows.length}</span>
      </Button>

      {open && (
        <div className="max-h-56 overflow-y-auto pb-1 pt-0.5">
          {rows.map(row => {
            const state = (t.skills.feed.states as Record<string, string>)[row.state] ?? row.state
            const stamp = feedTime(row)

            return (
              <div className="flex min-w-0 items-center gap-2 py-0.5 pl-1" key={row.name}>
                <span className="shrink-0 text-[length:var(--conversation-text-font-size)] font-medium">
                  {row.name}
                </span>
                {row.patch_count > 0 && <PanelPill tone="good">{f.patches(row.patch_count)}</PanelPill>}
                {row.state !== 'active' && <PanelPill tone="warn">{state}</PanelPill>}
                <span className="ml-auto shrink-0 text-[length:var(--conversation-caption-font-size)] text-(--ui-text-quaternary)">
                  {stamp ? relativeTime(Date.parse(stamp)) : ''}
                </span>
              </div>
            )
          })}
        </div>
      )}
    </section>
  )
}
