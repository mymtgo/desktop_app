<script setup lang="ts">
// A deck card's bulk assign controls: the grip that drags the deck (or every
// selected deck) onto an archetype, and the selection checkbox. The card
// places them; a click on them never opens the deck.
import { GripVertical } from 'lucide-vue-next'
import type { HTMLAttributes } from 'vue'
import type { DeckView } from '@/bindings'
import { Checkbox } from '@/components/ui/checkbox'
import { canSetArchetypeByHand, selectionBlockedTitle } from '@/lib/deckIndex'
import { deckDragLabel, dragIds, setDeckDragImage, writeDeckDrag } from '@/lib/deckDrag'

const props = defineProps<{
  deck: DeckView
  selected: boolean
  /** A selection of another format is active: the checkbox is off limits. */
  selectionBlocked: boolean
  /** Every selected deck: a selected card drags them all. */
  selectedIds: number[]
  checkboxClass?: HTMLAttributes['class']
}>()

const emit = defineEmits<{
  'update:selected': [deckId: number, value: boolean]
  /** A drag onto the sidebar started (the deck's format) or ended. */
  dragstart: [format: string]
  dragend: []
}>()

// Only the grip starts a drag (0.x): a draggable card made an ordinary
// click too easy to turn into a drag.
function onGripDragStart(event: DragEvent) {
  if (!event.dataTransfer) return
  const ids = dragIds(props.deck.id, props.selected, props.selectedIds)
  writeDeckDrag(event.dataTransfer, ids)
  setDeckDragImage(event.dataTransfer, deckDragLabel(ids.length, props.deck.name))
  emit('dragstart', props.deck.format)
}
</script>

<template>
  <!-- A commander format deck takes its commander's archetype: no grip to drag it onto one. -->
  <span
    v-if="canSetArchetypeByHand(deck)"
    draggable="true"
    class="cursor-grab text-white/70 transition-colors hover:text-white active:cursor-grabbing"
    title="Drag onto an archetype"
    @dragstart.stop="onGripDragStart"
    @dragend="emit('dragend')"
  >
    <GripVertical class="size-4" />
  </span>
  <Checkbox
    :model-value="selected"
    :aria-label="`Select ${deck.name}`"
    :disabled="selectionBlocked"
    :title="selectionBlocked ? selectionBlockedTitle(deck) : undefined"
    :class="checkboxClass"
    @update:model-value="(value) => emit('update:selected', deck.id, value === true)"
  />
</template>
