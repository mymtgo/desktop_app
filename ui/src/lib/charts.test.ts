import { describe, expect, it } from 'vitest'
import {
  chartBucketDays,
  bucketLabel,
  chartPoints,
  axisDays,
  dayRange,
  dayTickLabel,
  dayTicks,
  deckRecord,
  DOT_MAX_RADIUS,
  DOT_MIN_RADIUS,
  dotRadius,
  joinedPath,
  linearScale,
  letterRecord,
  LONE_POINT_HALF_WIDTH,
  linePath,
  nearestIndex,
  niceTicks,
  peerLabel,
  peerTrend,
  pointMatches,
  sparklinePoints,
  sessionLine,
  sumDecks,
  tooltipDecks,
  TOOLTIP_DECK_ROWS,
  trendStyle,
  TREND_COLOURS,
} from './charts'

function day(date: string, wins: number, losses: number, draws = 0, trend: number | null = null) {
  return { day: date, wins, losses, draws, trend }
}

describe('linearScale', () => {
  it('maps the domain onto the range', () => {
    const scale = linearScale([0, 10], [0, 200])
    expect(scale(0)).toBe(0)
    expect(scale(5)).toBe(100)
    expect(scale(10)).toBe(200)
  })

  it('can flip the range, so a higher value sits higher on screen', () => {
    const scale = linearScale([0, 100], [300, 0])
    expect(scale(0)).toBe(300)
    expect(scale(100)).toBe(0)
    expect(scale(25)).toBe(225)
  })

  it('puts every value in the middle when the domain is a single point', () => {
    const scale = linearScale([7, 7], [0, 100])
    expect(scale(7)).toBe(50)
  })
})

describe('niceTicks', () => {
  it('steps a percentage axis by 20', () => {
    expect(niceTicks(100, 5)).toEqual([0, 20, 40, 60, 80, 100])
  })

  it('uses steps of 1, 2 or 5 times a power of ten that stay within the maximum', () => {
    expect(niceTicks(7, 5)).toEqual([0, 2, 4, 6])
    expect(niceTicks(23, 5)).toEqual([0, 5, 10, 15, 20])
    expect(niceTicks(1000, 4)).toEqual([0, 500, 1000])
  })

  it('never splits a count below whole numbers', () => {
    expect(niceTicks(1, 5, { integer: true })).toEqual([0, 1])
    expect(niceTicks(3, 5, { integer: true })).toEqual([0, 1, 2, 3])
  })

  it('gives a single zero tick for an empty axis', () => {
    expect(niceTicks(0, 5)).toEqual([0])
  })
})

describe('linePath', () => {
  it('moves to the first point and draws to the rest', () => {
    expect(
      linePath([
        { x: 0, y: 10 },
        { x: 5, y: 6 },
        { x: 10.123, y: 0 },
      ]),
    ).toBe('M0,10L5,6L10.12,0')
  })

  it('leaves a gap where a value is missing', () => {
    expect(
      linePath([
        { x: 0, y: null },
        { x: 1, y: 4 },
        { x: 2, y: 5 },
        { x: 3, y: null },
        { x: 4, y: 6 },
      ]),
    ).toBe('M1,4L2,5M0,6L8,6')
  })

  it('draws a lone point as a short level segment, so a single day shows', () => {
    expect(linePath([{ x: 150, y: 40 }])).toBe(`M${150 - LONE_POINT_HALF_WIDTH},40L${150 + LONE_POINT_HALF_WIDTH},40`)
    expect(linePath([{ x: 10.123, y: 5 }])).toBe('M6.12,5L14.12,5')
  })

  it('draws nothing without a value', () => {
    expect(linePath([])).toBe('')
    expect(linePath([{ x: 1, y: null }])).toBe('')
  })

  it('is empty without points', () => {
    expect(linePath([])).toBe('')
    expect(linePath([{ x: 1, y: null }])).toBe('')
  })
})

describe('sparklinePoints', () => {
  it('spreads the rates across a 100 by 32 box, higher rates higher', () => {
    expect(sparklinePoints([0, 50, 100])).toBe('0.00,32.00 50.00,16.00 100.00,0.00')
  })

  it('keeps unrounded rates to two decimals', () => {
    expect(sparklinePoints([100 / 3, 75])).toBe('0.00,21.33 100.00,8.00')
  })

  it('needs two points to draw a line', () => {
    expect(sparklinePoints([])).toBeNull()
    expect(sparklinePoints([60])).toBeNull()
  })

  it('skips missing values', () => {
    expect(sparklinePoints([null, 50, null, 100])).toBe('0.00,16.00 100.00,0.00')
  })
})

describe('chartBucketDays', () => {
  it('plots every day for the short presets', () => {
    expect(chartBucketDays('week', 900)).toBe(1)
    expect(chartBucketDays('biweekly', 900)).toBe(1)
    expect(chartBucketDays('monthly', 900)).toBe(1)
  })

  it('plots weeks for this year', () => {
    expect(chartBucketDays('year', 10)).toBe(7)
  })

  it('buckets a custom range by its length', () => {
    expect(chartBucketDays('2026-01-01..2026-03-01', 3)).toBe(1)
    expect(chartBucketDays('2026-01-01..2026-03-02', 3)).toBe(7)
    expect(chartBucketDays('2025-01-01..2026-02-04', 3)).toBe(7)
    expect(chartBucketDays('2025-01-01..2026-02-05', 3)).toBe(30)
  })

  it('buckets all time by how many days the data spans', () => {
    expect(chartBucketDays('alltime', 60)).toBe(1)
    expect(chartBucketDays('alltime', 61)).toBe(7)
    expect(chartBucketDays('alltime', 400)).toBe(7)
    expect(chartBucketDays('alltime', 401)).toBe(30)
  })
})

describe('chartPoints', () => {
  it('keeps one point per day with its own rate and the day trend', () => {
    const points = chartPoints(
      [day('2026-10-01', 2, 1, 0, 67), day('2026-10-02', 0, 0, 0, 67), day('2026-10-03', 0, 1, 1, 48)],
      1,
    )
    expect(points).toEqual([
      { date: '2026-10-01', endDate: '2026-10-01', wins: 2, losses: 1, draws: 0, rate: 67, trend: 67, decks: [] },
      { date: '2026-10-02', endDate: '2026-10-02', wins: 0, losses: 0, draws: 0, rate: null, trend: 67, decks: [] },
      { date: '2026-10-03', endDate: '2026-10-03', wins: 0, losses: 1, draws: 1, rate: 0, trend: 48, decks: [] },
    ])
  })

  it('sums each bucket of days and takes the trend of its last day', () => {
    const days = [
      day('2026-10-01', 1, 0, 0, 100),
      day('2026-10-02', 1, 1, 0, 60),
      day('2026-10-03', 0, 1, 0, 50),
      day('2026-10-04', 3, 0, 0, 70),
      day('2026-10-05', 0, 0, 0, 70),
    ]
    const points = chartPoints(days, 2)
    expect(points.map((p) => [p.date, p.endDate, p.wins, p.losses, p.rate, p.trend])).toEqual([
      ['2026-10-01', '2026-10-02', 2, 1, 67, 60],
      ['2026-10-03', '2026-10-04', 3, 1, 75, 70],
      ['2026-10-05', '2026-10-05', 0, 0, null, 70],
    ])
  })

  it('has no trend before the first match', () => {
    const points = chartPoints([day('2026-10-01', 0, 0), day('2026-10-02', 1, 0, 0, 100)], 1)
    expect(points.map((p) => p.trend)).toEqual([null, 100])
  })

  it('is empty without days', () => {
    expect(chartPoints([], 1)).toEqual([])
  })

  it('sums the decks of a bucket by key', () => {
    const burn = { key: 'archetype:1', name: 'Burn', wins: 1, losses: 0, draws: 0 }
    const days = [
      { ...day('2026-10-01', 1, 0, 0, 100), decks: [burn] },
      { ...day('2026-10-02', 0, 1, 0, 50), decks: [{ ...burn, wins: 0, losses: 1 }] },
      { ...day('2026-10-03', 1, 0, 0, 60), decks: [burn] },
    ]
    const points = chartPoints(days, 2)
    expect(points.map((p) => p.decks)).toEqual([
      [{ key: 'archetype:1', name: 'Burn', wins: 1, losses: 1, draws: 0 }],
      [burn],
    ])
    expect(chartPoints([day('2026-10-01', 1, 0, 0, 100)], 1)[0].decks).toEqual([])
  })

  it('counts the matches of a bucket, draws included', () => {
    expect(pointMatches({ wins: 2, losses: 1, draws: 3 })).toBe(6)
  })
})

describe('session dots and lines', () => {
  it('sizes a dot by the matches, from 3px for one, capped at 8px', () => {
    expect(dotRadius(0)).toBe(0)
    expect(dotRadius(1)).toBe(DOT_MIN_RADIUS)
    expect(dotRadius(2)).toBe(4)
    expect(dotRadius(5)).toBe(5)
    expect(dotRadius(10)).toBe(6)
    expect(dotRadius(26)).toBe(DOT_MAX_RADIUS)
    expect(dotRadius(40)).toBe(DOT_MAX_RADIUS)
    expect(dotRadius(400)).toBe(DOT_MAX_RADIUS)
  })

  it('grows with the matches', () => {
    const radii = [1, 2, 3, 5, 9, 16].map(dotRadius)
    expect([...radii].sort((a, b) => a - b)).toEqual(radii)
  })

  it('joins buckets with a value across the ones without', () => {
    expect(
      joinedPath([
        { x: 0, y: 10 },
        { x: 10, y: null },
        { x: 20, y: null },
        { x: 30, y: 30 },
      ]),
    ).toBe('M0,10L30,30')
  })

  it('draws a lone session as a short level segment and none as nothing', () => {
    expect(joinedPath([{ x: 5, y: null }, { x: 10, y: 20 }])).toBe('M6,20L14,20')
    expect(joinedPath([{ x: 5, y: null }])).toBe('')
  })
})

describe('sessionLine', () => {
  const y = linearScale([0, 100], [100, 0])

  it('joins the played points and sizes a dot by the matches', () => {
    const line = sessionLine(
      [
        { x: 0, rate: 50, matches: 1 },
        { x: 10, rate: null, matches: 0 },
        { x: 20, rate: 100, matches: 5 },
      ],
      y,
    )
    expect(line.path).toBe('M0,50L20,0')
    expect(line.dots).toEqual([
      { x: 0, y: 50, r: dotRadius(1) },
      { x: 20, y: 0, r: dotRadius(5) },
    ])
  })

  it('has nothing without a played point', () => {
    expect(sessionLine([{ x: 0, rate: null, matches: 0 }], y)).toEqual({ path: '', dots: [] })
  })
})

describe('dayTicks', () => {
  it('ticks every day when they fit', () => {
    expect(dayTicks('2026-10-01', '2026-10-04', 6)).toEqual(['2026-10-01', '2026-10-02', '2026-10-03', '2026-10-04'])
  })

  it('widens the step until the ticks fit', () => {
    expect(dayTicks('2026-10-01', '2026-10-10', 5)).toEqual(['2026-10-01', '2026-10-03', '2026-10-05', '2026-10-07', '2026-10-09'])
    expect(dayTicks('2026-01-01', '2026-02-28', 6)).toEqual([
      '2026-01-01',
      '2026-01-15',
      '2026-01-29',
      '2026-02-12',
      '2026-02-26',
    ])
  })

  it('crosses months and the clock change on the calendar', () => {
    expect(dayTicks('2026-10-24', '2026-10-27', 4)).toEqual(['2026-10-24', '2026-10-25', '2026-10-26', '2026-10-27'])
    expect(dayTicks('2026-02-27', '2026-03-02', 4)).toEqual(['2026-02-27', '2026-02-28', '2026-03-01', '2026-03-02'])
  })

  it('gives the one day of a single-day chart', () => {
    expect(dayTicks('2026-10-01', '2026-10-01', 5)).toEqual(['2026-10-01'])
  })
})

describe('nearestIndex', () => {
  it('finds the point closest to the pointer', () => {
    expect(nearestIndex([0, 10, 20, 30], 14)).toBe(1)
    expect(nearestIndex([0, 10, 20, 30], 16)).toBe(2)
    expect(nearestIndex([0, 10, 20, 30], -5)).toBe(0)
    expect(nearestIndex([0, 10, 20, 30], 99)).toBe(3)
  })

  it('is -1 without points', () => {
    expect(nearestIndex([], 3)).toBe(-1)
  })
})

describe('chart labels', () => {
  it('names an axis tick by day and month', () => {
    expect(dayTickLabel('2026-10-01', 'en-GB')).toBe('1 Oct')
  })

  it('names a one-day bucket by its day and a longer one by both ends', () => {
    expect(bucketLabel({ date: '2026-10-01', endDate: '2026-10-01' }, 'en-GB')).toBe('1 Oct 2026')
    expect(bucketLabel({ date: '2026-10-01', endDate: '2026-10-07' }, 'en-GB')).toBe('1 Oct 2026 – 7 Oct 2026')
  })

  it('writes a record as letters, draws only when there are any', () => {
    expect(letterRecord(3, 1, 0)).toBe('3W - 1L')
    expect(letterRecord(3, 1, 2)).toBe('3W - 1L - 2D')
  })
})

describe('the peer archetype trend line', () => {
  function peerDay(date: string, trend: number | null) {
    return { date, wins: 1, losses: 0, draws: 0, trend }
  }

  it('is the peers trend after their latest day in each bucket', () => {
    const points = chartPoints(
      [day('2026-03-01', 1, 0), day('2026-03-02', 0, 0), day('2026-03-03', 0, 1), day('2026-03-04', 1, 0)],
      2,
    )
    const peer = [peerDay('2026-02-20', 90), peerDay('2026-03-02', 50), peerDay('2026-03-04', 25)]
    expect(peerTrend(points, peer)).toEqual([50, 25])
  })

  it('has no value before the first peer day and keeps the last one after', () => {
    const points = chartPoints([day('2026-03-01', 1, 0), day('2026-03-02', 1, 0), day('2026-03-03', 1, 0)], 1)
    expect(peerTrend(points, [peerDay('2026-03-02', 80)])).toEqual([null, 80, 80])
  })

  it('takes the trend of a peer day before the chart starts', () => {
    const points = chartPoints([day('2026-03-05', 1, 0)], 1)
    expect(peerTrend(points, [peerDay('2026-03-01', 42)])).toEqual([42])
  })

  it('names the other decks', () => {
    expect(peerLabel({ archetypeName: 'Qtron', deckCount: 1, days: [] })).toBe('Other Qtron deck (1)')
    expect(peerLabel({ archetypeName: 'Qtron', deckCount: 3, days: [] })).toBe('Other Qtron decks (3)')
  })
})

function pday(date: string, wins: number, losses: number, draws = 0, trend: number | null = null) {
  return { date, wins, losses, draws, trend }
}

describe('the archetype trend lines', () => {
  it('lists every day between two days, inclusive', () => {
    expect(dayRange('2026-03-30', '2026-04-02')).toEqual(['2026-03-30', '2026-03-31', '2026-04-01', '2026-04-02'])
    expect(dayRange('2026-03-01', '2026-03-01')).toEqual(['2026-03-01'])
    expect(dayRange('2026-03-02', '2026-03-01')).toEqual([])
    expect(dayRange('nope', '2026-03-01')).toEqual([])
    expect(dayRange('2026-12-31', '2027-01-01')).toEqual(['2026-12-31', '2027-01-01'])
    expect(dayRange('2028-02-28', '2028-03-01')).toEqual(['2028-02-28', '2028-02-29', '2028-03-01'])
  })

  it('lays a line on the axis, empty days carrying the last trend and none before the first match', () => {
    const all = dayRange('2026-03-01', '2026-03-05')
    const days = axisDays([pday('2026-03-02', 1, 0, 0, 100), pday('2026-03-04', 0, 1, 0, 60)], all)
    expect(days.map((d) => [d.day, d.wins, d.losses, d.trend])).toEqual([
      ['2026-03-01', 0, 0, null],
      ['2026-03-02', 1, 0, 100],
      ['2026-03-03', 0, 0, 100],
      ['2026-03-04', 0, 1, 60],
      ['2026-03-05', 0, 0, 60],
    ])
  })

  it('keeps the trend of a day before the axis, and draws', () => {
    const all = dayRange('2026-03-02', '2026-03-03')
    const days = axisDays([pday('2026-03-01', 5, 0, 0, 90), pday('2026-03-03', 1, 0, 2, 70)], all)
    expect(days.map((d) => d.trend)).toEqual([90, 70])
    expect(days[1].draws).toBe(2)
    expect(days[0].wins).toBe(0)
  })

  it('feeds the session line from a laid out axis, a rate only on days played', () => {
    const all = dayRange('2026-03-01', '2026-03-03')
    const points = chartPoints(axisDays([pday('2026-03-02', 3, 1, 0, 75)], all), 1)
    expect(points.map((p) => p.rate)).toEqual([null, 75, null])
    expect(points.map((p) => p.trend)).toEqual([null, 75, 75])
  })

  it('gives a deck a colour from the chart palette, and a dash pattern once the colours repeat', () => {
    expect(trendStyle(0)).toEqual({ colour: TREND_COLOURS[0], dash: null })
    expect(trendStyle(4).colour).toBe(TREND_COLOURS[4])
    expect(trendStyle(5).colour).toBe(TREND_COLOURS[0])
    expect(trendStyle(5).dash).not.toBeNull()
    const seen = new Set(Array.from({ length: 25 }, (_, i) => JSON.stringify(trendStyle(i))))
    expect(seen.size).toBe(25)
  })
})

describe('sumDecks', () => {
  it('orders by matches played, then by name', () => {
    const deck = (key: string, name: string, wins: number, losses = 0, draws = 0) => ({ key, name, wins, losses, draws })
    const summed = sumDecks([
      { decks: [deck('a', 'Zoo', 1), deck('b', 'Burn', 1), deck('c', 'Tron', 2, 1)] },
      { decks: [deck('a', 'Zoo', 0, 0, 1)] },
    ])
    expect(summed.map((d) => [d.name, pointMatches(d)])).toEqual([
      ['Tron', 3],
      ['Zoo', 2],
      ['Burn', 1],
    ])
  })

  it('keeps the name of the latest day and sums every count', () => {
    const summed = sumDecks([
      { decks: [{ key: 'deck:4', name: 'Old', wins: 1, losses: 0, draws: 0 }] },
      { decks: [{ key: 'deck:4', name: 'New', wins: 0, losses: 1, draws: 1 }] },
    ])
    expect(summed).toEqual([{ key: 'deck:4', name: 'New', wins: 1, losses: 1, draws: 1 }])
  })

  it('does not change the days it is given', () => {
    const days = [{ decks: [{ key: 'a', name: 'A', wins: 1, losses: 0, draws: 0 }] }, { decks: [{ key: 'a', name: 'A', wins: 1, losses: 0, draws: 0 }] }]
    sumDecks(days)
    expect(days[0].decks[0].wins).toBe(1)
  })

  it('is empty for days without decks', () => {
    expect(sumDecks([{}, { decks: [] }])).toEqual([])
  })
})

describe('tooltipDecks', () => {
  const deck = (name: string, wins: number, losses: number, draws = 0) => ({ key: name, name, wins, losses, draws })

  it('lists every deck when there are few, with win rates', () => {
    const shown = tooltipDecks([deck('Burn', 3, 1), deck('Tron', 0, 2)])
    expect(shown.more).toBeNull()
    expect(shown.rows.map((r) => [r.name, r.rate])).toEqual([
      ['Burn', 75],
      ['Tron', 0],
    ])
  })

  it('counts draws as matches played in a win rate', () => {
    expect(tooltipDecks([deck('Burn', 1, 0, 1)]).rows[0].rate).toBe(50)
  })

  it(`keeps ${TOOLTIP_DECK_ROWS} rows and sums the rest into one line`, () => {
    const shown = tooltipDecks([deck('A', 4, 0), deck('B', 3, 0), deck('C', 2, 0), deck('D', 1, 1), deck('E', 0, 1, 1)])
    expect(shown.rows.map((r) => r.name)).toEqual(['A', 'B', 'C'])
    expect(shown.more).toMatchObject({ name: '+2 more', count: 2, wins: 1, losses: 2, draws: 1, rate: 25 })
  })

  it('has no rows without decks', () => {
    expect(tooltipDecks([])).toEqual({ rows: [], more: null })
  })
})

describe('deckRecord', () => {
  it('is wins and losses, with draws only when there are any', () => {
    expect(deckRecord({ wins: 3, losses: 1, draws: 0 })).toBe('3-1')
    expect(deckRecord({ wins: 3, losses: 1, draws: 2 })).toBe('3-1-2')
  })
})
