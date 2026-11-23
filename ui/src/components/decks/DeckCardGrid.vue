<script setup lang="ts">
import { computed } from 'vue'
import type { DeckCardSize, DeckView } from '@/bindings'
import DeckCard from '@/components/decks/DeckCard.vue'
import DeckCardCompact from '@/components/decks/DeckCardCompact.vue'
import DeckContextMenu from '@/components/decks/DeckContextMenu.vue'
import { canSelectDeck, dimsInSelection } from '@/lib/deckIndex'

const props = withDefaults(
  defineProps<{
    decks: DeckView[]
    cardSize?: DeckCardSize
    /** The time "last played" counts back from. */
    now: number
    /** Cards show checkboxes for the bulk archetype assign. */
    selectable?: boolean
    selectedIds?: number[]
    /** The selection's shared format: decks of another one cannot join. */
    selectionFormat?: string | null
  }>(),
  { cardSize: 'large', selectable: false, selectedIds: () => [], selectionFormat: null },
)

const emit = defineEmits<{
  'update:selected': [deckId: number, value: boolean]
  dragstart: [format: string]
  dragend: []
  setArchetype: [deck: DeckView]
  clearArchetype: [deck: DeckView]
}>()

const cardComponent = computed(() => (props.cardSize === 'compact' ? DeckCardCompact : DeckCard))
</script>

<template>
  <div class="grid grid-cols-1 gap-3 sm:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4">
    <DeckContextMenu
      v-for="deck in decks"
      :key="deck.id"
      :has-archetype="deck.archetype !== null"
      :locked="deck.commanderFormat"
      @set-archetype="emit('setArchetype', deck)"
      @clear-archetype="emit('clearArchetype', deck)"
    >
      <component
        :is="cardComponent"
        :deck="deck"
        :now="now"
        :selectable="selectable"
        :selected="selectedIds.includes(deck.id)"
        :selection-blocked="!canSelectDeck(deck, selectionFormat)"
        :dimmed="dimsInSelection(deck, selectionFormat)"
        :selected-ids="selectedIds"
        @update:selected="(id: number, value: boolean) => emit('update:selected', id, value)"
        @dragstart="(format: string) => emit('dragstart', format)"
        @dragend="emit('dragend')"
      />
    </DeckContextMenu>
  </div>
</template>
