import type { OpponentSortKey, RecordView } from '@/bindings'

export type OpponentTag = 'nemesis' | 'rival'

/**
 * 0.x `getTag`: with two or more matches, a win rate of 39% or less is a
 * nemesis and up to 60% a rival. Draws and unknown results count in the
 * total, so the rate is the record's own.
 */
export function opponentTag(record: Pick<RecordView, 'total' | 'winRate'>): OpponentTag | null {
  if (record.total < 2) return null
  if (record.winRate <= 39) return 'nemesis'
  if (record.winRate <= 60) return 'rival'
  return null
}

export function tagTooltip(tag: OpponentTag): string {
  return tag === 'nemesis'
    ? 'Your win rate is under 40% against this opponent'
    : 'Your win rate is between 40-60% against this opponent'
}

const MINUTE = 60_000
const HOUR = 60 * MINUTE
const DAY = 24 * HOUR

function plural(count: number, unit: string): string {
  return `${count} ${unit}${count === 1 ? '' : 's'} ago`
}

/** "5 minutes ago", "2 weeks ago" (Carbon `diffForHumans`); a time not yet reached is "just now". */
export function relativeTime(ms: number, now: number): string {
  const diff = now - ms
  if (diff < MINUTE) return 'just now'
  if (diff < HOUR) return plural(Math.floor(diff / MINUTE), 'minute')
  if (diff < DAY) return plural(Math.floor(diff / HOUR), 'hour')
  if (diff < 7 * DAY) return plural(Math.floor(diff / DAY), 'day')
  if (diff < 30 * DAY) return plural(Math.floor(diff / (7 * DAY)), 'week')
  if (diff < 365 * DAY) return plural(Math.floor(diff / (30 * DAY)), 'month')
  return plural(Math.floor(diff / (365 * DAY)), 'year')
}

export const OPPONENT_SORTS: { value: OpponentSortKey; label: string }[] = [
  { value: 'most_played', label: 'Most played' },
  { value: 'winrate_asc', label: 'Win rate ↑ (worst first)' },
  { value: 'winrate_desc', label: 'Win rate ↓ (best first)' },
  { value: 'most_recent', label: 'Most recent' },
]

/** What the Opponents page shows; it lives in the URL so back and forward step through it. */
export type OpponentsState = {
  search: string
  sort: OpponentSortKey
  /** A format code, or null for every format. */
  format: string | null
  page: number
}

export const DEFAULT_OPPONENTS: OpponentsState = { search: '', sort: 'most_played', format: null, page: 1 }

function first(value: unknown): string {
  const one = Array.isArray(value) ? value[0] : value
  return typeof one === 'string' ? one : ''
}

export function opponentsFromUrl(query: Record<string, unknown>): OpponentsState {
  const sort = OPPONENT_SORTS.find((s) => s.value === first(query.sort))?.value ?? DEFAULT_OPPONENTS.sort
  const page = Number.parseInt(first(query.page), 10)
  return {
    search: first(query.search).trim(),
    sort,
    format: first(query.format) || null,
    page: Number.isFinite(page) && page >= 1 ? page : 1,
  }
}

export function opponentsUrlQuery(state: OpponentsState): Record<string, string> {
  const query: Record<string, string> = {}
  if (state.search.trim() !== '') query.search = state.search
  if (state.sort !== DEFAULT_OPPONENTS.sort) query.sort = state.sort
  if (state.format) query.format = state.format
  if (state.page > 1) query.page = String(state.page)
  return query
}

/**
 * The last real page when `page` is past the end (the list shrank, or an old
 * link), else null. An empty list has one page.
 */
export function clampedPage(page: number, total: number, perPage: number): number | null {
  const lastPage = Math.max(1, Math.ceil(total / perPage))
  return page > lastPage ? lastPage : null
}
