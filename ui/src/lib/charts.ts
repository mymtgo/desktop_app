/**
 * The maths behind the deck dashboard's hand-drawn SVG charts (0.x drew them
 * with unovis): scales, ticks, path strings, and the performance chart's
 * bucketing, session dots and trend (0.x `MatchHistoryChart.vue`).
 */
import type { PeerChartView, PeerDayView } from '@/bindings'
import { customRangeDays, dayToUtcMs, utcMsToDay } from './timeframes'

export type Scale = (value: number) => number

/** A straight-line map from `domain` to `range`; a one-point domain maps to the middle. */
export function linearScale(domain: [number, number], range: [number, number]): Scale {
  const [d0, d1] = domain
  const [r0, r1] = range
  if (d0 === d1) {
    const middle = (r0 + r1) / 2
    return () => middle
  }
  return (value) => r0 + ((value - d0) / (d1 - d0)) * (r1 - r0)
}

/**
 * Axis ticks from 0 up to `max`: the smallest step of 1, 2 or 5 times a power
 * of ten that gives at most about `count` steps. `integer` keeps the step at
 * 1 or more, for counts.
 */
export function niceTicks(max: number, count: number, options: { integer?: boolean } = {}): number[] {
  if (max <= 0 || count <= 0) return [0]

  const rough = max / count
  const power = 10 ** Math.floor(Math.log10(rough))
  let step = power * 10
  for (const factor of [1, 2, 5, 10]) {
    if (power * factor >= rough) {
      step = power * factor
      break
    }
  }
  if (options.integer) step = Math.max(1, Math.round(step))

  const ticks: number[] = []
  for (let index = 0; index * step <= max + step * 1e-9; index += 1) {
    // Multiply rather than add, so 0.1 steps do not drift.
    ticks.push(Number((index * step).toPrecision(12)))
  }
  return ticks
}

function round2(value: number): number {
  return Math.round(value * 100) / 100
}

/** Half the width of the level segment that stands for a lone point. */
export const LONE_POINT_HALF_WIDTH = 4

/**
 * An SVG path through the points; a missing `y` leaves a gap. A point with
 * no neighbour to join (a single day of data) is a short level segment, as a
 * one-point path draws nothing.
 */
export function linePath(points: { x: number; y: number | null }[]): string {
  // Runs of points with values, split at the gaps.
  const runs: { x: number; y: number }[][] = []
  let run: { x: number; y: number }[] = []
  for (const point of points) {
    if (point.y === null) {
      if (run.length > 0) runs.push(run)
      run = []
      continue
    }
    run.push({ x: point.x, y: point.y })
  }
  if (run.length > 0) runs.push(run)

  let path = ''
  for (const line of runs) {
    if (line.length === 1) {
      const { x, y } = line[0]
      path += `M${round2(x - LONE_POINT_HALF_WIDTH)},${round2(y)}L${round2(x + LONE_POINT_HALF_WIDTH)},${round2(y)}`
      continue
    }
    line.forEach((point, index) => {
      path += `${index === 0 ? 'M' : 'L'}${round2(point.x)},${round2(point.y)}`
    })
  }
  return path
}

/**
 * Polyline points for the win rate sparkline (0.x `KpiMatchWinRate`): rates
 * (0 to 100) spread across a `width` by `height` box, flipped so a higher
 * rate sits higher. Null below two points: one point is not a line.
 */
export function sparklinePoints(series: (number | null)[], width = 100, height = 32): string | null {
  const rates = series.filter((rate): rate is number => rate !== null)
  if (rates.length < 2) return null

  const step = width / (rates.length - 1)
  return rates.map((rate, index) => `${(index * step).toFixed(2)},${(height - (rate / 100) * height).toFixed(2)}`).join(' ')
}

/**
 * Days per plotted point (0.x `bucketDays`). The short presets plot every
 * day, this year plots weeks, a custom range follows its length, and all
 * time follows how many days the data spans.
 */
export function chartBucketDays(timeframe: string, dayCount: number): number {
  if (['week', 'biweekly', 'monthly'].includes(timeframe)) return 1

  const customDays = customRangeDays(timeframe)
  if (customDays !== null) {
    if (customDays <= 60) return 1
    return customDays <= 400 ? 7 : 30
  }

  if (timeframe === 'year') return 7

  if (dayCount <= 60) return 1
  if (dayCount <= 400) return 7
  return 30
}

/** One of the player's decks in a day or bucket (the home chart's tooltip): grouped by archetype, else by deck. */
export type DeckRow = { key: string; name: string; wins: number; losses: number; draws: number }

export type ChartDay = {
  day: string
  wins: number
  losses: number
  draws: number
  trend: number | null
  /** The decks played that day; only the home dashboard sends it. */
  decks?: DeckRow[]
}

export type ChartPoint = {
  /** First and last day of the bucket (`YYYY-MM-DD`). */
  date: string
  endDate: string
  wins: number
  losses: number
  draws: number
  /** The bucket's own win rate (its session); null without matches. */
  rate: number | null
  /** The win rate trend after the bucket's last day; null before the first match. */
  trend: number | null
  /** The decks played in the bucket, summed by key, most matches first and then by name. */
  decks: DeckRow[]
}

/**
 * The decks of several days summed by key (a bucket of days), most matches
 * played first and ties by name. A group keeps the name of its latest day.
 */
export function sumDecks(days: { decks?: DeckRow[] }[]): DeckRow[] {
  const byKey = new Map<string, DeckRow>()
  for (const day of days) {
    for (const deck of day.decks ?? []) {
      const sum = byKey.get(deck.key)
      if (sum) {
        sum.name = deck.name
        sum.wins += deck.wins
        sum.losses += deck.losses
        sum.draws += deck.draws
      } else {
        byKey.set(deck.key, { ...deck })
      }
    }
  }
  return [...byKey.values()].sort((a, b) => pointMatches(b) - pointMatches(a) || a.name.localeCompare(b.name))
}

/** "3-1", draws only when there are any: "3-1-2". */
export function deckRecord(deck: { wins: number; losses: number; draws: number }): string {
  return deck.draws > 0 ? `${deck.wins}-${deck.losses}-${deck.draws}` : `${deck.wins}-${deck.losses}`
}

/** A deck row with its win rate, draws counted as matches played as everywhere else. */
export type DeckLine = DeckRow & { rate: number }

/** The most deck rows the tooltip lists before it sums the rest into "+N more". */
export const TOOLTIP_DECK_ROWS = 3

/**
 * The tooltip's deck rows: the first `limit` (the list is already ordered by
 * matches played) and, when there are more, the rest summed as one line that
 * says how many decks it holds.
 */
export function tooltipDecks(decks: DeckRow[], limit = TOOLTIP_DECK_ROWS): { rows: DeckLine[]; more: (DeckLine & { count: number }) | null } {
  const line = (deck: DeckRow): DeckLine => ({ ...deck, rate: Math.round((deck.wins / Math.max(1, pointMatches(deck))) * 100) })
  const rows = decks.slice(0, limit).map(line)
  const rest = decks.slice(limit)
  if (rest.length === 0) return { rows, more: null }

  const total = { key: 'more', name: `+${rest.length} more`, wins: 0, losses: 0, draws: 0 }
  for (const deck of rest) {
    total.wins += deck.wins
    total.losses += deck.losses
    total.draws += deck.draws
  }
  return { rows, more: { ...line(total), count: rest.length } }
}

/**
 * The chart's points: days summed into buckets of `bucketDays`, each with
 * its own win rate and the trend after its last day. Draws count as matches
 * played, as in every other win rate.
 */
export function chartPoints(days: ChartDay[], bucketDays: number): ChartPoint[] {
  const size = Math.max(1, bucketDays)
  const points: ChartPoint[] = []

  for (let index = 0; index < days.length; index += size) {
    const slice = days.slice(index, index + size)
    let wins = 0
    let losses = 0
    let draws = 0
    for (const day of slice) {
      wins += day.wins
      losses += day.losses
      draws += day.draws
    }

    const total = wins + losses + draws
    points.push({
      date: slice[0].day,
      endDate: slice[slice.length - 1].day,
      wins,
      losses,
      draws,
      rate: total > 0 ? Math.round((wins / total) * 100) : null,
      trend: slice[slice.length - 1].trend,
      decks: sumDecks(slice),
    })
  }
  return points
}

/** Matches in a bucket. */
export function pointMatches(point: { wins: number; losses: number; draws: number }): number {
  return point.wins + point.losses + point.draws
}

/** The smallest and largest session dot, in pixels. */
export const DOT_MIN_RADIUS = 3
export const DOT_MAX_RADIUS = 8

/**
 * A session dot's radius: 3px for one match, growing with the square root of
 * the matches so area follows the count, and capped at 8px so a 40-match day
 * does not swamp the chart. 0 for a bucket without matches (no dot).
 */
export function dotRadius(matches: number): number {
  if (matches <= 0) return 0
  return Math.min(DOT_MAX_RADIUS, DOT_MIN_RADIUS + Math.sqrt(matches - 1))
}

/**
 * A line through the points that have a value, joined across the ones that
 * do not: a bucket without matches leaves no gap and no zero.
 */
export function joinedPath(points: { x: number; y: number | null }[]): string {
  return linePath(points.filter((point) => point.y !== null))
}

/**
 * The sessions encoding both win rate charts share: each point's own rate as
 * a line joined across points without matches, and a dot per played point
 * sized by its matches.
 */
export function sessionLine(
  points: { x: number; rate: number | null; matches: number }[],
  y: Scale,
): { path: string; dots: { x: number; y: number; r: number }[] } {
  const path = joinedPath(points.map((point) => ({ x: point.x, y: point.rate === null ? null : y(point.rate) })))
  const dots = points.flatMap((point) =>
    point.rate === null ? [] : [{ x: point.x, y: y(point.rate), r: dotRadius(point.matches) }],
  )
  return { path, dots }
}

/** Day steps the x axis tries, smallest first. */
const TICK_STEPS = [1, 2, 3, 7, 14, 28, 56, 91, 182, 364, 728]

/**
 * X axis ticks between two days: every `step` days from the first, with the
 * smallest step that gives at most `maxTicks`. Counted on the calendar, so a
 * clock change does not shift a tick.
 */
export function dayTicks(first: string, last: string, maxTicks: number): string[] {
  const start = dayToUtcMs(first)
  const end = dayToUtcMs(last)
  if (start === null || end === null || end < start) return []

  const span = Math.round((end - start) / 86_400_000)
  const limit = Math.max(1, maxTicks)
  const step = TICK_STEPS.find((candidate) => Math.floor(span / candidate) + 1 <= limit) ?? TICK_STEPS[TICK_STEPS.length - 1]

  const ticks: string[] = []
  for (let offset = 0; offset <= span; offset += step) {
    ticks.push(utcMsToDay(start + offset * 86_400_000))
  }
  return ticks
}

/** Index of the value closest to `x`; -1 without values. */
export function nearestIndex(xs: number[], x: number): number {
  let best = -1
  let bestDistance = Infinity
  xs.forEach((value, index) => {
    const distance = Math.abs(value - x)
    if (distance < bestDistance) {
      best = index
      bestDistance = distance
    }
  })
  return best
}

function dayDate(day: string): Date {
  return new Date(dayToUtcMs(day) ?? 0)
}

/** An x axis tick: "1 Oct" (0.x `formatTick`, en-GB). */
export function dayTickLabel(day: string, locale = 'en-GB'): string {
  return dayDate(day).toLocaleDateString(locale, { day: 'numeric', month: 'short', timeZone: 'UTC' })
}

/** A tooltip's heading: the bucket's day, or both ends of a longer bucket (0.x `formatBucketLabel`). */
export function bucketLabel(point: { date: string; endDate: string }, locale = 'en-GB'): string {
  const options: Intl.DateTimeFormatOptions = { day: 'numeric', month: 'short', year: 'numeric', timeZone: 'UTC' }
  const start = dayDate(point.date).toLocaleDateString(locale, options)
  if (point.endDate === point.date) return start
  return `${start} \u2013 ${dayDate(point.endDate).toLocaleDateString(locale, options)}`
}

/** "3W - 1L", draws only when there are any (0.x `formatMatchRecord`). */
export function letterRecord(wins: number, losses: number, draws: number): string {
  return draws > 0 ? `${wins}W - ${losses}L - ${draws}D` : `${wins}W - ${losses}L`
}

/**
 * The other decks' trend on each bucket: the trend after their latest day on
 * or before the bucket's last day, so it holds across buckets without a peer
 * match. Null before their first match.
 */
export function peerTrend(points: ChartPoint[], days: PeerDayView[]): (number | null)[] {
  const ordered = [...days].sort((a, b) => a.date.localeCompare(b.date))
  let next = 0
  let latest: number | null = null
  return points.map((point) => {
    while (next < ordered.length && ordered[next].date <= point.endDate) {
      latest = ordered[next].trend ?? latest
      next += 1
    }
    return latest
  })
}

/** The peer line's legend (0.x `peerLabel`). */
export function peerLabel(peer: PeerChartView): string {
  const noun = peer.deckCount === 1 ? 'deck' : 'decks'
  return `Other ${peer.archetypeName} ${noun} (${peer.deckCount})`
}

/** Every `YYYY-MM-DD` from `first` to `last`, inclusive; empty when either is not a day or they run backwards. */
export function dayRange(first: string, last: string): string[] {
  const start = dayToUtcMs(first)
  const end = dayToUtcMs(last)
  if (start === null || end === null || end < start) return []

  const days: string[] = []
  for (let ms = start; ms <= end; ms += 86_400_000) {
    days.push(utcMsToDay(ms))
  }
  return days
}

/**
 * A line's days laid on every day of `allDays`: the line's own day on days it
 * has one, and on the others an empty day that keeps the last trend. The
 * trend is null before the line's first day; a day before the axis still sets
 * the trend the axis starts on.
 */
export function axisDays(days: PeerDayView[], allDays: string[]): ChartDay[] {
  const byDay = new Map<string, PeerDayView>()
  for (const day of days) byDay.set(day.date, day)

  let trend: number | null = null
  const first = allDays[0]
  for (const day of [...days].sort((a, b) => a.date.localeCompare(b.date))) {
    if (first !== undefined && day.date < first) trend = day.trend ?? trend
  }

  return allDays.map((date) => {
    const day = byDay.get(date)
    if (day) trend = day.trend ?? trend
    return { day: date, wins: day?.wins ?? 0, losses: day?.losses ?? 0, draws: day?.draws ?? 0, trend }
  })
}

/** The app's chart colours (`--chart-1` to `--chart-5` in `app.css`, which follow the theme). */
export const TREND_COLOURS = [
  'var(--color-chart-1)',
  'var(--color-chart-2)',
  'var(--color-chart-3)',
  'var(--color-chart-4)',
  'var(--color-chart-5)',
]

/** Dash patterns for the rounds after the first, so a repeated colour still differs by line style. */
const TREND_DASHES: (string | null)[] = [null, '7 4', '2 3', '10 3 2 3', '14 5']

/**
 * A deck's line style by its slot (its place among all the archetype's decks
 * in scope, so it does not change with the timeframe): the colours in order, and
 * on each further round of the colours the next dash pattern. Five colours
 * and five patterns tell 25 decks apart; past that the styles repeat.
 */
export function trendStyle(index: number): { colour: string; dash: string | null } {
  const slot = index % (TREND_COLOURS.length * TREND_DASHES.length)
  return {
    colour: TREND_COLOURS[slot % TREND_COLOURS.length],
    dash: TREND_DASHES[Math.floor(slot / TREND_COLOURS.length)],
  }
}
