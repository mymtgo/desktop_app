/**
 * Beta-Binomial shrinkage for the card stats win rates (0.x
 * `lib/stats/shrinkage.ts`). Cards with few games are pulled toward the
 * deck's own win rate so a lucky 2-0 does not read as 100%.
 */

/** The win rate columns that are shrunk. */
export const SHRINK_KEYS = ['kept', 'cast', 'seen', 'pregame'] as const
export type ShrinkKey = (typeof SHRINK_KEYS)[number]

export type Perspective = 'mine' | 'theirs'

export interface ShrinkageInput {
  readonly wins: number
  readonly games: number
  /** The deck's win rate, 0 to 1. */
  readonly prior: number
  /** Pseudo-games (k), at least 0: the trust setting. */
  readonly strength: number
}

function clamp01(value: number): number {
  if (!Number.isFinite(value)) return 0.5
  if (value < 0) return 0
  if (value > 1) return 1
  return value
}

function nonNegative(value: number): number {
  return Number.isFinite(value) && value >= 0 ? value : 0
}

/**
 * `(wins + k * prior) / (games + k)`, clamped to 0 to 1. With no games and no
 * strength there is nothing to weigh: the prior.
 */
export function shrunkRate(input: ShrinkageInput): number {
  const prior = clamp01(Number.isFinite(input.prior) ? input.prior : 0.5)
  const wins = nonNegative(input.wins)
  const games = nonNegative(input.games)
  const strength = nonNegative(input.strength)
  const denominator = games + strength
  if (denominator === 0) return prior
  return clamp01((wins + strength * prior) / denominator)
}

/** The raw win rate, or null without games. */
export function rawRate(wins: number, games: number): number | null {
  if (!Number.isFinite(wins) || !Number.isFinite(games) || games <= 0) return null
  return clamp01(wins / games)
}

/** Their cards read as their win rate: `1 - rate`. */
export function perspectiveAdjust(rate: number, perspective: Perspective): number {
  return perspective === 'theirs' ? 1 - rate : rate
}
