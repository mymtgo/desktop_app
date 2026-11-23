/**
 * The Leagues page's pure helpers (0.x `pages/leagues/Index.vue` and
 * `components/leagues/*`): the URL filters, the card's labels, the KPI text,
 * the copied summary and the manual league dialog's defaults.
 */
import type {
  ClassificationView,
  GameMarkView,
  LeagueListQuery,
  LeagueRefView,
  LeagueSort,
  LeagueStateChip,
  MarkView,
  RunCardView,
  RunStateView,
  TimeOfDayView,
} from '@/bindings'
import { lastPlayedText } from '@/lib/decks'

/** Shown where 0.x showed its "no value" dash. */
export const NO_VALUE = '-'

/** Rounds in a league; a manual league links at most this many matches. */
export const LEAGUE_ROUNDS = 5

/** The state chips, in 0.x order. */
export const STATE_CHIPS: { key: LeagueStateChip; label: string }[] = [
  { key: 'all', label: 'All' },
  { key: 'live', label: 'Live' },
  { key: 'trophies', label: 'Trophies' },
  { key: 'cash', label: '4-1+' },
  { key: 'finish', label: 'Finish' },
  { key: 'bricks', label: 'Bricks' },
]

export const LEAGUE_SORTS: { value: LeagueSort; label: string }[] = [
  { value: 'newest', label: 'Newest' },
  { value: 'oldest', label: 'Oldest' },
  { value: 'best', label: 'Best' },
  { value: 'worst', label: 'Worst' },
]

function text(value: unknown): string | null {
  if (typeof value !== 'string') return null
  return value.trim() === '' ? null : value
}

function positive(value: unknown): number | null {
  if (typeof value !== 'string' || !/^\d+$/.test(value)) return null
  const number = Number(value)
  return number > 0 ? number : null
}

/**
 * The page's filters from the URL query (0.x kept them in the query string:
 * `format`, `state`, `deck`, `archetype`, `q`, `sort`, plus `page`). Anything unknown
 * falls back to its default.
 */
export function leagueQueryFromUrl(query: Record<string, unknown>): LeagueListQuery {
  const state = STATE_CHIPS.find((chip) => chip.key === query.state)?.key ?? 'all'
  const sort = LEAGUE_SORTS.find((option) => option.value === query.sort)?.value ?? 'newest'
  return {
    format: text(query.format),
    state,
    deckId: positive(query.deck),
    archetypeId: positive(query.archetype),
    search: text(query.q),
    sort,
    page: positive(query.page) ?? 1,
  }
}

/** The URL query for a set of filters, defaults left out. */
export function leagueUrlQuery(query: LeagueListQuery): Record<string, string> {
  const url: Record<string, string> = {}
  if (query.format) url.format = query.format
  if (query.state !== 'all') url.state = query.state
  if (query.deckId !== null) url.deck = String(query.deckId)
  if (query.archetypeId !== null && query.archetypeId !== undefined) url.archetype = String(query.archetypeId)
  if (query.search) url.q = query.search
  if (query.sort !== 'newest') url.sort = query.sort
  if (query.page > 1) url.page = String(query.page)
  return url
}

/** Wins and losses of the played rounds; an open round counts as neither. */
export function runRecord(results: (MarkView | null)[]): { wins: number; losses: number } {
  let wins = 0
  let losses = 0
  for (const result of results) {
    if (result === 'W') wins += 1
    if (result === 'L') losses += 1
  }
  return { wins, losses }
}

export type ResultDot = 'win' | 'loss' | 'open'

/** One dot per round; an Active run comes padded to five, its open rounds empty. */
export function resultDots(results: (MarkView | null)[]): ResultDot[] {
  return results.map((result) => {
    if (result === 'W') return 'win'
    if (result === 'L') return 'loss'
    return 'open'
  })
}

/** "LIVE-R3" for a live run with a known round, else the classification. */
export function resultBadgeLabel(classification: ClassificationView, liveRound: number | null): string {
  if (classification === 'LIVE') {
    return liveRound ? `LIVE-R${liveRound}` : 'LIVE'
  }
  return classification
}

/**
 * The ring around the score, drawn as a gradient behind a 2px inset. Every
 * tone but the trophy is a theme token; the trophy keeps its gradient on
 * purpose (0.x: the one result worth making loud).
 */
export function resultBadgeTone(classification: ClassificationView): string {
  switch (classification) {
    case 'TROPHY':
      return 'from-pink-400 via-sky-300 to-blue-400'
    case 'CASH':
      return 'from-success/60 to-success text-success'
    case 'FINISH':
      return 'from-border to-border'
    case 'BRICK':
      return 'from-destructive/50 to-destructive/80 text-destructive'
    case 'LIVE':
      return 'from-primary/50 to-primary text-primary'
    default:
      return 'from-border to-border text-muted-foreground'
  }
}

/** The average match in whole minutes; nothing when unknown (or zero, as 0.x). */
export function avgMinutes(seconds: number | null): number | null {
  if (!seconds) return null
  return Math.round(seconds / 60)
}

export function timeOfDayLabel(timeOfDay: TimeOfDayView | null): string | null {
  return timeOfDay ? timeOfDay.charAt(0).toUpperCase() + timeOfDay.slice(1) : null
}

/** "+12 tix", "-8 tix", "0 tix". */
export function tixText(delta: number | null): string | null {
  if (delta === null) return null
  return `${delta > 0 ? '+' : ''}${delta.toFixed(0)} tix`
}

export function tixTone(delta: number | null): string {
  if (delta === null) return ''
  if (delta > 0) return 'text-emerald-400'
  if (delta < 0) return 'text-destructive'
  return 'text-muted-foreground'
}

/** The run's start as "08 Oct" (0.x: day and short month, the viewer's locale). */
export function startedDayMonth(ms: number, locale?: string): string {
  return new Date(ms).toLocaleDateString(locale, { day: '2-digit', month: 'short' })
}

/** The Dropped badge's hover text. */
export function droppedTitle(run: RunCardView, nowMs: number): string {
  if (run.droppedAtMs === null) return 'Dropped before completion'
  return `Dropped ${lastPlayedText(run.droppedAtMs, nowMs)}`
}

/** A match's length in the matches table. */
export function durationText(seconds: number | null): string {
  if (!seconds) return NO_VALUE
  return `${Math.round(seconds / 60)}m`
}

/** A game cell: "Win (OTP)", "Loss (OTD)"; null for a game not played. */
export function gameCell(
  game: GameMarkView | undefined,
): { text: string; tone: string; side: 'OTP' | 'OTD' | null } | null {
  if (!game) return null
  const won = game.result === 'W'
  return {
    text: won ? 'Win' : 'Loss',
    tone: won ? 'text-success' : 'text-destructive',
    side: game.onPlay === null ? null : game.onPlay ? 'OTP' : 'OTD',
  }
}

/** How many more matches a manual league can link. */
export function remainingSlots(matchCount: number): number {
  return Math.max(0, LEAGUE_ROUNDS - matchCount)
}

/**
 * Whether a run card offers "Add manual match": the sheet's league options
 * take only Active runs and manual leagues, so any other run would open the
 * sheet on "No league". Both need fewer than five matches.
 */
export function takesManualMatch(run: RunCardView): boolean {
  if (run.matches.length >= LEAGUE_ROUNDS) return false
  return run.manual || run.state === 'active'
}

/** A KPI rate: "25%", or a dash without finished runs. */
export function rateText(rate: number | null): string {
  return rate !== null ? `${rate}%` : NO_VALUE
}

export function dropRateText(rate: number | null): string {
  return rate !== null ? `${rate}% of runs` : 'no finished runs'
}

export function avgFinishText(avg: number | null): { value: string; caption: string } {
  if (avg === null) return { value: NO_VALUE, caption: 'no finished runs' }
  return { value: String(avg), caption: 'wins per run' }
}

/**
 * "Copy league summary" (0.x text format):
 *
 * ```
 * Qamurai Burn Modern v2
 * 3 hours ago · 4-1
 * W vs Qorvath
 * ```
 *
 * The score counts played rounds; each match line names the opponent's
 * archetype, else the opponent (0.x).
 */
export function leagueSummaryText(run: RunCardView, nowMs: number): string {
  const lines: string[] = []
  if (run.limited) {
    // A limited run has no deck: its name is its set and kind ("QNW Draft").
    lines.push(run.name)
  } else {
    const deckName = run.deck?.name ?? 'Unknown deck'
    const versionPart = run.versionLabel ? ` ${run.versionLabel}` : ''
    lines.push(`${deckName} ${run.format}${versionPart}`)
  }
  const { wins, losses } = runRecord(run.results)
  lines.push(`${lastPlayedText(run.startedAtMs, nowMs)} · ${wins}-${losses}`)
  for (const match of run.matches) {
    lines.push(`${match.result} vs ${match.opponentArchetype ?? match.opponentName ?? 'Unknown'}`)
  }
  return lines.join('\n')
}

function pad(value: number): string {
  return value.toString().padStart(2, '0')
}

/** The create dialog's start: now, to the minute, as a `datetime-local` value. */
export function defaultStartedAt(now: Date): string {
  return `${now.getFullYear()}-${pad(now.getMonth() + 1)}-${pad(now.getDate())}T${pad(now.getHours())}:${pad(now.getMinutes())}`
}

/** The create dialog's default name: "Modern League 08-10-2026 07:05pm" (0.x). */
export function defaultLeagueName(formatLabel: string, startedAt: string): string {
  if (!startedAt) return ''
  const date = new Date(startedAt)
  if (Number.isNaN(date.getTime())) return ''
  const day = `${pad(date.getDate())}-${pad(date.getMonth() + 1)}-${date.getFullYear()}`
  const hours = date.getHours()
  const ampm = hours >= 12 ? 'pm' : 'am'
  return `${formatLabel} League ${day} ${pad(hours % 12 || 12)}:${pad(date.getMinutes())}${ampm}`
}

/** The create dialog field a refusal is about (0.x showed errors per field). */
export function createErrorField(message: string): 'deck' | 'startedAt' | 'name' | null {
  if (message.includes('deck id')) return 'deck'
  if (message.includes('started at')) return 'startedAt'
  if (message.includes('name field')) return 'name'
  return null
}

/** The statuses a player can set, in the "Set status" menu's order (league self-management spec 5.1). */
export const STATUS_CHOICES: { value: RunStateView; label: string }[] = [
  { value: 'active', label: 'Active' },
  { value: 'complete', label: 'Complete' },
  { value: 'dropped', label: 'Dropped' },
  { value: 'partial', label: 'Partial' },
]

export function statusLabel(state: RunStateView): string {
  return STATUS_CHOICES.find((choice) => choice.value === state)?.label ?? state
}

/** "Automatic (Complete)": Automatic with the fold's state in brackets. */
export function automaticLabel(autoState: RunStateView): string {
  return `Automatic (${statusLabel(autoState)})`
}

/** "Delete league..." is offered once the run does not read Active (spec 4.4). */
export function canDeleteLeague(run: Pick<RunCardView, 'state'>): boolean {
  return run.state !== 'active'
}

/** The delete dialog's question (spec 5.2). */
export function deleteLeagueQuestion(matchCount: number): string {
  if (matchCount === 0) return 'Delete this league? You can restore it from Settings, Archives.'
  return `What should happen to its ${matchCount} match${matchCount === 1 ? '' : 'es'}?`
}

/** Where a match sits now, in the pickers (spec 5.3): "in 11 Oct league", "no league". */
export function whereNowText(current: LeagueRefView | null, locale?: string): string {
  if (current === null) return 'no league'
  return `in ${startedDayMonth(current.startedAtMs, locale)} league`
}

/** The move toast names where the match came from (spec 5.3). */
export function movedFromText(from: LeagueRefView | null, locale?: string): string {
  if (from === null) return 'It had no league before.'
  return `It was in the ${startedDayMonth(from.startedAtMs, locale)} league.`
}
