import { describe, expect, it, vi } from 'vitest'
import type { ListCardView } from '@/bindings'
import { htmlText, renderHtml } from '@/replay/replayTestHarness'

vi.mock('@/components/CardImage.vue', async () => {
  const { defineComponent, h } = await import('vue')
  return { default: defineComponent({ inheritAttrs: false, setup: () => () => h('img') }) }
})

const { default: DeckList } = await import('./DeckList.vue')

function card(name: string, quantity = 1): ListCardView {
  return {
    catalogId: 1,
    name,
    typeLine: 'Creature',
    colors: [],
    image: null,
    artCrop: null,
    localImage: null,
    localArtCrop: null,
    cmc: 2,
    manaCost: null,
    quantity,
    sideboard: false,
    known: true,
  }
}

const main = [{ typeName: 'Creature', count: 1, cards: [card('Qgoyf')] }]

describe('the deck list', () => {
  it('shows the commanders before the main deck and no Sideboard heading', async () => {
    const { html } = await renderHtml(DeckList, {
      commanders: [card('Qking'), card('Qpal')],
      maindeck: main,
      sideboard: [],
    })
    const text = htmlText(html)
    expect(text).toContain('Commander (2)')
    expect(text.indexOf('Commander (2)')).toBeLessThan(text.indexOf('Maindeck'))
    expect(text).not.toContain('Sideboard')
    expect(html.match(/data-commander="true"/g)).toHaveLength(2)
  })

  it('draws a commander like any other card: the heading is the only marker', async () => {
    const { html } = await renderHtml(DeckList, { commanders: [card('Qking')], maindeck: main, sideboard: [] })
    const text = htmlText(html)
    expect(text.match(/Commander/g)).toHaveLength(1)
    expect(html).not.toContain('ring-primary')
  })

  it('keeps a normal deck as it was: no Commander section, its sideboard', async () => {
    const { html } = await renderHtml(DeckList, { commanders: [], maindeck: main, sideboard: [card('Qwall', 2)] })
    const text = htmlText(html)
    expect(text).not.toContain('Commander')
    expect(text).toContain('Sideboard (2)')
    expect(html).not.toContain('data-commander')
  })

  it('still shows an empty Sideboard (0) for a normal deck', async () => {
    const { html } = await renderHtml(DeckList, { commanders: [], maindeck: main, sideboard: [] })
    expect(htmlText(html)).toContain('Sideboard (0)')
  })
})
