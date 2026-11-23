import type { RouteLocationRaw } from 'vue-router'
import type { DeckArchetypeOptionView, DeckView, RecordView } from '@/bindings'

export type DeckIndexTab = 'decks' | 'matches' | 'matchups' | 'trends' | 'card-stats'

/** The tabs under an archetype on the deck index (0.x `DECK_INDEX_TABS`). */
export const DECK_INDEX_TABS: { key: DeckIndexTab; label: string }[] = [
  { key: 'decks', label: 'Decks' },
  { key: 'matches', label: 'Matches' },
  { key: 'matchups', label: 'Matchups' },
  { key: 'trends', label: 'Trends' },
  { key: 'card-stats', label: 'Card Stats' },
]

const TAB_ROUTES: Record<Exclude<DeckIndexTab, 'decks'>, string> = {
  matches: 'deck-archetype-matches',
  matchups: 'deck-archetype-matchups',
  trends: 'deck-archetype-trends',
  'card-stats': 'deck-archetype-card-stats',
}

/**
 * One tab of an archetype (0.x `deckIndexTabUrl`): the grid keeps the
 * archetype in the remembered filters; the stats tabs carry the timeframe,
 * left off when it is all time.
 */
export function deckIndexTabRoute(tab: DeckIndexTab, archetypeId: number, timeframe: string): RouteLocationRaw {
  if (tab === 'decks') return { name: 'decks' }
  return {
    name: TAB_ROUTES[tab],
    params: { archetypeId: String(archetypeId) },
    query: timeframe !== 'alltime' ? { timeframe } : {},
  }
}

/** A sidebar row click: the chosen row again clears the filter. */
export function toggleArchetype(current: string | null, value: string): string | null {
  return current === value ? null : value
}

/** A row's rate: `-` before any match. */
export function archetypeRate(record: RecordView): string {
  return record.total === 0 ? '-' : `${record.winRate}%`
}

/** A rate's colour: muted without matches, green from 50, else red (0.x `winrateClass`). */
export function rateTone(record: RecordView): string {
  if (record.total === 0) return 'text-muted-foreground'
  return record.winRate >= 50 ? 'text-success' : 'text-destructive'
}

/** A row's tooltip (0.x). */
export function archetypeRowTitle(option: DeckArchetypeOptionView): string {
  return `${option.name} · ${option.record.label} · ${option.record.total} matches`
}

export function deckCount(count: number): string {
  return `${count} ${count === 1 ? 'deck' : 'decks'}`
}

/**
 * The format every selected deck shares, or null (0.x `selectionFormat`):
 * same-named archetypes exist once per format, so a shared format scopes the
 * picker.
 */
export function selectionFormat(selectedIds: number[], decks: DeckView[]): string | null {
  const formats = new Set(
    selectedIds.map((id) => decks.find((deck) => deck.id === id)?.format).filter((format) => !!format),
  )
  return formats.size === 1 ? ([...formats][0] ?? null) : null
}

/**
 * A commander format deck (an Arena Brawl deck, decided by the backend's
 * `commanderFormat`) takes its commander's archetype: detection would undo a
 * manual one.
 */
export function canSetArchetypeByHand(deck: DeckView): boolean {
  return !deck.commanderFormat
}

/** The ids whose archetype can be set by hand: decks not on the page are kept, commander format decks dropped. */
export function assignableDeckIds(ids: number[], decks: DeckView[]): number[] {
  const locked = new Set(decks.filter((deck) => !canSetArchetypeByHand(deck)).map((deck) => deck.id))
  return ids.filter((id) => !locked.has(id))
}

/** The tooltip on a disabled selection checkbox. */
export function selectionBlockedTitle(deck: DeckView): string {
  return canSetArchetypeByHand(deck)
    ? 'Only decks of one format can be selected together'
    : COMMANDER_ARCHETYPE_NOTE_SHORT
}

export const COMMANDER_ARCHETYPE_NOTE_SHORT = "Brawl decks take their commander's archetype"

/**
 * A deck can join the selection when it has the selection's format (or none
 * is chosen), and its archetype can be set by hand.
 */
export function canSelectDeck(deck: DeckView, format: string | null): boolean {
  return canSetArchetypeByHand(deck) && (format === null || deck.format === format)
}

/**
 * A deck card fades back while a selection of another format is on. A Brawl
 * deck is drawn like any other: only its checkbox is unavailable.
 */
export function dimsInSelection(deck: DeckView, format: string | null): boolean {
  return format !== null && deck.format !== format
}
