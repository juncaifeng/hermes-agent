/**
 * Presentation model for the quick-screenshot window picker: group the
 * bridge's flat window list by process for a scannable popover. Pure —
 * the component just maps over the result.
 */

import type { HermesWindowInfo } from '@/global'

export interface WindowGroup {
  /** Display name for the group header ("unknown" buckets last). */
  process: string
  windows: HermesWindowInfo[]
}

export const UNKNOWN_PROCESS = ''

export function groupWindowsByProcess(windows: HermesWindowInfo[]): WindowGroup[] {
  const byProcess = new Map<string, HermesWindowInfo[]>()

  for (const win of windows) {
    const key = win.process || UNKNOWN_PROCESS
    const bucket = byProcess.get(key)

    if (bucket) {
      bucket.push(win)
    } else {
      byProcess.set(key, [win])
    }
  }

  return [...byProcess.entries()]
    .map(([process, wins]) => ({
      process,
      windows: [...wins].sort((a, b) => a.title.localeCompare(b.title))
    }))
    .sort((a, b) => {
      // Named processes A–Z; the unnamed bucket sinks to the bottom.
      if (a.process === UNKNOWN_PROCESS) {
        return b.process === UNKNOWN_PROCESS ? 0 : 1
      }

      if (b.process === UNKNOWN_PROCESS) {
        return -1
      }

      return a.process.localeCompare(b.process)
    })
}
