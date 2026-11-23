<script setup lang="ts">
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import ReplayShareBody from './ReplayShareBody.vue'
import type { ReplayShareView } from './types'

/** 0.x's share dialog: title, description, then the body. */
defineProps<ReplayShareView & { open: boolean }>()
const emit = defineEmits<{ 'update:open': [open: boolean]; share: []; revoke: []; copy: [] }>()
</script>

<template>
  <Dialog :open="open" @update:open="emit('update:open', $event)">
    <DialogContent class="max-w-md">
      <DialogHeader>
        <DialogTitle>Share this match</DialogTitle>
        <DialogDescription>
          Anyone with the link can watch every game of this match, starting from this one. Your opponent's name is
          hidden.
        </DialogDescription>
      </DialogHeader>
      <ReplayShareBody
        :share="share"
        :busy="busy"
        :error="error"
        :copied="copied"
        @share="emit('share')"
        @revoke="emit('revoke')"
        @copy="emit('copy')"
      />
    </DialogContent>
  </Dialog>
</template>
