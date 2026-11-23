import { reactive } from 'vue'
import type { DeckSortKey, DeckView, FormatOptionView, RecordView } from '@/bindings'
import { MANA_COLORS } from './mana'

/**
 * The deck list's format and "recently played" filters, kept for the session
 * (0.x `RememberDeckIndexFilters`). With nothing chosen yet the list opens on
 * the recently played decks. Search is not remembered: it is a one-off
 * narrowing, not a place the user works from.
 */
export const deckFilters = reactive({
  /** A raw format code, or `null` for every format. */
  format: null as string | null,
  recent: true,
  /**
   * The archetype row chosen (0.x `archetype`): `none` for Unclassified, an
   * archetype id, or `null`. It replaces "recently played" while set.
   */
  archetype: null as string | null,
})

/**
 * How the "Recent decks" row shows. `asked` is the remembered filter,
 * `applied` whether the list says it applied (`null` while there is no
 * settled list), `archetype` the archetype row chosen. Asked for but not
 * applied means no deck has any activity yet, so every deck shows (0.x) and
 * clicking the row could change nothing. A chosen archetype row replaces
 * recent, so the list never applies it then: the row is off and clickable.
 */
export function recentRowState(
  asked: boolean,
  applied: boolean | null,
  archetype: string | null = null,
): 'on' | 'off' | 'unavailable' {
  if (archetype !== null) return 'off'
  if (applied === null) return asked ? 'on' : 'off'
  if (asked && !applied) return 'unavailable'
  return applied ? 'on' : 'off'
}

export const SORT_OPTIONS: { value: DeckSortKey; label: string }[] = [
  { value: 'last_played', label: 'Last Played' },
  { value: 'win_rate', label: 'Win Rate' },
  { value: 'match_count', label: 'Match Count' },
  { value: 'name', label: 'Name' },
]

export const PER_PAGE_OPTIONS = [12, 24, 48]

// ---- Format select ------------------------------------------------------------

/** The select refuses an empty value, so "All" uses this and maps back to no filter. */
export const ALL_FORMATS = '__all__'

export function totalDecks(options: FormatOptionView[]): number {
  let total = 0
  for (const option of options) {
    total += option.count
  }
  return total
}

/** The format select's rows: "All" with every deck counted, then each format. */
export function formatSelectOptions(options: FormatOptionView[]): { value: string; label: string; count: number }[] {
  const rows = [{ value: ALL_FORMATS, label: 'All', count: totalDecks(options) }]
  for (const option of options) {
    rows.push({ value: option.code, label: option.label, count: option.count })
  }
  return rows
}

// ---- Records ------------------------------------------------------------------

export type RecordTone = 'win' | 'loss' | 'draw'

/** A record as letters ("3W - 1L"), draws only when there are any (0.x `formatMatchRecord`). */
export function recordParts(record: RecordView): { text: string; tone: RecordTone }[] {
  const parts: { text: string; tone: RecordTone }[] = [
    { text: `${record.wins}W`, tone: 'win' },
    { text: `${record.losses}L`, tone: 'loss' },
  ]
  if (record.draws > 0) {
    parts.push({ text: `${record.draws}D`, tone: 'draw' })
  }
  return parts
}

/** How a win rate reads: below half is a loss, exactly half is even. */
export function winRateTone(rate: number): 'win' | 'even' | 'loss' {
  if (rate < 50) return 'loss'
  if (rate === 50) return 'even'
  return 'win'
}

// ---- Mana ---------------------------------------------------------------------

const NEUTRAL = '#8A8A8A'

/** The colour hexes of the known identity letters. */
export function manaColorsFor(colors: string[]): string[] {
  const hexes: string[] = []
  for (const letter of colors) {
    const hex = MANA_COLORS[letter.trim().toUpperCase()]
    if (hex) hexes.push(hex)
  }
  return hexes
}

function withAlpha(hex: string, alpha: number): string {
  const value = hex.replace('#', '')
  const r = parseInt(value.slice(0, 2), 16)
  const g = parseInt(value.slice(2, 4), 16)
  const b = parseInt(value.slice(4, 6), 16)
  return `rgba(${r}, ${g}, ${b}, ${alpha})`
}

/**
 * A diagonal wash in a deck's colours, faint enough to sit under text. Fills
 * the art band of a deck with no cover art.
 */
export function manaWash(colors: string[], alpha = 0.28): string {
  const hexes = manaColorsFor(colors)
  const palette = hexes.length > 0 ? hexes : [NEUTRAL, NEUTRAL]
  const stops = palette.length === 1 ? [palette[0], palette[0]] : palette
  const step = 100 / (stops.length - 1)
  const gradient = stops.map((hex, index) => `${withAlpha(hex, alpha)} ${Math.round(index * step)}%`).join(', ')
  return `linear-gradient(135deg, ${gradient})`
}

/** The symbols to draw: colours in WUBRG order, colourless when asked for or when there are none. */
export function shownManaSymbols(colors: string[]): string[] {
  const coloured = ['W', 'U', 'B', 'R', 'G'].filter((letter) => colors.includes(letter))
  if (colors.includes('C') || colors.length === 0) {
    coloured.push('C')
  }
  return coloured
}

// ---- Last played --------------------------------------------------------------

function addMonths(date: Date, months: number): Date {
  const next = new Date(date.getTime())
  // Overflows like PHP: 31 January plus one month is 3 March.
  next.setMonth(next.getMonth() + months)
  return next
}

/**
 * When a deck was last played, as 0.x's Carbon `diffForHumans()` says it: the
 * largest whole unit ("3 weeks ago"), counted on the viewer's calendar.
 * `null` when the deck was never played.
 */
export function lastPlayedText(playedMs: number | null, nowMs: number): string | null {
  if (playedMs === null) return null

  const past = playedMs <= nowMs
  const earlier = new Date(Math.floor(Math.min(playedMs, nowMs) / 1000) * 1000)
  const later = new Date(Math.floor(Math.max(playedMs, nowMs) / 1000) * 1000)

  let months = (later.getFullYear() - earlier.getFullYear()) * 12 + (later.getMonth() - earlier.getMonth())
  while (months > 0 && addMonths(earlier, months) > later) {
    months -= 1
  }
  const seconds = Math.floor((later.getTime() - addMonths(earlier, months).getTime()) / 1000)
  const days = Math.floor(seconds / 86400)

  const units: [number, string][] = [
    [Math.floor(months / 12), 'year'],
    [months % 12, 'month'],
    [Math.floor(days / 7), 'week'],
    [days % 7, 'day'],
    [Math.floor(seconds / 3600) % 24, 'hour'],
    [Math.floor(seconds / 60) % 60, 'minute'],
    [seconds % 60, 'second'],
  ]
  let count = 0
  let unit = 'second'
  for (const [value, name] of units) {
    if (value > 0) {
      count = value
      unit = name
      break
    }
  }
  return `${count} ${unit}${count === 1 ? '' : 's'} ${past ? 'ago' : 'from now'}`
}

/** "12 matches · 2 days ago"; an archived deck shows only its match count. */
export function activityText(deck: DeckView, nowMs: number, never: string): string {
  const matches = `${deck.record.total} matches`
  if (deck.archived) return matches
  return `${matches} · ${lastPlayedText(deck.lastPlayedAtMs, nowMs) ?? never}`
}

/** The kind tags of an Arena deck card (Bo3 first, then Bo1); none for an MTGO deck. */
export function deckKindLabels(deck: DeckView): string[] {
  return deck.kinds.map((kind) => (kind === 'bo1' ? 'Bo1' : 'Bo3'))
}

/** The cover fields of a deck view (`DeckView`, `RecentDeckView`; a draft has no custom one). */
export interface DeckCoverFields {
  coverArt: string | null
  localCoverArt: string | null
  customCoverArt?: string | null
}

/**
 * What `CardImage` takes for a deck's cover, and whether there is art to
 * show: the deck's own image first, the printing when it has none or its
 * file is gone (`CardImage` falls back on a load error).
 */
export function deckCover(view: DeckCoverFields) {
  const custom = view.customCoverArt ?? null
  return {
    src: view.coverArt,
    local: view.localCoverArt,
    custom,
    hasCover: Boolean(custom) || Boolean(view.coverArt),
  }
}
