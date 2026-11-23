import { describe, expect, it } from 'vitest'
import type { RunCardView, RunMatchView } from '@/bindings'
import {
  automaticLabel,
  avgFinishText,
  canDeleteLeague,
  avgMinutes,
  createErrorField,
  defaultLeagueName,
  defaultStartedAt,
  deleteLeagueQuestion,
  droppedTitle,
  dropRateText,
  durationText,
  gameCell,
  LEAGUE_SORTS,
  leagueQueryFromUrl,
  leagueSummaryText,
  leagueUrlQuery,
  movedFromText,
  rateText,
  remainingSlots,
  resultBadgeLabel,
  resultBadgeTone,
  resultDots,
  runRecord,
  STATE_CHIPS,
  STATUS_CHOICES,
  statusLabel,
  takesManualMatch,
  timeOfDayLabel,
  tixText,
  tixTone,
  whereNowText,
} from './leagues'

const HOUR = 3_600_000
const DAY = 24 * HOUR

function match(overrides: Partial<RunMatchView> = {}): RunMatchView {
  return {
    token: 't1',
    result: 'W',
    opponentName: 'Qorvath',
    games: [],
    startedAtMs: 0,
    durationSeconds: null,
    notes: null,
    manual: false,
    opponentArchetype: null,
    opponentArchetypeId: null,
    ...overrides,
  }
}

function run(overrides: Partial<RunCardView> = {}): RunCardView {
  return {
    key: 'log:1',
    source: 'log',
    name: 'League',
    format: 'Modern',
    formatCode: 'CMODERN',
    state: 'complete',
    manual: false,
    startedAtMs: 0,
    droppedAtMs: null,
    deck: { id: 7, name: 'Qamurai Burn', colors: ['R'], coverArt: null, localCoverArt: null },
    versionLabel: 'v2',
    results: ['W', 'L', 'W', 'W', 'W'],
    matches: [],
    notes: null,
    classification: 'CASH',
    liveRound: null,
    avgMatchSeconds: null,
    timeOfDay: null,
    gameWins: 0,
    gameLosses: 0,
    onPlayRecord: { wins: 0, losses: 0 },
    onDrawRecord: { wins: 0, losses: 0 },
    tixDelta: null,
    topOpponentArchetype: null,
    topMatchups: [],
    limited: false,
    colors: null,
    userState: null,
    autoState: 'complete',
    ...overrides,
  }
}

describe('state chips and sorts', () => {
  it('lists the 0.x chips in order, cash reading "4-1+"', () => {
    expect(STATE_CHIPS).toEqual([
      { key: 'all', label: 'All' },
      { key: 'live', label: 'Live' },
      { key: 'trophies', label: 'Trophies' },
      { key: 'cash', label: '4-1+' },
      { key: 'finish', label: 'Finish' },
      { key: 'bricks', label: 'Bricks' },
    ])
  })

  it('lists the four sorts, newest first', () => {
    expect(LEAGUE_SORTS.map((sort) => [sort.value, sort.label])).toEqual([
      ['newest', 'Newest'],
      ['oldest', 'Oldest'],
      ['best', 'Best'],
      ['worst', 'Worst'],
    ])
  })
})

describe('leagueQueryFromUrl', () => {
  it('opens on every run, newest first, page 1', () => {
    expect(leagueQueryFromUrl({})).toEqual({
      format: null,
      state: 'all',
      deckId: null,
      archetypeId: null,
      search: null,
      sort: 'newest',
      page: 1,
    })
  })

  it('reads each filter from the query string', () => {
    expect(
      leagueQueryFromUrl({
        format: 'CMODERN',
        state: 'cash',
        deck: '12',
        archetype: '7',
        q: 'Qor',
        sort: 'best',
        page: '3',
      }),
    ).toEqual({ format: 'CMODERN', state: 'cash', deckId: 12, archetypeId: 7, search: 'Qor', sort: 'best', page: 3 })
  })

  it('falls back to the default for anything it does not know', () => {
    expect(
      leagueQueryFromUrl({ format: '', state: 'gold', deck: 'abc', archetype: 'x', q: '   ', sort: 'random', page: '-2' }),
    ).toEqual({ format: null, state: 'all', deckId: null, archetypeId: null, search: null, sort: 'newest', page: 1 })
  })
})

describe('leagueUrlQuery', () => {
  it('leaves defaults out', () => {
    expect(leagueUrlQuery(leagueQueryFromUrl({}))).toEqual({})
  })

  it('round trips every filter', () => {
    const query = {
      format: 'CPAUPER',
      state: 'bricks',
      deckId: 4,
      archetypeId: 9,
      search: 'Qa',
      sort: 'worst',
      page: 2,
    } as const
    expect(leagueUrlQuery(query)).toEqual({
      format: 'CPAUPER',
      state: 'bricks',
      deck: '4',
      archetype: '9',
      q: 'Qa',
      sort: 'worst',
      page: '2',
    })
    expect(leagueQueryFromUrl(leagueUrlQuery(query))).toEqual(query)
  })
})

describe('result dots and the score', () => {
  it('counts wins and losses, ignoring open rounds', () => {
    expect(runRecord(['W', 'L', 'W', null, null])).toEqual({ wins: 2, losses: 1 })
  })

  it('keeps the padding: a live run shows its open rounds as empty dots', () => {
    expect(resultDots(['W', 'L', null, null, null])).toEqual(['win', 'loss', 'open', 'open', 'open'])
  })

  it('shows a finished run as it was played', () => {
    expect(resultDots(['W', 'W', 'W', 'W', 'W'])).toEqual(['win', 'win', 'win', 'win', 'win'])
    expect(resultDots(['L', 'L'])).toEqual(['loss', 'loss'])
  })
})

describe('result badge', () => {
  it('labels a live run with its round, else the classification', () => {
    expect(resultBadgeLabel('LIVE', 3)).toBe('LIVE-R3')
    expect(resultBadgeLabel('LIVE', null)).toBe('LIVE')
    expect(resultBadgeLabel('TROPHY', null)).toBe('TROPHY')
    expect(resultBadgeLabel('BRICK', null)).toBe('BRICK')
  })

  it('gives each classification its 0.x ring', () => {
    expect(resultBadgeTone('TROPHY')).toBe('from-pink-400 via-sky-300 to-blue-400')
    expect(resultBadgeTone('CASH')).toBe('from-success/60 to-success text-success')
    expect(resultBadgeTone('FINISH')).toBe('from-border to-border')
    expect(resultBadgeTone('BRICK')).toBe('from-destructive/50 to-destructive/80 text-destructive')
    expect(resultBadgeTone('LIVE')).toBe('from-primary/50 to-primary text-primary')
    expect(resultBadgeTone('EMPTY')).toBe('from-border to-border text-muted-foreground')
  })
})

describe('card labels', () => {
  it('rounds the average match to whole minutes, nothing when unknown or zero', () => {
    expect(avgMinutes(1530)).toBe(26)
    expect(avgMinutes(null)).toBeNull()
    expect(avgMinutes(0)).toBeNull()
  })

  it('capitalises the time of day', () => {
    expect(timeOfDayLabel('morning')).toBe('Morning')
    expect(timeOfDayLabel('night')).toBe('Night')
    expect(timeOfDayLabel(null)).toBeNull()
  })

  it('signs the tix delta and tones it', () => {
    expect(tixText(12.4)).toBe('+12 tix')
    expect(tixText(-8)).toBe('-8 tix')
    expect(tixText(0)).toBe('0 tix')
    expect(tixText(null)).toBeNull()
    expect(tixTone(5)).toBe('text-emerald-400')
    expect(tixTone(-5)).toBe('text-destructive')
    expect(tixTone(0)).toBe('text-muted-foreground')
    expect(tixTone(null)).toBe('')
  })

  it('says when a run was dropped', () => {
    const now = 10 * DAY
    expect(droppedTitle(run({ state: 'dropped', droppedAtMs: now - 2 * DAY }), now)).toBe('Dropped 2 days ago')
    expect(droppedTitle(run({ state: 'dropped', droppedAtMs: null }), now)).toBe('Dropped before completion')
  })

  it('shows a match length in minutes and a dash when unknown', () => {
    expect(durationText(1830)).toBe('31m')
    expect(durationText(null)).toBe('-')
    expect(durationText(0)).toBe('-')
  })

  it('shows a game as Win or Loss with the side it was played from', () => {
    expect(gameCell({ result: 'W', onPlay: true })).toEqual({ text: 'Win', tone: 'text-success', side: 'OTP' })
    expect(gameCell({ result: 'L', onPlay: false })).toEqual({ text: 'Loss', tone: 'text-destructive', side: 'OTD' })
    expect(gameCell({ result: 'W', onPlay: null })).toEqual({ text: 'Win', tone: 'text-success', side: null })
    expect(gameCell(undefined)).toBeNull()
  })

  it('counts the matches a manual league can still link', () => {
    expect(remainingSlots(0)).toBe(5)
    expect(remainingSlots(3)).toBe(2)
    expect(remainingSlots(5)).toBe(0)
    expect(remainingSlots(6)).toBe(0)
  })

  it('offers a manual match only where the sheet can place it: an Active run or a manual league, under five matches', () => {
    const four = [match(), match(), match(), match()]
    const five = [...four, match()]
    expect(takesManualMatch(run({ state: 'active', matches: four }))).toBe(true)
    expect(takesManualMatch(run({ state: 'active', matches: five }))).toBe(false)
    for (const state of ['complete', 'dropped', 'partial'] as const) {
      expect(takesManualMatch(run({ state, matches: four }))).toBe(false)
    }
    expect(takesManualMatch(run({ source: 'manual', manual: true, state: 'complete', matches: four }))).toBe(true)
    expect(takesManualMatch(run({ source: 'manual', manual: true, state: 'complete', matches: five }))).toBe(false)
  })
})

describe('KPI labels', () => {
  it('shows a rate as a percentage, a dash without finished runs', () => {
    expect(rateText(25)).toBe('25%')
    expect(rateText(0)).toBe('0%')
    expect(rateText(null)).toBe('-')
  })

  it('reads the drop rate as a share of runs', () => {
    expect(dropRateText(10)).toBe('10% of runs')
    expect(dropRateText(null)).toBe('no finished runs')
  })

  it('shows the average finish with one decimal as 0.x did', () => {
    expect(avgFinishText(3.4)).toEqual({ value: '3.4', caption: 'wins per run' })
    expect(avgFinishText(3)).toEqual({ value: '3', caption: 'wins per run' })
    expect(avgFinishText(null)).toEqual({ value: '-', caption: 'no finished runs' })
  })
})

describe('leagueSummaryText', () => {
  const now = 10 * DAY

  it('copies the 0.x summary: deck, format and version, when and the score, then each match', () => {
    const text = leagueSummaryText(
      run({
        startedAtMs: now - 3 * HOUR,
        results: ['W', 'L', 'W', 'W', 'W'],
        matches: [
          match({ result: 'W', opponentName: 'Qorvath' }),
          match({ result: 'L', opponentName: 'Qelmira' }),
          match({ result: 'W', opponentName: null }),
        ],
      }),
      now,
    )
    expect(text).toBe(
      ['Qamurai Burn Modern v2', '3 hours ago · 4-1', 'W vs Qorvath', 'L vs Qelmira', 'W vs Unknown'].join('\n'),
    )
  })

  it('names a limited run by its set and kind, without a deck', () => {
    const text = leagueSummaryText(
      run({ deck: null, versionLabel: null, limited: true, name: 'QNW Draft', format: 'Draft', results: ['W'] }),
      now,
    )
    expect(text.split('\n')[0]).toBe('QNW Draft')
  })

  it('names the opponent archetype first, as 0.x', () => {
    const text = leagueSummaryText(
      run({
        startedAtMs: now - 3 * HOUR,
        results: ['W'],
        matches: [match({ result: 'W', opponentName: 'Qorvath', opponentArchetype: 'Qburn' })],
      }),
      now,
    )
    expect(text.split('\n')[2]).toBe('W vs Qburn')
  })

  it('counts only played rounds of a live run and leaves out a missing version', () => {
    const text = leagueSummaryText(
      run({ versionLabel: null, startedAtMs: now - DAY, results: ['W', 'W', null, null, null], matches: [] }),
      now,
    )
    expect(text).toBe(['Qamurai Burn Modern', '1 day ago · 2-0'].join('\n'))
  })

  it('names a run without a deck "Unknown deck"', () => {
    const text = leagueSummaryText(run({ deck: null, startedAtMs: now - 60_000, results: [] }), now)
    expect(text).toBe(['Unknown deck Modern v2', '1 minute ago · 0-0'].join('\n'))
  })
})

describe('manual league dialog', () => {
  it('starts at the current local minute', () => {
    expect(defaultStartedAt(new Date(2026, 9, 8, 7, 5, 59))).toBe('2026-10-08T07:05')
  })

  it('names the league after the format and the start, on a 12 hour clock', () => {
    expect(defaultLeagueName('Modern', '2026-10-08T19:05')).toBe('Modern League 08-10-2026 07:05pm')
    expect(defaultLeagueName('Pauper', '2026-01-02T00:30')).toBe('Pauper League 02-01-2026 12:30am')
    expect(defaultLeagueName('Legacy', '2026-01-02T12:00')).toBe('Legacy League 02-01-2026 12:00pm')
  })

  it('has no default name until a start is given', () => {
    expect(defaultLeagueName('Modern', '')).toBe('')
  })

  it('puts a refusal under the field it is about', () => {
    expect(createErrorField('The selected deck id is invalid.')).toBe('deck')
    expect(createErrorField('The started at field is required.')).toBe('startedAt')
    expect(createErrorField('The started at field must be a valid date.')).toBe('startedAt')
    expect(createErrorField('The name field is required.')).toBe('name')
    expect(createErrorField('The name field must not be greater than 100 characters.')).toBe('name')
    expect(createErrorField('tracking is not running')).toBeNull()
  })
})

describe('league self-management labels', () => {
  const OCT_11 = Date.UTC(2026, 9, 11, 12)

  it('offers the statuses in the menu order and names Automatic by the fold', () => {
    expect(STATUS_CHOICES.map((choice) => choice.label)).toEqual(['Active', 'Complete', 'Dropped', 'Partial'])
    expect(statusLabel('partial')).toBe('Partial')
    expect(automaticLabel('complete')).toBe('Automatic (Complete)')
  })

  it('offers delete only once the run does not read Active', () => {
    expect(canDeleteLeague(run({ state: 'active' }))).toBe(false)
    expect(canDeleteLeague(run({ state: 'complete' }))).toBe(true)
    // The player marked an Active run Complete: it reads Complete.
    expect(canDeleteLeague(run({ state: 'complete', userState: 'complete', autoState: 'active' }))).toBe(true)
  })

  it('asks what to do with the matches only when there are some', () => {
    expect(deleteLeagueQuestion(0)).toBe('Delete this league? You can restore it from Settings, Archives.')
    expect(deleteLeagueQuestion(1)).toBe('What should happen to its 1 match?')
    expect(deleteLeagueQuestion(4)).toBe('What should happen to its 4 matches?')
  })

  it('says where a match is now and where it came from', () => {
    const league = { key: 'q:1', name: 'Qleague', startedAtMs: OCT_11 }
    expect(whereNowText(league, 'en-GB')).toBe('in 11 Oct league')
    expect(whereNowText(null, 'en-GB')).toBe('no league')
    expect(movedFromText(league, 'en-GB')).toBe('It was in the 11 Oct league.')
    expect(movedFromText(null, 'en-GB')).toBe('It had no league before.')
  })
})
