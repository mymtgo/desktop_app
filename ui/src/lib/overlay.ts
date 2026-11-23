/**
 * The game overlay's pure rules (overlay spec 7; 0.x `GameOverlay.vue`,
 * `OverlayTabs.vue`, `DrawOddsPanel.vue`, `RevealedCards.vue`,
 * `SideboardGuide.vue`, `ArchetypeNotes.vue`). Every string is 0.x's; 0.x's
 * em dash cells are written "-".
 */
import type {
  DrawOdds,
  GameOverlayView,
  OverlayArchetypeOption,
  SideboardGuide,
  SidedInCard,
  SidedOutCard,
} from '@/bindings'
import { hypergeometric } from '@/lib/hypergeometric'

/** The empty cell (0.x's em dash, spec 7.6). */
export const EMPTY_CELL = '-'

/** A changed draw odds row flashes this long (0.x `HIGHLIGHT_MS`). */
export const FLASH_MS = 1200

/** The page waits this long after a measure before it fits the window. */
export const FIT_DEBOUNCE_MS = 100

export const WAITING_FOR_MATCH = 'Waiting for match…'
export const WAITING_FOR_OPPONENT = 'Waiting for opponent…'
export const NO_SECTIONS = 'Enable draw odds, revealed cards, or the sideboard guide in Settings.'
export const UNKNOWN_ARCHETYPE = 'Unknown archetype'
export const UNREACHABLE_ARCHETYPE = "Couldn't reach MyMTGO"
export const SEARCH_ARCHETYPES = 'Search archetypes…'
export const NO_ARCHETYPES = 'No archetypes found.'
export const NOTE_PLACEHOLDER = 'Add a note for this matchup…'

/**
 * A probability as a percentage. An uncertain outcome (0 < value < 1) never
 * rounds to 0% or 100%: that would claim impossibility or certainty the
 * maths does not support (0.x `pct`).
 */
export function pct(value: number, digits = 1): string {
  let display = value * 100
  if (value > 0 && value < 1) {
    const step = 10 ** -digits
    display = Math.min(Math.max(display, step), 100 - step)
  }
  return `${display.toFixed(digits)}%`
}

/** The stepper's ceiling: the library left, at least 1. */
export function maxSample(odds: DrawOdds | null): number {
  return Math.max(1, odds?.librarySize ?? 1)
}

/** The sample pulled back when the library shrinks below it. */
export function clampSample(sample: number, max: number): number {
  return Math.min(Math.max(1, sample), max)
}

/** "Next draw" or "Next draws". */
export function nextDrawsLabel(sample: number): string {
  return `Next ${sample === 1 ? 'draw' : 'draws'}`
}

/**
 * P(at least one of `remaining` copies in the next `sample` draws from
 * `library`). No copies left can never be drawn: 0, not P(X >= 0).
 */
export function drawChance(library: number, remaining: number, sample: number): number {
  if (remaining <= 0) return 0
  return hypergeometric(library, remaining, sample, 1).atLeast
}

/** A draw odds row's flash key: its printing, else its name (0.x). */
export function cardKey(card: { catalogId: number | null; name: string }): string {
  return card.catalogId !== null ? String(card.catalogId) : card.name
}

/**
 * The rows whose remaining copies changed since `previous` (a drawn card,
 * or one shuffled back). `previous` is updated to the new counts; a row
 * seen for the first time does not flash.
 */
export function changedRows(
  previous: Map<string, number>,
  cards: { catalogId: number; name: string; remaining: number }[],
): string[] {
  const changed: string[] = []
  for (const card of cards) {
    const key = cardKey(card)
    const before = previous.get(key)
    if (before !== undefined && before !== card.remaining) changed.push(key)
    previous.set(key, card.remaining)
  }
  return changed
}

/** The draw odds panel's empty message, or null when it lists cards. */
export function drawOddsEmpty(odds: DrawOdds | null): string | null {
  if (!odds) return WAITING_FOR_MATCH
  if (odds.cards.length === 0) return 'No deck data for this match'
  return null
}

/** The reveals panel's empty message, or null when it lists cards. */
export function revealsEmpty(view: Pick<GameOverlayView, 'hasMatch' | 'reveals' | 'potential'>): string | null {
  if (!view.hasMatch) return WAITING_FOR_MATCH
  const revealed = (view.reveals ?? []).length > 0
  const potential = (view.potential?.maindeck.length ?? 0) + (view.potential?.sideboard.length ?? 0) > 0
  if (!revealed && !potential) return 'Nothing revealed yet'
  return null
}

/** The sideboard panel's empty message (0.x order), or null. */
export function sideboardEmpty(view: Pick<GameOverlayView, 'hasMatch' | 'hasDeck' | 'hasArchetype'>): string | null {
  if (!view.hasMatch) return WAITING_FOR_MATCH
  if (!view.hasDeck) return 'No deck linked to this match'
  if (!view.hasArchetype) return 'Pick an archetype to see your sideboard guide'
  return null
}

/**
 * The guide's header strip: "Your guide · " with a plan, then the
 * postboard games and record, or "No games vs this archetype yet". No
 * field caption (spec ruling 6).
 */
export function sideboardHeader(guide: SideboardGuide): string {
  const lead = guide.hasPlan ? 'Your guide · ' : ''
  if (guide.postboardGames > 0) {
    const games = guide.postboardGames === 1 ? 'game' : 'games'
    return `${lead}${guide.postboardGames} post-board ${games} · ${guide.postboardRecord} overall`
  }
  return `${lead}No games vs this archetype yet`
}

/** The "Your W–L" cell: `{w}–{l} ({rate}%)`, or "-" without sided-in games. */
export function winLossCell(card: SidedInCard): string {
  if (card.sidedInGames > 0) return `${card.wins}–${card.losses} (${card.winrate ?? 0}%)`
  return EMPTY_CELL
}

/** The "You cut" cell: `{n}×`, or "-" when never cut. */
export function cutCell(card: SidedOutCard): string {
  if (card.sidedOutGames > 0) return `${card.sidedOutGames}×`
  return EMPTY_CELL
}

/** A sided-in row's name, marked when the plan's card left the sideboard. */
export function sidedInName(card: SidedInCard): string {
  return card.stale ? `${card.name} (not in sideboard)` : card.name
}

/** A sided-out row's name, marked when the plan's card left the maindeck. */
export function sidedOutName(card: SidedOutCard): string {
  return card.stale ? `${card.name} (not in maindeck)` : card.name
}

/** A sided-in row is faded when stale, or in History with no games. */
export function sidedInFaded(card: SidedInCard, hasPlan: boolean): boolean {
  return card.stale || (!hasPlan && card.sidedInGames === 0)
}

/** The sided-out heading: "Take out" with a plan, else "Usually cut". */
export function sidedOutHeading(guide: SideboardGuide): string {
  return `${guide.hasPlan ? 'Take out' : 'Usually cut'} (${guide.sidedOut.length})`
}

export type TabValue = 'draw-odds' | 'reveals' | 'sideboard'

export interface OverlayTab {
  value: TabValue
  label: string
}

/** The tabs of the sections switched on, in 0.x's order. */
export function overlayTabs(sections: GameOverlayView['sections']): OverlayTab[] {
  const all: (OverlayTab & { enabled: boolean })[] = [
    { value: 'draw-odds', label: 'Draw odds', enabled: sections.drawOdds },
    { value: 'reveals', label: 'Revealed', enabled: sections.reveals },
    { value: 'sideboard', label: 'Sideboarding', enabled: sections.sideboard },
  ]
  return all.filter((tab) => tab.enabled).map(({ value, label }) => ({ value, label }))
}

/** Whether any tab section is on (the window then has a tab area). */
export function hasTabSections(sections: GameOverlayView['sections']): boolean {
  return sections.drawOdds || sections.reveals || sections.sideboard
}

/** The collapse toggle's label. */
export function toggleLabel(collapsed: boolean): string {
  return collapsed ? 'Expand overlay' : 'Collapse overlay'
}

/** Picking a tab while collapsed expands the overlay: the player wants to read it. */
export function expandsOnTabPick(collapsed: boolean): boolean {
  return collapsed
}

/**
 * The tab after a new view: the Sideboarding tab the moment sideboarding
 * starts (with its section on), once; leaving sideboarding changes nothing,
 * so the tab carries over into the next game (0.x).
 */
export function tabAfter(active: string, wasSideboarding: boolean, view: GameOverlayView): string {
  if (view.isSideboarding && !wasSideboarding && view.sections.sideboard) return 'sideboard'
  return active
}

/** Whether a pushed view replaces the one held (only a newer revision). */
export function isNewer(pushed: number, held: number | null): boolean {
  return held === null || pushed > held
}

/** A note's date (0.x `stamp`): day, short month, year. */
export function noteStamp(ms: number, locale?: string): string {
  return new Date(ms).toLocaleDateString(locale, { day: 'numeric', month: 'short', year: 'numeric' })
}

/** An other deck's note's caption: `{deck} · {date}`. */
export function otherNoteCaption(deckName: string, ms: number, locale?: string): string {
  return `${deckName} · ${noteStamp(ms, locale)}`
}

/**
 * What the archetype picker lists (0.x `useArchetypeSplit`): the
 * fallbacks (every format, never searched) and the other archetypes whose
 * name holds the search, any case. Regular ones come first.
 */
export function pickerList(options: OverlayArchetypeOption[], search: string): OverlayArchetypeOption[] {
  const term = search.trim().toLowerCase()
  const regular = options.filter((option) => !option.isFallback && (term === '' || option.name.toLowerCase().includes(term)))
  const fallbacks = options.filter((option) => option.isFallback)
  return [...regular, ...fallbacks]
}

/** The picker's highlighted row after an arrow key, wrapping (0.x). */
export function moveHighlight(index: number, delta: number, count: number): number {
  if (count === 0) return 0
  return (index + delta + count) % count
}

/** An option's key: its uuid (unmet archetypes have no id). */
export function optionKey(option: OverlayArchetypeOption): string {
  return option.uuid
}

/** Whether an option is the archetype shown now. */
export function isCurrent(option: OverlayArchetypeOption, currentId: number | null, currentUuid: string | null): boolean {
  if (option.id !== null && currentId !== null) return option.id === currentId
  return currentUuid !== null && option.uuid === currentUuid
}
