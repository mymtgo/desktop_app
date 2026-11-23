import type { GameStatsRowView, MatchHistoryView, MatchupView, PerGameView } from '@/bindings'
import { winRateTone } from './decks'

/**
 * Fewer matches than this and a matchup row sits below the "Less than 4
 * matches" divider (0.x `MatchupSpreadTable` `LOW_DATA_THRESHOLD`).
 */
export const LOW_DATA_THRESHOLD = 4

/**
 * Fewer matches than this and a win rate is a coin flip with a number on it,
 * so the archetype cannot be called best or worst (0.x `KpiBestWorstMatchups`).
 */
export const MIN_MATCHES = 3

export type SpreadSortKey = 'name' | 'matches' | 'record' | 'winrate' | 'game_winrate' | 'otp_winrate' | 'avg_turns'

export type SpreadSort = { key: SpreadSortKey; desc: boolean }

/** The spread's starting sort: most matches first. */
export const DEFAULT_SPREAD_SORT: SpreadSort = { key: 'matches', desc: true }

/** The spread's columns after Archetype; Bo1 has no games to count. */
export function spreadColumns(bo1: boolean): { key: SpreadSortKey; label: string; class: string }[] {
  const columns = [
    { key: 'record', label: 'Record', class: 'text-right' },
    { key: 'matches', label: 'Matches', class: 'text-right' },
    { key: 'winrate', label: 'Win %', class: 'text-right' },
    { key: 'game_winrate', label: 'Games', class: 'border-l border-border text-right' },
    { key: 'game_winrate', label: 'Game %', class: 'text-right' },
    { key: 'otp_winrate', label: 'OTP %', class: 'text-right' },
    { key: 'avg_turns', label: 'Avg Turns', class: 'text-right' },
  ] satisfies { key: SpreadSortKey; label: string; class: string }[]
  if (!bo1) return columns
  return columns.filter((column) => column.label !== 'Games' && column.label !== 'Game %')
}

/** A header click: the same column flips, another starts descending (name ascending). */
export function nextSpreadSort(current: SpreadSort, key: SpreadSortKey): SpreadSort {
  if (current.key === key) return { key, desc: !current.desc }
  return { key, desc: key !== 'name' }
}

function sortValue(matchup: MatchupView, key: SpreadSortKey): number | string {
  switch (key) {
    case 'name':
      return matchup.name
    case 'matches':
      return matchup.matches
    case 'record':
      return matchup.matchWins
    case 'winrate':
      return matchup.matchWinRate
    case 'game_winrate':
      return matchup.gameWinRate
    case 'otp_winrate':
      return matchup.otpWinRate
    case 'avg_turns':
      return matchup.avgTurns ?? 0
  }
}

/** The rows in the chosen order (a stable sort). */
export function sortMatchups(list: MatchupView[], sort: SpreadSort): MatchupView[] {
  return [...list].sort((a, b) => {
    const left = sortValue(a, sort.key)
    const right = sortValue(b, sort.key)
    const cmp = left < right ? -1 : left > right ? 1 : 0
    return sort.desc ? -cmp : cmp
  })
}

/** Rows with enough matches, and the ones shown under the divider. */
export function splitByData(list: MatchupView[]): { main: MatchupView[]; lowData: MatchupView[] } {
  return {
    main: list.filter((matchup) => matchup.matches >= LOW_DATA_THRESHOLD),
    lowData: list.filter((matchup) => matchup.matches < LOW_DATA_THRESHOLD),
  }
}

/** A rate's text colour (`winRateTone`): above 50 green, below 50 red, 50 plain. */
export function winRateTextClass(rate: number): string {
  const tone = winRateTone(rate)
  if (tone === 'win') return 'text-success'
  if (tone === 'loss') return 'text-destructive'
  return ''
}

/**
 * The dashboard's matchup spread card (0.x `KpiBestWorstMatchups`): among
 * archetypes faced `MIN_MATCHES` times or more, by match win rate then
 * matches, the best, the median (with three or more) and the worst (with
 * two or more).
 */
export function rankMatchups(spread: MatchupView[]): {
  best: MatchupView | null
  middle: MatchupView | null
  worst: MatchupView | null
} {
  const ranked = spread
    .filter((matchup) => matchup.matches >= MIN_MATCHES)
    .sort((a, b) => b.matchWinRate - a.matchWinRate || b.matches - a.matches)
  return {
    best: ranked[0] ?? null,
    middle: ranked.length > 2 ? ranked[Math.floor(ranked.length / 2)] : null,
    worst: ranked.length > 1 ? ranked[ranked.length - 1] : null,
  }
}

const GROUP_LABELS: Record<string, string> = {
  all_games: 'All Games',
  game_1: 'Game 1',
  game_2: 'Game 2',
  game_3: 'Game 3',
}

const SPLIT_LABELS: Record<string, string> = {
  overall: 'Overall',
  play: 'On Play',
  draw: 'On Draw',
}

const GROUPS = ['all_games', 'game_1', 'game_2', 'game_3']
const SPLITS = ['overall', 'play', 'draw']

/** The Game Stats rows by game (0.x `GameStatsTable` `groupedRows`). */
export function gameStatGroups(
  rows: GameStatsRowView[],
): { key: string; label: string; rows: (GameStatsRowView & { label: string })[] }[] {
  return GROUPS.map((group) => ({
    key: group,
    label: GROUP_LABELS[group],
    rows: SPLITS.flatMap((split) => {
      const row = rows.find((candidate) => candidate.group === group && candidate.split === split)
      return row ? [{ ...row, label: SPLIT_LABELS[split] }] : []
    }),
  }))
}

/** An average per game, two decimals; `-` without games. */
export function formatAverage(value: number | null): string {
  return value === null ? '-' : value.toFixed(2)
}

/** A win rate with one decimal; `-` without games. */
export function formatPercent(value: number | null): string {
  return value === null ? '-' : `${value.toFixed(1)}%`
}

/** Whether any row counts a game. */
export function hasGames(rows: GameStatsRowView[]): boolean {
  return rows.some((row) => row.wins + row.losses > 0)
}

/** One match of the drawer's trend, oldest first. */
export type TrendPoint = {
  index: number
  dateLabel: string
  opponent: string | null
  score: string
  outcome: string
  cumWins: number
  cumLosses: number
  cumDraws: number
  /** The win rate of every match so far, draws counted as played. */
  cumRate: number
}

/**
 * The drawer's "Win Rate Trend" (0.x `MatchupTrendChart`): the running win
 * rate walked oldest to newest over the match sequence (a few matches spread
 * over months would clump on a calendar axis). `history` is newest first.
 */
export function trendPoints(history: MatchHistoryView[]): TrendPoint[] {
  let cumWins = 0
  let cumLosses = 0
  let cumDraws = 0
  return [...history].reverse().map((match, index) => {
    if (match.outcome === 'win') cumWins += 1
    else if (match.outcome === 'loss') cumLosses += 1
    else if (match.outcome === 'draw') cumDraws += 1
    const total = cumWins + cumLosses + cumDraws
    return {
      index,
      dateLabel: match.dateLabel,
      opponent: match.opponent,
      score: match.score,
      outcome: match.outcome,
      cumWins,
      cumLosses,
      cumDraws,
      cumRate: total > 0 ? Math.round((cumWins / total) * 100) : 0,
    }
  })
}

/** The matches the trend labels: at most six, always the last (0.x `tickValues`). */
export function trendTicks(count: number): number[] {
  const maxTicks = 6
  if (count <= maxTicks) return Array.from({ length: count }, (_, index) => index)
  const step = Math.ceil((count - 1) / (maxTicks - 1))
  const ticks: number[] = []
  for (let index = 0; index < count; index += step) ticks.push(index)
  if (ticks[ticks.length - 1] !== count - 1) ticks.push(count - 1)
  return ticks
}

/** Games 1 to 3 of the screenshot, `null` where none was played (0.x `gameAt`). */
export function screenshotGames(perGame: PerGameView[]): { gameNumber: number; game: PerGameView | null }[] {
  return [1, 2, 3].map((gameNumber) => ({
    gameNumber,
    game: perGame.find((game) => game.gameNumber === gameNumber) ?? null,
  }))
}

/** The screenshot's file name when it is saved instead of copied. */
export function screenshotName(archetypeName: string): string {
  const slug = archetypeName
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-+|-+$/g, '')
  return slug ? `${slug}-matchup.png` : 'matchup.png'
}
