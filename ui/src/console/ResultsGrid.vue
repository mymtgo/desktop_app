<script setup lang="ts">
import { computed, ref } from 'vue'
import type { ConsoleResult, ConsoleValue } from '@/bindings'

// The result rows as a dense grid: sticky header, numbers right aligned,
// NULL and blobs dim, long text cut at the column's width (the full value
// in the cell's title).
const props = defineProps<{ result: ConsoleResult }>()

const scroller = ref<HTMLElement | null>(null)
defineExpose({ focus: () => scroller.value?.focus() })

function isNumber(value: ConsoleValue): boolean {
  return value.kind === 'integer' || value.kind === 'bigInteger' || (value.kind === 'real' && value.value !== null)
}

function cellText(value: ConsoleValue): string {
  switch (value.kind) {
    case 'null':
      return 'NULL'
    case 'integer':
      return String(value.value)
    case 'bigInteger':
      return value.value
    case 'real':
      return value.value === null ? 'NULL' : String(value.value)
    case 'text':
      return value.value
    case 'blob':
      return `<blob ${value.value} bytes>`
  }
}

function cellClass(value: ConsoleValue): string {
  if (isNumber(value)) return 'tui-num'
  if (value.kind === 'text') return ''
  return 'tui-dim'
}

/** A column is numeric when its first value that is not NULL is a number. */
const numericColumns = computed(() =>
  props.result.columns.map((_, index) => {
    for (const row of props.result.rows) {
      const value = row[index]
      if (value && value.kind !== 'null' && !(value.kind === 'real' && value.value === null)) return isNumber(value)
    }
    return false
  }),
)
</script>

<template>
  <div ref="scroller" class="tui-grid-scroll" tabindex="0" aria-label="Query results">
    <table class="tui-grid">
      <thead>
        <tr>
          <th v-for="(column, index) in result.columns" :key="index" :class="{ 'tui-num': numericColumns[index] }">
            <span class="tui-cell" :title="column">{{ column }}</span>
          </th>
        </tr>
      </thead>
      <tbody>
        <tr v-for="(row, rowIndex) in result.rows" :key="rowIndex">
          <td v-for="(value, index) in row" :key="index" :class="cellClass(value)">
            <span class="tui-cell" :title="cellText(value)">{{ cellText(value) }}</span>
          </td>
        </tr>
      </tbody>
    </table>
  </div>
</template>
