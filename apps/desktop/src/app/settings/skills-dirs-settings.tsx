import { useQuery } from '@tanstack/react-query'
import { useState } from 'react'

import { Button } from '@/components/ui/button'
import { Tip } from '@/components/ui/tooltip'
import { getHermesConfigRecord, getStatus, saveHermesConfig } from '@/hermes'
import { useI18n } from '@/i18n'
import { isDesktopFsRemoteMode, readDesktopDir, selectDesktopPaths } from '@/lib/desktop-fs'
import { displayPath } from '@/lib/display-path'
import { triggerHaptic } from '@/lib/haptics'
import { AlertTriangle, FolderOpen, Plus, Trash2 } from '@/lib/icons'
import { queryClient } from '@/lib/query-client'
import {
  addExternalDir,
  builtinSkillsDir,
  dirIdentityKey,
  homeFromHermesHome,
  readExternalDirs,
  removeExternalDir,
  withExternalDirs
} from '@/lib/skills-dirs'
import { invalidateSlashCompletions } from '@/lib/slash-completion-cache'
import { notify, notifyError } from '@/store/notifications'

import { setHermesConfigCache, useHermesConfigRecord } from '../hooks/use-config-record'
import { SKILLS_QUERY_KEY } from '../skills/store'

import { EmptyState, ListRow, Pill, SectionHeading, SettingsContent, SettingsSkeleton } from './primitives'

const CAPTION = 'text-[length:var(--conversation-caption-font-size)] text-(--ui-text-tertiary)'

// Probe each external dir once per dirs-list content; the key changes with the
// list, so adds/removes re-probe automatically. Failed reads (missing dir,
// unreachable share, no bridge) land as `false` and paint the warning state.
function useDirHealth(dirs: string[], home: string) {
  return useQuery({
    enabled: dirs.length > 0,
    queryFn: async () => {
      const entries = await Promise.all(
        dirs.map(async dir => {
          const result = await readDesktopDir(dir).catch(() => ({ entries: [], error: 'unreachable' }) as const)

          return [dirIdentityKey(dir, home), !result.error] as const
        })
      )

      return Object.fromEntries(entries) as Record<string, boolean>
    },
    queryKey: ['skills-dir-health', home, ...dirs]
  })
}

export function SkillsDirsSettings({ onConfigSaved }: { onConfigSaved?: () => void }) {
  const { t } = useI18n()
  const s = t.settings.skillsDirs
  const { data: config, isPending } = useHermesConfigRecord()
  const { data: status } = useQuery({ queryFn: getStatus, queryKey: ['hermes-status'] })
  const [busy, setBusy] = useState(false)

  const builtinDir = status?.hermes_home ? builtinSkillsDir(status.hermes_home) : ''
  const home = status?.hermes_home ? homeFromHermesHome(status.hermes_home) : ''
  const dirs = config ? readExternalDirs(config) : []

  const { data: health } = useDirHealth(dirs, home)
  const isMissing = (dir: string) => health?.[dirIdentityKey(dir, home)] === false
  const missingDirs = dirs.filter(isMissing)

  // openDir is a local-OS bridge call; against a remote backend the paths are
  // remote and there is nothing sensible to open in the local file manager.
  const canOpen = !isDesktopFsRemoteMode()

  const persist = async (next: string[], toast?: string) => {
    if (busy) {
      return
    }

    setBusy(true)

    try {
      // Merge onto a FRESH record: another surface (or the CLI) may have
      // moved config since our cache painted, and saving the stale record
      // would clobber that.
      const fresh = await getHermesConfigRecord()
      const merged = withExternalDirs(fresh, next)

      await saveHermesConfig(merged)
      setHermesConfigCache(merged)
      // The skills list gains/loses the directory's skills; the composer's
      // cached slash commands change with it.
      void queryClient.invalidateQueries({ queryKey: SKILLS_QUERY_KEY })
      invalidateSlashCompletions()
      onConfigSaved?.()

      if (toast) {
        notify({ kind: 'success', message: toast })
      }
    } catch (err) {
      notifyError(err, s.saveFailed)
    } finally {
      setBusy(false)
    }
  }

  const addDir = async () => {
    triggerHaptic('open')

    let candidate: string | undefined

    try {
      candidate = (await selectDesktopPaths({ directories: true, multiple: false, title: s.addPickerTitle }))[0]
    } catch (err) {
      notifyError(err, s.add)

      return
    }

    if (!candidate) {
      return
    }

    const probe = await readDesktopDir(candidate).catch(() => ({ entries: [], error: 'unreachable' }) as const)

    if (probe.error) {
      notify({ kind: 'warning', message: s.addNotFound })

      return
    }

    const result = addExternalDir(dirs, candidate, { builtinDir, home })

    if (!result.ok) {
      notify({ kind: 'warning', message: result.reason === 'builtin' ? s.addBuiltinConflict : s.addDuplicate })

      return
    }

    await persist(result.dirs, s.added(displayPath(candidate, { home })))
  }

  const openDir = (dir: string) => {
    void window.hermesDesktop
      ?.openDir?.(dir)
      .then(result => {
        if (result && !result.ok) {
          notifyError(result.error ?? 'unknown error', s.open)
        }
      })
      .catch((err: unknown) => notifyError(err, s.open))
  }

  if (isPending) {
    return <SettingsSkeleton sections={[{ heading: true, rows: 3 }]} />
  }

  if (!config) {
    return (
      <SettingsContent>
        <EmptyState title={s.saveFailed} />
      </SettingsContent>
    )
  }

  const openButton = (dir: string) =>
    canOpen && (
      <Tip label={s.open}>
        <Button aria-label={`${s.open} ${dir}`} onClick={() => openDir(dir)} size="icon" variant="ghost">
          <FolderOpen />
        </Button>
      </Tip>
    )

  return (
    <SettingsContent>
      <SectionHeading
        aside={
          missingDirs.length > 0 && (
            <Button
              disabled={busy}
              onClick={() =>
                void persist(
                  dirs.filter(dir => !isMissing(dir)),
                  s.cleanedUp(missingDirs.length)
                )
              }
              size="sm"
              variant="outline"
            >
              <AlertTriangle />
              {s.cleanupMissing}
            </Button>
          )
        }
        icon={FolderOpen}
        meta={dirs.length ? s.count(dirs.length) : undefined}
        title={s.title}
      />
      <p className={`mb-4 ${CAPTION}`}>{s.blurb}</p>

      <div className="divide-y divide-(--ui-stroke-tertiary)">
        {builtinDir && (
          <ListRow
            action={openButton(builtinDir)}
            title={
              <span className="flex min-w-0 items-center gap-2">
                <span className="truncate">{displayPath(builtinDir, { home })}</span>
                <Pill>{s.builtinBadge}</Pill>
              </span>
            }
          />
        )}

        {dirs.map(dir => {
          const missing = isMissing(dir)

          return (
            <ListRow
              action={
                <div className="flex items-center justify-end gap-1">
                  {!missing && openButton(dir)}
                  <Tip label={s.remove}>
                    <Button
                      aria-label={`${s.remove} ${dir}`}
                      className="text-muted-foreground hover:text-destructive"
                      disabled={busy}
                      onClick={() =>
                        void persist(removeExternalDir(dirs, dir, home), s.removed(displayPath(dir, { home })))
                      }
                      size="icon"
                      variant="ghost"
                    >
                      <Trash2 />
                    </Button>
                  </Tip>
                </div>
              }
              description={
                missing ? <span className="text-(--ui-danger,#f87171)">{displayPath(dir, { home })}</span> : undefined
              }
              key={dirIdentityKey(dir, home)}
              title={
                missing ? (
                  <span className="flex min-w-0 items-center gap-2">
                    <span className="truncate">{displayPath(dir, { home })}</span>
                    <Pill tone="warn">{s.missingBadge}</Pill>
                  </span>
                ) : (
                  displayPath(dir, { home })
                )
              }
            />
          )
        })}
      </div>

      {dirs.length === 0 && <EmptyState title={s.empty} />}

      <div className="mt-4 flex flex-col gap-2">
        <Button className="self-start" disabled={busy} onClick={() => void addDir()} size="sm" variant="outline">
          <Plus />
          {s.add}
        </Button>
        <p className={CAPTION}>{s.noteReadOnly}</p>
        <p className={CAPTION}>{s.noteNewSessions}</p>
        <p className={CAPTION}>{s.noteTrust}</p>
      </div>
    </SettingsContent>
  )
}
