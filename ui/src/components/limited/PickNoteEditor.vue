<script setup lang="ts">
import { Check, Loader2 } from 'lucide-vue-next'
import { computed } from 'vue'
import type { DraftPickView } from '@/bindings'
import { usePickNoteAutosave } from '@/composables/usePickNoteAutosave'
import { NOTE_MAX_LENGTH, noteStatusIcon, noteStatusText } from '@/lib/limited'

/**
 * The selected pick's note (0.x `PickNoteEditor`): saved 600 ms after the
 * last keystroke, and at once on blur, on a pick change and on leaving the
 * page (`usePickNoteAutosave`, shared with the draft notes window). The
 * tracker's `limited-changed` then refetches the review. The text box is
 * reset when the pick changes, and takes a refetched note only while nothing
 * typed is unsaved, so a refetch never eats typing and a page shown from a
 * stale cache catches up with the saved note.
 */
const props = defineProps<{ draftToken: string; pick: DraftPickView }>()

const { text, state, input, flush } = usePickNoteAutosave(() => ({
  draftToken: props.draftToken,
  ordinal: props.pick.ordinal,
  note: props.pick.note,
  savedAtMs: props.pick.noteSavedAtMs,
}))

const statusText = computed(() => noteStatusText(state.value.status, state.value.savedAtMs))
const statusIcon = computed(() => noteStatusIcon(state.value.status, state.value.savedAtMs))
</script>

<template>
  <div class="flex flex-col gap-2">
    <div class="flex items-center justify-between text-xs">
      <span class="font-semibold">Your note</span>
      <span class="inline-flex items-center gap-1 text-muted-foreground">
        <Loader2 v-if="statusIcon === 'spinner'" class="size-3 animate-spin" />
        <Check v-else-if="statusIcon === 'check'" class="size-3 text-emerald-400" />
        <span v-if="statusText" :class="state.status === 'error' ? 'text-rose-400' : ''">{{ statusText }}</span>
      </span>
    </div>
    <textarea
      v-model="text"
      rows="5"
      :maxlength="NOTE_MAX_LENGTH"
      placeholder="Why this pick?"
      class="w-full resize-none rounded-md border border-black/60 bg-background px-3 py-2 text-sm placeholder:text-muted-foreground focus:ring-2 focus:ring-primary focus:outline-none"
      @input="input"
      @blur="flush"
    />
  </div>
</template>
