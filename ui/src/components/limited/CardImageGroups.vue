<script setup lang="ts">
import DeckListCard from '@/components/decks/DeckListCard.vue'
import { cardCount } from '@/lib/decklist'
import type { CardImageGroup } from '@/lib/limitedPages'

/** Card images in labelled groups, the same stacked cards the decklist shows (0.x `CardImageGroups`). */
defineProps<{ groups: CardImageGroup[]; emptyText?: string }>()
</script>

<template>
  <div class="flex flex-wrap items-start gap-4">
    <section v-for="group in groups" :key="group.key" class="flex flex-col gap-2">
      <h4 class="text-xs font-medium text-muted-foreground">{{ group.label }} ({{ cardCount(group.cards) }})</h4>
      <ul class="flex flex-wrap gap-2">
        <li v-for="card in group.cards" :key="`${group.key}_${card.catalogId}`">
          <DeckListCard :card="card" />
        </li>
      </ul>
    </section>
    <p v-if="groups.length === 0" class="text-sm text-muted-foreground">{{ emptyText ?? 'Empty.' }}</p>
  </div>
</template>
