<script setup lang="ts" generic="T extends string">
defineProps<{
  options: { value: T; label: string }[]
  label?: string
}>()

const model = defineModel<T>({ required: true })
</script>

<template>
  <div role="group" :aria-label="label" class="inline-flex items-center rounded-md border border-border bg-muted/40 p-0.5 text-xs">
    <button
      v-for="option in options"
      :key="option.value"
      type="button"
      class="rounded px-2 py-0.5 transition-colors"
      :class="
        model === option.value
          ? 'bg-background text-foreground shadow-sm'
          : 'text-muted-foreground hover:text-foreground'
      "
      :aria-pressed="model === option.value"
      @click="model = option.value"
    >
      {{ option.label }}
    </button>
  </div>
</template>
