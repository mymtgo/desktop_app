<script setup lang="ts">
import { computed, ref } from 'vue'
import type { ConsoleResult } from '@/bindings'
import type { ChartChoice } from './chart'
import ParamFields from './ParamFields.vue'
import ResultsChart, { type ChartKind } from './ResultsChart.vue'
import ResultsGrid from './ResultsGrid.vue'
import TuiPane from './TuiPane.vue'
import type { PickerOptions } from './useConsole'

// The Results pane: the query's parameter fields above the grid or the
// chart, the row count and time and the Grid | Chart toggle in the title,
// and an error in the error colour. The view and the chart's choices last
// while the window is open.
const props = defineProps<{
  focused: boolean
  result: ConsoleResult | null
  error: string | null
  busy: 'run' | 'export' | null
  paramNames: string[]
  values: Record<string, string>
  pickers: { archetype: PickerOptions; deck: PickerOptions }
}>()
const emit = defineEmits<{ change: [name: string, value: string] }>()

type View = 'grid' | 'chart'
const VIEWS: { view: View; label: string }[] = [
  { view: 'grid', label: 'Grid' },
  { view: 'chart', label: 'Chart' },
]
const view = ref<View>('grid')
const choice = ref<ChartChoice>({ x: null, y: null })
const kind = ref<ChartKind>('bar')

const grid = ref<InstanceType<typeof ResultsGrid> | null>(null)
const chart = ref<InstanceType<typeof ResultsChart> | null>(null)
const empty = ref<HTMLElement | null>(null)

function focus() {
  if (grid.value) grid.value.focus()
  else if (chart.value) chart.value.focus()
  else empty.value?.focus()
}

defineExpose({
  focus,
  toggleView: () => {
    view.value = view.value === 'grid' ? 'chart' : 'grid'
  },
})

function count(n: number): string {
  return n.toLocaleString('en-US')
}

/** The notes after the title: running, or the rows and the time. */
const notes = computed(() => {
  if (props.busy === 'run') return { text: 'running...  esc cancel', warn: true }
  const result = props.result
  if (!result || props.error) return null
  const rows = result.truncated
    ? `first ${count(result.rows.length)} rows`
    : `${count(result.rows.length)} ${result.rows.length === 1 ? 'row' : 'rows'}`
  return { text: `${rows}  ${count(result.elapsedMs)} ms`, warn: result.truncated }
})
</script>

<template>
  <TuiPane title="Results" :focused="focused">
    <template #title>
      <span v-if="notes" :class="notes.warn ? 'tui-warn' : 'tui-dim'">{{ notes.text }}</span>
      <span class="tui-view-toggle">
        <template v-for="(option, index) in VIEWS" :key="option.view">
          <span v-if="index > 0" class="tui-dim">│</span>
          <button
            type="button"
            class="tui-view"
            :class="{ 'tui-view-on': view === option.view }"
            tabindex="-1"
            :data-view="option.view"
            @click="view = option.view"
          >
            {{ option.label }}
          </button>
        </template>
      </span>
    </template>
    <div class="tui-results">
      <ParamFields
        v-if="paramNames.length > 0"
        :names="paramNames"
        :values="values"
        :pickers="pickers"
        @change="(name, value) => emit('change', name, value)"
      />
      <pre v-if="error" ref="empty" class="tui-error" data-error="results" tabindex="0">{{ error }}</pre>
      <template v-else-if="result && result.columns.length > 0">
        <ResultsGrid v-if="view === 'grid'" ref="grid" :result="result" />
        <ResultsChart v-else ref="chart" v-model:choice="choice" v-model:kind="kind" :result="result" />
      </template>
      <p v-else ref="empty" class="tui-empty" tabindex="0">
        {{ result ? 'The query returned no columns.' : 'ctrl+enter runs the query' }}
      </p>
    </div>
  </TuiPane>
</template>
