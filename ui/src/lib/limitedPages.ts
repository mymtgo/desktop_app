/**
 * The Limited event pages' pure helpers past the draft review (0.x
 * `pages/limited/{Deck,Cards,Matches}.vue` and their partials): the deck's
 * versions, groups, curve and changes; the cards table's text; and the
 * matches page's text and client-side sort.
 */
import type {
  CardRowsSummaryView,
  ColourCount,
  CurvePoint,
  EvolutionSummaryView,
  LimitedCardRowView,
  LimitedCardView,
  LimitedMatchesKpisView,
  ListCardView,
  MatchSummary,
  PoolView,
  RegisteredVersionView,
} from '@/bindings'
import { normalizeType, TYPE_ORDER } from './cardTypes'
import { cardFor, colourLetters, type LimitedCards, NO_VALUE, ordinalLabel, timeLabel } from './limited'
import { matchDurationText, type SortColumn, type SortDir } from './matchList'

function plural(count: number, word: string, many = `${word}s`): string {
  return count === 1 ? word : many
}

// ---- Pool status -------------------------------------------------------------

/** 0.x `POOL_STATUS_ORDER`. */
export const POOL_STATUS_ORDER = ['main', 'side', 'pool', 'cut'] as const

const POOL_STATUS_LABELS: Record<string, string> = { main: 'Main', side: 'Side', pool: 'Pool', cut: 'Cut' }

export function poolStatusLabel(status: string): string {
  return POOL_STATUS_LABELS[status] ?? status
}

/**
 * Pool status renders as a tinted outline chip (0.x `poolStatusTint`): these
 * sit in a dense table beside plain text, so a filled badge reads as noise.
 */
export function poolStatusTint(status: string): string {
  switch (status) {
    case 'main':
      return 'border-sky-400/30 text-sky-300'
    case 'cut':
      return 'border-rose-500/30 text-rose-300'
    default:
      return 'border-border text-muted-foreground'
  }
}

// ---- Deck page ---------------------------------------------------------------

/** Card images in a labelled group (0.x `CardImageGroup`). */
export type CardImageGroup = { key: string; label: string; cards: ListCardView[] }

/** One card at a quantity, as the decklist's card images take it (`#id`, no art, no colours without card data). */
export function imageCard(cards: LimitedCards, catalogId: number, quantity: number): ListCardView {
  const card = cardFor(cards, catalogId)
  return {
    catalogId,
    name: card.name,
    typeLine: card.type,
    colors: colourLetters(card.colors),
    image: card.image,
    artCrop: card.artCrop,
    localImage: card.localImage,
    localArtCrop: card.localArtCrop,
    cmc: card.cmc,
    manaCost: card.manaCost,
    quantity,
    sideboard: false,
    known: card.resolved,
  }
}

/** The version the page shows: the one picked in the strip, else the current build. */
export function shownVersion(versions: RegisteredVersionView[], selected: number | null): RegisteredVersionView | null {
  const current = versions.find((version) => version.isCurrent) ?? null
  if (selected === null) return current
  return versions.find((version) => version.index === selected) ?? current
}

function byCurve(a: ListCardView, b: ListCardView): number {
  return (a.cmc ?? 0) - (b.cmc ?? 0) || a.name.localeCompare(b.name)
}

/** A version's main deck, grouped by card type like a constructed decklist. */
export function mainGroups(version: RegisteredVersionView | null, cards: LimitedCards): CardImageGroup[] {
  const groups = new Map<string, ListCardView[]>()
  for (const row of version?.mainCards ?? []) {
    const card = imageCard(cards, row.catalogId, row.quantity)
    const type = normalizeType(card.typeLine ?? 'Other')
    groups.set(type, [...(groups.get(type) ?? []), card])
  }
  return [...groups.entries()]
    .sort(([a], [b]) => (TYPE_ORDER[a] ?? 99) - (TYPE_ORDER[b] ?? 99))
    .map(([type, list]) => ({ key: type, label: type, cards: list.sort(byCurve) }))
}

/** Every copy in the pool, main deck included: what the boosters (or picks) gave to build with. */
export function wholePool(pool: PoolView, cards: LimitedCards): ListCardView[] {
  return pool.groups.flatMap((group) => group.cards.map((card) => imageCard(cards, card.catalogId, card.quantity)))
}

/**
 * Every pool copy the version left out of the main deck, grouped by colour.
 * Copies from a sealed run's added booster get their own group first. With
 * no version, the run's own pool.
 */
export function poolGroups(version: RegisteredVersionView | null, pool: PoolView, cards: LimitedCards): CardImageGroup[] {
  const added: ListCardView[] = []
  const groups = (version?.pool.groups ?? pool.groups).map((group) => {
    const list: ListCardView[] = []
    for (const card of group.cards) {
      const left = card.quantity - card.mainQty
      const fromBooster = Math.min(card.added, Math.max(left, 0))
      if (fromBooster > 0) added.push(imageCard(cards, card.catalogId, fromBooster))
      if (left - fromBooster > 0) list.push(imageCard(cards, card.catalogId, left - fromBooster))
    }
    return { key: group.key, label: group.label, cards: list.sort(byCurve) }
  })
  const first = added.length > 0 ? [{ key: 'added', label: 'Added booster', cards: added.sort(byCurve) }] : []
  return [...first, ...groups].filter((group) => group.cards.length > 0)
}

const CURVE_COLOURS = ['W', 'U', 'B', 'R', 'G', 'C'] as const

/**
 * The curve (7 and up in one bucket) and colour split of the nonland cards,
 * in the shape the deck pages' chart takes (0.x `CurveAndColours` counted
 * them in the component). A card without colours counts as colourless.
 */
export function curveAndColours(cards: ListCardView[]): { curve: CurvePoint[]; colours: ColourCount[]; total: number } {
  const nonLand = cards.filter((card) => !card.typeLine?.includes('Land'))
  const buckets = new Map<number, number>()
  for (const card of nonLand) {
    const cmc = Math.min(Math.floor(card.cmc ?? 0), 7)
    buckets.set(cmc, (buckets.get(cmc) ?? 0) + card.quantity)
  }
  const curve = [...buckets.entries()].sort((a, b) => a[0] - b[0]).map(([cmc, count]) => ({ cmc, count }))
  const colours = CURVE_COLOURS.map((colour) => ({
    colour,
    count: nonLand
      .filter((card) => (colour === 'C' ? card.colors.length === 0 : card.colors.includes(colour)))
      .reduce((sum, card) => sum + card.quantity, 0),
  }))
  const total = nonLand.reduce((sum, card) => sum + card.quantity, 0)
  return { curve, colours, total }
}

/** "42 drafted · 23 spells registered + 17 basics · 2 registered versions · deck built 11:05 → 12:40". */
export function deckSummaryText(summary: EvolutionSummaryView, sealed: boolean): string {
  let text = `${summary.drafted} ${sealed ? 'in pool' : 'drafted'}`
  if (summary.added > 0) text += ` (${summary.added} from the added booster)`
  text += ` · ${summary.mainSpells} spells registered + ${summary.basics} basics`
  text += ` · ${summary.versionCount} registered ${plural(summary.versionCount, 'version')}`
  if (summary.firstRegisteredAtMs !== null) {
    text += ` · deck built ${timeLabel(summary.firstRegisteredAtMs)} → ${timeLabel(summary.lastRegisteredAtMs)}`
  }
  return text
}

/** The version strip's switch: `v1`, `v2`, ... */
export function versionOptions(versions: RegisteredVersionView[]): { value: string; label: string }[] {
  return versions.map((version) => ({ value: String(version.index), label: `v${version.index}` }))
}

/** The version strip's text (0.x `VersionStrip`). */
export function versionLine(version: RegisteredVersionView): {
  title: string
  matches: string
  time: string
  counts: string
} {
  return {
    title: `Registered build v${version.index}${version.isCurrent ? ' · current' : ''}`,
    matches: version.matchLabels.join(', ') || 'unplayed',
    time: timeLabel(version.capturedAtMs),
    counts: `${version.main} main · ${version.side} side`,
  }
}

export type ChangeRow = {
  key: string
  sign: '+' | '−'
  quantity: number
  name: string
  /** The full card, for the hover preview. */
  image: string | null
  localImage: string | null
  artCrop: string | null
  localArtCrop: string | null
  colors: string
}

export type DeckChanges = {
  /** `none`: no version; `first`: nothing before it; `identical`: no changes. */
  state: 'none' | 'first' | 'identical' | 'changed'
  /** The version compared against. */
  previous: number | null
  sections: { key: string; label: string; rows: ChangeRow[] }[]
}

/** What changed between the shown version and the one before it (0.x `DeckChanges`). */
export function deckChanges(
  versions: RegisteredVersionView[],
  cards: LimitedCards,
  selected: number | null,
): DeckChanges {
  const active = versions.find((version) => version.index === selected) ?? null
  if (!active) return { state: 'none', previous: null, sections: [] }
  const previous = versions.find((version) => version.index === active.index - 1) ?? null
  if (!previous) return { state: 'first', previous: null, sections: [] }

  const rows = (entries: { catalogId: number; quantity: number }[], sign: '+' | '−', prefix: string): ChangeRow[] =>
    entries.map((entry) => {
      const card = cardFor(cards, entry.catalogId)
      return {
        key: `${prefix}${sign}${entry.catalogId}`,
        sign,
        quantity: entry.quantity,
        name: card.name,
        image: card.image,
        localImage: card.localImage,
        artCrop: card.artCrop,
        localArtCrop: card.localArtCrop,
        colors: card.colors,
      }
    })
  const sections = [
    {
      key: 'main',
      label: 'Main deck',
      rows: [...rows(active.diffMain.added, '+', 'm'), ...rows(active.diffMain.removed, '−', 'm')],
    },
    {
      key: 'side',
      label: 'Sideboard',
      rows: [...rows(active.diffSide.added, '+', 's'), ...rows(active.diffSide.removed, '−', 's')],
    },
  ].filter((section) => section.rows.length > 0)
  return { state: sections.length ? 'changed' : 'identical', previous: previous.index, sections }
}

// ---- Cards page --------------------------------------------------------------

/**
 * The status filter. Nothing registered yet means every card reads "Pool",
 * so that segment only shows once a row carries the status.
 */
export function cardFilterOptions(rows: LimitedCardRowView[]): { value: string; label: string }[] {
  const present = new Set(rows.map((row) => row.status))
  const statuses = POOL_STATUS_ORDER.filter((status) => status !== 'pool' || present.has('pool'))
  return [{ value: 'all', label: 'All' }, ...statuses.map((status) => ({ value: status, label: poolStatusLabel(status) }))]
}

/** "30 distinct cards drafted · game stats from 4 games with this deck · prior columns from 2 other HOB drafts". */
export function cardsSummaryText(summary: CardRowsSummaryView, sealed: boolean, setCode: string | null): string {
  let text = `${summary.distinct} distinct ${plural(summary.distinct, 'card')} ${sealed ? 'in pool' : 'drafted'}`
  text += ` · game stats from ${summary.games} ${plural(summary.games, 'game')} with this deck`
  if (summary.otherDrafts) {
    text += ` · prior columns from ${summary.otherDrafts} other ${setCode ?? ''} ${plural(summary.otherDrafts, 'draft')}`
  }
  return text
}

export function cardsEmptyText(sealed: boolean): { title: string; body: string } {
  return sealed
    ? { title: 'No pool recorded yet.', body: 'Your pool appears here once a match with this deck has started.' }
    : { title: 'No picks recorded.', body: 'Cards appear here once a draft has been tracked for this event.' }
}

/** "Creature · common" under the card's name; empty without card data. */
export function cardSubline(card: LimitedCardView): string {
  return [card.type, card.rarity].filter(Boolean).join(' · ')
}

export function winPctText(value: number | null): string {
  return value !== null ? `${value}%` : NO_VALUE
}

export function winPctTone(value: number | null): string {
  if (value === null) return 'text-muted-foreground'
  return value >= 50 ? 'text-emerald-400' : 'text-rose-400'
}

/** The Prior drafts cell: "taken 2× · avg P1p3", "wheeled in 2 of 3", "passed" or a dash. */
export function priorDraftsText(row: LimitedCardRowView): string {
  if (row.priorDrafts === 0) return NO_VALUE
  if (row.priorTaken > 0) {
    const avg = row.priorAvgOrdinal !== null ? ` · avg ${ordinalLabel(Math.round(row.priorAvgOrdinal))}` : ''
    return `taken ${row.priorTaken}×${avg}`
  }
  if (row.priorWheeled > 0) return `wheeled in ${row.priorWheeled} of ${row.priorDrafts}`
  return 'passed'
}

// ---- Matches page ------------------------------------------------------------

/** "3 league matches · queued 11:00 · finished 11:32 · 32 min total". */
export function matchesHeaderText(count: number, kpis: LimitedMatchesKpisView): string {
  let text = `${count} league ${plural(count, 'match', 'matches')}`
  if (kpis.queuedAtMs !== null) {
    text += ` · queued ${timeLabel(kpis.queuedAtMs)} · finished ${timeLabel(kpis.finishedAtMs)} · ${kpis.totalMinutes ?? NO_VALUE} min total`
  }
  return text
}

/** The Avg match tile: whole minutes, "22m". */
export function avgMatchText(seconds: number | null): string {
  return seconds !== null ? `${Math.round(seconds / 60)}m` : NO_VALUE
}

export function totalMinutesText(minutes: number | null): string {
  return minutes !== null ? `${minutes}m` : NO_VALUE
}

/** A win outranks a loss, and both outrank a game never played or with no known winner. */
function gameRank(match: MatchSummary, index: number): number {
  const game = match.gameResults[index]
  if (game === undefined || game.won === null) return 0
  return game.won ? 2 : 1
}

const DURATION_UNIT_SECONDS: Record<string, number> = { second: 1, minute: 60, hour: 3600, day: 86400, week: 604800 }

/**
 * The elapsed time behind the shown label ("5 minutes"), as 0.x sorted it:
 * two matches that read the same tie, whatever their milliseconds. A match
 * still running has no duration and sorts lowest (0.x: SQL NULLs).
 */
function durationSeconds(match: MatchSummary): number {
  const label = matchDurationText(match.startedAtMs, match.endedAtMs)
  if (label === null) return -1
  let total = 0
  for (const [, amount, unit] of label.matchAll(/(\d+)\s*([a-z]+)/gi)) {
    total += Number(amount) * (DURATION_UNIT_SECONDS[unit.toLowerCase().replace(/s$/, '')] ?? 0)
  }
  return total
}

function sortValue(match: MatchSummary, column: SortColumn): number {
  switch (column) {
    case 'outcome':
      return match.outcome === 'win' ? 1 : 0
    case 'game_1':
      return gameRank(match, 0)
    case 'game_2':
      return gameRank(match, 1)
    case 'game_3':
      return gameRank(match, 2)
    case 'duration':
      return durationSeconds(match)
    default:
      return match.startedAtMs
  }
}

/**
 * The whole run is in the payload, so the Matches page sorts on the client
 * (0.x). Unsorted keeps the command's order.
 */
export function sortLimitedMatches(rows: MatchSummary[], sortBy: SortColumn | null, sortDir: SortDir): MatchSummary[] {
  if (sortBy === null) return rows
  const direction = sortDir === 'asc' ? 1 : -1
  return [...rows].sort((a, b) => direction * (sortValue(a, sortBy) - sortValue(b, sortBy)))
}
