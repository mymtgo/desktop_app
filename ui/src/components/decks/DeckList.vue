<script setup lang="ts">
import { computed } from 'vue'
import type { ListCardView, TypeGroupView } from '@/bindings'
import DeckListCard from '@/components/decks/DeckListCard.vue'
import { Separator } from '@/components/ui/separator'
import { cardCount, cardRowKey, groupedMaindeck } from '@/lib/decklist'

const props = defineProps<{
  maindeck: TypeGroupView[]
  sideboard: ListCardView[]
  /** A commander format deck's commanders: their own section before the main deck. */
  commanders?: ListCardView[]
}>()

const groups = computed(() => groupedMaindeck(props.maindeck))
</script>

<template>
  <div class="space-y-4">
    <template v-if="commanders?.length">
      <div class="space-y-2" data-section="commander">
        <h3 class="text-sm font-semibold tracking-tight">Commander ({{ cardCount(commanders) }})</h3>
        <ul class="flex flex-wrap gap-2">
          <li v-for="(card, index) in commanders" :key="cardRowKey(card, index)">
            <DeckListCard :card="card" commander />
          </li>
        </ul>
      </div>
      <Separator />
    </template>

    <div class="space-y-2">
      <h3 class="text-sm font-semibold tracking-tight">Maindeck</h3>

      <div class="flex flex-wrap items-start gap-4">
        <section v-for="group in groups" :key="`group_${group.type}`" class="flex flex-col gap-2">
          <h4 class="text-xs font-medium text-muted-foreground">{{ group.type }} ({{ group.count }})</h4>
          <ul class="flex flex-wrap gap-2">
            <li v-for="(card, index) in group.cards" :key="cardRowKey(card, index)">
              <DeckListCard :card="card" />
            </li>
          </ul>
        </section>
      </div>
    </div>

    <Separator v-if="sideboard.length || !commanders?.length" />

    <div v-if="sideboard.length || !commanders?.length" class="space-y-2">
      <h4 class="text-xs font-medium text-muted-foreground">Sideboard ({{ cardCount(sideboard) }})</h4>
      <ul class="flex flex-wrap gap-2">
        <li v-for="(card, index) in sideboard" :key="cardRowKey(card, index)">
          <DeckListCard :card="card" />
        </li>
      </ul>
    </div>
  </div>
</template>
