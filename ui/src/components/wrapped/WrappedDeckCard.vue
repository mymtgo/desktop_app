<script setup lang="ts">
import { computed } from 'vue'
import CardImage from '@/components/CardImage.vue'
import ManaColors from '@/components/mana/ManaColors.vue'
import { Card } from '@/components/ui/card'
import type { CardView } from '@/lib/wrapped/copy'

/**
 * A deck or an opponent's archetype (deck of the year, best deck, most faced,
 * best matchup, toughest rival): its art under a scrim, its name with its
 * mana symbols, one line of numbers, then its key cards overlapping like a
 * hand set down on the table. The words and numbers come from `cardView`.
 */
const props = defineProps<{ view: CardView }>()

const colors = computed(() => props.view.deck?.colors ?? [])
const cards = computed(() => props.view.deck?.cards ?? [])
</script>

<template>
  <Card class="relative flex h-full flex-col gap-0 overflow-hidden py-0">
    <CardImage
      v-if="view.art"
      :src="view.art.remote"
      :local="view.art.local"
      :custom="view.art.custom"
      alt=""
      class="pointer-events-none absolute inset-0 size-full object-cover object-top"
    />
    <!-- Dark behind the words at the top, lighter towards the cards. -->
    <div class="pointer-events-none absolute inset-0 bg-linear-to-b from-black/85 via-black/60 to-black/35" aria-hidden="true" />

    <div class="relative min-w-0 px-8 pt-7 [text-shadow:_0_1px_4px_rgb(0_0_0_/_80%)]">
      <p class="text-xs font-medium tracking-widest text-muted-foreground uppercase">{{ view.label }}</p>
      <div class="mt-2 flex min-w-0 items-center gap-3">
        <h2 class="wrapped-title line-clamp-2 min-w-0 text-3xl leading-tight font-extrabold">{{ view.title }}</h2>
        <ManaColors v-if="colors.length > 0" :colors="colors" size="lg" class="wrapped-colors shrink-0" />
      </div>
      <p v-for="line in view.lines" :key="line" class="mt-1 text-sm text-pretty text-foreground/85">{{ line }}</p>
      <p class="wrapped-numbers mt-4 flex flex-wrap items-baseline gap-x-3 gap-y-1 text-base text-foreground/70">
        <template v-for="(figure, index) in view.figures" :key="figure.label">
          <span v-if="index > 0" class="text-foreground/30" aria-hidden="true">·</span>
          <span class="wrapped-figure"><b class="text-xl font-extrabold text-foreground tabular-nums">{{ figure.value }}</b> {{ figure.label }}</span>
        </template>
      </p>
      <p v-if="view.badge" class="wrapped-badge mt-2 text-xs text-balance text-foreground/70">{{ view.badge }}</p>
    </div>

    <!-- The deck's key cards, overlapping a little like a hand set down on
         the table. Decorative: the name is the label. -->
    <div v-if="cards.length > 0" aria-hidden="true" class="relative mt-6 flex min-h-0 flex-1 justify-center -space-x-4 px-8 pb-8">
      <CardImage
        v-for="(keyCard, index) in cards"
        :key="keyCard.name"
        :src="keyCard.image.remote"
        :local="keyCard.image.local"
        :custom="keyCard.image.custom"
        alt=""
        class="wrapped-key-card relative aspect-[488/680] h-full max-h-full min-w-0 rounded-[4.5%/3.3%] object-cover shadow-[0_10px_24px_-8px_rgba(0,0,0,0.9)]"
        :style="{ zIndex: String(index) }"
      />
    </div>
    <div v-else class="flex-1" />
  </Card>
</template>
