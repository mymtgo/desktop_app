import { describe, expect, it } from 'vitest'
import type { LeagueStatus } from '@/bindings'
import { sampleEventState, TOURNAMENT_ROWS } from './overlaySamples'

const NOW = 1_800_000_000_000
const MINUTE = 60_000

function sample(row: number) {
  return sampleEventState(TOURNAMENT_ROWS[row], { art: null, size: 'full', now: NOW })
}

describe('the tournament samples', () => {
  it('cover every row of spec 5.4 with its status', () => {
    const statuses: Record<number, LeagueStatus> = {
      1: 'registered',
      2: 'registered',
      3: 'in_game',
      4: 'sideboarding',
      5: 'waiting_for_round',
      6: 'waiting_for_round',
      7: 'waiting_for_round',
      8: 'bye',
      9: 'made_cut',
      10: 'in_game',
      11: 'sideboarding',
      12: 'waiting_for_round',
      13: 'trophied',
      14: 'top_cut',
      15: 'missed_cut',
      16: 'complete',
      17: 'dropped',
      18: 'eliminated',
      19: 'idle',
    }
    for (let row = 1; row <= 19; row++) {
      expect(sample(row).status, `row ${row}`).toBe(statuses[row])
    }
  })

  it('are tournaments with a block, except idle', () => {
    for (let row = 1; row <= 18; row++) {
      const state = sample(row)
      expect(state.event?.kind, `row ${row}`).toBe('tournament')
      expect(state.tournament, `row ${row}`).toBeDefined()
    }
    expect(sample(19).tournament).toBeUndefined()
    expect(sample(19).event).toBeNull()
  })

  it('count their times from now, the starting and soon rows in the past', () => {
    expect(sample(1).tournament?.startsAtMs).toBe(NOW + 12 * MINUTE)
    expect(sample(2).tournament?.startsAtMs).toBeLessThan(NOW)
    // No estimate while a round runs (spec 5.2), and no registration count.
    expect(sample(5).tournament?.nextRoundAtMs).toBeNull()
    expect(sample(1).tournament?.players).toBeNull()
    expect(sample(6).tournament?.matchesLeft).toBe(0)
    expect(sample(6).tournament?.nextRoundAtMs).toBeLessThan(NOW)
    expect(sample(7).tournament?.matchesLeft).toBe(0)
  })

  it('play the playoffs with the overall record and the stage', () => {
    expect(sample(10).tournament?.stage).toBe('semifinal')
    expect(sample(10).record).toEqual({ wins: 6, losses: 1 })
    expect(sample(14).tournament?.place).toBe(6)
    expect(sample(16).tournament?.cutSize).toBeNull()
    expect(sample(16).event?.name).toBe('Modern Preliminary')
  })
})

describe('the strip samples', () => {
  it('put a waiting Challenge beside a league match', () => {
    const state = sampleEventState('league_with_challenge', { art: null, size: 'full', now: NOW })
    expect(state.status).toBe('in_game')
    expect(state.event?.kind).toBe('league')
    expect(state.other).toMatchObject({ kind: 'tournament', status: 'waiting_for_round', record: { wins: 4, losses: 0 } })
    expect(state.other?.tournament?.nextRoundAtMs).toBe(NOW + 14 * MINUTE)
  })

  it('put a live league match beside a Challenge round', () => {
    const state = sampleEventState('challenge_with_league', { art: null, size: 'full', now: NOW })
    expect(state.status).toBe('in_game')
    expect(state.tournament?.round).toBe(4)
    expect(state.other).toMatchObject({ kind: 'league', status: 'in_game', match: { game: 2, gamesWon: 1, gamesLost: 0 } })
  })

  it('leave a league card without tournament or strip keys', () => {
    for (const status of ['in_game', 'waiting', 'complete', 'idle'] as const) {
      const state = sampleEventState(status, { art: null, size: 'full', now: NOW })
      expect('tournament' in state, status).toBe(false)
      expect('other' in state, status).toBe(false)
    }
  })
})
