import { describe, expect, it, vi } from 'vitest'
import type { LeagueRunBarView, LeagueRunsView } from '@/bindings'
import { findAll, hasClass, mountFake, textOf } from '@/replay/replayTestHarness'

vi.mock('vue-router', async () => {
  const { RouterLinkStub } = await import('@/lib/testRouterStub')
  return { RouterLink: RouterLinkStub }
})

const { default: LeagueResultsCard } = await import('./LeagueResultsCard.vue')

function bar(fields: Partial<LeagueRunBarView>): LeagueRunBarView {
  return {
    key: 'q-run',
    kind: 'constructed',
    rounds: 5,
    wins: 3,
    losses: 2,
    state: 'complete',
    name: 'Qleague',
    subject: 'Qburn',
    startedAtMs: 0,
    height: 0.6,
    tone: 'winning',
    deckId: 7,
    ...fields,
  }
}

const VIEW: LeagueRunsView = {
  runs: [
    bar({ key: '1', startedAtMs: new Date(2026, 9, 1, 12).getTime() }),
    bar({ key: '2', wins: 5, losses: 0, height: 1, tone: 'trophy' }),
    bar({ key: '3' }),
    bar({ key: '4', startedAtMs: new Date(2026, 9, 27, 12).getTime(), wins: 1, losses: 1, state: 'dropped', height: 0.2, tone: 'dropped' }),
  ],
  trophies: 1,
  avgWins: 3.7,
  axisRounds: 5,
}

function texts(view: LeagueRunsView, cls: string): string[] {
  const mounted = mountFake(LeagueResultsCard, { view, limited: false })
  expect(mounted.warnings).toEqual([])
  return findAll(mounted.root, (node) => hasClass(node, cls)).map((node) => textOf(node).trim())
}

describe('the League results card', () => {
  it('shows the ladder in order with its counts', () => {
    expect(texts(VIEW, 'ladder-label')).toEqual(['5-0', '4-1', '3-2', '2-3', '1-4', '0-5', 'Dropped'])
    expect(texts(VIEW, 'ladder-count')).toEqual(['1', '0', '2', '0', '0', '0', '1'])
    expect(texts(VIEW, 'ladder-heading')).toEqual([])
  })

  it('heads the card and has no run strip', () => {
    const mounted = mountFake(LeagueResultsCard, { view: VIEW, limited: false })
    expect(textOf(mounted.root)).toContain('4 leagues · 1 trophy · 3.7 avg wins')
    expect(textOf(mounted.root)).not.toContain('Oldest')
    expect(findAll(mounted.root, (node) => hasClass(node, 'run-square'))).toEqual([])
  })

  it('names the ladders when the kinds are mixed', () => {
    const mixed = { ...VIEW, runs: [...VIEW.runs, bar({ key: '5', kind: 'draft', rounds: 3, wins: 2, losses: 1 })] }
    expect(texts(mixed, 'ladder-heading')).toEqual(['Constructed', 'Draft'])
  })
})
