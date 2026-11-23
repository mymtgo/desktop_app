import { describe, expect, it, vi } from 'vitest'
import type { MatchDetail } from '@/bindings'
import { findAll, mountFake, textOf } from '@/replay/replayTestHarness'

const stub = vi.hoisted(() => async (tag: string) => {
  const { defineComponent, h } = await import('vue')
  return { default: defineComponent({ inheritAttrs: false, setup: () => () => h(tag) }) }
})

vi.mock('vue-router', () => ({ useRouter: () => ({ push: async () => {}, back: () => {} }) }))
vi.mock('@/components/matches/ManualMatchSheet.vue', () => stub('manual-sheet'))
vi.mock('@/components/matches/MatchGame.vue', () => stub('match-game'))
vi.mock('@/components/matches/MatchHero.vue', () => stub('match-hero'))
vi.mock('@/components/matches/MatchNotes.vue', () => stub('match-notes'))
vi.mock('@/components/matches/OpponentRevealsRail.vue', () => stub('reveals-rail'))
vi.mock('@/components/matches/OpponentCommanders.vue', () => stub('opponent-commanders'))

const { default: MatchDetailBody } = await import('./MatchDetailBody.vue')

const NOTE = 'Match detail is not available for MTG Arena matches yet.'
const ENDSTEP_NOTE = 'Match detail is not available for Endstep matches yet.'

type Platform = 'mtgo' | 'arena' | 'endstep'

function match(platform: Platform, result = 'win', opponentCommanders: unknown[] = []): MatchDetail {
  return {
    opponentCommanders,
    id: 1,
    token: 'q-1',
    platform,
    source: 'log',
    opponent: 'Opponent',
    manual: false,
    imported: false,
    manualEditing: null,
    notes: null,
    games: [
      { number: 1, result, onThePlay: null, startedAtMs: 1000, endedAtMs: 61000, turns: null },
    ],
  } as unknown as MatchDetail
}

function mountBody(platform: Platform, result = 'win', opponentCommanders: unknown[] = []) {
  const mounted = mountFake(MatchDetailBody, {
    match: match(platform, result, opponentCommanders),
    fallbackPath: '/matches',
  })
  const tags = (tag: string) => findAll(mounted.root, (node) => node.tag === tag).length
  return { mounted, tags }
}

describe('the match body', () => {
  it('an arena match shows its detail panels', () => {
    const { mounted, tags } = mountBody('arena')
    expect(textOf(mounted.root)).not.toContain(NOTE)
    expect(tags('match-game')).toBe(1)
    expect(tags('reveals-rail')).toBe(1)
    expect(tags('match-hero')).toBe(1)
    expect(tags('match-notes')).toBe(1)
  })

  it('an MTGO match shows its detail panels', () => {
    const { mounted, tags } = mountBody('mtgo')
    expect(textOf(mounted.root)).not.toContain(NOTE)
    expect(tags('match-game')).toBe(1)
    expect(tags('reveals-rail')).toBe(1)
  })

  it('an Endstep match shows its detail panels', () => {
    const { mounted, tags } = mountBody('endstep')
    expect(textOf(mounted.root)).not.toContain(ENDSTEP_NOTE)
    expect(tags('match-game')).toBe(1)
    expect(tags('reveals-rail')).toBe(1)
  })

  it('a game with no known result is tiled Unknown, never Loss', () => {
    const { mounted } = mountBody('endstep', 'unknown')
    const text = textOf(mounted.root)
    expect(text).toContain('Unknown')
    expect(text).not.toContain('Loss')
  })

  it('shows the opponent commander above the reveals for a Brawl match only', () => {
    const brawl = mountBody('arena', 'win', [{ catalogId: 7, name: 'Qomnath' }])
    expect(brawl.tags('opponent-commanders')).toBe(1)
    const order = findAll(brawl.mounted.root, (node) => node.tag === 'opponent-commanders' || node.tag === 'reveals-rail')
    expect(order.map((node) => node.tag)).toEqual(['opponent-commanders', 'reveals-rail'])
    expect(mountBody('arena').tags('opponent-commanders')).toBe(0)
  })
})
