import { describe, expect, it, vi } from 'vitest'
import type { CommanderCardView, OpponentArchetypeView } from '@/bindings'
import { htmlText, renderHtml } from '@/replay/replayTestHarness'

// The app's card hover (CardHoverPreview) as a tag that shows what it got.
vi.mock('@/components/cards/CardHoverPreview.vue', async () => {
  const { defineComponent, h } = await import('vue')
  return {
    default: defineComponent({
      inheritAttrs: false,
      setup: (_, { attrs, slots }) => () =>
        h(
          'card-hover',
          {
            'data-name': attrs.name,
            'data-image': attrs.image,
            'data-partners': ((attrs.partners as CommanderCardView[] | undefined) ?? []).map((card) => card.name).join('|'),
          },
          slots.default?.(),
        ),
    }),
  }
})

const { default: ArchetypeName } = await import('./ArchetypeName.vue')

function card(catalogId: number, name: string, image: string | null = null): CommanderCardView {
  return { catalogId, name, image, localImage: null, colors: [] }
}

function archetype(commanderCards: CommanderCardView[]): OpponentArchetypeView {
  return { id: 3, name: 'Qkraum & Qtymna', colors: [], commanderCards }
}

describe('an archetype name', () => {
  it('is plain text for an archetype that is not a commander', async () => {
    const { html } = await renderHtml(ArchetypeName, { archetype: archetype([]) })
    expect(htmlText(html)).toBe('Qkraum &amp; Qtymna')
    expect(html).not.toContain('card-hover')
  })

  it('previews a commander archetype’s card on hover, partners both', async () => {
    const { html } = await renderHtml(ArchetypeName, {
      archetype: archetype([card(1, 'Qkraum', 'https://cards/1.jpg'), card(2, 'Qtymna', 'https://cards/2.jpg')]),
    })
    expect(html).toContain('data-name="Qkraum"')
    expect(html).toContain('data-image="https://cards/1.jpg"')
    expect(html).toContain('data-partners="Qtymna"')
    expect(htmlText(html)).toBe('Qkraum &amp; Qtymna')
  })
})
