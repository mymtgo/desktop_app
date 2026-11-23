import { describe, expect, it } from 'vitest'
import type { GameStatsRowView, MatchHistoryView, MatchupView, PerGameView } from '@/bindings'
import {
  LOW_DATA_THRESHOLD,
  formatAverage,
  formatPercent,
  gameStatGroups,
  hasGames,
  screenshotGames,
  screenshotName,
  trendPoints,
  trendTicks,
  nextSpreadSort,
  spreadColumns,
  rankMatchups,
  sortMatchups,
  splitByData,
  winRateTextClass,
} from './matchups'

function matchup(name: string, overrides: Partial<MatchupView> = {}): MatchupView {
  return {
    archetypeId: name.length,
    name,
    colors: [],
    matchWinRate: 50,
    gameWinRate: 50,
    matches: 5,
    matchRecord: '1 - 1',
    gameRecord: '1 - 1',
    matchWins: 1,
    matchLosses: 1,
    matchDraws: 0,
    gamesWon: 1,
    gamesLost: 1,
    totalGames: 2,
    otpWinRate: 50,
    avgTurns: null,
    ...overrides,
  }
}

describe('the matchup spread (0.x MatchupSpreadTable)', () => {
  it('sorts by matches, most first, by default and flips a column clicked twice', () => {
    const list = [matchup('Qa', { matches: 4 }), matchup('Qb', { matches: 9 }), matchup('Qc', { matches: 6 })]
    expect(sortMatchups(list, { key: 'matches', desc: true }).map((m) => m.name)).toEqual(['Qb', 'Qc', 'Qa'])
    expect(nextSpreadSort({ key: 'matches', desc: true }, 'matches')).toEqual({ key: 'matches', desc: false })
    expect(nextSpreadSort({ key: 'matches', desc: true }, 'name')).toEqual({ key: 'name', desc: false })
    expect(nextSpreadSort({ key: 'name', desc: false }, 'winrate')).toEqual({ key: 'winrate', desc: true })
    expect(sortMatchups(list, { key: 'name', desc: false }).map((m) => m.name)).toEqual(['Qa', 'Qb', 'Qc'])
  })

  it('sorts the record by match wins and missing turns as zero', () => {
    const list = [matchup('Qa', { matchWins: 3, avgTurns: null }), matchup('Qb', { matchWins: 1, avgTurns: 7.5 })]
    expect(sortMatchups(list, { key: 'record', desc: true })[0].name).toBe('Qa')
    expect(sortMatchups(list, { key: 'avg_turns', desc: true })[0].name).toBe('Qb')
  })

  it('puts archetypes faced fewer than four times below a divider', () => {
    expect(LOW_DATA_THRESHOLD).toBe(4)
    const { main, lowData } = splitByData([matchup('Qa', { matches: 4 }), matchup('Qb', { matches: 3 })])
    expect(main.map((m) => m.name)).toEqual(['Qa'])
    expect(lowData.map((m) => m.name)).toEqual(['Qb'])
  })

  it('colours a rate above 50 green, below 50 red and 50 plain', () => {
    expect(winRateTextClass(51)).toBe('text-success')
    expect(winRateTextClass(49)).toBe('text-destructive')
    expect(winRateTextClass(50)).toBe('')
  })
})

describe('best and worst matchups (0.x KpiBestWorstMatchups)', () => {
  it('needs three matches, ranks by win rate then matches, and takes the median', () => {
    const spread = [
      matchup('Qthin', { matches: 2, matchWinRate: 100 }),
      matchup('Qlow', { matches: 3, matchWinRate: 20 }),
      matchup('Qhigh', { matches: 3, matchWinRate: 80 }),
      matchup('Qhigher', { matches: 5, matchWinRate: 80 }),
      matchup('Qmid', { matches: 4, matchWinRate: 50 }),
    ]
    const ranked = rankMatchups(spread)
    expect(ranked.best?.name).toBe('Qhigher')
    expect(ranked.worst?.name).toBe('Qlow')
    expect(ranked.middle?.name).toBe('Qmid')
  })

  it('names no worst with one archetype and no middle with two', () => {
    expect(rankMatchups([matchup('Qone', { matches: 3 })])).toEqual({
      best: expect.objectContaining({ name: 'Qone' }),
      middle: null,
      worst: null,
    })
    const two = rankMatchups([matchup('Qa', { matchWinRate: 60 }), matchup('Qb', { matchWinRate: 40 })])
    expect(two.middle).toBeNull()
    expect(two.worst?.name).toBe('Qb')
    expect(rankMatchups([])).toEqual({ best: null, middle: null, worst: null })
  })
})

function statRow(group: string, split: string, wins = 0, losses = 0): GameStatsRowView {
  return {
    group,
    split,
    wins,
    losses,
    winRate: null,
    mulligans: null,
    opponentMulligans: null,
    turns: null,
  }
}

describe('the Game Stats table (0.x GameStatsTable)', () => {
  it('groups the rows by game with 0.x labels', () => {
    const rows = ['all_games', 'game_1', 'game_2', 'game_3'].flatMap((group) =>
      ['overall', 'play', 'draw'].map((split) => statRow(group, split)),
    )
    const groups = gameStatGroups(rows)
    expect(groups.map((group) => group.label)).toEqual(['All Games', 'Game 1', 'Game 2', 'Game 3'])
    expect(groups[0].rows.map((row) => row.label)).toEqual(['Overall', 'On Play', 'On Draw'])
  })

  it('formats averages with two decimals, rates with one, and a dash for none', () => {
    expect(formatAverage(1)).toBe('1.00')
    expect(formatAverage(null)).toBe('-')
    expect(formatPercent(66.7)).toBe('66.7%')
    expect(formatPercent(null)).toBe('-')
  })

  it('knows when there are no games yet', () => {
    expect(hasGames([statRow('all_games', 'overall')])).toBe(false)
    expect(hasGames([statRow('all_games', 'overall', 1, 0)])).toBe(true)
  })
})

function history(outcome: string, dateLabel: string): MatchHistoryView {
  return {
    token: `q-${dateLabel}`,
    startedAtMs: 0,
    dateLabel,
    isLeague: false,
    opponent: 'Qopponent',
    score: outcome === 'win' ? '2-0' : '0-2',
    outcome,
    gameResults: [],
  }
}

describe('the drawer trend (0.x MatchupTrendChart)', () => {
  it('runs the win rate oldest to newest over the match sequence, draws counted', () => {
    const points = trendPoints([history('win', 'Mar 4'), history('draw', 'Mar 3'), history('loss', 'Mar 2')])
    expect(points.map((point) => [point.index, point.dateLabel, point.cumRate])).toEqual([
      [0, 'Mar 2', 0],
      [1, 'Mar 3', 0],
      [2, 'Mar 4', 33],
    ])
    expect(points[2]).toMatchObject({ cumWins: 1, cumLosses: 1, cumDraws: 1, outcome: 'win' })
  })

  it('labels at most six matches, always the last', () => {
    expect(trendTicks(4)).toEqual([0, 1, 2, 3])
    expect(trendTicks(12)).toEqual([0, 3, 6, 9, 11])
    expect(trendTicks(0)).toEqual([])
  })
})

describe('the matchup screenshot (0.x MatchupScreenshot)', () => {
  it('shows games 1 to 3, empty where none was played', () => {
    const played: PerGameView[] = [{ gameNumber: 2, winRate: 50, record: '1 - 1', wins: 1, losses: 1 }]
    expect(screenshotGames(played)).toEqual([
      { gameNumber: 1, game: null },
      { gameNumber: 2, game: played[0] },
      { gameNumber: 3, game: null },
    ])
  })

  it('names the file after the archetype', () => {
    expect(screenshotName('Qmono Red / Burn')).toBe('qmono-red-burn-matchup.png')
    expect(screenshotName('   ')).toBe('matchup.png')
  })
})

describe('the spread columns', () => {
  it('has the game columns in Bo3', () => {
    const labels = spreadColumns(false).map((c) => c.label)
    expect(labels).toContain('Games')
    expect(labels).toContain('Game %')
  })

  it('drops them in Bo1', () => {
    const labels = spreadColumns(true).map((c) => c.label)
    expect(labels).not.toContain('Games')
    expect(labels).not.toContain('Game %')
    expect(labels).toEqual(['Record', 'Matches', 'Win %', 'OTP %', 'Avg Turns'])
  })
})
