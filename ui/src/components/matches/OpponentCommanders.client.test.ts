import { describe, expect, it } from 'vitest'
import type { CommanderCardView } from '@/bindings'
import { htmlText, renderHtml } from '@/replay/replayTestHarness'
import OpponentCommanders from './OpponentCommanders.vue'

function commander(catalogId: number, name: string, overrides: Partial<CommanderCardView> = {}): CommanderCardView {
  return { catalogId, name, image: null, localImage: null, colors: [], ...overrides }
}

describe('the opponent commander panel', () => {
  it('shows the full card, readable, with no overlaid name', async () => {
    const { html, warnings } = await renderHtml(OpponentCommanders, {
      commanders: [commander(7, 'Qomnath', { image: 'https://cards/7.jpg', colors: ['W', 'U', 'R', 'G'] })],
    })
    expect(warnings).toEqual([])
    const text = htmlText(html)
    expect(text).toContain('Opponent&#39;s commander')
    expect(html).toContain('src="https://cards/7.jpg"')
    expect(html).toContain('alt="Qomnath"')
    expect(text).not.toContain('Qomnath')
    expect(html).not.toContain('ms-w')
  })

  it('puts partners side by side, a card without an image as its name and colours', async () => {
    const { html } = await renderHtml(OpponentCommanders, {
      commanders: [commander(7, 'Qkraum', { image: 'https://cards/7.jpg' }), commander(8, 'Card 8', { colors: ['B'] })],
    })
    const text = htmlText(html)
    expect(text).toContain('Opponent&#39;s commanders')
    expect(text).toContain('Card 8')
    expect(text).not.toContain('Qkraum')
    expect(html).toContain('ms-b')
    expect(html.match(/data-commander=/g)).toHaveLength(2)
    expect(html).toContain('grid-cols-2')
  })
})
