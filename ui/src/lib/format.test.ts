import { describe, expect, it } from 'vitest'
import type { MatchSummary } from '@/bindings'
import { decksByToken, outcomeLabel, signedNumber, stateBadge } from './format'

function row(overrides: Partial<MatchSummary>): MatchSummary {
  return {
    id: null,
    token: 't1',
    opponent: 'Qopponent',
    format: 'Modern',
    matchType: 'Constructed',
    platform: 'mtgo',
    bestOf: null,
    state: 'complete',
    outcome: 'win',
    tentativeOutcome: null,
    wins: 2,
    losses: 1,
    needsAttention: false,
    attentionReason: null,
    startedAtMs: 0,
    endedAtMs: null,
    revision: 1,
    notes: null,
    gameResults: [],
    manual: false,
    limited: false,
    opponentArchetype: null,
    ...overrides,
  }
}

describe('format', () => {
  it('labels finished outcomes', () => {
    expect(outcomeLabel(row({ outcome: 'win' }))).toEqual({ text: 'Win', tone: 'win', provisional: false })
    expect(outcomeLabel(row({ outcome: 'loss' }))).toEqual({ text: 'Loss', tone: 'loss', provisional: false })
    expect(outcomeLabel(row({ outcome: 'draw' }))).toEqual({ text: 'Draw', tone: 'draw', provisional: false })
    expect(outcomeLabel(row({ outcome: 'unknown' }))).toEqual({ text: 'Unknown', tone: 'muted', provisional: false })
  })

  it('shows a live hint as provisional and never as final', () => {
    const live = row({ state: 'in_progress', outcome: 'unknown', tentativeOutcome: 'win' })
    expect(outcomeLabel(live)).toEqual({ text: 'Win', tone: 'win', provisional: true })
    const early = row({ state: 'in_progress', outcome: 'unknown', tentativeOutcome: null })
    expect(outcomeLabel(early)).toEqual({ text: 'In progress', tone: 'muted', provisional: true })
    expect(outcomeLabel(row({ state: 'abandoned', outcome: 'unknown' }))).toEqual({
      text: 'Abandoned',
      tone: 'muted',
      provisional: false,
    })
  })

  it('badges only unfinished states', () => {
    expect(stateBadge(row({ state: 'in_progress' }))).toBe('Live')
    expect(stateBadge(row({ state: 'ended' }))).toBe('Ended')
    expect(stateBadge(row({ state: 'complete' }))).toBeNull()
    expect(stateBadge(row({ state: 'abandoned' }))).toBeNull()
  })
})

describe('decksByToken', () => {
  it('keys each listed match to its deck, for the Deck column', () => {
    expect(
      decksByToken([
        { token: 'a', deckId: 3, deckName: 'Burn' },
        { token: 'b', deckId: 4, deckName: 'Tron' },
      ]),
    ).toEqual({ a: { id: 3, name: 'Burn' }, b: { id: 4, name: 'Tron' } })
  })

  it('is empty without decks, so a table shows Unknown for each match', () => {
    expect(decksByToken([])).toEqual({})
  })
})

describe('signedNumber', () => {
  it('signs a gain, keeps a loss and leaves zero bare', () => {
    expect(signedNumber(12)).toBe('+12')
    expect(signedNumber(-35)).toBe('-35')
    expect(signedNumber(0)).toBe('0')
  })
})
