<script setup lang="ts">
import CardImage from '@/components/CardImage.vue'
import { ImageOff } from 'lucide-vue-next'
import type { LimitedCardView } from '@/bindings'

/** A card image, or its name (`#id` without card data) on a muted tile (0.x `CardThumb`). */
withDefaults(
  defineProps<{
    card: LimitedCardView
    width?: number
    highlighted?: boolean
    dimmed?: boolean
    /** Fill the parent's width instead of a fixed pixel width. */
    fluid?: boolean
  }>(),
  { width: 110, highlighted: false, dimmed: false, fluid: false },
)
</script>

<template>
  <div
    class="relative overflow-hidden rounded-md border bg-muted transition-opacity"
    :class="[highlighted ? 'border-primary ring-2 ring-primary/60' : 'border-black/60', dimmed ? 'opacity-40' : '']"
    :style="{ width: fluid ? '100%' : `${width}px`, aspectRatio: '5 / 7' }"
  >
    <CardImage v-if="card.image" :src="card.image" :local="card.localImage" :alt="card.name" loading="lazy" class="h-full w-full object-cover" />
    <div v-else class="flex h-full w-full flex-col items-center justify-center gap-1 p-1 text-center">
      <ImageOff class="size-4 text-muted-foreground/60" />
      <span class="text-[10px] leading-tight text-muted-foreground">{{ card.name }}</span>
    </div>
    <slot />
  </div>
</template>
