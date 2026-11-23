import type { GamePip } from '@/bindings'
import type { LeagueDisplay, LeagueOverlayState, TournamentBlock } from '@/bindings'
import {
  afterLastSwiss,
  approxTime,
  eventLabels,
  finishLabel,
  formatRecord,
  inTime,
  nextStage,
  ordinal,
  placeText,
  stageLabel,
} from '@/lib/eventText'

/**
 * The event card's Classic text (overlay spec 9.2; 0.x `LeagueOverlayCard.vue`,
 * `OverlayGamePips.vue` at the `release_0.46.0` tip; event overlay spec 5.4
 * for tournaments): the status chip, the subline of the full and compact
 * cards, the title and the game pips. Pure, so every state's strings are
 * tested without mounting the card; times take `now`. The display choices
 * (`state.display`) decide which parts show; the hosted overlay in the API
 * carries a copy of this file.
 */

export type ChipTone = 'neutral' | 'amber' | 'green' | 'red' | 'sky' | 'trophy'

export interface CardChip {
  tone: ChipTone
  label: string
  /** An icon before the label: the swap arrows while sideboarding, the trophy on a win or a final. */
  icon?: 'swap' | 'trophy'
}

/** Every part on, games as dots: the card before the choices existed. Standing (new) is off. */
export const DEFAULT_DISPLAY: LeagueDisplay = {
  games: 'dots',
  event: true,
  format: true,
  title: true,
  progress: true,
  status: true,
  record: true,
  standing: false,
}

/** The state's choices, each missing one at its default (a 0.46 payload has none). */
export function displayOf(state: LeagueOverlayState): LeagueDisplay {
  return { ...DEFAULT_DISPLAY, ...state.display }
}

/** Parts joined with " · ", empty ones left out. */
function joined(parts: Array<string | null>): string {
  return parts.filter((part): part is string => part !== null && part !== '').join(' · ')
}

function capitalised(text: string): string {
  return text.charAt(0).toUpperCase() + text.slice(1)
}

/** "Round 4/6", or "Round 4" while the Swiss round count is unknown. */
function roundProgress(block: TournamentBlock): string {
  return block.swissRounds === null ? `Round ${block.round}` : `Round ${block.round}/${block.swissRounds}`
}

/** "Top 8!" when the cut is known. */
function cutText(block: TournamentBlock): string {
  return block.cutSize === null ? 'Made the cut!' : `Top ${block.cutSize}!`
}

/** The rank with Standing on, in the Swiss only ("12th of 64"); none before the first result. */
export function standingText(state: LeagueOverlayState): string | null {
  const block = state.tournament
  if (!block || !displayOf(state).standing || block.stage !== 'swiss' || block.rank === null) return null
  return placeText(block.rank, block.players)
}

/** The top-right chip: progress while playing or waiting, else the state; none when idle or switched off. */
export function cardChip(state: LeagueOverlayState, now: number): CardChip | null {
  if (state.tournament) return tournamentCard(state, now)?.chip ?? null
  const { status, match, progress } = state
  const display = displayOf(state)
  switch (status) {
    case 'in_game':
      return display.progress ? { tone: 'neutral', label: `Match ${match?.number ?? 1}/${progress?.of ?? 5}` } : null
    case 'waiting':
      return display.progress ? { tone: 'neutral', label: `Next: match ${(progress?.played ?? 0) + 1}` } : null
    case 'sideboarding':
      return display.status ? { tone: 'amber', label: 'Sideboarding', icon: 'swap' } : null
    case 'complete':
      return display.status ? { tone: 'green', label: 'Complete' } : null
    case 'trophied':
      return display.status ? { tone: 'trophy', label: 'Trophied!', icon: 'trophy' } : null
    case 'dropped':
      return display.status ? { tone: 'red', label: 'Dropped' } : null
    default:
      return null
  }
}

/**
 * The event chip's labels, longest first (spec 5.2): a league's format
 * ("Modern"); a tournament's format plus kind ("Modern Challenge"), then its
 * kind ("Challenge"), by the Event name and Format switches. The chip shows
 * the first that fits beside the right chip, which always stays whole.
 */
export function eventLabelCandidates(state: LeagueOverlayState): string[] {
  if (!state.event) return []
  // The hosted copy can receive a null format (the API stores an empty one as null).
  return eventLabels(state.event.format ?? null, state.event.kind, state.tournament?.series ?? null, displayOf(state))
}

/** The full event label, where there is room for it (the Versus meta line and callouts). */
export function eventText(state: LeagueOverlayState): string | null {
  return eventLabelCandidates(state)[0] ?? null
}

/** The games show while a match is on, whatever the switches. */
export function showsGames(state: LeagueOverlayState): boolean {
  return state.status === 'in_game' || state.status === 'sideboarding'
}

/** The game after the ones played (the one sideboarded for); shared with the Versus card. */
export function nextGameNumber(state: LeagueOverlayState): number {
  return (state.match?.games.length ?? 0) + 1
}

/** The idle text under the title. */
export const IDLE_LINE = 'Join a league or Challenge on MTGO'

export interface CardTitle {
  text: string
  icon?: 'trophy'
}

/**
 * A Classic tournament card (spec 5.2, "Classic tournament title"): the
 * title says where the player is, the Full chip the state's one key detail,
 * the status lines their parts in the order they are dropped from the end.
 * `title` is null when its switch is off: the card then shows the archetype
 * title as a league card does, and the lines leave the archetype out.
 */
export interface TournamentCard {
  title: CardTitle | null
  chip: CardChip | null
  /** Classic Full, after the pips. */
  line: string[]
  /** Classic Compact, after the pips: no chip there, so it carries the chip's detail. */
  compactLine: string[]
}

/** The tournament title and whether the Progress switch (else Status) governs it. */
function tournamentTitle(state: LeagueOverlayState, block: TournamentBlock, now: number): { title: CardTitle; progress: boolean } | null {
  const playoff = block.stage !== 'swiss'
  switch (state.status) {
    case 'registered': {
      const start = block.startsAtMs
      const text = start === null ? 'Registered' : start <= now ? 'Starting soon' : capitalised(`starts ${inTime(start, now)}`)
      return { title: { text }, progress: false }
    }
    case 'in_game':
    case 'sideboarding':
      if (!playoff) return { title: { text: roundProgress(block) }, progress: true }
      return { title: block.stage === 'final' ? { text: 'Final', icon: 'trophy' } : { text: stageLabel(block.stage) }, progress: true }
    case 'waiting_for_round':
      return {
        title: {
          text: playoff ? `${stageLabel(nextStage(block.stage))} next` : (afterLastSwiss(block) ?? `Round ${block.round + 1} next`),
        },
        progress: true,
      }
    case 'bye':
      return { title: { text: roundProgress(block) }, progress: true }
    case 'made_cut':
      return { title: { text: cutText(block) }, progress: false }
    case 'trophied':
      return { title: { text: 'Champion', icon: 'trophy' }, progress: false }
    case 'top_cut':
      return { title: { text: finishLabel(block.stage) }, progress: false }
    case 'missed_cut':
      return { title: { text: 'Missed cut' }, progress: false }
    case 'complete':
      return { title: { text: 'Finished' }, progress: false }
    case 'dropped':
      return { title: { text: 'Dropped' }, progress: false }
    case 'eliminated':
      return { title: { text: 'Out' }, progress: false }
    default:
      return null
  }
}

/** The state's one key detail: the Full chip's label and tone, and the Compact line's words. */
function tournamentDetail(state: LeagueOverlayState, block: TournamentBlock, now: number): { chip: CardChip; compact: string[] } | null {
  const playoff = block.stage !== 'swiss'
  const next = block.nextRoundAtMs
  const place = block.place === null ? null : placeText(block.place, block.players)
  switch (state.status) {
    case 'registered':
      return { chip: { tone: 'neutral', label: 'Registered' }, compact: [] }
    case 'sideboarding':
      return { chip: { tone: 'amber', label: 'Sideboarding', icon: 'swap' }, compact: [`Game ${nextGameNumber(state)} next`] }
    case 'waiting_for_round': {
      const tone: ChipTone = playoff ? 'sky' : 'neutral'
      if (block.matchesLeft === 0) {
        if (next === null) return null
        return { chip: { tone, label: `Pairings ${approxTime(next, now)}` }, compact: [`Pairings ${inTime(next, now)}`] }
      }
      if (block.matchesLeft === null) return null
      // While the round runs there is no time estimate, only matches left (spec 5.2).
      const left = `${block.matchesLeft} ${block.matchesLeft === 1 ? 'match' : 'matches'} left`
      return { chip: { tone, label: left }, compact: [left] }
    }
    case 'bye':
      return { chip: { tone: 'neutral', label: 'Bye' }, compact: ['Bye', 'free win'] }
    case 'made_cut':
      if (next === null) return null
      return { chip: { tone: 'sky', label: `Playoffs ${approxTime(next, now)}` }, compact: [`Playoffs ${inTime(next, now)}`] }
    case 'trophied':
      return place === null ? null : { chip: { tone: 'trophy', label: place }, compact: [place] }
    case 'top_cut':
    case 'complete':
      return place === null ? null : { chip: { tone: 'green', label: place }, compact: [place] }
    case 'missed_cut':
      return place === null ? null : { chip: { tone: 'neutral', label: place }, compact: [place] }
    case 'dropped':
      return { chip: { tone: 'red', label: `After round ${block.round}` }, compact: [`After round ${block.round}`] }
    case 'eliminated':
      return { chip: { tone: 'neutral', label: `After round ${block.round}` }, compact: [`After round ${block.round}`] }
    default:
      return null
  }
}

/**
 * The Classic tournament card for `state`, or null on a league or idle card.
 * `label` is the streamer's deck label, shown in place of the archetype.
 */
export function tournamentCard(state: LeagueOverlayState, now: number, label = ''): TournamentCard | null {
  const block = state.tournament
  if (!block) return null
  const display = displayOf(state)
  const titled = tournamentTitle(state, block, now)
  const title = titled && (titled.progress ? display.progress : display.status) ? titled.title : null
  // The archetype moves to the line when the title shows the round; never both.
  const deck = display.title && title !== null ? label.trim() || deckTitle(state) : null
  const detail = display.status ? tournamentDetail(state, block, now) : null
  const standing = standingText(state)
  const chip = state.status === 'in_game' ? (standing === null ? null : { tone: 'neutral' as const, label: standing }) : (detail?.chip ?? null)

  const parts = (...items: Array<string | null>) => items.filter((item): item is string => item !== null && item !== '')
  const games = state.gameRecord && display.status ? `${state.gameRecord.won}-${state.gameRecord.lost} in games` : null
  let line: string[]
  switch (state.status) {
    case 'sideboarding':
      line = parts(display.status ? `Game ${nextGameNumber(state)} next` : null, deck)
      break
    case 'waiting_for_round':
      line = parts(deck, standing)
      break
    case 'bye':
      line = parts(display.status ? 'Free win' : null, deck)
      break
    case 'made_cut':
      line = parts(deck, display.status && block.rank !== null ? `${ordinal(block.rank)} after Swiss` : null)
      break
    case 'trophied':
      line = parts(deck, games)
      break
    default:
      line = parts(deck)
  }
  // Compact shows the archetype only while a match is live, and on a registered card (nothing else to say).
  const compactLine = state.status === 'in_game' || state.status === 'registered' ? parts(deck) : (detail?.compact ?? [])
  return { title, chip, line, compactLine }
}

/** A league's state text, before the status switch. */
function leagueStateText(state: LeagueOverlayState): string {
  const { status, gameRecord, progress } = state
  switch (status) {
    case 'sideboarding':
      return `Game ${nextGameNumber(state)} next`
    case 'waiting':
      return 'Looking for an opponent…'
    case 'complete':
    case 'trophied':
      return gameRecord ? `${gameRecord.won}-${gameRecord.lost} in games` : ''
    case 'dropped':
      return `Dropped after match ${progress?.played ?? 0}`
    case 'idle':
      return IDLE_LINE
    default:
      return ''
  }
}

/** The full card's text after the games; empty with status off (the rank by its own switch). */
export function fullSubline(state: LeagueOverlayState, now: number): string {
  const status = displayOf(state).status
  const card = tournamentCard(state, now)
  if (card) return card.line.join(' · ')
  return status ? leagueStateText(state) : ''
}

/**
 * The compact card's line lead while a game is played, longest first (the
 * card shows the first that fits): "Match 5/5" then "5 of 5" in a league,
 * the round or the stage in a tournament. None while sideboarding.
 */
export function compactLeads(state: LeagueOverlayState): string[] {
  // A tournament has no lead: the round is its title (spec 5.2).
  if (state.status !== 'in_game' || !displayOf(state).progress || state.tournament) return []
  const number = state.match?.number ?? 1
  const of = state.progress?.of ?? 5
  return [`Match ${number}/${of}`, `${number} of ${of}`]
}

/** The compact card's text after the games. */
export function compactSubline(state: LeagueOverlayState, now: number): string {
  const card = tournamentCard(state, now)
  if (card) return card.compactLine.join(' · ')
  const display = displayOf(state)
  switch (state.status) {
    case 'in_game':
      return ''
    case 'waiting':
      if (display.progress) return `Next: match ${(state.progress?.played ?? 0) + 1}`
      return display.status ? leagueStateText(state) : ''
    case 'complete':
    case 'trophied':
    case 'dropped':
      return cardChip(state, now)?.label ?? ''
    default:
      return display.status ? leagueStateText(state) : ''
  }
}

/** The untyped title: the archetype viewers recognise, else the MTGO deck name. */
export function deckTitle(state: LeagueOverlayState): string {
  if (state.status === 'idle') return 'Not in an event'
  return state.deck?.archetype ?? state.deck?.name ?? 'Unknown deck'
}

/** The compact card's title: the streamer's label, else the deck title. */
export function compactTitle(state: LeagueOverlayState, label: string): string {
  return label || deckTitle(state)
}

/** The deck whose label the full card's title edits; none when idle. */
export function labelDeckId(state: LeagueOverlayState): number | null {
  return state.deck && state.status !== 'idle' ? state.deck.id : null
}

/** The record ("3-1", "3-1-1" with draws); none without one or switched off. */
export function recordText(state: LeagueOverlayState): string | null {
  if (!displayOf(state).record) return null
  return state.record ? formatRecord(state.record) : null
}

/** The small mymtgo.com mark: idle, with status on. */
export function showsMark(state: LeagueOverlayState): boolean {
  return state.status === 'idle' && displayOf(state).status
}

/** Which rows of the full card have something to show. */
export function cardRows(state: LeagueOverlayState, now: number): { top: boolean; bottom: boolean } {
  const top = eventText(state) !== null || cardChip(state, now) !== null
  const bottom =
    displayOf(state).title || showsGames(state) || fullSubline(state, now) !== '' || recordText(state) !== null
  return { top, bottom }
}

export interface Pip {
  key: number
  won: boolean | null
  live: boolean
  /** A game row exists for it. */
  played: boolean
}

/**
 * The game pips: padded to best of three, or further when a draw pushed
 * the match past three games. While a game is played (`live`) its pip is
 * the last game's when that result is unknown, game 1 before any game row.
 */
export function pips(games: GamePip[], live: boolean): Pip[] {
  const total = Math.max(3, games.length)
  const liveIndex = Math.max(0, games.length - 1)
  const result: Pip[] = []
  for (let index = 0; index < total; index++) {
    const won = games[index]?.won ?? null
    result.push({ key: index, won, live: live && index === liveIndex && won === null, played: index < games.length })
  }
  return result
}

export type PipTone = 'won' | 'lost' | 'drawn' | 'live' | 'unplayed'

/** A pip's meaning: a played game with no result that is not live is a draw. */
export function pipTone(pip: Pip): PipTone {
  if (pip.won === true) return 'won'
  if (pip.won === false) return 'lost'
  if (pip.live) return 'live'
  return pip.played ? 'drawn' : 'unplayed'
}

/** The letter for each pip in letters mode. */
export const PIP_LETTERS: Record<PipTone, string> = { won: 'W', lost: 'L', drawn: 'D', live: '-', unplayed: '-' }
