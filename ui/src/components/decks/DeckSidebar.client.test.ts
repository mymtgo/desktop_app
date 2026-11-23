import { describe, expect, it, vi } from 'vitest'
import { computed } from 'vue'
import type { DeckHeaderView } from '@/bindings'
import { findAll, mountFake, textOf } from '@/replay/replayTestHarness'

const stub = vi.hoisted(() => async (tag: string) => {
  const { defineComponent, h } = await import('vue')
  return { default: defineComponent({ inheritAttrs: false, setup: () => () => h(tag) }) }
})

const context = vi.hoisted(() => ({
  platform: 'mtgo' as 'mtgo' | 'arena',
  bestOf: null as 'bo1' | 'bo3' | null,
  showSwitch: false,
  choose: undefined as unknown as (kind: string) => void,
}))

vi.mock('vue-router', async () => {
  const { RouterLinkStub } = await import('@/lib/testRouterStub')
  return { RouterLink: RouterLinkStub }
})
vi.mock('@/components/CardImage.vue', () => stub('card-image'))
vi.mock('@/components/mana/ManaColors.vue', () => stub('mana-colors'))
vi.mock('@/components/decks/EditableDeckName.vue', () => stub('deck-name'))
vi.mock('@/components/ui/select', async () => {
  const { defineComponent, h } = await import('vue')
  const passthrough = defineComponent({ setup: (_, { slots }) => () => h('div', slots.default?.()) })
  return { Select: passthrough, SelectContent: passthrough, SelectItem: passthrough, SelectTrigger: passthrough, SelectValue: passthrough }
})
vi.mock('@/composables/usePlatformContext', () => ({
  usePlatformContext: () => ({ hasFeature: () => true }),
}))
vi.mock('@/composables/useDeckBestOf', () => ({
  useDeckBestOfContext: () => ({
    bestOf: computed(() => context.bestOf),
    showSwitch: computed(() => context.showSwitch),
    bo1Mode: computed(() => context.bestOf === 'bo1'),
    choose: (kind: string) => context.choose(kind),
  }),
}))

const { default: DeckSidebar } = await import('./DeckSidebar.vue')

function header(platform: 'mtgo' | 'arena', trophies: number | null): DeckHeaderView {
  return {
    platform,
    trophies,
    kinds: platform === 'arena' ? { bo1: 2, bo3: 2, latest: 'bo1' } : null,
    versions: [],
    currentVersionId: null,
    deck: { id: 3, name: 'Qdeck', originalName: 'Qdeck', archived: false, colors: [], coverArt: null, formatLabel: 'Modern' },
  } as unknown as DeckHeaderView
}

function render(deck: DeckHeaderView) {
  const mounted = mountFake(DeckSidebar, { header: deck, version: null, timeframe: 'alltime', currentPage: 'deck-dashboard' })
  expect(mounted.warnings).toEqual([])
  return mounted
}

describe('the deck sidebar', () => {
  it('keeps the trophy pill and every item for an MTGO deck', () => {
    context.platform = 'mtgo'
    context.bestOf = null
    context.showSwitch = false
    const mounted = render(header('mtgo', 5))
    const text = textOf(mounted.root)
    expect(findAll(mounted.root, (node) => node.props['aria-label'] === 'Trophies')).toHaveLength(1)
    expect(findAll(mounted.root, (node) => node.tag === 'button')).toHaveLength(0)
    expect(text).toContain('Game Stats')
    expect(text).toContain('Sideboard Guides')
  })

  it('shows the switch instead of a trophy pill on an Arena deck with both kinds', () => {
    context.bestOf = 'bo3'
    context.showSwitch = true
    const chosen: string[] = []
    context.choose = (kind) => chosen.push(kind)
    const mounted = render(header('arena', 0))
    expect(findAll(mounted.root, (node) => node.props['aria-label'] === 'Trophies')).toHaveLength(0)
    const buttons = findAll(mounted.root, (node) => node.tag === 'button')
    expect(buttons.map((node) => textOf(node).trim())).toEqual(['Bo3', 'Bo1'])
    ;(buttons[1].props.onClick as () => void)()
    expect(chosen).toEqual(['bo1'])
    expect(textOf(mounted.root)).toContain('Game Stats')
  })

  it('shows no switch on an Arena deck with one kind', () => {
    context.bestOf = 'bo3'
    context.showSwitch = false
    const mounted = render(header('arena', 0))
    expect(findAll(mounted.root, (node) => node.tag === 'button')).toHaveLength(0)
  })

  it('hides Game Stats and Sideboard Guides in Bo1 mode', () => {
    context.bestOf = 'bo1'
    context.showSwitch = true
    const text = textOf(render(header('arena', 0)).root)
    expect(text).not.toContain('Game Stats')
    expect(text).not.toContain('Sideboard Guides')
    expect(text).toContain('Matchups')
  })
})
