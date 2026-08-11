import { Codecs, persistentAtom } from '@/lib/persisted'

// RQ cache key for the /api/skills list. Exported so other surfaces (Settings
// → Skill Directories) can invalidate it when the scanned directories change.
export const SKILLS_QUERY_KEY = ['skills-list'] as const

// Per-view sort direction for the Capabilities lists — persisted so each tab
// remembers most/least-used across navigations and restarts.
export const $skillsSortDesc = persistentAtom('hermes.desktop.capabilities.skillsSortDesc', true, Codecs.bool)
export const $toolsetsSortDesc = persistentAtom('hermes.desktop.capabilities.toolsetsSortDesc', true, Codecs.bool)
