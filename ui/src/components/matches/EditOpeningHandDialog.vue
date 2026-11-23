<script setup lang="ts">
import { Minus, Plus, X } from 'lucide-vue-next'
import { computed, ref } from 'vue'
import type { DeckCardOptionView, GameDetailView, ManualHandInput } from '@/bindings'
import CardTile from '@/components/matches/CardTile.vue'
import CardTilePill from '@/components/matches/CardTilePill.vue'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { useMatchEdit } from '@/composables/useMatchEdit'
import {
  CLEARED_HAND,
  finalHand,
  firstError,
  HAND_SIZE,
  handHint,
  handPayload,
  handStateFromGame,
  MAX_MULLIGANS,
  removeFromHand,
  setMulligans as withMulligans,
  toggleBottomed as withBottomedToggled,
  type FieldErrors,
  type HandState,
} from '@/lib/manualMatch'

/**
 * A manual game's opening hand (0.x `EditKeptHandDialog.vue`): the seven
 * cards drawn, the cards bottomed after a mulligan, and the hands put back.
 * The archetype quick picks wait for #9.
 */
const props = defineProps<{
  token: string
  game: GameDetailView
  /** This game's maindeck: the registered mains with its sideboard changes applied. */
  mains: DeckCardOptionView[]
}>()

const open = ref(false)
const state = ref<HandState>({ mulligans: 0, hands: [[]], bottomed: new Set() })
/** Which hand is being edited: 0..mulligans-1 are put-back hands, `mulligans` is the final hand. */
const stage = ref(0)
const errors = ref<FieldErrors>({})
const { saving, saveManualHand } = useMatchEdit()

const mulligans = computed(() => state.value.mulligans)
const editingFinal = computed(() => stage.value === mulligans.value)
const currentHand = computed(() => state.value.hands[stage.value] ?? [])
const currentFull = computed(() => currentHand.value.length >= HAND_SIZE)
const bottomedCount = computed(() => state.value.bottomed.size)
const hint = computed(() => handHint(state.value))
const canSave = computed(() => hint.value === null)
const error = computed(() => firstError(errors.value))

const stages = computed(() =>
  Array.from({ length: mulligans.value + 1 }, (_, i) => ({
    index: i,
    label: i === mulligans.value ? 'Final hand' : `Put back ${i + 1}`,
    count: state.value.hands[i]?.length ?? 0,
  })),
)

const inCurrentHand = computed(() => {
  const counts = new Map<number, number>()
  for (const id of currentHand.value) counts.set(id, (counts.get(id) ?? 0) + 1)
  return counts
})

const currentCards = computed(() =>
  currentHand.value
    .map((id) => props.mains.find((c) => c.catalogId === id))
    .filter((c): c is DeckCardOptionView => !!c),
)

function remaining(card: DeckCardOptionView): number {
  return card.quantity - (inCurrentHand.value.get(card.catalogId) ?? 0)
}

function add(card: DeckCardOptionView) {
  if (currentFull.value || remaining(card) <= 0) return
  state.value = {
    ...state.value,
    hands: state.value.hands.map((hand, i) => (i === stage.value ? [...hand, card.catalogId] : hand)),
  }
}

function removeAt(index: number) {
  state.value = removeFromHand(state.value, stage.value, index)
}

function toggleBottomed(index: number) {
  if (!editingFinal.value) return
  state.value = withBottomedToggled(state.value, index)
}

function isBottomed(index: number): boolean {
  return editingFinal.value && state.value.bottomed.has(index)
}

function clearCurrent() {
  state.value = {
    ...state.value,
    hands: state.value.hands.map((hand, i) => (i === stage.value ? [] : hand)),
    bottomed: editingFinal.value ? new Set() : state.value.bottomed,
  }
}

function setMulligans(value: number) {
  state.value = withMulligans(state.value, value)
  if (stage.value > state.value.mulligans) stage.value = state.value.mulligans
}

function openDialog() {
  errors.value = {}
  state.value = handStateFromGame(props.game)
  stage.value = state.value.mulligans
  open.value = true
}

async function submit(payload?: ManualHandInput) {
  errors.value = {}
  const outcome = await saveManualHand(props.token, props.game.number, payload ?? handPayload(state.value))
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
    <DialogContent class="flex max-h-[85vh] flex-col gap-4 overflow-hidden sm:max-w-3xl">
      <DialogHeader>
        <DialogTitle>Game {{ game.number }} opening hand</DialogTitle>
        <DialogDescription>
          Seven cards drawn. After a mulligan, pick the seven you saw and mark what you bottomed. Put-back hands are
          optional.
        </DialogDescription>
      </DialogHeader>

      <div class="flex items-center gap-3 text-sm">
        <span class="text-muted-foreground">Mulligans</span>
        <div class="inline-flex items-center gap-1">
          <Button
            type="button"
            variant="outline"
            size="icon-sm"
            class="size-7"
            :disabled="mulligans === 0"
            @click="setMulligans(mulligans - 1)"
          >
            <Minus :size="12" />
          </Button>
          <span class="w-6 text-center font-mono tabular-nums">{{ mulligans }}</span>
          <Button
            type="button"
            variant="outline"
            size="icon-sm"
            class="size-7"
            :disabled="mulligans === MAX_MULLIGANS"
            @click="setMulligans(mulligans + 1)"
          >
            <Plus :size="12" />
          </Button>
        </div>
        <span class="ml-auto font-mono text-xs text-muted-foreground tabular-nums">
          {{ currentHand.length }} / {{ HAND_SIZE }}
          <template v-if="editingFinal && mulligans > 0">, {{ bottomedCount }} / {{ mulligans }} bottomed</template>
        </span>
      </div>

      <div v-if="mulligans > 0" class="flex flex-wrap gap-1">
        <button
          v-for="s in stages"
          :key="s.index"
          type="button"
          class="rounded-md border px-2 py-1 font-mono text-[11px] transition"
          :class="s.index === stage ? 'border-foreground/40 bg-muted' : 'hover:bg-muted/60'"
          @click="stage = s.index"
        >
          {{ s.label }}
          <span class="text-muted-foreground">{{ s.count }}/{{ HAND_SIZE }}</span>
        </button>
      </div>

      <div class="flex min-h-24 flex-wrap gap-1.5 rounded-md border border-dashed p-2">
        <div
          v-for="(card, i) in currentCards"
          :key="`hand_${stage}_${i}`"
          class="group relative flex flex-col items-center gap-1"
        >
          <button
            type="button"
            class="aspect-[63/88] w-16 overflow-hidden rounded-md transition"
            :class="isBottomed(i) ? 'opacity-60' : ''"
            :title="editingFinal && mulligans > 0 ? (isBottomed(i) ? 'Unmark bottomed' : 'Mark bottomed') : 'Remove'"
            @click="editingFinal && mulligans > 0 ? toggleBottomed(i) : removeAt(i)"
          >
            <CardTile :name="card.name" :image="card.image" :local="card.localImage" />
          </button>
          <CardTilePill v-if="isBottomed(i)" tone="bottomed">Bottom</CardTilePill>
          <button
            v-if="editingFinal && mulligans > 0"
            type="button"
            class="absolute -top-1 -right-1 hidden size-4 items-center justify-center rounded-full bg-background text-muted-foreground shadow ring-1 ring-border group-hover:flex hover:text-destructive"
            :title="`Remove ${card.name}`"
            @click.stop="removeAt(i)"
          >
            <X :size="10" />
          </button>
        </div>
        <p v-if="currentCards.length === 0" class="w-full self-center text-center text-xs text-muted-foreground italic">
          {{
            editingFinal
              ? 'Click cards below to build the hand.'
              : 'Click cards below, or leave empty if you do not remember this hand.'
          }}
        </p>
      </div>

      <div class="flex items-center justify-between text-xs text-muted-foreground">
        <span v-if="editingFinal && mulligans > 0">Click a card in the hand to mark it bottomed.</span>
        <span v-else-if="editingFinal">Click a card in the hand to remove it.</span>
        <span v-else>Hand you put back, then reshuffled.</span>
        <Button
          v-if="currentHand.length"
          type="button"
          variant="ghost"
          size="sm"
          class="h-6 px-1.5 text-[11px]"
          @click="clearCurrent"
        >
          Empty this hand
        </Button>
      </div>

      <div class="grid grid-cols-2 gap-1 overflow-y-auto sm:grid-cols-3">
        <button
          v-for="card in mains"
          :key="card.catalogId"
          type="button"
          class="flex items-center justify-between gap-2 rounded-md border px-2 py-1 text-left text-xs transition hover:bg-muted disabled:cursor-not-allowed disabled:opacity-40"
          :disabled="currentFull || remaining(card) <= 0"
          @click="add(card)"
        >
          <span class="truncate">{{ card.name }}</span>
          <span class="shrink-0 font-mono text-[10px] text-muted-foreground tabular-nums">
            {{ remaining(card) }}/{{ card.quantity }}
          </span>
        </button>
      </div>

      <p v-if="error" class="text-xs text-destructive">{{ error }}</p>
      <p v-else-if="hint" class="text-xs text-muted-foreground">{{ hint }}</p>

      <DialogFooter class="gap-2">
        <Button
          v-if="game.openingHand.length"
          type="button"
          variant="ghost"
          :disabled="saving"
          @click="submit(CLEARED_HAND)"
        >
          Clear hand
        </Button>
        <Button type="button" variant="ghost" @click="open = false">Cancel</Button>
        <Button type="button" :disabled="!canSave || saving" @click="submit()">Save hand</Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>
