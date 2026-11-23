/**
 * The home dashboard's choices and text (dashboard redesign spec): the
 * format switch and its remembered value, the League results bars, and the
 * helpers the cards share.
 */
import type {
  BreakdownRowView,
  FormatSwitchOptionView,
  HomeRecentMatchView,
  LeagueKindView,
  LeagueRunBarView,
  LeagueRunsView,
  LimitedPickCountView,
  PickSetOptionView,
  PlatformView,
  RecentEntryView,
  RecentSessionView,
  RunBarToneView,
  RunStateView,
} from '@/bindings'
import { dayToUtcMs } from './timeframes'

// ---- The format switch ------------------------------------------------------

export type FormatChoice = { value: string; label: string }

/** Constructed and Limited come first; the constructed formats played follow. */
export const FIXED_FORMATS: FormatChoice[] = [
  { value: 'constructed', label: 'Constructed' },
  { value: 'limited', label: 'Limited' },
]

/** Arena and Endstep have no Limited data (arena spec section 15), so their combined choice reads All formats. */
export function formatChoices(formats: FormatSwitchOptionView[], platform: PlatformView = 'mtgo'): FormatChoice[] {
  const fixed =
    platform === 'mtgo'
      ? FIXED_FORMATS
      : [{ value: 'constructed', label: 'All formats' }]
  return [...fixed, ...formats.map((format) => ({ value: format.key, label: format.label }))]
}

/** An Arena or Endstep account that played one format (or none) has nothing to switch between. */
export function showFormatSwitch(formats: FormatSwitchOptionView[], platform: PlatformView = 'mtgo'): boolean {
  return platform === 'mtgo' || formats.length > 1
}

/** The format the page asks for: `?format=`, else empty, which the backend reads as the format last played. */
export function askedFormat(fromUrl: unknown): string {
  return typeof fromUrl === 'string' && fromUrl.trim() !== '' ? fromUrl : ''
}

/** The page's query with `key` set to `value`; the default (`fallback`) is left out of the URL. */
export function withQueryValue<T>(
  query: Record<string, T>,
  key: string,
  value: string,
  fallback: string,
): Record<string, T | string> {
  const { [key]: _previous, ...rest } = query
  return value === fallback ? rest : { ...rest, [key]: value }
}

/** Results' empty text, naming the format when one is chosen. */
export function noMatchesText(
  format: string,
  formats: FormatSwitchOptionView[],
  platform: PlatformView = 'mtgo',
): string {
  if (format === 'all' || (platform !== 'mtgo' && format === 'constructed')) return 'No matches in this timeframe'
  if (format === 'constructed') return 'No constructed matches in this timeframe'
  if (format === 'limited') return 'No limited matches in this timeframe'
  const label = formats.find((option) => option.key === format)?.label ?? format
  return `No ${label} matches in this timeframe`
}

// ---- League results ---------------------------------------------------------

/** "14 leagues · 1 trophy · 2.4 avg wins"; no average without a complete run. */
export function leagueRunsHeader(view: LeagueRunsView): string {
  const runs = view.runs.length
  const parts = [
    `${runs} ${runs === 1 ? 'league' : 'leagues'}`,
    `${view.trophies} ${view.trophies === 1 ? 'trophy' : 'trophies'}`,
  ]
  if (view.avgWins !== null) parts.push(`${view.avgWins.toFixed(1)} avg wins`)
  return parts.join(' · ')
}

const BAR_CLASS: Record<RunBarToneView, string> = {
  trophy: 'bg-yellow-400',
  winning: 'bg-success',
  losing: 'bg-destructive',
  dropped: 'bg-muted-foreground/40',
  partial: 'bg-muted-foreground/30',
  active: 'border border-dashed border-primary bg-primary/10',
}

export type LadderTone = RunBarToneView | 'empty'
export type LadderRow = { label: string; count: number; tone: LadderTone }
export type Ladder = { kind: LeagueKindView; label: string | null; rows: LadderRow[] }

const LADDER_KINDS: LeagueKindView[] = ['constructed', 'draft', 'sealed']
const DEFAULT_ROUNDS: Record<LeagueKindView, number> = { constructed: 5, draft: 3, sealed: 5 }

/** A finish's tone from its record, as the bars were coloured: a full run won is gold, more wins than losses green, else red. */
function finishTone(wins: number, losses: number, rounds: number): RunBarToneView {
  if (rounds > 0 && wins >= rounds) return 'trophy'
  return wins > losses ? 'winning' : 'losing'
}

function ladderFor(kind: LeagueKindView, runs: LeagueRunBarView[], label: string | null): Ladder {
  const rounds = Math.max(DEFAULT_ROUNDS[kind], ...runs.map((run) => run.rounds))
  const counts: number[] = []
  for (let wins = 0; wins <= rounds; wins += 1) counts.push(0)
  let dropped = 0
  let unfinished = 0
  for (const run of runs) {
    if (run.state === 'dropped') dropped += 1
    else if (run.state === 'partial') unfinished += 1
    else if (run.state === 'complete') counts[Math.min(rounds, Math.max(0, run.wins))] += 1
  }
  const rows: LadderRow[] = []
  for (let wins = rounds; wins >= 0; wins -= 1) {
    const count = counts[wins]
    rows.push({
      label: `${wins}-${rounds - wins}`,
      count,
      tone: count === 0 ? 'empty' : finishTone(wins, rounds - wins, rounds),
    })
  }
  rows.push({ label: 'Dropped', count: dropped, tone: dropped === 0 ? 'empty' : 'dropped' })
  if (unfinished > 0) rows.push({ label: 'Unfinished', count: unfinished, tone: 'partial' })
  return { kind, label, rows }
}

/**
 * The League results ladders: per kind (constructed, draft, sealed, in that
 * order) every finish from all wins to none, then Dropped, and Unfinished
 * only when a run ended early. A record that does not add up to the rounds
 * goes to the bucket of its wins, clamped. Active runs are not counted.
 * With more than one kind present each ladder is named.
 */
export function leagueLadders(runs: LeagueRunBarView[]): Ladder[] {
  const present = LADDER_KINDS.filter((kind) => runs.some((run) => run.kind === kind))
  const named = present.length > 1
  return present.map((kind) =>
    ladderFor(
      kind,
      runs.filter((run) => run.kind === kind),
      named ? limitedKindLabel(kind) : null,
    ),
  )
}

/** The widest ladder row, at least 1: what a row's bar is a share of. */
export function ladderMax(ladder: Ladder): number {
  return Math.max(1, ...ladder.rows.map((row) => row.count))
}

/** A ladder bar's fill; Dropped and Unfinished are neutral. */
export function ladderBarClass(tone: LadderTone): string {
  if (tone === 'empty') return 'bg-transparent'
  if (tone === 'partial' || tone === 'dropped') return 'bg-muted-foreground/40'
  return BAR_CLASS[tone]
}

// ---- Marks and numbers ------------------------------------------------------

export type MarkTone = 'win' | 'loss' | 'neutral'
export type OutcomeMark = { text: 'W' | 'L' | 'D' | '?'; tone: MarkTone }

/** A match's mark: W, L, D for a draw and a neutral ? for an unknown outcome, never L (spec 4.9). */
export function sessionMark(outcome: string): OutcomeMark {
  if (outcome === 'win') return { text: 'W', tone: 'win' }
  if (outcome === 'loss') return { text: 'L', tone: 'loss' }
  if (outcome === 'draw') return { text: 'D', tone: 'neutral' }
  return { text: '?', tone: 'neutral' }
}

export function markClass(tone: MarkTone): string {
  if (tone === 'win') return 'bg-success/15 text-success'
  if (tone === 'loss') return 'bg-destructive/15 text-destructive'
  return 'bg-muted text-muted-foreground'
}

/**
 * A Recent sessions label: Today, Yesterday, the weekday up to six days ago,
 * the date after that ("3 Nov"). The page counts the days in the app's own
 * timezone, so the label does not depend on this browser's.
 */
export function recentSessionLabel(session: Pick<RecentSessionView, 'day' | 'daysAgo'>, locale = 'en-GB'): string {
  if (session.daysAgo === 0) return 'Today'
  if (session.daysAgo === 1) return 'Yesterday'
  const ms = dayToUtcMs(session.day)
  if (ms === null) return session.day
  const date = new Date(ms)
  if (session.daysAgo < 7) return date.toLocaleDateString(locale, { weekday: 'long', timeZone: 'UTC' })
  return date.toLocaleDateString(locale, { day: 'numeric', month: 'short', timeZone: 'UTC' })
}

/** A win rate change: "+5%", "-3%". */
export function deltaText(delta: number): string {
  return `${delta > 0 ? '+' : ''}${delta}%`
}

/** The change pill: green up, red down, neutral flat. */
export function deltaClass(delta: number): string {
  if (delta > 0) return 'bg-success/15 text-success'
  if (delta < 0) return 'bg-destructive/15 text-destructive'
  return 'bg-muted text-muted-foreground'
}

/** A row's win rate colour: green from 50%. */
export function winRateClass(winRate: number): string {
  return winRate >= 50 ? 'text-success' : 'text-destructive'
}

/** "24 matches · 4% of matches": a count and its share, as text. */
export function shareText(count: number, unit: string, share: number, of: string): string {
  return `${count} ${count === 1 ? unit === 'matches' ? 'match' : unit.replace(/s$/, '') : unit} · ${share}% of ${of}`
}

/** A share as a CSS width, clamped to 0 to 100%. */
export function shareWidth(share: number): string {
  return `${Math.max(0, Math.min(100, share))}%`
}

/** Clearly different hues, so neighbouring segments never blur together. */
const SEGMENT_CLASSES = ['bg-sky-500', 'bg-teal-500', 'bg-green-500', 'bg-amber-500', 'bg-violet-500', 'bg-rose-500']

/** The Formats and Sets bar's segment colour, by row order (stable, cycling after six). */
export function segmentClass(index: number): string {
  return SEGMENT_CLASSES[index % SEGMENT_CLASSES.length]
}

/** A segment's width: the share, but never thinner than a sliver (1%) so a small entry still shows. */
export function segmentWidth(share: number): string {
  return shareWidth(Math.max(1, share))
}

/** The card's one-line description. */
export function breakdownDescription(kind: string): string {
  const by = kind === 'sets' ? 'set' : 'format'
  return `Share of your matches by ${by}, and your match win rate in each.`
}

/** Past this many entries, those whose share rounds to 0 fold into "Other". */
const MAX_LISTED = 5

/**
 * The Formats and Sets rows as shown: all of them, unless there are more
 * than five, when the ones whose share rounds to 0% become one "Other" row
 * (after the rest, its win rate over its own matches).
 */
export function breakdownRows(rows: BreakdownRowView[]): BreakdownRowView[] {
  if (rows.length <= MAX_LISTED) return rows
  const kept = rows.filter((row) => row.share > 0)
  const folded = rows.filter((row) => row.share === 0)
  if (folded.length === 0) return rows
  let matches = 0
  let wins = 0
  let losses = 0
  let draws = 0
  for (const row of folded) {
    matches += row.matches
    wins += row.record.wins
    losses += row.record.losses
    draws += row.record.draws
  }
  const winRate = matches > 0 ? Math.round((100 * wins) / matches) : 0
  const label = draws > 0 ? `${wins} - ${losses} - ${draws}` : `${wins} - ${losses}`
  kept.push({
    key: 'other-folded',
    label: 'Other',
    matches,
    share: 0,
    record: { wins, losses, draws, total: matches, winRate, label },
  })
  return kept
}

// ---- The side cards ---------------------------------------------------------

export function recentTitle(format: string): string {
  if (format === 'limited') return 'Recent drafts'
  if (format === 'all') return 'Recent decks and drafts'
  return 'Recent decks'
}

export function recentLink(entry: RecentEntryView): string {
  return entry.type === 'deck' ? `/decks/${entry.id}` : `/limited/${encodeURIComponent(entry.key)}`
}

/**
 * A Recent matches row's format tag and event (All mode mixes both kinds,
 * spec decision 7): "Modern · League" for a constructed match; a limited
 * match's set is already in its row, so only its event.
 */
export function recentMatchEvent(row: {
  runKey: HomeRecentMatchView['runKey']
  match: Pick<HomeRecentMatchView['match'], 'format' | 'matchType' | 'limited'>
}): string {
  if (row.runKey !== null || row.match.limited) return row.match.matchType ?? 'Limited'
  return [row.match.format, row.match.matchType].filter((part) => part !== null && part !== '').join(' · ')
}

/** Most picked's switch value for every set. */
export const ALL_SETS = 'all'

export function pickSetChoices(sets: PickSetOptionView[]): FormatChoice[] {
  return [...sets.map((set) => ({ value: set.code, label: set.name })), { value: ALL_SETS, label: 'All sets' }]
}

/** A set's title: its name, else its code (0.x). */
export function limitedSetTitle(setName: string | null, setCode: string | null): string {
  return setName ?? setCode ?? 'Unknown set'
}

/** The kind badge: "Draft", "Sealed". */
export function limitedKindLabel(kind: string): string {
  return kind.charAt(0).toUpperCase() + kind.slice(1)
}

/** A pick's name; "Card #id" without card data (0.x). */
export function limitedPickName(pick: LimitedPickCountView): string {
  return pick.name ?? `Card #${pick.catalogId}`
}
