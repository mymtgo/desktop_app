<script setup lang="ts">
// Wraps an inline element so hovering it floats the full card image (0.x
// `CardHoverPreview`, reka `HoverCard`). The slot renders bare when there is
// no image to show. `partners` are more cards shown beside it (a Brawl
// commander's partner).
import { HoverCardContent, HoverCardPortal, HoverCardRoot, HoverCardTrigger } from 'reka-ui'
import CardImage from '@/components/CardImage.vue'
import { PREVIEW_CLOSE_DELAY_MS, PREVIEW_OPEN_DELAY_MS, hasCardPreview } from '@/lib/cards'

type PreviewCard = { name: string; image: string | null; localImage?: string | null }

withDefaults(
  defineProps<{
    image: string | null
    localImage?: string | null
    name: string
    side?: 'top' | 'right' | 'bottom' | 'left'
    partners?: PreviewCard[]
  }>(),
  { localImage: null, side: 'right', partners: () => [] },
)
</script>

<template>
  <HoverCardRoot
    v-if="hasCardPreview(image, localImage)"
    :open-delay="PREVIEW_OPEN_DELAY_MS"
    :close-delay="PREVIEW_CLOSE_DELAY_MS"
  >
    <HoverCardTrigger as-child>
      <slot />
    </HoverCardTrigger>
    <HoverCardPortal>
      <HoverCardContent
        :side="side"
        :side-offset="8"
        class="z-50 rounded-lg border border-border bg-popover p-1 shadow-xl"
      >
        <div class="flex gap-1">
          <CardImage :src="image" :local="localImage" :alt="name" class="w-56 rounded-lg" />
          <template v-for="partner in partners" :key="partner.name">
            <CardImage
              v-if="hasCardPreview(partner.image, partner.localImage)"
              :src="partner.image"
              :local="partner.localImage"
              :alt="partner.name"
              class="w-56 rounded-lg"
            />
          </template>
        </div>
      </HoverCardContent>
    </HoverCardPortal>
  </HoverCardRoot>
  <slot v-else />
</template>
