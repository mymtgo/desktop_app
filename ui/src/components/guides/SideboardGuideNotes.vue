<script setup lang="ts">
// The notes for one matchup (0.x `SideboardGuideNotes`): add one, delete
// this deck's, and read the other decks' notes against the archetype. Each
// note is written on Add; the plan is saved apart.
import { useQueryClient } from '@tanstack/vue-query'
import { ChevronDown, ChevronRight, Trash2 } from 'lucide-vue-next'
import { ref } from 'vue'
import { commands, type NoteView } from '@/bindings'
import { Button } from '@/components/ui/button'
import { useToast } from '@/composables/useToast'
import { errorMessage, unwrap } from '@/lib/commands'
import { guideKeys } from '@/lib/guides'

const props = defineProps<{
  deckId: number
  archetypeId: number
  current: NoteView[]
  other: NoteView[]
}>()

const queryClient = useQueryClient()
const { add } = useToast()

const body = ref('')
const saving = ref(false)
const showOther = ref(false)

async function addNote() {
  if (!body.value.trim() || saving.value) return
  saving.value = true
  try {
    unwrap(await commands.addNote(props.deckId, props.archetypeId, body.value))
    body.value = ''
    await queryClient.invalidateQueries({ queryKey: guideKeys.all })
  } catch (failure) {
    add({ type: 'error', title: 'Could not add the note', message: errorMessage(failure) })
  } finally {
    saving.value = false
  }
}

async function remove(id: number) {
  try {
    unwrap(await commands.deleteNote(id))
    await queryClient.invalidateQueries({ queryKey: guideKeys.all })
  } catch (failure) {
    add({ type: 'error', title: 'Could not delete the note', message: errorMessage(failure) })
  }
}
</script>

<template>
  <section class="flex flex-col gap-3">
    <h3 class="text-sm font-semibold">Notes</h3>

    <div class="flex flex-col gap-1.5">
      <textarea
        v-model="body"
        rows="3"
        maxlength="2000"
        placeholder="What did you learn this time?"
        class="w-full resize-none rounded-md border border-border bg-background px-2 py-1.5 text-sm"
      />
      <Button size="sm" class="self-end" :disabled="saving || !body.trim()" @click="addNote">Add note</Button>
    </div>

    <p v-if="current.length === 0" class="text-xs text-muted-foreground">No notes for this matchup yet.</p>

    <article
      v-for="note in current"
      :key="note.id"
      class="flex items-start justify-between gap-2 rounded-md border border-border px-3 py-2"
    >
      <div class="flex min-w-0 flex-col gap-1">
        <p class="text-sm whitespace-pre-wrap">{{ note.body }}</p>
        <time class="text-[10px] text-muted-foreground tabular-nums">{{ note.createdLabel }}</time>
      </div>
      <Button variant="ghost" size="icon" class="size-6 shrink-0" aria-label="Delete note" @click="remove(note.id)">
        <Trash2 class="size-3" />
      </Button>
    </article>

    <div v-if="other.length" class="flex flex-col gap-1.5">
      <button
        type="button"
        class="flex items-center gap-1 self-start text-xs font-semibold text-muted-foreground hover:text-foreground"
        :aria-expanded="showOther"
        @click="showOther = !showOther"
      >
        <ChevronDown v-if="showOther" class="size-3" />
        <ChevronRight v-else class="size-3" />
        Other decks ({{ other.length }})
      </button>
      <template v-if="showOther">
        <article v-for="note in other" :key="note.id" class="rounded-md border border-border/60 px-3 py-2">
          <p class="text-sm whitespace-pre-wrap">{{ note.body }}</p>
          <p class="text-[10px] text-muted-foreground">{{ note.deckName }} · {{ note.createdLabel }}</p>
        </article>
      </template>
    </div>
  </section>
</template>
