<script setup lang="ts">
import { computed } from 'vue'
import type { BreakdownView } from '@/bindings'
import HomeCard from '@/components/dashboard/HomeCard.vue'
import { breakdownDescription, breakdownRows, segmentClass, segmentWidth, winRateClass } from '@/lib/homeDashboard'

/**
 * Formats, or Sets in Limited mode (dashboard redesign spec 4.10): the
 * description, total matches, a split bar by share in distinct hues, then a
 * row per entry with its matches and its match win rate, both labelled.
 */
const props = defineProps<{ breakdown: BreakdownView }>()

const rows = computed(() => breakdownRows(props.breakdown.rows))
</script>

<template>
  <HomeCard :title="breakdown.kind === 'sets' ? 'Sets' : 'Formats'">
    <div class="flex flex-col gap-3">
      <p class="breakdown-description -mt-2 text-xs text-muted-foreground">
        {{ breakdownDescription(breakdown.kind) }}
      </p>
      <p class="flex items-baseline gap-1.5">
        <span class="text-3xl font-semibold tabular-nums">{{ breakdown.total }}</span>
        <span class="text-sm text-muted-foreground">matches</span>
      </p>
      <div class="flex h-2 w-full gap-1 overflow-hidden rounded-full border border-black/40 bg-black/10 shadow-inner shadow-black/30">
        <span
          v-for="(row, index) in rows"
          :key="row.key"
          class="bevel block h-full min-w-1 rounded-full shadow shadow-black/50"
          :class="segmentClass(index)"
          :style="{ width: segmentWidth(row.share) }"
        />
      </div>
      <ul class="flex flex-col gap-1.5">
        <li v-for="(row, index) in rows" :key="row.key" class="breakdown-row flex items-center gap-2 text-sm">
          <span class="size-2 shrink-0 rounded-full" :class="segmentClass(index)" />
          <span class="flex-1 truncate">{{ row.label }}</span>
          <span class="text-xs text-muted-foreground tabular-nums">{{ row.matches }} matches</span>
          <span class="w-24 text-right text-xs tabular-nums">
            <span class="font-medium" :class="winRateClass(row.record.winRate)">{{ row.record.winRate }}%</span>
            <span class="text-muted-foreground"> win rate</span>
          </span>
        </li>
      </ul>
    </div>
  </HomeCard>
</template>
