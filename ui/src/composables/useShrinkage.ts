import { computed, type ComputedRef, type MaybeRefOrGetter, toValue } from 'vue'
import { perspectiveAdjust, rawRate, SHRINK_KEYS, type Perspective, type ShrinkKey, shrunkRate } from '@/lib/cardStats/shrinkage'

/** The won and lost games a card stat holds for each shrunk column. */
export interface ShrinkableCardStat {
  readonly keptWon: number
  readonly keptLost: number
  readonly castWon: number
  readonly castLost: number
  readonly seenWon: number
  readonly seenLost: number
  readonly pregameWon: number
  readonly pregameLost: number
}

export interface ShrunkStat<T extends ShrinkableCardStat> {
  readonly raw: T
  readonly samples: Readonly<Record<ShrinkKey, number>>
  readonly rawRates: Readonly<Record<ShrinkKey, number | null>>
  readonly shrunk: Readonly<Record<ShrinkKey, number>>
}

interface Options<T extends ShrinkableCardStat> {
  stats: MaybeRefOrGetter<readonly T[]>
  /** The deck's win rate, 0 to 1. */
  prior: MaybeRefOrGetter<number>
  /** The trust setting (k). */
  strength: MaybeRefOrGetter<number>
  perspective: MaybeRefOrGetter<Perspective>
}

function winsFor(stat: ShrinkableCardStat, key: ShrinkKey): number {
  switch (key) {
    case 'kept':
      return stat.keptWon
    case 'cast':
      return stat.castWon
    case 'seen':
      return stat.seenWon
    case 'pregame':
      return stat.pregameWon
  }
}

function lossesFor(stat: ShrinkableCardStat, key: ShrinkKey): number {
  switch (key) {
    case 'kept':
      return stat.keptLost
    case 'cast':
      return stat.castLost
    case 'seen':
      return stat.seenLost
    case 'pregame':
      return stat.pregameLost
  }
}

/** Each stat with its samples, raw rates and shrunk rates, from the shown side. */
export function useShrinkage<T extends ShrinkableCardStat>(options: Options<T>): ComputedRef<ShrunkStat<T>[]> {
  return computed(() => {
    const stats = toValue(options.stats)
    const prior = toValue(options.prior)
    const strength = toValue(options.strength)
    const perspective = toValue(options.perspective)

    return stats.map((stat) => {
      const samples = {} as Record<ShrinkKey, number>
      const rawRates = {} as Record<ShrinkKey, number | null>
      const shrunk = {} as Record<ShrinkKey, number>
      for (const key of SHRINK_KEYS) {
        const wins = winsFor(stat, key)
        const games = wins + lossesFor(stat, key)
        samples[key] = games
        const raw = rawRate(wins, games)
        rawRates[key] = raw === null ? null : perspectiveAdjust(raw, perspective)
        shrunk[key] = perspectiveAdjust(shrunkRate({ wins, games, prior, strength }), perspective)
      }
      return { raw: stat, samples, rawRates, shrunk }
    })
  })
}
