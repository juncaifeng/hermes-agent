/**
 * Pure helpers for managing `skills.external_dirs` (additional skill scan
 * roots) and for grouping skills by the directory they were discovered in.
 *
 * Shared by Settings → Skill directories (add/remove/validate) and the
 * Capabilities → Skills directory filter. Identity comparisons are lexical:
 * separators unified, trailing slashes dropped, Windows-style roots
 * case-folded — the backend resolves these same entries to absolute paths
 * before scanning (agent/skill_utils.get_external_skills_dirs).
 */

import { normalizeDisplayPath } from '@/lib/display-path'
import type { HermesConfigRecord, SkillInfo } from '@/types/hermes'

/** Best-effort home directory derived from a hermes_home path. */
export function homeFromHermesHome(hermesHome: string): string {
  const home = normalizeDisplayPath(hermesHome).replace(/\/+$/, '')
  const match = home.match(/^(.*)\/\.hermes(?:\/profiles\/[^/]+)?$/)

  return match?.[1] ?? ''
}

/** Expand a leading `~` when the home directory is known. */
function expandTilde(raw: string, home: string): string {
  if (!home) {
    return raw
  }

  if (raw === '~') {
    return home
  }

  return raw.startsWith('~/') ? `${home}/${raw.slice(2)}` : raw
}

/**
 * Comparison key for directory identity. Two paths that resolve to the same
 * directory on disk — `D:\team\skills\` vs `d:/team/skills` — share a key.
 * POSIX paths keep their case (case-sensitive filesystems); drive-letter and
 * UNC roots fold (Windows / SMB shares are case-insensitive).
 */
export function dirIdentityKey(raw: string, home = ''): string {
  const normalized = normalizeDisplayPath(expandTilde((raw || '').trim(), home))

  if (/^[A-Za-z]:\//.test(normalized) || normalized.startsWith('//')) {
    return normalized.toLowerCase()
  }

  return normalized
}

/** Built-in skills root for a hermes_home: `<hermes_home>/skills`. */
export function builtinSkillsDir(hermesHome: string): string {
  const home = normalizeDisplayPath(hermesHome).replace(/\/+$/, '')

  return home ? `${home}/skills` : ''
}

/**
 * Read `skills.external_dirs` out of a config record, tolerating missing or
 * hand-edited-malformed values. Blank/non-string entries are dropped.
 */
export function readExternalDirs(config: HermesConfigRecord | null | undefined): string[] {
  const skills = config?.skills

  if (!skills || typeof skills !== 'object') {
    return []
  }

  const raw = (skills as Record<string, unknown>).external_dirs

  if (!Array.isArray(raw)) {
    return []
  }

  return raw.filter((entry): entry is string => typeof entry === 'string' && entry.trim() !== '')
}

export type AddDirFailure = 'builtin' | 'duplicate'

export type AddDirResult = { dirs: string[]; ok: true } | { ok: false; reason: AddDirFailure }

/**
 * Validate a picked directory and append it. Rejects the built-in root (the
 * backend would silently skip it anyway) and duplicates of existing entries.
 * The stored value is the normalized absolute path so later dedupe compares
 * like with like.
 */
export function addExternalDir(
  dirs: string[],
  candidate: string,
  options: { builtinDir?: string; home?: string } = {}
): AddDirResult {
  const home = options.home ?? ''
  const key = dirIdentityKey(candidate, home)

  if (!key) {
    return { ok: false, reason: 'duplicate' }
  }

  if (options.builtinDir && key === dirIdentityKey(options.builtinDir, home)) {
    return { ok: false, reason: 'builtin' }
  }

  if (dirs.some(dir => dirIdentityKey(dir, home) === key)) {
    return { ok: false, reason: 'duplicate' }
  }

  return { dirs: [...dirs, normalizeDisplayPath(expandTilde(candidate.trim(), home))], ok: true }
}

/** Drop every entry that identifies the same directory as `target`. */
export function removeExternalDir(dirs: string[], target: string, home = ''): string[] {
  const key = dirIdentityKey(target, home)

  return dirs.filter(dir => dirIdentityKey(dir, home) !== key)
}

/**
 * Deep-merge a new external_dirs list into the config record. Sibling keys
 * under `skills` (template_vars, disabled, platform_disabled, …) survive.
 */
export function withExternalDirs(config: HermesConfigRecord, dirs: string[]): HermesConfigRecord {
  const skills = config.skills && typeof config.skills === 'object' ? (config.skills as Record<string, unknown>) : {}

  return { ...config, skills: { ...skills, external_dirs: dirs } }
}

/** Directory a skill was discovered in, normalized for display/identity. */
export function skillSourceDir(skill: Pick<SkillInfo, 'source_dir'>): string {
  return skill.source_dir ? normalizeDisplayPath(skill.source_dir) : ''
}

/**
 * Distinct external source directories actually present in a skills list
 * (i.e. every source_dir that is not the built-in root), sorted for stable
 * display. Data-derived on purpose: configured-but-empty directories have
 * nothing to filter to, and config strings may be unresolvable (`~`,
 * `${VAR}`) in the renderer — the backend's resolved source_dir is the one
 * honest value to group by.
 */
export function externalSourceDirs(skills: Pick<SkillInfo, 'source_dir'>[], builtinDir: string): string[] {
  const builtinKey = builtinDir ? dirIdentityKey(builtinDir) : ''
  const byKey = new Map<string, string>()

  for (const skill of skills) {
    const dir = skillSourceDir(skill)

    if (!dir) {
      continue
    }

    const key = dirIdentityKey(dir)

    if (key === builtinKey || byKey.has(key)) {
      continue
    }

    byKey.set(key, dir)
  }

  return [...byKey.values()].sort((a, b) => a.localeCompare(b))
}

/** True when the skill was discovered under the directory `dir` names. */
export function matchesSourceDir(skill: Pick<SkillInfo, 'source_dir'>, dir: string): boolean {
  return Boolean(skill.source_dir) && dirIdentityKey(skill.source_dir!) === dirIdentityKey(dir)
}
