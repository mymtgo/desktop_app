<script setup lang="ts">
// The Metagame timeframes: "Last 30 days", "Last 7 days", "All time"
// (archetypes spec section 9, ruling 7).
import type { TimeframeView } from '@/bindings'
import SegmentedControl from '@/components/SegmentedControl.vue'

defineProps<{ timeframes: TimeframeView[] }>()
const model = defineModel<string>({ required: true })
</script>

<template>
  <div role="group" aria-label="Timeframe">
    <SegmentedControl
      v-model="model"
      :options="timeframes.map((timeframe) => ({ value: timeframe.key, label: timeframe.label }))"
    />
  </div>
</template>
