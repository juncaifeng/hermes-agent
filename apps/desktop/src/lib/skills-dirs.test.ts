import { describe, expect, it } from 'vitest'

import {
  addExternalDir,
  builtinSkillsDir,
  dirIdentityKey,
  externalSourceDirs,
  homeFromHermesHome,
  matchesSourceDir,
  readExternalDirs,
  removeExternalDir,
  withExternalDirs
} from './skills-dirs'

describe('dirIdentityKey', () => {
  it('unifies separators and drops trailing slashes', () => {
    expect(dirIdentityKey('D:\\team\\skills\\')).toBe(dirIdentityKey('d:/team/skills'))
  })

  it('case-folds drive-letter and UNC roots, keeps POSIX case', () => {
    expect(dirIdentityKey('C:/Users/A/.agents/skills')).toBe(dirIdentityKey('c:/users/a/.agents/skills'))
    expect(dirIdentityKey('//NAS/Share/Skills')).toBe(dirIdentityKey('//nas/share/skills'))
    expect(dirIdentityKey('/opt/TeamSkills')).not.toBe(dirIdentityKey('/opt/teamskills'))
  })

  it('expands a leading tilde when home is known', () => {
    expect(dirIdentityKey('~/.agents/skills', '/home/u')).toBe(dirIdentityKey('/home/u/.agents/skills'))
    // Unknown home: no expansion, no crash.
    expect(dirIdentityKey('~/.agents/skills')).toBe('~/.agents/skills')
  })
})

describe('builtinSkillsDir / homeFromHermesHome', () => {
  it('joins the skills root off hermes_home', () => {
    expect(builtinSkillsDir('/home/u/.hermes')).toBe('/home/u/.hermes/skills')
    expect(builtinSkillsDir('C:\\Users\\u\\.hermes\\')).toBe('C:/Users/u/.hermes/skills')
  })

  it('derives home from default and profile hermes_home layouts', () => {
    expect(homeFromHermesHome('/home/u/.hermes')).toBe('/home/u')
    expect(homeFromHermesHome('/home/u/.hermes/profiles/coder')).toBe('/home/u')
    expect(homeFromHermesHome('/opt/custom-home')).toBe('')
  })
})

describe('readExternalDirs', () => {
  it('tolerates missing or malformed config', () => {
    expect(readExternalDirs(null)).toEqual([])
    expect(readExternalDirs({})).toEqual([])
    expect(readExternalDirs({ skills: 'nope' })).toEqual([])
    expect(readExternalDirs({ skills: { external_dirs: 'nope' } })).toEqual([])
  })

  it('returns only non-blank string entries', () => {
    expect(readExternalDirs({ skills: { external_dirs: ['/a', '', '  ', 42, '/b'] } })).toEqual(['/a', '/b'])
  })
})

describe('addExternalDir', () => {
  const opts = { builtinDir: '/home/u/.hermes/skills', home: '/home/u' }

  it('appends a new directory in normalized form', () => {
    const result = addExternalDir(['/ext/a'], '/ext/b/', opts)

    expect(result).toEqual({ dirs: ['/ext/a', '/ext/b'], ok: true })
  })

  it('rejects duplicates, including separator/case variants and tilde aliases', () => {
    expect(addExternalDir(['/ext/a'], '/ext/a/', opts)).toEqual({ ok: false, reason: 'duplicate' })
    expect(addExternalDir(['D:/Team/Skills'], 'd:\\team\\skills', opts)).toEqual({ ok: false, reason: 'duplicate' })
    expect(addExternalDir(['~/.agents/skills'], '/home/u/.agents/skills', opts)).toEqual({
      ok: false,
      reason: 'duplicate'
    })
  })

  it('rejects the built-in directory however it is spelled', () => {
    expect(addExternalDir([], '/home/u/.hermes/skills/', opts)).toEqual({ ok: false, reason: 'builtin' })
  })
})

describe('removeExternalDir', () => {
  it('drops every entry identifying the same directory', () => {
    expect(removeExternalDir(['/ext/a', '/ext/b'], '/ext/a/')).toEqual(['/ext/b'])
  })
})

describe('withExternalDirs', () => {
  it('merges without dropping sibling skills keys or top-level sections', () => {
    const config = {
      model: { provider: 'nous' },
      skills: { disabled: ['x'], external_dirs: ['/old'], template_vars: { k: 'v' } }
    }
    const merged = withExternalDirs(config, ['/new'])

    expect(merged.skills).toEqual({ disabled: ['x'], external_dirs: ['/new'], template_vars: { k: 'v' } })
    expect(merged.model).toEqual({ provider: 'nous' })
  })

  it('creates the skills section when absent', () => {
    expect(withExternalDirs({}, ['/new']).skills).toEqual({ external_dirs: ['/new'] })
  })
})

describe('externalSourceDirs / matchesSourceDir', () => {
  const skills = [
    { source_dir: '/home/u/.hermes/skills' },
    { source_dir: '/ext/b' },
    { source_dir: '/ext/a' },
    { source_dir: '/ext/B/' }, // same dir as /ext/b on case-folded roots? no — POSIX keeps case
    { source_dir: undefined },
    {}
  ]

  it('collects distinct non-builtin source dirs, sorted', () => {
    expect(externalSourceDirs(skills, '/home/u/.hermes/skills')).toEqual(['/ext/a', '/ext/b', '/ext/B'])
  })

  it('matches by directory identity', () => {
    expect(matchesSourceDir({ source_dir: '/ext/a' }, '/ext/a/')).toBe(true)
    expect(matchesSourceDir({ source_dir: '/ext/a' }, '/ext/b')).toBe(false)
    expect(matchesSourceDir({}, '/ext/a')).toBe(false)
  })
})
