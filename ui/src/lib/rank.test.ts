import { describe, expect, it } from 'vitest'
import type { RankLineView } from '@/bindings'
import { climbSentence, RANK_CHART_MARGIN, RANK_ICON_SIZE, RANK_POINT_INSET, rankChartModel } from './rank'

const WIDTH = 400
const HEIGHT = 180

function line(fields: Partial<RankLineView> = {}): RankLineView {
  return {
    ladder: 'constructed',
    season: 1,
    seasonStartMs: null,
    seasonEndMs: null,
    current: 'Gold 4',
    points: [
      { atMs: 1_000, steps: 30, label: 'Silver 3', mythic: false },
      { atMs: 2_000, steps: 36, label: 'Silver 2', mythic: false },
      { atMs: 3_000, steps: 36, label: 'Silver 2', mythic: false },
      { atMs: 4_000, steps: 50, label: 'Gold 4', mythic: false },
    ],
    gaps: [
      { from: 0, to: 1, kind: 'attributed', changeSteps: 6, ofDeck: true, bestOf: null },
      { from: 1, to: 2, kind: 'quiet', changeSteps: 0, ofDeck: false, bestOf: null },
      { from: 2, to: 3, kind: 'played_elsewhere', changeSteps: 14, ofDeck: false, bestOf: null },
    ],
    matches: [],
    ...fields,
  }
}

describe('rankChartModel', () => {
  it('one segment per gap with its kind', () => {
    const model = rankChartModel(line(), WIDTH, HEIGHT)
    expect(model.segments.map((segment) => segment.kind)).toEqual(['attributed', 'quiet', 'played_elsewhere'])
    for (const segment of model.segments) expect(segment.d).toMatch(/^M[\d.]+,[\d.]+L[\d.]+,[\d.]+$/)
  })

  it('keeps points outside the season window inside the plot', () => {
    // Season times are read as UTC and may be hours off the readings.
    const model = rankChartModel(line({ seasonStartMs: 1_500, seasonEndMs: 3_500 }), WIDTH, HEIGHT)
    const xs = model.points.map((point) => point.x)
    const first = RANK_CHART_MARGIN.left + RANK_POINT_INSET
    expect(xs[0]).toBe(first)
    expect(xs[3]).toBe(WIDTH - RANK_CHART_MARGIN.right - RANK_POINT_INSET)
    expect(model.icons[0].x).toBe(first)
    expect(model.segments[0].d.startsWith(`M${first},`)).toBe(true)
  })

  it("keeps the first reading's icon clear of the class labels", () => {
    // The labels end left of the plot; an icon is centred on its point.
    const model = rankChartModel(line({ seasonStartMs: 1_000 }), WIDTH, HEIGHT)
    expect(model.icons[0].x - RANK_ICON_SIZE / 2).toBeGreaterThan(RANK_CHART_MARGIN.left)
  })

  it("a deck's attributed gap is marked ofDeck", () => {
    const model = rankChartModel(line(), WIDTH, HEIGHT)
    expect(model.segments.map((segment) => segment.ofDeck)).toEqual([true, false, false])
  })

  it('places points on the season time axis when it is known', () => {
    const model = rankChartModel(line({ seasonStartMs: 0, seasonEndMs: 8_000 }), WIDTH, HEIGHT)
    const xs = model.points.map((point) => point.x)
    // On a time axis: 1000 to 4000 is three times 1000 to 2000.
    expect((xs[3] - xs[0]) / (xs[1] - xs[0])).toBeCloseTo(3)
    // Without season times the points span the plot; with them, they sit inside it.
    const own = rankChartModel(line(), WIDTH, HEIGHT).points.map((point) => point.x)
    expect(xs[0]).toBeGreaterThan(own[0])
    expect(xs[3]).toBeLessThan(own[3])
  })

  const bronzeOnly = () =>
    line({
      points: [
        { atMs: 1_000, steps: 0, label: 'Bronze 4', mythic: false },
        { atMs: 2_000, steps: 120, label: 'Mythic 95%', mythic: true },
      ],
      gaps: [],
    })

  it('the constructed ladder has a Spark row below Bronze', () => {
    const model = rankChartModel(bronzeOnly(), WIDTH, HEIGHT)
    const bottom = HEIGHT - RANK_CHART_MARGIN.bottom
    expect(model.bands.map((band) => band.label)).toEqual(['Mythic', 'Diamond', 'Platinum', 'Gold', 'Silver', 'Bronze', 'Spark'])
    const spark = model.bands[6]
    expect(spark.y).toBeCloseTo(bottom)
    expect(model.bands[5].y).toBeLessThan(spark.y)
    expect(model.points[0].y).toBeCloseTo(model.bands[5].y)
  })

  it('a Spark reading sits in the Spark row with the Spark icon', () => {
    const model = rankChartModel(
      line({ points: [{ atMs: 1_000, steps: -20, label: 'Spark 4', mythic: false }], gaps: [] }),
      WIDTH,
      HEIGHT,
    )
    expect(model.points[0].y).toBeCloseTo(HEIGHT - RANK_CHART_MARGIN.bottom)
    expect(model.icons[0].url).toContain('constructed_spark_t4')
  })

  it('bands are always the six classes on the limited ladder', () => {
    const model = rankChartModel(
      line({ ladder: 'limited', points: [{ atMs: 1_000, steps: 3, label: 'Bronze 4', mythic: false }], gaps: [] }),
      WIDTH,
      HEIGHT,
    )
    expect(model.bands.map((band) => band.label)).toEqual(['Mythic', 'Diamond', 'Platinum', 'Gold', 'Silver', 'Bronze'])
    for (let index = 1; index < model.bands.length; index += 1) {
      expect(model.bands[index].y).toBeGreaterThan(model.bands[index - 1].y)
      expect(model.bands[index].labelY).toBeGreaterThan(model.bands[index - 1].labelY)
    }
    // Each label sits above its own gridline (its row runs up to the next class start).
    for (const band of model.bands) expect(band.labelY).toBeLessThan(band.y)
  })

  it('the limited y axis spans bronze to mythic whatever the readings', () => {
    const model = rankChartModel({ ...bronzeOnly(), ladder: 'limited' }, WIDTH, HEIGHT)
    const bottom = HEIGHT - RANK_CHART_MARGIN.bottom
    const bronze = model.bands[5]
    const mythic = model.bands[0]
    expect(model.points[0].y).toBeCloseTo(bottom)
    expect(bronze.y).toBeCloseTo(bottom)
    expect(model.points[1].y).toBeCloseTo(mythic.y)
    expect(mythic.y).toBeGreaterThan(RANK_CHART_MARGIN.top)
  })

  it('a single point draws no segment', () => {
    const model = rankChartModel(
      line({ points: [{ atMs: 1_000, steps: 30, label: 'Silver 3', mythic: false }], gaps: [] }),
      WIDTH,
      HEIGHT,
    )
    expect(model.segments).toEqual([])
    expect(model.points).toHaveLength(1)
  })
})

describe('climbSentence', () => {
  it('climbSentence covers climbed, dropped and none', () => {
    expect(climbSentence({ direction: 'climbed', text: '1 tier, 3 steps', mythicText: null, attributed: 2 })).toBe(
      'Climbed 1 tier, 3 steps with this deck this season',
    )
    expect(climbSentence({ direction: 'dropped', text: '1 step', mythicText: null, attributed: 1 })).toBe(
      'Dropped 1 step with this deck this season',
    )
    expect(climbSentence({ direction: 'none', text: 'no change', mythicText: null, attributed: 0 })).toBe(
      'No rank change from this deck this season',
    )
    expect(
      climbSentence({ direction: 'climbed', text: '2 tiers', mythicText: 'and 5.0% in Mythic', attributed: 3 }),
    ).toBe('Climbed 2 tiers with this deck this season and 5.0% in Mythic')
  })
})

describe('rankChartModel icons', () => {
  it('icons mark tier changes, the peak and the current point', () => {
    const model = rankChartModel(line(), WIDTH, HEIGHT)
    expect(model.icons.map((icon) => icon.label)).toEqual(['Silver 3', 'Silver 2', 'Gold 4'])
    expect(model.icons[2].x).toBe(model.points[3].x)
    expect(model.icons[0].url).toContain('constructed_silver_t3')
  })

  it('a flat line has one icon at each end', () => {
    const flat = line({
      points: [
        { atMs: 1_000, steps: 30, label: 'Silver 3', mythic: false },
        { atMs: 2_000, steps: 30, label: 'Silver 3', mythic: false },
        { atMs: 3_000, steps: 30, label: 'Silver 3', mythic: false },
      ],
      gaps: [],
    })
    const model = rankChartModel(flat, WIDTH, HEIGHT)
    expect(model.icons).toHaveLength(2)
    expect(model.icons[0].x).toBe(model.points[0].x)
    expect(model.icons[1].x).toBe(model.points[2].x)
  })

  it('segments carry the gap kind of play', () => {
    const model = rankChartModel(
      line({
        gaps: [
          { from: 0, to: 1, kind: 'attributed', changeSteps: 6, ofDeck: true, bestOf: 'bo1' },
          { from: 1, to: 2, kind: 'quiet', changeSteps: 0, ofDeck: false, bestOf: 'bo3' },
          { from: 2, to: 3, kind: 'played_elsewhere', changeSteps: 14, ofDeck: false, bestOf: null },
        ],
      }),
      WIDTH,
      HEIGHT,
    )
    expect(model.segments.map((segment) => segment.bestOf)).toEqual(['bo1', 'bo3', null])
  })
})
