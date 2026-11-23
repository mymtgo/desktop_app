/** The Card Stats tab's server-side filters and how they become the command's query. */
import type { CardStatsQuery } from '@/bindings'
import type { Perspective } from './shrinkage'

/** The select value for "no filter" (0.x `__all__`). */
export const ALL = '__all__'

export type CardStatsFilters = {
  perspective: Perspective
  /** `__all__`, `game1` or `postboard`. */
  board: string
  /** `__all__`, `play` or `draw`. */
  playDraw: string
  /** `__all__` or an opponent archetype id. */
  archetype: string
}

export const DEFAULT_FILTERS: CardStatsFilters = { perspective: 'mine', board: ALL, playDraw: ALL, archetype: ALL }

/** The filters the stats are read with: Bo1 has no postboard, so a board chosen on Bo3 does not carry over. */
export function effectiveFilters(filters: CardStatsFilters, bo1: boolean): CardStatsFilters {
  return bo1 ? { ...filters, board: ALL } : filters
}

export function filtersToQuery(filters: CardStatsFilters, timeframe: string): CardStatsQuery {
  const archetype = filters.archetype === ALL ? Number.NaN : Number(filters.archetype)
  return {
    timeframe,
    board: filters.board === 'game1' || filters.board === 'postboard' ? filters.board : null,
    playDraw: filters.playDraw === 'play' || filters.playDraw === 'draw' ? filters.playDraw : null,
    archetype: Number.isInteger(archetype) && archetype >= 0 ? archetype : null,
    perspective: filters.perspective,
  }
}

/** Whether any filter is off its default (the filter bar stays shown, so it can be undone). */
export function filtersDiffer(filters: CardStatsFilters): boolean {
  return (
    filters.perspective !== DEFAULT_FILTERS.perspective ||
    filters.board !== DEFAULT_FILTERS.board ||
    filters.playDraw !== DEFAULT_FILTERS.playDraw ||
    filters.archetype !== DEFAULT_FILTERS.archetype
  )
}
