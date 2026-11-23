import { beforeEach, describe, expect, it, vi } from 'vitest'
import { findAll, mountFake } from '@/replay/replayTestHarness'

const state = vi.hoisted(() => ({
  platform: 'mtgo' as 'mtgo' | 'arena',
  firstRun: 'no-mtgo-data' as unknown,
}))
const stub = vi.hoisted(() => async (tag: string) => {
  const { defineComponent, h } = await import('vue')
  return { default: defineComponent({ inheritAttrs: false, setup: () => () => h(tag) }) }
})

vi.mock('@/composables/useUrlMatchList', async () => {
  const { ref } = await import('vue')
  return {
    useUrlMatchList: () => ({
      list: ref({ sortBy: 'date', sortDir: 'desc', page: 1 }),
      query: ref({}),
      show: () => {},
      sort: () => {},
      stepBackFromEmptyPage: () => {},
    }),
  }
})
vi.mock('@/composables/useArchetypeDetection', async () => {
  const { ref } = await import('vue')
  return { useArchetypeDetection: () => ({ detectingTokens: ref([]), detectMatches: () => {} }) }
})
vi.mock('@/composables/usePlatformContext', async () => {
  const { ref } = await import('vue')
  return { usePlatformContext: () => ({ platform: ref(state.platform) }) }
})
vi.mock('@/lib/queries', async () => {
  const { ref } = await import('vue')
  return {
    useMatchesPage: () => ({
      data: ref({ rows: [], total: 0, perPage: 25, page: 1, archetypes: [], unknownArchetypeCount: 0 }),
      isPending: ref(false),
      error: ref(null),
    }),
    useMatchDecks: () => ({ data: ref([]) }),
    useStatus: () => ({ data: ref({}) }),
  }
})
vi.mock('@/lib/matchList', () => ({ isFiltered: () => false }))
vi.mock('@/lib/paging', () => ({ showingText: () => '' }))
vi.mock('@/lib/status', () => ({ firstRunState: () => state.firstRun }))
vi.mock('@/components/FirstRunState.vue', () => stub('first-run'))
vi.mock('@/components/matches/MatchListFilters.vue', () => stub('match-filters'))
vi.mock('@/components/MatchesTable.vue', () => stub('matches-table'))
vi.mock('@/components/PagedNav.vue', () => stub('paged-nav'))

const { default: MatchesPage } = await import('./MatchesPage.vue')

beforeEach(() => {
  state.platform = 'mtgo'
  state.firstRun = 'no-mtgo-data'
})

describe('the matches page first-run screen', () => {
  it('shows only in the mtgo context', () => {
    state.platform = 'arena'
    const arena = mountFake(MatchesPage)
    expect(findAll(arena.root, (node) => node.tag === 'first-run')).toHaveLength(0)
    state.platform = 'mtgo'
    const mtgo = mountFake(MatchesPage)
    expect(findAll(mtgo.root, (node) => node.tag === 'first-run')).toHaveLength(1)
  })
})
