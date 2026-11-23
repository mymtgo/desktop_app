<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from 'vue'
import { commands, events, type DraftNotesView } from '@/bindings'
import { useTauriEvents } from '@/composables/useTauriEvents'
import { usePickNoteAutosave } from '@/composables/usePickNoteAutosave'
import { unwrap } from '@/lib/commands'
import {
  afterNewView,
  LIVE,
  navigation,
  notePlaceholder,
  pickStep,
  secondsLeft,
  step,
  type Selection,
} from '@/lib/draftNotesWindow'
import { NOTE_MAX_LENGTH } from '@/lib/limited'
import DraftNotesHeader from './DraftNotesHeader.vue'

/**
 * The draft notes window (overlay spec 8; 0.x `DraftNotes.vue`). The first
 * state comes from `draft_notes`, then `draft-notes-changed` pushes replace
 * it (nothing polls). The editor follows the newest pick until ‹ or Alt+←
 * pins one. The countdown ticks locally from the pick's deadline. Notes save
 * by draft, so a draft not linked to a league takes them too.
 */
const notes = ref<DraftNotesView | null>(null)
const selection = ref<Selection>(LIVE)

function show(next: DraftNotesView | null): void {
  selection.value = afterNewView(selection.value, notes.value, next)
  notes.value = next
}

const nav = computed(() => navigation(notes.value, selection.value))

const now = ref(Date.now())
const left = computed(() => secondsLeft(nav.value.pick, nav.value.isLive, now.value))
const canEdit = computed(() => nav.value.pick !== null)

const { text, state, input, flush } = usePickNoteAutosave(() => {
  const pick = nav.value.pick
  if (notes.value === null || pick === null) return null
  return {
    draftToken: notes.value.draftToken,
    ordinal: pick.ordinal,
    note: pick.note,
    savedAtMs: pick.noteSavedAtMs,
  }
})

function go(delta: -1 | 1): void {
  selection.value = step(notes.value, selection.value, delta)
}

function goLive(): void {
  selection.value = LIVE
}

function onKeydown(event: KeyboardEvent): void {
  const delta = pickStep(event)
  if (delta === null) return
  event.preventDefault()
  go(delta)
}

let ticker: ReturnType<typeof setInterval> | null = null
let pushes = 0
const listening = useTauriEvents(() => [
  events.draftNotesChanged.listen((event) => {
    pushes += 1
    show(event.payload.view)
  }),
])

onMounted(async () => {
  window.addEventListener('keydown', onKeydown)
  ticker = setInterval(() => (now.value = Date.now()), 1000)
  if (!(await listening.ready)) return
  const before = pushes
  const first = unwrap(await commands.draftNotes())
  // A push that arrived meanwhile is newer than what the read saw.
  if (pushes === before) show(first)
})

onBeforeUnmount(() => {
  window.removeEventListener('keydown', onKeydown)
  if (ticker) clearInterval(ticker)
})
</script>

<template>
  <div class="flex h-full flex-col gap-2 p-3">
    <DraftNotesHeader
      :pick="nav.pick"
      :has-draft="notes !== null"
      :state="notes?.state ?? null"
      :is-live="nav.isLive"
      :picks-ahead="nav.picksAhead"
      :can-prev="nav.canPrev"
      :can-next="nav.canNext"
      :seconds-left="left"
      :status="state.status"
      @prev="go(-1)"
      @next="go(1)"
      @live="goLive"
    />

    <textarea
      v-model="text"
      :disabled="!canEdit"
      :maxlength="NOTE_MAX_LENGTH"
      :placeholder="notePlaceholder(canEdit)"
      class="min-h-0 w-full flex-1 resize-none rounded-md border border-black/60 bg-card px-3 py-2 text-sm select-text placeholder:text-muted-foreground focus:ring-2 focus:ring-primary focus:outline-none disabled:opacity-60"
      @input="input"
      @blur="flush"
    />
  </div>
</template>
