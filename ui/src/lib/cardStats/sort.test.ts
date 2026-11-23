import { describe, expect, it } from 'vitest'
import type { ShrunkStat } from '@/composables/useShrinkage'
import { nextSort, sortEntries, type SortState } from './sort'


function entry(over: Record<string, any>): ShrunkStat<any> {
  const raw = {
    name: 'Card',
    totalGames: 10,
    keptGames: 0,
    seenGames: 0,
    castGames: 0,
    playedGames: 0,
    pregameGames: 0,
    sidedOutGames: 0,
    sidedInGames: 0,
    postboardGames: 0,
    totalKicked: 0,
    totalActivated: 0,
    totalWarp: 0,
    ...over,
  }
  return {
    raw,
    samples: { kept: 0, cast: 0, seen: 0, pregame: 0, ...over.samples },
    rawRates: { kept: null, cast: null, seen: null, pregame: null },
    shrunk: { kept: 0.5, cast: 0.5, seen: 0.5, pregame: 0.5, ...over.shrunk },
  } as ShrunkStat<any>
}

const names = (entries: ShrunkStat<any>[]) => entries.map((e) => e.raw.name)

describe('nextSort', () => {
  it('starts name ascending and any other column descending', () => {
    const start: SortState = { key: 'name', desc: false }
    expect(nextSort(start, 'castPct')).toEqual({ key: 'castPct', desc: true })
    expect(nextSort(start, 'name')).toEqual({ key: 'name', desc: true })
    expect(nextSort({ key: 'castPct', desc: true }, 'castPct')).toEqual({ key: 'castPct', desc: false })
    expect(nextSort({ key: 'castPct', desc: true }, 'name')).toEqual({ key: 'name', desc: false })
  })
})

describe('sortEntries', () => {
  it('sorts by name ascending', () => {
    const rows = [entry({ name: 'B' }), entry({ name: 'A' })]
    expect(names(sortEntries(rows, { key: 'name', desc: false }))).toEqual(['A', 'B'])
  })

  it('puts a win rate column without data below every card with data', () => {
    const rows = [
      entry({ name: 'None', samples: { cast: 0 } as never }),
      entry({ name: 'Low', samples: { cast: 3 } as never, shrunk: { cast: 0.1 } as never }),
    ]
    expect(names(sortEntries(rows, { key: 'castWinPct', desc: true }))).toEqual(['Low', 'None'])
  })

  it('breaks a win rate tie on sample size', () => {
    const rows = [
      entry({ name: 'Few', samples: { cast: 11 } as never, shrunk: { cast: 0.82 } as never }),
      entry({ name: 'Many', samples: { cast: 78 } as never, shrunk: { cast: 0.821 } as never }),
    ]
    // Both show 82%: the larger sample ranks first when descending.
    expect(names(sortEntries(rows, { key: 'castWinPct', desc: true }))).toEqual(['Many', 'Few'])
  })

  it('sorts a rate column with no games below the others', () => {
    const rows = [entry({ name: 'Never', totalGames: 0 }), entry({ name: 'Some', castGames: 1 })]
    expect(names(sortEntries(rows, { key: 'castPct', desc: true }))).toEqual(['Some', 'Never'])
  })

  it('sorts a casting method column by its total', () => {
    const rows = [entry({ name: 'A', totalWarp: 1 }), entry({ name: 'B', totalWarp: 5 })]
    expect(names(sortEntries(rows, { key: 'warp', desc: true }))).toEqual(['B', 'A'])
  })

  it('does not change its input', () => {
    const rows = [entry({ name: 'B' }), entry({ name: 'A' })]
    sortEntries(rows, { key: 'name', desc: false })
    expect(names(rows)).toEqual(['B', 'A'])
  })
})
