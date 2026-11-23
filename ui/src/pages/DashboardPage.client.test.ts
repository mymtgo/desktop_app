import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { Ref } from 'vue'
import type {
  BreakdownView,
  HomeDashboardView,
  LastSessionView,
  LeagueRunsView,
  MostPickedView,
  RankLineView,
  ResultsView,
  RunCardView,
} from '@/bindings'
import { findAll, mountFake, textOf } from '@/replay/replayTestHarness'

const state = vi.hoisted(() => ({
  route: { query: {} as Record<string, unknown> },
  pushed: [] as unknown[],
  replaced: [] as unknown[],
  asked: { format: null as Ref<string> | null },
  view: null as unknown,
  placeholder: false,
  platform: 'mtgo' as 'mtgo' | 'arena',
  firstRun: null as unknown,
}))
const stub = vi.hoisted(() => async (tag: string) => {
  const { defineComponent, h } = await import('vue')
  return { default: defineComponent({ inheritAttrs: false, setup: () => () => h(tag) }) }
})

vi.mock('vue-router', async () => {
  const { RouterLinkStub } = await import('@/lib/testRouterStub')
  return {
    RouterLink: RouterLinkStub,
    useRoute: () => state.route,
    useRouter: () => ({
      push: (to: unknown) => {
        state.pushed.push(to)
        return Promise.resolve()
      },
      replace: (to: unknown) => {
        state.replaced.push(to)
        return Promise.resolve()
      },
    }),
  }
})
vi.mock('@/lib/queries', async () => {
  const { ref } = await import('vue')
  return {
    useHomeDashboard: (_timeframe: Ref<string>, format: Ref<string>) => {
      state.asked.format = format
      return {
        data: ref(state.view),
        dataUpdatedAt: ref(0),
        error: ref(null),
        isPlaceholderData: ref(state.placeholder),
      }
    },
    useStatus: () => ({ data: ref(state.firstRun ? {} : undefined) }),
    useLegacyImport: () => ({ data: ref(undefined) }),
  }
})
vi.mock('@/composables/usePlatformContext', async () => {
  const { ref } = await import('vue')
  return { usePlatformContext: () => ({ platform: ref(state.platform) }) }
})
vi.mock('@/lib/status', () => ({ firstRunState: () => state.firstRun }))
vi.mock('@/lib/legacyImport', () => ({ offersImportHint: () => false }))
vi.mock('@/components/decks/TimeframeFilter.vue', () => stub('timeframe-filter'))
vi.mock('@/components/FirstRunState.vue', () => stub('first-run'))
vi.mock('@/components/dashboard/ResultsCard.vue', () => stub('results-card'))
vi.mock('@/components/dashboard/LeagueResultsCard.vue', () => stub('league-results-card'))
vi.mock('@/components/dashboard/TopMatchupsCard.vue', () => stub('top-matchups-card'))
vi.mock('@/components/dashboard/YourColoursCard.vue', () => stub('your-colours-card'))
vi.mock('@/components/dashboard/RecentMatchesCard.vue', () => stub('recent-matches-card'))
vi.mock('@/components/dashboard/LeagueCard.vue', () => stub('league-card'))
vi.mock('@/components/dashboard/RecentCard.vue', () => stub('recent-card'))
vi.mock('@/components/dashboard/MostPickedCard.vue', () => stub('most-picked-card'))
vi.mock('@/components/dashboard/LastSessionCard.vue', () => stub('last-session-card'))
vi.mock('@/components/dashboard/BreakdownCard.vue', () => stub('breakdown-card'))
vi.mock('@/components/rank/RankCard.vue', () => stub('rank-card'))

const { default: DashboardPage } = await import('./DashboardPage.vue')

function view(fields: Partial<HomeDashboardView>): HomeDashboardView {
  return {
    hasMatches: true,
    format: 'all',
    formats: [{ key: 'modern', label: 'Modern' }],
    results: {} as ResultsView,
    leagueResults: null,
    matchups: null,
    yourColours: null,
    recentMatches: null,
    league: null,
    recent: null,
    mostPicked: null,
    lastSession: null,
    breakdown: null,
    rank: null,
    ...fields,
  }
}

function tags(root: Parameters<typeof findAll>[0]): string[] {
  return findAll(root, (node) => node.tag.endsWith('-card')).map((node) => node.tag)
}

beforeEach(() => {
  state.route.query = {}
  state.pushed = []
  state.replaced = []
  state.placeholder = false
  state.platform = 'mtgo'
  state.firstRun = null
})

describe('the home dashboard page', () => {
  it('the arena format switch has no Limited', () => {
    state.platform = 'arena'
    state.view = view({
      format: 'constructed',
      formats: [{ key: 'historic', label: 'Historic' }, { key: 'standard', label: 'Standard' }],
    })
    const mounted = mountFake(DashboardPage)
    const buttons = findAll(mounted.root, (node) => node.tag === 'button')
    expect(buttons.map((node) => textOf(node).trim())).toEqual(['All formats', 'Historic', 'Standard'])
  })

  it('hides the arena format switch when one format was played', () => {
    state.platform = 'arena'
    state.view = view({ format: 'constructed', formats: [{ key: 'historic', label: 'Historic' }] })
    const mounted = mountFake(DashboardPage)
    expect(findAll(mounted.root, (node) => node.tag === 'button')).toHaveLength(0)
  })

  it('keeps the mtgo format switch with one format', () => {
    state.view = view({ format: 'all', formats: [{ key: 'modern', label: 'Modern' }] })
    const mounted = mountFake(DashboardPage)
    expect(findAll(mounted.root, (node) => node.tag === 'button').length).toBeGreaterThan(0)
  })

  it('arena_only_users_get_no_first_run_screen', () => {
    state.firstRun = 'no-mtgo-data'
    state.view = view({ hasMatches: false })
    state.platform = 'arena'
    const arena = mountFake(DashboardPage)
    expect(findAll(arena.root, (node) => node.tag === 'first-run')).toHaveLength(0)
    state.platform = 'mtgo'
    const mtgo = mountFake(DashboardPage)
    expect(findAll(mtgo.root, (node) => node.tag === 'first-run')).toHaveLength(1)
  })

  it('asks for the URL format, else empty (the format last played)', () => {
    state.route.query = { format: 'modern' }
    state.view = view({ format: 'modern' })
    mountFake(DashboardPage)
    expect(state.asked.format?.value).toBe('modern')

    state.route.query = {}
    state.view = view({ format: 'limited' })
    mountFake(DashboardPage)
    expect(state.asked.format?.value).toBe('')
  })

  it('has no All option and shows the resolved format as selected', () => {
    state.view = view({ format: 'limited', formats: [{ key: 'modern', label: 'Modern' }] })
    const mounted = mountFake(DashboardPage)
    expect(mounted.warnings).toEqual([])
    const buttons = findAll(mounted.root, (node) => node.tag === 'button')
    expect(buttons.map((node) => textOf(node).trim())).toEqual(['Constructed', 'Limited', 'Modern'])
    const pressed = buttons.filter((node) => node.props['aria-pressed'] === true).map((node) => textOf(node).trim())
    expect(pressed).toEqual(['Limited'])
  })

  it('writes a chosen format to the URL', () => {
    state.view = view({ format: 'constructed' })
    state.route.query = { timeframe: 'week' }
    const mounted = mountFake(DashboardPage)
    expect(mounted.warnings).toEqual([])
    const limited = findAll(mounted.root, (node) => node.tag === 'button' && textOf(node).trim() === 'Limited')[0]
    ;(limited.props.onClick as () => void)()
    expect(state.pushed).toEqual([{ query: { timeframe: 'week', format: 'limited' } }])
  })

  it('shows the asked format in the switch while the previous page is on screen', () => {
    state.route.query = { format: 'limited' }
    state.view = view({ format: 'constructed' })
    const pressed = (label: string) => {
      const mounted = mountFake(DashboardPage)
      const button = findAll(mounted.root, (node) => node.tag === 'button' && textOf(node).trim() === label)[0]
      return button.props['aria-pressed']
    }
    state.placeholder = true
    expect(pressed('Limited')).toBe(true)
    expect(pressed('Constructed')).toBe(false)
    state.placeholder = false
    expect(pressed('Limited')).toBe(false)
    expect(pressed('Constructed')).toBe(true)
  })

  it('shows the constructed cards in All mode and hides the empty ones', () => {
    state.view = view({
      leagueResults: {} as LeagueRunsView,
      matchups: [],
      recentMatches: [],
      league: {} as RunCardView,
      recent: [],
      lastSession: {} as LastSessionView,
      breakdown: {} as BreakdownView,
    })
    expect(tags(mountFake(DashboardPage).root)).toEqual([
      'results-card',
      'league-results-card',
      'top-matchups-card',
      'recent-matches-card',
      'league-card',
      'recent-card',
      'last-session-card',
      'breakdown-card',
    ])
    state.view = view({})
    expect(tags(mountFake(DashboardPage).root)).toEqual(['results-card'])
  })

  it('swaps in the limited cards in Limited mode', () => {
    state.route.query = { format: 'limited' }
    state.view = view({
      format: 'limited',
      leagueResults: {} as LeagueRunsView,
      yourColours: [],
      recentMatches: [],
      league: {} as RunCardView,
      recent: [],
      mostPicked: {} as MostPickedView,
      lastSession: {} as LastSessionView,
      breakdown: {} as BreakdownView,
    })
    expect(tags(mountFake(DashboardPage).root)).toEqual([
      'results-card',
      'league-results-card',
      'your-colours-card',
      'recent-matches-card',
      'league-card',
      'recent-card',
      'most-picked-card',
      'last-session-card',
      'breakdown-card',
    ])
  })

  it('the arena dashboard shows two rank cards and no league cards', () => {
    state.platform = 'arena'
    const constructed = { ladder: 'constructed' } as RankLineView
    const limited = { ladder: 'limited' } as RankLineView
    state.view = view({
      format: 'constructed',
      leagueResults: {} as LeagueRunsView,
      league: {} as RunCardView,
      rank: [constructed, limited],
    })
    expect(tags(mountFake(DashboardPage).root)).toEqual(['results-card', 'rank-card', 'rank-card'])
  })

  it('the mtgo dashboard shows no rank card', () => {
    state.view = view({ rank: [{ ladder: 'constructed' } as RankLineView], league: {} as RunCardView })
    expect(tags(mountFake(DashboardPage).root)).toEqual(['results-card', 'league-card'])
  })
})
