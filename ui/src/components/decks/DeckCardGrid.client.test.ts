import { describe, expect, it, vi } from 'vitest'
import type { DeckView } from '@/bindings'
import { renderHtml } from '@/replay/replayTestHarness'

vi.mock('@/components/ui/checkbox', async () => {
  const { defineComponent, h } = await import('vue')
  return {
    Checkbox: defineComponent({ inheritAttrs: false, setup: (_, { attrs }) => () => h('input', attrs) }),
  }
})

const { default: DeckCardGrid } = await import('./DeckCardGrid.vue')

function deck(id: number, format: string, commanderFormat = false): DeckView {
  return {
    id, name: `Qdeck ${id}`, originalName: null, format, formatLabel: format, colors: [],
    coverArt: null, localCoverArt: null, customCoverArt: null, archived: false,
    record: { wins: 0, losses: 0, draws: 0, total: 0, winRate: 0, label: '0 - 0' }, kinds: [],
    lastPlayedAtMs: null, archetype: null, commanderFormat,
  }
}

describe('the deck card grid', () => {
  it.each(['large', 'compact'] as const)('draws a %s Brawl deck card undimmed beside the others', async (cardSize) => {
    const { html } = await renderHtml(DeckCardGrid, {
      decks: [deck(1, 'CMODERN'), deck(2, 'ARENA_HISTORIC_BRAWL', true)],
      cardSize,
      now: 0,
      selectable: true,
      selectedIds: [],
      selectionFormat: null,
    })
    expect(html).toContain('Qdeck 2')
    expect(html).not.toContain('opacity-40')
  })

  it('dims the decks of another format while a selection is on', async () => {
    const { html } = await renderHtml(DeckCardGrid, {
      decks: [deck(1, 'CMODERN'), deck(2, 'ARENA_HISTORIC_BRAWL', true)],
      now: 0,
      selectable: true,
      selectedIds: [1],
      selectionFormat: 'CMODERN',
    })
    expect(html.match(/opacity-40/g)?.length).toBe(1)
  })
})
