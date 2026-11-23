import type {
  CommanderCardView,
  DecksChanged,
  GameDetailView,
  HandCardView,
  LeaguesChanged,
  MatchDeckView,
  MatchDetail,
  MatchRemoved,
  MatchUpdated,
  PlatformView,
  RatingView,
  RevealView,
  SideboardChangeView,
} from '@/bindings'
import { normalizeType, TYPE_ORDER } from './cardTypes'
import { lastPlayedText } from './decks'
import { MANA_COLORS } from './mana'
import { matchDateText } from './matchList'

// The match detail page's rules (0.x `MatchDetail.vue`, `MatchHero.vue`,
// `MatchGame.vue` and `OpponentRevealsRail.vue`), kept out of the components
// so they can be tested.

// Query keys. Not under `['matches']`: the live client treats every query
// there as a list page.
export const matchDetailKeys = {
  all: ['matchDetail'] as const,
  detail: (token: string) => ['matchDetail', token] as const,
  gameLog: (token: string, gameNumber: number) => ['matchDetail', token, 'log', gameNumber] as const,
}

// ---------------------------------------------------------------------------
// Refresh pushes

type Listen<T> = (cb: (event: { payload: T }) => void) => Promise<() => void>

/** The pushes the page listens to (the `events` of `@/bindings`). */
export interface DetailEventSource {
  matchUpdated: { listen: Listen<MatchUpdated> }
  matchRemoved: { listen: Listen<MatchRemoved> }
  decksChanged: { listen: Listen<DecksChanged> }
  leaguesChanged: { listen: Listen<LeaguesChanged> }
}

/**
 * Call `refresh` on every push that changes what the match page shows: its
 * own match's update or removal, any deck change (the sidebar, the deck
 * link), and any league change (the hero's league name). Resolves to the
 * function that stops listening.
 */
export async function listenForDetailChanges(
  source: DetailEventSource,
  token: () => string,
  refresh: () => void,
): Promise<() => void> {
  const stops = await Promise.all([
    source.matchUpdated.listen((e) => {
      if (e.payload.summary.token === token()) refresh()
    }),
    source.matchRemoved.listen((e) => {
      if (e.payload.token === token()) refresh()
    }),
    source.decksChanged.listen(() => refresh()),
    source.leaguesChanged.listen(() => refresh()),
  ])
  return () => stops.forEach((stop) => stop())
}

/** The notes field's limit (0.x `UpdateNotesController`). */
export const NOTES_MAX = 5000

export function notesTooLong(notes: string): boolean {
  return notes.length > NOTES_MAX
}

// ---------------------------------------------------------------------------
// Hero

/** "Win" or "Loss" in the hero. The outcome decides; without one, 0.x's rule (more games won). */
export function heroResult(detail: Pick<MatchDetail, 'outcome' | 'wins' | 'losses'>): { text: string; win: boolean } {
  let win: boolean
  if (detail.outcome === 'win') {
    win = true
  } else if (detail.outcome === 'loss') {
    win = false
  } else {
    win = detail.wins > detail.losses
  }
  return { text: win ? 'Win' : 'Loss', win }
}

/** "2–1", with 0.x's en dash. */
export function scoreText(wins: number, losses: number): string {
  return `${wins}–${losses}`
}

export function gamesCountText(count: number): string {
  return `${count} ${count === 1 ? 'game' : 'games'}`
}

/** The start in the viewer's timezone, as the matches table shows it ("7 Sep 2026, 3:05pm"). */
export function startedAtText(ms: number): string {
  return matchDateText(ms)
}

/** How long the match took, as Carbon's absolute `diffForHumans` ("23 minutes"); null while it runs. */
export function matchTimeText(startedAtMs: number, endedAtMs: number | null): string | null {
  if (endedAtMs === null) return null
  const text = lastPlayedText(startedAtMs, endedAtMs) ?? ''
  return text.replace(/ (ago|from now)$/, '')
}

/** An Endstep ranked match's line on the match page, "Rating 1371 to 1336"; the change beside it is `endstepRatingCell`. */
export function ratingText(rating: RatingView): string {
  return `Rating ${rating.before} to ${rating.after}`
}

/** Where "Back to matches" goes when there is no history: the deck's matches, else every match. */
export function matchesFallbackPath(deck: MatchDeckView | null): string {
  return deck ? `/decks/${deck.id}/matches` : '/matches'
}

// ---------------------------------------------------------------------------
// Imported matches

/**
 * An imported match none of whose games has a hand, a deck list or reveals:
 * the page shows the imported banner and leaves out the hand, sideboard and
 * reveals panels, as for every imported match before 0.x detail was
 * imported (Plan O).
 */
export function importedWithoutDetail(detail: Pick<MatchDetail, 'imported' | 'games'>): boolean {
  return detail.imported && !detail.games.some(gameHasDetail)
}

/** A game shows something of its own: a hand, its deck list (sideboard changes) or reveals. */
function gameHasDetail(
  game: Pick<GameDetailView, 'openingHand' | 'mulliganedHands' | 'hasDeckList' | 'opponentCardsSeen'>,
): boolean {
  return (
    game.openingHand.length > 0 ||
    game.mulliganedHands.length > 0 ||
    game.hasDeckList ||
    game.opponentCardsSeen.length > 0
  )
}

// ---------------------------------------------------------------------------
// Game panel

/**
 * The game card's "Replay" button: a match that was not entered by hand or
 * read from MTGO's match history, and a game with frames (v1 adds the frames
 * check; 0.x opened an empty window).
 */
export function showReplayButton(
  match: Pick<MatchDetail, 'source'>,
  game: Pick<GameDetailView, 'hasFrames'>,
): boolean {
  return match.source !== 'manual' && match.source !== 'history' && game.hasFrames
}

/** A game tile's result badge: a game whose result was never observed is "Unknown", never a loss. */
export function gameTile(game: Pick<GameDetailView, 'result'>): { text: 'Win' | 'Loss' | 'Unknown'; tone: 'win' | 'loss' | 'unknown' } {
  if (game.result === 'win') return { text: 'Win', tone: 'win' }
  if (game.result === 'loss') return { text: 'Loss', tone: 'loss' }
  return { text: 'Unknown', tone: 'unknown' }
}

/** Where a platform has no match detail, the game panel says so: "Match detail is not available for Endstep matches yet." */
export function noDetailText(platform: PlatformView): string {
  const name = platform === 'endstep' ? 'Endstep' : platform === 'arena' ? 'MTG Arena' : 'MTGO'
  return `Match detail is not available for ${name} matches yet.`
}

/** 0.x cast an unknown play or draw to "on the draw"; kept so the page reads the same. */
export function playDrawText(onThePlay: boolean | null): string {
  return onThePlay ? 'On the play' : 'On the draw'
}

/** "12m 34s", or "45s" under a minute (0.x `BuildMatchGameData`); null without both ends. */
export function durationText(startedAtMs: number | null, endedAtMs: number | null): string | null {
  if (startedAtMs === null || endedAtMs === null) return null
  const totalSeconds = Math.trunc(Math.abs(endedAtMs - startedAtMs) / 1000)
  const mins = Math.floor(totalSeconds / 60)
  const secs = totalSeconds % 60
  return mins > 0 ? `${mins}m ${secs}s` : `${secs}s`
}

export function turnsText(turns: number | null): string | null {
  return turns === null ? null : String(turns)
}

/** The game tile's mulligan facts: one per side that mulliganed. */
export function mulliganStrip(
  game: Pick<GameDetailView, 'localMulligans' | 'opponentMulligans'>,
  opponentName: string,
): { label: string; value: string }[] {
  const cells: { label: string; value: string }[] = []
  if (game.localMulligans > 0) cells.push({ label: 'You mull', value: `×${game.localMulligans}` })
  if (game.opponentMulligans > 0) cells.push({ label: `${opponentName} mull`, value: `×${game.opponentMulligans}` })
  return cells
}

/** "Opening hand", with "(mulligan to N)" after a mulligan. */
export function openingHandTitle(localMulligans: number): string {
  return localMulligans > 0 ? `Opening hand (mulligan to ${7 - localMulligans})` : 'Opening hand'
}

/** A card of the opening hand and its pill. */
export type HandTile = { card: HandCardView; pill: 'bottomed' | 'draw' | null }

/**
 * The opening hand as the panel shows it: the app's order (mana value, then
 * name, each card's bottomed copies marked), with the turn 1 draw on the
 * draw last and its "T1 draw" pill.
 */
export function openingHandTiles(game: Pick<GameDetailView, 'openingHand'>): HandTile[] {
  const kept = game.openingHand.filter((card) => !card.firstDraw)
  const drawn = game.openingHand.filter((card) => card.firstDraw)
  return [...kept, ...drawn].map((card) => ({
    card,
    pill: card.bottomed ? 'bottomed' : card.firstDraw ? 'draw' : null,
  }))
}

export function handGridClass(count: number): string {
  return count >= 8 ? 'grid-cols-8' : 'grid-cols-7'
}

/** How many cards moved in or out. */
export function sideboardCount(changes: SideboardChangeView[]): number {
  return changes.reduce((sum, change) => sum + change.quantity, 0)
}

/** "+2" or "−1" (a minus sign, as 0.x). */
export function changeLabel(change: Pick<SideboardChangeView, 'kind' | 'quantity'>): string {
  return `${change.kind === 'in' ? '+' : '−'}${change.quantity}`
}

// ---------------------------------------------------------------------------
// Opponent reveals rail

/** The rail's game filter: every game, or one game number. */
export type RevealFilter = 'all' | number

/** One card across the games shown. */
export type AggregatedReveal = {
  name: string
  image: string | null
  localImage: string | null
  typeLine: string | null
  colors: string[]
  /** The games it was seen in, in game order. */
  games: number[]
  /** The most copies seen in one game. */
  quantity: number
}

export type RevealGroup = { type: string; count: number; cards: AggregatedReveal[] }

/** The filter buttons: none for a single game, else All and G1, G2, ... */
export function revealFilters(games: Pick<GameDetailView, 'number'>[]): RevealFilter[] {
  if (games.length <= 1) return []
  return ['all', ...games.map((game) => game.number)]
}

/** The group a card goes in: its first canonical type, its type line, or Other without one. */
export function revealType(typeLine: string | null): string {
  if (!typeLine) return 'Other'
  return normalizeType(typeLine)
}

/**
 * Every card seen in the filtered games, merged by name (first seen keeps its
 * image and type). The opponent's commanders (`hidden`, by catalog id or
 * name) have their own panel, so they are left out.
 */
export function aggregateReveals(
  games: Pick<GameDetailView, 'number' | 'opponentCardsSeen'>[],
  filter: RevealFilter,
  hidden: Pick<CommanderCardView, 'catalogId' | 'name'>[] = [],
): AggregatedReveal[] {
  const byName = new Map<string, AggregatedReveal>()
  for (const game of games) {
    if (filter !== 'all' && game.number !== filter) continue
    for (const card of game.opponentCardsSeen as RevealView[]) {
      if (hidden.some((commander) => commander.catalogId === card.catalogId || commander.name === card.name)) continue
      let entry = byName.get(card.name)
      if (entry === undefined) {
        entry = {
          name: card.name,
          image: card.image,
          localImage: card.localImage,
          typeLine: card.typeLine,
          colors: card.colors,
          games: [],
          quantity: 0,
        }
        byName.set(card.name, entry)
      }
      if (!entry.games.includes(game.number)) entry.games.push(game.number)
      entry.quantity = Math.max(entry.quantity, card.quantity)
    }
  }
  return [...byName.values()]
}

/** Cards grouped by type in 0.x's order (unknown types last), each group by name. */
export function groupReveals(reveals: AggregatedReveal[]): RevealGroup[] {
  const groups = new Map<string, AggregatedReveal[]>()
  for (const reveal of reveals) {
    const type = revealType(reveal.typeLine)
    const list = groups.get(type) ?? []
    list.push(reveal)
    groups.set(type, list)
  }
  const result: RevealGroup[] = []
  for (const [type, cards] of groups) {
    cards.sort((a, b) => a.name.localeCompare(b.name))
    result.push({ type, count: cards.reduce((sum, c) => sum + c.quantity, 0), cards })
  }
  result.sort((a, b) => (TYPE_ORDER[a.type] ?? 99) - (TYPE_ORDER[b.type] ?? 99))
  return result
}

/** A card row's left border in its colours (0.x `borderStyle`). */
export function revealBorderStyle(colors: string[]): Record<string, string> {
  // The five colours only: a colourless card has no shade.
  const shades = colors
    .map((c) => c.trim())
    .filter((c) => c !== 'C')
    .map((c) => MANA_COLORS[c])
    .filter((c): c is string => c !== undefined)
  if (shades.length === 0) {
    return { borderLeftColor: 'transparent', borderLeftWidth: '3px', borderLeftStyle: 'solid' }
  }
  if (shades.length === 1) {
    return { borderLeftColor: shades[0], borderLeftWidth: '3px', borderLeftStyle: 'solid' }
  }
  const pct = 100 / shades.length
  const stops = shades.map((c, i) => `${c} ${i * pct}% ${(i + 1) * pct}%`).join(', ')
  return {
    borderImage: `linear-gradient(to bottom, ${stops}) 1`,
    borderLeftWidth: '3px',
    borderLeftStyle: 'solid',
  }
}

/** The hover preview's top: level with the row, kept inside the window (0.x). */
export function previewTop(rowTop: number, windowHeight: number): number {
  const maxTop = windowHeight - 280
  return Math.max(8, Math.min(rowTop, maxTop))
}
