import { describe, expect, it } from 'vitest'
import type { ArchetypeDecklistsView, ListCardView } from '@/bindings'
import { archetypeDecklist, decklistVersions, pickVersion, pileOf } from './archetypeDecklist'

function card(name: string, typeLine: string | null, quantity: number, overrides: Partial<ListCardView> = {}): ListCardView {
  return {
    catalogId: name.length,
    name,
    typeLine,
    colors: [],
    image: null,
    artCrop: null,
    localImage: null,
    localArtCrop: null,
    cmc: null,
    manaCost: null,
    quantity,
    sideboard: false,
    known: typeLine !== null,
    ...overrides,
  }
}

describe('the website-style decklist (issue #326)', () => {
  it('puts each type line in the website pile, lands first, creatures before artifacts', () => {
    expect(pileOf('Legendary Creature')).toBe('Creatures')
    expect(pileOf('Artifact Creature')).toBe('Creatures')
    expect(pileOf('Artifact Land')).toBe('Lands')
    expect(pileOf('Land Creature')).toBe('Lands')
    expect(pileOf('Kindred Instant')).toBe('Instants')
    expect(pileOf('Basic Land')).toBe('Lands')
    expect(pileOf('Artifact')).toBe('Artifacts')
    expect(pileOf(null)).toBe('Other')
    expect(pileOf('Conspiracy')).toBe('Other')
  })

  it('groups the maindeck by pile in order, lands and sideboard apart, with counts', () => {
    const list = archetypeDecklist([
      card('Qbolt', 'Instant', 4, { cmc: 1 }),
      card('Qforest', 'Basic Land', 6),
      card('Qtitan', 'Creature', 2, { cmc: 6 }),
      card('Qmite', 'Artifact Creature', 1, { cmc: 1 }),
      card('Qsaga', 'Enchantment Land', 4),
      card('Qbauble', 'Artifact', 2, { cmc: 0 }),
      card('Qmystery', null, 1),
      card('Qneedle', 'Artifact', 1, { cmc: 1, sideboard: true }),
      card('Qclaim', 'Instant', 2, { cmc: 1, sideboard: true }),
    ])
    expect(list.spells.map((pile) => [pile.name, pile.count])).toEqual([
      ['Creatures', 3],
      ['Instants', 4],
      ['Artifacts', 2],
      ['Other', 1],
    ])
    expect(list.lands.count).toBe(10)
    expect(list.lands.cards.map((c) => c.name)).toEqual(['Qforest', 'Qsaga'])
    expect(list.maindeckCount).toBe(20)
    expect(list.sideboard.count).toBe(3)
    // The sideboard reads in pile order too: the instant before the artifact.
    expect(list.sideboard.cards.map((c) => c.name)).toEqual(['Qclaim', 'Qneedle'])
    expect(list.total).toBe(23)
  })

  it('orders a pile cheapest first, then by name', () => {
    const list = archetypeDecklist([
      card('Qzeta', 'Creature', 1, { cmc: 2 }),
      card('Qbig', 'Creature', 1, { cmc: 13 }),
      card('Qalpha', 'Creature', 1, { cmc: 2 }),
      card('Qone', 'Creature', 1, { cmc: 1 }),
    ])
    expect(list.spells[0].cards.map((c) => c.name)).toEqual(['Qone', 'Qalpha', 'Qzeta', 'Qbig'])
  })
})

describe('the decklist versions (issue #326)', () => {
  const cards = [card('Qbolt', 'Instant', 4)]
  const live: ArchetypeDecklistsView = {
    state: 'ready',
    lists: [
      { uuid: 'q-one', seenCount: 9, label: 'Variant 1 · seen 9×', cards },
      { uuid: 'q-two', seenCount: 3, label: 'Variant 2 · seen 3×', cards },
    ],
  }

  it('offers the live lists by uuid once downloaded', () => {
    const { versions, note } = decklistVersions('downloaded', live, false, [])
    expect(versions.map((v) => [v.key, v.label, v.list, v.live])).toEqual([
      ['q-one', 'Variant 1 · seen 9×', 1, true],
      ['q-two', 'Variant 2 · seen 3×', 2, true],
    ])
    expect(note).toBeNull()
  })

  it("offers a manual archetype's variants, editable, by list number", () => {
    const { versions } = decklistVersions('manual', undefined, false, [
      { list: 1, cards },
      { list: 3, cards },
    ])
    expect(versions.map((v) => [v.key, v.label, v.editable])).toEqual([
      ['1', 'Variant 1', true],
      ['3', 'Variant 2', true],
    ])
  })

  it('says why there is no list', () => {
    expect(decklistVersions('manual', undefined, false, []).note).toBe('No variants yet. Use the form to add one.')
    expect(decklistVersions('downloaded', undefined, true, []).note).toBe('Downloading decklists...')
    expect(decklistVersions('downloaded', { state: 'ready', lists: [] }, false, []).note).toBe(
      'No variants downloaded yet.',
    )
    expect(decklistVersions('fallback', undefined, false, []).versions).toEqual([])
    expect(decklistVersions('commander', undefined, false, [{ list: 1, cards: [] }]).versions).toEqual([])
    const offline = decklistVersions('downloaded', { state: 'offline', lists: [] }, false, [{ list: 1, cards }])
    expect(offline.note).toBe('Decklists need a connection.')
    expect(offline.versions.map((v) => v.label)).toEqual(['Key cards'])
    const two = decklistVersions('downloaded', undefined, false, [
      { list: 1, cards },
      { list: 2, cards },
    ])
    expect(two.versions.map((v) => v.label)).toEqual(['Key cards 1', 'Key cards 2'])
  })

  it('picks the version the route names, else the first', () => {
    const { versions } = decklistVersions('downloaded', live, false, [])
    expect(pickVersion(versions, 'q-two')?.key).toBe('q-two')
    expect(pickVersion(versions, 'q-gone')?.key).toBe('q-one')
    expect(pickVersion(versions, undefined)?.key).toBe('q-one')
    expect(pickVersion([], 'q-one')).toBeNull()
  })
})
