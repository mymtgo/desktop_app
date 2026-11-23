<script setup lang="ts">
import { computed } from 'vue'
import type { RecordView, WinRateDeltaView } from '@/bindings'
import KpiCard from '@/components/decks/KpiCard.vue'
import Sparkline from '@/components/decks/Sparkline.vue'
import { winRateTextClass } from '@/lib/matchups'

const props = defineProps<{
  record: RecordView
  delta: WinRateDeltaView
  /** The 7-day rolling win rate per chart day (the backend smooths it). */
  sparkline: (number | null)[]
  timeframeLabel: string
}>()

const tone = computed(() => winRateTextClass(props.record.winRate))

const deltaTone = computed(() => {
  if (props.delta.delta === null) return ''
  if (props.delta.delta > 0) return 'bg-success/15 text-success'
  if (props.delta.delta < 0) return 'bg-destructive/15 text-destructive'
  return 'bg-muted text-muted-foreground'
})
</script>

<template>
  <KpiCard label="Match win rate" :scope="timeframeLabel">
    <div class="flex items-center gap-2.5">
      <span class="text-4xl font-bold tabular-nums" :class="tone">{{ record.winRate }}%</span>
      <span
        v-if="delta.delta !== null"
        class="rounded-md px-1.5 py-0.5 text-xs font-semibold tabular-nums"
        :class="deltaTone"
      >
        {{ delta.delta > 0 ? '▲' : delta.delta < 0 ? '▼' : '' }}
        {{ Math.abs(delta.delta) }} pts
      </span>
    </div>

    <p class="text-sm text-muted-foreground tabular-nums">
      {{ record.label }}
      <template v-if="delta.previousRate !== null">
        <span class="mx-1">&middot;</span>
        {{ delta.previousRate }}% previous {{ timeframeLabel.toLowerCase() }}
      </template>
    </p>

    <Sparkline :series="sparkline" :positive="record.winRate >= 50" />
  </KpiCard>
</template>
