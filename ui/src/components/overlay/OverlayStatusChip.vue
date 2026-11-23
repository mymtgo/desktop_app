<script setup lang="ts">
import type { ChipTone } from '@/lib/leagueOverlayCard'

// A pill on the event card (0.x `OverlayStatusChip.vue`; spacing per event
// overlay spec 10): 16 px tall on every tone, a 1 px border in place of a ring.
defineProps<{ tone: ChipTone }>()
</script>

<template>
  <span
    class="inline-flex h-4 min-w-0 items-center gap-1 rounded-full border px-[7px] text-[10px] leading-3 font-bold tracking-widest whitespace-nowrap uppercase [text-shadow:none]"
    :class="{
      'border-transparent bg-black/45 text-white/85 backdrop-blur-sm': tone === 'neutral',
      'border-amber-400/85 bg-black/75 text-amber-400': tone === 'amber',
      'border-emerald-400/85 bg-black/75 text-emerald-300': tone === 'green',
      'border-red-400/85 bg-black/75 text-red-300': tone === 'red',
      'border-sky-400/85 bg-black/75 text-sky-300': tone === 'sky',
      'border-transparent text-white [background:linear-gradient(rgb(10_12_16/0.85),rgb(10_12_16/0.85))_padding-box,linear-gradient(135deg,var(--color-pink-400),var(--color-sky-300),var(--color-blue-400))_border-box]':
        tone === 'trophy',
    }"
  >
    <slot />
  </span>
</template>
