import { describe, expect, it, vi } from 'vitest'
import type { ListCardView } from '@/bindings'
import { mountFake, textOf } from '@/replay/replayTestHarness'

vi.mock('@/bindings', () => ({ commands: {} }))

const { default: ArchetypeDecklist } = await import('./ArchetypeDecklist.vue')

function card(name: string, typeLine: string, quantity: number, overrides: Partial<ListCardView> = {}): ListCardView {
  return {
    catalogId: name.length,
    name,
    typeLine,
    colors: [],
    image: null,
    artCrop: null,
    localImage: null,
    localArtCrop: null,
    cmc: 1,
    manaCost: null,
    quantity,
    sideboard: false,
    known: true,
    ...overrides,
  }
}

describe('the website-style decklist (issue #326)', () => {
  it('shows the maindeck piles, the lands and the sideboard with counts and the total', () => {
    const mounted = mountFake(ArchetypeDecklist, {
      cards: [
        card('Qmite', 'Artifact Creature', 4, { manaCost: '{G}' }),
        card('Qbolt', 'Instant', 2, { manaCost: '{R}' }),
        card('Qforest', 'Basic Land', 6),
        card('Qneedle', 'Artifact', 1, { sideboard: true, manaCost: '{1}' }),
      ],
    })
    const text = textOf(mounted.root).replace(/\s+/g, ' ')
    expect(text).toContain('Maindeck (12)')
    expect(text).toContain('Creatures (4)')
    expect(text).toContain('Instants (2)')
    expect(text).toContain('Lands (6)')
    expect(text).toContain('Sideboard (1)')
    expect(text).toContain('13 cards total')
    expect(text.indexOf('Qmite')).toBeLessThan(text.indexOf('Qbolt'))
    expect(text.indexOf('Qforest')).toBeLessThan(text.indexOf('Qneedle'))
  })
})
