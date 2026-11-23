<script setup lang="ts">
import CardImage from '@/components/CardImage.vue'
import type { ListCardView } from '@/bindings'

defineProps<{
  card: ListCardView
  /** A commander: drawn like any card; the Commander heading marks it. */
  commander?: boolean
}>()

// Pixel offset per additional copy, for the stacked look.
const stackOffset = 18

// Card images are about 745x1040px (aspect ratio about 0.717).
const cardWidth = 150
const cardHeight = Math.round(cardWidth / 0.717)
</script>

<template>
  <div
    class="relative"
    :data-commander="commander ? 'true' : undefined"
    :style="{
      width: `${cardWidth}px`,
      height: `${cardHeight + (card.quantity - 1) * stackOffset}px`,
    }"
  >
    <template v-for="i in card.quantity" :key="`qty_${i}`">
      <CardImage
        v-if="card.image"
        :src="card.image"
        :local="card.localImage"
        :alt="card.name"
        class="absolute rounded-[7px] shadow-md"
        :style="{
          width: `${cardWidth}px`,
          top: `${(i - 1) * stackOffset}px`,
          zIndex: i,
        }"
      />
      <!-- No card data or image: a plain card face with the name (decks spec 5). -->
      <div
        v-else
        class="absolute flex items-start rounded-[7px] border border-border bg-muted px-2 py-1.5 text-xs font-medium text-muted-foreground shadow-md"
        :style="{
          width: `${cardWidth}px`,
          height: `${cardHeight}px`,
          top: `${(i - 1) * stackOffset}px`,
          zIndex: i,
        }"
      >
        {{ card.name }}
      </div>
    </template>
  </div>
</template>
