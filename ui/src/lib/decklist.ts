import type { ColourCount, CurvePoint, DekExport, ListCardView, TypeGroupView } from '@/bindings'
import { normalizeType, TYPE_ORDER } from './cardTypes'
import { errorMessage, unwrap, type CommandResult } from './commands'
import { formatPercent, type HypergeometricResult } from './hypergeometric'
import { MANA_COLOR_NAMES } from './mana'

/** The cards of a list, quantities added (0.x `getCount`). */
export function cardCount(cards: ListCardView[]): number {
  return cards.reduce((sum, card) => sum + card.quantity, 0)
}

export type MaindeckGroup = { type: string; count: number; cards: ListCardView[] }

/**
 * The main deck by canonical type in display order (0.x `DeckList.vue`):
 * groups whose type line shares a canonical type are merged, and types with
 * no place in the order keep theirs after the canonical ones.
 */
export function groupedMaindeck(groups: TypeGroupView[]): MaindeckGroup[] {
  const merged = new Map<string, ListCardView[]>()
  for (const group of groups) {
    const key = normalizeType(group.typeName)
    const cards = merged.get(key) ?? []
    cards.push(...group.cards)
    merged.set(key, cards)
  }
  const result: MaindeckGroup[] = []
  for (const [type, cards] of merged) {
    result.push({ type, count: cardCount(cards), cards })
  }
  // Array sort is stable, so types without an order keep theirs.
  return result.sort((a, b) => (TYPE_ORDER[a.type] ?? 99) - (TYPE_ORDER[b.type] ?? 99))
}

/**
 * The decklist.org registration link (0.x `Decklist.vue`): one `N Name` line
 * per card in `deckmain` and `deckside`, and the format's display name in
 * `eventformat`. Cards without card data are left out: 0.x never listed them
 * and decklist.org cannot read "Card <id>".
 */
export function decklistOrgUrl(main: TypeGroupView[], side: ListCardView[], format: string): string {
  const line = (card: ListCardView) => `${card.quantity} ${card.name}`
  const mainCards = main
    .flatMap((group) => group.cards)
    .filter((card) => card.known)
    .map(line)
    .join('\n')
  const sideCards = side
    .filter((card) => card.known)
    .map(line)
    .join('\n')
  const params = new URLSearchParams({
    deckmain: mainCards,
    deckside: sideCards,
    eventformat: format,
  })
  return `https://decklist.org/?${params.toString()}`
}

export type DrawOddsCard = { name: string; quantity: number }

/**
 * The cards the draw odds calculator offers: every main deck card, in list
 * order. Printings that share a name are one card, their copies added.
 */
export function drawOddsCards(main: TypeGroupView[]): DrawOddsCard[] {
  const cards: DrawOddsCard[] = []
  for (const card of main.flatMap((group) => group.cards)) {
    const same = cards.find((existing) => existing.name === card.name)
    if (same) {
      same.quantity += card.quantity
    } else {
      cards.push({ name: card.name, quantity: card.quantity })
    }
  }
  return cards
}

/** The calculator's four result rows (0.x `HypergeometricCalculator.vue`). */
export function resultLines(result: HypergeometricResult, wanted: number): { label: string; value: string }[] {
  const k = Math.max(0, wanted)
  return [
    { label: `Chance to draw ${k} or more`, value: formatPercent(result.atLeast) },
    { label: `Chance to draw exactly ${k}`, value: formatPercent(result.exactly) },
    { label: `Chance to draw ${k} or less`, value: formatPercent(result.atMost) },
    { label: 'Chance to draw 0', value: formatPercent(result.zero) },
  ]
}

export type CurveBucket = { cmc: string; count: number }

/** The curve's bars: mana values 0 to 6, and `7+`. */
export function curveBuckets(curve: CurvePoint[]): CurveBucket[] {
  return curve.map((point) => ({ cmc: point.cmc >= 7 ? '7+' : String(point.cmc), count: point.count }))
}

/** The tallest bar, at least 1. */
export function curveMax(buckets: CurveBucket[]): number {
  return Math.max(...buckets.map((bucket) => bucket.count), 1)
}

export type ColourStat = { color: string; label: string; count: number; total: number; percentage: number }

/** Each colour's share of the nonland cards (0.x `CurveAndColours.vue`). */
export function colourStats(colours: ColourCount[], total: number): ColourStat[] {
  if (total === 0) return []
  return colours
    .filter((entry) => entry.count > 0)
    .map((entry) => ({
      color: entry.colour,
      label: MANA_COLOR_NAMES[entry.colour] ?? entry.colour,
      count: entry.count,
      total,
      percentage: Math.round((entry.count / total) * 100),
    }))
}

export type ToastOptions = { type: 'success' | 'error'; title: string; message: string }

/** What to tell the user after the save dialog: the path, or nothing when cancelled. */
export function dekExportToast(result: DekExport): ToastOptions | null {
  if (result.kind === 'cancelled') return null
  return { type: 'success', title: 'Saved deck file', message: result.path }
}

export function dekExportFailedToast(error: unknown): ToastOptions {
  return { type: 'error', title: 'Export failed', message: errorMessage(error) }
}

type OpenExternal = (
  url: string,
) => Promise<CommandResult<null>>

/**
 * Open the registration link in the user's browser through the app
 * (`commands.openExternal`): the webview cannot open new windows. `null` when
 * it opened, else the error toast.
 */
export async function openRegistrationLink(url: string, open: OpenExternal): Promise<ToastOptions | null> {
  try {
    unwrap(await open(url))
    return null
  } catch (failure) {
    return { type: 'error', title: 'Could not open the link', message: errorMessage(failure) }
  }
}

/**
 * A list row's Vue key. A list can repeat a printing (signature rows are not
 * merged), so the catalog id alone is not unique: the row's place is part of it.
 */
export function cardRowKey(card: ListCardView, index: number): string {
  return `card_${index}_${card.catalogId}`
}

/** Why a commander format deck's archetype cannot be set by hand. */
export const COMMANDER_ARCHETYPE_NOTE = 'Brawl decks take their commander\'s archetype, so it cannot be changed by hand.'
