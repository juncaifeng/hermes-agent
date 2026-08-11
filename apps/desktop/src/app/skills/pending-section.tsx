import { useQuery } from '@tanstack/react-query'
import { useState } from 'react'

import { Button } from '@/components/ui/button'
import {
  approvePendingSkillWrite,
  getPendingSkillWrites,
  getPendingSkillWriteDiff,
  rejectPendingSkillWrite
} from '@/hermes'
import { useI18n } from '@/i18n'
import { queryClient } from '@/lib/query-client'
import { readWriteApproval } from '@/lib/skills-dirs'
import { invalidateSlashCompletions } from '@/lib/slash-completion-cache'
import { relativeTime } from '@/lib/time'
import { notify, notifyError } from '@/store/notifications'
import type { PendingSkillWrite } from '@/types/hermes'

import { useHermesConfigRecord } from '../hooks/use-config-record'
import { PanelPill } from '../overlays/panel'

import { SKILLS_PENDING_QUERY_KEY, SKILLS_QUERY_KEY, SKILLS_USAGE_QUERY_KEY } from './store'

const ACTION_TONE: Record<string, 'bad' | 'good' | 'muted'> = {
  create: 'good',
  delete: 'bad',
  remove_file: 'bad'
}

function actionLabel(t: ReturnType<typeof useI18n>['t'], action: string): string {
  const known = t.skills.pending.actions as Record<string, string>

  return known[action] ?? action
}

// Full diff / content for one staged write — fetched on first expand, then
// served from the RQ cache.
function PendingDiff({ id }: { id: string }) {
  const { t } = useI18n()
  const { data, isPending, isError } = useQuery({
    queryFn: () => getPendingSkillWriteDiff(id),
    queryKey: ['skills-pending-diff', id]
  })

  if (isPending) {
    return (
      <p className="px-2 py-1 text-[length:var(--conversation-caption-font-size)] text-(--ui-text-tertiary)">
        {t.skills.loading}
      </p>
    )
  }

  if (isError || !data) {
    return null
  }

  return (
    <pre className="max-h-72 overflow-auto rounded-md bg-(--ui-bg-quinary) p-2 font-mono text-[0.68rem] leading-4 whitespace-pre-wrap">
      {data.diff}
    </pre>
  )
}

function PendingRow({
  busy,
  item,
  onApprove,
  onReject
}: {
  busy: boolean
  item: PendingSkillWrite
  onApprove: () => void
  onReject: () => void
}) {
  const { t } = useI18n()
  const p = t.skills.pending
  const [expanded, setExpanded] = useState(false)

  return (
    <div className="py-1.5">
      <div className="flex min-w-0 items-center gap-2">
        <PanelPill tone={ACTION_TONE[item.action] ?? 'muted'}>{actionLabel(t, item.action)}</PanelPill>
        <span className="shrink-0 text-[length:var(--conversation-text-font-size)] font-medium">{item.name}</span>
        {item.origin === 'background_review' && <PanelPill>{p.autoOrigin}</PanelPill>}
        <span className="min-w-0 flex-1 truncate text-[length:var(--conversation-caption-font-size)] text-(--ui-text-tertiary)">
          {item.gist}
        </span>
        <span className="shrink-0 text-[length:var(--conversation-caption-font-size)] text-(--ui-text-quaternary)">
          {relativeTime(item.created_at * 1000)}
        </span>
      </div>
      <div className="mt-1 flex items-center gap-1.5 pl-1">
        <Button disabled={busy} onClick={onApprove} size="xs" variant="textStrong">
          {p.approve}
        </Button>
        <Button
          className="text-destructive hover:text-destructive"
          disabled={busy}
          onClick={onReject}
          size="xs"
          variant="text"
        >
          {p.reject}
        </Button>
        <Button onClick={() => setExpanded(on => !on)} size="xs" variant="text">
          {expanded ? p.hideDiff : p.showDiff}
        </Button>
      </div>
      {expanded && <PendingDiff id={item.id} />}
    </div>
  )
}

// The write-approval queue on the Skills tab. Hidden entirely unless the gate
// is on AND there is something to review — an empty queue is not a section.
export function PendingSkillsSection() {
  const { t } = useI18n()
  const p = t.skills.pending
  const { data: config } = useHermesConfigRecord()
  const gateOn = readWriteApproval(config)
  const [busyId, setBusyId] = useState<string | null>(null)

  const { data: items } = useQuery({
    enabled: gateOn,
    queryFn: getPendingSkillWrites,
    queryKey: SKILLS_PENDING_QUERY_KEY,
    refetchInterval: 30_000
  })

  if (!gateOn || !items?.length) {
    return null
  }

  const settle = async (id: string, action: 'approve' | 'reject') => {
    const item = items.find(row => row.id === id)

    if (!item || busyId) {
      return
    }

    setBusyId(id)

    try {
      if (action === 'approve') {
        await approvePendingSkillWrite(id)
        notify({ kind: 'success', message: p.approved(item.name) })
        // The skill library + learning feed changed with the replayed write.
        void queryClient.invalidateQueries({ queryKey: SKILLS_QUERY_KEY })
        void queryClient.invalidateQueries({ queryKey: SKILLS_USAGE_QUERY_KEY })
        invalidateSlashCompletions()
      } else {
        await rejectPendingSkillWrite(id)
        notify({ kind: 'info', message: p.rejected(item.name) })
      }

      void queryClient.invalidateQueries({ queryKey: SKILLS_PENDING_QUERY_KEY })
    } catch (err) {
      notifyError(err, p.failed(item.name))
    } finally {
      setBusyId(null)
    }
  }

  return (
    <section className="max-h-72 shrink-0 overflow-y-auto border-b border-(--ui-stroke-tertiary) px-3 py-2">
      <header className="mb-1 flex items-center gap-2">
        <span className="text-[length:var(--conversation-text-font-size)] font-medium">{p.title}</span>
        <PanelPill tone="warn">{p.count(items.length)}</PanelPill>
      </header>
      <div className="divide-y divide-(--ui-stroke-tertiary)">
        {items.map(item => (
          <PendingRow
            busy={busyId === item.id}
            item={item}
            key={item.id}
            onApprove={() => void settle(item.id, 'approve')}
            onReject={() => void settle(item.id, 'reject')}
          />
        ))}
      </div>
    </section>
  )
}
