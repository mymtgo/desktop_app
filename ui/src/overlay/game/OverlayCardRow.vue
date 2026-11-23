<script setup lang="ts">
import CardImage from '@/components/CardImage.vue'

/**
 * One card row of an overlay list (0.x `OverlayCardRow`): the count cell,
 * a 28 px art crop, the name, then the caller's right-hand cells. `count:
 * null` keeps an empty count cell for alignment; leaving it out drops the
 * cell. The image preview follows the thumbnail only, so scrolling over the
 * rows never pops it up.
 */
defineProps<{
  name: string
  count?: number | null
  artCrop: string | null
  localArtCrop?: string | null
}>()

const emit = defineEmits<{
  previewEnter: [event: MouseEvent]
  previewLeave: []
}>()
</script>

<template>
  <div class="flex items-center text-sm">
    <span v-if="count !== undefined" class="w-8 min-w-0 shrink-0 border-r bg-black/20 px-2 py-1 text-center">
      <span class="font-semibold tabular-nums">{{ count }}</span>
    </span>
    <span
      class="h-7 w-7 shrink-0 cursor-zoom-in overflow-hidden bg-black/20 transition-[filter] hover:brightness-125"
      @mouseenter="emit('previewEnter', $event)"
      @mouseleave="emit('previewLeave')"
    >
      <CardImage :src="artCrop" :local="localArtCrop" :alt="name" class="h-full w-full object-cover" />
    </span>
    <span class="min-w-0 grow truncate px-2">{{ name }}</span>
    <slot />
  </div>
</template>
