<script setup lang="ts">
import { computed } from 'vue'
import MeterBar from '@/components/decks/MeterBar.vue'
import { winRateTone } from '@/lib/decks'

const props = defineProps<{
  winrate: number
  size?: 'sm' | 'default'
  /** A darker track, for bars drawn over cover art. */
  solid?: boolean
}>()

const toneClass = computed(() => {
  const tone = winRateTone(props.winrate)
  if (tone === 'loss') return 'text-destructive'
  if (tone === 'even') return 'text-muted-foreground'
  return ''
})

/** The fill follows the rate, as the KPI bars do: red below 50, grey at 50, green above. */
const fillClass = computed(() => {
  const tone = winRateTone(props.winrate)
  if (tone === 'loss') return 'bg-destructive'
  if (tone === 'even') return 'bg-muted-foreground'
  return 'bg-success'
})
</script>

<template>
  <div class="flex items-center gap-2">
    <MeterBar :value="winrate" :size="size" :solid="solid" :fill-class="fillClass" />
    <span class="shrink-0 font-semibold tabular-nums" :class="[size === 'sm' ? 'text-xs' : 'text-sm', toneClass]">
      {{ winrate }}%
    </span>
  </div>
</template>
