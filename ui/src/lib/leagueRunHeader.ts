import type { ClassificationView, LimitedIndexRowView, MarkView, RunCardView, TimeOfDayView } from '@/bindings'
import { droppedTitle, runRecord } from '@/lib/leagues'
import { limitedPath, rowLinks } from '@/lib/limited'

/**
 * The one league run header (`LeagueRunHeader`) the Leagues page, the deck
 * dashboard and the Limited index share: what it shows, from a run card
 * (`RunCardView`) or a Limited index row (`LimitedIndexRowView`).
 */

/** A state worth a badge: a run that did not simply run its course. */
export type RunHeaderState = {
  label: string
  tone: 'destructive' | 'warning'
  /** Hover text ("Dropped 2 days ago"). */
  title: string | null
}

export type LeagueRunHeaderView = {
  classification: ClassificationView
  wins: number
  losses: number
  liveRound: number | null
  results: (MarkView | null)[]
  /** The deck's or the event's name; `null` hides the identity (name, colours, format). */
  name: string | null
  /** Where the name goes; `null` shows it as plain text. */
  link: string | null
  /** Mana colour letters; empty shows none. */
  colors: string[]
  /** The format badge ("Modern", "Draft"). */
  format: string | null
  manual: boolean
  versionLabel: string | null
  /** `null` for a run in progress or completed. */
  state: RunHeaderState | null
  /** `null` hides the date (a draft with no time). */
  startedAtMs: number | null
  avgMatchSeconds: number | null
  timeOfDay: TimeOfDayView | null
  tixDelta: number | null
}

type Identity = Pick<LeagueRunHeaderView, 'name' | 'link' | 'colors' | 'format'>

/** A limited run is named by its event; a constructed run by its deck, unless hidden. */
function cardIdentity(run: RunCardView, hideDeckIdentity: boolean): Identity {
  if (run.limited) {
    // Sealed has no picks to review, so it opens on its deck (as the Limited index).
    const page = run.format === 'Sealed' ? 'deck' : 'draft'
    return { name: run.name, link: limitedPath(run.key, page), colors: run.colors ?? [], format: run.format }
  }
  if (run.deck && !hideDeckIdentity) {
    return { name: run.deck.name, link: `/decks/${run.deck.id}`, colors: run.deck.colors, format: run.format }
  }
  return { name: null, link: null, colors: [], format: null }
}

/** A run card's header. Only a dropped run is badged, as the Leagues page always did. */
export function runCardHeader(run: RunCardView, nowMs: number, hideDeckIdentity = false): LeagueRunHeaderView {
  const record = runRecord(run.results)
  const state: RunHeaderState | null =
    run.state === 'dropped' ? { label: 'Dropped', tone: 'destructive', title: droppedTitle(run, nowMs) } : null
  return {
    classification: run.classification,
    wins: record.wins,
    losses: record.losses,
    liveRound: run.liveRound,
    results: run.results,
    ...cardIdentity(run, hideDeckIdentity),
    manual: run.manual,
    versionLabel: run.versionLabel,
    state,
    startedAtMs: run.startedAtMs,
    avgMatchSeconds: run.avgMatchSeconds,
    timeOfDay: run.timeOfDay,
    tixDelta: run.tixDelta,
  }
}

/**
 * The row's state badge: none for a run in progress (`default`) or
 * completed (`success`); dropped, abandoned, unlinked and unplayed runs say so.
 */
function rowState(row: LimitedIndexRowView): RunHeaderState | null {
  if (row.stateVariant === 'destructive') return { label: row.state, tone: 'destructive', title: null }
  if (row.stateVariant === 'warning') return { label: row.state, tone: 'warning', title: null }
  return null
}

/** `Draft`, `Sealed`. */
function kindLabel(kind: string): string {
  return kind.charAt(0).toUpperCase() + kind.slice(1)
}

/** A Limited index row's header (no colours: the index reads no deck cards). */
export function limitedRowHeader(row: LimitedIndexRowView): LeagueRunHeaderView {
  return {
    classification: row.classification,
    wins: row.wins,
    losses: row.losses,
    liveRound: row.liveRound,
    results: row.results,
    name: row.title,
    link: rowLinks(row).title,
    colors: [],
    format: kindLabel(row.kind),
    manual: false,
    versionLabel: null,
    state: rowState(row),
    startedAtMs: row.startedAtMs,
    avgMatchSeconds: row.avgMatchSeconds,
    timeOfDay: row.timeOfDay,
    tixDelta: null,
  }
}
