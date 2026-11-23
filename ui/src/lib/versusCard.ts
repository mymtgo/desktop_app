import type { LeagueOverlayState, TournamentBlock } from '@/bindings'
import { afterLastSwiss, finishLabel, inTime, nextStage, ordinal, placeText, stageLabel, stageShort } from '@/lib/eventText'
import { deckTitle, displayOf, eventLabelCandidates, eventText, nextGameNumber, recordText, standingText } from '@/lib/leagueOverlayCard'

/**
 * The Versus league card's rules (league overlay themes spec 6): the life
 * bars, round pips, text, callouts and which animation a state change
 * plays; tournament states per event overlay spec 5.4. Games are "GAME n"
 * everywhere; ROUND n is only a tournament round. Pure and tested (times
 * take `now`); the hosted overlay in the API carries a copy.
 */

export const LOW_LIFE = 4
export const CHALLENGER = 'CHALLENGER'

/** A life as a share of the starting life, 0 to 1 (an empty bar for a start of 0 or less). */
export function lifeShare(life: number, start: number): number {
  if (start <= 0) return 0
  return Math.min(1, Math.max(0, life / start))
}

export function isLow(life: number): boolean {
  return life <= LOW_LIFE
}

export interface Bars {
  you: number
  opponent: number
  youLife: number | null
  opponentLife: number | null
}

/** The bars: live life in game, full between games (numbers from the last start seen), full and bare otherwise. */
export function bars(state: LeagueOverlayState, lastStart: { you: number; opponent: number } | null): Bars {
  const life = state.status === 'in_game' ? (state.match?.life ?? null) : null
  if (life) {
    return {
      you: lifeShare(life.you, life.startYou),
      opponent: lifeShare(life.opponent, life.startOpponent),
      youLife: life.you,
      opponentLife: life.opponent,
    }
  }
  if (state.status === 'sideboarding' && lastStart) {
    return { you: 1, opponent: 1, youLife: lastStart.you, opponentLife: lastStart.opponent }
  }
  return { you: 1, opponent: 1, youLife: null, opponentLife: null }
}

export type Side = 'you' | 'opponent'

/** A bar's share lost in one push: the white chunk that fades out. */
export interface DamageChunk {
  side: Side
  from: number
  to: number
}

/** A chunk for each side whose bar dropped between two pushes; none for the first. */
export function damageChunks(before: Bars | null, after: Bars): DamageChunk[] {
  if (!before) return []
  const chunks: DamageChunk[] = []
  for (const side of ['you', 'opponent'] as const) {
    if (after[side] < before[side]) chunks.push({ side, from: before[side], to: after[side] })
  }
  return chunks
}

/** Games won each side, two slots at least (a best of three). */
export function roundPips(state: LeagueOverlayState): { you: boolean[]; opponent: boolean[] } {
  const games = state.match?.games ?? []
  const won = games.filter((game) => game.won === true).length
  const lost = games.filter((game) => game.won === false).length
  const slots = Math.max(2, won, lost)
  const row = (count: number) => Array.from({ length: slots }, (_, index) => index < count)
  return { you: row(won), opponent: row(lost) }
}

/**
 * Your side's name: the streamer's label when one is set, else the deck
 * title; "YOU" with Archetype off. The label is the one the card holds
 * (typed or pushed), so the name is sized from the text actually shown.
 */
export function yourName(state: LeagueOverlayState, label = ''): string {
  if (!displayOf(state).title) return 'YOU'
  return label.trim() || deckTitle(state)
}

/** The opponent's name: their archetype as the game overlay shows it, else CHALLENGER. */
export function opponentName(state: LeagueOverlayState): string {
  return state.match?.opponentArchetype?.trim() || CHALLENGER
}

/**
 * The text under VS: "MATCH 4/5" (compact "M4/5") in a league, the live
 * match or the next one while waiting; "ROUND 4/6" (compact "R4/6") or the
 * playoff stage ("SEMIFINAL", compact "SF") in a tournament.
 */
export function matchText(state: LeagueOverlayState, compact: boolean): string | null {
  if (!displayOf(state).progress) return null
  const block = state.tournament
  if (block) {
    if (block.stage !== 'swiss') return (compact ? stageShort(block.stage) : stageLabel(block.stage)).toUpperCase()
    const of = block.swissRounds === null ? '' : `/${block.swissRounds}`
    return compact ? `R${block.round}${of}` : `ROUND ${block.round}${of}`
  }
  if (!state.progress) return null
  const number = state.match?.number ?? state.progress.played + 1
  return compact ? `M${number}/${state.progress.of}` : `MATCH ${number}/${state.progress.of}`
}

/** The text under VS is gold in the playoffs. */
export function matchGold(state: LeagueOverlayState): boolean {
  return state.tournament ? state.tournament.stage !== 'swiss' : false
}

/** A line whose record shows white and bold after the lead text ("MODERN CHALLENGE · **3-0** · 12TH OF 64"). */
export interface RecordLine {
  lead: string | null
  record: string | null
  /** After the record: the rank with Standing on. */
  tail?: string
}

function recordLine(lead: string | null, record: string | null, tail: string | null = null): RecordLine | null {
  if (lead === null && record === null && tail === null) return null
  return tail === null ? { lead, record } : { lead, record, tail }
}

/** "MODERN · 3-1", each part by its switch; a Swiss round adds the rank with Standing on ("MODERN CHALLENGE · 3-0 · 12TH OF 64"). */
export function bottomLine(state: LeagueOverlayState): RecordLine | null {
  return bottomLines(state)[0] ?? null
}

/**
 * The meta line's candidates, longest first (the card shows the first that
 * fits): the rank dropped first, then the event label stepped down to its
 * kind ("CHALLENGE"), then the label left out. Empty when nothing shows.
 */
export function bottomLines(state: LeagueOverlayState): RecordLine[] {
  const labels = eventLabelCandidates(state).map((label) => label.toUpperCase())
  const record = recordText(state)
  const tail = standingText(state)?.toUpperCase() ?? null
  const candidates: Array<RecordLine | null> = []
  if (tail !== null) candidates.push(recordLine(labels[0] ?? null, record, tail))
  for (const label of labels) candidates.push(recordLine(label, record))
  if (labels.length > 0 && record !== null) candidates.push(recordLine(null, record))
  if (labels.length === 0) candidates.push(recordLine(null, record))
  const lines: RecordLine[] = []
  for (const line of candidates) {
    if (line && !lines.some((kept) => JSON.stringify(kept) === JSON.stringify(line))) lines.push(line)
  }
  return lines
}

/** The compact centre cell: "M4/5 · 2-1" or "R4/6 · 3-0" by the switches, or the short game banner while sideboarding. */
export function compactMiddle(state: LeagueOverlayState): RecordLine | null {
  if (sideboardBanner(state)) return { lead: `GAME ${nextGameNumber(state)}`, record: null }
  return recordLine(matchText(state, true), recordText(state))
}

/**
 * How far a name is sized down to fit its slot: names up to 14 characters
 * keep the full size, up to 17 a step smaller, longer ones smaller again
 * (and truncated with an ellipsis as the last resort).
 */
export function nameFit(name: string): 'normal' | 'long' | 'longer' {
  const length = name.trim().length
  if (length <= 14) return 'normal'
  if (length <= 17) return 'long'
  return 'longer'
}

export interface Callout {
  title: string
  /** Every part joined; the card shows the longest of `subtitleLadder` that fits. */
  subtitle: string
  /** The subtitle's parts, in the order they are dropped from the end. */
  parts: string[]
  tone: 'cyan' | 'red' | 'gold' | 'grey' | 'white'
  pulse: boolean
}

function upperParts(parts: Array<string | null>): string[] {
  return parts.filter((part): part is string => part !== null && part !== '').map((part) => part.toUpperCase())
}

function made(title: string, parts: Array<string | null>, tone: Callout['tone'], pulse: boolean): Callout {
  const kept = upperParts(parts)
  return { title, subtitle: kept.join(' · '), parts: kept, tone, pulse }
}

/** The subtitle candidates, longest first: parts dropped from the end until one is left. */
export function subtitleLadder(shown: Callout): string[] {
  const ladder: string[] = []
  for (let count = shown.parts.length; count >= 1; count--) ladder.push(shown.parts.slice(0, count).join(' · '))
  return ladder.length > 0 ? ladder : ['']
}

/** A tournament's callout between and after matches (spec 5.4). */
function tournamentCallout(state: LeagueOverlayState, block: TournamentBlock, now: number, label: string): Callout | null {
  const record = recordText(state)
  const deck = displayOf(state).title ? yourName(state, label) : null
  const next = block.nextRoundAtMs
  const place = block.place === null ? null : placeText(block.place, block.players)
  switch (state.status) {
    case 'registered': {
      const start = block.startsAtMs
      const passed = start !== null && start <= now
      const when = start === null ? null : passed ? 'starting soon' : `starts ${inTime(start, now)}`
      return made('GET READY', [eventText(state), when], 'white', true)
    }
    case 'waiting_for_round': {
      const title =
        block.stage === 'swiss'
          ? (afterLastSwiss(block)?.toUpperCase() ?? `ROUND ${block.round + 1}`)
          : stageLabel(nextStage(block.stage)).toUpperCase()
      if (block.matchesLeft === 0) {
        return made(title, [deck, record, next === null ? null : `pairings ${inTime(next, now)}`], 'cyan', true)
      }
      const left = block.matchesLeft === null ? null : `${block.matchesLeft} ${block.matchesLeft === 1 ? 'match' : 'matches'} left`
      // While the round runs there is no time estimate, only matches left (spec 5.2).
      return made(title, [deck, record, left], 'cyan', true)
    }
    case 'bye':
      return made('BYE', [`round ${block.round}`, 'free win', record], 'cyan', false)
    case 'made_cut':
      return made(
        block.cutSize === null ? 'PLAYOFFS' : `TOP ${block.cutSize}`,
        [next === null ? null : `playoffs ${inTime(next, now)}`, block.rank === null ? null : `${ordinal(block.rank)} after Swiss`, record],
        'gold',
        false,
      )
    case 'trophied':
      return made('CHAMPION', [eventText(state), place, record], 'gold', false)
    case 'top_cut':
      return made(finishLabel(block.stage).toUpperCase(), [place === null ? null : `finished ${place}`, record], 'red', false)
    case 'missed_cut':
      return made('CONTINUE?', ['missed the cut', place, record], 'grey', false)
    case 'complete':
      return made('FINISHED', [eventText(state), place, record], 'white', false)
    case 'dropped':
      return made('GAME OVER', [`dropped after round ${block.round}`, record], 'grey', false)
    case 'eliminated':
      return made('GAME OVER', [`out after round ${block.round}`, record], 'grey', false)
    default:
      return null
  }
}

/**
 * The big text between matches; none while a match is on or with Status off.
 * `label` is the streamer's deck label, which the waiting subtitles start with.
 */
export function callout(state: LeagueOverlayState, now: number, label = ''): Callout | null {
  if (!displayOf(state).status) return null
  if (state.tournament) return tournamentCallout(state, state.tournament, now, label)
  const record = recordText(state)
  const games = state.gameRecord ? `${state.gameRecord.won}-${state.gameRecord.lost} in games` : null
  switch (state.status) {
    case 'waiting':
      return made('A CHALLENGER APPROACHES', ['Looking for an opponent', matchText(state, false), record], 'cyan', true)
    case 'trophied':
      return made('PERFECT', [eventText(state), record, games], 'gold', false)
    case 'complete': {
      const won = (state.record?.wins ?? 0) > (state.record?.losses ?? 0)
      const final = record ? `Final ${record}` : null
      return won ? made('YOU WIN', [eventText(state), final], 'red', false) : made('CONTINUE?', [eventText(state), final], 'grey', false)
    }
    case 'dropped':
      return made('GAME OVER', [`Dropped after match ${state.progress?.played ?? 0}`, record], 'grey', false)
    case 'idle':
      return made('INSERT COIN', ['Join an event on MTGO'], 'white', true)
    default:
      return null
  }
}

/** The banner while sideboarding: the next game. */
export function sideboardBanner(state: LeagueOverlayState): string | null {
  if (state.status !== 'sideboarding' || !displayOf(state).status) return null
  return `GAME ${nextGameNumber(state)} · SIDEBOARDING`
}

/** The intro before a game: "GAME n", or a playoff game 1's stage ("SEMIFINAL"). */
export function introText(state: LeagueOverlayState, game: number): string {
  const block = state.tournament
  if (block && block.stage !== 'swiss' && game === 1) return stageLabel(block.stage).toUpperCase()
  return `GAME ${game}`
}

export type VersusAnimation =
  | { kind: 'fight'; round: number }
  | { kind: 'ko'; loser: Side; final?: true }
  | { kind: 'draw'; final?: true }
  | { kind: 'round'; round: number }

export const ANIMATION_MS: Record<VersusAnimation['kind'], number> = { fight: 2200, ko: 2500, draw: 2000, round: 2400 }

/**
 * The match the card last saw, carried between pushes: its number and whether
 * its result is settled (two games won or lost). Null before any match.
 */
export type LastMatch = { number: number; decided: boolean } | null

function results(state: LeagueOverlayState): Array<boolean | null> {
  return state.match?.games.map((game) => game.won) ?? []
}

/**
 * The memory for the next push. A live match is recorded by its number and
 * decided state. When the match has left the live state, the memory stays,
 * marked decided once its final result (a K.O. or DRAW from the record) has
 * played.
 */
export function nextLastMatch(next: LeagueOverlayState, lastMatch: LastMatch, played: VersusAnimation[]): LastMatch {
  if (next.match) {
    return {
      number: next.match.number,
      decided: next.match.gamesWon >= 2 || next.match.gamesLost >= 2,
    }
  }
  const finalResult = played.some((animation) => 'final' in animation && animation.final === true)
  if (lastMatch && finalResult) return { ...lastMatch, decided: true }
  return lastMatch
}

/**
 * Whether this push completes the remembered match: a league's played count
 * moves past its number, or a tournament's record counts the round's match.
 * A record move for another match (an older one finishing after the
 * remembered one was abandoned) does not count. The match is
 * known only by the played count, so matches completing out of order, or a
 * switch to another run between pushes, can credit the callout to the wrong
 * match; that is cosmetic, and telling them apart would need a match id in
 * the payload.
 */
function completes(prev: LeagueOverlayState, next: LeagueOverlayState, lastMatch: { number: number }): boolean {
  if (prev.progress && next.progress) {
    return prev.progress.played < lastMatch.number && next.progress.played >= lastMatch.number
  }
  // A tournament has no progress: its match number is the round, and the
  // round's match completes when the record counts one more match while the
  // card still stands on that round (a later round's bye moves the record
  // with no match).
  if (prev.tournament && next.tournament && prev.record && next.record) {
    return prev.tournament.round === lastMatch.number && next.tournament.round === lastMatch.number && matchesIn(next.record) > matchesIn(prev.record)
  }
  return false
}

function matchesIn(record: { wins: number; losses: number; draws?: number | null }): number {
  return record.wins + record.losses + (record.draws ?? 0)
}

/**
 * What a state change plays, in order. The first state (no `prev`) plays
 * nothing: a reload never replays. Log order is not guaranteed, so a game's
 * result may arrive after the state turns sideboarding, and a match may leave
 * the live state before its record moves; `lastMatch` covers the second.
 */
export function versusAnimations(
  prev: LeagueOverlayState | null,
  next: LeagueOverlayState,
  lastMatch: LastMatch,
): VersusAnimation[] {
  if (!prev) return []
  const played: VersusAnimation[] = []
  const sameMatch = prev.match !== null && next.match !== null && prev.match.number === next.match.number
  const before = results(prev)
  const after = results(next)

  if (sameMatch) {
    for (let index = 0; index < after.length; index++) {
      if ((before[index] ?? null) !== null) continue
      if (after[index] === true) played.push({ kind: 'ko', loser: 'opponent' })
      if (after[index] === false) played.push({ kind: 'ko', loser: 'you' })
    }
    // A later game row appeared while the earlier game stayed null: it ended with no winner.
    const last = before.length - 1
    const movedOn = after.length > before.length
    if ((prev.status === 'in_game' || prev.status === 'sideboarding') && last >= 0 && before[last] === null && (after[last] ?? null) === null && movedOn) {
      played.push({ kind: 'draw' })
    }
  } else if (prev.record && next.record && lastMatch && !lastMatch.decided && completes(prev, next, lastMatch)) {
    // The match's end reached the record before the card saw its result: a
    // final knock out, or a draw when neither wins nor losses moved. A match
    // completed with no known result (Rust `Outcome::Unknown`, record
    // unmoved) also plays DRAW; accepted as rare.
    if (next.record.wins > prev.record.wins) played.push({ kind: 'ko', loser: 'opponent', final: true })
    else if (next.record.losses > prev.record.losses) played.push({ kind: 'ko', loser: 'you', final: true })
    else played.push({ kind: 'draw', final: true })
  }

  if (next.status === 'sideboarding' && prev.status !== 'sideboarding') {
    played.push({ kind: 'round', round: after.length + 1 })
  }
  if (next.status === 'in_game' && next.match) {
    const grew = after.length > before.length && before.length > 0
    if (prev.status !== 'in_game' || !sameMatch || grew) {
      played.push({ kind: 'fight', round: Math.max(1, after.length) })
    }
  }
  return played
}
