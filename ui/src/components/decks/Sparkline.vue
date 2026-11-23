<script setup lang="ts">
import { computed } from 'vue'
import { sparklinePoints } from '@/lib/charts'

const props = defineProps<{
  /** Win rates, 0 to 100, oldest first. */
  series: (number | null)[]
  /** Draws the line green (true) or red. */
  positive: boolean
}>()

/** Points for a 100x32 viewBox; nothing below two points. */
const points = computed(() => sparklinePoints(props.series))
</script>

<template>
  <svg v-if="points" class="h-8 w-full" viewBox="0 0 100 32" preserveAspectRatio="none" aria-hidden="true">
    <line
      x1="0"
      y1="16"
      x2="100"
      y2="16"
      stroke="currentColor"
      stroke-width="0.5"
      stroke-dasharray="2 2"
      class="text-muted-foreground/40"
    />
    <polyline
      :points="points"
      fill="none"
      stroke="currentColor"
      stroke-width="1.5"
      vector-effect="non-scaling-stroke"
      stroke-linejoin="round"
      stroke-linecap="round"
      :class="positive ? 'text-success' : 'text-destructive'"
    />
  </svg>
</template>
