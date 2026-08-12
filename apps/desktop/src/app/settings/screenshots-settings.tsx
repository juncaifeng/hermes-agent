import { useStore } from '@nanostores/react'
import { useQuery } from '@tanstack/react-query'
import { useState } from 'react'

import { Button } from '@/components/ui/button'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { useI18n } from '@/i18n'
import { displayPath } from '@/lib/display-path'
import { triggerHaptic } from '@/lib/haptics'
import { Activity, ImageIcon, Trash2 } from '@/lib/icons'
import { queryClient } from '@/lib/query-client'
import { notify, notifyError } from '@/store/notifications'
import { $screenshotDirMode, $screenshotGitExclude, $screenshotOverlayEnabled } from '@/store/screenshots'
import { $currentCwd } from '@/store/session'

import { resolveScreenshotDir } from '../chat/composer/screenshot/save-dir'

import { ListRow, Pill, SectionHeading, SettingsContent, ToggleRow } from './primitives'

// Both managed locations are always listed and always cleanable — a user who
// switched modes still has files in the old spot (proposal §3).
const STATS_QUERY_KEY = ['screenshot-dir-stats'] as const

function formatBytes(bytes: number): string {
  if (bytes <= 0) {
    return '0 KB'
  }

  if (bytes >= 1024 * 1024) {
    return `${(bytes / (1024 * 1024)).toFixed(1)} MB`
  }

  return `${Math.max(1, Math.round(bytes / 1024))} KB`
}

export function ScreenshotsSettings() {
  const { t } = useI18n()
  const s = t.settings.screenshots
  const dirMode = useStore($screenshotDirMode)
  const gitExclude = useStore($screenshotGitExclude)
  const overlayEnabled = useStore($screenshotOverlayEnabled)
  const currentCwd = useStore($currentCwd)
  const [cleaning, setCleaning] = useState(false)

  const projectDir = currentCwd?.trim() ? currentCwd : null
  const { data: stats, isError: statsFailed } = useQuery({
    queryFn: () => window.hermesDesktop!.screenshotDirStats!(projectDir),
    queryKey: [...STATS_QUERY_KEY, projectDir]
  })

  const effectiveProjectDir = resolveScreenshotDir('project', projectDir)

  const applyGitExclude = async (on: boolean) => {
    $screenshotGitExclude.set(on)

    if (!on) {
      return
    }

    if (!projectDir) {
      notify({ kind: 'warning', message: s.projectNoWorkspace })

      return
    }

    try {
      const result = await window.hermesDesktop!.excludePathFromGit!(projectDir, '.hermes/screenshots/')
      notify({ kind: 'success', message: result === 'added' ? s.gitExcludeApplied : s.gitExcludeAlready })
    } catch (err) {
      notifyError(err, s.gitExcludeFailed)
    }
  }

  const cleanAll = async () => {
    if (cleaning || !stats || !window.confirm(s.cleanConfirm)) {
      return
    }

    setCleaning(true)

    try {
      let files = 0
      let bytes = 0

      for (const dir of stats) {
        if (dir.files === 0) {
          continue
        }

        const result = await window.hermesDesktop!.clearScreenshotDir!(dir.path)
        files += result.removed_files
        bytes += result.freed_bytes
      }

      triggerHaptic('success')
      notify({ kind: 'success', message: s.cleaned(files, formatBytes(bytes)) })
      void queryClient.invalidateQueries({ queryKey: STATS_QUERY_KEY })
    } catch (err) {
      notifyError(err, s.cleanFailed)
    } finally {
      setCleaning(false)
    }
  }

  return (
    <SettingsContent>
      <SectionHeading icon={ImageIcon} title={s.title} />
      <p className="mb-4 text-[length:var(--conversation-caption-font-size)] text-(--ui-text-tertiary)">{s.blurb}</p>

      <ListRow
        action={
          <Select
            onValueChange={value => {
              triggerHaptic('selection')
              $screenshotDirMode.set(value === 'project' ? 'project' : 'app_data')
            }}
            value={dirMode}
          >
            <SelectTrigger className="min-w-56" size="sm">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="app_data">{s.dirModeAppData}</SelectItem>
              <SelectItem value="project">{s.dirModeProject}</SelectItem>
            </SelectContent>
          </Select>
        }
        description={s.dirModeDesc}
        title={s.dirModeTitle}
      />

      {dirMode === 'project' && !projectDir && (
        <p className="mb-3 text-[length:var(--conversation-caption-font-size)] text-amber-600 dark:text-amber-300">
          {s.projectNoWorkspace}
        </p>
      )}

      {dirMode === 'project' && (
        <ToggleRow
          checked={gitExclude}
          description={s.gitExcludeDesc}
          label={s.gitExcludeTitle}
          onChange={on => void applyGitExclude(on)}
        />
      )}

      <div className="mt-6">
        <SectionHeading
          aside={
            <Button
              disabled={cleaning || !stats?.some(dir => dir.files > 0)}
              onClick={() => void cleanAll()}
              size="sm"
              variant="outline"
            >
              <Trash2 />
              {cleaning ? s.cleaning : s.clean}
            </Button>
          }
          icon={Activity}
          title={s.storageTitle}
        />

        {statsFailed ? (
          <p className="text-[length:var(--conversation-caption-font-size)] text-(--ui-danger,#f87171)">
            {s.storageLoadFailed}
          </p>
        ) : (
          <div className="divide-y divide-(--ui-stroke-tertiary)">
            {(stats ?? []).map(dir => (
              <ListRow
                description={displayPath(dir.path)}
                key={dir.kind}
                title={
                  <span className="flex min-w-0 items-center gap-2">
                    <span className="truncate">{dir.kind === 'project' ? s.dirModeProject : s.dirModeAppData}</span>
                    <Pill>{dir.exists ? s.storageFiles(dir.files, formatBytes(dir.bytes)) : s.storageEmpty}</Pill>
                  </span>
                }
              />
            ))}
          </div>
        )}
      </div>

      <div className="mt-6">
        <ToggleRow
          checked={overlayEnabled}
          description={s.overlayDesc}
          label={s.overlayTitle}
          onChange={on => $screenshotOverlayEnabled.set(on)}
        />
      </div>
    </SettingsContent>
  )
}
