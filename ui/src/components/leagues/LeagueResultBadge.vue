<script setup lang="ts">
import { computed } from 'vue'
import type { ClassificationView } from '@/bindings'
import { resultBadgeLabel, resultBadgeTone } from '@/lib/leagues'

/** A run's score inside a ring toned by its classification (0.x `LeagueResultBadge`). */
const props = defineProps<{
  classification: ClassificationView
  wins: number
  losses: number
  liveRound: number | null
}>()

const score = computed(() => `${props.wins}-${props.losses}`)
const label = computed(() => resultBadgeLabel(props.classification, props.liveRound))
const tone = computed(() => resultBadgeTone(props.classification))
</script>

<template>
  <div
    class="relative flex shrink-0 flex-col items-center justify-center rounded-md bg-linear-to-t p-0.5"
    :class="tone"
    :title="label"
  >
    <div class="rounded-sm bg-background px-3 py-2">
      <span class="leading-none font-bold">{{ score }}</span>
    </div>
  </div>
</template>
