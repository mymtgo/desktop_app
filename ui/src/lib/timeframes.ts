/**
 * The deck pages' timeframes (0.x `lib/timeframes.ts`). A key travels in the
 * `timeframe` query parameter: a preset, or a custom range
 * `YYYY-MM-DD..YYYY-MM-DD` with both days included. The backend reads
 * anything else as all time.
 *
 * Days are plain `YYYY-MM-DD` strings here (0.x used `CalendarDate`); only
 * the calendar picker works in `CalendarDate`, through `dayToCalendarDate` and
 * `calendarDateToDay`.
 */

import { CalendarDate, type DateValue } from '@internationalized/date'

export type TimeframeKey = 'alltime' | 'year' | 'monthly' | 'biweekly' | 'week'

export const TIMEFRAME_OPTIONS: { value: TimeframeKey; label: string }[] = [
  { value: 'alltime', label: 'All time' },
  { value: 'year', label: 'This year' },
  { value: 'monthly', label: '30 days' },
  { value: 'biweekly', label: '2 weeks' },
  { value: 'week', label: '7 days' },
]

/** The dashboard's shorter names (0.x `DeckDashboard` `TIMEFRAME_LABELS`). */
const DASHBOARD_LABELS: Record<string, string> = {
  week: '7d',
  biweekly: '2w',
  monthly: '30d',
  year: 'This year',
  alltime: 'All time',
}

const CUSTOM_RANGE = /^(\d{4}-\d{2}-\d{2})\.\.(\d{4}-\d{2}-\d{2})$/
const DAY_MS = 86_400_000

/** UTC milliseconds of a `YYYY-MM-DD` day, or null when the day does not exist. */
export function dayToUtcMs(day: string): number | null {
  const [year, month, date] = day.split('-').map(Number)
  const ms = Date.UTC(year, month - 1, date)
  const check = new Date(ms)
  if (check.getUTCFullYear() !== year || check.getUTCMonth() !== month - 1 || check.getUTCDate() !== date) {
    return null
  }
  return ms
}

/** The `YYYY-MM-DD` day of UTC milliseconds. */
export function utcMsToDay(ms: number): string {
  return new Date(ms).toISOString().slice(0, 10)
}

/** Both days of a custom range; null for a preset or a range that does not parse. */
export function customRange(timeframe: string): { start: string; end: string } | null {
  const match = CUSTOM_RANGE.exec(timeframe)
  if (!match) return null
  if (dayToUtcMs(match[1]) === null || dayToUtcMs(match[2]) === null) return null
  return { start: match[1], end: match[2] }
}

/** The key of a custom range, earlier day first. */
export function customTimeframe(start: string, end: string): string {
  const [from, to] = start <= end ? [start, end] : [end, start]
  return `${from}..${to}`
}

/** Days a custom range covers, both ends included; null for a preset. */
export function customRangeDays(timeframe: string): number | null {
  const range = customRange(timeframe)
  if (!range) return null
  const start = dayToUtcMs(range.start) ?? 0
  const end = dayToUtcMs(range.end) ?? 0
  return Math.round(Math.abs(end - start) / DAY_MS) + 1
}

/** "1 Sep to 15 Sep 2026", with the year on the start too when the range crosses one. */
export function customRangeLabel(timeframe: string, locale?: string): string | null {
  const range = customRange(timeframe)
  if (!range) return null

  const dayFormat = new Intl.DateTimeFormat(locale, { day: 'numeric', month: 'short', timeZone: 'UTC' })
  const dayYearFormat = new Intl.DateTimeFormat(locale, {
    day: 'numeric',
    month: 'short',
    year: 'numeric',
    timeZone: 'UTC',
  })
  const start = new Date(dayToUtcMs(range.start) ?? 0)
  const end = new Date(dayToUtcMs(range.end) ?? 0)
  const sameYear = range.start.slice(0, 4) === range.end.slice(0, 4)
  const startText = sameYear ? dayFormat.format(start) : dayYearFormat.format(start)

  return range.start === range.end ? dayYearFormat.format(end) : `${startText} to ${dayYearFormat.format(end)}`
}

export function timeframeLabel(key: string, locale?: string): string {
  return customRangeLabel(key, locale) ?? TIMEFRAME_OPTIONS.find((o) => o.value === key)?.label ?? 'All time'
}

/** The KPI cards' scope note: short preset names, or the custom range. */
export function dashboardTimeframeLabel(key: string, locale?: string): string {
  return DASHBOARD_LABELS[key] ?? customRangeLabel(key, locale) ?? 'All time'
}

/** The timeframe a query parameter asks for; anything unknown is all time. */
export function normalizeTimeframe(value: unknown): string {
  if (typeof value !== 'string') return 'alltime'
  if (TIMEFRAME_OPTIONS.some((option) => option.value === value)) return value
  return customRange(value) ? value : 'alltime'
}

/** The day `days` after (or before, when negative) a `YYYY-MM-DD` day. */
export function shiftDay(day: string, days: number): string {
  return utcMsToDay((dayToUtcMs(day) ?? 0) + days * DAY_MS)
}

/** The `YYYY-MM-DD` day of a moment on the local calendar. */
export function localDay(date: Date): string {
  const month = String(date.getMonth() + 1).padStart(2, '0')
  const day = String(date.getDate()).padStart(2, '0')
  return `${date.getFullYear()}-${month}-${day}`
}

/** The picker's date for a `YYYY-MM-DD` day; undefined when the day is empty or does not exist. */
export function dayToCalendarDate(day: string): CalendarDate | undefined {
  if (!/^\d{4}-\d{2}-\d{2}$/.test(day) || dayToUtcMs(day) === null) return undefined
  const [year, month, date] = day.split('-').map(Number)
  return new CalendarDate(year, month, date)
}

/** The `YYYY-MM-DD` day the picker shows; empty when nothing is picked. */
export function calendarDateToDay(date: DateValue | undefined): string {
  if (!date) return ''
  const year = String(date.year).padStart(4, '0')
  const month = String(date.month).padStart(2, '0')
  const day = String(date.day).padStart(2, '0')
  return `${year}-${month}-${day}`
}
