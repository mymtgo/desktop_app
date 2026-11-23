<script setup lang="ts">
import { computed } from 'vue'
import type { DeckView } from '@/bindings'
import ArchetypePicker from '@/components/matches/ArchetypePicker.vue'
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { canSetArchetypeByHand } from '@/lib/deckIndex'
import { useArchetypeOptions } from '@/composables/useOpponentArchetype'

/** One deck's "Set Archetype" from its card's menu: the picker offers the deck format's archetypes. */
const props = defineProps<{
  /** The deck being set; null closes the dialog. */
  deck: DeckView | null
  busy: boolean
}>()

const emit = defineEmits<{
  select: [archetypeId: number]
  close: []
}>()

// A commander format deck's archetype is its commander's: the dialog never opens for it.
const open = computed(() => props.deck !== null && canSetArchetypeByHand(props.deck))
const { data: options } = useArchetypeOptions(
  computed(() => props.deck?.format ?? null),
  open,
)

function onOpenChange(value: boolean) {
  if (!value) emit('close')
}
</script>

<template>
  <Dialog :open="open" @update:open="onOpenChange">
    <DialogContent class="flex max-h-[80vh] flex-col">
      <DialogHeader>
        <DialogTitle>Set Archetype</DialogTitle>
        <DialogDescription>Choose the archetype for {{ deck?.name }}.</DialogDescription>
      </DialogHeader>
      <ArchetypePicker
        v-if="deck"
        :archetypes="options"
        :format="deck.format"
        :current-id="deck.archetype?.id ?? null"
        :disabled="busy"
        class="min-h-0 flex-1"
        list-class="max-h-none min-h-0 flex-1 border-0 p-0"
        autofocus
        @select="(id: number) => emit('select', id)"
      />
    </DialogContent>
  </Dialog>
</template>
