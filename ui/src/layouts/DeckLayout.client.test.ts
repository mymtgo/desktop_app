import { afterEach, describe, expect, it, vi } from 'vitest'
import type { DeckBestOf } from '@/composables/useDeckBestOf'
import { mountFake } from '@/replay/replayTestHarness'

const state = vi.hoisted(() => ({
  route: { name: 'deck-dashboard', query: {} as Record<string, string> },
  replace: undefined as unknown as (to: unknown) => void,
  context: null as DeckBestOf | null,
  kinds: null as unknown,
  storage: null as null | { getItem: (key: string) => string | null; setItem: (key: string, value: string) => void },
  headerData: null as null | { value: unknown },
  kindsAsked: [] as unknown[],
  platform: 'arena' as string,
}))

vi.mock('vue-router', async () => {
  const { RouterLinkStub, RouterViewStub } = await import('@/lib/testRouterStub')
  const { reactive } = await import('vue')
  state.route = reactive(state.route)
  return {
    RouterLink: RouterLinkStub,
    RouterView: RouterViewStub,
    useRoute: () => state.route,
    useRouter: () => ({ replace: (to: unknown) => state.replace(to), push: () => {} }),
  }
})
vi.mock('@/composables/useDeckBestOf', async (importOriginal) => {
  const real = await importOriginal<typeof import('@/composables/useDeckBestOf')>()
  return {
    ...real,
    storedBestOf: (deckId: number) => real.storedBestOf(deckId, state.storage),
    useDeckBestOf: (...args: Parameters<typeof real.useDeckBestOf>) => real.useDeckBestOf(args[0], args[1], state.storage),
  }
})
vi.mock('@/composables/useDeckPage', async () => {
  const { ref, watch } = await import('vue')
  return {
    useDeckPage: () => ({
      deckId: ref(3),
      version: ref(null),
      timeframe: ref('alltime'),
      setVersion: () => {},
    }),
    useCurrentDeckHeader: (bestOf: { value: unknown }) => {
      watch(bestOf, (kind) => state.kindsAsked.push(kind), { immediate: true, flush: 'sync' })
      const data = ref(
        state.headerData === null
          ? { deck: { id: 3, name: 'Qdeck', archived: false, archetype: 'x' }, kinds: state.kinds, platform: 'arena' }
          : undefined,
      )
      if (state.headerData !== null) state.headerData = data
      return { data, error: ref(null) }
    },
  }
})
vi.mock('@/composables/useWindowTitle', () => ({ usePageDetail: () => {} }))
vi.mock('@/composables/usePlatformContext', async () => {
  const { computed } = await import('vue')
  return { usePlatformContext: () => ({ context: computed(() => ({ current: state.platform })) }) }
})
vi.mock('@/components/decks/DeckSidebar.vue', async () => {
  const { defineComponent, h } = await import('vue')
  const { useDeckBestOfContext } = await import('@/composables/useDeckBestOf')
  return {
    default: defineComponent({
      setup() {
        state.context = useDeckBestOfContext()
        return () => h('sidebar')
      },
    }),
  }
})

const { default: DeckLayout } = await import('./DeckLayout.vue')

const open: { unmount: () => void }[] = []
afterEach(() => open.splice(0).forEach((mounted) => mounted.unmount()))

afterEach(() => {
  state.platform = 'arena'
  state.storage = null
  state.headerData = null
  state.kindsAsked = []
})

function mountOn(name: string, kinds: unknown) {
  const calls: unknown[] = []
  state.replace = (to) => calls.push(to)
  state.route.name = name
  state.route.query = { version: '2' }
  state.kinds = kinds
  const mounted = mountFake(DeckLayout)
  open.push(mounted)
  return { mounted, calls }
}

const both = { bo1: 3, bo3: 3, latest: 'bo3' }
const dashboard = { name: 'deck-dashboard', params: { id: 3 }, query: { version: '2' } }

describe('the deck layout in Bo1 mode', () => {
  it.each(['deck-game-stats', 'deck-sideboard-guides', 'deck-sideboard-guide'])(
    'sends %s to the dashboard when the deck has only Bo1 matches',
    async (name) => {
      const { mounted, calls } = mountOn(name, { bo1: 3, bo3: 0, latest: 'bo1' })
      await mounted.settle()
      expect(mounted.warnings).toEqual([])
      expect(calls).toEqual([dashboard])
    },
  )

  it('sends a Game Stats page to the dashboard when the switch flips to Bo1', async () => {
    const { mounted, calls } = mountOn('deck-game-stats', both)
    await mounted.settle()
    expect(calls).toEqual([])
    state.context?.choose('bo1')
    await mounted.settle()
    expect(calls).toEqual([dashboard])
  })

  it('leaves other pages and Bo3 alone', async () => {
    const bo1 = mountOn('deck-matches', { bo1: 3, bo3: 0, latest: 'bo1' })
    await bo1.mounted.settle()
    expect(bo1.calls).toEqual([])
    bo1.mounted.unmount()
    open.pop()
    const bo3 = mountOn('deck-game-stats', both)
    await bo3.mounted.settle()
    expect(bo3.calls).toEqual([])
  })

  it('does not touch an MTGO deck', async () => {
    const { mounted, calls } = mountOn('deck-game-stats', null)
    await mounted.settle()
    expect(calls).toEqual([])
  })
})

describe('the deck layout header', () => {
  it('reads the header once for the remembered kind', async () => {
    const data: Record<string, string> = { 'deck-best-of:3': 'bo3' }
    state.storage = { getItem: (key) => data[key] ?? null, setItem: (key, value) => (data[key] = value) }
    state.headerData = { value: undefined }
    const { mounted } = mountOn('deck-dashboard', null)
    await mounted.settle()
    expect(state.kindsAsked).toEqual(['bo3'])
    state.headerData.value = {
      deck: { id: 3, name: 'Qdeck', archived: false, archetype: 'x' },
      kinds: both,
      platform: 'arena',
    }
    await mounted.settle()
    expect(state.kindsAsked).toEqual(['bo3'])
    expect(state.context?.bestOf.value).toBe('bo3')
  })
})

describe('the deck layout platform', () => {
  it('sends a deck of the other platform to the deck list', async () => {
    state.platform = 'mtgo'
    const { mounted, calls } = mountOn('deck-dashboard', null)
    await mounted.settle()
    expect(calls).toEqual([{ name: 'decks' }])
  })

  it('keeps a deck of the current platform', async () => {
    const { mounted, calls } = mountOn('deck-dashboard', null)
    await mounted.settle()
    expect(calls).toEqual([])
  })
})
