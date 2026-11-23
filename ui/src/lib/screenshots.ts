/**
 * The deck, league and card stat screenshots' numbers and labels (release
 * spec section 11; 0.x `DeckScreenshot`, `LeagueScreenshot`,
 * `CardStatScreenshot` and `DeckCardStats`). 0.x's em dash placeholders are
 * written `-`.
 */
import type { CardStatView, GameMarkView, MarkView, ScreenshotCardView } from '@/bindings'
import type { Visibility } from '@/lib/cardStats/columns'
import { pct } from '@/lib/cardStats/format'

/** A missing value. */
export const NONE = '-'

// ---- Deck --------------------------------------------------------------------

export const CARD_WIDTH = 100
export const CARD_HEIGHT = Math.round(CARD_WIDTH / 0.717)
export const STACK_OFFSET = 22

export type StackItem = { name: string; imageBase64: string | null }
export type Stack = { items: StackItem[]; height: number }

function copies(card: ScreenshotCardView): StackItem[] {
  return Array.from({ length: card.quantity }, () => ({ name: card.name, imageBase64: card.imageBase64 }))
}

/** One stack per card, a copy per quantity (0.x `groupIntoStacks`). */
export function groupIntoStacks(cards: ScreenshotCardView[]): Stack[] {
  return cards.map((card) => ({
    items: copies(card),
    height: CARD_HEIGHT + (card.quantity - 1) * STACK_OFFSET,
  }))
}

/** Every sideboard copy, in stacks of up to `maxPerStack` (0.x `groupSideboardStacks`). */
export function groupSideboardStacks(cards: ScreenshotCardView[], maxPerStack = 4): Stack[] {
  const all = cards.flatMap(copies)
  const stacks: Stack[] = []
  for (let index = 0; index < all.length; index += maxPerStack) {
    const chunk = all.slice(index, index + maxPerStack)
    stacks.push({ items: chunk, height: CARD_HEIGHT + (chunk.length - 1) * STACK_OFFSET })
  }
  return stacks
}

/** The tallest curve bucket, at least 1. */
export function cmcMax(buckets: { count: number }[]): number {
  return Math.max(...buckets.map((bucket) => bucket.count), 1)
}

/** A curve bar's height in pixels (40 for the tallest; at least 3 when it has cards). */
export function cmcBarHeight(count: number, max: number): number {
  return Math.max((count / max) * 40, count > 0 ? 3 : 0)
}

function copiesOf(cards: ScreenshotCardView[]): number {
  return cards.reduce((sum, card) => sum + card.quantity, 0)
}

/** The footer: `60 cards main · 15 sideboard`. */
export function deckFooterText(
  nonLand: ScreenshotCardView[],
  lands: ScreenshotCardView[],
  sideboard: ScreenshotCardView[],
): string {
  return `${copiesOf(nonLand) + copiesOf(lands)} cards main · ${copiesOf(sideboard)} sideboard`
}

// ---- File names ----------------------------------------------------------------

function slug(text: string): string {
  return text
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-+|-+$/g, '')
}

/** The file name a screenshot is saved under when the clipboard refuses it. */
export function screenshotFileName(name: string | null | undefined, kind: 'deck' | 'league' | 'card'): string {
  const base = slug(name ?? '')
  return base ? `${base}-${kind}.png` : `${kind}.png`
}

// ---- League --------------------------------------------------------------------

/** `3-1`: the run's wins and losses. */
export function leagueScoreText(results: (MarkView | null)[]): string {
  const wins = results.filter((result) => result === 'W').length
  const losses = results.filter((result) => result === 'L').length
  return `${wins}-${losses}`
}

/** `Win` or `Loss`. */
export function resultText(result: MarkView): string {
  return result === 'W' ? 'Win' : 'Loss'
}

/** `(OTP)` or `(OTD)`, `null` when the side is unknown. */
export function playSideText(game: GameMarkView): string | null {
  if (game.onPlay === null) return null
  return game.onPlay ? '(OTP)' : '(OTD)'
}

/** Games 1 to 3, `null` where none was played. */
export function threeGames(games: GameMarkView[]): (GameMarkView | null)[] {
  return [0, 1, 2].map((index) => games[index] ?? null)
}

/** `12m`, or `-` without a duration. */
export function durationText(seconds: number | null): string {
  return seconds ? `${Math.round(seconds / 60)}m` : NONE
}

// ---- Card stat -----------------------------------------------------------------

export type Tone = 'good' | 'bad' | 'neutral'
export type StatBlock = { label: string; value: string; sub?: string; tone?: Tone }

/** Green above 55%, red below 45% (0.x `toneFor`). */
export function toneFor(rate: number | null): Tone {
  if (rate === null) return 'neutral'
  if (rate > 55) return 'good'
  if (rate < 45) return 'bad'
  return 'neutral'
}

function percent(rate: number | null): string {
  return rate === null ? NONE : `${rate}%`
}

function rateBlock(label: string, won: number, lost: number): StatBlock {
  const sample = won + lost
  const rate = pct(won, sample)
  return { label, value: percent(rate), sub: `(${sample})`, tone: toneFor(rate) }
}

/** The stat blocks of the visible columns, in 0.x's order (0.x `blocks`). */
export function cardStatBlocks(stat: CardStatView, columns: Visibility): StatBlock[] {
  const out: StatBlock[] = []
  if (columns.keptPct) {
    out.push({ label: 'Kept', value: percent(pct(stat.keptGames, stat.totalGames)), sub: `(${stat.keptGames})` })
  }
  if (columns.keptWinPct) out.push(rateBlock('Kept Win', stat.keptWon, stat.keptLost))
  if (columns.castPct) {
    out.push({ label: 'Cast', value: percent(pct(stat.castGames, stat.totalGames)), sub: `(${stat.castGames})` })
  }
  if (columns.castWinPct) out.push(rateBlock('Cast Win', stat.castWon, stat.castLost))
  if (columns.playedPct) {
    out.push({ label: 'Played', value: percent(pct(stat.playedGames, stat.totalGames)), sub: `(${stat.playedGames})` })
  }
  if (columns.kicked) out.push({ label: 'Kicked', value: stat.totalKicked > 0 ? String(stat.totalKicked) : NONE })
  if (columns.activated) {
    out.push({ label: 'Activated', value: stat.totalActivated > 0 ? String(stat.totalActivated) : NONE })
  }
  if (columns.pregamePct) {
    const rate = pct(stat.pregameGames, stat.totalGames)
    out.push({
      label: 'Pregame',
      value: rate === null || stat.pregameGames === 0 ? NONE : `${rate}%`,
      sub: stat.pregameGames > 0 ? `(${stat.pregameGames})` : undefined,
    })
  }
  if (columns.pregameWinPct) out.push(rateBlock('Pregame Win', stat.pregameWon, stat.pregameLost))
  if (columns.seenPct) {
    out.push({ label: 'Seen', value: percent(pct(stat.seenGames, stat.totalGames)), sub: `(${stat.seenGames})` })
  }
  if (columns.seenWinPct) out.push(rateBlock('Seen Win', stat.seenWon, stat.seenLost))
  if (columns.sbOutPct) {
    out.push({
      label: 'SB Out',
      value: percent(pct(stat.sidedOutGames, stat.postboardGames)),
      sub: `(${stat.sidedOutGames})`,
    })
  }
  if (columns.sbInPct) {
    out.push({
      label: 'SB In',
      value: percent(pct(stat.sidedInGames, stat.postboardGames)),
      sub: `(${stat.sidedInGames})`,
    })
  }
  if (columns.games) out.push({ label: 'Games', value: String(stat.totalGames) })
  return out
}

/** The board filter's chip: `Game 1`, `Postboard`, or none. */
export function boardChip(board: string): string | null {
  if (board === 'game1') return 'Game 1'
  if (board === 'postboard') return 'Postboard'
  return null
}

/** The play or draw filter's chip: `On the play`, `On the draw`, or none. */
export function playDrawChip(playDraw: string): string | null {
  if (playDraw === 'play') return 'On the play'
  if (playDraw === 'draw') return 'On the draw'
  return null
}

/** The chips under the card's name: board, play or draw, then the timeframe. */
export function contextChips(board: string | null, playDraw: string | null, timeframe: string): string[] {
  const chips: string[] = []
  if (board) chips.push(board)
  if (playDraw) chips.push(playDraw)
  chips.push(timeframe)
  return chips
}
