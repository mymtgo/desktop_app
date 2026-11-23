import type {
  HistoryDeckVersionView,
  HistoryImportFilters,
  HistoryImportResult,
  HistoryScanQuery,
  HistoryScanStatus,
  HistorySort,
  ScanOutcome,
} from '@/bindings'
import { dayMonthYearText } from './matchList'

/** The import page's three states (0.x `pages/import/Index.vue`). */
export type ImportPageState = 'setup' | 'processing' | 'results'

/** The results filters the page holds. */
export type ResultFilters = {
  /** A display format; blank for all. */
  format: string
  /** The slider, 0 to 1; 0 is Off. */
  minConfidence: number
  sort: HistorySort
}

/** The slider starts at 0.1, which hides matches without a game log (0.x). */
export const DEFAULT_MIN_CONFIDENCE = 0.1

/** The sorts 0.x offered. */
export const SORT_OPTIONS: { value: HistorySort; label: string }[] = [
  { value: 'confidence', label: 'Confidence (high first)' },
  { value: 'started_at', label: 'Date (newest first)' },
  { value: 'opponent', label: 'Opponent (A-Z)' },
]

/** Where the page starts, from the scan the app holds. */
export function pageStateFor(status: HistoryScanStatus | null): ImportPageState {
  if (status?.status === 'processing') return 'processing'
  if (status?.status === 'complete') return 'results'
  return 'setup'
}

/**
 * A status older than the one shown, so not to be applied:
 * - from an older scan (starting a scan cancels the running one, whose last
 *   report can arrive after the new scan began);
 * - taking an ended scan back to processing (a poll sent before the end can
 *   answer after the end was pushed).
 */
export function isStaleStatus(shown: HistoryScanStatus | null, incoming: HistoryScanStatus | null): boolean {
  if (shown === null || incoming === null) return false
  if (incoming.scanId < shown.scanId) return true
  return incoming.scanId === shown.scanId && shown.status !== 'processing' && incoming.status === 'processing'
}

/** One results page request; a blank format and an Off slider are left out. */
export function scanQuery(filters: ResultFilters, page: number): HistoryScanQuery {
  return {
    ...importAllFilters(filters),
    sort: filters.sort,
    // 0.x sorted opponents A to Z and everything else newest or highest first.
    dir: filters.sort === 'opponent' ? 'asc' : 'desc',
    page,
    perPage: null,
  }
}

/** "Import all" takes the rows through the page's filters. */
export function importAllFilters(filters: ResultFilters): HistoryImportFilters {
  return {
    format: filters.format === '' ? null : filters.format,
    minConfidence: filters.minConfidence > 0 ? filters.minConfidence : null,
  }
}

const STAGE_LABELS: Record<string, string> = {
  discovering: 'Discovering game logs...',
  decoding: 'Decoding game logs...',
  parsing: 'Parsing match history...',
  scoring: 'Scoring matches...',
}

export function stageLabel(stage: string): string {
  return STAGE_LABELS[stage] ?? 'Processing...'
}

export function progressPercent(progress: number, total: number): number {
  if (total === 0) return 0
  return Math.round((progress / total) * 100)
}

/** "12 / 40 (30%)". */
export function progressLine(progress: number, total: number): string {
  return `${progress} / ${total} (${progressPercent(progress, total)}%)`
}

/** The confidence column's colour (0.x thresholds). */
export function confidenceColor(confidence: number | null): string {
  if (confidence === null) return 'text-muted-foreground'
  if (confidence >= 0.8) return 'text-green-500'
  if (confidence >= 0.6) return 'text-yellow-500'
  return 'text-red-500'
}

export function confidenceText(confidence: number | null): string {
  if (confidence === null) return '-'
  return `${Math.round(confidence * 100)}%`
}

/** The label beside the min confidence slider. */
export function minConfidenceLabel(min: number): string {
  if (min === 0) return 'Off'
  if (min <= 0.1) return 'Any'
  return `${Math.round(min * 100)}%+`
}

/** The result badge: a win is the default badge, anything else destructive (0.x). */
export function outcomeVariant(outcome: ScanOutcome): 'default' | 'destructive' {
  return outcome === 'win' ? 'default' : 'destructive'
}

/**
 * The match's day in the viewer's timezone, the month by name ("4 Mar 2026");
 * 0.x showed an `en-GB` dd/mm/yyyy, read month first by many (#390). `timeZone` is for tests.
 */
export function formatScanDate(ms: number | null, timeZone?: string): string {
  if (ms === null) return '-'
  const parts = new Intl.DateTimeFormat('en-GB', { day: 'numeric', month: 'numeric', year: 'numeric', timeZone }).formatToParts(
    new Date(ms),
  )
  const part = (type: string): number => Number(parts.find((found) => found.type === type)?.value)
  return dayMonthYearText(part('day'), part('month'), part('year'))
}

/** The line under an empty results table. */
export function emptyLine(format: string, minConfidence: number): string {
  const forFormat = format !== '' ? ' for this format' : ''
  const atLevel = minConfidence > 0 ? ' at this confidence level' : ''
  return `No matches found${forFormat}${atLevel}.`
}

/** The banner after an import. 0.x queued a job; v1 stores before answering. */
export function importResultLine(result: HistoryImportResult): string {
  const imported = `${result.imported} match(es) imported.`
  if (result.skipped === 0) return imported
  return `${imported} ${result.skipped} skipped: already stored, or no start time or account.`
}

/** One `optgroup` of a deck version select. */
export type DeckVersionGroup = { label: string; versions: HistoryDeckVersionView[] }

/**
 * A deck version select's groups (the Import page's and "Set deck"'s):
 * active decks, then deleted (archived) ones, each newest first as listed.
 * An empty group is left out.
 */
export function deckVersionGroups(versions: HistoryDeckVersionView[]): DeckVersionGroup[] {
  const groups: DeckVersionGroup[] = [
    { label: 'Active Decks', versions: versions.filter((version) => !version.archived) },
    { label: 'Deleted Decks', versions: versions.filter((version) => version.archived) },
  ]
  return groups.filter((group) => group.versions.length > 0)
}

/**
 * The deck versions "Set deck" offers (issue #332): the current account's
 * decks and decks no account has adopted, never a Limited deck (the store
 * refuses the rest).
 */
export function settableDeckVersions(versions: HistoryDeckVersionView[]): HistoryDeckVersionView[] {
  return versions.filter((version) => version.currentAccount && !version.limited)
}

/** The "Set deck" dialog's line for one match, or the selected ones. */
export function setDeckDescription(count: number): string {
  if (count === 1) return 'Choose the deck version this match was played with.'
  return `Choose the deck version the ${count} selected matches were played with.`
}

/** A deck select option (0.x labels; "deleted" is an archived deck in v1). */
export function deckVersionLabel(version: HistoryDeckVersionView): string {
  if (version.archived) return `${version.deckName} (deleted) (${version.modifiedAt})`
  return `${version.deckName} (${version.format}, ${version.modifiedAt})`
}
