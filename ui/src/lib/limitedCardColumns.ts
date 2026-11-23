/**
 * The Cards page's table columns and sorting (0.x
 * `pages/limited/partials/limitedCardColumns.ts`).
 */
import type { LimitedCardRowView } from '@/bindings'

export type SortKey = 'pick' | 'name' | 'status' | 'gamesCast' | 'winPctCast' | 'seenCount' | 'wheeled' | 'prior'

export type SortDir = 'asc' | 'desc'

/** `draftOnly` columns describe picks, so sealed (no picks) hides them. */
export type CardColumn = { key: SortKey; label: string; align: 'left' | 'right' | 'center'; draftOnly?: boolean }

export const COLUMNS: CardColumn[] = [
  { key: 'name', label: 'Card', align: 'left' },
  { key: 'pick', label: 'Pick', align: 'left', draftOnly: true },
  { key: 'status', label: 'In deck', align: 'center' },
  { key: 'gamesCast', label: 'Games cast', align: 'right' },
  { key: 'winPctCast', label: 'Win % cast', align: 'right' },
  { key: 'seenCount', label: 'Seen', align: 'right', draftOnly: true },
  { key: 'wheeled', label: 'Wheeled', align: 'center', draftOnly: true },
  { key: 'prior', label: 'Prior drafts', align: 'right', draftOnly: true },
]

/** Ascending by default for these: the rest read best largest first. */
export const ASCENDING_BY_DEFAULT: SortKey[] = ['name', 'pick', 'status']

/** The columns shown: sealed has no picks, so the pick columns go. */
export function visibleColumns(sealed: boolean): CardColumn[] {
  return COLUMNS.filter((col) => !(sealed && col.draftOnly))
}

const STATUS_RANK: Record<string, number> = { main: 0, side: 1, pool: 2, cut: 3 }

function statusRank(status: string): number {
  return STATUS_RANK[status] ?? 4
}

export function compareRows(
  a: LimitedCardRowView,
  b: LimitedCardRowView,
  key: SortKey,
  names: (id: number) => string,
): number {
  switch (key) {
    case 'name':
      return names(a.catalogId).localeCompare(names(b.catalogId))
    case 'pick':
      return (a.ordinals[0] ?? Number.MAX_SAFE_INTEGER) - (b.ordinals[0] ?? Number.MAX_SAFE_INTEGER)
    case 'status':
      return statusRank(a.status) - statusRank(b.status)
    case 'gamesCast':
      return a.gamesCast - b.gamesCast
    case 'winPctCast':
      return (a.winPctCast ?? -1) - (b.winPctCast ?? -1)
    case 'seenCount':
      return a.seenCount - b.seenCount
    case 'wheeled':
      return Number(a.wheeled) - Number(b.wheeled)
    case 'prior':
      return a.priorTaken - b.priorTaken
  }
}

export type CardSort = { key: SortKey; dir: SortDir }

/** A header click: the same column flips, another starts at its default direction. */
export function nextCardSort(current: CardSort, key: SortKey): CardSort {
  if (current.key === key) return { key, dir: current.dir === 'asc' ? 'desc' : 'asc' }
  return { key, dir: ASCENDING_BY_DEFAULT.includes(key) ? 'asc' : 'desc' }
}

/** The rows of one status (`all` for every row), sorted; descending is the ascending order reversed (0.x). */
export function sortCardRows(
  rows: LimitedCardRowView[],
  filter: string,
  sort: CardSort,
  names: (id: number) => string,
): LimitedCardRowView[] {
  const list = rows.filter((row) => filter === 'all' || row.status === filter)
  const sorted = [...list].sort((a, b) => compareRows(a, b, sort.key, names))
  return sort.dir === 'asc' ? sorted : sorted.reverse()
}
