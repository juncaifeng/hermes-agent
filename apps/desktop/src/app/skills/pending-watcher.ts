import { useQuery } from '@tanstack/react-query'
import { useEffect, useRef } from 'react'
import { useNavigate } from 'react-router'

import { getPendingSkillWrites } from '@/hermes'
import { useI18n } from '@/i18n'
import { readWriteApproval } from '@/lib/skills-dirs'
import { notify } from '@/store/notifications'

import { useHermesConfigRecord } from '../hooks/use-config-record'
import { SKILLS_ROUTE } from '../routes'

import { $pendingSkillWriteCount, SKILLS_PENDING_QUERY_KEY } from './store'

// Ambient poll of the skills write-approval queue. Mounted once (sidebar);
// polls only while the gate (skills.write_approval) is on. Keeps the shared
// count atom current for the sidebar/tab badges and raises one toast on the
// rising edge (0 → N) — an already-nonempty queue at app start is not news.
export function useSkillsPendingWatcher(): void {
  const { t } = useI18n()
  const navigate = useNavigate()
  const { data: config } = useHermesConfigRecord()
  const gateOn = readWriteApproval(config)

  const { data } = useQuery({
    enabled: gateOn,
    queryFn: getPendingSkillWrites,
    queryKey: SKILLS_PENDING_QUERY_KEY,
    refetchInterval: 30_000
  })

  const count = gateOn ? (data?.length ?? 0) : 0

  useEffect(() => {
    $pendingSkillWriteCount.set(count)

    return () => $pendingSkillWriteCount.set(0)
  }, [count])

  // null = no successful read yet (suppress the first paint); a number is the
  // previously observed queue size. Toast only when that was 0.
  const lastSeen = useRef<number | null>(null)

  useEffect(() => {
    if (!gateOn) {
      lastSeen.current = null

      return
    }

    if (data === undefined) {
      return
    }

    const n = data.length

    if (lastSeen.current === 0 && n > 0) {
      notify({
        action: { label: t.skills.pending.toastAction, onClick: () => navigate(SKILLS_ROUTE) },
        kind: 'info',
        message: t.skills.pending.toast(n)
      })
    }

    lastSeen.current = n
  }, [data, gateOn, navigate, t])
}
