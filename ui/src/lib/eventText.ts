import type { EventKind, LeagueRecord, TournamentBlock, TournamentSeries, TournamentStage } from '@/bindings'

/**
 * The event card's shared text rules (event overlay spec 5.2): ordinals,
 * records, relative times, the event label, kind and stage labels.
 * Pure: times take `now` (UTC ms). The hosted overlay in the API carries a
 * copy of this file.
 */

/** 1st, 2nd, 3rd, 4th, 11th, 12th, 13th, 21st, 101st, 111th. */
export function ordinal(n: number): string {
  const lastTwo = n % 100
  if (lastTwo >= 11 && lastTwo <= 13) return `${n}th`
  const last = n % 10
  if (last === 1) return `${n}st`
  if (last === 2) return `${n}nd`
  if (last === 3) return `${n}rd`
  return `${n}th`
}

/** "12th of 64", or "12th" when the player count is unknown. */
export function placeText(place: number, players: number | null): string {
  return players === null ? ordinal(place) : `${ordinal(place)} of ${players}`
}

/** "3-1", or "3-1-1" only when there are draws. */
export function formatRecord(record: LeagueRecord): string {
  const draws = record.draws ?? 0
  return draws > 0 ? `${record.wins}-${record.losses}-${draws}` : `${record.wins}-${record.losses}`
}

/** Whole minutes until `atMs` (at least 1), or null once it has passed. */
function minutesUntil(atMs: number, now: number): number | null {
  if (atMs <= now) return null
  return Math.max(1, Math.round((atMs - now) / 60_000))
}

/** "~14 min", or "soon" once the estimate has passed. */
export function approxTime(atMs: number, now: number): string {
  const minutes = minutesUntil(atMs, now)
  return minutes === null ? 'soon' : `~${minutes} min`
}

/** "in ~14 min", or "soon" once the estimate has passed. */
export function inTime(atMs: number, now: number): string {
  const minutes = minutesUntil(atMs, now)
  return minutes === null ? 'soon' : `in ~${minutes} min`
}

/** Whether the time has passed or is at most two minutes away. */
export function startsShortly(atMs: number, now: number): boolean {
  return atMs - now <= 2 * 60_000
}

/** The words of a label, lower case, for the repeated-word check. */
function words(text: string): string[] {
  return text.toLowerCase().split(/[^a-z0-9]+/).filter((word) => word !== '')
}

/**
 * The event label's candidates (spec 5.2), longest first; the card shows
 * the first that fits beside the right chip. Never the raw event name: a
 * league is its format ("Modern", "Sealed"), a tournament its format plus
 * kind ("Modern Challenge"), then its kind alone ("Challenge"). The Format
 * switch off leaves the kind ("League", "Challenge"); the Event name switch
 * off leaves a tournament its format (a league is its format anyway). A
 * format that already ends in the kind word is not repeated. Empty when
 * nothing is left.
 */
export function eventLabels(
  format: string | null,
  kind: EventKind,
  series: TournamentSeries | null,
  display: { event: boolean; format: boolean },
): string[] {
  const formatWord = display.format ? (format ?? '').trim() : ''
  const kindWord = display.event ? kindLabel(kind, series) : ''
  if (kind === 'league') {
    const label = formatWord !== '' ? formatWord : kindWord
    return label === '' ? [] : [label]
  }
  const formatWords = words(formatWord)
  const repeats = kindWord !== '' && formatWords[formatWords.length - 1] === kindWord.toLowerCase()
  const full = [formatWord, repeats ? '' : kindWord].filter((part) => part !== '').join(' ')
  const labels: string[] = []
  for (const label of [full, kindWord]) {
    if (label !== '' && !labels.includes(label)) labels.push(label)
  }
  return labels
}

/** The kind label for Compact and the strip: League, Challenge, Showcase, Prelim, Qualifier, Event. */
export function kindLabel(kind: EventKind, series: TournamentSeries | null): string {
  if (kind === 'league') return 'League'
  switch (series) {
    case 'challenge':
      return 'Challenge'
    case 'showcase_challenge':
      return 'Showcase'
    case 'preliminary':
      return 'Prelim'
    case 'qualifier':
      return 'Qualifier'
    default:
      return 'Event'
  }
}

/** A playoff stage: the quarterfinal is "Top 8" everywhere. Empty for Swiss. */
export function stageLabel(stage: TournamentStage): string {
  switch (stage) {
    case 'top8':
      return 'Top 8'
    case 'semifinal':
      return 'Semifinal'
    case 'final':
      return 'Final'
    default:
      return ''
  }
}

/** The short stage for the tightest slots (Versus Compact, the compact strip). */
export function stageShort(stage: TournamentStage): string {
  return stage === 'semifinal' ? 'SF' : stageLabel(stage)
}

/** The stage after one played (waiting between playoff rounds). */
export function nextStage(stage: TournamentStage): TournamentStage {
  if (stage === 'top8') return 'semifinal'
  return 'final'
}

/** A top cut finish by the stage lost: Top 8, Top 4, Finalist. */
export function finishLabel(stage: TournamentStage): string {
  if (stage === 'semifinal') return 'Top 4'
  if (stage === 'final') return 'Finalist'
  return 'Top 8'
}

/**
 * After the last Swiss round, before the playoffs (the Swiss round count
 * known): what comes next, "Top 8 next" with a cut or "Results soon" without
 * one (a Preliminary). Null while Swiss rounds remain or the count is unknown.
 */
export function afterLastSwiss(block: TournamentBlock): string | null {
  if (block.stage !== 'swiss' || block.swissRounds === null || block.round < block.swissRounds) return null
  return block.cutSize === null ? 'Results soon' : `Top ${block.cutSize} next`
}
