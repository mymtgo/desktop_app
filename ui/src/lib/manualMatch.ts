/**
 * The manual match forms' rules (0.x `ManualMatchSheet.vue`,
 * `EditOpeningHandDialog.vue`, `EditSideboardDialog.vue` and
 * `EditRevealsDialog.vue`), kept out of the components so they can be tested.
 */
import type {
  DeckCardOptionView,
  GameDetailView,
  ManualGameInput,
  ManualHandInput,
  ManualLeagueOption,
  ManualRevealInput,
  ManualSideboardChangeInput,
  MatchCorrectionInput,
  MatchEditGameView,
  MatchEditView,
  RankGapKey,
  BestOfView,
  SideboardChangeView,
} from '@/bindings'

// ---------------------------------------------------------------------------
// The "Add manual match" sheet

/** A match's default length: Ended follows Started by this until it is edited. */
export const MATCH_MINUTES = 45

/** One game row of the sheet. `turns` is what the number input holds. */
export type GameRow = { result: 'W' | 'L' | ''; onPlay: 'play' | 'draw'; turns: string | number }

function pad(value: number): string {
  return value.toString().padStart(2, '0')
}

/** A local date and time as a `datetime-local` input holds it (`YYYY-MM-DDTHH:MM`). */
export function toLocalInput(date: Date): string {
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}T${pad(date.getHours())}:${pad(date.getMinutes())}`
}

/** A `datetime-local` value moved by `minutes`; empty when it is not a date. */
export function plusMinutes(value: string, minutes: number): string {
  if (!value) return ''
  const date = new Date(value)
  if (Number.isNaN(date.getTime())) return ''
  date.setMinutes(date.getMinutes() + minutes)
  return toLocalInput(date)
}

/** Ended: Started plus 45 minutes, until the player edits it. */
export function endedAtFor(startedAt: string, endedAt: string, endedTouched: boolean): string {
  return endedTouched ? endedAt : plusMinutes(startedAt, MATCH_MINUTES)
}

/** Three empty games, game 2 on the draw (0.x `emptyGames`). */
export function emptyGames(): GameRow[] {
  return [
    { result: '', onPlay: 'play', turns: '' },
    { result: '', onPlay: 'draw', turns: '' },
    { result: '', onPlay: 'play', turns: '' },
  ]
}

/**
 * The rows a kind plays: Bo1 is one game; Bo3 is three, keeping the rows
 * already there and taking the rest back from `setAside` (the rows before
 * Bo1 was chosen), then empty rows. A row taken back that no longer unlocks
 * (game 1 changed meanwhile) loses its result, as a result click does.
 */
export function gamesForKind(games: GameRow[], kind: BestOfView, setAside: GameRow[] = []): GameRow[] {
  if (kind === 'bo1') return games.slice(0, 1)
  const empty = emptyGames()
  const rows = empty.map((blank, index) => ({ ...(games[index] ?? setAside[index] ?? blank) }))
  for (let index = games.length; index < rows.length; index += 1) {
    if (!isGameEnabled(rows, index)) rows[index].result = ''
  }
  return rows
}

/**
 * Rows unlock in order. A game is playable only when every game before it
 * has a result and the match is still undecided (neither side on two wins).
 * A 1-1 after two games is a valid draw and can be submitted as is.
 */
export function isGameEnabled(games: GameRow[], index: number): boolean {
  if (index === 0) return true
  const previous = games.slice(0, index)
  if (previous.some((g) => g.result === '')) return false
  const wins = previous.filter((g) => g.result === 'W').length
  const losses = previous.filter((g) => g.result === 'L').length
  return wins < 2 && losses < 2
}

/**
 * The rows after a result click: clicking the same result again clears it,
 * and later rows that no longer unlock lose their result. A locked row
 * changes nothing. Returns new rows.
 */
export function setResult(games: GameRow[], index: number, value: string): GameRow[] {
  const next = games.map((g) => ({ ...g }))
  if (!isGameEnabled(next, index)) return next
  const current = next[index].result
  next[index].result = current === value ? '' : (value as GameRow['result'])
  for (let i = index + 1; i < next.length; i++) {
    if (!isGameEnabled(next, i)) next[i].result = ''
  }
  return next
}

/** The number input gives numbers, or a string when cleared. */
export function parseTurns(value: string | number): number | null {
  const parsed = typeof value === 'number' ? value : Number.parseInt(value.trim(), 10)
  return Number.isFinite(parsed) ? parsed : null
}

/** The games to send: only those with a result, in order. */
export function gamesPayload(games: GameRow[]): ManualGameInput[] {
  return games
    .filter((g) => g.result !== '')
    .map((g) => ({ won: g.result === 'W', onPlay: g.onPlay === 'play', turns: parseTurns(g.turns) }))
}

/** Whether Add can be pressed. An Arena match needs no opponent (`opponentRequired` false). */
export function canSubmitMatch(form: {
  deckId: number | null
  opponentName: string
  games: GameRow[]
  saving: boolean
  opponentRequired: boolean
}): boolean {
  return (
    form.deckId !== null &&
    (!form.opponentRequired || form.opponentName.trim() !== '') &&
    form.games.some((g) => g.result !== '') &&
    !form.saving
  )
}

// ---------------------------------------------------------------------------
// The "Edit match" sheet (edit a match spec section 3)

/** The fields the edit sheet fills from a match. */
export type EditForm = { opponentName: string; startedAt: string; endedAt: string; games: GameRow[] }

/**
 * The game rows of a stored match: the empty rows, filled in game order. A
 * drawn or unknown game has no result, and a game that no longer unlocks
 * loses its result; a game with no known play or draw keeps the default.
 */
export function gameRowsFromView(games: MatchEditGameView[]): GameRow[] {
  const rows = emptyGames()
  games.slice(0, rows.length).forEach((game, index) => {
    rows[index] = {
      result: game.result === 'win' ? 'W' : game.result === 'loss' ? 'L' : '',
      onPlay: game.onPlay === null ? rows[index].onPlay : game.onPlay ? 'play' : 'draw',
      turns: game.turns ?? '',
    }
  })
  for (let i = 0; i < rows.length; i++) {
    if (!isGameEnabled(rows, i)) rows[i].result = ''
  }
  return rows
}

/** The sheet's fields for a match: its local start and end (blank for none) and its games. */
export function editFormFromView(view: MatchEditView): EditForm {
  return {
    opponentName: view.opponentName ?? '',
    startedAt: toLocalInput(new Date(view.startedAtMs)),
    endedAt: view.endedAtMs === null ? '' : toLocalInput(new Date(view.endedAtMs)),
    games: gameRowsFromView(view.games),
  }
}

/**
 * What Save sends: a blank opponent as none (the match keeps it unknown). A
 * time the player left as `opened` showed it, or a blank end, is sent as
 * `null` and the stored one is kept: the inputs show minutes, so sending it
 * back would drop its seconds.
 */
export function correctionPayload(
  form: EditForm,
  opened: Pick<EditForm, 'startedAt' | 'endedAt'>,
): MatchCorrectionInput {
  const opponent = form.opponentName.trim()
  const ended = form.endedAt.trim()
  return {
    opponentName: opponent === '' ? null : opponent,
    startedAt: form.startedAt === opened.startedAt ? null : form.startedAt,
    endedAt: ended === '' || form.endedAt === opened.endedAt ? null : form.endedAt,
    games: gamesPayload(form.games),
  }
}

/**
 * Add's rules without the deck: an opponent and at least one game. An Arena
 * match needs no opponent (`opponentRequired` false): a blank one clears it.
 */
export function canSubmitEdit(form: {
  opponentName: string
  games: GameRow[]
  saving: boolean
  opponentRequired: boolean
}): boolean {
  return canSubmitMatch({ ...form, deckId: 0 })
}

// ---------------------------------------------------------------------------
// Arena: ranked or unranked, and the rank gap a match was played in

/** The "Rank gap" select's value for no gap. */
export const NO_RANK_GAP = 'none'

/** A gap's select value (`fileId:byteOffset`). */
export function rankGapValue(key: RankGapKey): string {
  return `${key.fileId}:${key.byteOffset}`
}

/**
 * The gap a select value names, `null` for none. Not checked against the
 * current options: a gap gone after a refetch is still sent, so the store
 * refuses it with a `rank_gap` error rather than saving with no claim.
 */
export function rankGapFor(value: string): RankGapKey | null {
  const [fileId, byteOffset, ...rest] = value.split(':').map(Number)
  if (rest.length > 0 || !Number.isInteger(fileId) || !Number.isInteger(byteOffset)) return null
  return { fileId, byteOffset }
}

/** The leagues the select offers: not full, of this deck or of none (0.x `leagueOptions`). */
export function leagueOptionsFor(options: ManualLeagueOption[], deckId: number | null): ManualLeagueOption[] {
  return options.filter(
    (l) => l.matchCount < l.roundCount && (l.deckId === null || deckId === null || l.deckId === deckId),
  )
}

/** The chosen league, while the select still offers it. */
export function keepLeague(key: string | null, options: ManualLeagueOption[]): string | null {
  if (key === null) return null
  return options.some((l) => l.key === key) ? key : null
}

// ---------------------------------------------------------------------------
// Field errors (0.x's keys: `opponent_name`, `games.0.turns`, `kept_hand`, ...)

export type FieldErrors = { [key in string]: string }

/** The first `games.N.turns` error (the sheet shows one under the games). */
export function turnsError(errors: FieldErrors): string | null {
  return Object.entries(errors).find(([key]) => /^games\.\d+\.turns$/.test(key))?.[1] ?? null
}

/** The first error, as the dialogs show it. */
export function firstError(errors: FieldErrors): string | null {
  return Object.values(errors)[0] ?? null
}

/** The errors of the fields not on show (the deck on a deck's own page). */
export function errorsBesides(errors: FieldErrors, shown: string[]): string[] {
  return Object.entries(errors)
    .filter(([key]) => !shown.includes(key) && !/^games\.\d+\.turns$/.test(key))
    .map(([, message]) => message)
}

// ---------------------------------------------------------------------------
// The opening hand dialog

export const HAND_SIZE = 7
export const MAX_MULLIGANS = 6

/**
 * The hand being edited: one seven card hand per mulligan (empty when not
 * remembered), then the final hand last; `bottomed` holds indexes into the
 * final hand.
 */
export type HandState = { mulligans: number; hands: number[][]; bottomed: Set<number> }

/** The recorded hand: the opening hand with its bottomed cards marked, and the put-back hands. */
export function handStateFromGame(game: Pick<GameDetailView, 'localMulligans' | 'openingHand' | 'mulliganedHands'>): HandState {
  const mulligans = game.localMulligans
  const opening = game.openingHand.filter((c) => !c.firstDraw)
  const final = opening.map((c) => c.catalogId)
  const bottomed = new Set<number>()
  opening.forEach((c, i) => {
    if (c.bottomed) bottomed.add(i)
  })
  const putBack = game.mulliganedHands.map((hand) => hand.map((c) => c.catalogId))
  while (putBack.length < mulligans) putBack.push([])
  return { mulligans, hands: [...putBack.slice(0, mulligans), final], bottomed }
}

export function finalHand(state: HandState): number[] {
  return state.hands[state.mulligans] ?? []
}

/** A new mulligan count (0 to 6): the final hand stays, the put-back hands follow. */
export function setMulligans(state: HandState, value: number): HandState {
  const next = Math.min(MAX_MULLIGANS, Math.max(0, value))
  const final = finalHand(state)
  const putBack = state.hands.slice(0, state.mulligans)
  while (putBack.length < next) putBack.push([])
  let bottomed = state.bottomed
  if (bottomed.size > next) bottomed = new Set([...bottomed].slice(0, next))
  return { mulligans: next, hands: [...putBack.slice(0, next), final], bottomed }
}

/** Takes the card at `index` out of hand `stage`; the final hand's bottomed marks shift. */
export function removeFromHand(state: HandState, stage: number, index: number): HandState {
  const hands = state.hands.map((hand, i) => (i === stage ? hand.filter((_, j) => j !== index) : hand))
  if (stage !== state.mulligans) return { ...state, hands }
  const bottomed = new Set<number>()
  for (const i of state.bottomed) {
    if (i < index) bottomed.add(i)
    else if (i > index) bottomed.add(i - 1)
  }
  return { ...state, hands, bottomed }
}

/** Marks or unmarks a final hand card as bottomed, at most one per mulligan. */
export function toggleBottomed(state: HandState, index: number): HandState {
  if (state.mulligans === 0) return state
  const bottomed = new Set(state.bottomed)
  if (bottomed.has(index)) {
    bottomed.delete(index)
  } else if (bottomed.size < state.mulligans) {
    bottomed.add(index)
  }
  return { ...state, bottomed }
}

/** Why the hand cannot be saved yet; `null` when it can. */
export function handHint(state: HandState): string | null {
  if (finalHand(state).length !== HAND_SIZE) return `Final hand needs ${HAND_SIZE} cards.`
  if (state.bottomed.size !== state.mulligans) {
    return state.mulligans === 1 ? 'Mark the card you bottomed.' : `Mark the ${state.mulligans} cards you bottomed.`
  }
  const putBackOk = state.hands.slice(0, state.mulligans).every((h) => h.length === 0 || h.length === HAND_SIZE)
  if (!putBackOk) return 'A put-back hand is seven cards, or leave it empty if you do not remember it.'
  return null
}

/** What the hand dialog sends: the starting hand and the bottomed cards split out. */
export function handPayload(state: HandState): ManualHandInput {
  const final = finalHand(state)
  return {
    mulliganCount: state.mulligans,
    startingHand: final.filter((_, i) => !state.bottomed.has(i)),
    bottomed: final.filter((_, i) => state.bottomed.has(i)),
    mulliganedHands: state.hands.slice(0, state.mulligans),
  }
}

/** "Clear hand": no mulligans and no cards. */
export const CLEARED_HAND: ManualHandInput = { mulliganCount: 0, startingHand: [], bottomed: [], mulliganedHands: [] }

// ---------------------------------------------------------------------------
// The sideboard dialog

/** Copies taken out of the maindeck and brought in, by catalog id. */
export type SideboardState = { outs: Map<number, number>; ins: Map<number, number> }

export function sideboardStateFromGame(changes: SideboardChangeView[]): SideboardState {
  return {
    outs: new Map(changes.filter((c) => c.kind === 'out').map((c) => [c.catalogId, c.quantity])),
    ins: new Map(changes.filter((c) => c.kind === 'in').map((c) => [c.catalogId, c.quantity])),
  }
}

/** One copy more or fewer, between none and the copies the list has. Returns a new map. */
export function bumpSideboard(
  counts: Map<number, number>,
  card: Pick<DeckCardOptionView, 'catalogId' | 'quantity'>,
  delta: number,
): Map<number, number> {
  const next = Math.min(card.quantity, Math.max(0, (counts.get(card.catalogId) ?? 0) + delta))
  const copy = new Map(counts)
  if (next === 0) copy.delete(card.catalogId)
  else copy.set(card.catalogId, next)
  return copy
}

export function sideboardTotal(counts: Map<number, number>): number {
  return [...counts.values()].reduce((a, b) => a + b, 0)
}

/** What the sideboard dialog sends: the outs, then the ins. */
export function sideboardPayload(state: SideboardState): ManualSideboardChangeInput[] {
  return [
    ...[...state.outs.entries()].map(([catalogId, quantity]) => ({ catalogId, quantity, kind: 'out' as const })),
    ...[...state.ins.entries()].map(([catalogId, quantity]) => ({ catalogId, quantity, kind: 'in' as const })),
  ]
}

// ---------------------------------------------------------------------------
// The reveals dialog

/** At most this many copies of a revealed card (0.x). */
export const MAX_REVEAL_COPIES = 20

export type RevealRow = { catalogId: number; name: string; quantity: number }

export function revealRowsFromGame(game: Pick<GameDetailView, 'opponentCardsSeen'>): RevealRow[] {
  return game.opponentCardsSeen.map((c) => ({ catalogId: c.catalogId, name: c.name, quantity: c.quantity }))
}

/** A picked card: a new row, or one more copy of a row already there. */
export function addRevealCard(rows: RevealRow[], card: { catalogId: number; name: string }): RevealRow[] {
  if (rows.some((r) => r.catalogId === card.catalogId)) {
    return rows.map((r) =>
      r.catalogId === card.catalogId ? { ...r, quantity: Math.min(MAX_REVEAL_COPIES, r.quantity + 1) } : r,
    )
  }
  return [...rows, { catalogId: card.catalogId, name: card.name, quantity: 1 }]
}

/** One copy more or fewer of row `index`, between 1 and 20. */
export function bumpReveal(rows: RevealRow[], index: number, delta: number): RevealRow[] {
  return rows.map((r, i) =>
    i === index ? { ...r, quantity: Math.min(MAX_REVEAL_COPIES, Math.max(1, r.quantity + delta)) } : r,
  )
}

/** The other games with revealed cards, to copy from. */
export function revealCopySources<T extends Pick<GameDetailView, 'number' | 'opponentCardsSeen'>>(
  games: T[],
  gameNumber: number,
): T[] {
  return games.filter((g) => g.number !== gameNumber && g.opponentCardsSeen.length > 0)
}

export function revealsPayload(rows: RevealRow[]): ManualRevealInput[] {
  return rows.map(({ catalogId, quantity }) => ({ catalogId, quantity }))
}

// ---------------------------------------------------------------------------
// The card search

/** The shortest search the app answers (0.x `min:2`). */
export const MIN_SEARCH_CHARS = 2
/** How long the search waits for typing to stop (0.x). */
export const SEARCH_DEBOUNCE_MS = 250

/** The term to search for, or `null` when it is too short. */
export function searchTerm(value: string): string | null {
  const term = value.trim()
  return term.length >= MIN_SEARCH_CHARS ? term : null
}
