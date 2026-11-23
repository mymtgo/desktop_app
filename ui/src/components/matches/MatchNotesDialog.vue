<script setup lang="ts">
import { ref } from 'vue'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { useMatchEdit } from '@/composables/useMatchEdit'

/** A match's notes editor (0.x `MatchNotesDialog`). Blank notes clear them; at most 5000 characters. */
const MAX_NOTES = 5000

const open = ref(false)
const token = ref<string | null>(null)
const notes = ref('')
const { saving, saveNotes } = useMatchEdit()

function openForMatch(matchToken: string, current: string | null) {
  token.value = matchToken
  notes.value = current ?? ''
  open.value = true
}

async function save() {
  const matchToken = token.value
  if (matchToken === null) return
  if (await saveNotes(matchToken, notes.value)) open.value = false
}

defineExpose({ openForMatch })
</script>

<template>
  <Dialog v-model:open="open">
    <DialogContent class="sm:max-w-md">
      <DialogHeader>
        <DialogTitle>Match Notes</DialogTitle>
      </DialogHeader>
      <textarea
        v-model="notes"
        :maxlength="MAX_NOTES"
        class="min-h-[120px] w-full rounded-md border border-input bg-background px-3 py-2 text-sm ring-offset-background placeholder:text-muted-foreground focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
        placeholder="Add your notes about this match..."
      />
      <DialogFooter>
        <Button variant="outline" size="sm" @click="open = false">Cancel</Button>
        <Button size="sm" :disabled="saving" @click="save">Save</Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>
