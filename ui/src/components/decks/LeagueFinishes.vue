<script setup lang="ts">
import { computed } from 'vue'
import type { FinishesView } from '@/bindings'
import { Card, CardContent } from '@/components/ui/card'
import { finishBars } from '@/lib/leagueRunCard'

/** The deck dashboard's "League Finishes" bars (0.x `DeckDashboard`): 5-0 down to 0-5, then drops. */
const props = defineProps<{ finishes: FinishesView }>()

const bars = computed(() => finishBars(props.finishes))
</script>

<template>
  <Card class="gap-0 overflow-hidden p-0">
    <CardContent class="p-4">
      <p class="mb-3 text-xs font-medium tracking-wide text-muted-foreground uppercase">League Finishes</p>
      <div class="flex flex-col gap-2">
        <div v-for="bar in bars" :key="bar.key" class="flex items-center gap-3">
          <span class="w-8 text-right text-sm font-medium tabular-nums">{{ bar.label }}</span>
          <div class="relative h-5 flex-1 rounded bg-muted">
            <div class="h-full rounded" :class="bar.barClass" :style="{ width: `${bar.percent}%` }" />
          </div>
          <span class="w-6 text-right text-sm text-muted-foreground tabular-nums">{{ bar.count }}</span>
        </div>
      </div>
    </CardContent>
  </Card>
</template>
