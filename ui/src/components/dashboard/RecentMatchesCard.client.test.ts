import { describe, expect, it, vi } from 'vitest'
import type { HomeRecentMatchView, MatchSummary } from '@/bindings'
import { mountFake, textOf } from '@/replay/replayTestHarness'

vi.mock('vue-router', async () => {
  const { RouterLinkStub } = await import('@/lib/testRouterStub')
  return { RouterLink: RouterLinkStub }
})

const { default: RecentMatchesCard } = await import('./RecentMatchesCard.vue')

const LATER = 1_800_000_000_000

function row(startedAtMs: number): HomeRecentMatchView {
  const match = { token: 'q-1', outcome: 'win', opponentArchetype: null, startedAtMs } as unknown as MatchSummary
  return { match, deckId: null, deckName: 'Qdeck', runKey: null, setCode: null, colors: null }
}

describe('the Recent matches card', () => {
  it('reads times against the now it is given, never "from now"', () => {
    // Started after the page first loaded; the refetch moved now past it.
    const mounted = mountFake(RecentMatchesCard, { matches: [row(LATER - 60_000)], now: LATER })
    expect(mounted.warnings).toEqual([])
    expect(textOf(mounted.root)).not.toContain('from now')
  })
})
