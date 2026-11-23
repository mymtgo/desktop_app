<script setup lang="ts">
import { Minus, Plus, X } from 'lucide-vue-next'
import { computed, ref } from 'vue'
import type { ArchetypeCardView, CardSearchView, GameDetailView } from '@/bindings'
import CardSearchInput from '@/components/matches/CardSearchInput.vue'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { useMatchEdit } from '@/composables/useMatchEdit'
import {
  addRevealCard,
  bumpReveal,
  firstError,
  MAX_REVEAL_COPIES,
  revealCopySources,
  revealRowsFromGame,
  revealsPayload,
  type FieldErrors,
  type RevealRow,
} from '@/lib/manualMatch'

/**
 * A manual game's revealed cards (0.x `EditRevealsDialog.vue`): searched by
 * name, copied from another game, or picked from the opponent archetype's
 * list ("Expected for this archetype": maindeck first, sideboard cards
 * dashed; a card already listed is highlighted, a click adds a copy).
 */
const props = withDefaults(
  defineProps<{
    token: string
    game: GameDetailView
    games: GameDetailView[]
    archetypeDecklist?: ArchetypeCardView[] | null
  }>(),
  { archetypeDecklist: null },
)

const open = ref(false)
const rows = ref<RevealRow[]>([])
const errors = ref<FieldErrors>({})
const { saving, saveManualReveals } = useMatchEdit()

const archetypeCards = computed(() => {
  const list = props.archetypeDecklist ?? []
  return [...list.filter((card) => !card.sideboard), ...list.filter((card) => card.sideboard)]
})

function listed(catalogId: number): boolean {
  return rows.value.some((row) => row.catalogId === catalogId)
}

function pick(card: ArchetypeCardView) {
  const index = rows.value.findIndex((row) => row.catalogId === card.catalogId)
  if (index >= 0) {
    rows.value = bumpReveal(rows.value, index, 1)
    return
  }
  rows.value = addRevealCard(rows.value, { catalogId: card.catalogId, name: card.name })
}

const copySources = computed(() => revealCopySources(props.games, props.game.number))
const error = computed(() => firstError(errors.value))

function onSearchSelect(card: CardSearchView) {
  rows.value = addRevealCard(rows.value, { catalogId: card.catalogId, name: card.name })
}

function bump(index: number, delta: number) {
  rows.value = bumpReveal(rows.value, index, delta)
}

function remove(index: number) {
  rows.value = rows.value.filter((_, i) => i !== index)
}

function copyFrom(value: unknown) {
  const source = props.games.find((g) => String(g.number) === String(value))
  if (!source) return
  rows.value = revealRowsFromGame(source)
}

function openDialog() {
  errors.value = {}
  rows.value = revealRowsFromGame(props.game)
  open.value = true
}

async function submit() {
  errors.value = {}
  const outcome = await saveManualReveals(props.token, props.game.number, revealsPayload(rows.value))
  if (outcome.ok) {
    open.value = false
    return
  }
  errors.value = outcome.errors
}

defineExpose({ open: openDialog })
</script>

<template>
  <Dialog v-model:open="open">
    <DialogContent class="flex max-h-[85vh] flex-col gap-4 overflow-hidden sm:max-w-2xl">
      <DialogHeader>
        <DialogTitle>Game {{ game.number }} revealed cards</DialogTitle>
        <DialogDescription>Cards the opponent showed you this game.</DialogDescription>
      </DialogHeader>

      <div v-if="copySources.length" class="flex items-center gap-2 text-xs">
        <span class="text-muted-foreground">Copy from</span>
        <Select @update:model-value="copyFrom">
          <SelectTrigger class="h-8 w-44 text-xs">
            <SelectValue placeholder="Another game" />
          </SelectTrigger>
          <SelectContent>
            <SelectItem v-for="g in copySources" :key="g.number" :value="String(g.number)" class="text-xs">
              Game {{ g.number }} ({{ g.opponentCardsSeen.length }} cards)
            </SelectItem>
          </SelectContent>
        </Select>
      </div>

      <div v-if="archetypeCards.length" class="flex flex-col gap-1.5">
        <h3 class="text-[11px] font-semibold tracking-widest text-muted-foreground uppercase">
          Expected for this archetype
        </h3>
        <div class="flex max-h-32 flex-wrap gap-1 overflow-y-auto">
          <button
            v-for="card in archetypeCards"
            :key="`arch_${card.catalogId}_${card.sideboard}`"
            type="button"
            class="rounded-full border px-2 py-0.5 text-[11px] transition hover:bg-muted"
            :class="[listed(card.catalogId) ? 'border-primary/50 bg-primary/10' : '', card.sideboard ? 'border-dashed' : '']"
            @click="pick(card)"
          >
            {{ card.name }}
          </button>
        </div>
      </div>

      <CardSearchInput @select="onSearchSelect" />

      <div class="flex min-h-0 flex-1 flex-col gap-1 overflow-y-auto">
        <div
          v-for="(row, index) in rows"
          :key="row.catalogId"
          class="flex items-center justify-between gap-2 rounded-md border px-2 py-1 text-xs"
        >
          <span class="truncate">{{ row.name }}</span>
          <div class="inline-flex shrink-0 items-center gap-1">
            <Button
              type="button"
              variant="outline"
              size="icon-sm"
              class="size-6"
              :disabled="row.quantity <= 1"
              @click="bump(index, -1)"
            >
              <Minus :size="10" />
            </Button>
            <span class="w-5 text-center font-mono tabular-nums">{{ row.quantity }}</span>
            <Button
              type="button"
              variant="outline"
              size="icon-sm"
              class="size-6"
              :disabled="row.quantity >= MAX_REVEAL_COPIES"
              @click="bump(index, 1)"
            >
              <Plus :size="10" />
            </Button>
            <Button type="button" variant="ghost" size="icon-sm" class="size-6" @click="remove(index)">
              <X :size="10" />
            </Button>
          </div>
        </div>
        <p v-if="rows.length === 0" class="py-4 text-center text-xs text-muted-foreground italic">No cards yet.</p>
      </div>

      <p v-if="error" class="text-xs text-destructive">{{ error }}</p>

      <DialogFooter class="gap-2">
        <Button type="button" variant="ghost" @click="open = false">Cancel</Button>
        <Button type="button" :disabled="saving" @click="submit()">Save cards</Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>
