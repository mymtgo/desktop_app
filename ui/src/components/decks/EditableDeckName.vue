<script setup lang="ts">
// The deck name in the sidebar: click to rename, Enter or leaving the field
// saves, Escape cancels; a custom name shows a revert button on hover
// (0.x `components/decks/EditableDeckName.vue`).
import { Pencil, RotateCcw } from 'lucide-vue-next'
import { computed, nextTick, ref } from 'vue'
import { commands } from '@/bindings'
import { useDeckEdit } from '@/composables/useDeckEdit'
import { ARCHIVED_TITLE, DECK_NAME_INPUT_MAX, clampDeckName, renameTarget } from '@/lib/deckSettings'

const props = defineProps<{
  deckId: number
  name: string
  originalName: string | null
  archived: boolean
}>()

const { saving, run } = useDeckEdit()
const isEditing = ref(false)
const draft = ref(props.name)
const inputEl = ref<HTMLInputElement | null>(null)

const isCustomName = computed(() => Boolean(props.originalName))

async function startEditing() {
  if (props.archived) {
    return
  }
  draft.value = props.name
  isEditing.value = true
  await nextTick()
  inputEl.value?.focus()
  inputEl.value?.select()
}

/** Keep the draft to 255 characters as typed (an input's maxlength counts UTF-16 units). */
function onInput(event: Event) {
  const input = event.target as HTMLInputElement
  const clamped = clampDeckName(input.value)
  if (clamped !== input.value) {
    input.value = clamped
  }
  draft.value = clamped
}

function cancel() {
  draft.value = props.name
  isEditing.value = false
}

async function commit() {
  // Leaving the field while a save is running (it is disabled then) is not a second save.
  if (saving.value || !isEditing.value) {
    return
  }
  const next = renameTarget(draft.value, props.name)
  if (next === null) {
    cancel()
    return
  }
  const done = await run('Could not rename the deck', () => commands.renameDeck(props.deckId, next))
  if (!done) {
    draft.value = props.name
  }
  isEditing.value = false
}

async function revert() {
  const original = props.originalName
  if (!original || saving.value || props.archived) {
    return
  }
  await run('Could not revert the deck name', () => commands.renameDeck(props.deckId, original))
}
</script>

<template>
  <div class="group flex w-full items-center gap-1.5">
    <template v-if="isEditing">
      <input
        ref="inputEl"
        :value="draft"
        type="text"
        :maxlength="DECK_NAME_INPUT_MAX"
        :disabled="saving"
        class="min-w-0 flex-1 truncate rounded-sm border border-white/20 bg-black/40 px-1.5 py-0.5 text-base leading-tight font-semibold outline-none focus:border-sky-400/70 focus:ring-1 focus:ring-sky-400/30"
        @input="onInput"
        @keydown.enter.prevent="commit"
        @keydown.esc.prevent="cancel"
        @blur="commit"
      />
    </template>
    <template v-else>
      <button
        type="button"
        class="flex min-w-0 flex-1 items-center gap-1.5 truncate text-left disabled:cursor-default"
        :disabled="archived"
        :title="archived ? ARCHIVED_TITLE : isCustomName ? `Custom name (was: ${originalName})` : 'Click to rename'"
        @click="startEditing"
      >
        <h2 class="truncate text-base leading-tight font-semibold">{{ name }}</h2>
        <Pencil v-if="!archived" class="size-3 shrink-0 opacity-0 transition-opacity group-hover:opacity-60" />
      </button>
      <button
        v-if="isCustomName"
        type="button"
        :disabled="saving || archived"
        :title="archived ? ARCHIVED_TITLE : `Revert to '${originalName}'`"
        class="shrink-0 rounded p-0.5 text-muted-foreground opacity-0 transition-opacity group-hover:opacity-100 hover:text-foreground disabled:cursor-default disabled:opacity-0"
        @click.stop="revert"
      >
        <RotateCcw class="size-3" />
      </button>
    </template>
  </div>
</template>
