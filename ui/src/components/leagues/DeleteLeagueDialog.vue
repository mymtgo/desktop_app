<script setup lang="ts">
import { ref } from 'vue'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { useLeagueManage } from '@/composables/useLeagueManage'
import { deleteLeagueQuestion } from '@/lib/leagues'

/**
 * "Delete league..." (league self-management spec 5.2). With matches, the
 * player keeps them as non-league matches (the default) or deletes them
 * too. The toast after it offers Undo.
 */
const open = ref(false)
const runKey = ref<string | null>(null)
const matchCount = ref(0)
const choice = ref<'keep' | 'delete'>('keep')
const { saving, deleteLeague } = useLeagueManage()

function openForRun(key: string, count: number) {
  runKey.value = key
  matchCount.value = count
  choice.value = 'keep'
  open.value = true
}

async function confirm() {
  const key = runKey.value
  if (key === null) return
  if (await deleteLeague(key, choice.value)) open.value = false
}

defineExpose({ openForRun })
</script>

<template>
  <Dialog v-model:open="open">
    <DialogContent class="sm:max-w-md">
      <DialogHeader>
        <DialogTitle>Delete league</DialogTitle>
        <DialogDescription>{{ deleteLeagueQuestion(matchCount) }}</DialogDescription>
      </DialogHeader>
      <div v-if="matchCount > 0" class="flex flex-col gap-2 text-sm">
        <label class="flex items-start gap-2">
          <input
            type="radio"
            name="delete-league-matches"
            data-testid="delete-keep"
            :checked="choice === 'keep'"
            @change="choice = 'keep'"
          />
          <span>Keep them as non-league matches</span>
        </label>
        <label class="flex items-start gap-2">
          <input
            type="radio"
            name="delete-league-matches"
            data-testid="delete-too"
            :checked="choice === 'delete'"
            @change="choice = 'delete'"
          />
          <span>Delete them too</span>
        </label>
        <p class="text-xs text-muted-foreground">You can restore it from Settings, Archives.</p>
      </div>
      <DialogFooter>
        <Button variant="outline" size="sm" @click="open = false">Cancel</Button>
        <Button
          variant="destructive"
          size="sm"
          data-testid="delete-league-confirm"
          :disabled="saving"
          @click="confirm"
        >
          {{ matchCount > 0 ? 'Delete league' : 'Delete' }}
        </Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>
