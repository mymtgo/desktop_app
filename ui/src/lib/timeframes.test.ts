import { CalendarDate } from '@internationalized/date'
import { describe, expect, it } from 'vitest'
import {
  calendarDateToDay,
  customRange,
  customRangeDays,
  customRangeLabel,
  customTimeframe,
  dashboardTimeframeLabel,
  dayToCalendarDate,
  localDay,
  normalizeTimeframe,
  shiftDay,
  TIMEFRAME_OPTIONS,
  timeframeLabel,
} from './timeframes'

describe('TIMEFRAME_OPTIONS', () => {
  it('lists the presets as 0.x does', () => {
    expect(TIMEFRAME_OPTIONS).toEqual([
      { value: 'alltime', label: 'All time' },
      { value: 'year', label: 'This year' },
      { value: 'monthly', label: '30 days' },
      { value: 'biweekly', label: '2 weeks' },
      { value: 'week', label: '7 days' },
    ])
  })
})

describe('customRange', () => {
  it('reads both days of a range', () => {
    expect(customRange('2026-09-01..2026-09-15')).toEqual({ start: '2026-09-01', end: '2026-09-15' })
  })

  it('is null for a preset, a malformed range or a day that does not exist', () => {
    expect(customRange('monthly')).toBeNull()
    expect(customRange('2026-09-01..2026-9-15')).toBeNull()
    expect(customRange('2026-02-30..2026-03-02')).toBeNull()
    expect(customRange('2026-13-01..2026-13-02')).toBeNull()
  })
})

describe('customTimeframe', () => {
  it('puts the earlier day first', () => {
    expect(customTimeframe('2026-09-01', '2026-09-15')).toBe('2026-09-01..2026-09-15')
    expect(customTimeframe('2026-09-15', '2026-09-01')).toBe('2026-09-01..2026-09-15')
  })
})

describe('customRangeDays', () => {
  it('counts both ends', () => {
    expect(customRangeDays('2026-09-01..2026-09-15')).toBe(15)
    expect(customRangeDays('2026-09-01..2026-09-01')).toBe(1)
    expect(customRangeDays('2026-03-28..2026-03-30')).toBe(3)
  })

  it('is null for a preset', () => {
    expect(customRangeDays('alltime')).toBeNull()
  })
})

describe('customRangeLabel', () => {
  it('names both days, the year once when it is the same', () => {
    expect(customRangeLabel('2026-03-01..2026-03-15', 'en-GB')).toBe('1 Mar to 15 Mar 2026')
  })

  it('gives the start its year when the range crosses one', () => {
    expect(customRangeLabel('2025-12-20..2026-01-05', 'en-GB')).toBe('20 Dec 2025 to 5 Jan 2026')
  })

  it('names a single day once', () => {
    expect(customRangeLabel('2026-01-05..2026-01-05', 'en-GB')).toBe('5 Jan 2026')
  })

  it('is null for a preset', () => {
    expect(customRangeLabel('week', 'en-GB')).toBeNull()
  })
})

describe('timeframeLabel', () => {
  it('names presets and ranges, and falls back to all time', () => {
    expect(timeframeLabel('monthly', 'en-GB')).toBe('30 days')
    expect(timeframeLabel('2026-03-01..2026-03-15', 'en-GB')).toBe('1 Mar to 15 Mar 2026')
    expect(timeframeLabel('nonsense', 'en-GB')).toBe('All time')
  })
})

describe('dashboardTimeframeLabel', () => {
  it('uses the short dashboard names', () => {
    expect(dashboardTimeframeLabel('week', 'en-GB')).toBe('7d')
    expect(dashboardTimeframeLabel('biweekly', 'en-GB')).toBe('2w')
    expect(dashboardTimeframeLabel('monthly', 'en-GB')).toBe('30d')
    expect(dashboardTimeframeLabel('year', 'en-GB')).toBe('This year')
    expect(dashboardTimeframeLabel('alltime', 'en-GB')).toBe('All time')
    expect(dashboardTimeframeLabel('2026-03-01..2026-03-15', 'en-GB')).toBe('1 Mar to 15 Mar 2026')
    expect(dashboardTimeframeLabel('nonsense', 'en-GB')).toBe('All time')
  })
})

describe('normalizeTimeframe', () => {
  it('keeps presets and valid ranges', () => {
    expect(normalizeTimeframe('year')).toBe('year')
    expect(normalizeTimeframe('2026-09-01..2026-09-15')).toBe('2026-09-01..2026-09-15')
  })

  it('reads anything else as all time', () => {
    expect(normalizeTimeframe(undefined)).toBe('alltime')
    expect(normalizeTimeframe(null)).toBe('alltime')
    expect(normalizeTimeframe('forever')).toBe('alltime')
    expect(normalizeTimeframe(['week'])).toBe('alltime')
    expect(normalizeTimeframe('2026-02-30..2026-03-02')).toBe('alltime')
  })
})

describe('shiftDay', () => {
  it('moves a day on the calendar, across months and years', () => {
    expect(shiftDay('2026-10-07', -29)).toBe('2026-09-08')
    expect(shiftDay('2026-01-10', -29)).toBe('2025-12-12')
    expect(shiftDay('2026-02-28', 1)).toBe('2026-03-01')
  })
})

describe('localDay', () => {
  it('is the day on the local calendar', () => {
    expect(localDay(new Date(2026, 9, 7, 23, 59))).toBe('2026-10-07')
    expect(localDay(new Date(2026, 0, 3, 0, 0))).toBe('2026-01-03')
  })
})

describe('dayToCalendarDate', () => {
  it('is the calendar day the picker shows', () => {
    const date = dayToCalendarDate('2026-02-28')
    expect(date).toBeInstanceOf(CalendarDate)
    expect([date?.year, date?.month, date?.day]).toEqual([2026, 2, 28])
  })

  it('is undefined for an empty or impossible day', () => {
    expect(dayToCalendarDate('')).toBeUndefined()
    expect(dayToCalendarDate('2026-02-30')).toBeUndefined()
    expect(dayToCalendarDate('2026-9-1')).toBeUndefined()
  })
})

describe('calendarDateToDay', () => {
  it('is the `YYYY-MM-DD` day of a picked date', () => {
    expect(calendarDateToDay(new CalendarDate(2026, 9, 1))).toBe('2026-09-01')
    expect(calendarDateToDay(new CalendarDate(987, 12, 31))).toBe('0987-12-31')
  })

  it('is empty when nothing is picked', () => {
    expect(calendarDateToDay(undefined)).toBe('')
  })

  it('round trips with dayToCalendarDate into a custom range', () => {
    const from = calendarDateToDay(dayToCalendarDate('2026-09-15'))
    const to = calendarDateToDay(dayToCalendarDate('2026-09-01'))
    expect(customTimeframe(from, to)).toBe('2026-09-01..2026-09-15')
  })
})
