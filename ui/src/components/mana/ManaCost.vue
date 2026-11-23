<script setup lang="ts">
// A card's mana cost drawn with mana-font (issue #326): the website's
// `ManaCost.vue`. Order and repeats are kept; hybrid, twobrid, Phyrexian,
// X, snow and generics are mana-font's own symbols. Null (a cost never
// stored) and empty (a land) draw nothing.
import { computed } from 'vue'
import { manaCostTokens, manaSymbolClass } from '@/lib/mana'

const props = defineProps<{ cost: string | null }>()
const tokens = computed(() => manaCostTokens(props.cost))
</script>

<template>
  <span v-if="tokens.length" class="inline-flex items-center gap-0.5 align-middle text-[13px]">
    <i
      v-for="(token, index) in tokens"
      :key="`${token}-${index}`"
      :class="manaSymbolClass(token)"
      :aria-label="token"
      role="img"
    />
  </span>
</template>
