<script setup lang="ts">
import { computed, ref } from 'vue'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { Select, SelectContent, SelectGroup, SelectItem, SelectLabel, SelectTrigger, SelectValue } from '@/components/ui/select'
import { useMatchEdit } from '@/composables/useMatchEdit'
import { deckVersionGroups, deckVersionLabel, setDeckDescription, settableDeckVersions } from '@/lib/historyImport'
import { useHistoryDeckVersions } from '@/lib/queries'

/**
 * "Set deck" (issue #332): the deck version one match (a row's menu,
 * `openForMatches([token])`) or the selected rows were played with. The
 * select is the Import page's deck version picker (active decks, then
 * deleted ones), limited to the current account's constructed decks. Saving replaces each match's deck and closes the dialog.
 */
const emit = defineEmits<{ deckSet: [] }>()

const open = ref(false)
const tokens = ref<string[]>([])
const chosen = ref<number | null>(null)

const { data: versions, isPending } = useHistoryDeckVersions(open)
const { saving, setDeck } = useMatchEdit()

const groups = computed(() => deckVersionGroups(settableDeckVersions(versions.value ?? [])))
const description = computed(() => setDeckDescription(tokens.value.length))

function openForMatches(matchTokens: string[]) {
  tokens.value = [...matchTokens]
  chosen.value = null
  open.value = true
}

async function save() {
  if (chosen.value === null) return
  if (await setDeck(tokens.value, chosen.value)) {
    open.value = false
    emit('deckSet')
  }
}

defineExpose({ openForMatches })
</script>

<template>
  <Dialog v-model:open="open">
    <DialogContent class="sm:max-w-md">
      <DialogHeader>
        <DialogTitle>Set deck</DialogTitle>
        <DialogDescription>{{ description }}</DialogDescription>
      </DialogHeader>
      <p v-if="!isPending && groups.length === 0" class="text-sm text-muted-foreground">No decks found.</p>
      <Select
        v-else
        :model-value="chosen ?? undefined"
        data-testid="set-deck-select"
        :disabled="saving || isPending"
        @update:model-value="(value) => (chosen = typeof value === 'number' ? value : null)"
      >
        <SelectTrigger class="w-full" aria-label="Deck version">
          <SelectValue placeholder="Choose a deck version..." />
        </SelectTrigger>
        <SelectContent>
          <SelectGroup v-for="group in groups" :key="group.label">
            <SelectLabel>{{ group.label }}</SelectLabel>
            <SelectItem v-for="version in group.versions" :key="version.id" :value="version.id">
              {{ deckVersionLabel(version) }}
            </SelectItem>
          </SelectGroup>
        </SelectContent>
      </Select>
      <DialogFooter>
        <Button variant="outline" size="sm" @click="open = false">Cancel</Button>
        <Button size="sm" :disabled="chosen === null || saving" data-testid="set-deck-save" @click="save">
          Set deck
        </Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>
