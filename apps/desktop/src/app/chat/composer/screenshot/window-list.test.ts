import { describe, expect, it } from 'vitest'

import { groupWindowsByProcess } from './window-list'

function win(id: string, process: string, title: string) {
  return { id, pid: Number(id), process, title }
}

describe('groupWindowsByProcess', () => {
  it('groups by process, sorts processes A–Z and titles within a group', () => {
    const groups = groupWindowsByProcess([
      win('1', 'code.exe', 'Zeta file'),
      win('2', 'notepad.exe', 'b.txt'),
      win('3', 'code.exe', 'Alpha file'),
      win('4', 'notepad.exe', 'a.txt')
    ])

    expect(groups.map(g => g.process)).toEqual(['code.exe', 'notepad.exe'])
    expect(groups[0].windows.map(w => w.title)).toEqual(['Alpha file', 'Zeta file'])
    expect(groups[1].windows.map(w => w.title)).toEqual(['a.txt', 'b.txt'])
  })

  it('sinks the unknown-process bucket to the bottom', () => {
    const groups = groupWindowsByProcess([win('1', '', 'mystery'), win('2', 'app.exe', 'App')])

    expect(groups.map(g => g.process)).toEqual(['app.exe', ''])
  })

  it('returns no groups for an empty list', () => {
    expect(groupWindowsByProcess([])).toEqual([])
  })
})
