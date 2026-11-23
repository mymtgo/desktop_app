<script setup lang="ts">
import { useElementSize } from '@vueuse/core'
import { computed, ref } from 'vue'
import type { RankLineView } from '@/bindings'
import { dayTickLabel } from '@/lib/charts'
import { RANK_CHART_MARGIN, RANK_ICON_SIZE, rankChartModel, type RankSegment } from '@/lib/rank'
import { utcMsToDay } from '@/lib/timeframes'

/**
 * One Arena ladder's season (arena spec section 15): the readings joined by a
 * line, a played elsewhere stretch dashed and muted, the deck's own stretches
 * in the accent colour (`highlightDeck`, Bo1 thinner), Bo3 and Bo1 stretches in two
 * theme chart colours otherwise, and Arena's rank icon where the rank
 * changes (and at the peak and the current reading). Class boundaries are the
 * gridlines.
 */
const props = withDefaults(defineProps<{ line: RankLineView; highlightDeck?: boolean; height?: number }>(), {
  height: 176,
})

/** The chart's height: the dashboards give it more room under their main charts. */
const HEIGHT = computed(() => props.height)
const ICON_SIZE = RANK_ICON_SIZE
/** Before the box is measured (and in tests). */
const FALLBACK_WIDTH = 360

const container = ref<HTMLElement>()
const { width: measured } = useElementSize(container)
const width = computed(() => (measured.value > 0 ? measured.value : FALLBACK_WIDTH))

const model = computed(() => rankChartModel(props.line, width.value, HEIGHT.value))
const left = RANK_CHART_MARGIN.left
const right = computed(() => width.value - RANK_CHART_MARGIN.right)
const bottom = computed(() => HEIGHT.value - RANK_CHART_MARGIN.bottom)

const ladderName = computed(() => (props.line.ladder === 'limited' ? 'Limited' : 'Constructed'))
const ariaLabel = computed(() =>
  props.line.current
    ? `${ladderName.value} rank this season, now ${props.line.current}`
    : `${ladderName.value} rank this season, no reading yet`,
)

/** The season's first and last day under the plot, when known (else the readings'). */
const dates = computed(() => {
  const times = props.line.points.map((point) => point.atMs)
  const start = props.line.seasonStartMs ?? (times.length ? Math.min(...times) : null)
  const end = props.line.seasonEndMs ?? (times.length ? Math.max(...times) : null)
  const label = (ms: number | null) => (ms === null ? '' : dayTickLabel(utcMsToDay(ms)))
  return { start: label(start), end: label(end) }
})

const hasDeckStretch = computed(() => props.highlightDeck && model.value.segments.some((segment) => segment.ofDeck))
const hasElsewhere = computed(() => model.value.segments.some((segment) => segment.kind === 'played_elsewhere'))

const hasBo3 = computed(() => !props.highlightDeck && model.value.segments.some((s) => isKind(s, 'bo3')))
const hasBo1 = computed(() => !props.highlightDeck && model.value.segments.some((s) => isKind(s, 'bo1')))

function isKind(segment: RankSegment, kind: 'bo1' | 'bo3'): boolean {
  return segment.kind !== 'played_elsewhere' && segment.bestOf === kind
}

/** Bo1 stretches on the deck chart are drawn thinner than Bo3. */
function segmentWidth(segment: RankSegment): number {
  return props.highlightDeck && segment.bestOf === 'bo1' ? 1.25 : 2
}

function segmentClass(segment: RankSegment): string {
  if (segment.kind === 'played_elsewhere') return 'stroke-muted-foreground/60'
  if (props.highlightDeck) return segment.ofDeck ? 'stroke-primary' : 'stroke-foreground/60'
  if (segment.bestOf === 'bo3') return 'rank-bo3 stroke-chart-1'
  if (segment.bestOf === 'bo1') return 'rank-bo1 stroke-chart-3'
  return 'stroke-foreground/60'
}
</script>

<template>
  <div>
    <div ref="container" class="w-full" :style="{ height: `${HEIGHT}px` }">
      <svg :width="width" :height="HEIGHT" role="img" :aria-label="ariaLabel" class="block overflow-visible text-[10px]">
        <g v-for="band in model.bands" :key="band.label">
          <line :x1="left" :x2="right" :y1="band.y" :y2="band.y" class="rank-grid" />
          <text :x="left - 6" :y="band.labelY" text-anchor="end" dominant-baseline="middle" class="fill-muted-foreground">
            {{ band.label }}
          </text>
        </g>
        <text :x="left" :y="bottom + 26" class="fill-muted-foreground">{{ dates.start }}</text>
        <text :x="right" :y="bottom + 26" text-anchor="end" class="fill-muted-foreground">{{ dates.end }}</text>

        <g v-for="(segment, index) in model.segments" :key="`s${index}`">
          <path
            v-if="segment.kind === 'played_elsewhere'"
            :d="segment.d"
            fill="none"
            :stroke-width="segmentWidth(segment)"
            stroke-dasharray="4 3"
            stroke-linecap="round"
            :class="segmentClass(segment)"
          />
          <path
            v-else
            :d="segment.d"
            fill="none"
            :stroke-width="segmentWidth(segment)"
            stroke-linecap="round"
            :class="segmentClass(segment)"
          />
          <!-- A wider invisible stroke, so the stretch is easy to hover. -->
          <path v-if="segment.kind === 'played_elsewhere'" :d="segment.d" fill="none" stroke="transparent" stroke-width="10">
            <title>Played elsewhere</title>
          </path>
        </g>

        <image
          v-for="(icon, index) in model.icons"
          :key="`i${index}`"
          :href="icon.url"
          :x="icon.x - ICON_SIZE / 2"
          :y="icon.y - ICON_SIZE / 2"
          :width="ICON_SIZE"
          :height="ICON_SIZE"
          preserveAspectRatio="xMidYMid meet"
        >
          <title>{{ icon.label }}</title>
        </image>
      </svg>
    </div>

    <div v-if="hasDeckStretch || hasBo3 || hasBo1 || hasElsewhere" class="mt-1 flex flex-wrap items-center gap-4 text-xs text-muted-foreground">
      <span v-if="hasDeckStretch" class="flex items-center gap-1.5">
        <span class="inline-block h-0.5 w-3 bg-primary" />
        This deck
      </span>
      <span v-if="hasBo3" class="flex items-center gap-1.5">
        <span class="inline-block h-0.5 w-3 bg-chart-1" />
        Bo3
      </span>
      <span v-if="hasBo1" class="flex items-center gap-1.5">
        <span class="inline-block h-0.5 w-3 bg-chart-3" />
        Bo1
      </span>
      <span v-if="hasElsewhere" class="flex items-center gap-1.5">
        <span class="rank-legend-dash inline-block h-0 w-3" />
        Played elsewhere
      </span>
    </div>
  </div>
</template>

<style scoped>
.rank-grid {
  stroke: var(--color-border);
  stroke-dasharray: 4 4;
  stroke-opacity: 0.6;
}

.rank-legend-dash {
  border-top: 2px dashed var(--color-muted-foreground);
}
</style>
