import { describe, expect, it } from 'vitest'
import type { LimitedIndexRowView, RunCardView } from '@/bindings'
import { limitedRowHeader, runCardHeader } from '@/lib/leagueRunHeader'

const DAY = 86_400_000

function card(overrides: Partial<RunCardView> = {}): RunCardView {
  return {
    key: 'q-run:1',
    source: 'log',
    name: 'Qleague',
    format: 'Modern',
    formatCode: 'CMODERN',
    state: 'complete',
    manual: false,
    startedAtMs: 1_000,
    droppedAtMs: null,
    deck: { id: 7, name: 'Qburn', colors: ['R'], coverArt: null, localCoverArt: null },
    versionLabel: 'v2',
    results: ['W', 'W', 'L', 'W', 'W'],
    matches: [],
    notes: null,
    classification: 'CASH',
    liveRound: null,
    avgMatchSeconds: 1500,
    timeOfDay: 'evening',
    gameWins: 8,
    gameLosses: 3,
    onPlayRecord: { wins: 2, losses: 1 },
    onDrawRecord: { wins: 2, losses: 0 },
    tixDelta: 12.7,
    topOpponentArchetype: null,
    topMatchups: [],
    limited: false,
    colors: null,
    userState: null,
    autoState: 'complete',
    ...overrides,
  }
}

function row(overrides: Partial<LimitedIndexRowView> = {}): LimitedIndexRowView {
  return {
    runKey: 'draft:Q-token',
    draftToken: 'Q-token',
    title: 'HOB Draft',
    setCode: 'HOB',
    kind: 'draft',
    state: 'Complete',
    stateVariant: 'success',
    startedAtMs: 1_000,
    wins: 2,
    losses: 1,
    results: ['W', 'L', 'W'],
    picksMade: 42,
    picksExpected: 42,
    packs: null,
    deckRegistered: true,
    versionCount: 2,
    avgPickSeconds: 26,
    opponents: [],
    matches: [],
    gameWins: 4,
    gameLosses: 3,
    onPlayRecord: { wins: 1, losses: 1 },
    onDrawRecord: { wins: 1, losses: 0 },
    note: null,
    linked: true,
    classification: 'FINISH',
    liveRound: null,
    avgMatchSeconds: 1200,
    timeOfDay: 'morning',
    userState: null,
    autoState: 'complete',
    notes: null,
    untrackedMatches: 0,
    ...overrides,
  }
}

describe('runCardHeader', () => {
  it('names a constructed run by its deck, linking to it', () => {
    expect(runCardHeader(card(), 0)).toEqual({
      classification: 'CASH',
      wins: 4,
      losses: 1,
      liveRound: null,
      results: ['W', 'W', 'L', 'W', 'W'],
      name: 'Qburn',
      link: '/decks/7',
      colors: ['R'],
      format: 'Modern',
      manual: false,
      versionLabel: 'v2',
      state: null,
      startedAtMs: 1_000,
      avgMatchSeconds: 1500,
      timeOfDay: 'evening',
      tixDelta: 12.7,
    })
  })

  it('hides the deck identity when asked, and without a deck', () => {
    for (const header of [runCardHeader(card(), 0, true), runCardHeader(card({ deck: null }), 0)]) {
      expect(header.name).toBeNull()
      expect(header.link).toBeNull()
      expect(header.colors).toEqual([])
      expect(header.format).toBeNull()
    }
  })

  it('names a limited run by its event, linking to its draft (sealed: its deck)', () => {
    const limited = card({ limited: true, deck: null, name: 'QNW Draft', format: 'Draft', colors: ['U', 'R'], key: 'q:2' })
    expect(runCardHeader(limited, 0, true)).toMatchObject({
      name: 'QNW Draft',
      link: '/limited/q%3A2/draft',
      colors: ['U', 'R'],
      format: 'Draft',
    })
    expect(runCardHeader({ ...limited, format: 'Sealed', colors: null }, 0)).toMatchObject({
      link: '/limited/q%3A2/deck',
      colors: [],
    })
  })

  it('badges only a dropped run, saying when it was dropped', () => {
    expect(runCardHeader(card({ state: 'active' }), 0).state).toBeNull()
    expect(runCardHeader(card({ state: 'partial' }), 0).state).toBeNull()
    expect(runCardHeader(card({ state: 'dropped', droppedAtMs: 0 }), 2 * DAY).state).toEqual({
      label: 'Dropped',
      tone: 'destructive',
      title: 'Dropped 2 days ago',
    })
  })
})

describe('limitedRowHeader', () => {
  it('names the event by its title and kind, linking to its draft', () => {
    expect(limitedRowHeader(row())).toEqual({
      classification: 'FINISH',
      wins: 2,
      losses: 1,
      liveRound: null,
      results: ['W', 'L', 'W'],
      name: 'HOB Draft',
      link: '/limited/draft%3AQ-token/draft',
      colors: [],
      format: 'Draft',
      manual: false,
      versionLabel: null,
      state: null,
      startedAtMs: 1_000,
      avgMatchSeconds: 1200,
      timeOfDay: 'morning',
      tixDelta: null,
    })
  })

  it('links a sealed event to its deck, and an unlinked draft nowhere', () => {
    expect(limitedRowHeader(row({ kind: 'sealed' }))).toMatchObject({ link: '/limited/draft%3AQ-token/deck', format: 'Sealed' })
    expect(limitedRowHeader(row({ runKey: null, linked: false })).link).toBeNull()
  })

  it('badges every state but a run in progress or completed', () => {
    const state = (label: string, variant: string) => limitedRowHeader(row({ state: label, stateVariant: variant })).state
    expect(state('Active', 'default')).toBeNull()
    expect(state('Complete', 'success')).toBeNull()
    expect(state('Dropped', 'destructive')).toEqual({ label: 'Dropped', tone: 'destructive', title: null })
    expect(state('No matches', 'warning')).toEqual({ label: 'No matches', tone: 'warning', title: null })
    expect(state('Draft abandoned', 'warning')).toEqual({ label: 'Draft abandoned', tone: 'warning', title: null })
    expect(state('Unlinked', 'warning')).toEqual({ label: 'Unlinked', tone: 'warning', title: null })
  })
})
