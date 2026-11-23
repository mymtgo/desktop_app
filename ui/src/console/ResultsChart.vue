<script setup lang="ts">
import { useElementSize } from '@vueuse/core'
import { computed, ref } from 'vue'
import type { ConsoleResult } from '@/bindings'
import { chartAxes, chartPoints, chartRange, chartTicks, labelStep, numericColumns, type ChartChoice } from './chart'

// The result as a simple chart: an X column, a numeric Y column, bars or a
// line. Only the rows on screen are drawn (the capped result). Plain SVG in
// the console's colours: accent marks, dim labels, faint gridlines.
export type ChartKind = 'bar' | 'line'

const props = defineProps<{ result: ConsoleResult; choice: ChartChoice; kind: ChartKind }>()
const emit = defineEmits<{ 'update:choice': [choice: ChartChoice]; 'update:kind': [kind: ChartKind] }>()

const root = ref<HTMLElement | null>(null)
const plotBox = ref<HTMLElement | null>(null)
defineExpose({ focus: () => root.value?.focus() })

// The size of the drawing area; a fixed size until it is measured.
const measured = useElementSize(plotBox)
const width = computed(() => (measured.width.value > 0 ? Math.floor(measured.width.value) : 640))
const height = computed(() => (measured.height.value > 0 ? Math.floor(measured.height.value) : 240))

const FONT = 11
const CHAR = FONT * 0.6
const MAX_LABEL_CHARS = 12
const MARKER_LIMIT = 60

const numeric = computed(() => numericColumns(props.result.columns, props.result.rows))
const axes = computed(() => chartAxes(props.result.columns, numeric.value, props.choice))
const points = computed(() => (axes.value ? chartPoints(props.result.rows, axes.value.x, axes.value.y) : []))
/** The chart's name: "<y> by <x>". */
const title = computed(() => (axes.value ? `${props.result.columns[axes.value.y]} by ${props.result.columns[axes.value.x]}` : ''))
const skipped = computed(() => props.result.rows.length - points.value.length)

function count(n: number): string {
  return n.toLocaleString('en-US')
}

/** A point's value in its tooltip, in full. */
function valueText(n: number): string {
  return n.toLocaleString('en-US', { maximumFractionDigits: 20 })
}

function cut(label: string): string {
  return label.length > MAX_LABEL_CHARS ? `${label.slice(0, MAX_LABEL_CHARS - 1)}…` : label
}

const range = computed(() => chartRange(points.value.map((point) => point.value)))
const ticks = computed(() => chartTicks(range.value))

const layout = computed(() => {
  const longestTick = Math.max(...ticks.value.map((tick) => tick.label.length))
  const left = Math.ceil(longestTick * CHAR) + 10
  const top = 8
  const right = 8
  const bottom = FONT + 10
  const plotWidth = Math.max(10, width.value - left - right)
  const plotHeight = Math.max(10, height.value - top - bottom)
  return { left, top, plotWidth, plotHeight }
})

function yAt(value: number): number {
  const { top: high, bottom: low } = range.value
  const { top, plotHeight } = layout.value
  // chartRange never gives an empty range; the guard keeps a bad one finite.
  const span = high - low || 1
  return top + ((high - value) / span) * plotHeight
}

const slot = computed(() => layout.value.plotWidth / Math.max(1, points.value.length))

function xAt(index: number): number {
  return layout.value.left + (index + 0.5) * slot.value
}

/** Bar width: the slot less a 2px gap, thinner gaps as slots shrink, never under 1px. */
const barWidth = computed(() => {
  const gap = slot.value >= 6 ? 2 : slot.value >= 3 ? 1 : 0
  return Math.max(1, slot.value - gap)
})

const bars = computed(() =>
  points.value.map((point, index) => {
    const zero = yAt(0)
    const end = yAt(point.value)
    return {
      x: xAt(index) - barWidth.value / 2,
      y: Math.min(zero, end),
      height: Math.abs(zero - end),
    }
  }),
)

const linePath = computed(() => points.value.map((point, index) => `${xAt(index)},${yAt(point.value)}`).join(' '))

/** X labels: every Nth point so the cut labels never touch. */
const xLabels = computed(() => {
  const longest = Math.min(MAX_LABEL_CHARS, Math.max(1, ...points.value.map((point) => point.label.length)))
  const room = longest * CHAR + 2 * CHAR
  const step = labelStep(points.value.length, Math.floor(layout.value.plotWidth / room))
  const labels: { x: number; text: string; full: string }[] = []
  for (let index = 0; index < points.value.length; index += step) {
    const label = points.value[index].label
    labels.push({ x: xAt(index), text: cut(label), full: label })
  }
  return labels
})

const columnOptions = computed(() => props.result.columns.map((name, index) => ({ name, index })))
const numericOptions = computed(() => columnOptions.value.filter((option) => numeric.value.includes(option.index)))

function pick(axis: 'x' | 'y', event: Event) {
  const index = Number((event.target as HTMLSelectElement).value)
  const current = axes.value
  if (!current) return
  const columns = props.result.columns
  const next = { x: columns[current.x], y: columns[current.y] }
  next[axis] = columns[index]
  emit('update:choice', next)
}

function pickKind(event: Event) {
  emit('update:kind', (event.target as HTMLSelectElement).value === 'line' ? 'line' : 'bar')
}
</script>

<template>
  <div ref="root" class="tui-chart" tabindex="0" aria-label="Results chart">
    <p v-if="!axes" class="tui-empty">No numeric column to chart.</p>
    <template v-else>
      <div class="tui-params">
        <label class="tui-param">
          <span class="tui-param-name">x</span>
          <select class="tui-field" aria-label="Chart X column" :value="axes.x" @change="pick('x', $event)">
            <option v-for="option in columnOptions" :key="option.index" :value="option.index">{{ option.name }}</option>
          </select>
        </label>
        <label class="tui-param">
          <span class="tui-param-name">y</span>
          <select class="tui-field" aria-label="Chart Y column" :value="axes.y" @change="pick('y', $event)">
            <option v-for="option in numericOptions" :key="option.index" :value="option.index">{{ option.name }}</option>
          </select>
        </label>
        <label class="tui-param">
          <span class="tui-param-name">as</span>
          <select class="tui-field" aria-label="Chart kind" :value="kind" @change="pickKind">
            <option value="bar">bar</option>
            <option value="line">line</option>
          </select>
        </label>
      </div>
      <p class="tui-chart-title">
        <span>{{ title }}</span>
        <span v-if="result.truncated" class="tui-warn">first {{ count(result.rows.length) }} rows</span>
        <span v-if="skipped > 0" class="tui-dim">{{ count(skipped) }} NULL {{ skipped === 1 ? 'value' : 'values' }} left out</span>
      </p>
      <p v-if="points.length === 0" class="tui-empty">No rows to chart.</p>
      <div v-else ref="plotBox" class="tui-chart-plot">
        <svg
          class="tui-chart-svg"
          :width="width"
          :height="height"
          :viewBox="`0 0 ${width} ${height}`"
          role="img"
          :aria-label="title"
        >
          <g class="tui-chart-grid">
            <g v-for="tick in ticks" :key="tick.value">
              <line
                :x1="layout.left"
                :x2="layout.left + layout.plotWidth"
                :y1="yAt(tick.value)"
                :y2="yAt(tick.value)"
                :class="tick.value === 0 ? 'tui-chart-baseline' : 'tui-chart-gridline'"
              />
              <text class="tui-chart-label" :x="layout.left - 6" :y="yAt(tick.value)" text-anchor="end" dominant-baseline="middle">
                {{ tick.label }}
              </text>
            </g>
          </g>
          <polyline v-if="kind === 'line'" class="tui-chart-line" :points="linePath" />
          <g v-for="(point, index) in points" :key="index" class="tui-chart-point">
            <title>{{ point.label }}: {{ valueText(point.value) }}</title>
            <rect
              class="tui-chart-hit"
              :x="xAt(index) - slot / 2"
              :y="layout.top"
              :width="slot"
              :height="layout.plotHeight"
            />
            <rect
              v-if="kind === 'bar'"
              data-bar
              class="tui-chart-bar"
              :x="bars[index].x"
              :y="bars[index].y"
              :width="barWidth"
              :height="bars[index].height"
            />
            <circle
              v-else-if="points.length <= MARKER_LIMIT"
              data-point
              class="tui-chart-marker"
              :cx="xAt(index)"
              :cy="yAt(point.value)"
              r="3"
            />
          </g>
          <text
            v-for="label in xLabels"
            :key="label.x"
            class="tui-chart-label"
            :x="label.x"
            :y="layout.top + layout.plotHeight + FONT + 4"
            text-anchor="middle"
          >
            <title>{{ label.full }}</title>{{ label.text }}
          </text>
        </svg>
      </div>
    </template>
  </div>
</template>
