import { afterEach, describe, expect, it, vi } from 'vitest'
import { hasFeature } from '@/lib/platformFeatures'
import { findAll, mountFake } from '@/replay/replayTestHarness'

const state = vi.hoisted(() => ({
  platform: 'arena' as 'mtgo' | 'arena',
  rankAsked: [] as boolean[],
  panelsAsked: [] as boolean[],
}))
const stub = vi.hoisted(() => async (tag: string) => {
  const { defineComponent, h } = await import('vue')
  return { default: defineComponent({ inheritAttrs: false, setup: () => () => h(tag) }) }
})

vi.mock('@/composables/useDeckPage', async () => {
  const { ref } = await import('vue')
  return {
    useDeckPage: () => ({ deckId: ref(3), version: ref(null), timeframe: ref('alltime'), setTimeframe: () => {} }),
  }
})
vi.mock('@/composables/useDeckBestOf', async () => {
  const { ref } = await import('vue')
  return { useDeckBestOfContext: () => ({ bestOf: ref(null), bo1Mode: ref(false) }) }
})
vi.mock('@/composables/usePlatformContext', () => ({
  usePlatformContext: () => ({ hasFeature: (feature: Parameters<typeof hasFeature>[1]) => hasFeature(state.platform, feature) }),
}))
vi.mock('@/lib/queries', async () => {
  const { ref } = await import('vue')
  const dashboard = {
    record: {},
    delta: null,
    sparkline: [],
    boarding: {},
    playDraw: {},
    matchupSpread: [],
    daily: [],
    peerChart: null,
    standout: null,
    games: { won: 0, lost: 0 },
  }
  return {
    useDeckDashboard: () => ({ data: ref(dashboard), error: ref(null) }),
    useDeckLeaguePanels: (_deck: unknown, _version: unknown, enabled: { value: boolean }) => {
      state.panelsAsked.push(enabled.value)
      return { data: ref({ finishes: [], current: null }), dataUpdatedAt: ref(0) }
    },
    useDeckRank: (_deck: unknown, enabled: { value: boolean }) => {
      state.rankAsked.push(enabled.value)
      return { data: ref(null), isPending: ref(false), error: ref(null) }
    },
  }
})
vi.mock('@/components/decks/TimeframeFilter.vue', () => stub('timeframe-filter'))
vi.mock('@/components/decks/KpiMatchWinRate.vue', () => stub('kpi-win-rate'))
vi.mock('@/components/decks/KpiBoardingSplit.vue', () => stub('kpi-boarding'))
vi.mock('@/components/decks/KpiPlayDrawGap.vue', () => stub('kpi-play-draw'))
vi.mock('@/components/decks/KpiBestWorstMatchups.vue', () => stub('kpi-matchups'))
vi.mock('@/components/decks/MatchHistoryChart.vue', () => stub('match-history'))
vi.mock('@/components/decks/StandoutCards.vue', () => stub('standout-cards'))
vi.mock('@/components/decks/LeagueFinishes.vue', () => stub('league-finishes'))
vi.mock('@/components/decks/LeagueRunCard.vue', () => stub('league-run-card'))
vi.mock('@/components/rank/DeckRankCard.vue', () => stub('deck-rank-card'))

const { default: DashboardPage } = await import('./DashboardPage.vue')

const open: { unmount: () => void }[] = []
afterEach(() => {
  open.splice(0).forEach((mounted) => mounted.unmount())
  state.rankAsked = []
  state.panelsAsked = []
})

async function mounted() {
  const page = mountFake(DashboardPage)
  open.push(page)
  await page.settle()
  expect(page.warnings).toEqual([])
  return page
}

const tags = (root: Parameters<typeof findAll>[0], tag: string) => findAll(root, (node) => node.tag === tag)

describe('the deck dashboard rank branch', () => {
  it('an Arena deck shows its rank card and reads no leagues', async () => {
    state.platform = 'arena'
    const page = await mounted()
    expect(tags(page.root, 'deck-rank-card')).toHaveLength(1)
    expect(tags(page.root, 'league-finishes')).toEqual([])
    expect(state.rankAsked).toEqual([true])
    expect(state.panelsAsked).toEqual([false])
  })

  it('an MTGO deck shows its leagues and reads no rank', async () => {
    state.platform = 'mtgo'
    const page = await mounted()
    expect(tags(page.root, 'deck-rank-card')).toEqual([])
    expect(tags(page.root, 'league-finishes')).toHaveLength(1)
    expect(state.rankAsked).toEqual([false])
    expect(state.panelsAsked).toEqual([true])
  })
})
