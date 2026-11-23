import { describe, expect, it } from 'vitest'
import type { StatusView } from '@/bindings'
import { firstRunState, newestEventMs, platformFiles, sinceText, statusWarnings, trackingState } from './status'

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

describe('trackingState', () => {
  it('says the tracker is starting before its first report', () => {
    expect(trackingState(status({ started: false }), 'mtgo')).toEqual({ tone: 'idle', text: 'Starting' })
  })

  it('says MTGO was not found when no log is watched', () => {
    expect(trackingState(status({ files: [] }), 'mtgo')).toEqual({ tone: 'warn', text: 'MTGO not found' })
  })

  it('counts watched logs', () => {
    expect(trackingState(status(), 'mtgo')).toEqual({ tone: 'ok', text: 'Watching 1 log' })
    const two = status().files.concat(status().files)
    expect(trackingState(status({ files: two }), 'mtgo').text).toBe('Watching 2 logs')
  })

  it('says Stopped while the watcher is off, found logs or not', () => {
    expect(trackingState(status({ paused: true }), 'mtgo')).toEqual({ tone: 'error', text: 'Stopped' })
    expect(trackingState(status({ paused: true, files: [] }), 'mtgo')).toEqual({ tone: 'error', text: 'Stopped' })
  })

  it('shows a restart above everything else', () => {
    expect(trackingState(status({ restarting: 'disk full', files: [] }), 'mtgo')).toEqual({
      tone: 'error',
      text: 'Tracking paused, restarting',
    })
  })
})

describe('time since the last event', () => {
  it('takes the newest live growth of any file', () => {
    const files = [
      { ...status().files[0], lastLiveGrowthMs: 1_000 },
      { ...status().files[0], lastLiveGrowthMs: 5_000 },
    ]
    expect(newestEventMs(status({ files }), 'mtgo')).toBe(5_000)
    expect(newestEventMs(status(), 'mtgo')).toBeNull()
  })

  it('reads as a short relative time', () => {
    expect(sinceText(null, 10_000)).toBe('No live events yet')
    expect(sinceText(9_000, 10_000)).toBe('Last event just now')
    expect(sinceText(10_000 - 42_000, 10_000)).toBe('Last event 42 seconds ago')
    expect(sinceText(10_000 - 5 * 60_000, 10_000)).toBe('Last event 5 minutes ago')
    expect(sinceText(10_000 - 3 * 3_600_000, 10_000)).toBe('Last event 3 hours ago')
    expect(sinceText(10_000 - 2 * 86_400_000, 10_000)).toBe('Last event 2 days ago')
  })
})

describe('the Arena context', () => {
  const player = { path: 'C:/a/MTGA/Player.log', size: 10, cursor: 10, lastLiveGrowthMs: 7_000, badTimezone: false }

  it('watches Player.log, not mtgo.log', () => {
    expect(trackingState(status(), 'arena')).toEqual({ tone: 'warn', text: 'Arena not found' })
    expect(trackingState(status({ files: [], arenaFiles: [player] }), 'arena')).toEqual({
      tone: 'ok',
      text: 'Watching 1 log',
    })
    expect(trackingState(status({ files: [], arenaFiles: [player] }), 'mtgo').text).toBe('MTGO not found')
  })

  it('takes the last event from Player.log', () => {
    expect(newestEventMs(status({ arenaFiles: [player] }), 'arena')).toBe(7_000)
    expect(newestEventMs(status({ arenaFiles: [player] }), 'mtgo')).toBeNull()
  })

  it('says not connected while the extension was never seen or has gone quiet', () => {
    const expected = { tone: 'warn', text: 'Endstep not connected' }
    expect(trackingState(status(), 'endstep', { kind: 'neverSeen' })).toEqual(expected)
    expect(trackingState(status(), 'endstep', { kind: 'stale', browser: 'chrome', extension: '1', atMs: 0 })).toEqual(expected)
    const recent = { kind: 'recent', browser: 'chrome', extension: '1', atMs: 0 } as const
    expect(trackingState(status(), 'endstep', recent).text).toBe('Reading Endstep captures')
    expect(trackingState(status({ paused: true }), 'endstep', { kind: 'neverSeen' }).text).toBe('Stopped')
    expect(trackingState(status(), 'mtgo', { kind: 'neverSeen' }).text).toBe('Watching 1 log')
  })

  it('leaves out the MTGO-only warnings', () => {
    expect(statusWarnings(status({ twoClients: true, awaitingIdentity: 1, quarantined: 1 }), 'arena')).toEqual([
      '1 log event skipped after repeated errors',
    ])
  })
})

describe('the Endstep context', () => {
  it('has no log files to watch', () => {
    expect(platformFiles(status(), 'endstep')).toEqual([])
    expect(newestEventMs(status(), 'endstep')).toBeNull()
  })

  it('reads captures, never MTGO not found', () => {
    expect(trackingState(status(), 'endstep')).toEqual({ tone: 'ok', text: 'Reading Endstep captures' })
    expect(trackingState(status({ paused: true }), 'endstep').tone).toBe('error')
  })

  it('leaves out the MTGO-only warnings', () => {
    expect(statusWarnings(status({ twoClients: true, awaitingIdentity: 1 }), 'endstep')).toEqual([])
  })
})

describe('statusWarnings', () => {
  it('lists every problem the status bar must show', () => {
    const warnings = statusWarnings(
      status({
        twoClients: true,
        awaitingIdentity: 2,
        quarantined: 1,
        failedMatches: 1,
        timezoneWarning: 'Your system timezone could not be read (x), so match times are dated in UTC.',
        files: [{ ...status().files[0], badTimezone: true }],
      }),
      'mtgo',
    )
    expect(warnings).toEqual([
      'Two MTGO clients are running',
      'Waiting to identify your MTGO username',
      '1 log event skipped after repeated errors',
      '1 match could not be read from the log; a future update will retry it',
      'Your system timezone could not be read (x), so match times are dated in UTC.',
      'A log file has an unknown timezone; its times use yours',
    ])
    expect(statusWarnings(status(), 'mtgo')).toEqual([])
    expect(statusWarnings(status({ quarantined: 3 }), 'mtgo')).toEqual(['3 log events skipped after repeated errors'])
    expect(statusWarnings(status({ failedMatches: 2 }), 'mtgo')).toEqual([
      '2 matches could not be read from the log; a future update will retry them',
    ])
  })
})

describe('firstRunState', () => {
  it('waits for the first report and the first page', () => {
    expect(firstRunState(undefined, 0)).toBeNull()
    expect(firstRunState(status({ started: false, files: [] }), 0)).toBeNull()
    expect(firstRunState(status(), undefined)).toBeNull()
  })

  it('says MTGO was not found before anything else on a first run', () => {
    expect(firstRunState(status({ files: [], awaitingIdentity: 1 }), 0)).toBe('not_found')
  })

  it('keeps showing stored matches when MTGO is not found later (the status bar warns)', () => {
    expect(firstRunState(status({ files: [] }), 3)).toBeNull()
  })

  it('waits for the username, then for the first match', () => {
    expect(firstRunState(status({ awaitingIdentity: 1 }), 0)).toBe('awaiting_identity')
    expect(firstRunState(status(), 0)).toBe('waiting_for_match')
  })

  it('a live row on the first page is never hidden by a first-run state', () => {
    expect(firstRunState(status(), 0, 1)).toBeNull()
  })

  it('shows the list once there are matches', () => {
    expect(firstRunState(status({ awaitingIdentity: 1 }), 2)).toBeNull()
  })
})
