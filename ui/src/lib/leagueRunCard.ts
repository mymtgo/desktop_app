import type { FinishesView, GameMarkView, OddsSourceView, RunStateView } from '@/bindings'

/**
 * Display helpers for the deck dashboard's league panels (0.x
 * `DeckDashboard` "League Finishes" and `LeagueRunCard`). The run's record,
 * dots and tix come from `lib/leagues`, shared with the league cards.
 */

/** The card's heading: the run being played, or the last one finished. */
export function runCardTitle(state: RunStateView): string {
  return state === 'active' ? 'League in progress' : 'Latest league'
}

/** A match's games won and lost, "2–1" (0.x used an en dash). */
export function gameRecordText(games: GameMarkView[]): string {
  const won = games.filter((game) => game.result === 'W').length
  const lost = games.filter((game) => game.result === 'L').length
  return `${won}–${lost}`
}

/** "1 round left", "3 rounds left". */
export function roundsLeftText(rounds: number): string {
  return `${rounds} ${rounds === 1 ? 'round' : 'rounds'} left`
}

/** "1 win" or "2 wins" still needed for a prize; null hides the line (0.x `v-if`). */
export function winsForPrizeText(wins: number | null): string | null {
  if (!wins) return null
  return `${wins} ${wins === 1 ? 'win' : 'wins'}`
}

/**
 * Where the projection came from. A deck that has barely played leagues is
 * predicted from its overall record instead, and saying so keeps the number
 * from reading as better informed than it is (0.x).
 */
export function oddsNote(source: OddsSourceView, winProbability: number): string {
  if (source === 'league') return `from a ${winProbability}% league win rate`
  if (source === 'deck') return `from this deck's ${winProbability}% overall win rate`
  return 'from an even chance each round, for want of a record'
}

/** "+3.5", "0", "-4": a plus on gains only (the expected tix). */
export function signedNumber(value: number): string {
  return `${value > 0 ? '+' : ''}${value}`
}

/** One "League Finishes" row. */
export type FinishBar = { key: string; label: string; count: number; percent: number; barClass: string }

function barClass(label: string): string {
  const [wins, losses] = label.split('-').map(Number)
  if (wins === undefined || losses === undefined) return 'bg-muted-foreground'
  if (wins > losses) return 'bg-success'
  if (wins < losses) return 'bg-destructive'
  return 'bg-muted-foreground'
}

/**
 * Finishes from 5-0 down, then the runs the player dropped from. Each bar is
 * sized against every run, drops included (0.x `leagueResultsTotal`).
 */
export function finishBars(finishes: FinishesView): FinishBar[] {
  let total = finishes.dropped
  for (const bucket of finishes.buckets) total += bucket.count
  const divisor = total || 1

  const bars: FinishBar[] = finishes.buckets.map((bucket) => ({
    key: bucket.label,
    label: bucket.label,
    count: bucket.count,
    percent: (bucket.count / divisor) * 100,
    barClass: barClass(bucket.label),
  }))
  bars.push({
    key: 'dropped',
    label: 'Drop',
    count: finishes.dropped,
    percent: (finishes.dropped / divisor) * 100,
    barClass: 'bg-destructive',
  })
  return bars
}
