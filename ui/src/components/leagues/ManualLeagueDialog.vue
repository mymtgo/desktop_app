<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { commands, type ManualDeckOption } from '@/bindings'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { useLeagueEdit } from '@/composables/useLeagueEdit'
import { createErrorField, defaultLeagueName, defaultStartedAt } from '@/lib/leagues'

/**
 * Creates a manual league (0.x `LeagueCreateDialog`): a deck, a start and a
 * name that follows the deck's format and the start until it is typed in.
 */
const props = defineProps<{ decks: ManualDeckOption[] }>()

const open = ref(false)
const deckId = ref<number | null>(null)
const startedAt = ref(defaultStartedAt(new Date()))
const name = ref('')
const nameTouched = ref(false)
const errors = ref<{ deck?: string; startedAt?: string; name?: string; other?: string }>({})
const { saving, attempt } = useLeagueEdit()

const selectedDeck = computed(() => props.decks.find((deck) => deck.id === deckId.value) ?? null)
const formatLabel = computed(() => selectedDeck.value?.format ?? '')

watch([deckId, startedAt], () => {
  if (!nameTouched.value) {
    name.value = selectedDeck.value ? defaultLeagueName(formatLabel.value, startedAt.value) : ''
  }
})

function openDialog() {
  deckId.value = null
  startedAt.value = defaultStartedAt(new Date())
  name.value = ''
  nameTouched.value = false
  errors.value = {}
  open.value = true
}

function onNameInput(value: string | number) {
  nameTouched.value = true
  name.value = String(value)
}

function onDeckChange(value: unknown) {
  const parsed = Number(value)
  deckId.value = value === '' || value === null || value === undefined || !Number.isFinite(parsed) ? null : parsed
}

async function submit() {
  const deck = deckId.value
  if (deck === null) return
  errors.value = {}
  const outcome = await attempt(() => commands.createManualLeague(deck, name.value, startedAt.value))
  if (outcome.ok) {
    open.value = false
    return
  }
  errors.value = { [createErrorField(outcome.message) ?? 'other']: outcome.message }
}

defineExpose({ open: openDialog })
</script>

<template>
  <Dialog v-model:open="open">
    <DialogContent class="sm:max-w-md">
      <DialogHeader>
        <DialogTitle>Create manual league</DialogTitle>
        <DialogDescription>Build a league out of imported or unlinked matches.</DialogDescription>
      </DialogHeader>

      <form class="flex flex-col gap-4" @submit.prevent="submit">
        <div class="flex flex-col gap-1.5">
          <Label for="manual-league-deck">Deck</Label>
          <Select :model-value="deckId !== null ? String(deckId) : ''" @update:model-value="onDeckChange">
            <SelectTrigger id="manual-league-deck">
              <SelectValue placeholder="Select a deck" />
            </SelectTrigger>
            <SelectContent>
              <SelectItem v-for="deck in decks" :key="deck.id" :value="String(deck.id)">
                {{ deck.name }} <span class="text-xs text-muted-foreground">({{ deck.format }})</span>
              </SelectItem>
            </SelectContent>
          </Select>
          <p v-if="errors.deck" class="text-xs text-destructive">{{ errors.deck }}</p>
          <p v-if="formatLabel" class="text-xs text-muted-foreground">
            Format: <span class="font-medium text-foreground">{{ formatLabel }}</span>
          </p>
        </div>

        <div class="flex flex-col gap-1.5">
          <Label for="manual-league-started-at">Started at</Label>
          <Input id="manual-league-started-at" v-model="startedAt" type="datetime-local" />
          <p v-if="errors.startedAt" class="text-xs text-destructive">{{ errors.startedAt }}</p>
        </div>

        <div class="flex flex-col gap-1.5">
          <Label for="manual-league-name">Name</Label>
          <Input id="manual-league-name" :model-value="name" @update:model-value="onNameInput" />
          <p v-if="errors.name" class="text-xs text-destructive">{{ errors.name }}</p>
        </div>

        <p v-if="errors.other" class="text-xs text-destructive">{{ errors.other }}</p>

        <DialogFooter>
          <Button type="button" variant="ghost" @click="open = false">Cancel</Button>
          <Button type="submit" :disabled="saving || deckId === null">Create league</Button>
        </DialogFooter>
      </form>
    </DialogContent>
  </Dialog>
</template>
