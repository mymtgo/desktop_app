<script setup lang="ts">
/**
 * Confirms leaving offline mode (0.x OfflineModeRejoinDialog). Leaving starts
 * a cooldown before offline mode can be switched back on, so this says so
 * before the fact. Only shown on the way out: turning privacy back on never
 * needs confirming.
 */
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog'

const props = defineProps<{ open: boolean; submitting?: boolean }>()

const emit = defineEmits<{
  'update:open': [value: boolean]
  confirm: []
}>()
</script>

<template>
  <Dialog :open="props.open" @update:open="emit('update:open', $event)">
    <DialogContent class="max-w-md">
      <DialogHeader>
        <DialogTitle>Come back online?</DialogTitle>
        <DialogDescription>
          Your matches will start being shared again, and archetype estimates and updates, metagame shares, opponent
          scouting and, when signed in, cloud sync come back with them.
        </DialogDescription>
      </DialogHeader>

      <p class="text-sm text-muted-foreground">
        Archetypes you downloaded while offline become MyMTGO's archetypes again, and edits you made to them are dropped.
      </p>
      <p class="text-sm text-warning">You won't be able to turn offline mode back on until tomorrow.</p>

      <DialogFooter>
        <Button variant="outline" :disabled="props.submitting" @click="emit('update:open', false)">Stay offline</Button>
        <Button :disabled="props.submitting" @click="emit('confirm')">Come back online</Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>
