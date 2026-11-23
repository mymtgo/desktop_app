import { describe, expect, it } from 'vitest'
import {
  dateRange,
  arenaKindLabel,
  arenaRankCell,
  arenaTypeLabel,
  endstepRatingCell,
  noMatchesRecordedText,
  gameCells,
  isFiltered,
  bestOfOptions,
  kindOptions,
  matchDateText,
  matchDurationText,
  matchListFromUrl,
  matchListQuery,
  matchListUrlQuery,
  nextSort,
  rowMenuEnabled,
  withListQuery,
  withoutHiddenSort,
  type MatchListState,
} from './matchList'

/** The list with no filter, sort or page: what the URL gives without keys. */
const DEFAULT_LIST: MatchListState = {
  from: null,
  to: null,
  result: null,
  kind: null,
  bestOf: null,
  archetype: null,
  sortBy: null,
  sortDir: 'desc',
  page: 1,
}

function state(change: Partial<MatchListState> = {}): MatchListState {
  return { ...DEFAULT_LIST, ...change }
}

describe('nextSort', () => {
  it('starts a new column newest (or largest) first', () => {
    expect(nextSort({ sortBy: null, sortDir: 'desc' }, 'duration')).toEqual({ sortBy: 'duration', sortDir: 'desc' })
  })

  it('cycles the same column desc, then asc, then off', () => {
    const first = nextSort({ sortBy: null, sortDir: 'desc' }, 'outcome')
    const second = nextSort(first, 'outcome')
    const third = nextSort(second, 'outcome')
    expect(second).toEqual({ sortBy: 'outcome', sortDir: 'asc' })
    expect(third).toEqual({ sortBy: null, sortDir: 'desc' })
  })

  it('moving to another column starts it descending', () => {
    expect(nextSort({ sortBy: 'game_1', sortDir: 'asc' }, 'game_2')).toEqual({ sortBy: 'game_2', sortDir: 'desc' })
  })
})

describe('matchListQuery', () => {
  it('is newest first with no filters by default', () => {
    expect(matchListQuery(DEFAULT_LIST)).toEqual({
      from: null,
      to: null,
      result: null,
      kind: null,
      bestOf: null,
      archetype: null,
      sort: 'started_at',
      sortDir: 'desc',
      page: 1,
    })
  })

  it('carries the filters, the sort and the page', () => {
    const query = matchListQuery(
      state({ from: '2026-09-01', to: '2026-09-07', result: 'loss', kind: 'league', sortBy: 'game_3', sortDir: 'asc', page: 3 }),
    )
    expect(query).toEqual({
      from: '2026-09-01',
      to: '2026-09-07',
      result: 'loss',
      kind: 'league',
      bestOf: null,
      archetype: null,
      sort: 'game_3',
      sortDir: 'asc',
      page: 3,
    })
  })

  it('an unsorted list is newest first whatever direction was left behind', () => {
    expect(matchListQuery(state({ sortBy: null, sortDir: 'asc' }))).toMatchObject({ sort: 'started_at', sortDir: 'desc' })
  })
})

describe('the archetype filter and sort (0.x filter_archetype, sort archetype)', () => {
  it('reads none or an archetype id from the URL, sorts by archetype and round trips', () => {
    expect(matchListFromUrl({ archetype: 'none', sort: 'archetype' })).toMatchObject({
      archetype: 'none',
      sortBy: 'archetype',
    })
    const byId = state({ archetype: '12', sortBy: 'archetype', sortDir: 'asc' })
    const url = matchListUrlQuery(byId)
    expect(url).toEqual({ archetype: '12', sort: 'archetype', dir: 'asc' })
    expect(matchListFromUrl(url)).toEqual(byId)
    expect(matchListQuery(byId)).toMatchObject({ archetype: '12', sort: 'archetype', sortDir: 'asc' })
  })

  it('counts as a filter', () => {
    expect(isFiltered(state({ archetype: 'none' }))).toBe(true)
  })
})

describe('dateRange', () => {
  it('keeps blank days as no bound', () => {
    expect(dateRange('', null)).toEqual({ from: null, to: null })
    expect(dateRange('2026-09-07', '')).toEqual({ from: '2026-09-07', to: null })
    expect(dateRange(undefined, '2026-09-07')).toEqual({ from: null, to: '2026-09-07' })
  })

  it('keeps a single day as both bounds (both are inclusive)', () => {
    expect(dateRange('2026-09-07', '2026-09-07')).toEqual({ from: '2026-09-07', to: '2026-09-07' })
  })

  it('puts the earlier day first', () => {
    expect(dateRange('2026-09-30', '2026-09-01')).toEqual({ from: '2026-09-01', to: '2026-09-30' })
  })

  it('drops a day that is not a calendar day', () => {
    expect(dateRange('2026-02-30', '7/9/2026')).toEqual({ from: null, to: null })
    expect(dateRange(['2026-09-07'], 7)).toEqual({ from: null, to: null })
  })
})

describe('withoutHiddenSort', () => {
  it.each(['game_1', 'game_2', 'game_3'] as const)('treats a %s sort as unsorted in a single result list', (column) => {
    expect(withoutHiddenSort(state({ sortBy: column, sortDir: 'asc', page: 2 }), true)).toEqual(state({ page: 2 }))
  })

  it('keeps a game sort in a Bo3 list and any other sort in either', () => {
    const game = state({ sortBy: 'game_2', sortDir: 'asc' })
    expect(withoutHiddenSort(game, false)).toEqual(game)
    const duration = state({ sortBy: 'duration', sortDir: 'asc' })
    expect(withoutHiddenSort(duration, true)).toEqual(duration)
  })
})

describe('the URL', () => {
  it('reads every filter, the sort and the page', () => {
    expect(
      matchListFromUrl({
        from: '2026-09-01',
        to: '2026-09-07',
        result: 'win',
        type: 'casual',
        sort: 'duration',
        dir: 'asc',
        page: '2',
      }),
    ).toEqual({
      from: '2026-09-01',
      to: '2026-09-07',
      result: 'win',
      kind: 'casual',
      bestOf: null,
      archetype: null,
      sortBy: 'duration',
      sortDir: 'asc',
      page: 2,
    })
  })

  it('falls back to the defaults for anything unknown', () => {
    expect(matchListFromUrl({ result: 'draw', type: 'tournament', sort: 'colour', archetype: 'x7', dir: 'up', page: '0' })).toEqual(
      DEFAULT_LIST,
    )
    expect(matchListFromUrl({})).toEqual(DEFAULT_LIST)
  })

  it('has no direction without a sort column', () => {
    expect(matchListFromUrl({ dir: 'asc' }).sortDir).toBe('desc')
  })

  it('leaves the defaults out and round trips', () => {
    expect(matchListUrlQuery(DEFAULT_LIST)).toEqual({})
    const shown = state({ from: '2026-09-01', result: 'loss', kind: 'manual', sortBy: 'started_at', sortDir: 'asc', page: 4 })
    const url = matchListUrlQuery(shown)
    expect(url).toEqual({ from: '2026-09-01', result: 'loss', type: 'manual', sort: 'started_at', dir: 'asc', page: '4' })
    expect(matchListFromUrl(url)).toEqual(shown)
  })

  it('keeps the other keys of the page (the deck version and timeframe)', () => {
    const current = { version: '3', timeframe: 'week', result: 'win', page: '2' }
    expect(withListQuery(current, state({ kind: 'league' }))).toEqual({ version: '3', timeframe: 'week', type: 'league' })
  })
})

describe('isFiltered', () => {
  it('is true when any filter is set, not for a sort or page', () => {
    expect(isFiltered(DEFAULT_LIST)).toBe(false)
    expect(isFiltered(state({ sortBy: 'outcome', page: 3 }))).toBe(false)
    expect(isFiltered(state({ to: '2026-09-07' }))).toBe(true)
    expect(isFiltered(state({ result: 'win' }))).toBe(true)
    expect(isFiltered(state({ kind: 'casual' }))).toBe(true)
  })
})

describe('matchDurationText', () => {
  const start = Date.UTC(2026, 8, 7, 12, 0, 0)
  const after = (seconds: number) => start + seconds * 1000

  it('is null for a match that has not ended', () => {
    expect(matchDurationText(start, null)).toBeNull()
  })

  it('names the largest whole unit, as 0.x (Carbon diffForHumans)', () => {
    expect(matchDurationText(start, after(1))).toBe('1 second')
    expect(matchDurationText(start, after(59.9))).toBe('59 seconds')
    expect(matchDurationText(start, after(60))).toBe('1 minute')
    expect(matchDurationText(start, after(35 * 60 + 50))).toBe('35 minutes')
    expect(matchDurationText(start, after(3600))).toBe('1 hour')
    expect(matchDurationText(start, after(2 * 3600 - 1))).toBe('1 hour')
    expect(matchDurationText(start, after(5 * 3600))).toBe('5 hours')
    expect(matchDurationText(start, after(26 * 3600))).toBe('1 day')
    expect(matchDurationText(start, after(15 * 86400))).toBe('2 weeks')
  })

  it('never says zero, and ignores the order of the times', () => {
    expect(matchDurationText(start, start)).toBe('1 second')
    expect(matchDurationText(start, start - 120_000)).toBe('2 minutes')
  })
})

describe('matchDateText', () => {
  it('names the month, on the local clock (#390: d/m/Y was read month first)', () => {
    expect(matchDateText(new Date(2026, 9, 2, 15, 5).getTime())).toBe('2 Oct 2026, 3:05pm')
    expect(matchDateText(new Date(2026, 8, 7, 15, 5).getTime())).toBe('7 Sep 2026, 3:05pm')
    expect(matchDateText(new Date(2026, 11, 25, 9, 30).getTime())).toBe('25 Dec 2026, 9:30am')
  })

  it('shows midnight and noon as 12', () => {
    expect(matchDateText(new Date(2026, 0, 1, 0, 0).getTime())).toBe('1 Jan 2026, 12:00am')
    expect(matchDateText(new Date(2026, 0, 1, 12, 45).getTime())).toBe('1 Jan 2026, 12:45pm')
  })
})

describe('gameCells', () => {
  it('gives three cells, a missing game as null', () => {
    const cells = gameCells([
      { won: true, onPlay: true },
      { won: false, onPlay: false },
    ])
    expect(cells).toEqual([
      { text: 'Win', tone: 'text-success', side: '(OTP)' },
      { text: 'Loss', tone: 'text-destructive', side: '(OTD)' },
      null,
    ])
  })

  it('leaves the side out when the play or draw is unknown', () => {
    expect(gameCells([{ won: false, onPlay: null }])[0]).toEqual({ text: 'Loss', tone: 'text-destructive', side: null })
  })

  it('shows no cells for a match without decided games', () => {
    expect(gameCells([])).toEqual([null, null, null])
  })

  it('keeps a game with no known winner in its place', () => {
    const cells = gameCells([
      { won: true, onPlay: true },
      { won: null, onPlay: null },
      { won: false, onPlay: false },
    ])
    expect(cells.map((cell) => cell?.text)).toEqual(['Win', '?', 'Loss'])
    expect(cells[1]).toEqual({ text: '?', tone: 'text-muted-foreground', side: null })
  })
})

describe('rowMenuEnabled', () => {
  it('offers no menu on a match still in progress', () => {
    expect(rowMenuEnabled({ state: 'in_progress' })).toBe(false)
  })

  it('offers the menu once the match has ended or completed', () => {
    for (const state of ['ended', 'complete', 'abandoned']) {
      expect(rowMenuEnabled({ state }), state).toBe(true)
    }
  })
})

describe('the type filter per platform', () => {
  it('lists the Arena options, Casual sending unranked', () => {
    expect(kindOptions('arena')).toEqual([
      { value: 'all', label: 'All Types' },
      { value: 'ranked', label: 'Ranked' },
      { value: 'unranked', label: 'Casual' },
      { value: 'manual', label: 'Manual' },
    ])
  })

  it('keeps the MTGO options', () => {
    expect(kindOptions('mtgo').map((option) => option.label)).toEqual(['All Types', 'League', 'Casual', 'Manual'])
  })

  it('keeps ranked and unranked from the URL on Arena only', () => {
    expect(matchListFromUrl({ type: 'ranked' }, 'arena').kind).toBe('ranked')
    expect(matchListFromUrl({ type: 'unranked' }, 'arena').kind).toBe('unranked')
    expect(matchListFromUrl({ type: 'ranked' }, 'mtgo').kind).toBeNull()
    expect(matchListFromUrl({ type: 'unranked' }).kind).toBeNull()
    expect(matchListFromUrl({ type: 'league' }, 'arena').kind).toBeNull()
  })
})

describe('the Bo1 and Bo3 filter', () => {
  it('lists All kinds, Bo1 and Bo3', () => {
    expect(bestOfOptions()).toEqual([
      { value: 'all', label: 'All kinds' },
      { value: 'bo1', label: 'Bo1' },
      { value: 'bo3', label: 'Bo3' },
    ])
  })

  it('round trips through the URL as bo on Arena', () => {
    const shown = state({ bestOf: 'bo3', page: 2 })
    expect(matchListUrlQuery(shown)).toEqual({ bo: 'bo3', page: '2' })
    expect(matchListFromUrl(matchListUrlQuery(shown), 'arena')).toEqual(shown)
    expect(matchListFromUrl({ bo: 'bo1' }, 'arena').bestOf).toBe('bo1')
  })

  it('drops bo on MTGO and for unknown values', () => {
    expect(matchListFromUrl({ bo: 'bo1' }, 'mtgo').bestOf).toBeNull()
    expect(matchListFromUrl({ bo: 'bo1' }).bestOf).toBeNull()
    expect(matchListFromUrl({ bo: 'bo5' }, 'arena').bestOf).toBeNull()
  })

  it('is sent in the query, replaced in the page URL and counts as a filter', () => {
    expect(matchListQuery(state({ bestOf: 'bo1' })).bestOf).toBe('bo1')
    expect(withListQuery({ version: '3', bo: 'bo3' }, state())).toEqual({ version: '3' })
    expect(isFiltered(state({ bestOf: 'bo1' }))).toBe(true)
  })
})

describe('arenaRankCell', () => {
  const gold = { class: 'Gold', level: 2, step: 3, mythicPercentile: null, mythicPlacement: null, label: 'Gold 2' }

  it('always picks the constructed icon for a ranked match', () => {
    const cell = arenaRankCell({ platform: 'arena', matchType: 'Ranked', rank: gold })
    expect(cell?.iconUrl).toContain('constructed_gold_t2')
    expect(cell?.label).toBe('Gold 2')
    expect(cell?.text).toBe('')
  })

  it('has no cell for a casual match', () => {
    expect(arenaRankCell({ platform: 'arena', matchType: 'Unranked', rank: null })).toBeNull()
  })

  it('has no cell for MTGO', () => {
    expect(arenaRankCell({ platform: 'mtgo', matchType: 'Ranked', rank: gold })).toBeNull()
  })
})

describe('Endstep in the list', () => {
  it('lists the Arena type options and keeps ranked and best of from the URL', () => {
    expect(kindOptions('endstep')).toEqual(kindOptions('arena'))
    expect(matchListFromUrl({ type: 'ranked', bo: 'bo3' }, 'endstep')).toMatchObject({ kind: 'ranked', bestOf: 'bo3' })
  })

  it('tags rows with their kind and type like Arena rows', () => {
    expect(arenaKindLabel({ platform: 'endstep', bestOf: 'bo1' })).toBe('Bo1')
    expect(arenaTypeLabel({ platform: 'endstep', matchType: 'Ranked' })).toBe('Ranked')
  })

  it('shows the rating after with its signed change and trend', () => {
    expect(endstepRatingCell({ before: 1371, after: 1336, delta: -35 })).toEqual({ after: '1336', change: '-35', trend: 'down', tone: 'text-destructive' })
    expect(endstepRatingCell({ before: 1500, after: 1512, delta: 12 })).toEqual({ after: '1512', change: '+12', trend: 'up', tone: 'text-success' })
    expect(endstepRatingCell({ before: 1500, after: 1500, delta: 0 })).toEqual({ after: '1500', change: '0', trend: 'flat', tone: 'text-muted-foreground' })
  })

  it('says how to get Endstep matches when there are none', () => {
    expect(noMatchesRecordedText('endstep')).toBe(
      'No Endstep matches yet. Switch on Endstep in Settings, Integrations, and install the browser extension.',
    )
    expect(noMatchesRecordedText('mtgo')).toBe('No matches recorded')
    expect(noMatchesRecordedText('arena')).toBe('No matches recorded')
  })
})
