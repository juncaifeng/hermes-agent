import { atom } from 'nanostores'

import { Codecs, persistentAtom } from '@/lib/persisted'

// RQ cache key for the /api/skills list. Exported so other surfaces (Settings
// → Skill Directories) can invalidate it when the scanned directories change.
export const SKILLS_QUERY_KEY = ['skills-list'] as const

// RQ cache key for the skills write-approval queue (/api/skills/pending).
export const SKILLS_PENDING_QUERY_KEY = ['skills-pending'] as const

// RQ cache key for the learning-activity feed (/api/skills/usage).
export const SKILLS_USAGE_QUERY_KEY = ['skills-usage'] as const

// Live count of staged skill writes awaiting approval. Written by
// useSkillsPendingWatcher (mounted once in the sidebar); read by the sidebar
// nav badge and the Capabilities → Skills tab badge. 0 when the gate is off.
export const $pendingSkillWriteCount = atom(0)

// Per-view sort direction for the Capabilities lists — persisted so each tab
// remembers most/least-used across navigations and restarts.
export const $skillsSortDesc = persistentAtom('hermes.desktop.capabilities.skillsSortDesc', true, Codecs.bool)
export const $toolsetsSortDesc = persistentAtom('hermes.desktop.capabilities.toolsetsSortDesc', true, Codecs.bool)
