<script setup lang="ts">
// One list laid out like the mymtgo.com website's decklist (issue #326;
// the API `DecklistPanel.vue` and `DecklistGroup.vue`): the maindeck by type
// pile on the left, lands then the sideboard on the right, and the total
// underneath. Hovering a card name floats its image. v1 has no card prices,
// so the website's price column is left out.
import { computed } from 'vue'
import type { ListCardView } from '@/bindings'
import CardHoverPreview from '@/components/cards/CardHoverPreview.vue'
import ManaCost from '@/components/mana/ManaCost.vue'
import { archetypeDecklist } from '@/lib/archetypeDecklist'
import { cardRowKey } from '@/lib/decklist'

const props = defineProps<{ cards: ListCardView[] }>()
const list = computed(() => archetypeDecklist(props.cards))
const standalone = computed(() => [list.value.lands, list.value.sideboard].filter((pile) => pile.cards.length > 0))
</script>

<template>
  <div class="flex flex-col">
    <div class="grid gap-x-6 gap-y-4 sm:grid-cols-2">
      <div class="min-w-0">
        <h3 class="mb-2 text-xs font-bold tracking-wide text-muted-foreground uppercase">
          Maindeck ({{ list.maindeckCount }})
        </h3>
        <div v-for="pile in list.spells" :key="pile.name" class="mb-3 last:mb-0">
          <h4 class="mb-1 text-xs font-semibold text-muted-foreground">{{ pile.name }} ({{ pile.count }})</h4>
          <ul>
            <li v-for="(card, index) in pile.cards" :key="cardRowKey(card, index)" class="flex items-center gap-2 py-0.5 text-sm">
              <span class="w-5 shrink-0 text-right text-muted-foreground tabular-nums">{{ card.quantity }}</span>
              <CardHoverPreview :image="card.image" :local-image="card.localImage" :name="card.name">
                <span class="min-w-0 cursor-default truncate">{{ card.name }}</span>
              </CardHoverPreview>
              <span class="ml-auto flex shrink-0 items-center"><ManaCost :cost="card.manaCost" /></span>
            </li>
          </ul>
        </div>
      </div>

      <div class="min-w-0">
        <div v-for="pile in standalone" :key="pile.name" class="mb-4 last:mb-0">
          <h3 class="mb-2 text-xs font-bold tracking-wide text-muted-foreground uppercase">
            {{ pile.name }} ({{ pile.count }})
          </h3>
          <ul>
            <li v-for="(card, index) in pile.cards" :key="cardRowKey(card, index)" class="flex items-center gap-2 py-0.5 text-sm">
              <span class="w-5 shrink-0 text-right text-muted-foreground tabular-nums">{{ card.quantity }}</span>
              <CardHoverPreview :image="card.image" :local-image="card.localImage" :name="card.name" side="left">
                <span class="min-w-0 cursor-default truncate">{{ card.name }}</span>
              </CardHoverPreview>
              <span class="ml-auto flex shrink-0 items-center"><ManaCost :cost="card.manaCost" /></span>
            </li>
          </ul>
        </div>
      </div>
    </div>

    <p class="mt-4 border-t pt-3 text-xs font-semibold text-muted-foreground">{{ list.total }} cards total</p>
  </div>
</template>
