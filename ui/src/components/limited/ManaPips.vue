<script setup lang="ts">
import { computed } from 'vue'
import ManaColors from '@/components/mana/ManaColors.vue'
import { colourLetters } from '@/lib/limited'
import { MANA_COLOR_NAMES } from '@/lib/mana'

/** Colours present, in WUBRG order, as mana symbols; colourless when none (0.x `ManaPips`). */
const props = defineProps<{ colors: string; size?: 'sm' | 'md' }>()

const letters = computed(() => colourLetters(props.colors))
const label = computed(() =>
  letters.value.length ? letters.value.map((letter) => MANA_COLOR_NAMES[letter]).join(', ') : 'Colorless',
)
</script>

<template>
  <span class="inline-flex items-center" role="img" :aria-label="label">
    <ManaColors :colors="letters" :size="size === 'md' ? 'md' : 'xs'" />
  </span>
</template>
