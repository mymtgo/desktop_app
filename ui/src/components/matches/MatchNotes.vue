<script setup lang="ts">
import { useQueryClient } from '@tanstack/vue-query'
import { NotepadText } from 'lucide-vue-next'
import { nextTick, ref, watch } from 'vue'
import { commands } from '@/bindings'
import { useToast } from '@/composables/useToast'
import { errorMessage, unwrap } from '@/lib/commands'
import { keys } from '@/lib/keys'
import { matchDetailKeys, NOTES_MAX, notesTooLong } from '@/lib/matchDetail'

/**
 * The match's notes, edited in place (0.x `MatchDetail.vue`): click to edit,
 * saved on blur. Saving pushes no event, so the detail and the match lists
 * (main, deck, dashboard and Limited) are refetched here.
 */
const props = defineProps<{
  token: string
  notes: string | null
}>()

const queryClient = useQueryClient()
const { add: toast } = useToast()

const editing = ref(false)
const draft = ref('')
/** What the page shows: the saved notes, or the last text sent while it saves. */
const shown = ref<string | null>(props.notes)
const textarea = ref<HTMLTextAreaElement | null>(null)

watch(
  () => props.notes,
  (notes) => {
    if (!editing.value) shown.value = notes
  },
)

function startEditing() {
  draft.value = shown.value ?? ''
  editing.value = true
  void nextTick(() => textarea.value?.focus())
}

async function save() {
  editing.value = false
  const text = draft.value
  if (text === (shown.value ?? '')) return
  if (notesTooLong(text)) {
    toast({ type: 'error', title: 'Could not save notes', message: `The notes field must not be greater than ${NOTES_MAX} characters.` })
    return
  }
  const previous = shown.value
  shown.value = text.trim() === '' ? null : text
  try {
    unwrap(await commands.setMatchNotes(props.token, text))
    await Promise.all([
      queryClient.invalidateQueries({ queryKey: matchDetailKeys.detail(props.token) }),
      queryClient.invalidateQueries({ queryKey: keys.allMatches }),
      queryClient.invalidateQueries({ queryKey: keys.allDeckMatches }),
      queryClient.invalidateQueries({ queryKey: keys.allDashboard }),
      queryClient.invalidateQueries({ queryKey: keys.allLimited }),
    ])
  } catch (error) {
    shown.value = previous
    toast({ type: 'error', title: 'Could not save notes', message: errorMessage(error) })
  }
}
</script>

<template>
  <section class="flex flex-col gap-1.5">
    <div class="flex items-center gap-1.5 text-xs font-medium tracking-wide text-muted-foreground uppercase">
      <NotepadText :size="13" />
      <span>Notes</span>
    </div>
    <textarea
      v-if="editing"
      ref="textarea"
      v-model="draft"
      :maxlength="NOTES_MAX"
      class="min-h-[80px] w-full rounded-md border border-input bg-background px-3 py-2 text-sm ring-offset-background placeholder:text-muted-foreground focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
      placeholder="Add your notes about this match..."
      @blur="save"
    />
    <p
      v-else
      class="cursor-text rounded-md border border-dashed px-3 py-2 text-sm text-muted-foreground transition-colors hover:border-primary/40 hover:text-foreground"
      @click="startEditing"
    >
      {{ shown || 'Click to add notes…' }}
    </p>
  </section>
</template>
