<script setup lang="ts">
// The Color Identity card of the deck Settings page (0.x `pages/decks/Settings.vue`).
import { computed, ref, watch } from 'vue'
import { commands } from '@/bindings'
import ManaColors from '@/components/mana/ManaColors.vue'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import { Spinner } from '@/components/ui/spinner'
import { useDeckEdit } from '@/composables/useDeckEdit'
import { ARCHIVED_TITLE, IDENTITY_OPTIONS, identityChanged, orderIdentity, toggleIdentity } from '@/lib/deckSettings'

const props = defineProps<{
  deckId: number
  /** The saved identity letters. */
  colors: string[]
  archived: boolean
}>()

const { saving, run } = useDeckEdit()
const selected = ref<string[]>([...props.colors])

// By value: a refetch brings a new array even when nothing changed, and
// that must not undo an unsaved selection.
watch(
  () => props.colors.join(','),
  () => {
    selected.value = [...props.colors]
  },
)

const changed = computed(() => identityChanged(selected.value, props.colors))
const preview = computed(() => orderIdentity(selected.value))

function toggle(letter: string) {
  if (props.archived) {
    return
  }
  selected.value = toggleIdentity(selected.value, letter)
}

async function save() {
  if (!changed.value) {
    return
  }
  const letters = orderIdentity(selected.value)
  const done = await run('Could not save the colour identity', () => commands.setDeckIdentity(props.deckId, letters))
  if (!done) {
    selected.value = [...props.colors]
  }
}

function reset() {
  selected.value = [...props.colors]
}
</script>

<template>
  <Card>
    <CardHeader>
      <CardTitle>Color Identity</CardTitle>
      <CardDescription>Override the deck's color identity. Used for filtering and matchup grouping.</CardDescription>
    </CardHeader>
    <CardContent class="flex flex-col gap-4">
      <div class="flex flex-wrap gap-2">
        <button
          v-for="option in IDENTITY_OPTIONS"
          :key="option.key"
          type="button"
          class="flex items-center gap-2 rounded-md border px-3 py-2 text-sm transition-colors"
          :class="[
            selected.includes(option.key)
              ? 'border-primary bg-primary/10 text-foreground'
              : 'border-border bg-transparent text-muted-foreground hover:border-muted-foreground hover:text-foreground',
            archived ? 'cursor-not-allowed opacity-50' : '',
          ]"
          :disabled="archived"
          :title="archived ? ARCHIVED_TITLE : option.label"
          :aria-pressed="selected.includes(option.key)"
          @click="toggle(option.key)"
        >
          <ManaColors :colors="[option.key]" />
          <span>{{ option.label }}</span>
        </button>
      </div>

      <div class="flex items-center gap-3 rounded-md border border-dashed border-border bg-muted/30 px-3 py-2">
        <span class="text-sm text-muted-foreground">Preview:</span>
        <ManaColors v-if="preview.length > 0" :colors="preview" />
        <span v-else class="text-sm text-muted-foreground italic">No colors selected</span>
      </div>

      <div class="flex items-center gap-2">
        <Button :disabled="!changed || saving || archived" :title="archived ? ARCHIVED_TITLE : undefined" @click="save">
          <Spinner v-if="saving" class="mr-2 size-4" />
          Save
        </Button>
        <Button v-if="changed" variant="ghost" :disabled="saving || archived" @click="reset">Cancel</Button>
      </div>
    </CardContent>
  </Card>
</template>
