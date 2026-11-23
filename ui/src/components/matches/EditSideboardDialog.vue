<script setup lang="ts">
import { Minus, Plus } from 'lucide-vue-next'
import { computed, ref } from 'vue'
import type { DeckCardOptionView, GameDetailView } from '@/bindings'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { useMatchEdit } from '@/composables/useMatchEdit'
import {
  bumpSideboard,
  firstError,
  sideboardPayload,
  sideboardStateFromGame,
  sideboardTotal,
  type FieldErrors,
} from '@/lib/manualMatch'

/**
 * A manual game's sideboard changes (0.x `EditSideboardDialog.vue`): copies
 * taken out of the maindeck, copies brought in from the sideboard.
 */
const props = defineProps<{
  token: string
  game: GameDetailView
  mains: DeckCardOptionView[]
  sideboard: DeckCardOptionView[]
}>()

const open = ref(false)
const outs = ref<Map<number, number>>(new Map())
const ins = ref<Map<number, number>>(new Map())
const errors = ref<FieldErrors>({})
const { saving, saveManualSideboard } = useMatchEdit()

const outTotal = computed(() => sideboardTotal(outs.value))
const inTotal = computed(() => sideboardTotal(ins.value))
const error = computed(() => firstError(errors.value))

function bump(direction: 'out' | 'in', card: DeckCardOptionView, delta: number) {
  if (direction === 'out') outs.value = bumpSideboard(outs.value, card, delta)
  else ins.value = bumpSideboard(ins.value, card, delta)
}

function openDialog() {
  errors.value = {}
  const state = sideboardStateFromGame(props.game.sideboardChanges)
  outs.value = state.outs
  ins.value = state.ins
  open.value = true
}

async function submit() {
  errors.value = {}
  const changes = sideboardPayload({ outs: outs.value, ins: ins.value })
  const outcome = await saveManualSideboard(props.token, props.game.number, changes)
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
        <DialogTitle>Game {{ game.number }} sideboard changes</DialogTitle>
        <DialogDescription>Take cards out of the maindeck and bring cards in from the sideboard.</DialogDescription>
      </DialogHeader>

      <div class="flex items-center gap-3 font-mono text-xs tabular-nums">
        <span class="text-destructive">−{{ outTotal }} out</span>
        <span class="text-success">+{{ inTotal }} in</span>
        <span v-if="outTotal !== inTotal" class="ml-auto text-muted-foreground italic">
          Totals differ, saved as entered
        </span>
      </div>

      <div class="grid min-h-0 flex-1 grid-cols-2 gap-3 overflow-hidden">
        <section class="flex min-h-0 flex-col gap-1.5">
          <h3 class="text-[11px] font-semibold tracking-widest text-muted-foreground uppercase">Maindeck</h3>
          <div class="flex flex-col gap-1 overflow-y-auto pr-1">
            <div
              v-for="card in mains"
              :key="`out_${card.catalogId}`"
              class="flex items-center justify-between gap-2 rounded-md border px-2 py-1 text-xs"
              :class="outs.get(card.catalogId) ? 'border-destructive/50 bg-destructive/5' : ''"
            >
              <span class="truncate">
                {{ card.name }} <span class="text-muted-foreground">×{{ card.quantity }}</span>
              </span>
              <div class="inline-flex shrink-0 items-center gap-1">
                <Button
                  type="button"
                  variant="outline"
                  size="icon-sm"
                  class="size-6"
                  title="Put a copy back"
                  :disabled="!outs.get(card.catalogId)"
                  @click="bump('out', card, -1)"
                >
                  <Plus :size="10" />
                </Button>
                <span class="w-5 text-center font-mono tabular-nums">{{ outs.get(card.catalogId) ?? 0 }}</span>
                <Button
                  type="button"
                  variant="outline"
                  size="icon-sm"
                  class="size-6"
                  title="Take a copy out"
                  :disabled="(outs.get(card.catalogId) ?? 0) >= card.quantity"
                  @click="bump('out', card, 1)"
                >
                  <Minus :size="10" />
                </Button>
              </div>
            </div>
          </div>
        </section>
        <section class="flex min-h-0 flex-col gap-1.5">
          <h3 class="text-[11px] font-semibold tracking-widest text-muted-foreground uppercase">Sideboard</h3>
          <div class="flex flex-col gap-1 overflow-y-auto pr-1">
            <div
              v-for="card in sideboard"
              :key="`in_${card.catalogId}`"
              class="flex items-center justify-between gap-2 rounded-md border px-2 py-1 text-xs"
              :class="ins.get(card.catalogId) ? 'border-success/50 bg-success/5' : ''"
            >
              <span class="truncate">
                {{ card.name }} <span class="text-muted-foreground">×{{ card.quantity }}</span>
              </span>
              <div class="inline-flex shrink-0 items-center gap-1">
                <Button
                  type="button"
                  variant="outline"
                  size="icon-sm"
                  class="size-6"
                  title="Leave a copy in the sideboard"
                  :disabled="!ins.get(card.catalogId)"
                  @click="bump('in', card, -1)"
                >
                  <Minus :size="10" />
                </Button>
                <span class="w-5 text-center font-mono tabular-nums">{{ ins.get(card.catalogId) ?? 0 }}</span>
                <Button
                  type="button"
                  variant="outline"
                  size="icon-sm"
                  class="size-6"
                  title="Bring a copy in"
                  :disabled="(ins.get(card.catalogId) ?? 0) >= card.quantity"
                  @click="bump('in', card, 1)"
                >
                  <Plus :size="10" />
                </Button>
              </div>
            </div>
            <p v-if="sideboard.length === 0" class="text-xs text-muted-foreground italic">
              This deck version has no sideboard.
            </p>
          </div>
        </section>
      </div>

      <p v-if="error" class="text-xs text-destructive">{{ error }}</p>

      <DialogFooter class="gap-2">
        <Button type="button" variant="ghost" @click="open = false">Cancel</Button>
        <Button type="button" :disabled="saving" @click="submit()">Save changes</Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>
