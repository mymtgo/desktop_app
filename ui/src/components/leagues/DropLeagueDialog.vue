<script setup lang="ts">
import { TriangleAlert } from 'lucide-vue-next'
import { computed, ref } from 'vue'
import type { PlatformView } from '@/bindings'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { useLeagueManage } from '@/composables/useLeagueManage'

/**
 * "Set status, Dropped" (league self-management spec 4.3): the run stops
 * taking matches; it does not drop the player in the game. Worded per
 * platform: an MTGO league, or an Arena run (#834).
 */
const props = withDefaults(defineProps<{ platform?: PlatformView }>(), { platform: 'mtgo' })

const arena = computed(() => props.platform === 'arena')

const open = ref(false)
const runKey = ref<string | null>(null)
const { saving, setStatus } = useLeagueManage()

function openForRun(key: string) {
  runKey.value = key
  open.value = true
}

async function confirm() {
  const key = runKey.value
  if (key === null) return
  await setStatus(key, 'dropped')
  open.value = false
}

defineExpose({ openForRun })
</script>

<template>
  <Dialog v-model:open="open">
    <DialogContent class="sm:max-w-md">
      <DialogHeader>
        <DialogTitle>{{ arena ? 'Drop this run?' : 'Drop league?' }}</DialogTitle>
      </DialogHeader>
      <div class="space-y-3 text-sm">
        <p>This marks the run as dropped in your tracker.</p>
        <div class="flex gap-2 rounded-md border border-amber-500/30 bg-amber-500/10 p-3 text-amber-400">
          <TriangleAlert class="size-4 shrink-0" />
          <p v-if="arena" class="text-xs">
            This does <strong>not</strong> drop you from the event in Arena. You must do that in Arena itself.
          </p>
          <p v-else class="text-xs">
            This does <strong>not</strong> drop you from the league on MTGO. You must do that in the MTGO client
            itself.
          </p>
        </div>
      </div>
      <DialogFooter>
        <Button variant="outline" size="sm" @click="open = false">Cancel</Button>
        <Button variant="destructive" size="sm" :disabled="saving" @click="confirm">
          {{ arena ? 'Drop run' : 'Drop league' }}
        </Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>
