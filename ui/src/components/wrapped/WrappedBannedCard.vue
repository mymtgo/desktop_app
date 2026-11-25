<script setup lang="ts">
import { computed } from 'vue'
import CardImage from '@/components/CardImage.vue'
import { Card } from '@/components/ui/card'
import type { CardView } from '@/lib/wrapped/copy'

/**
 * The year's banned and restricted cards, as the cards themselves, each
 * with the formats it left under it. The list comes from `bannedCards`.
 */
const props = defineProps<{ view: CardView }>()

const cards = computed(() => (props.view.visual?.kind === 'banned' ? props.view.visual.cards : []))

function month(date: string): string {
  return new Date(`${date}T00:00:00Z`).toLocaleString('en-GB', { month: 'long', timeZone: 'UTC' })
}

function image(name: string): string {
  return `https://api.scryfall.com/cards/named?exact=${encodeURIComponent(name)}&format=image&version=normal`
}
</script>

<template>
  <Card class="relative flex h-full flex-col gap-0 overflow-hidden py-0">
    <div class="flex flex-1 flex-col p-8">
      <p class="text-xs font-medium tracking-widest text-muted-foreground uppercase">{{ view.label }}</p>
      <h2 class="wrapped-title mt-2 text-3xl leading-tight font-extrabold">{{ view.title }}</h2>

      <ul v-if="cards.length > 0" class="mt-6 grid grid-cols-5 gap-x-4 gap-y-5">
        <li v-for="card in cards" :key="card.name" class="wrapped-banned-card flex min-w-0 flex-col items-center gap-2">
          <CardImage
            :src="image(card.name)"
            :local="null"
            :custom="null"
            :alt="card.name"
            class="aspect-[488/680] w-full rounded-[4.5%/3.3%] object-cover shadow-[0_10px_24px_-8px_rgba(0,0,0,0.9)]"
          />
          <div class="flex flex-col items-center gap-0.5 text-center">
            <p class="wrapped-banned-when flex items-center gap-1 text-xs font-medium text-foreground/85">
              <!-- A headstone (lucide has none), in lucide's line style. -->
              <svg viewBox="0 0 24 24" class="size-3.5" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
                <path d="M6 20V10a6 6 0 0 1 12 0v10" />
                <path d="M4 20h16" />
                <path d="M12 8v6M9.5 10.5h5" />
              </svg>
              {{ month(card.date) }}
            </p>
            <p class="text-[11px] leading-tight text-muted-foreground">{{ card.formats.join(' · ') }}</p>
          </div>
        </li>
      </ul>
    </div>
  </Card>
</template>
