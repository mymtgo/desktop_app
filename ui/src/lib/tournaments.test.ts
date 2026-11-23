import { describe, expect, it } from 'vitest'
import type { TournamentMatchView, TournamentView } from '@/bindings'
import { isLive, liveText, roundLabel, roundRows, roundsText, sideTotal } from './tournaments'

function match(overrides: Partial<TournamentMatchView> = {}): TournamentMatchView {
  return {
    token: 'm1',
    state: 'complete',
    result: 'W',
    opponentName: 'Qopponent',
    opponentArchetype: null,
    games: [
      { result: 'W', onPlay: true },
      { result: 'L', onPlay: false },
      { result: 'W', onPlay: null },
    ],
    startedAtMs: Date.UTC(2026, 9, 6, 12, 0),
    durationSeconds: 1500,
    round: 1,
    notes: null,
    ...overrides,
  }
}

function tournament(matches: TournamentMatchView[]): TournamentView {
  return {
    eventId: 500,
    name: 'Tournament 500 (06-10-2026)',
    format: 'Modern',
    startedAtMs: Date.UTC(2026, 9, 6, 12, 0),
    versionLabel: 'v1',
    results: ['W'],
    matches,
    avgMatchSeconds: 1500,
    gameWins: 2,
    gameLosses: 1,
    onPlayRecord: { wins: 1, losses: 0 },
    onDrawRecord: { wins: 0, losses: 1 },
    matchesCount: matches.length,
    inProgressCount: 0,
    wins: 1,
    losses: 0,
  }
}

describe('roundLabel', () => {
  it('names the round as 0.x did', () => {
    expect(roundLabel(1)).toBe('R1')
    expect(roundLabel(12)).toBe('R12')
  })

  it('is a dash when the round is unknown', () => {
    expect(roundLabel(null)).toBe('-')
  })
})

describe('isLive', () => {
  it('is live while the match has no result', () => {
    expect(isLive(match({ state: 'in_progress', result: null }))).toBe(true)
    expect(isLive(match({ state: 'ended', result: null }))).toBe(true)
    expect(isLive(match())).toBe(false)
  })
})

describe('roundRows', () => {
  it('turns every match into a rounds table row, in order', () => {
    const rows = roundRows(
      tournament([
        match({ opponentArchetype: 'Qburn' }),
        match({
          token: 'm2',
          state: 'in_progress',
          result: null,
          opponentName: null,
          games: [{ result: 'L', onPlay: true }],
          durationSeconds: null,
          round: 2,
        }),
      ]),
    )
    expect(rows).toEqual([
      {
        token: 'm1',
        round: 'R1',
        live: false,
        won: true,
        opponent: 'Qopponent',
        archetype: 'Qburn',
        games: [
          { text: 'Win', tone: 'text-success', side: 'OTP' },
          { text: 'Loss', tone: 'text-destructive', side: 'OTD' },
          { text: 'Win', tone: 'text-success', side: null },
        ],
        time: '25m',
      },
      {
        token: 'm2',
        round: 'R2',
        live: true,
        won: null,
        opponent: null,
        archetype: null,
        games: [{ text: 'Loss', tone: 'text-destructive', side: 'OTP' }, null, null],
        time: '-',
      },
    ])
  })

  it('reads a lost match as not won', () => {
    expect(roundRows(tournament([match({ result: 'L' })]))[0]!.won).toBe(false)
  })

  it('always gives three game cells and shows only the first three games', () => {
    const games = [1, 2, 3, 4].map(() => ({ result: 'W' as const, onPlay: true }))
    expect(roundRows(tournament([match({ games })]))[0]!.games).toHaveLength(3)
    expect(roundRows(tournament([match({ games: [] })]))[0]!.games).toEqual([null, null, null])
  })

  it('shows a dash without a duration', () => {
    expect(roundRows(tournament([match({ durationSeconds: null })]))[0]!.time).toBe('-')
  })
})

describe('roundsText and liveText', () => {
  it('counts rounds as 0.x did', () => {
    expect(roundsText(1)).toBe('1 round')
    expect(roundsText(0)).toBe('0 rounds')
    expect(roundsText(3)).toBe('3 rounds')
  })

  it('counts live matches', () => {
    expect(liveText(2)).toBe('2 live')
  })
})

describe('sideTotal', () => {
  it('adds wins and losses on one side', () => {
    expect(sideTotal({ wins: 3, losses: 2 })).toBe(5)
  })
})

