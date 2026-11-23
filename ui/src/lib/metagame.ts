import { keepPreviousData, useQuery } from '@tanstack/vue-query'
import { computed, ref, type Ref } from 'vue'
import { commands, type GlobalState, type PlatformView, type MetagameTileView, type SeenCardView, type TileCardView } from '@/bindings'
import { readStored, writeStored, type KeyValueStorage } from './browserStorage'
import { unwrap } from './commands'

// The Metagame page and the archetype detail (archetypes spec sections 9
// and 10). The numbers come from the backend; this file formats them.

/** The default timeframe (spec section 9). */
export const DEFAULT_TIMEFRAME = 'last30'

/** The timeframe keys the backend reads. */
export const TIMEFRAMES = ['last30', 'last7', 'alltime'] as const

function isTimeframe(value: unknown): value is string {
  return typeof value === 'string' && (TIMEFRAMES as readonly string[]).includes(value)
}

/**
 * The timeframe a metagame or archetype page shows (issue #326): the route's
 * when it names one, else the one last chosen, else the default. Coming back
 * to the Metagame page without a query keeps "All time".
 */
export function chosenTimeframe(value: unknown, remembered: string | null): string {
  if (isTimeframe(value)) return value
  return isTimeframe(remembered) ? remembered : DEFAULT_TIMEFRAME
}

/** The Arena rank band keys, in ladder order (the backend's `RankBand` keys). */
export const RANK_KEYS = ['spark', 'bronze', 'silver', 'gold', 'platinum', 'diamond', 'mythic', 'unranked'] as const

/**
 * The month and rank the request sends. Only Arena has them; on MTGO both
 * are always null, so MTGO's request is unchanged. A value that is not a
 * month (`2026-11`) or a rank key is dropped (the backend then shows the
 * current month, All).
 */
export function arenaSelection(
  platform: PlatformView,
  month: unknown,
  rank: unknown,
): { month: string | null; rank: string | null } {
  if (platform !== 'arena') return { month: null, rank: null }
  return {
    month: typeof month === 'string' && /^\d{4}-\d{2}$/.test(month) ? month : null,
    rank: typeof rank === 'string' && (RANK_KEYS as readonly string[]).includes(rank) ? rank : null,
  }
}

/** The rank icon's file name (without extension) for a band key. */
export function rankIconFile(key: string): string {
  if (key === 'mythic' || key === 'unranked') return `constructed_${key}`
  return `constructed_${key}_t1`
}

/** `12.3%`. */
export function shareText(share: number): string {
  return `${share.toFixed(1)}%`
}

/**
 * The global column: blank while the shares are not here (offline,
 * loading, failed), "-" for a tile the website cannot list (manual,
 * fallback, retired, unlisted).
 */
export function globalText(tile: MetagameTileView, state: GlobalState): string {
  if (!tile.comparable) return '-'
  if (state !== 'ready') return ''
  return tile.globalShare === null ? '-' : shareText(tile.globalShare)
}

/** The difference column: `+3.8 pts`, `-2.3 pts`, `0.0 pts`, or as `globalText`. */
export function differenceText(tile: MetagameTileView, state: GlobalState): string {
  if (!tile.comparable) return '-'
  if (state !== 'ready') return ''
  if (tile.difference === null) return '-'
  const sign = tile.difference > 0 ? '+' : ''
  return `${sign}${tile.difference.toFixed(1)} pts`
}

/**
 * The card whose art fills a tile (the deck card's cover art): its first
 * card with an art crop.
 */
export function tileBackdrop(tile: MetagameTileView): TileCardView | null {
  return tile.cards.find((card) => card.art || card.localArt) ?? null
}

/** The tone of a difference: above the field, below it, or level. */
export function differenceTone(tile: MetagameTileView, state: GlobalState): 'up' | 'down' | 'none' {
  if (!tile.comparable || state !== 'ready' || tile.difference === null || tile.difference === 0) return 'none'
  return tile.difference > 0 ? 'up' : 'down'
}

/** The line under the grid while the global column is blank. */
export function globalNotice(state: GlobalState): string | null {
  if (state === 'unavailable') return 'Global shares need a connection.'
  if (state === 'loading') return 'Loading global shares…'
  return null
}

/** The shared header: "{Format} metagame · {timeframe}". */
export function headerTitle(formatLabel: string, timeframeLabel: string): string {
  return `${formatLabel} metagame · ${timeframeLabel}`
}

/** `9 Feb 2027 to 10 Mar 2027`, or null for all time. */
export function rangeText(from: string | null, to: string | null): string | null {
  if (!from || !to) return null
  return `${dayText(from)} to ${dayText(to)}`
}

const MONTHS = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec']

/** `2027-03-09` as `9 Mar 2027`. */
export function dayText(day: string): string {
  const [year, month, date] = day.split('-').map(Number)
  if (!year || !month || !date) return day
  return `${date} ${MONTHS[month - 1]} ${year}`
}

/** "n match(es) without an opponent archetype", or null without any. */
export function withoutArchetypeText(count: number): string | null {
  if (count === 0) return null
  return `${count} ${count === 1 ? 'match' : 'matches'} without an opponent archetype`
}

/** The saved image's name: `modern-metagame-last30`. */
export function imageName(format: string, timeframe: string): string {
  return `${format}-metagame-${timeframe}`
}

/** The line under a card seen: the most copies in one game, then the matches. */
export function seenCaption(card: { mostCopies: number; matches: number }): string {
  return `up to ${card.mostCopies} · ${card.matches} ${card.matches === 1 ? 'match' : 'matches'}`
}

/** Cards seen split by the typical lists ("Not in typical lists" apart). */
export function splitSeen(cards: SeenCardView[]): { typical: SeenCardView[]; other: SeenCardView[] } {
  return {
    typical: cards.filter((card) => card.inTypical),
    other: cards.filter((card) => !card.inTypical),
  }
}

// ---- The remembered format ------------------------------------------------------

export const METAGAME_FORMAT_KEY = 'mymtgo.metagame.format'

export function readRememberedFormat(storage: KeyValueStorage): string | null {
  const value = readStored(storage, METAGAME_FORMAT_KEY)
  return value && /^[a-z_]+$/.test(value) ? value : null
}

export function writeRememberedFormat(storage: KeyValueStorage, format: string) {
  writeStored(storage, METAGAME_FORMAT_KEY, format)
}

export const METAGAME_TIMEFRAME_KEY = 'mymtgo.metagame.timeframe'

export function readRememberedTimeframe(storage: KeyValueStorage): string | null {
  const value = readStored(storage, METAGAME_TIMEFRAME_KEY)
  return isTimeframe(value) ? value : null
}

export function writeRememberedTimeframe(storage: KeyValueStorage, timeframe: string) {
  if (isTimeframe(timeframe)) writeStored(storage, METAGAME_TIMEFRAME_KEY, timeframe)
}

/** The Arena month and rank a link keeps (both null or absent on MTGO). */
export type ArenaSelection = { month?: string | null; rank?: string | null }

function addSelection(query: Record<string, string>, selection: ArenaSelection) {
  if (selection.month) query.month = selection.month
  if (selection.rank) query.rank = selection.rank
}

/**
 * The name of the window in the header and share image: the shown month's
 * label on Arena (the key when the months do not list it), the timeframe's
 * label otherwise.
 */
export function windowLabel(
  arena: boolean,
  months: { key: string; label: string }[],
  month: string | null,
  timeframeLabel: string,
): string {
  if (!arena) return timeframeLabel
  return months.find((entry) => entry.key === month)?.label ?? month ?? timeframeLabel
}

/** The Metagame page with the remembered format, timeframe, search and Arena month and rank (the archetype page's way back). */
export function metagameLocation(
  format: string | null,
  timeframe: string,
  search = '',
  selection: ArenaSelection = {},
): { path: string; query: Record<string, string> } {
  const query: Record<string, string> = { timeframe }
  if (search !== '') query.q = search
  addSelection(query, selection)
  return { path: format ? `/metagame/${format}` : '/metagame', query }
}

/** The search text kept in a route's `q` query (empty when there is none). */
export function searchQuery(value: unknown): string {
  return typeof value === 'string' ? value : ''
}

/** A tile name as matched by the search: the "(Provisional)" suffix is not part of it. */
function searchableName(name: string): string {
  return name.replace(/\s*\(provisional\)\s*$/i, '')
}

/** The tiles whose name holds the search text anywhere, ignoring case and the "(Provisional)" suffix. */
export function filterTiles<T extends { name: string }>(tiles: T[], search: string): T[] {
  const needle = search.trim().toLowerCase()
  if (needle === '') return tiles
  return tiles.filter((tile) => searchableName(tile.name).toLowerCase().includes(needle))
}

// ---- The archetype page's tabs (issue #326) -----------------------------------

export const ARCHETYPE_TABS = [
  { key: 'decklist', label: 'Decklist' },
  { key: 'matches', label: 'Matches' },
  { key: 'sideboard-guides', label: 'Sideboard guides' },
] as const

export type ArchetypeTab = (typeof ARCHETYPE_TABS)[number]['key']

/** A route's tab, Decklist for anything else. */
export function archetypeTab(value: unknown): ArchetypeTab {
  const found = ARCHETYPE_TABS.find((tab) => tab.key === value)
  return found ? found.key : 'decklist'
}

/**
 * One archetype on a tab, keeping the timeframe. The side menu uses it with
 * the open tab; the list version belongs to one archetype, so it is not kept.
 */
export function archetypeLocation(
  id: number,
  tab: ArchetypeTab,
  timeframe: string,
  version: string | null = null,
  search = '',
  selection: ArenaSelection = {},
): { name: 'archetype'; params: { id: string; tab: string }; query: Record<string, string> } {
  const query: Record<string, string> = { timeframe }
  if (version !== null) query.version = version
  if (search !== '') query.q = search
  addSelection(query, selection)
  return { name: 'archetype', params: { id: String(id), tab }, query }
}

// ---- Queries ------------------------------------------------------------------

export const metagameKeys = {
  all: ['metagame'] as const,
  page: (format: string | null, timeframe: string, month: string | null = null, rank: string | null = null) =>
    ['metagame', 'page', format, timeframe, month, rank] as const,
  detail: (id: number, timeframe: string) => ['metagame', 'detail', id, timeframe] as const,
  decklists: (id: number) => ['metagame', 'decklists', id] as const,
}

/** `enabled` false holds the fetch (the archetype page waits for its format). */
export function useMetagame(
  format: Ref<string | null>,
  timeframe: Ref<string>,
  enabled: Ref<boolean> = ref(true),
  month: Ref<string | null> = ref(null),
  rank: Ref<string | null> = ref(null),
) {
  return useQuery({
    queryKey: computed(() => metagameKeys.page(format.value, timeframe.value, month.value, rank.value)),
    queryFn: async ({ queryKey }) => unwrap(await commands.metagame(queryKey[2], queryKey[3], queryKey[4], queryKey[5])),
    placeholderData: keepPreviousData,
    enabled,
  })
}

export function useArchetypeDetail(id: Ref<number>, timeframe: Ref<string>) {
  return useQuery({
    queryKey: computed(() => metagameKeys.detail(id.value, timeframe.value)),
    queryFn: async ({ queryKey }) => unwrap(await commands.archetypeDetail(queryKey[2], queryKey[3])),
    placeholderData: keepPreviousData,
  })
}

export function useArchetypeDecklists(id: Ref<number>) {
  return useQuery({
    queryKey: computed(() => metagameKeys.decklists(id.value)),
    queryFn: async ({ queryKey }) => unwrap(await commands.archetypeDecklists(queryKey[2])),
  })
}

/**
 * The side menu's search and format (issue #326): 0.x's archetype sidebar
 * shared the archetype index's filters, so this one shares the manage
 * screen's remembered ones; with none remembered it opens on the
 * archetype's own format.
 */
export function sidebarFilters(
  remembered: { search: string; format: string } | null,
  archetypeFormat: string | null,
): { search: string; format: string } {
  return remembered ?? { search: '', format: archetypeFormat ?? '' }
}
