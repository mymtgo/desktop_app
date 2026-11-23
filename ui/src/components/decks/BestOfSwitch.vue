<script setup lang="ts">
import type { BestOfView } from '@/bindings'

defineProps<{ modelValue: BestOfView }>()
defineEmits<{ 'update:modelValue': [kind: BestOfView] }>()

const options: { value: BestOfView; label: string }[] = [
  { value: 'bo3', label: 'Bo3' },
  { value: 'bo1', label: 'Bo1' },
]
</script>

<template>
  <div class="flex items-center rounded-md border border-black/70 bg-background p-0.5 text-xs font-medium">
    <button
      v-for="option in options"
      :key="option.value"
      type="button"
      :aria-pressed="modelValue === option.value"
      class="rounded px-2 py-0.5 transition-colors hover:cursor-pointer"
      :class="modelValue === option.value ? 'bg-muted text-foreground' : 'text-muted-foreground hover:text-foreground'"
      @click="$emit('update:modelValue', option.value)"
    >
      {{ option.label }}
    </button>
  </div>
</template>
