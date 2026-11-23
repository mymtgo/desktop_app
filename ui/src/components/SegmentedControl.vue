<script setup lang="ts">
export type SegmentOption = {
  value: string
  label: string
}

/**
 * `inline`: buttons at their own width (the default). `fill`: equal columns
 * across the full width. `grid`: two equal columns, wrapping into rows (the
 * event card's Artwork, 2 x 2).
 */
export type SegmentLayout = 'inline' | 'fill' | 'grid'

withDefaults(
  defineProps<{
    modelValue: string
    options: SegmentOption[]
    disabled?: boolean
    layout?: SegmentLayout
  }>(),
  { layout: 'inline' },
)

const emit = defineEmits<{
  'update:modelValue': [value: string]
}>()

const LAYOUTS: Record<SegmentLayout, string> = {
  inline: 'flex items-center',
  fill: 'grid w-full auto-cols-fr grid-flow-col',
  grid: 'grid w-full grid-cols-2',
}
</script>

<template>
  <div role="group" class="gap-1" :class="LAYOUTS[layout]">
    <button
      v-for="opt in options"
      :key="opt.value"
      type="button"
      :disabled="disabled"
      :aria-pressed="modelValue === opt.value"
      class="relative cursor-pointer rounded border border-black px-4 py-2 text-xs font-medium transition-all disabled:cursor-not-allowed disabled:opacity-40"
      :class="
        modelValue === opt.value
          ? 'nav-item-active'
          : 'bevel border bg-background text-muted-foreground hover:text-foreground hover:brightness-125'
      "
      @click="emit('update:modelValue', opt.value)"
    >
      {{ opt.label }}
    </button>
  </div>
</template>
