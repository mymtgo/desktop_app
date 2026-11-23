<script setup lang="ts">
import { Share2 } from 'lucide-vue-next'
import { onMounted, onUnmounted, ref } from 'vue'
import { useReplayShareState } from '@/composables/useReplayShareState'
import ReplayToggleButton from '../ReplayToggleButton.vue'
import ReplayShareDialog from './ReplayShareDialog.vue'

/** 0.x's "Share" button for the viewer's `#actions` slot; the dialog shares the match this game is in. */
const props = defineProps<{ gameId: number }>()

const open = ref(false)
const { state, busy, error, copied, refresh, share, revoke, copy, reopened } = useReplayShareState(() => props.gameId)

function setOpen(value: boolean) {
  open.value = value
  if (value) void reopened()
}

// Signing in or switching offline mode happens in the main window; the state
// is read again when this window is focused.
function onFocus() {
  void refresh()
}
onMounted(() => {
  void refresh()
  window.addEventListener('focus', onFocus)
})
onUnmounted(() => window.removeEventListener('focus', onFocus))
</script>

<template>
  <ReplayToggleButton :active="open" :icon="Share2" @click="setOpen(!open)">Share</ReplayToggleButton>
  <ReplayShareDialog
    :open="open"
    :share="state"
    :busy="busy"
    :error="error"
    :copied="copied"
    @update:open="setOpen"
    @share="share"
    @revoke="revoke"
    @copy="copy"
  />
</template>
