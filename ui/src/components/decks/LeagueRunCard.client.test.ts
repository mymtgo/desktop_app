import { describe, expect, it, vi } from 'vitest'
import type { PanelRunView } from '@/bindings'
import { findAll, mountFake, textOf } from '@/replay/replayTestHarness'

const stub = vi.hoisted(() => async (tag: string) => {
  const { defineComponent, h } = await import('vue')
  return { default: defineComponent({ inheritAttrs: false, setup: () => () => h(tag) }) }
})

vi.mock('vue-router', async () => {
  const { RouterLinkStub } = await import('@/lib/testRouterStub')
  return { RouterLink: RouterLinkStub }
})
vi.mock('@/components/mana/ManaColors.vue', () => stub('mana-colors'))
vi.mock('@/components/leagues/LeagueResultBadge.vue', () => stub('result-badge'))
vi.mock('@/components/leagues/ResultDots.vue', () => stub('result-dots'))
vi.mock('@/components/leagues/RunSummaryStats.vue', () => stub('run-summary-stats'))
vi.mock('@/components/screenshots/LeagueScreenshot.vue', () => stub('league-screenshot'))
vi.mock('@/composables/useOffscreenScreenshot', async () => {
  const { ref } = await import('vue')
  return {
    useOffscreenScreenshot: () => ({
      shot: ref(null),
      busy: ref(false),
      saveOffered: ref(false),
      saving: ref(false),
      target: ref(null),
      take: async () => {},
      save: () => {},
    }),
  }
})

const { default: LeagueRunCard } = await import('./LeagueRunCard.vue')

const PANEL = {
  run: {
    state: 'complete',
    liveRound: null,
    results: [],
    startedAtMs: 0,
    tixDelta: null,
    gameWins: 4,
    gameLosses: 2,
    onPlayRecord: null,
    onDrawRecord: null,
    classification: 'FINISH',
    name: 'Qleague',
    format: 'Modern',
    manual: false,
    limited: false,
    colors: null,
    versionLabel: null,
    avgMatchSeconds: null,
    timeOfDay: null,
    deck: { id: 3, name: 'Qdeck', colors: [] },
    matches: [{ token: 'q-1', result: 'W', opponentName: 'Qplayer', games: [] }],
  },
  inProgress: null,
} as unknown as PanelRunView

describe('the league run card', () => {
  it('shows the header, the stats and the matches by default', () => {
    const mounted = mountFake(LeagueRunCard, { panel: PANEL, now: 1000 })
    expect(mounted.warnings).toEqual([])
    const tags = findAll(mounted.root, () => true).map((node) => node.tag)
    expect(tags).toContain('run-summary-stats')
    expect(textOf(mounted.root)).toContain('Qplayer')
  })

  it('heads a finished run as the Leagues page, labelled the latest league, with its tix', () => {
    const panel = { ...PANEL, run: { ...PANEL.run, tixDelta: 12.7 } } as PanelRunView
    const mounted = mountFake(LeagueRunCard, { panel, now: 1000 })
    const text = textOf(mounted.root)
    expect(text).toContain('Qdeck')
    expect(text).toContain('Latest league')
    expect(text).toContain('+13 tix')
    expect(findAll(mounted.root, (node) => node.tag === 'a').map((link) => link.props.href)).toEqual(['/decks/3'])
    expect(findAll(mounted.root, (node) => node.props['aria-label'] === 'Copy screenshot of league')).toHaveLength(1)
  })

  it('heads a run in progress with its rounds left and wins for prize', () => {
    const panel = {
      run: { ...PANEL.run, state: 'active', classification: 'LIVE', liveRound: 3, results: ['W', 'L', null, null, null] },
      inProgress: {
        wins: 1,
        losses: 1,
        winProbability: 55,
        oddsSource: 'deck',
        odds: { roundsLeft: 3, prizeWins: 4, winsForPrize: 2, chanceOfPrize: 40, expectedTix: 1.5 },
      },
    } as unknown as PanelRunView
    const mounted = mountFake(LeagueRunCard, { panel, now: 1000 })
    expect(mounted.warnings).toEqual([])
    const text = textOf(mounted.root)
    expect(text).toContain('League in progress')
    expect(text).toContain('3 rounds left')
    expect(text).toContain('2 wins')
    expect(text).toContain('for prize')
  })
})
