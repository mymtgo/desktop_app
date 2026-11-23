<script setup lang="ts">
// The deck Settings page (0.x `pages/decks/Settings.vue`): name, the
// archetype (#9), colour identity, cover art and the danger zone. Cloud
// sync (#7) waits for its sub-project.
import { RotateCcw } from 'lucide-vue-next'
import { computed, ref, watch } from 'vue'
import { commands } from '@/bindings'
import ArchiveDeckCard from '@/components/decks/ArchiveDeckCard.vue'
import ColorIdentityPicker from '@/components/decks/ColorIdentityPicker.vue'
import DeckArchetypeCard from '@/components/decks/DeckArchetypeCard.vue'
import CoverArtPicker from '@/components/decks/CoverArtPicker.vue'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import { Input } from '@/components/ui/input'
import { Spinner } from '@/components/ui/spinner'
import { useDeckEdit } from '@/composables/useDeckEdit'
import { useDeckBestOfContext } from '@/composables/useDeckBestOf'
import { useCurrentDeckHeader } from '@/composables/useDeckPage'
import { ARCHIVED_TITLE, DECK_NAME_INPUT_MAX, renameTarget } from '@/lib/deckSettings'

const { data: header } = useCurrentDeckHeader(useDeckBestOfContext().bestOf)
const deck = computed(() => header.value?.deck ?? null)
const archived = computed(() => deck.value?.archived ?? false)

const { saving: savingName, run: runRename } = useDeckEdit()
const nameDraft = ref(deck.value?.name ?? '')

watch(
  () => deck.value?.name,
  (name) => {
    nameDraft.value = name ?? ''
  },
)

const nameToSave = computed(() => (deck.value ? renameTarget(nameDraft.value, deck.value.name) : null))

async function saveName() {
  const current = deck.value
  const next = nameToSave.value
  if (!current || next === null || savingName.value) {
    return
  }
  const done = await runRename('Could not rename the deck', () => commands.renameDeck(current.id, next))
  if (!done) {
    nameDraft.value = current.name
  }
}

async function revertName() {
  const current = deck.value
  const original = current?.originalName
  if (!current || !original || savingName.value) {
    return
  }
  await runRename('Could not revert the deck name', () => commands.renameDeck(current.id, original))
}
</script>

<template>
  <div v-if="deck" class="p-3 lg:p-4">
    <div class="flex max-w-2xl flex-col gap-4">
      <Card>
        <CardHeader>
          <CardTitle>Name</CardTitle>
          <CardDescription>Rename this deck. The original MTGO name is kept so you can revert at any time.</CardDescription>
        </CardHeader>
        <CardContent class="flex flex-col gap-3">
          <div class="flex flex-col gap-2 sm:flex-row">
            <Input
              :model-value="nameDraft"
              :maxlength="DECK_NAME_INPUT_MAX"
              placeholder="Deck name"
              class="sm:flex-1"
              :disabled="archived || savingName"
              :title="archived ? ARCHIVED_TITLE : undefined"
              @update:model-value="(value) => (nameDraft = String(value))"
              @keydown.enter.prevent="saveName"
            />
            <Button
              :disabled="nameToSave === null || savingName || archived"
              :title="archived ? ARCHIVED_TITLE : undefined"
              @click="saveName"
            >
              <Spinner v-if="savingName" class="mr-2 size-4" />
              Save
            </Button>
          </div>
          <div
            v-if="deck.originalName"
            class="flex items-center justify-between rounded-md border border-dashed border-border bg-muted/30 px-3 py-2 text-sm"
          >
            <span class="text-muted-foreground">
              Original MTGO name: <span class="font-medium text-foreground">{{ deck.originalName }}</span>
            </span>
            <Button
              variant="ghost"
              size="sm"
              :disabled="savingName || archived"
              :title="archived ? ARCHIVED_TITLE : undefined"
              @click="revertName"
            >
              <RotateCcw class="mr-1 size-3" />
              Revert
            </Button>
          </div>
        </CardContent>
      </Card>

      <DeckArchetypeCard
        :key="`archetype-${deck.id}`"
        :deck-id="deck.id"
        :format="deck.format"
        :commander-format="deck.commanderFormat"
        :archetype="deck.archetype"
        :archived="archived"
      />
      <ColorIdentityPicker :key="`identity-${deck.id}`" :deck-id="deck.id" :colors="deck.colors" :archived="archived" />
      <CoverArtPicker :key="`cover-${deck.id}`" :deck-id="deck.id" :archived="archived" />
      <ArchiveDeckCard :key="`archive-${deck.id}`" :deck-id="deck.id" :archived="archived" />
    </div>
  </div>
</template>
