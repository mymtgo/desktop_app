<script setup lang="ts">
import { Trash2 } from 'lucide-vue-next'
import { ref } from 'vue'
import { commands, type OverlayNote } from '@/bindings'
import { Button } from '@/components/ui/button'
import { errorMessage, unwrap } from '@/lib/commands'
import { NOTE_PLACEHOLDER, noteStamp, otherNoteCaption } from '@/lib/overlay'

/**
 * The matchup notes (0.x `ArchetypeNotes`): a composer, this deck's notes
 * (deletable) and other decks' notes. The deck and archetype are the live
 * match's, found in Rust; the view is pushed again once saved.
 */
const props = defineProps<{
  current: OverlayNote[]
  other: OverlayNote[]
  disabled?: boolean
}>()

const body = ref('')
const saving = ref(false)
const error = ref<string | null>(null)

/** Grow the composer with its content instead of scrolling a two-line box. */
function autoSize(event: Event): void {
  const el = event.target as HTMLTextAreaElement
  el.style.height = 'auto'
  el.style.height = `${el.scrollHeight}px`
}

async function add(): Promise<void> {
  if (!body.value.trim() || saving.value) return
  saving.value = true
  error.value = null
  try {
    unwrap(await commands.addOverlayNote(body.value))
    body.value = ''
  } catch (failure) {
    error.value = errorMessage(failure)
  } finally {
    saving.value = false
  }
}

async function remove(id: number): Promise<void> {
  try {
    unwrap(await commands.deleteOverlayNote(id))
  } catch (failure) {
    error.value = errorMessage(failure)
  }
}
</script>

<template>
  <section class="flex flex-col gap-2">
    <div class="flex flex-col gap-1.5">
      <textarea
        v-model="body"
        :disabled="props.disabled"
        rows="2"
        :placeholder="NOTE_PLACEHOLDER"
        class="w-full resize-none overflow-hidden rounded-md border border-border bg-background px-2 py-1.5 text-xs disabled:opacity-50"
        @input="autoSize"
      />
      <p v-if="error" class="text-xs text-destructive">{{ error }}</p>
      <Button size="sm" class="self-end" :disabled="props.disabled || saving || !body.trim()" @click="add">Add note</Button>
    </div>

    <div v-if="props.current.length" class="flex flex-col gap-1.5">
      <h3 class="text-xs font-semibold text-muted-foreground">Notes for this deck</h3>
      <article
        v-for="note in props.current"
        :key="note.id"
        class="flex items-start justify-between gap-2 rounded-md border border-border px-2 py-1.5"
      >
        <div class="flex min-w-0 flex-col gap-0.5">
          <p class="text-xs whitespace-pre-wrap">{{ note.body }}</p>
          <span class="text-[10px] text-muted-foreground">{{ noteStamp(note.createdAtMs) }}</span>
        </div>
        <button type="button" class="shrink-0 text-muted-foreground hover:text-destructive" @click="remove(note.id)">
          <Trash2 class="size-3" />
        </button>
      </article>
    </div>

    <div v-if="props.other.length" class="flex flex-col gap-1.5">
      <h3 class="text-xs font-semibold text-muted-foreground">Other notes</h3>
      <article
        v-for="note in props.other"
        :key="note.id"
        class="flex flex-col gap-0.5 rounded-md border border-dashed border-border px-2 py-1.5"
      >
        <p class="text-xs whitespace-pre-wrap">{{ note.body }}</p>
        <span class="text-[10px] text-muted-foreground">{{ otherNoteCaption(note.deckName, note.createdAtMs) }}</span>
      </article>
    </div>
  </section>
</template>
