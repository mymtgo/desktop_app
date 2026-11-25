<script setup lang="ts">
import { computed } from 'vue'
import CardImage from '@/components/CardImage.vue'
import { Card } from '@/components/ui/card'
import type { CardView } from '@/lib/wrapped/copy'
import { setIcon, type SetRelease } from '@/lib/wrapped/sets'

/**
 * The opening card, the year in Magic: each set as a tall panel of its art
 * side by side with its symbol and release month over it, then one
 * sentence of the year's big winners. The facts come from `magicYear`.
 */
const props = defineProps<{ view: CardView }>()

const year = computed(() => (props.view.visual?.kind === 'magicYear' ? props.view.visual.year : null))

function artUrl(release: SetRelease): string {
  return `https://api.scryfall.com/cards/${release.art.set}/${release.art.number}?format=image&version=art_crop`
}

function month(date: string): string {
  return new Date(`${date}T00:00:00Z`).toLocaleString('en-GB', { month: 'short', timeZone: 'UTC' })
}
</script>

<template>
  <Card class="relative flex h-full flex-col gap-0 overflow-hidden py-0">
    <div class="flex flex-1 flex-col p-8">
      <p class="text-xs font-medium tracking-widest text-muted-foreground uppercase">{{ view.label }}</p>
      <h2 class="wrapped-title mt-2 text-3xl leading-tight font-extrabold">{{ view.title }}</h2>

      <!-- Edge to edge, no gaps: each set's art with its symbol and month over the foot. -->
      <div v-if="year && year.sets.length > 0" class="-mx-8 mt-6 flex h-56 shrink-0">
        <figure
          v-for="release in year.sets"
          :key="release.code"
          class="wrapped-set relative min-w-0 flex-1 overflow-hidden"
          :title="`${release.name}: ${release.art.card}, art by ${release.art.artist}`"
        >
          <CardImage :src="artUrl(release)" :local="null" :custom="null" alt="" class="absolute inset-0 size-full object-cover" />
          <div class="absolute inset-x-0 bottom-0 h-1/2 bg-linear-to-t from-black/80 to-transparent" aria-hidden="true" />
          <figcaption class="absolute inset-x-0 bottom-3 flex flex-col items-center gap-1">
            <img
              v-if="setIcon(release.code)"
              :src="setIcon(release.code) ?? undefined"
              :alt="release.name"
              class="wrapped-set-icon size-8 object-contain invert drop-shadow-[0_2px_4px_rgb(0_0_0_/_90%)]"
            />
            <span v-else class="text-xs font-bold tracking-wide uppercase">{{ release.code }}</span>
            <span class="text-[11px] font-medium tracking-wider text-white/90 uppercase [text-shadow:_0_1px_3px_rgb(0_0_0_/_90%)]">{{ month(release.released) }}</span>
          </figcaption>
        </figure>
      </div>

      <p v-if="year && year.winners.length > 0" class="wrapped-winners mt-6 max-w-3xl text-lg leading-snug text-balance text-foreground/80">
        <template v-for="(part, index) in year.winners" :key="index">
          <template v-if="typeof part === 'string'">{{ part }}</template>
          <strong v-else class="font-semibold text-foreground">{{ part.deck }}</strong>
        </template>
      </p>
    </div>
  </Card>
</template>
