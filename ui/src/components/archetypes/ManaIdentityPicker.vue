<script setup lang="ts">
// 0.x `ManaIdentityPicker.vue`: click a symbol to toggle it.
import { computed } from 'vue'
import ManaColors from '@/components/mana/ManaColors.vue'
import { COLOR_SYMBOLS, colorLetters, toggleColor } from '@/lib/archetypes'

const model = defineModel<string | null>({ default: null })

const selected = computed(() => new Set(colorLetters(model.value)))
</script>

<template>
  <div class="flex gap-2.5">
    <button
      v-for="symbol in COLOR_SYMBOLS"
      :key="symbol.key"
      type="button"
      class="flex cursor-pointer items-center justify-center rounded-full transition-all"
      :class="selected.has(symbol.key) ? 'opacity-100' : 'opacity-35'"
      :title="symbol.label"
      :aria-pressed="selected.has(symbol.key)"
      @click="model = toggleColor(model, symbol.key)"
    >
      <ManaColors :colors="[symbol.key]" />
      <span class="sr-only">{{ symbol.key }}</span>
    </button>
  </div>
</template>
