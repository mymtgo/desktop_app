/**
 * The Limited pages' pure helpers (0.x `types/limited.ts`, `pages/limited/*`
 * and `components/limited/*`): the index filters and row text, the event
 * paths, the draft review's labels and grid, the cross-draft rows and the
 * pick note saver.
 */
import type {
  ColorSignalView,
  CrossDraftStatsView,
  DraftHeaderView,
  DraftPickView,
  LimitedCardView,
  LimitedIndexQuery,
  LimitedIndexRowView,
  LimitedMatchColoursView,
  LimitedPage,
  RunMatchView,
  SeenWheelView,
} from '@/bindings'

/** Shown where 0.x showed its "no value" dash. */
export const NO_VALUE = '-'

/** The index timeframes (0.x `TIMEFRAMES`). */
export const LIMITED_TIMEFRAMES: { value: string; label: string }[] = [
  { value: 'alltime', label: 'All time' },
  { value: 'monthly', label: '30d' },
  { value: 'week', label: '7d' },
]

/** WUBRG, the order colours are always shown in. */
export const COLOR_ORDER = ['W', 'U', 'B', 'R', 'G'] as const

/** Cards by catalog id, as the commands send them. */
export type LimitedCards = { [key in number]: LimitedCardView }

/** "26s" under a minute, else "m:ss" (0.x `formatSeconds`). */
export function formatSeconds(seconds: number | null | undefined): string {
  if (seconds === null || seconds === undefined) return NO_VALUE
  if (seconds < 60) return `${seconds}s`
  const minutes = Math.floor(seconds / 60)
  const rest = seconds % 60
  return `${minutes}:${String(rest).padStart(2, '0')}`
}

/** A signed margin: "-1:30" rather than "1:30" when over time. */
export function signedSeconds(seconds: number | null): string {
  if (seconds === null) return NO_VALUE
  return seconds < 0 ? `-${formatSeconds(Math.abs(seconds))}` : formatSeconds(seconds)
}

/** "18:07" in local time (0.x `timeLabel`). */
export function timeLabel(ms: number | null): string {
  if (ms === null) return NO_VALUE
  const date = new Date(ms)
  return `${String(date.getHours()).padStart(2, '0')}:${String(date.getMinutes()).padStart(2, '0')}`
}

// ---- Index filters -----------------------------------------------------------

function text(value: unknown): string | null {
  if (typeof value !== 'string') return null
  return value.trim() === '' ? null : value
}

/** The index filters from the URL query (`set`, `kind`, `timeframe`); unknowns fall back. */
export function limitedQueryFromUrl(query: Record<string, unknown>): LimitedIndexQuery & { timeframe: string } {
  const kind = query.kind === 'draft' || query.kind === 'sealed' ? query.kind : null
  const timeframe = LIMITED_TIMEFRAMES.find((option) => option.value === query.timeframe)?.value ?? 'alltime'
  return { set: text(query.set), kind, timeframe }
}

/** The URL query for a set of filters, defaults left out. */
export function limitedUrlQuery(query: LimitedIndexQuery): Record<string, string> {
  const url: Record<string, string> = {}
  if (query.set) url.set = query.set
  if (query.kind) url.kind = query.kind
  if (query.timeframe && query.timeframe !== 'alltime') url.timeframe = query.timeframe
  return url
}

// ---- Paths -------------------------------------------------------------------

/** An event page's path; the run key is URL-encoded. */
export function limitedPath(key: string, page: LimitedPage): string {
  return `/limited/${encodeURIComponent(key)}/${page}`
}

/** One of the run's matches, inside the event's pages (0.x `limited.match`). */
export function limitedMatchPath(key: string, token: string): string {
  return `${limitedPath(key, 'matches')}/${encodeURIComponent(token)}`
}

// ---- KPIs --------------------------------------------------------------------

export function percentText(value: number | null): string {
  return value !== null ? `${value}%` : NO_VALUE
}

export function avgRecordText(wins: number | null, losses: number | null): string {
  return wins !== null ? `${wins}-${losses ?? 0}` : NO_VALUE
}

// ---- Index row ---------------------------------------------------------------

/** The row's links: sealed has no picks to review, so its title goes to the deck. */
export function rowLinks(row: LimitedIndexRowView): { draft: string | null; deck: string | null; title: string | null } {
  const draft = row.runKey && row.kind !== 'sealed' ? limitedPath(row.runKey, 'draft') : null
  const deck = row.runKey ? limitedPath(row.runKey, 'deck') : null
  return { draft, deck, title: draft ?? deck }
}

export function versionsText(count: number): string {
  return `${count} deck version${count === 1 ? '' : 's'}`
}

export function avgPickText(seconds: number | null): string | null {
  return seconds === null ? null : `avg ${formatSeconds(seconds)} / pick`
}

export type RowInfo = {
  /** "6 packs" for sealed, else "40/42 picks". */
  count: string
  /** Only once a deck is registered. */
  versions: string | null
  avgPick: string | null
  /** "vs a · b". */
  opponents: string | null
  /** Shown only when there are no opponents. */
  note: string | null
}

/** The row's info lines (0.x `LimitedEventRow`). */
export function rowInfo(row: LimitedIndexRowView): RowInfo {
  const opponents = row.opponents.length ? `vs ${row.opponents.join(' · ')}` : null
  return {
    count: row.packs !== null ? `${row.packs} packs` : `${row.picksMade}/${row.picksExpected} picks`,
    versions: row.deckRegistered ? versionsText(row.versionCount) : null,
    avgPick: avgPickText(row.avgPickSeconds),
    opponents,
    note: opponents ? null : row.note,
  }
}

/**
 * A state badge tints an outline rather than filling it (0.x `StateBadge`);
 * a live run (`default`) also gets a pulsing dot.
 */
export function stateBadgeTint(variant: string): string {
  switch (variant) {
    case 'success':
      return 'border-emerald-500/30 text-emerald-300'
    case 'destructive':
      return 'border-rose-500/30 text-rose-300'
    case 'warning':
      return 'border-amber-500/30 text-amber-300'
    case 'default':
      return 'border-sky-400/30 text-sky-300'
    default:
      return ''
  }
}

// ---- Event sidebar -----------------------------------------------------------

/** The sidebar's record: muted before a match, emerald when level or ahead. */
export function recordTone(wins: number, losses: number): string {
  if (wins + losses === 0) return 'text-muted-foreground'
  return wins >= losses ? 'text-emerald-400' : 'text-rose-400'
}

// ---- Cards and colours -------------------------------------------------------

/** A card from the payload, or its `#id` stand-in without card data. */
export function cardFor(cards: LimitedCards, id: number): LimitedCardView {
  return (
    cards[id] ?? {
      catalogId: id,
      name: `#${id}`,
      resolved: false,
      type: null,
      subType: null,
      rarity: null,
      colors: '',
      manaCost: null,
      cmc: null,
      image: null,
      artCrop: null,
      localImage: null,
      localArtCrop: null,
      oracleId: null,
    }
  )
}

/** Colour letters present in a stored colour string (`G,U`), in WUBRG order. */
export function colourLetters(colors: string | null | undefined): string[] {
  if (!colors) return []
  return COLOR_ORDER.filter((letter) => colors.includes(letter))
}

/**
 * An index row's matches with the opponents' colours read when it was
 * expanded (`limited_run_colours`); the rows as they are until then.
 */
export function withOpponentColours(
  matches: RunMatchView[],
  colours: LimitedMatchColoursView[] | null | undefined,
): RunMatchView[] {
  if (!colours) return matches
  const byToken = new Map(colours.map((entry) => [entry.token, entry.opponentColors]))
  return matches.map((match) => ({ ...match, opponentColors: byToken.get(match.token) ?? null }))
}

// ---- Draft header and signals ------------------------------------------------

/** "Seat 3 of 8 · booster 154228 · HOB × 3 · 14 cards per pack · 38 min". */
export function draftSubtitle(header: DraftHeaderView, setCode: string | null): string {
  const parts: string[] = []
  if (header.seatIndex !== null) parts.push(`Seat ${header.seatIndex + 1} of ${header.seatCount}`)
  if (header.boosterCatalogId) parts.push(`booster ${header.boosterCatalogId}`)
  parts.push(`${setCode ?? '?'} × 3`)
  parts.push(`${header.packSize} cards per pack`)
  if (header.durationMinutes !== null) parts.push(`${header.durationMinutes} min`)
  return parts.join(' · ')
}

export function signalText(signal: ColorSignalView): string {
  return `${signal.wheeled} wheeled · ${signal.seenTwice} seen 2+`
}

/** The signal bar's width: never under 4% so an empty colour still shows a sliver. */
export function signalWidth(share: number | null): string {
  return `${Math.max(4, (share ?? 0) * 100)}%`
}

// ---- Picks -------------------------------------------------------------------

export function pickLabel(pack: number, pick: number): string {
  return `P${pack}p${pick}`
}

/** "P2p1" for an ordinal (0.x `ordinalLabel`). */
export function ordinalLabel(ordinal: number, packSize = 14): string {
  const pack = Math.floor((ordinal - 1) / packSize) + 1
  const pick = ((ordinal - 1) % packSize) + 1
  return pickLabel(pack, pick)
}

export function pickHeading(pick: DraftPickView): string {
  return `Pack ${pick.packNumber} · Pick ${pick.pickNumber}`
}

/** The pick select's groups: picks by pack, packs in order. */
export function pickGroups(picks: DraftPickView[]): [number, DraftPickView[]][] {
  const groups = new Map<number, DraftPickView[]>()
  for (const pick of picks) {
    const list = groups.get(pick.packNumber) ?? []
    list.push(pick)
    groups.set(pick.packNumber, list)
  }
  for (const list of groups.values()) list.sort((a, b) => a.ordinal - b.ordinal)
  return [...groups.entries()].sort(([a], [b]) => a - b)
}

/** The pick asked for when the draft has it, else the first pick (0.x `DraftController`). */
export function selectPick(picks: DraftPickView[], requested: number | null): number | null {
  if (requested !== null && picks.some((pick) => pick.ordinal === requested)) return requested
  return picks[0]?.ordinal ?? null
}

/** The pick before (`-1`) or after (`1`) the current one; null at either end. */
export function stepPick(picks: DraftPickView[], ordinal: number, offset: number): number | null {
  const index = picks.findIndex((pick) => pick.ordinal === ordinal)
  if (index < 0) return null
  return picks[index + offset]?.ordinal ?? null
}

/** "43s spare" or "1:15 over". */
export function marginText(seconds: number | null): string | null {
  if (seconds === null) return null
  return `${formatSeconds(Math.abs(seconds))} ${seconds >= 0 ? 'spare' : 'over'}`
}

/** The pick's time badge: "12s · 43s spare"; null without a pick time. */
export function pickBadgeTime(pick: DraftPickView): string | null {
  if (pick.elapsedSeconds === null) return null
  const margin = marginText(pick.marginSeconds)
  return margin ? `${pick.elapsedSeconds}s · ${margin}` : `${pick.elapsedSeconds}s`
}

export type PickTile = {
  key: string
  card: LimitedCardView
  picked: boolean
  wheeled: boolean
  /** Taken by someone else before the pack came back: dimmed. */
  gone: boolean
  /** "seen 2" off the pick and the wheel. */
  seen: string | null
  /** "passed ×2" off the pick and the wheel. */
  passed: string | null
}

/**
 * The pack grid: the pick first, the rest in the order the pack arrived,
 * padded with `fillers` up to the pack's opening size so the grid keeps its
 * height as picks shrink it.
 */
export function pickTiles(
  pick: DraftPickView,
  cards: LimitedCards,
  seenWheel: { [key in number]: SeenWheelView },
  packSize: number,
): { tiles: PickTile[]; fillers: number } {
  const built = pick.available.map((id, index) => {
    const picked = id === pick.pickedCatalogId
    const wheeled = pick.wheeledIds.includes(id)
    const fact = seenWheel[id]
    const plain = !picked && !wheeled
    return {
      key: `${id}-${index}`,
      card: cardFor(cards, id),
      picked,
      wheeled,
      gone: pick.wheelReturnOrdinal !== null && pick.takenIds.includes(id),
      seen: fact && plain && fact.seenCount > 1 ? `seen ${fact.seenCount}` : null,
      passed: fact && plain && fact.passedCount > 1 ? `passed ×${fact.passedCount}` : null,
    }
  })
  const tiles = [...built.filter((tile) => tile.picked), ...built.filter((tile) => !tile.picked)]
  return { tiles, fillers: Math.max(0, packSize - tiles.length) }
}

/** "Wheeled at pick 9: Qc (you took it). Gone: Qa."; null when the pack never came back. */
export function wheelSummary(
  pick: DraftPickView,
  cards: LimitedCards,
  seenWheel: { [key in number]: SeenWheelView },
): string | null {
  if (pick.wheelReturnOrdinal === null) return null
  const name = (id: number) => cardFor(cards, id).name
  const wheeled = pick.wheeledIds.map((id) => (seenWheel[id]?.wheeledToMe ? `${name(id)} (you took it)` : name(id)))
  const gone = pick.takenIds.map(name)
  return `Wheeled at pick ${pick.wheelReturnOrdinal}: ${wheeled.length ? wheeled.join(', ') : 'nothing'}. Gone: ${gone.length ? gone.join(', ') : 'nothing'}.`
}

// ---- Cross-draft card --------------------------------------------------------

/** The cross-draft card's rows (0.x `CrossDraftCard`). */
export function crossDraftRows(
  stat: CrossDraftStatsView,
  packSize: number,
): { label: string; value: string; extra: string | null }[] {
  return [
    {
      label: 'Taken',
      value: `${stat.timesTaken}×`,
      extra: stat.avgOrdinal !== null ? `· avg ${ordinalLabel(Math.round(stat.avgOrdinal), packSize)}` : null,
    },
    { label: 'Seen & passed', value: `${stat.timesPassed}×`, extra: null },
    { label: 'Wheeled', value: `${stat.timesWheeled} of ${stat.drafts} drafts`, extra: null },
    { label: 'Made your deck', value: stat.madeDeck > 0 ? `${stat.madeDeck} of ${stat.drafts}` : NO_VALUE, extra: null },
  ]
}

// ---- Pick notes --------------------------------------------------------------

/** Wait after the last keystroke before saving (0.x). */
export const NOTE_SAVE_DELAY_MS = 600

/** 0.x `UpdatePickNoteRequest` `max:2000`. */
export const NOTE_MAX_LENGTH = 2000

export type NoteStatus = 'idle' | 'dirty' | 'saving' | 'saved' | 'error'

export type NoteState = { status: NoteStatus; savedAtMs: number | null }

/** The editor's status text: "not saved", "unsaved", "saved HH:MM". */
export function noteStatusText(status: NoteStatus, savedAtMs: number | null): string | null {
  if (status === 'error') return 'not saved'
  if (status === 'dirty') return 'unsaved'
  if (status === 'saving') return null
  return savedAtMs !== null ? `saved ${timeLabel(savedAtMs)}` : null
}

/** A spinner while saving, a check once saved (or on arrival with a stored note time). */
export function noteStatusIcon(status: NoteStatus, savedAtMs: number | null): 'spinner' | 'check' | null {
  if (status === 'saving') return 'spinner'
  if (status === 'saved' || (status === 'idle' && savedAtMs !== null)) return 'check'
  return null
}

/**
 * The text a pick note box should show after a refetch brought `incoming`,
 * or `null` to leave the box alone. Typing not saved yet (unsaved, saving,
 * or refused) is never overwritten, nor is a note this editor saved after
 * the refetched one was read.
 */
export function refreshedNote(
  text: string,
  state: NoteState,
  incoming: { note: string | null; savedAtMs: number | null },
): string | null {
  if (state.status === 'dirty' || state.status === 'saving' || state.status === 'error') return null
  const note = incoming.note ?? ''
  if (note === text) return null
  if (state.savedAtMs !== null && incoming.savedAtMs !== null && incoming.savedAtMs < state.savedAtMs) return null
  return note
}

type SaverOptions = {
  /** The pick being edited. */
  ordinal: number
  /** When its note was last saved. */
  savedAtMs: number | null
  /** Saves one pick's note; rejects when the save is refused. */
  save: (ordinal: number, note: string) => Promise<void>
  now: () => number
  onChange: (state: NoteState) => void
}

/**
 * The pick note editor's saving (0.x `PickNoteEditor`): a save 600 ms after
 * the last keystroke, flushed on blur and on a pick change. What to save is
 * captured at typing time against the pick being edited, so a save that
 * lands after a pick change never writes or marks the wrong pick.
 */
export class PickNoteSaver {
  state: NoteState
  private activeOrdinal: number
  private pending: { ordinal: number; note: string } | null = null
  private timer: ReturnType<typeof setTimeout> | null = null

  constructor(private options: SaverOptions) {
    this.activeOrdinal = options.ordinal
    this.state = { status: 'idle', savedAtMs: options.savedAtMs }
  }

  /** The text changed. */
  input(note: string): void {
    this.pending = { ordinal: this.activeOrdinal, note }
    this.set({ status: 'dirty' })
    this.clearTimer()
    this.timer = setTimeout(() => {
      this.timer = null
      this.saveNow()
    }, NOTE_SAVE_DELAY_MS)
  }

  /** Save unsaved text now (blur, pick change, leaving the page). */
  flush(): void {
    this.clearTimer()
    if (this.state.status === 'dirty' && this.pending) this.saveNow()
  }

  /** Another pick is shown: save this one's text, then start over on it. */
  switchTo(ordinal: number, savedAtMs: number | null): void {
    this.flush()
    this.activeOrdinal = ordinal
    this.pending = null
    this.set({ status: 'idle', savedAtMs })
  }

  /** The box took a refetched note (`refreshedNote`): idle, saved at `savedAtMs`. */
  adopt(savedAtMs: number | null): void {
    this.set({ status: 'idle', savedAtMs })
  }

  private saveNow(): void {
    const snapshot = this.pending
    if (!snapshot) return
    this.pending = null
    this.set({ status: 'saving' })
    this.options.save(snapshot.ordinal, snapshot.note).then(
      () => {
        if (this.activeOrdinal !== snapshot.ordinal || this.state.status === 'dirty') return
        this.set({ status: 'saved', savedAtMs: this.options.now() })
      },
      () => {
        if (this.activeOrdinal !== snapshot.ordinal || this.state.status === 'dirty') return
        this.set({ status: 'error' })
      },
    )
  }

  private clearTimer(): void {
    if (this.timer) {
      clearTimeout(this.timer)
      this.timer = null
    }
  }

  private set(next: Partial<NoteState>): void {
    this.state = { ...this.state, ...next }
    this.options.onChange(this.state)
  }
}
