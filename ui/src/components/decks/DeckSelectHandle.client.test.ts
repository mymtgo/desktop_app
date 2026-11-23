import { describe, expect, it, vi } from 'vitest'
import type { DeckView } from '@/bindings'
import { renderHtml } from '@/replay/replayTestHarness'

vi.mock('@/components/ui/checkbox', async () => {
  const { defineComponent, h } = await import('vue')
  return {
    Checkbox: defineComponent({ inheritAttrs: false, setup: (_, { attrs }) => () => h('input', attrs) }),
  }
})

const { default: DeckSelectHandle } = await import('./DeckSelectHandle.vue')

function deck(format: string, commanderFormat = false): DeckView {
  return {
    id: 1, name: 'Qdeck', originalName: null, format, formatLabel: format, colors: [],
    coverArt: null, localCoverArt: null, customCoverArt: null, archived: false,
    record: { wins: 0, losses: 0, draws: 0, total: 0, winRate: 0, label: '0 - 0' }, kinds: [],
    lastPlayedAtMs: null, archetype: null, commanderFormat,
  }
}

describe('the deck select handle', () => {
  it('has a drag grip for a normal deck', async () => {
    const { html } = await renderHtml(DeckSelectHandle, {
      deck: deck('CMODERN'), selected: false, selectionBlocked: false, selectedIds: [],
    })
    expect(html).toContain('draggable="true"')
  })

  it('has no drag grip for a commander format deck, and says why it cannot be selected', async () => {
    const { html } = await renderHtml(DeckSelectHandle, {
      deck: deck('ARENA_HISTORIC_BRAWL', true), selected: false, selectionBlocked: true, selectedIds: [],
    })
    expect(html).not.toContain('draggable')
    expect(html).toContain("Brawl decks take their commander&#39;s archetype")
  })
})
