<script setup lang="ts">
import { computed } from 'vue'

/**
 * The numbers card's activity grid: one square per day of the counted year,
 * a column per week (Monday at the top), greener the more matches that day.
 * `byDay` starts on 1 January.
 */
const props = defineProps<{ byDay: number[]; year: number }>()

const DAY_MS = 86_400_000
const ROW_LABELS = ['Mon', '', 'Wed', '', 'Fri', '', '']

const start = computed(() => Date.UTC(props.year, 0, 1))
// Empty squares before 1 January, so it sits on its weekday's row.
const lead = computed(() => (new Date(start.value).getUTCDay() + 6) % 7)
const columns = computed(() => Math.ceil((lead.value + props.byDay.length) / 7))
const max = computed(() => Math.max(0, ...props.byDay))

type Square = { key: string; title: string; shade: string }

const squares = computed<(Square | null)[]>(() => {
  const out: (Square | null)[] = Array.from({ length: lead.value }, () => null)
  props.byDay.forEach((matches, index) => {
    const day = new Date(start.value + index * DAY_MS)
    const date = day.toLocaleString('en-GB', { day: 'numeric', month: 'short', timeZone: 'UTC' })
    out.push({ key: String(index), title: `${date}: ${matches} ${matches === 1 ? 'match' : 'matches'}`, shade: shade(matches) })
  })
  return out
})

// Each month's name over the week its first day falls in.
const months = computed(() => {
  const out: { name: string; column: number }[] = []
  for (let month = 0; month < 12; month += 1) {
    const index = Math.round((Date.UTC(props.year, month, 1) - start.value) / DAY_MS)
    if (index >= props.byDay.length) break
    const name = new Date(Date.UTC(props.year, month, 1)).toLocaleString('en-GB', { month: 'short', timeZone: 'UTC' })
    out.push({ name, column: Math.floor((lead.value + index) / 7) + 1 })
  }
  return out
})

// GitHub-style greens: an empty day stays a faint square.
const LEVELS = ['bg-white/[0.07]', 'bg-emerald-900', 'bg-emerald-700', 'bg-emerald-500', 'bg-emerald-300']

function shade(matches: number): string {
  if (matches === 0 || max.value === 0) return LEVELS[0]
  const share = matches / max.value
  if (share > 0.75) return LEVELS[4]
  if (share > 0.5) return LEVELS[3]
  if (share > 0.25) return LEVELS[2]
  return LEVELS[1]
}
</script>

<template>
  <div class="wrapped-activity grid grid-cols-[auto_minmax(0,1fr)] gap-x-2">
    <span />
    <div class="relative mb-1 grid text-[10px] text-muted-foreground" :style="{ gridTemplateColumns: `repeat(${columns}, minmax(0, 1fr))` }">
      <span v-for="month in months" :key="month.name" class="whitespace-nowrap" :style="{ gridColumnStart: month.column, gridRowStart: 1 }">
        {{ month.name }}
      </span>
    </div>

    <div class="grid grid-rows-7 gap-[3px] text-[10px] leading-none text-muted-foreground">
      <span v-for="(label, row) in ROW_LABELS" :key="row" class="flex items-center">{{ label }}</span>
    </div>
    <div
      class="grid grid-flow-col grid-rows-7 gap-[3px]"
      :style="{ gridTemplateColumns: `repeat(${columns}, minmax(0, 1fr))` }"
    >
      <template v-for="(square, index) in squares" :key="square?.key ?? `lead-${index}`">
        <span v-if="square" class="wrapped-day aspect-square rounded-[2px]" :class="square.shade" :title="square.title" />
        <span v-else class="aspect-square" aria-hidden="true" />
      </template>
    </div>

    <span />
    <div class="mt-2 flex items-center justify-end gap-1 text-[10px] text-muted-foreground">
      <span class="mr-1">less</span>
      <span v-for="level in LEVELS" :key="level" class="size-2.5 rounded-[2px]" :class="level" />
      <span class="ml-1">more</span>
    </div>
  </div>
</template>
