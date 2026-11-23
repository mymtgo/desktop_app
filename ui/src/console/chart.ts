import type { ConsoleValue } from '@/bindings'

// The console chart's rules: which columns can be a Y axis, the points of a
// result, the axis top and how often to label X. ResultsChart.vue draws them.

export type ChartPoint = { label: string; value: number }

/** The chosen columns, by name, so a choice survives a new result with the same column. */
export type ChartChoice = { x: string | null; y: string | null }

function numberOf(value: ConsoleValue | undefined): number | null {
  if (!value) return null
  if (value.kind === 'integer') return value.value
  if (value.kind === 'real') return value.value
  return null
}

function isNull(value: ConsoleValue | undefined): boolean {
  return !value || value.kind === 'null' || (value.kind === 'real' && value.value === null)
}

/** A value as the chart labels it (as the grid shows it). */
export function valueLabel(value: ConsoleValue | undefined): string {
  if (!value || isNull(value)) return 'NULL'
  switch (value.kind) {
    case 'integer':
    case 'real':
      return String(value.value)
    case 'text':
    case 'bigInteger':
      return value.value
    case 'blob':
      return `<blob ${value.value} bytes>`
    default:
      return 'NULL'
  }
}

/** The indices of the columns whose values that are not NULL are all numbers, with at least one. */
export function numericColumns(columns: string[], rows: ConsoleValue[][]): number[] {
  const numeric: number[] = []
  columns.forEach((_, index) => {
    let seen = false
    for (const row of rows) {
      const value = row[index]
      // An integer too big for a JavaScript number is left off the chart.
      if (isNull(value) || value?.kind === 'bigInteger') continue
      if (numberOf(value) === null) return
      seen = true
    }
    if (seen) numeric.push(index)
  })
  return numeric
}

/** One point per row, labelled by column `x`, valued by column `y`; rows with a NULL Y are dropped. */
export function chartPoints(rows: ConsoleValue[][], x: number, y: number): ChartPoint[] {
  const points: ChartPoint[] = []
  for (const row of rows) {
    const value = numberOf(row[y])
    if (value === null) continue
    points.push({ label: valueLabel(row[x]), value })
  }
  return points
}

/** The smallest of 1, 2, 2.5 and 5 times a power of ten that is at least `n` (1 for 0 or less). */
export function niceMax(n: number): number {
  if (!(n > 0)) return 1
  const power = Math.floor(Math.log10(n))
  for (let exponent = power - 1; exponent <= power + 1; exponent++) {
    for (const step of [1, 2, 2.5, 5]) {
      // toPrecision drops the float noise of 2.5 * 0.1 and the like.
      const candidate = Number((step * 10 ** exponent).toPrecision(12))
      if (candidate >= n) return candidate
    }
  }
  return Number((10 ** (power + 1)).toPrecision(12))
}

/**
 * The X and Y column indices for a result: the chosen columns while the
 * result still has them, otherwise X is the first column and Y the first
 * numeric column that is not X. Null when no column is numeric.
 */
export function chartAxes(columns: string[], numeric: number[], choice: ChartChoice): { x: number; y: number } | null {
  if (numeric.length === 0) return null
  const chosenX = choice.x === null ? -1 : columns.indexOf(choice.x)
  const x = chosenX >= 0 ? chosenX : 0
  const chosenY = choice.y === null ? -1 : columns.indexOf(choice.y)
  if (numeric.includes(chosenY)) return { x, y: chosenY }
  const y = numeric.find((index) => index !== x) ?? numeric[0]
  return { x, y }
}

/** Label every Nth X so at most `maxLabels` labels show (the first point is always labelled). */
export function labelStep(count: number, maxLabels: number): number {
  return Math.max(1, Math.ceil(count / Math.max(1, maxLabels)))
}

export type ChartRange = { top: number; bottom: number }
export type ChartTick = { value: number; label: string }

/**
 * The Y range of some values: up to niceMax of the largest value above zero
 * (zero when none is), down to niceMax below zero of the smallest value
 * below it (zero when none is). All zero or no values gives 0 to 1.
 */
export function chartRange(values: number[]): ChartRange {
  let low = 0
  let high = 0
  for (const value of values) {
    low = Math.min(low, value)
    high = Math.max(high, value)
  }
  const top = high > 0 ? niceMax(high) : 0
  const bottom = low < 0 ? -niceMax(-low) : 0
  if (top === 0 && bottom === 0) return { top: 1, bottom: 0 }
  return { top, bottom }
}

/** A tick value without float noise (0.0001 * 3 / 4 is 0.000075, not 0.00007500000000000001). */
function clean(value: number): number {
  return Number(value.toPrecision(12))
}

/** The decimals a value needs to be shown exactly (at most 20). */
function decimalsOf(value: number): number {
  for (let digits = 0; digits < 20; digits++) {
    if (Number(value.toFixed(digits)) === value) return digits
  }
  return 20
}

/**
 * The gridlines, top to bottom: quarters of the top, then quarters of the
 * bottom below zero. Every label has as many decimals as the finest tick needs.
 */
export function chartTicks(range: ChartRange): ChartTick[] {
  const values: number[] = []
  if (range.top > 0) for (let k = 4; k >= 0; k--) values.push(clean((range.top * k) / 4))
  else values.push(0)
  if (range.bottom < 0) for (let k = 1; k <= 4; k++) values.push(clean((range.bottom * k) / 4))
  const digits = Math.max(...values.map(decimalsOf))
  return values.map((value) => ({
    value,
    label: value.toLocaleString('en-US', { maximumFractionDigits: digits }),
  }))
}
