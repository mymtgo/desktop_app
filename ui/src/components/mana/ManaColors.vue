<script setup lang="ts">
// A colour identity as mana symbols (mana-font, issue #326): the colours in
// WUBRG order, colourless when asked for or when there are none.
import { computed } from 'vue'
import { shownManaSymbols } from '@/lib/decks'
import { MANA_FONT_SIZES, type ManaSize, manaSymbolClass } from '@/lib/mana'

const props = withDefaults(
  defineProps<{
    /** Colour identity letters, e.g. `["U", "R"]`; empty shows colourless. */
    colors: string[]
    /** The disc size: xs 12px, sm 14px, md 16px, lg 18px. */
    size?: ManaSize
  }>(),
  { size: 'md' },
)

const shown = computed(() => shownManaSymbols(props.colors))
</script>

<template>
  <!-- The symbols are a font: a text shadow on a parent (cover art cards)
       would shadow them too, so it stops here. -->
  <div class="flex items-center gap-1 [text-shadow:none]" :class="MANA_FONT_SIZES[size]">
    <i v-for="symbol in shown" :key="symbol" :class="manaSymbolClass(symbol)" :aria-label="symbol" role="img" />
  </div>
</template>
