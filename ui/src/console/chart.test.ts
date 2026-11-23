import { describe, expect, it } from 'vitest'
import type { ConsoleValue } from '@/bindings'
import { chartAxes, chartPoints, chartRange, chartTicks, labelStep, niceMax, numericColumns } from './chart'

const text = (value: string): ConsoleValue => ({ kind: 'text', value })
const integer = (value: number): ConsoleValue => ({ kind: 'integer', value })
const real = (value: number | null): ConsoleValue => ({ kind: 'real', value })
const NULL: ConsoleValue = { kind: 'null' }

describe('numericColumns', () => {
  it('keeps columns whose values that are not NULL are all numbers', () => {
    expect(numericColumns(['a', 'b'], [[text('x'), integer(1)], [text('y'), NULL]])).toEqual([1])
  })

  it('needs at least one number, and every value a number', () => {
    const rows = [
      [NULL, integer(1), real(0.5), integer(2)],
      [NULL, text('3'), real(null), integer(4)],
    ]
    expect(numericColumns(['a', 'b', 'c', 'd'], rows)).toEqual([2, 3])
  })

  it('skips integers too big for a JavaScript number, and charts the rest', () => {
    const big: ConsoleValue = { kind: 'bigInteger', value: '9007199254740993' }
    const rows = [
      [text('x'), integer(1)],
      [text('y'), big],
    ]
    expect(numericColumns(['a', 'b'], rows)).toEqual([1])
    expect(chartPoints(rows, 0, 1)).toEqual([{ label: 'x', value: 1 }])
    expect(chartPoints([[big, integer(2)]], 0, 1)).toEqual([{ label: '9007199254740993', value: 2 }])
  })
})

describe('chartPoints', () => {
  it('drops the rows whose Y is NULL', () => {
    const rows = [
      [text('x'), integer(1)],
      [text('y'), NULL],
      [NULL, real(2.5)],
      [integer(7), real(null)],
    ]
    expect(chartPoints(rows, 0, 1)).toEqual([
      { label: 'x', value: 1 },
      { label: 'NULL', value: 2.5 },
    ])
  })
})

describe('niceMax', () => {
  it('is the smallest 1, 2, 2.5 or 5 times a power of ten that is at least n', () => {
    expect(niceMax(0)).toBe(1)
    expect(niceMax(7)).toBe(10)
    expect(niceMax(0.43)).toBe(0.5)
    expect(niceMax(212)).toBe(250)
    expect(niceMax(1)).toBe(1)
    expect(niceMax(2.1)).toBe(2.5)
  })
})

describe('chartAxes', () => {
  const columns = ['format', 'matches', 'wins']

  it('defaults to the first column and the first numeric column that is not X', () => {
    expect(chartAxes(columns, [1, 2], { x: null, y: null })).toEqual({ x: 0, y: 1 })
    expect(chartAxes(['matches', 'wins'], [0, 1], { x: null, y: null })).toEqual({ x: 0, y: 1 })
  })

  it('keeps choices the columns still hold and resets the others', () => {
    expect(chartAxes(columns, [1, 2], { x: 'wins', y: 'wins' })).toEqual({ x: 2, y: 2 })
    expect(chartAxes(columns, [1, 2], { x: 'gone', y: 'format' })).toEqual({ x: 0, y: 1 })
  })

  it('is null with no numeric column', () => {
    expect(chartAxes(['format'], [], { x: null, y: null })).toBeNull()
  })
})

describe('labelStep', () => {
  it('labels every point when they fit, every Nth when they do not', () => {
    expect(labelStep(10, 10)).toBe(1)
    expect(labelStep(60, 10)).toBe(6)
    expect(labelStep(1000, 12)).toBe(84)
    expect(labelStep(5, 0)).toBe(5)
  })
})

describe('chartRange', () => {
  it('runs from zero to niceMax of the largest value', () => {
    expect(chartRange([3, 7, 1])).toEqual({ top: 10, bottom: 0 })
    expect(chartRange([5])).toEqual({ top: 5, bottom: 0 })
  })

  it('tops out at zero when no value is above it', () => {
    expect(chartRange([-3, -40])).toEqual({ top: 0, bottom: -50 })
  })

  it('reaches below zero for mixed signs', () => {
    expect(chartRange([-3, 212])).toEqual({ top: 250, bottom: -5 })
  })

  it('is never empty', () => {
    expect(chartRange([0, 0])).toEqual({ top: 1, bottom: 0 })
    expect(chartRange([])).toEqual({ top: 1, bottom: 0 })
  })
})

describe('chartTicks', () => {
  const labels = (range: { top: number; bottom: number }) => chartTicks(range).map((tick) => tick.label)

  it('puts gridlines at quarters of the top', () => {
    expect(labels({ top: 10, bottom: 0 })).toEqual(['10', '7.5', '5', '2.5', '0'])
    expect(labels({ top: 1, bottom: 0 })).toEqual(['1', '0.75', '0.5', '0.25', '0'])
  })

  it('puts gridlines at quarters of the bottom below zero, with no stray top', () => {
    expect(labels({ top: 0, bottom: -50 })).toEqual(['0', '-12.5', '-25', '-37.5', '-50'])
  })

  it('covers both sides for mixed signs', () => {
    expect(labels({ top: 250, bottom: -5 })).toEqual(['250', '187.5', '125', '62.5', '0', '-1.25', '-2.5', '-3.75', '-5'])
  })

  it('keeps enough decimals for tiny steps', () => {
    expect(labels({ top: 0.0001, bottom: 0 })).toEqual(['0.0001', '0.000075', '0.00005', '0.000025', '0'])
    expect(chartTicks({ top: 0.0001, bottom: 0 }).map((tick) => tick.value)).toEqual([0.0001, 0.000075, 0.00005, 0.000025, 0])
  })

  it('groups thousands', () => {
    expect(labels({ top: 2000, bottom: 0 })[0]).toBe('2,000')
  })
})
