import type { ArchetypeDecklistsView, ArchetypeSourceView, ListCardView } from '@/bindings'

// The archetype page's decklist laid out like the mymtgo.com website's
// (issue #326; the API `DecklistPanel.vue`, `CardTypeGroup.php` and
// `RenderDecklist.php`): the maindeck by type pile, lands in their own
// column above the sideboard, every pile cheapest first then by name.

/** The piles in the order a decklist reads (the API `CardTypeGroup::ORDER`). */
export const PILE_ORDER = [
  'Creatures',
  'Planeswalkers',
  'Instants',
  'Sorceries',
  'Artifacts',
  'Enchantments',
  'Battles',
  'Other',
  'Lands',
] as const

/**
 * Card types in precedence order, first match wins: Land before Creature puts
 * Dryad Arbor in Lands, Creature before Artifact puts an artifact creature in
 * Creatures (the API `CardTypeGroup::PRECEDENCE`).
 */
const PRECEDENCE: [string, string][] = [
  ['Land', 'Lands'],
  ['Creature', 'Creatures'],
  ['Planeswalker', 'Planeswalkers'],
  ['Instant', 'Instants'],
  ['Sorcery', 'Sorceries'],
  ['Artifact', 'Artifacts'],
  ['Enchantment', 'Enchantments'],
  ['Battle', 'Battles'],
]

/** The pile a card's type line belongs in; Other for an unknown type or no card data. */
export function pileOf(typeLine: string | null): string {
  const words = (typeLine ?? '').trim().split(/\s+/)
  for (const [cardType, pile] of PRECEDENCE) {
    if (words.includes(cardType)) return pile
  }
  return 'Other'
}

export type DecklistPile = { name: string; count: number; cards: ListCardView[] }

export type ArchetypeDecklist = {
  /** Every maindeck card, lands included (the website's "Maindeck (60)"). */
  maindeckCount: number
  /** The maindeck piles other than Lands, in `PILE_ORDER`; empty piles left out. */
  spells: DecklistPile[]
  lands: DecklistPile
  sideboard: DecklistPile
  /** Maindeck and sideboard: "75 cards total". */
  total: number
}

function copies(cards: ListCardView[]): number {
  return cards.reduce((sum, card) => sum + card.quantity, 0)
}

function pileRank(card: ListCardView): number {
  return PILE_ORDER.indexOf(pileOf(card.typeLine) as (typeof PILE_ORDER)[number])
}

/** Pile order, then mana value (none counts as 0), then name. */
function compareCards(a: ListCardView, b: ListCardView): number {
  return pileRank(a) - pileRank(b) || (a.cmc ?? 0) - (b.cmc ?? 0) || a.name.localeCompare(b.name)
}

/** One list as the website lays it out. */
export function archetypeDecklist(cards: ListCardView[]): ArchetypeDecklist {
  const main = cards.filter((card) => !card.sideboard).sort(compareCards)
  const side = cards.filter((card) => card.sideboard).sort(compareCards)
  const spells: DecklistPile[] = []
  for (const name of PILE_ORDER) {
    if (name === 'Lands') continue
    const inPile = main.filter((card) => pileOf(card.typeLine) === name)
    if (inPile.length > 0) spells.push({ name, count: copies(inPile), cards: inPile })
  }
  const landCards = main.filter((card) => pileOf(card.typeLine) === 'Lands')
  return {
    maindeckCount: copies(main),
    spells,
    lands: { name: 'Lands', count: copies(landCards), cards: landCards },
    sideboard: { name: 'Sideboard', count: copies(side), cards: side },
    total: copies(main) + copies(side),
  }
}

// ---- The list versions (the Decklist tab's picker) ------------------------------

/** One list the Decklist tab can show. */
export type DecklistVersion = {
  /** The route's `version`: a live list's uuid, a stored list's number. */
  key: string
  label: string
  cards: ListCardView[]
  /** The stored list number (a live list: its place, 1 first). */
  list: number
  /** A live decklist from the website: exported from its cards. */
  live: boolean
  /** A manual archetype's variant: can be moved and removed. */
  editable: boolean
}

export type DecklistVersions = {
  versions: DecklistVersion[]
  /** The line above (or instead of) the list. */
  note: string | null
}

type StoredList = { list: number; cards: ListCardView[] }

/**
 * The lists of an archetype (spec section 10, 0.x's variant picker): a
 * manual archetype's variants; the live decklists once downloaded; while
 * they cannot be had, the stored key list; a fallback has none.
 */
export function decklistVersions(
  source: ArchetypeSourceView,
  decklists: ArchetypeDecklistsView | undefined,
  loading: boolean,
  stored: StoredList[],
): DecklistVersions {
  if (source === 'fallback') {
    return {
      versions: [],
      note: 'System fallback for unidentified opponent decks. Auto-assigned when no archetype matches.',
    }
  }
  if (source === 'commander') {
    // Made from its commanders: no key list, no variants.
    return { versions: [], note: 'Made from its commanders. It has no decklists.' }
  }
  if (source === 'manual') {
    const versions = stored.map((variant, index) => ({
      key: String(variant.list),
      label: `Variant ${index + 1}`,
      cards: variant.cards,
      list: variant.list,
      live: false,
      editable: true,
    }))
    return { versions, note: versions.length === 0 ? 'No variants yet. Use the form to add one.' : null }
  }
  if (loading) return { versions: [], note: 'Downloading decklists...' }
  if (decklists?.state === 'ready') {
    const versions = decklists.lists.map((list, index) => ({
      key: list.uuid,
      label: list.label,
      cards: list.cards,
      list: index + 1,
      live: true,
      editable: false,
    }))
    return { versions, note: versions.length === 0 ? 'No variants downloaded yet.' : null }
  }
  const versions = stored.map((variant, index) => ({
    key: String(variant.list),
    label: stored.length > 1 ? `Key cards ${index + 1}` : 'Key cards',
    cards: variant.cards,
    list: variant.list,
    live: false,
    editable: false,
  }))
  return { versions, note: 'Decklists need a connection.' }
}

/** The version the route names, else the first; none without versions. */
export function pickVersion(versions: DecklistVersion[], value: unknown): DecklistVersion | null {
  return versions.find((version) => version.key === value) ?? versions[0] ?? null
}
