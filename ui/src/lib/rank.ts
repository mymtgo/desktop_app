/**
 * The Arena rank chart's geometry (arena spec section 15): one ladder's
 * readings on the season's time axis, the stretch between two readings
 * drawn by what happened in it, and the rank icons where the rank changes.
 */
import type { BestOfView, DeckClimbView, GapKindView, RankLineView } from '@/bindings'
import { linearScale, linePath } from './charts'
import { MYTHIC_STEPS, rankIconName, rankIconUrl, SPARK_STEPS, STEPS_PER_CLASS } from './rankIcons'

export type RankSegment = { d: string; kind: GapKindView; ofDeck: boolean; bestOf: BestOfView | null }

export type RankChartModel = {
  segments: RankSegment[]
  /** One row per class: `y` is the gridline at the class's start, `labelY` the middle of its row. */
  bands: { y: number; labelY: number; label: string }[]
  /** Each reading, in order. */
  points: { x: number; y: number; label: string }[]
  /** A rank icon on the points where the tier changes, the peak and the last point. */
  icons: { x: number; y: number; url: string; label: string }[]
}

/** Room around the plot: class labels on the left, the dates below. */
export const RANK_CHART_MARGIN = { top: 10, right: 10, bottom: 34, left: 62 }

/** Rank icons are about 28 px square, centred on their point. */
export const RANK_ICON_SIZE = 28

/**
 * How far the readings sit inside the plot's left and right edges: a rank
 * icon on the first reading clears the class labels left of the plot.
 */
export const RANK_POINT_INSET = RANK_ICON_SIZE / 2 + 2

/** The classes from Bronze up, in ladder order; Bronze starts at 0 steps. */
const CLASSES = ['Bronze', 'Silver', 'Gold', 'Platinum', 'Diamond', 'Mythic']
/** The y axis always spans the whole ladder, plus one class of room for the Mythic row. */
const Y_TOP = MYTHIC_STEPS + STEPS_PER_CLASS
/** The y axis bottom: the constructed ladder starts at Spark (negative steps), limited at Bronze. */
function yBottom(ladder: RankLineView['ladder']): number {
  return ladder === 'constructed' ? -SPARK_STEPS : 0
}

/**
 * The chart for a `width` by `height` box. Points' x come from their time,
 * on the season (start to end) when known, else the readings' own range,
 * clamped to the plot.
 * The y axis is the whole ladder, whatever the readings: Spark to Mythic on
 * constructed, Bronze to Mythic on limited (limited has no Spark).
 */
export function rankChartModel(line: RankLineView, width: number, height: number): RankChartModel {
  const left = RANK_CHART_MARGIN.left
  const right = Math.max(left + 1, width - RANK_CHART_MARGIN.right)
  const top = RANK_CHART_MARGIN.top
  const bottom = Math.max(top + 1, height - RANK_CHART_MARGIN.bottom)

  const times = line.points.map((point) => point.atMs)
  const firstMs = line.seasonStartMs ?? Math.min(...times)
  const lastMs = line.seasonEndMs ?? Math.max(...times)
  const firstX = Math.min(left + RANK_POINT_INSET, right)
  const lastX = Math.max(firstX, right - RANK_POINT_INSET)
  const scaleX = linearScale([firstMs, lastMs], [firstX, lastX])
  // Season times are read as UTC and may be hours off: a reading outside the
  // season stays on the plot's edge, not over the class labels.
  const x = (ms: number) => Math.min(lastX, Math.max(firstX, scaleX(ms)))

  const low = yBottom(line.ladder)
  const y = linearScale([low, Y_TOP], [bottom, top])

  const points = line.points.map((point) => ({
    x: x(point.atMs),
    y: y(Math.max(low, Math.min(point.steps, Y_TOP))),
    label: point.label,
  }))

  const segments: RankSegment[] = []
  for (const gap of line.gaps) {
    const from = points[gap.from]
    const to = points[gap.to]
    if (!from || !to) continue
    segments.push({ d: linePath([from, to]), kind: gap.kind, ofDeck: gap.ofDeck, bestOf: gap.bestOf })
  }

  const bands: { y: number; labelY: number; label: string }[] = []
  for (let index = CLASSES.length - 1; index >= 0; index -= 1) {
    const start = index * STEPS_PER_CLASS
    bands.push({ y: y(start), labelY: y(start + STEPS_PER_CLASS / 2), label: CLASSES[index] })
  }
  if (low < 0) bands.push({ y: y(low), labelY: y(low / 2), label: 'Spark' })

  const icons = rankIcons(line, points)

  return { segments, bands, points, icons }
}

/** Where the class or tier differs from the previous reading, the first, the peak and the last. */
function rankIcons(line: RankLineView, points: { x: number; y: number; label: string }[]): RankChartModel['icons'] {
  const chosen = new Set<number>()
  let peak = 0
  line.points.forEach((point, index) => {
    const previous = line.points[index - 1]
    if (!previous || rankIconName(line.ladder, previous.steps) !== rankIconName(line.ladder, point.steps)) chosen.add(index)
    if (point.steps > line.points[peak].steps) peak = index
  })
  if (line.points.length > 0) {
    chosen.add(peak)
    chosen.add(line.points.length - 1)
  }
  return [...chosen]
    .sort((a, b) => a - b)
    .map((index) => ({
      x: points[index].x,
      y: points[index].y,
      url: rankIconUrl(line.ladder, line.points[index].steps),
      label: points[index].label,
    }))
}

/** The deck rank card's sentence. */
export function climbSentence(climb: DeckClimbView): string {
  let sentence = 'No rank change from this deck this season'
  if (climb.direction === 'climbed') sentence = `Climbed ${climb.text} with this deck this season`
  if (climb.direction === 'dropped') sentence = `Dropped ${climb.text} with this deck this season`
  return climb.mythicText ? `${sentence} ${climb.mythicText}` : sentence
}
