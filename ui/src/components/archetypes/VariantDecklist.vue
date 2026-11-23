<script setup lang="ts">
// 0.x `VariantDecklist.vue` (and the edit page's variant card): the list,
// "Move to archetype", "Download .dek" and "Remove". The archetype page
// shows it as text with its version picker (issue #326).
import { Download, Loader2, MoveRight, Trash2 } from 'lucide-vue-next'
import { computed, ref } from 'vue'
import { commands, type ExportSource, type ListCardView } from '@/bindings'
import ReassignVariantDialog from '@/components/archetypes/ReassignVariantDialog.vue'
import DeckList from '@/components/decks/DeckList.vue'
import ArchetypeDecklist from '@/components/metagame/ArchetypeDecklist.vue'
import { Button } from '@/components/ui/button'
import { Card } from '@/components/ui/card'
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { useToast } from '@/composables/useToast'
import { groupListCards } from '@/lib/archetypes'
import { errorMessage, unwrap } from '@/lib/commands'
import { dekExportFailedToast, dekExportToast } from '@/lib/decklist'

const props = defineProps<{
  archetypeId: number
  archetypeName: string
  list: number
  variantLabel: string
  cards: ListCardView[]
  /** Manual archetypes only: reassign and remove. */
  editable: boolean
  canRemove: boolean
  /** A live decklist (the archetype detail): exported from its cards, not a stored list. */
  live?: boolean
  /** `text`: the archetype page's website-style list (issue #326); else card images. */
  layout?: 'images' | 'text'
}>()

const emit = defineEmits<{ changed: [] }>()

const { add } = useToast()
const exporting = ref(false)
const reassignOpen = ref(false)
const confirmRemove = ref(false)
const removing = ref(false)
const grouped = computed(() => groupListCards(props.cards))

async function downloadDek() {
  if (exporting.value) return
  exporting.value = true
  try {
    const source: ExportSource = props.live
      ? {
          kind: 'decklist',
          cards: props.cards.map((card) => ({
            catalogId: card.catalogId,
            quantity: card.quantity,
            sideboard: card.sideboard,
            name: card.name,
          })),
        }
      : { kind: 'list', list: props.list }
    const toast = dekExportToast(unwrap(await commands.exportArchetypeDek(props.archetypeId, source)))
    if (toast) add(toast)
  } catch (failure) {
    add(dekExportFailedToast(failure))
  } finally {
    exporting.value = false
  }
}

async function remove() {
  removing.value = true
  try {
    unwrap(await commands.removeVariant(props.archetypeId, props.list))
    confirmRemove.value = false
    emit('changed')
  } catch (failure) {
    add({ type: 'error', title: 'Could not remove the variant', message: errorMessage(failure) })
  } finally {
    removing.value = false
  }
}
</script>

<template>
  <Card class="gap-3 p-4">
    <div class="flex items-center justify-between gap-2">
      <!-- The archetype page puts its version picker here. -->
      <slot name="picker">
        <h3 class="text-sm font-semibold">{{ variantLabel }}</h3>
      </slot>
      <div class="flex items-center gap-2">
        <Button v-if="editable" variant="outline" size="sm" @click="reassignOpen = true">
          <MoveRight class="mr-1.5 size-3.5" />
          Move to archetype
        </Button>
        <Button variant="outline" size="sm" :disabled="exporting" @click="downloadDek">
          <Loader2 v-if="exporting" class="mr-1.5 size-3.5 animate-spin" />
          <Download v-else class="mr-1.5 size-3.5" />
          {{ exporting ? 'Saving…' : live ? 'Export .dek' : 'Download .dek' }}
        </Button>
        <Button
          v-if="editable"
          variant="outline"
          size="sm"
          :disabled="!canRemove"
          :title="canRemove ? undefined : 'Cannot delete the last variant.'"
          @click="confirmRemove = true"
        >
          <Trash2 class="size-3.5" />
          <span class="sr-only">Remove</span>
        </Button>
      </div>
    </div>

    <ArchetypeDecklist v-if="layout === 'text'" :cards="cards" />
    <DeckList v-else :maindeck="grouped.maindeck" :sideboard="grouped.sideboard" />

    <ReassignVariantDialog
      v-model:open="reassignOpen"
      :archetype-id="archetypeId"
      :archetype-name="archetypeName"
      :list="list"
      :variant-label="variantLabel"
      @reassigned="emit('changed')"
    />

    <Dialog v-model:open="confirmRemove">
      <DialogContent class="max-w-sm">
        <DialogHeader>
          <DialogTitle>Remove Variant</DialogTitle>
          <DialogDescription>Matches stay linked to the archetype; only this card list is removed.</DialogDescription>
        </DialogHeader>
        <DialogFooter>
          <Button variant="outline" size="sm" @click="confirmRemove = false">Cancel</Button>
          <Button variant="destructive" size="sm" :disabled="removing" @click="remove">Remove</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  </Card>
</template>
