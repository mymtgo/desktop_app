import { describe, expect, it } from 'vitest'
import type { StatusView } from '@/bindings'
import { automaticLogStatus, watcherState } from './storage'

function status(overrides: Partial<StatusView> = {}): StatusView {
  return {
    started: true,
    searched: ['C:/Users/Q/AppData/Local/Apps/2.0'],
    files: [{ path: 'C:/x/mtgo.log', size: 10, cursor: 10, lastLiveGrowthMs: null, badTimezone: false }],
    arenaSearched: ['C:/a/MTGA/Player.log'],
    arenaFiles: [],
    twoClients: false,
    awaitingIdentity: 0,
    quarantined: 0,
    failedMatches: 0,
    restarting: null,
    timezoneWarning: null,
    offlineMode: false,
    paused: false,
    ...overrides,
  }
}

describe('the watcher state (0.x WatcherCard)', () => {
  it('is running when on and a log is found, and offers Stop', () => {
    expect(watcherState(true, status())).toEqual({ running: true, canStart: true, badge: 'Running', button: 'Stop' })
  })

  it('is stopped when off, and offers Start while a log is found', () => {
    expect(watcherState(false, status())).toEqual({ running: false, canStart: true, badge: 'Stopped', button: 'Start' })
  })

  it('is stopped with Start disabled while no log is found, on or off', () => {
    const none = status({ files: [] })
    expect(watcherState(true, none)).toEqual({ running: false, canStart: false, badge: 'Stopped', button: 'Start' })
    expect(watcherState(false, none)).toEqual({ running: false, canStart: false, badge: 'Stopped', button: 'Start' })
    expect(watcherState(true, undefined).canStart).toBe(false)
  })
})

describe('the automatic log folder status (v1)', () => {
  it('shows where the log was found', () => {
    expect(automaticLogStatus(status(), 'mtgo')).toEqual({
      state: 'found',
      message: 'Found mtgo.log',
      paths: ['C:/x/mtgo.log'],
      searched: ['C:/Users/Q/AppData/Local/Apps/2.0'],
    })
    const two = status().files.concat({ ...status().files[0]!, path: 'C:/y/mtgo.log' })
    const found = automaticLogStatus(status({ files: two }), 'mtgo')
    expect(found.message).toBe('Found 2 logs')
    expect(found.paths).toEqual(['C:/x/mtgo.log', 'C:/y/mtgo.log'])
  })

  it('counts the locations checked when nothing is found', () => {
    const searched = ['C:/a/Apps/2.0', 'C:/b/Apps/2.0', 'C:/c/Apps/2.0']
    expect(automaticLogStatus(status({ files: [], searched }), 'mtgo')).toEqual({
      state: 'missing',
      message: "Can't find mtgo.log. Checked 3 possible locations.",
      paths: [],
      searched,
    })
    expect(automaticLogStatus(status({ files: [], searched: ['C:/a'] }), 'mtgo').message).toBe(
      "Can't find mtgo.log. Checked 1 possible location.",
    )
  })

  it('is searching until the first status arrives', () => {
    expect(automaticLogStatus(undefined, 'mtgo')).toEqual({
      state: 'searching',
      message: 'Searching for mtgo.log...',
      paths: [],
      searched: [],
    })
  })
})

describe('the automatic Arena log status', () => {
  it('names where Player.log is looked for and whether it was found', () => {
    expect(automaticLogStatus(status(), 'arena')).toEqual({
      state: 'missing',
      message: "Can't find Player.log. Checked 1 possible location.",
      paths: [],
      searched: ['C:/a/MTGA/Player.log'],
    })
    const player = { ...status().files[0]!, path: 'C:/a/MTGA/Player.log' }
    const found = automaticLogStatus(status({ files: [], arenaFiles: [player] }), 'arena')
    expect(found.message).toBe('Found Player.log')
    expect(found.paths).toEqual(['C:/a/MTGA/Player.log'])
  })

  it('lets the watcher start with only an Arena log', () => {
    const player = { ...status().files[0], path: 'C:/a/MTGA/Player.log' }
    expect(watcherState(true, status({ files: [], arenaFiles: [player] })).canStart).toBe(true)
  })
})
