<script setup lang="ts">
import { useElementSize } from '@vueuse/core'
import { computed, ref, watch } from 'vue'
import type { DayView, PeerChartView } from '@/bindings'
import {
  bucketLabel,
  chartBucketDays,
  chartPoints,
  dayTickLabel,
  dayTicks,
  deckRecord,
  dotRadius,
  letterRecord,
  linearScale,
  linePath,
  nearestIndex,
  niceTicks,
  peerLabel,
  peerTrend,
  pointMatches,
  sessionLine,
  tooltipDecks,
  type ChartPoint,
  type DeckRow,
} from '@/lib/charts'
import ChartModeToggle from './ChartModeToggle.vue'
import { readStored, writeStored } from '@/lib/browserStorage'
import { winRateTextClass } from '@/lib/matchups'
import { dayToUtcMs } from '@/lib/timeframes'

const props = withDefaults(
  defineProps<{
    /** The days; the home dashboard's carry the decks played, which the tooltip lists. */
    data: (DayView & { decks?: DeckRow[] })[]
    timeframe: string
    /** The other decks of the deck's archetype (0.x `peerChart`), drawn as a trend in the win rate mode. */
    peer?: PeerChartView | null
    /** The heading; empty for none (the home dashboard's Results card has its own). */
    title?: string
    /** Where the chosen mode is remembered. */
    storageKey?: string
    /** The chart's height in pixels. */
    height?: number
  }>(),
  {
    peer: null,
    title: 'Performance History',
    storageKey: 'deck:performance-chart-mode',
    height: 400,
  },
)

type ChartMode = 'counts' | 'winrate'

const MARGIN = { top: 10, right: 12, bottom: 28, left: 40 }

// 'bars' is what 0.x called counts when it drew stacked bars.
const mode = ref<ChartMode>(readStored(localStorage, props.storageKey) === 'winrate' ? 'winrate' : 'counts')
watch(mode, (value) => writeStored(localStorage, props.storageKey, value))

const container = ref<HTMLElement>()
const { width } = useElementSize(container)

const points = computed(() => chartPoints(props.data, chartBucketDays(props.timeframe, props.data.length)))

const left = MARGIN.left
const right = computed(() => Math.max(left + 1, width.value - MARGIN.right))
const top = MARGIN.top
const bottom = computed(() => props.height - MARGIN.bottom)

const xs = computed(() => {
  const days = points.value.map((point) => dayToUtcMs(point.date) ?? 0)
  const scale = linearScale([days[0] ?? 0, days[days.length - 1] ?? 0], [left, right.value])
  return days.map(scale)
})

/** Counts share one axis from 0 to the busiest bucket; the win rate runs 0 to 100. */
const yMax = computed(() =>
  mode.value === 'counts' ? Math.ceil(Math.max(...points.value.flatMap((p) => [p.wins, p.losses]), 1)) : 100,
)
const y = computed(() => linearScale([0, yMax.value], [bottom.value, top]))
const yTicks = computed(() => niceTicks(yMax.value, 5, { integer: mode.value === 'counts' }))

const xTicks = computed(() => {
  const first = points.value[0]?.date
  const last = points.value[points.value.length - 1]?.date
  if (!first || !last) return []
  const maxTicks = Math.max(2, Math.floor((right.value - left) / 80))
  const scale = linearScale([dayToUtcMs(first) ?? 0, dayToUtcMs(last) ?? 0], [left, right.value])
  return dayTicks(first, last, maxTicks).map((day) => ({ x: scale(dayToUtcMs(day) ?? 0), label: dayTickLabel(day) }))
})

function seriesPath(value: (point: ChartPoint) => number | null): string {
  return linePath(
    points.value.map((point, index) => {
      const v = value(point)
      return { x: xs.value[index], y: v === null ? null : y.value(v) }
    }),
  )
}

/** The other decks' trend over the same buckets; empty without peers. */
const peerLine = computed(() => (props.peer && props.peer.days.length ? peerTrend(points.value, props.peer.days) : []))
const hasPeer = computed(() => peerLine.value.some((value) => value !== null))
const peerPath = computed(() =>
  linePath(
    peerLine.value.map((value, index) => ({
      x: xs.value[index],
      y: value === null ? null : y.value(value),
    })),
  ),
)

const winsPath = computed(() => seriesPath((p) => p.wins))
const lossesPath = computed(() => seriesPath((p) => p.losses))
const trendPath = computed(() => seriesPath((p) => p.trend))

/** Each bucket's own win rate joined across buckets without matches, and a dot sized by its matches. */
const sessions = computed(() =>
  sessionLine(
    points.value.map((point, index) => ({ x: xs.value[index], rate: point.rate, matches: pointMatches(point) })),
    y.value,
  ),
)

// ---- Crosshair and tooltip ------------------------------------------------------

const hovered = ref<number | null>(null)

function onMove(event: MouseEvent) {
  // Measure from the svg, not the hover rect: the points' x is in svg space.
  const target = event.currentTarget as SVGGraphicsElement
  const box = (target.ownerSVGElement ?? target).getBoundingClientRect()
  const index = nearestIndex(xs.value, event.clientX - box.left)
  hovered.value = index < 0 ? null : index
}

const hoveredPoint = computed(() => (hovered.value === null ? null : (points.value[hovered.value] ?? null)))
const hoveredX = computed(() => (hovered.value === null ? 0 : xs.value[hovered.value]))

/** Dots on each line at the hovered point; none where a line has no value. */
const hoveredDots = computed(() => {
  const point = hoveredPoint.value
  if (!point) return []
  if (mode.value === 'counts') {
    return [
      { y: y.value(point.wins), class: 'fill-success' },
      { y: y.value(point.losses), class: 'fill-destructive' },
    ]
  }
  const dots: { y: number; class: string }[] = []
  if (point.rate !== null) dots.push({ y: y.value(point.rate), class: 'fill-chart-1' })
  if (point.trend !== null) dots.push({ y: y.value(point.trend), class: 'fill-chart-indigo' })
  const peerValue = hasPeer.value && hovered.value !== null ? peerLine.value[hovered.value] : null
  if (peerValue !== null && peerValue !== undefined) dots.push({ y: y.value(peerValue), class: 'fill-muted-foreground' })
  return dots
})

/** What the tooltip says; null hides it (an empty bucket in the counts mode, or before the first match). */
const tooltip = computed(() => {
  const point = hoveredPoint.value
  if (!point) return null
  if (mode.value === 'counts') {
    if (point.wins + point.losses + point.draws === 0) return null
    return {
      heading: bucketLabel(point),
      title: point.rate !== null ? `${point.rate}% win rate` : 'No data',
      detail: letterRecord(point.wins, point.losses, point.draws),
    }
  }
  if (point.trend === null) return null
  const peerValue = hasPeer.value && hovered.value !== null ? peerLine.value[hovered.value] : null
  const matches = pointMatches(point)
  return {
    heading: bucketLabel(point),
    title: point.rate !== null ? `${point.rate}% win rate` : 'No matches',
    detail: matches > 0 ? letterRecord(point.wins, point.losses, point.draws) : null,
    trend: { label: 'Trend', rate: `${point.trend}%` },
    peer: peerValue !== null && peerValue !== undefined && props.peer ? { label: peerLabel(props.peer), rate: `${peerValue}%` } : null,
  }
})

/** The decks played in the hovered bucket, for the win rate tooltip; empty when none are known. */
const deckLines = computed(() => {
  const point = hoveredPoint.value
  return point && mode.value === 'winrate' ? tooltipDecks(point.decks) : { rows: [], more: null }
})

/** The tooltip sits right of the crosshair, or left of it past the middle, and never wider than the room on its side. */
const tooltipStyle = computed(() =>
  hoveredX.value > width.value / 2
    ? { right: `${width.value - hoveredX.value + 12}px`, top: `${top}px`, maxWidth: `${Math.max(0, hoveredX.value - 12)}px` }
    : { left: `${hoveredX.value + 12}px`, top: `${top}px`, maxWidth: `${Math.max(0, width.value - hoveredX.value - 12)}px` },
)
</script>

<template>
  <div class="match-history-chart">
    <div class="flex items-center justify-between">
      <div class="flex items-center gap-3">
        <h3 v-if="title" class="text-sm font-semibold tracking-tight">{{ title }}</h3>
        <ChartModeToggle
          v-model="mode"
          :options="[
            { value: 'counts', label: 'Wins/Losses' },
            { value: 'winrate', label: 'W/R over time' },
          ]"
        />
      </div>
      <div v-if="mode === 'counts'" class="flex items-center gap-4">
        <span class="flex items-center gap-1.5 text-xs text-muted-foreground">
          <span class="inline-block h-0.5 w-3 bg-success" />
          Wins
        </span>
        <span class="flex items-center gap-1.5 text-xs text-muted-foreground">
          <span class="inline-block h-0.5 w-3 bg-destructive" />
          Losses
        </span>
      </div>
      <div v-else class="flex items-center gap-4 text-xs text-muted-foreground">
        <span class="flex items-center gap-1.5">
          <span class="inline-block h-2 w-2 rounded-full bg-chart-1" />
          Sessions
        </span>
        <span class="flex items-center gap-1.5">
          <span class="inline-block h-0.5 w-3 bg-chart-indigo" />
          Trend
        </span>
        <span v-if="hasPeer && peer" class="flex items-center gap-1.5">
          <span class="peer-legend-dash inline-block h-0 w-3" />
          {{ peerLabel(peer) }}
        </span>
      </div>
    </div>

    <div ref="container" class="relative mt-4 w-full" :style="{ height: `${height}px` }">
      <svg v-if="width > 0" :width="width" :height="height" class="block overflow-visible text-xs">
        <!-- Y axis: dashed grid lines and labels -->
        <g v-for="tick in yTicks" :key="`y${tick}`">
          <line :x1="left" :x2="right" :y1="y(tick)" :y2="y(tick)" class="chart-grid" />
          <text :x="left - 8" :y="y(tick)" text-anchor="end" dominant-baseline="middle" class="fill-muted-foreground">
            {{ mode === 'winrate' ? `${tick}%` : tick }}
          </text>
        </g>

        <!-- X axis -->
        <line :x1="left" :x2="right" :y1="bottom" :y2="bottom" class="chart-axis" />
        <g v-for="tick in xTicks" :key="`x${tick.label}${tick.x}`">
          <line :x1="tick.x" :x2="tick.x" :y1="bottom" :y2="bottom + 4" class="chart-axis" />
          <text :x="tick.x" :y="bottom + 16" text-anchor="middle" class="fill-muted-foreground">{{ tick.label }}</text>
        </g>

        <!-- Lines -->
        <template v-if="mode === 'counts'">
          <path :d="winsPath" fill="none" stroke-width="2" class="stroke-success" stroke-linejoin="round" />
          <path :d="lossesPath" fill="none" stroke-width="2" class="stroke-destructive" stroke-linejoin="round" />
        </template>
        <template v-else>
          <path
            v-if="hasPeer"
            :d="peerPath"
            fill="none"
            stroke-width="2"
            stroke-dasharray="5 4"
            class="stroke-muted-foreground"
            stroke-linejoin="round"
          />
          <path
            :d="sessions.path"
            fill="none"
            stroke-width="1.5"
            stroke-opacity="0.5"
            class="stroke-chart-1"
            stroke-linejoin="round"
          />
          <circle v-for="(dot, i) in sessions.dots" :key="i" :cx="dot.x" :cy="dot.y" :r="dot.r" fill-opacity="0.8" class="fill-chart-1" />
          <path :d="trendPath" fill="none" stroke-width="2.5" class="stroke-chart-indigo" stroke-linejoin="round" />
        </template>

        <!-- Crosshair -->
        <g v-if="hoveredPoint" class="pointer-events-none">
          <line :x1="hoveredX" :x2="hoveredX" :y1="top" :y2="bottom" class="chart-crosshair" />
          <circle v-for="(dot, i) in hoveredDots" :key="i" :cx="hoveredX" :cy="dot.y" r="4" :class="dot.class" />
        </g>

        <rect
          :x="left"
          :y="top"
          :width="Math.max(0, right - left)"
          :height="bottom - top"
          fill="transparent"
          @mousemove="onMove"
          @mouseleave="hovered = null"
        />
      </svg>

      <div
        v-if="tooltip"
        class="pointer-events-none absolute z-10 min-w-36 rounded-lg border border-border bg-popover px-3 py-2 leading-normal text-popover-foreground shadow-md"
        :style="tooltipStyle"
      >
        <div class="text-[11px] opacity-60">{{ tooltip.heading }}</div>
        <div class="text-sm font-semibold">{{ tooltip.title }}</div>
        <div v-if="tooltip.detail" class="text-xs opacity-80">{{ tooltip.detail }}</div>
        <div v-if="'trend' in tooltip && tooltip.trend" class="mt-1.5 flex items-center justify-between gap-3 text-xs">
          <span>{{ tooltip.trend.label }}</span>
          <span class="font-semibold">{{ tooltip.trend.rate }}</span>
        </div>
        <div v-if="'peer' in tooltip && tooltip.peer" class="mt-1 flex items-center justify-between gap-3 text-xs">
          <span>{{ tooltip.peer.label }}</span>
          <span class="font-semibold">{{ tooltip.peer.rate }}</span>
        </div>
        <div v-if="deckLines.rows.length" class="mt-2 border-t border-border pt-2 text-xs">
          <div
            v-for="deck in [...deckLines.rows, ...(deckLines.more ? [deckLines.more] : [])]"
            :key="deck.key"
            class="flex items-baseline justify-between gap-3"
          >
            <span class="min-w-0 max-w-32 truncate" :class="{ 'opacity-60': deck.key === 'more' }">{{ deck.name }}</span>
            <span class="flex shrink-0 items-baseline gap-2">
              <span class="opacity-80">{{ deckRecord(deck) }}</span>
              <span class="w-9 text-right font-semibold tabular-nums" :class="winRateTextClass(deck.rate)">{{ deck.rate }}%</span>
            </span>
          </div>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.chart-grid {
  stroke: var(--color-border);
  stroke-dasharray: 4 4;
  stroke-opacity: 0.5;
}

.chart-axis {
  stroke: var(--color-border);
  stroke-opacity: 0.3;
}

.peer-legend-dash {
  border-top: 2px dashed var(--color-muted-foreground);
}

.chart-crosshair {
  stroke: var(--color-muted-foreground);
  stroke-opacity: 0.5;
}
</style>
