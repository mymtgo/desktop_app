import type { BestOfView, GameResultView, MatchListQuery, MatchRankView, PlatformView, RatingView } from '@/bindings'
import { signedNumber } from './format'
import { isMtgo } from './platform'
import { rankIconUrl, rankSteps } from './rankIcons'
import { dayToUtcMs } from './timeframes'

/** A column the matches table sorts by (0.x `applySort` names). */
export type SortColumn = MatchListQuery['sort']
export type SortDir = MatchListQuery['sortDir']
export type ResultFilter = NonNullable<MatchListQuery['result']>
export type KindFilter = NonNullable<MatchListQuery['kind']>

/**
 * What a match list page shows: its filters, its sort and its page. `sortBy`
 * null is the unsorted list (newest first, every header showing no arrow).
 */
export type MatchListState = {
  /** Local day `YYYY-MM-DD`, inclusive; null for no bound. */
  from: string | null
  to: string | null
  result: ResultFilter | null
  kind: KindFilter | null
  /** Arena Bo1 or Bo3 only; null for every match. Always null on MTGO. */
  bestOf: BestOfView | null
  /** `none` (no opponent archetype) or an archetype id; null for every match. */
  archetype: string | null
  sortBy: SortColumn | null
  sortDir: SortDir
  page: number
}

const SORT_COLUMNS: SortColumn[] = ['started_at', 'outcome', 'archetype', 'duration', 'game_1', 'game_2', 'game_3']
const BEST_OFS: BestOfView[] = ['bo1', 'bo3']
const RESULTS: ResultFilter[] = ['win', 'loss']
const MTGO_KINDS: KindFilter[] = ['league', 'casual', 'manual']
const ARENA_KINDS: KindFilter[] = ['ranked', 'unranked', 'manual']

/** The result filter's options (0.x "All Results", "Win", "Loss"). */
export const RESULT_OPTIONS: { value: ResultFilter | 'all'; label: string }[] = [
  { value: 'all', label: 'All Results' },
  { value: 'win', label: 'Win' },
  { value: 'loss', label: 'Loss' },
]

type KindOption = { value: KindFilter | 'all'; label: string }

/** The type filter's options (0.x "All Types", "League", "Casual", "Manual"). */
export const KIND_OPTIONS: KindOption[] = [
  { value: 'all', label: 'All Types' },
  { value: 'league', label: 'League' },
  { value: 'casual', label: 'Casual' },
  { value: 'manual', label: 'Manual' },
]

/** Arena has no leagues: Ranked and Casual (the `unranked` kind) take their place. */
export const ARENA_KIND_OPTIONS: KindOption[] = [
  { value: 'all', label: 'All Types' },
  { value: 'ranked', label: 'Ranked' },
  { value: 'unranked', label: 'Casual' },
  { value: 'manual', label: 'Manual' },
]

/** The Arena Bo1/Bo3 filter's options. */
export function bestOfOptions(): { value: BestOfView | 'all'; label: string }[] {
  return [
    { value: 'all', label: 'All kinds' },
    { value: 'bo1', label: 'Bo1' },
    { value: 'bo3', label: 'Bo3' },
  ]
}

/** The type filter's options for a platform. */
export function kindOptions(platform: PlatformView): KindOption[] {
  return isMtgo(platform) ? KIND_OPTIONS : ARENA_KIND_OPTIONS
}

/** An Arena or Endstep row's kind tag, "Bo1" or "Bo3"; `null` for MTGO rows or an unknown kind. */
export function arenaKindLabel(row: { platform: PlatformView; bestOf: BestOfView | null }): string | null {
  if (isMtgo(row.platform) || row.bestOf === null) return null
  return row.bestOf === 'bo1' ? 'Bo1' : 'Bo3'
}

/**
 * The Rank cell's text when the match has no reading: Ranked or Casual; `null` for MTGO rows.
 * `matchType` is the display label (`match_type_label`): Arena's `ranked` arrives as "Ranked".
 */
export function arenaTypeLabel(row: { platform: PlatformView; matchType: string | null }): string | null {
  if (isMtgo(row.platform)) return null
  if (row.matchType === 'Ranked') return 'Ranked'
  if (row.matchType === 'Unranked') return 'Casual'
  return null
}

/**
 * A header click (0.x `updateSort`): a new column starts descending, the same
 * column goes descending, then ascending, then back to unsorted.
 */
export function nextSort(
  current: { sortBy: SortColumn | null; sortDir: SortDir },
  column: SortColumn,
): { sortBy: SortColumn | null; sortDir: SortDir } {
  if (current.sortBy !== column) return { sortBy: column, sortDir: 'desc' }
  if (current.sortDir === 'desc') return { sortBy: column, sortDir: 'asc' }
  return { sortBy: null, sortDir: 'desc' }
}

const GAME_COLUMNS: SortColumn[] = ['game_1', 'game_2', 'game_3']

/**
 * The list as a table with one result column shows it: a single result list
 * (Bo1) hides the game columns, so a `game_*` sort from the URL is unsorted.
 */
export function withoutHiddenSort(state: MatchListState, singleResult: boolean): MatchListState {
  if (!singleResult || state.sortBy === null || !GAME_COLUMNS.includes(state.sortBy)) return state
  return { ...state, sortBy: null, sortDir: 'desc' }
}

/** The command query for a list state. Unsorted is newest first. */
export function matchListQuery(state: MatchListState): MatchListQuery {
  const range = dateRange(state.from, state.to)
  return {
    from: range.from,
    to: range.to,
    result: state.result,
    kind: state.kind,
    bestOf: state.bestOf,
    archetype: state.archetype,
    sort: state.sortBy ?? 'started_at',
    sortDir: state.sortBy === null ? 'desc' : state.sortDir,
    page: state.page,
  }
}

function day(value: unknown): string | null {
  if (typeof value !== 'string' || !/^\d{4}-\d{2}-\d{2}$/.test(value)) return null
  return dayToUtcMs(value) === null ? null : value
}

/**
 * The date filter's days: blank or not a calendar day means no bound, and a
 * range given backwards is turned round. Both days are inclusive (0.x: from
 * the start of the first day to the end of the last).
 */
export function dateRange(from: unknown, to: unknown): { from: string | null; to: string | null } {
  const first = day(from)
  const last = day(to)
  if (first !== null && last !== null && first > last) return { from: last, to: first }
  return { from: first, to: last }
}

/** True when a filter narrows the list (a sort or a page does not). */
export function isFiltered(state: MatchListState): boolean {
  return (
    state.from !== null || state.to !== null || state.result !== null || state.kind !== null ||
    state.bestOf !== null ||
    state.archetype !== null
  )
}

/** An archetype filter from the URL: `none` or a positive id; anything else is none. */
function archetypeValue(value: unknown): string | null {
  if (value === 'none') return 'none'
  return positive(value) === null ? null : (value as string)
}

function pick<T extends string>(options: T[], value: unknown): T | null {
  return options.find((option) => option === value) ?? null
}

function positive(value: unknown): number | null {
  if (typeof value !== 'string' || !/^\d+$/.test(value)) return null
  const number = Number(value)
  return number > 0 ? number : null
}

/** The URL keys a match list owns; anything else in the URL belongs to the page. */
const LIST_KEYS = ['from', 'to', 'result', 'type', 'bo', 'archetype', 'sort', 'dir', 'page']

/**
 * The list state from the URL query (`from`, `to`, `result`, `type`,
 * `bo` (Arena only), `archetype`, `sort`, `dir`, `page`, the 0.x filter names without their `filter_` prefix).
 * Anything unknown falls back to its default; a type the platform lacks
 * (`ranked` on MTGO, `league` on Arena) is All Types.
 */
export function matchListFromUrl(query: Record<string, unknown>, platform: PlatformView = 'mtgo'): MatchListState {
  const range = dateRange(query.from, query.to)
  const sortBy = pick(SORT_COLUMNS, query.sort)
  return {
    from: range.from,
    to: range.to,
    result: pick(RESULTS, query.result),
    kind: pick(isMtgo(platform) ? MTGO_KINDS : ARENA_KINDS, query.type),
    bestOf: isMtgo(platform) ? null : pick(BEST_OFS, query.bo),
    archetype: archetypeValue(query.archetype),
    sortBy,
    sortDir: sortBy !== null && query.dir === 'asc' ? 'asc' : 'desc',
    page: positive(query.page) ?? 1,
  }
}

/** The URL query for a list state, defaults left out. */
export function matchListUrlQuery(state: MatchListState): Record<string, string> {
  const url: Record<string, string> = {}
  const range = dateRange(state.from, state.to)
  if (range.from !== null) url.from = range.from
  if (range.to !== null) url.to = range.to
  if (state.result !== null) url.result = state.result
  if (state.kind !== null) url.type = state.kind
  if (state.bestOf !== null) url.bo = state.bestOf
  if (state.archetype !== null) url.archetype = state.archetype
  if (state.sortBy !== null) {
    url.sort = state.sortBy
    if (state.sortDir === 'asc') url.dir = 'asc'
  }
  if (state.page > 1) url.page = String(state.page)
  return url
}

/** The page's URL query with the list's keys replaced by `state`'s. */
export function withListQuery<T>(current: Record<string, T>, state: MatchListState): Record<string, T | string> {
  const next: Record<string, T | string> = {}
  for (const [key, value] of Object.entries(current)) {
    if (!LIST_KEYS.includes(key)) next[key] = value
  }
  return { ...next, ...matchListUrlQuery(state) }
}

function plural(count: number, unit: string): string {
  return `${count} ${unit}${count === 1 ? '' : 's'}`
}

/**
 * A match's length (0.x `matchTime`, Carbon's absolute `diffForHumans`): the
 * largest whole unit, never zero. Null while the match has not ended.
 */
export function matchDurationText(startedAtMs: number, endedAtMs: number | null): string | null {
  if (endedAtMs === null) return null
  const seconds = Math.floor(Math.abs(endedAtMs - startedAtMs) / 1000)
  const minutes = Math.floor(seconds / 60)
  const hours = Math.floor(minutes / 60)
  const days = Math.floor(hours / 24)
  if (days >= 7) return plural(Math.floor(days / 7), 'week')
  if (days > 0) return plural(days, 'day')
  if (hours > 0) return plural(hours, 'hour')
  if (minutes > 0) return plural(minutes, 'minute')
  return plural(Math.max(1, seconds), 'second')
}

function twoDigits(value: number): string {
  return String(value).padStart(2, '0')
}

/**
 * Whether a row offers its right-click menu. A match still in progress is not
 * stored yet, so its edits (notes, archetype, "Remove from stats") would be refused.
 */
export function rowMenuEnabled(row: { state: string }): boolean {
  return row.state !== 'in_progress'
}

/** English short month names, so dates read the same whatever the machine's locale. */
const SHORT_MONTHS = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec']

/** "2 Oct 2026" from a day of the month, a month from 1 to 12 and a year. */
export function dayMonthYearText(day: number, month: number, year: number): string {
  return `${day} ${SHORT_MONTHS[month - 1]} ${year}`
}

/**
 * A match's start on the local clock, the month by name: "2 Oct 2026, 3:05pm".
 * 0.x showed `d/m/Y g:ia`, which many players read month first (#390).
 */
export function matchDateText(ms: number): string {
  const date = new Date(ms)
  const hour = date.getHours() % 12 === 0 ? 12 : date.getHours() % 12
  const suffix = date.getHours() < 12 ? 'am' : 'pm'
  return (
    `${dayMonthYearText(date.getDate(), date.getMonth() + 1, date.getFullYear())}, ` +
    `${hour}:${twoDigits(date.getMinutes())}${suffix}`
  )
}

export type GameCell = { text: 'Win' | 'Loss' | '?'; tone: string; side: '(OTP)' | '(OTD)' | null }

/**
 * The Game 1 to 3 cells: "Win" or "Loss", with "(OTP)" or "(OTD)" when known;
 * "?" for a game with no known winner (it keeps its place); null for a game not played.
 */
export function gameCells(results: GameResultView[]): (GameCell | null)[] {
  return [0, 1, 2].map((index) => {
    const game = results[index]
    if (game === undefined) return null
    const side = game.onPlay === null ? null : game.onPlay ? '(OTP)' : '(OTD)'
    if (game.won === null) return { text: '?', tone: 'text-muted-foreground', side }
    return {
      text: game.won ? 'Win' : 'Loss',
      tone: game.won ? 'text-success' : 'text-destructive',
      side,
    }
  })
}

/**
 * An Endstep row's Rank cell: the rating after the match, and its signed
 * change with a trend (up green, down red, no change muted and no icon).
 */
export function endstepRatingCell(rating: RatingView): {
  after: string
  change: string
  trend: 'up' | 'down' | 'flat'
  tone: string
} {
  const trend = rating.delta > 0 ? 'up' : rating.delta < 0 ? 'down' : 'flat'
  const tone = trend === 'up' ? 'text-success' : trend === 'down' ? 'text-destructive' : 'text-muted-foreground'
  return { after: String(rating.after), change: signedNumber(rating.delta), trend, tone }
}

/** The empty list's text: Endstep says how to get matches. */
export function noMatchesRecordedText(platform: PlatformView): string {
  if (platform === 'endstep') {
    return 'No Endstep matches yet. Switch on Endstep in Settings, Integrations, and install the browser extension.'
  }
  return 'No matches recorded'
}

/** The empty list's text on a deck that already exists: no setup advice. */
export const NO_DECK_MATCHES_TEXT = 'No matches with this deck yet'

/**
 * An Arena row's Rank cell: the rank icon (when the match has a reading) and its text; `null` for MTGO rows.
 * A match's stored rank is always the constructed reading, so the icon is the constructed one.
 */
export function arenaRankCell(row: {
  platform: PlatformView
  matchType: string | null
  rank?: MatchRankView | null
}): { iconUrl: string | null; label: string; text: string; muted: boolean } | null {
  // No rank means casual (owner, 2026-11-06): a casual match leaves the cell empty.
  const type = arenaTypeLabel(row)
  if (type === null || type === 'Casual') return null
  // A ranked match with no earlier reading still says Ranked, so it is not read as casual.
  if (!row.rank) return { iconUrl: null, label: type, text: type, muted: true }
  const steps = rankSteps(row.rank.class, row.rank.level, row.rank.step)
  const url = steps === null ? '' : rankIconUrl('constructed', steps)
  // The icon alone, named in its tooltip; the label only when there is no icon to show.
  if (url) return { iconUrl: url, label: row.rank.label, text: '', muted: false }
  return { iconUrl: null, label: row.rank.label, text: row.rank.label, muted: false }
}
