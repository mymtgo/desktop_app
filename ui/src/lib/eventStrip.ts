import type { OtherEvent, TournamentBlock } from '@/bindings'
import {
  afterLastSwiss,
  approxTime,
  finishLabel,
  formatRecord,
  inTime,
  kindLabel,
  nextStage,
  ordinal,
  stageLabel,
  stageShort,
  startsShortly,
} from '@/lib/eventText'

/**
 * The other event strip's text (event overlay spec 8.3): "KIND · text" on
 * Full, "Kind: short" on Compact (the strip uppercases both). The time and
 * the records are bold. Pure: times take `now`. The hosted overlay in
 * the API carries a copy.
 */

export interface StripSegment {
  text: string
  bold: boolean
}

const plain = (text: string): StripSegment => ({ text, bold: false })
const bold = (text: string): StripSegment => ({ text, bold: true })
const SEP = plain(' · ')

/** Segments with " · " between the groups, empty groups left out. */
function groups(...parts: Array<StripSegment[] | null>): StripSegment[] {
  const kept = parts.filter((part): part is StripSegment[] => part !== null && part.length > 0)
  return kept.flatMap((part, index) => (index === 0 ? part : [SEP, ...part]))
}

function recordOf(other: OtherEvent): string | null {
  return other.record ? formatRecord(other.record) : null
}

/** "ROUND 5" or the next playoff stage, for a tournament between rounds. */
function nextRoundName(block: TournamentBlock, compact: boolean): string {
  if (block.stage === 'swiss') return compact ? `R${block.round + 1}` : `Round ${block.round + 1}`
  const stage = nextStage(block.stage)
  return compact ? stageShort(stage) : stageLabel(stage)
}

/** The round being played ("ROUND 3", "SEMIFINAL"). */
function roundName(block: TournamentBlock, compact: boolean): string {
  if (block.stage === 'swiss') return compact ? `R${block.round}` : `Round ${block.round}`
  return compact ? stageShort(block.stage) : stageLabel(block.stage)
}

function tournamentFull(other: OtherEvent, block: TournamentBlock, now: number): StripSegment[] {
  const record = recordOf(other)
  const rec = record === null ? null : [bold(record)]
  const game = other.match?.game ?? 1
  switch (other.status) {
    case 'registered': {
      const start = block.startsAtMs
      if (start === null) return [plain('Registered')]
      return start <= now ? [plain('Starting '), bold('soon')] : [plain('Starts in '), bold(approxTime(start, now))]
    }
    case 'waiting_for_round': {
      // After the last Swiss round: "Top 8 next" or "Results soon", no round number (spec 8.3).
      const after = afterLastSwiss(block)
      if (after !== null) return groups([plain(after)], rec)
      // While the round runs there is no estimate: "Round 5 next" (spec 8.3).
      const next = block.nextRoundAtMs
      const time = next === null ? [plain(' next')] : next <= now ? [plain(' '), bold('soon')] : [plain(' in '), bold(approxTime(next, now))]
      return groups([plain(nextRoundName(block, false)), ...time], rec)
    }
    case 'bye':
      return groups([plain('Bye')], rec)
    case 'made_cut': {
      const cut = block.cutSize === null ? 'Made the cut!' : `Top ${block.cutSize}!`
      const next = block.nextRoundAtMs
      return next === null ? [plain(cut)] : [plain(`${cut} Playoffs ${inTime(next, now)}`)]
    }
    case 'in_game':
      return groups([plain(roundName(block, false))], [plain(`Game ${game}`)], rec)
    case 'sideboarding':
      return groups([plain(roundName(block, false))], [plain(`Sideboarding for game ${game}`)], rec)
    case 'trophied':
      return groups([plain('Champion')], rec)
    case 'top_cut':
      return groups([plain(finishLabel(block.stage))], block.place === null ? null : [plain(ordinal(block.place))])
    case 'missed_cut':
      return groups([plain('Missed cut')], block.place === null ? null : [plain(ordinal(block.place))], rec)
    case 'complete':
      return groups([plain('Complete')], block.place === null ? null : [plain(ordinal(block.place))], rec)
    case 'dropped':
      return [plain(`Dropped after R${block.round}`)]
    case 'eliminated':
      return [plain(`Out after R${block.round}`)]
    default:
      return rec ?? []
  }
}

function tournamentCompact(other: OtherEvent, block: TournamentBlock, now: number): StripSegment[] {
  const record = recordOf(other)
  const game = other.match?.game ?? 1
  const finished = (place: number | null) => {
    if (place === null) return record === null ? [] : [bold(record)]
    return record === null ? [plain(ordinal(place))] : [plain(`${ordinal(place)}, `), bold(record)]
  }
  switch (other.status) {
    case 'registered':
      return block.startsAtMs === null ? [plain('registered')] : [bold(approxTime(block.startsAtMs, now))]
    case 'waiting_for_round': {
      const after = afterLastSwiss(block)
      if (after !== null) return [plain(after === 'Results soon' ? 'results soon' : after)]
      return block.nextRoundAtMs === null
        ? [plain(`${nextRoundName(block, true)} next`)]
        : [plain(`${nextRoundName(block, true)} `), bold(approxTime(block.nextRoundAtMs, now))]
    }
    case 'bye':
      return [plain(`bye R${block.round}`)]
    case 'made_cut':
      return [plain(block.cutSize === null ? 'Playoffs next' : `Top ${block.cutSize} next`)]
    case 'in_game':
      return [plain(`${roundName(block, true)} G${game}`)]
    case 'sideboarding':
      return [plain(`${roundName(block, true)} SB G${game}`)]
    case 'trophied':
      return finished(block.place ?? 1)
    case 'top_cut':
    case 'missed_cut':
    case 'complete':
      return finished(block.place)
    case 'dropped':
      return [plain(`dropped R${block.round}`)]
    case 'eliminated':
      return [plain(`out R${block.round}`)]
    default:
      return record === null ? [] : [bold(record)]
  }
}

function leagueFull(other: OtherEvent): StripSegment[] {
  const record = recordOf(other)
  const run = record === null ? null : [plain('Run '), bold(record)]
  const match = other.match
  switch (other.status) {
    case 'in_game':
      return groups(
        [plain(`Game ${match?.game ?? 1}`)],
        match === null ? null : [bold(`${match.gamesWon}-${match.gamesLost}`)],
        run,
      )
    case 'sideboarding':
      return groups([plain(`Sideboarding for game ${match?.game ?? 1}`)], run)
    case 'trophied':
    case 'complete':
    case 'dropped': {
      const word = other.status === 'trophied' ? 'Trophied' : other.status === 'complete' ? 'Complete' : 'Dropped'
      return record === null ? [plain(word)] : [plain(`${word} `), bold(record)]
    }
    default:
      return run ?? []
  }
}

function leagueCompact(other: OtherEvent): StripSegment[] {
  const record = recordOf(other)
  const game = other.match?.game ?? 1
  switch (other.status) {
    case 'in_game':
      return record === null ? [plain(`G${game}`)] : [plain(`G${game}, `), bold(record)]
    case 'sideboarding':
      return [plain(`SB G${game}`)]
    default:
      return record === null ? [] : [bold(record)]
  }
}

/** The strip's segments: the kind, then the other event's state. */
export function stripSegments(other: OtherEvent, now: number, compact: boolean): StripSegment[] {
  const kind = kindLabel(other.kind, other.tournament?.series ?? null)
  const block = other.tournament
  const body = block
    ? compact
      ? tournamentCompact(other, block, now)
      : tournamentFull(other, block, now)
    : compact
      ? leagueCompact(other)
      : leagueFull(other)
  if (body.length === 0) return [plain(kind)]
  return compact ? [plain(`${kind}: `), ...body] : [plain(kind), SEP, ...body]
}

/** The status dot is lit (sky) while a match is live or a round is about to start; dim while waiting or finished. */
export function stripLive(other: OtherEvent, now: number): boolean {
  if (other.match || other.status === 'in_game' || other.status === 'sideboarding') return true
  const block = other.tournament
  if (!block) return false
  const waiting = ['registered', 'waiting_for_round', 'bye', 'made_cut'].includes(other.status)
  const at = other.status === 'registered' ? block.startsAtMs : block.nextRoundAtMs
  return waiting && at !== null && startsShortly(at, now)
}
