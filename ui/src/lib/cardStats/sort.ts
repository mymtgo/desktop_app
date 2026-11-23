/**
 * Sorting the Card Stats rows (0.x `CardStatsView.vue`): name ascending by
 * default, any other column descending first, win rates tie-broken on sample
 * size, and a column without data below every card that has some.
 */
import type { CardStatView } from '@/bindings'
import type { ShrunkStat } from '@/composables/useShrinkage'
import { CASTING_METHOD_COLUMNS, type CastingMethodColumnKey } from './columns'
import type { ShrinkKey } from './shrinkage'

export type SortKey =
  | 'name'
  | 'keptPct'
  | 'keptWinPct'
  | 'seenPct'
  | 'seenWinPct'
  | 'castPct'
  | 'castWinPct'
  | 'playedPct'
  | 'kicked'
  | 'activated'
  | 'pregamePct'
  | 'pregameWinPct'
  | 'sbOutPct'
  | 'sbInPct'
  | 'games'
  | CastingMethodColumnKey

export type SortState = { key: SortKey; desc: boolean }

export const DEFAULT_SORT: SortState = { key: 'name', desc: false }

const SHRINK_KEY_BY_SORT: Partial<Record<SortKey, ShrinkKey>> = {
  keptWinPct: 'kept',
  castWinPct: 'cast',
  seenWinPct: 'seen',
  pregameWinPct: 'pregame',
}

/** A header click: the same column flips direction, another starts at its default. */
export function nextSort(current: SortState, key: SortKey): SortState {
  if (current.key === key) return { key, desc: !current.desc }
  return { key, desc: key !== 'name' }
}

function rateOrNone(num: number, denom: number): number {
  return denom > 0 ? num / denom : -1
}

function sortValue(entry: ShrunkStat<CardStatView>, key: SortKey): number | string {
  const stat = entry.raw
  const shrinkKey = SHRINK_KEY_BY_SORT[key]
  if (shrinkKey) {
    // Sort on the rounded display percentage so cards showing the same %
    // tie and fall through to the sample size, not to hidden decimals.
    return entry.samples[shrinkKey] > 0 ? Math.round(entry.shrunk[shrinkKey] * 100) : -1
  }
  const casting = CASTING_METHOD_COLUMNS.find((col) => col.key === key)
  if (casting) return stat[casting.statField]

  switch (key) {
    case 'name':
      return stat.name
    case 'keptPct':
      return rateOrNone(stat.keptGames, stat.totalGames)
    case 'seenPct':
      return rateOrNone(stat.seenGames, stat.totalGames)
    case 'castPct':
      return rateOrNone(stat.castGames, stat.totalGames)
    case 'playedPct':
      return rateOrNone(stat.playedGames, stat.totalGames)
    case 'kicked':
      return stat.totalKicked
    case 'activated':
      return stat.totalActivated
    case 'pregamePct':
      return rateOrNone(stat.pregameGames, stat.totalGames)
    case 'sbOutPct':
      return rateOrNone(stat.sidedOutGames, stat.postboardGames)
    case 'sbInPct':
      return rateOrNone(stat.sidedInGames, stat.postboardGames)
    case 'games':
      return stat.totalGames
    default:
      return 0
  }
}

/** The rows in `sort` order; the input is left as it is. */
export function sortEntries(entries: readonly ShrunkStat<CardStatView>[], sort: SortState): ShrunkStat<CardStatView>[] {
  const tieKey = SHRINK_KEY_BY_SORT[sort.key]
  const decorated = entries.map((entry) => ({
    entry,
    value: sortValue(entry, sort.key),
    tie: tieKey ? entry.samples[tieKey] : 0,
  }))
  decorated.sort((a, b) => {
    const cmp = a.value < b.value ? -1 : a.value > b.value ? 1 : a.tie - b.tie
    return sort.desc ? -cmp : cmp
  })
  return decorated.map((d) => d.entry)
}
