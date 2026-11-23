import { describe, expect, it, vi } from 'vitest'
import type { DeckView, RecordView } from '@/bindings'
import { findAll, mountFake, textOf } from '@/replay/replayTestHarness'

const stub = vi.hoisted(() => async (tag: string) => {
  const { defineComponent, h } = await import('vue')
  return { default: defineComponent({ inheritAttrs: false, setup: () => () => h(tag) }) }
})

vi.mock('vue-router', async () => {
  const { RouterLinkStub } = await import('@/lib/testRouterStub')
  return { RouterLink: RouterLinkStub }
})
vi.mock('@/components/CardImage.vue', () => stub('card-image'))
vi.mock('@/components/mana/ManaColors.vue', () => stub('mana-colors'))
vi.mock('@/components/decks/DeckBadges.vue', () => stub('deck-badges'))
vi.mock('@/components/decks/DeckSelectHandle.vue', () => stub('deck-select'))
vi.mock('@/components/decks/WinRateBar.vue', async () => {
  const { defineComponent, h } = await import('vue')
  return {
    default: defineComponent({
      props: { winrate: { type: Number, required: true } },
      setup: (props) => () => h('win-rate', { 'data-rate': props.winrate }),
    }),
  }
})
vi.mock('@/components/decks/MatchRecord.vue', async () => {
  const { defineComponent, h } = await import('vue')
  return {
    default: defineComponent({
      props: { record: { type: Object, required: true } },
      setup: (props) => () => h('record', (props.record as { label: string }).label),
    }),
  }
})
vi.mock('@/components/ui/card', async () => {
  const { defineComponent, h } = await import('vue')
  const plain = defineComponent({ setup: (_, { slots }) => () => h('div', slots.default?.()) })
  return { Card: plain, CardContent: plain }
})
vi.mock('@/components/ui/badge', async () => {
  const { defineComponent, h } = await import('vue')
  return { Badge: defineComponent({ setup: (_, { slots }) => () => h('badge', slots.default?.()) }) }
})

const { default: DeckCard } = await import('./DeckCard.vue')
const { default: DeckCardCompact } = await import('./DeckCardCompact.vue')

function record(overrides: Partial<RecordView> = {}): RecordView {
  return { wins: 3, losses: 1, draws: 0, total: 4, winRate: 75, label: '3 - 1', ...overrides }
}

function deck(overrides: Partial<DeckView> = {}): DeckView {
  return {
    id: 1, name: 'Qdeck', originalName: null, format: 'CMODERN', formatLabel: 'Modern', colors: [],
    coverArt: null, localCoverArt: null, customCoverArt: null, archived: false, record: record(), kinds: [],
    lastPlayedAtMs: null, archetype: null, commanderFormat: false, ...overrides,
  }
}

const arenaDeck = deck({ kinds: ['bo3', 'bo1'] })

describe.each([
  ['DeckCard', DeckCard],
  ['DeckCardCompact', DeckCardCompact],
])('%s', (_name, component) => {
  it('shows the record over all matches and a tag for each kind on an Arena deck', () => {
    const mounted = mountFake(component, { deck: arenaDeck, now: 0 })
    expect(findAll(mounted.root, (n) => n.tag === 'win-rate')[0].props['data-rate']).toBe(75)
    expect(textOf(mounted.root)).toContain('3 - 1')
    expect(findAll(mounted.root, (n) => n.props['data-testid'] === 'deck-kind').map(textOf)).toEqual(['Bo3', 'Bo1'])
  })

  it('leaves an MTGO deck with no tag', () => {
    const mounted = mountFake(component, { deck: deck(), now: 0 })
    expect(findAll(mounted.root, (n) => n.tag === 'win-rate')[0].props['data-rate']).toBe(75)
    expect(textOf(mounted.root)).toContain('3 - 1')
    expect(findAll(mounted.root, (n) => n.props['data-testid'] === 'deck-kind')).toEqual([])
  })
})
