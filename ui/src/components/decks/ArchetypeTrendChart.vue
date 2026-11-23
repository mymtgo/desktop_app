<script setup lang="ts">
import { useElementSize } from '@vueuse/core'
import { computed, ref, watch } from 'vue'
import type { DeckTrendView, PeerDayView } from '@/bindings'
import {
  axisDays,
  bucketLabel,
  chartPoints,
  dayRange,
  dayTickLabel,
  dayTicks,
  letterRecord,
  linearScale,
  linePath,
  nearestIndex,
  pointMatches,
  sessionLine,
  trendStyle,
  type ChartPoint,
} from '@/lib/charts'
import ChartModeToggle from './ChartModeToggle.vue'
import { readStored, writeStored } from '@/lib/browserStorage'

/**
 * The archetype Trends tab's chart. Overall draws the archetype as the
 * results chart does: each day's own win rate as sessions and the trend over
 * the matches in order. Per deck draws one trend line per deck. Every line
 * keeps its value on days without a match.
 */
const props = defineProps<{
  /** The archetype's days with a match, oldest first. */
  overall: PeerDayView[]
  decks: DeckTrendView[]
}>()

const emit = defineEmits<{
  'open-deck': [deckId: number]
}>()

type TrendMode = 'overall' | 'decks'

const STORAGE_KEY = 'archetype:trends-mode'
const HEIGHT = 400
const MARGIN = { top: 10, right: 12, bottom: 28, left: 40 }
const Y_TICKS = [0, 25, 50, 75, 100]

const mode = ref<TrendMode>(readStored(localStorage, STORAGE_KEY) === 'decks' ? 'decks' : 'overall')
watch(mode, (value) => writeStored(localStorage, STORAGE_KEY, value))

const container = ref<HTMLElement>()
const { width } = useElementSize(container)

const allDays = computed(() => {
  const days = props.overall
  return days.length > 0 ? dayRange(days[0].date, days[days.length - 1].date) : []
})

type Line = {
  key: string
  name: string
  deckId: number
  colour: string
  dash: string | null
  points: ChartPoint[]
  path: string
}

const left = MARGIN.left
const right = computed(() => Math.max(left + 1, width.value - MARGIN.right))
const top = MARGIN.top
const bottom = computed(() => HEIGHT - MARGIN.bottom)

const x = computed(() => linearScale([0, allDays.value.length - 1], [left, right.value]))
const xs = computed(() => allDays.value.map((_, index) => x.value(index)))
const y = computed(() => linearScale([0, 100], [bottom.value, top]))

const xTicks = computed(() => {
  const first = allDays.value[0]
  const last = allDays.value[allDays.value.length - 1]
  if (!first || !last) return []
  const maxTicks = Math.max(2, Math.floor((right.value - left) / 80))
  return dayTicks(first, last, maxTicks).map((day) => ({
    x: x.value(allDays.value.indexOf(day)),
    label: dayTickLabel(day),
  }))
})

function trendPathOf(points: ChartPoint[]): string {
  return linePath(points.map((point, index) => ({ x: xs.value[index], y: point.trend === null ? null : y.value(point.trend) })))
}

/** One point per day of the axis: the day's own win rate and the trend after it. */
function pointsOf(days: PeerDayView[]): ChartPoint[] {
  return chartPoints(axisDays(days, allDays.value), 1)
}

const overallPoints = computed(() => pointsOf(props.overall))
const overallTrendPath = computed(() => trendPathOf(overallPoints.value))
const overallSessions = computed(() =>
  sessionLine(
    overallPoints.value.map((point, index) => ({ x: xs.value[index], rate: point.rate, matches: pointMatches(point) })),
    y.value,
  ),
)

const deckLines = computed<Line[]>(() =>
  props.decks.map((deck) => {
    const points = pointsOf(deck.days)
    return {
      key: `deck${deck.deckId}`,
      name: deck.name,
      deckId: deck.deckId,
      ...trendStyle(deck.slot),
      points,
      path: trendPathOf(points),
    }
  }),
)

// ---- Hover: one line raised, the rest faded ----------------------------------------

const activeKey = ref<string | null>(null)
/** The legend entry holding keyboard focus; its raise outlasts the pointer. */
const focusedKey = ref<string | null>(null)
const hovered = ref<number | null>(null)

// A mode change or a new range can drop the line under the pointer.
watch([mode, () => props.decks], () => {
  focusedKey.value = null
  activeKey.value = null
  hovered.value = null
})

/** Per deck's lines in drawing order: the raised one last, so it sits on top. */
const drawn = computed(() => {
  if (mode.value !== 'decks') return []
  const active = activeKey.value
  const lines = deckLines.value
  if (active === null) return lines
  return [...lines.filter((line) => line.key !== active), ...lines.filter((line) => line.key === active)]
})

function lineOpacity(line: Line): number {
  return activeKey.value === null || activeKey.value === line.key ? 1 : 0.15
}

function lineWidth(line: Line): number {
  return activeKey.value === line.key ? 3.5 : 2
}

function indexAt(event: MouseEvent): number | null {
  // Measure from the svg, not the element: the points' x is in svg space.
  const target = event.currentTarget as SVGGraphicsElement
  const box = (target.ownerSVGElement ?? target).getBoundingClientRect()
  const index = nearestIndex(xs.value, event.clientX - box.left)
  return index < 0 ? null : index
}

function onPlotMove(event: MouseEvent) {
  hovered.value = indexAt(event)
}

function onLineMove(event: MouseEvent, line: Line) {
  activeKey.value = line.key
  hovered.value = indexAt(event)
}

function leaveLine() {
  activeKey.value = focusedKey.value
  hovered.value = null
}

function focusEntry(key: string) {
  focusedKey.value = key
  activeKey.value = key
}

function blurEntry() {
  focusedKey.value = null
  activeKey.value = null
}

/** The line the tooltip speaks for in Per deck: the raised one. */
const tooltipLine = computed(() => deckLines.value.find((line) => line.key === activeKey.value) ?? null)

/** The hovered day's point of the line the tooltip speaks for; null before its first match. */
const hoveredPoint = computed(() => {
  if (hovered.value === null) return null
  const points = mode.value === 'overall' ? overallPoints.value : tooltipLine.value?.points
  const point = points?.[hovered.value]
  return point && point.trend !== null ? point : null
})

const hoveredX = computed(() => (hovered.value === null ? 0 : xs.value[hovered.value]))

/** Dots on the lines at the hovered day; none where a line has no value. */
const hoveredDots = computed(() => {
  const point = hoveredPoint.value
  if (!point || point.trend === null) return []
  if (mode.value === 'decks') return [{ y: y.value(point.trend), fill: tooltipLine.value?.colour ?? '' }]
  const dots = [{ y: y.value(point.trend), fill: 'var(--color-chart-indigo)' }]
  if (point.rate !== null) dots.push({ y: y.value(point.rate), fill: 'var(--color-chart-1)' })
  return dots
})

const tooltip = computed(() => {
  const point = hoveredPoint.value
  if (!point || point.trend === null) return null
  const matches = pointMatches(point)
  const record = matches > 0 ? letterRecord(point.wins, point.losses, point.draws) : null
  if (mode.value === 'decks') {
    return {
      heading: bucketLabel(point),
      name: tooltipLine.value?.name ?? null,
      title: `${point.trend}% trend`,
      detail: record ?? 'No matches that day',
      trend: null,
    }
  }
  return {
    heading: bucketLabel(point),
    name: null,
    title: point.rate !== null ? `${point.rate}% win rate` : 'No matches',
    detail: record,
    trend: `${point.trend}%`,
  }
})

const tooltipStyle = computed(() =>
  hoveredX.value > width.value / 2
    ? { right: `${width.value - hoveredX.value + 12}px`, top: `${top}px` }
    : { left: `${hoveredX.value + 12}px`, top: `${top}px` },
)

const legend = computed(() =>
  props.decks.map((deck) => {
    const total = deck.wins + deck.losses + deck.draws
    return {
      key: `deck${deck.deckId}`,
      deckId: deck.deckId,
      name: deck.name,
      record: letterRecord(deck.wins, deck.losses, deck.draws),
      rate: total > 0 ? `${Math.round((100 * deck.wins) / total)}%` : '-',
      style: trendStyle(deck.slot),
    }
  }),
)
</script>

<template>
  <div class="archetype-trend-chart">
    <div
      v-if="decks.length === 0"
      class="rounded-lg border border-dashed border-border px-3 py-10 text-center text-sm text-muted-foreground"
    >
      No matches in this timeframe
    </div>

    <template v-else>
      <div class="flex items-center justify-between">
        <ChartModeToggle
          v-model="mode"
          label="Chart mode"
          :options="[
            { value: 'overall', label: 'Overall' },
            { value: 'decks', label: 'Per deck' },
          ]"
        />
        <div v-if="mode === 'overall'" class="flex items-center gap-4 text-xs text-muted-foreground">
          <span class="flex items-center gap-1.5">
            <span class="inline-block h-2 w-2 rounded-full bg-chart-1" />
            Sessions
          </span>
          <span class="flex items-center gap-1.5">
            <span class="inline-block h-0.5 w-3 bg-chart-indigo" />
            Trend
          </span>
        </div>
      </div>

      <div ref="container" class="relative mt-4 w-full" :style="{ height: `${HEIGHT}px` }">
        <svg v-if="width > 0" :width="width" :height="HEIGHT"
          class="block overflow-visible text-xs"
          role="img"
          aria-label="Win rate over time">
          <g v-for="tick in Y_TICKS" :key="`y${tick}`">
            <line
              :x1="left"
              :x2="right"
              :y1="y(tick)"
              :y2="y(tick)"
              :class="tick === 50 ? 'chart-grid chart-grid-mid' : 'chart-grid'"
            />
            <text :x="left - 8" :y="y(tick)" text-anchor="end" dominant-baseline="middle" class="fill-muted-foreground">
              {{ tick }}%
            </text>
          </g>

          <line :x1="left" :x2="right" :y1="bottom" :y2="bottom" class="chart-axis" />
          <g v-for="tick in xTicks" :key="`x${tick.label}${tick.x}`">
            <line :x1="tick.x" :x2="tick.x" :y1="bottom" :y2="bottom + 4" class="chart-axis" />
            <text :x="tick.x" :y="bottom + 16" text-anchor="middle" class="fill-muted-foreground">{{ tick.label }}</text>
          </g>

          <!-- The plot catches the pointer between lines (Overall's crosshair). -->
          <rect
            :x="left"
            :y="top"
            :width="Math.max(0, right - left)"
            :height="bottom - top"
            fill="transparent"
            @mousemove="onPlotMove"
            @mouseleave="hovered = null"
          />

          <g v-if="mode === 'overall' && hoveredPoint" class="pointer-events-none">
            <line :x1="hoveredX" :x2="hoveredX" :y1="top" :y2="bottom" class="chart-crosshair" />
          </g>

          <template v-if="mode === 'overall'">
            <path
              :d="overallSessions.path"
              fill="none"
              stroke-width="1.5"
              stroke-opacity="0.5"
              class="stroke-chart-1 pointer-events-none"
              stroke-linejoin="round"
            />
            <circle
              v-for="(dot, i) in overallSessions.dots"
              :key="i"
              :cx="dot.x"
              :cy="dot.y"
              :r="dot.r"
              fill-opacity="0.8"
              class="fill-chart-1 pointer-events-none"
            />
            <path
              :d="overallTrendPath"
              fill="none"
              stroke-width="2.5"
              class="stroke-chart-indigo pointer-events-none"
              stroke-linejoin="round"
            />
          </template>

          <g
            v-for="line in drawn"
            :key="line.key"
            class="chart-line"
            :style="{ opacity: lineOpacity(line) }"
          >
            <path
              :d="line.path"
              fill="none"
              :stroke="line.colour"
              :stroke-width="lineWidth(line)"
              :stroke-dasharray="line.dash ?? undefined"
              stroke-linejoin="round"
              class="pointer-events-none"
            />
            <path
              :d="line.path"
              fill="none"
              stroke="transparent"
              stroke-width="12"
              stroke-linejoin="round"
              style="pointer-events: stroke"
              @mousemove="onLineMove($event, line)"
              @mouseleave="leaveLine"
            />
          </g>

          <circle
            v-for="(dot, i) in hoveredDots"
            :key="`hover${i}`"
            :cx="hoveredX"
            :cy="dot.y"
            r="4"
            :fill="dot.fill"
            class="pointer-events-none"
          />
        </svg>

        <div
          v-if="tooltip"
          class="pointer-events-none absolute z-10 min-w-36 rounded-lg border border-border bg-popover px-3 py-2 leading-normal text-popover-foreground shadow-md"
          :style="tooltipStyle"
        >
          <div class="text-[11px] opacity-60">{{ tooltip.heading }}</div>
          <div v-if="tooltip.name" class="max-w-56 truncate text-xs font-medium">{{ tooltip.name }}</div>
          <div class="text-sm font-semibold">{{ tooltip.title }}</div>
          <div v-if="tooltip.detail" class="text-xs opacity-80">{{ tooltip.detail }}</div>
          <div v-if="tooltip.trend" class="mt-1.5 flex items-center justify-between gap-3 text-xs">
            <span>Trend</span>
            <span class="font-semibold">{{ tooltip.trend }}</span>
          </div>
        </div>
      </div>

      <ul
        v-if="mode === 'decks'"
        class="mt-3 grid max-h-56 grid-cols-1 gap-x-4 overflow-y-auto text-xs sm:grid-cols-2 xl:grid-cols-3"
        aria-label="Decks"
        @mouseleave="activeKey = focusedKey"
      >
        <li v-for="entry in legend" :key="entry.key">
          <button
            type="button"
            class="flex w-full cursor-pointer items-center gap-2 rounded px-1.5 py-1 text-left hover:bg-muted/60 focus-visible:bg-muted/60"
            :title="`Open ${entry.name}`"
            @mouseenter="activeKey = entry.key"
            @focus="focusEntry(entry.key)"
            @blur="blurEntry"
            @click="emit('open-deck', entry.deckId)"
          >
            <svg width="22" height="6" class="shrink-0" aria-hidden="true">
              <line
                x1="0"
                x2="22"
                y1="3"
                y2="3"
                :stroke="entry.style.colour"
                stroke-width="2"
                :stroke-dasharray="entry.style.dash ?? undefined"
              />
            </svg>
            <span class="min-w-0 flex-1 truncate">{{ entry.name }}</span>
            <span class="shrink-0 text-muted-foreground tabular-nums">{{ entry.record }}</span>
            <span class="w-9 shrink-0 text-right font-medium tabular-nums">{{ entry.rate }}</span>
          </button>
        </li>
      </ul>
    </template>
  </div>
</template>

<style scoped>
.chart-grid {
  stroke: var(--color-border);
  stroke-dasharray: 4 4;
  stroke-opacity: 0.5;
}

.chart-grid-mid {
  stroke-opacity: 0.9;
}

.chart-axis {
  stroke: var(--color-border);
  stroke-opacity: 0.3;
}

.chart-crosshair {
  stroke: var(--color-muted-foreground);
  stroke-opacity: 0.5;
}

.chart-line {
  transition: opacity 120ms;
}
</style>
