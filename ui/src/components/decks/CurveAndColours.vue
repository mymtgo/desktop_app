<script setup lang="ts">
import { computed } from 'vue'
import type { ColourCount, CurvePoint } from '@/bindings'
import ManaColors from '@/components/mana/ManaColors.vue'
import { colourStats, curveBuckets, curveMax } from '@/lib/decklist'

/**
 * Mana curve and colour split of a card list, lands left out. The numbers
 * come from the backend (0.x worked them out here); `scope` names the list in
 * both headings, e.g. "maindeck, nonland".
 */
const props = defineProps<{
  curve: CurvePoint[]
  colours: ColourCount[]
  /** Nonland cards: the base of the colour percentages. */
  total: number
  scope: string
}>()

const cmcDistribution = computed(() => curveBuckets(props.curve))
const cmcMax = computed(() => curveMax(cmcDistribution.value))
const colorDistribution = computed(() => colourStats(props.colours, props.total))
</script>

<template>
  <div class="flex flex-col gap-4">
    <div v-if="cmcDistribution.length" class="flex flex-col gap-2">
      <h3 class="text-sm font-medium text-muted-foreground">
        Mana Curve <span class="text-xs font-normal">({{ scope }})</span>
      </h3>
      <div class="flex items-end gap-1" style="height: 120px">
        <div v-for="bucket in cmcDistribution" :key="bucket.cmc" class="flex flex-1 flex-col items-center gap-1">
          <span class="text-xs text-muted-foreground tabular-nums">{{ bucket.count }}</span>
          <div
            class="w-full rounded-t bg-primary/80 transition-all"
            :style="{ height: `${(bucket.count / cmcMax) * 90}px` }"
          />
          <span class="text-xs text-muted-foreground tabular-nums">{{ bucket.cmc }}</span>
        </div>
      </div>
    </div>

    <div v-if="colorDistribution.length" class="flex flex-col gap-2">
      <h3 class="text-sm font-medium text-muted-foreground">
        Color Distribution <span class="text-xs font-normal">({{ scope }})</span>
      </h3>
      <div class="grid grid-cols-2 gap-2">
        <div
          v-for="stat in colorDistribution"
          :key="stat.color"
          class="flex items-center gap-3 rounded-md border border-border bg-muted/30 px-3 py-2.5"
        >
          <ManaColors :colors="[stat.color]" class="shrink-0" />
          <div class="flex flex-1 flex-col">
            <span class="text-sm font-semibold tabular-nums">{{ stat.percentage }}%</span>
            <span class="text-xs text-muted-foreground">{{ stat.count }} of {{ stat.total }} cards</span>
          </div>
        </div>
      </div>
    </div>
  </div>
</template>
