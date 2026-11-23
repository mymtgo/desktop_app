<script setup lang="ts">
import { computed } from 'vue'

/**
 * The app's bevelled meter: a rounded track with a bevelled fill (the deck
 * tile's win rate bar, the dashboard's share and ladder bars).
 */
const props = withDefaults(
  defineProps<{
    /** 0 to 100; anything outside is clamped. */
    value: number
    size?: 'sm' | 'default'
    /** A darker track, for bars drawn over cover art. */
    solid?: boolean
    fillClass?: string
  }>(),
  { size: 'default', solid: false, fillClass: 'bg-success' },
)

const width = computed(() => `${Math.max(0, Math.min(100, props.value))}%`)
</script>

<template>
  <div
    class="meter-track flex-1 overflow-hidden rounded-full border border-black/40 shadow-inner shadow-black/30"
    :class="[size === 'sm' ? 'h-1.5' : 'h-2', solid ? 'bg-black/60' : 'bg-black/10']"
  >
    <div
      class="meter-fill bevel h-full rounded-full shadow shadow-black/50 transition-[width] duration-300 ease-out"
      :class="fillClass"
      :style="{ width }"
    />
  </div>
</template>
