<script setup lang="ts">
import { useElementSize } from '@vueuse/core'
import { computed, ref } from 'vue'
import type { MatchHistoryView } from '@/bindings'
import { letterRecord, linearScale, linePath, nearestIndex } from '@/lib/charts'
import { trendPoints, trendTicks } from '@/lib/matchups'

/**
 * The drawer's "Win Rate Trend" (0.x `MatchupTrendChart`): the running win
 * rate after each match, oldest to newest, a dot per match coloured by its
 * result, a dashed 50% line, and the match under the pointer.
 */
const props = defineProps<{
  /** Newest first, as the drawer reads it. */
  history: MatchHistoryView[]
}>()

const HEIGHT = 140
const MARGIN = { top: 8, right: 8, bottom: 20, left: 34 }

const points = computed(() => trendPoints(props.history))
const container = ref<HTMLElement>()
const { width } = useElementSize(container)

const left = MARGIN.left
const right = computed(() => Math.max(left + 1, width.value - MARGIN.right))
const top = MARGIN.top
const bottom = HEIGHT - MARGIN.bottom

const x = computed(() => linearScale([-0.5, points.value.length - 0.5], [left, right.value]))
const y = linearScale([0, 100], [bottom, top])
const xs = computed(() => points.value.map((point) => x.value(point.index)))
const path = computed(() => linePath(points.value.map((point, index) => ({ x: xs.value[index], y: y(point.cumRate) }))))
const ticks = computed(() => trendTicks(points.value.length))

function dotClass(outcome: string): string {
  if (outcome === 'win') return 'fill-success'
  if (outcome === 'loss') return 'fill-destructive'
  return 'fill-muted-foreground'
}

function dotBackground(outcome: string): string {
  if (outcome === 'win') return 'bg-success'
  if (outcome === 'loss') return 'bg-destructive'
  return 'bg-muted-foreground'
}

function outcomeLabel(outcome: string): string {
  if (outcome === 'win') return 'Win'
  if (outcome === 'loss') return 'Loss'
  if (outcome === 'draw') return 'Draw'
  return 'Unknown'
}

const hovered = ref<number | null>(null)
const hoveredPoint = computed(() => (hovered.value === null ? null : (points.value[hovered.value] ?? null)))

function onMove(event: MouseEvent) {
  const target = event.currentTarget as SVGGraphicsElement
  const box = (target.ownerSVGElement ?? target).getBoundingClientRect()
  const index = nearestIndex(xs.value, event.clientX - box.left)
  hovered.value = index < 0 ? null : index
}

const tooltipStyle = computed(() => {
  const at = hovered.value === null ? 0 : xs.value[hovered.value]
  return at > width.value / 2 ? { right: `${width.value - at + 12}px`, top: '0px' } : { left: `${at + 12}px`, top: '0px' }
})
</script>

<template>
  <div>
    <div
      v-if="points.length < 2"
      class="rounded-lg border border-dashed border-border px-3 py-4 text-center text-xs text-muted-foreground"
    >
      Need at least two matches to show a trend.
    </div>
    <div v-else ref="container" class="relative h-[140px] w-full">
      <svg v-if="width > 0" :width="width" :height="HEIGHT" class="block overflow-visible text-[10px]">
        <g v-for="tick in [0, 50, 100]" :key="`y${tick}`">
          <line :x1="left" :x2="right" :y1="y(tick)" :y2="y(tick)" class="trend-grid" />
          <text :x="left - 6" :y="y(tick)" text-anchor="end" dominant-baseline="middle" class="fill-muted-foreground">
            {{ tick }}%
          </text>
        </g>
        <g v-for="tick in ticks" :key="`x${tick}`">
          <text :x="xs[tick]" :y="bottom + 14" text-anchor="middle" class="fill-muted-foreground">
            {{ points[tick]?.dateLabel }}
          </text>
        </g>
        <line :x1="left" :x2="right" :y1="y(50)" :y2="y(50)" stroke-dasharray="4 4" class="stroke-muted-foreground" />
        <path :d="path" fill="none" stroke-width="2" class="stroke-chart-1" stroke-linejoin="round" />
        <circle
          v-for="(point, index) in points"
          :key="point.index"
          :cx="xs[index]"
          :cy="y(point.cumRate)"
          :r="hovered === index ? 4.5 : 3.5"
          :class="dotClass(point.outcome)"
        />
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
        v-if="hoveredPoint"
        class="pointer-events-none absolute z-10 min-w-44 rounded-lg border border-border bg-popover px-3 py-2 text-xs leading-normal text-popover-foreground shadow-md"
        :style="tooltipStyle"
      >
        <div class="text-[11px] opacity-60">{{ hoveredPoint.dateLabel }}</div>
        <div class="mt-1 flex items-center justify-between gap-4">
          <span class="flex items-center gap-1.5">
            <span class="size-2 rounded-full" :class="dotBackground(hoveredPoint.outcome)" />
            {{ hoveredPoint.opponent ?? 'Unknown opponent' }}
          </span>
          <span class="font-semibold">{{ outcomeLabel(hoveredPoint.outcome) }} {{ hoveredPoint.score }}</span>
        </div>
        <div class="mt-1.5 text-[11px] opacity-70">
          After this match: <strong>{{ hoveredPoint.cumRate }}%</strong>
          ({{ letterRecord(hoveredPoint.cumWins, hoveredPoint.cumLosses, hoveredPoint.cumDraws) }})
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.trend-grid {
  stroke: var(--color-border);
  stroke-dasharray: 4 4;
  stroke-opacity: 0.5;
}
</style>
