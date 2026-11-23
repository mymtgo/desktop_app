<script setup lang="ts">
import { ref } from 'vue'
import { commands } from '@/bindings'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { useLeagueEdit } from '@/composables/useLeagueEdit'
import { useToast } from '@/composables/useToast'

/** The run's notes editor (0.x `LeagueNotesDialog`). Blank notes clear them. */
const open = ref(false)
const runKey = ref<string | null>(null)
const notes = ref('')
const { saving, run } = useLeagueEdit()
const { add: toast } = useToast()

function openForRun(key: string, current: string | null) {
  runKey.value = key
  notes.value = current ?? ''
  open.value = true
}

async function save() {
  const key = runKey.value
  if (key === null) return
  const done = await run('Could not save notes', () => commands.setLeagueNotes(key, notes.value))
  if (!done) return
  open.value = false
  toast({ type: 'success', title: 'Notes saved', message: 'League notes updated.' })
}

defineExpose({ openForRun })
</script>

<template>
  <Dialog v-model:open="open">
    <DialogContent class="sm:max-w-md">
      <DialogHeader>
        <DialogTitle>League Notes</DialogTitle>
      </DialogHeader>
      <textarea
        v-model="notes"
        class="min-h-[120px] w-full rounded-md border border-input bg-background px-3 py-2 text-sm ring-offset-background placeholder:text-muted-foreground focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
        placeholder="Add your notes about this league run..."
      />
      <DialogFooter>
        <Button variant="outline" size="sm" @click="open = false">Cancel</Button>
        <Button size="sm" :disabled="saving" @click="save">Save</Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>
