import { describe, expect, it } from 'vitest'
import type { LimitedCardRowView } from '@/bindings'
import { ASCENDING_BY_DEFAULT, COLUMNS, compareRows, nextCardSort, sortCardRows, visibleColumns } from './limitedCardColumns'

function row(catalogId: number, overrides: Partial<LimitedCardRowView> = {}): LimitedCardRowView {
  return {
    catalogId,
    oracleId: null,
    ordinals: [],
    labels: [],
    status: 'main',
    added: false,
    gamesCast: 0,
    castWon: 0,
    castLost: 0,
    winPctCast: null,
    seenCount: 0,
    wheeled: false,
    priorTaken: 0,
    priorAvgOrdinal: null,
    priorWheeled: 0,
    priorDrafts: 0,
    ...overrides,
  }
}

const names = (id: number) => ({ 1: 'Bravo', 2: 'Alpha', 3: 'Charlie' })[id] ?? `#${id}`

describe('card columns', () => {
  it("has 0.x's columns, the pick ones hidden for sealed", () => {
    expect(COLUMNS.map((col) => col.label)).toEqual([
      'Card',
      'Pick',
      'In deck',
      'Games cast',
      'Win % cast',
      'Seen',
      'Wheeled',
      'Prior drafts',
    ])
    expect(visibleColumns(false).length).toBe(8)
    expect(visibleColumns(true).map((col) => col.key)).toEqual(['name', 'status', 'gamesCast', 'winPctCast'])
    expect(ASCENDING_BY_DEFAULT).toEqual(['name', 'pick', 'status'])
  })

  it('compares rows by each column', () => {
    expect(compareRows(row(1), row(2), 'name', names)).toBeGreaterThan(0)
    expect(compareRows(row(1, { ordinals: [3] }), row(2, { ordinals: [] }), 'pick', names)).toBeLessThan(0)
    expect(compareRows(row(1, { status: 'cut' }), row(2, { status: 'side' }), 'status', names)).toBeGreaterThan(0)
    expect(compareRows(row(1, { winPctCast: null }), row(2, { winPctCast: 0 }), 'winPctCast', names)).toBeLessThan(0)
    expect(compareRows(row(1, { wheeled: true }), row(2), 'wheeled', names)).toBeGreaterThan(0)
    expect(compareRows(row(1, { priorTaken: 2 }), row(2, { priorTaken: 1 }), 'prior', names)).toBeGreaterThan(0)
  })

  it('flips the direction on the same column, else starts at its default', () => {
    expect(nextCardSort({ key: 'pick', dir: 'asc' }, 'pick')).toEqual({ key: 'pick', dir: 'desc' })
    expect(nextCardSort({ key: 'pick', dir: 'desc' }, 'pick')).toEqual({ key: 'pick', dir: 'asc' })
    expect(nextCardSort({ key: 'pick', dir: 'asc' }, 'gamesCast')).toEqual({ key: 'gamesCast', dir: 'desc' })
    expect(nextCardSort({ key: 'gamesCast', dir: 'desc' }, 'name')).toEqual({ key: 'name', dir: 'asc' })
  })

  it('filters by status and sorts, descending as the ascending order reversed', () => {
    const rows = [row(1, { status: 'main' }), row(2, { status: 'side' }), row(3, { status: 'main' })]
    const ids = (list: LimitedCardRowView[]) => list.map((r) => r.catalogId)
    expect(ids(sortCardRows(rows, 'all', { key: 'name', dir: 'asc' }, names))).toEqual([2, 1, 3])
    expect(ids(sortCardRows(rows, 'all', { key: 'name', dir: 'desc' }, names))).toEqual([3, 1, 2])
    expect(ids(sortCardRows(rows, 'main', { key: 'name', dir: 'asc' }, names))).toEqual([1, 3])
  })
})
