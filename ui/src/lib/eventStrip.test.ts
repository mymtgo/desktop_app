import { describe, expect, it } from 'vitest'
import type { OtherEvent, TournamentBlock } from '@/bindings'
import { stripLive, stripSegments } from './eventStrip'

const NOW = 1_800_000_000_000
const MINUTE = 60_000

const BLOCK: TournamentBlock = {
  series: 'challenge',
  round: 4,
  stage: 'swiss',
  swissRounds: 6,
  cutSize: 8,
  rank: 12,
  players: 64,
  matchesLeft: null,
  nextRoundAtMs: null,
  startsAtMs: null,
  place: null,
}

function challenge(status: OtherEvent['status'], block: Partial<TournamentBlock>, wins: number, losses: number): OtherEvent {
  return {
    kind: 'tournament',
    name: 'Modern Challenge 64',
    status,
    record: { wins, losses },
    match: null,
    tournament: { ...BLOCK, ...block },
  }
}

function league(status: OtherEvent['status'], wins: number, losses: number, match: OtherEvent['match'] = null): OtherEvent {
  return { kind: 'league', name: 'Competitive League', status, record: { wins, losses }, match, tournament: null }
}

/** The strip as one line of text. */
function stripText(other: OtherEvent, now: number, compact: boolean): string {
  return stripSegments(other, now, compact)
    .map((segment) => segment.text)
    .join('')
}

const full = (other: OtherEvent) => stripText(other, NOW, false).toUpperCase()
const compact = (other: OtherEvent) => stripText(other, NOW, true)

describe('the other event strip (spec 8.3)', () => {
  it('shows a Challenge beside a live league match', () => {
    // While the round runs there is no estimate (spec 5.2).
    const waiting = challenge('waiting_for_round', { matchesLeft: 12 }, 4, 0)
    expect(full(waiting)).toBe('CHALLENGE · ROUND 5 NEXT · 4-0')
    expect(compact(waiting)).toBe('Challenge: R5 next')
    const between = challenge('waiting_for_round', { matchesLeft: 0, nextRoundAtMs: NOW + 2 * MINUTE }, 4, 0)
    expect(full(between)).toBe('CHALLENGE · ROUND 5 IN ~2 MIN · 4-0')
    expect(compact(between)).toBe('Challenge: R5 ~2 min')
    const passed = challenge('waiting_for_round', { matchesLeft: 0, nextRoundAtMs: NOW - MINUTE }, 4, 0)
    expect(full(passed)).toBe('CHALLENGE · ROUND 5 SOON · 4-0')
    expect(compact(passed)).toBe('Challenge: R5 soon')
    const registered = { ...challenge('registered', { round: 1, startsAtMs: NOW + 12 * MINUTE }, 0, 0), record: null }
    expect(full(registered)).toBe('CHALLENGE · STARTS IN ~12 MIN')
    expect(compact(registered)).toBe('Challenge: ~12 min')
    const cut = challenge('made_cut', { round: 6, rank: 3, nextRoundAtMs: NOW + 2 * MINUTE }, 5, 1)
    expect(full(cut)).toBe('CHALLENGE · TOP 8! PLAYOFFS IN ~2 MIN')
    expect(compact(cut)).toBe('Challenge: Top 8 next')
  })

  it('shows a finished Challenge for the finished time', () => {
    expect(full(challenge('missed_cut', { round: 6, place: 21 }, 4, 2))).toBe('CHALLENGE · MISSED CUT · 21ST · 4-2')
    expect(compact(challenge('missed_cut', { round: 6, place: 21 }, 4, 2))).toBe('Challenge: 21st, 4-2')
    expect(full(challenge('trophied', { round: 9, stage: 'final', place: 1 }, 8, 1))).toBe('CHALLENGE · CHAMPION · 8-1')
    expect(full(challenge('top_cut', { round: 7, stage: 'top8', place: 6 }, 5, 2))).toBe('CHALLENGE · TOP 8 · 6TH')
    expect(full(challenge('dropped', { round: 3 }, 1, 2))).toBe('CHALLENGE · DROPPED AFTER R3')
  })

  it('shows a league beside a live Challenge round', () => {
    const live = league('in_game', 3, 1, { game: 2, gamesWon: 1, gamesLost: 0 })
    expect(full(live)).toBe('LEAGUE · GAME 2 · 1-0 · RUN 3-1')
    expect(compact(live)).toBe('League: G2, 3-1')
    const sideboarding = league('sideboarding', 3, 1, { game: 2, gamesWon: 1, gamesLost: 0 })
    expect(full(sideboarding)).toBe('LEAGUE · SIDEBOARDING FOR GAME 2 · RUN 3-1')
    expect(compact(sideboarding)).toBe('League: SB G2')
    expect(full(league('trophied', 5, 0))).toBe('LEAGUE · TROPHIED 5-0')
    expect(full(league('complete', 4, 1))).toBe('LEAGUE · COMPLETE 4-1')
    expect(full(league('dropped', 1, 2))).toBe('LEAGUE · DROPPED 1-2')
    expect(compact(league('trophied', 5, 0))).toBe('League: 5-0')
  })

  it('shows a second tournament by its kind, waiting or live', () => {
    const showcase = (other: OtherEvent): OtherEvent => ({ ...other, name: 'Legacy Showcase Challenge', tournament: { ...other.tournament!, series: 'showcase_challenge' } })
    const waiting = showcase(challenge('waiting_for_round', { round: 2, matchesLeft: 5, nextRoundAtMs: NOW + 20 * MINUTE }, 2, 0))
    expect(full(waiting)).toBe('SHOWCASE · ROUND 3 IN ~20 MIN · 2-0')
    expect(compact(waiting)).toBe('Showcase: R3 ~20 min')
    const live = { ...showcase(challenge('in_game', { round: 3 }, 2, 0)), match: { game: 2, gamesWon: 1, gamesLost: 0 } }
    expect(full(live)).toBe('SHOWCASE · ROUND 3 · GAME 2 · 2-0')
  })

  it('names what comes after the last Swiss round', () => {
    const lastSwiss = challenge('waiting_for_round', { round: 6, matchesLeft: 0, nextRoundAtMs: NOW + 2 * MINUTE }, 5, 1)
    expect(full(lastSwiss)).toBe('CHALLENGE · TOP 8 NEXT · 5-1')
    expect(compact(lastSwiss)).toBe('Challenge: Top 8 next')
    const prelim = { ...lastSwiss, tournament: { ...lastSwiss.tournament!, series: 'preliminary' as const, round: 4, swissRounds: 4, cutSize: null } }
    expect(full(prelim)).toBe('PRELIM · RESULTS SOON · 5-1')
    expect(compact(prelim)).toBe('Prelim: results soon')
    const unknown = { ...lastSwiss, tournament: { ...lastSwiss.tournament!, swissRounds: null } }
    expect(full(unknown)).toBe('CHALLENGE · ROUND 7 IN ~2 MIN · 5-1')
  })

  it('bolds the time and the records', () => {
    const waiting = challenge('waiting_for_round', { matchesLeft: 12, nextRoundAtMs: NOW + 14 * MINUTE }, 4, 0)
    expect(stripSegments(waiting, NOW, false).filter((segment) => segment.bold).map((segment) => segment.text)).toEqual([
      '~14 min',
      '4-0',
    ])
  })

  it('lights the dot when a match is live or a round is about to start', () => {
    expect(stripLive(league('in_game', 3, 1, { game: 2, gamesWon: 1, gamesLost: 0 }), NOW)).toBe(true)
    expect(stripLive(challenge('waiting_for_round', { nextRoundAtMs: NOW + 14 * MINUTE }, 4, 0), NOW)).toBe(false)
    expect(stripLive(challenge('waiting_for_round', { nextRoundAtMs: NOW + 2 * MINUTE }, 4, 0), NOW)).toBe(true)
    expect(stripLive(challenge('waiting_for_round', { nextRoundAtMs: NOW - MINUTE }, 4, 0), NOW)).toBe(true)
    expect(stripLive(challenge('missed_cut', { place: 21 }, 4, 2), NOW)).toBe(false)
  })
})
