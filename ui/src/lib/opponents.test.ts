import { describe, expect, it } from 'vitest'
import type { RecordView } from '@/bindings'
import { keys } from './keys'
import {
  DEFAULT_OPPONENTS,
  clampedPage,
  OPPONENT_SORTS,
  opponentTag,
  opponentsFromUrl,
  opponentsUrlQuery,
  relativeTime,
  tagTooltip,
} from './opponents'

function record(total: number, winRate: number): Pick<RecordView, 'total' | 'winRate'> {
  return { total, winRate }
}

describe('opponentTag', () => {
  it('needs two or more matches', () => {
    expect(opponentTag(record(1, 0))).toBeNull()
    expect(opponentTag(record(0, 0))).toBeNull()
  })

  it('is a nemesis at 39% or less', () => {
    expect(opponentTag(record(2, 0))).toBe('nemesis')
    expect(opponentTag(record(10, 39))).toBe('nemesis')
  })

  it('is a rival from 40% to 60%', () => {
    expect(opponentTag(record(5, 40))).toBe('rival')
    expect(opponentTag(record(2, 50))).toBe('rival')
    expect(opponentTag(record(5, 60))).toBe('rival')
  })

  it('has no tag above 60%', () => {
    expect(opponentTag(record(5, 61))).toBeNull()
    expect(opponentTag(record(3, 100))).toBeNull()
  })

  it('explains each tag as 0.x does', () => {
    expect(tagTooltip('nemesis')).toBe('Your win rate is under 40% against this opponent')
    expect(tagTooltip('rival')).toBe('Your win rate is between 40-60% against this opponent')
  })
})

describe('relativeTime', () => {
  const now = Date.UTC(2026, 9, 8, 12, 0, 0)
  const ago = (ms: number) => relativeTime(now - ms, now)
  const MIN = 60_000
  const HOUR = 60 * MIN
  const DAY = 24 * HOUR

  it('reads like diffForHumans', () => {
    expect(ago(10_000)).toBe('just now')
    expect(ago(MIN)).toBe('1 minute ago')
    expect(ago(5 * MIN)).toBe('5 minutes ago')
    expect(ago(HOUR)).toBe('1 hour ago')
    expect(ago(3 * HOUR)).toBe('3 hours ago')
    expect(ago(DAY)).toBe('1 day ago')
    expect(ago(6 * DAY)).toBe('6 days ago')
    expect(ago(7 * DAY)).toBe('1 week ago')
    expect(ago(15 * DAY)).toBe('2 weeks ago')
    expect(ago(45 * DAY)).toBe('1 month ago')
    expect(ago(200 * DAY)).toBe('6 months ago')
    expect(ago(400 * DAY)).toBe('1 year ago')
    expect(ago(800 * DAY)).toBe('2 years ago')
  })

  it('treats a time in the future as just now', () => {
    expect(relativeTime(now + HOUR, now)).toBe('just now')
  })
})

describe('the page query', () => {
  it('offers the four sorts in 0.x order', () => {
    expect(OPPONENT_SORTS.map((s) => s.value)).toEqual(['most_played', 'winrate_asc', 'winrate_desc', 'most_recent'])
    expect(OPPONENT_SORTS.map((s) => s.label)).toEqual([
      'Most played',
      'Win rate ↑ (worst first)',
      'Win rate ↓ (best first)',
      'Most recent',
    ])
  })

  it('reads the URL with defaults for anything missing or unknown', () => {
    expect(opponentsFromUrl({})).toEqual(DEFAULT_OPPONENTS)
    expect(opponentsFromUrl({ sort: 'nope', page: '-3', search: '  ', format: '' })).toEqual(DEFAULT_OPPONENTS)
    expect(opponentsFromUrl({ search: 'Qan', sort: 'winrate_desc', format: 'CMODERN', page: '3' })).toEqual({
      search: 'Qan',
      sort: 'winrate_desc',
      format: 'CMODERN',
      page: 3,
    })
  })

  it('writes only what differs from the defaults', () => {
    expect(opponentsUrlQuery(DEFAULT_OPPONENTS)).toEqual({})
    expect(opponentsUrlQuery({ search: 'Qan', sort: 'most_recent', format: 'CMODERN', page: 2 })).toEqual({
      search: 'Qan',
      sort: 'most_recent',
      format: 'CMODERN',
      page: '2',
    })
  })
})

describe('the page key', () => {
  it('sits under the dashboard key so match, deck and league events refetch it', () => {
    expect(keys.opponentList(DEFAULT_OPPONENTS)[0]).toBe(keys.allDashboard[0])
  })
})

describe('clampedPage', () => {
  it('moves a page past the end back to the last page', () => {
    expect(clampedPage(3, 30, 25)).toBe(2)
    expect(clampedPage(9, 10, 25)).toBe(1)
  })

  it('leaves a page that exists, or an empty list, alone', () => {
    expect(clampedPage(2, 30, 25)).toBeNull()
    expect(clampedPage(1, 0, 25)).toBeNull()
    expect(clampedPage(4, 0, 25)).toBe(1)
  })
})
