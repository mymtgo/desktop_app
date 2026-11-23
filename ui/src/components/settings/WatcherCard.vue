<script setup lang="ts">
import { useQueryClient } from '@tanstack/vue-query'
import { computed, ref } from 'vue'
import { commands } from '@/bindings'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Label } from '@/components/ui/label'
import { Spinner } from '@/components/ui/spinner'
import { useToast } from '@/composables/useToast'
import { errorMessage, unwrap } from '@/lib/commands'
import { keys } from '@/lib/keys'
import { useSettings, useStatus } from '@/lib/queries'
import { watcherState } from '@/lib/storage'

// 0.x WatcherCard (release spec 4.1). Off, the tracker reads no log and no
// deck XML; on again, it resumes where it stopped. "Running" needs a found
// mtgo.log, and so does Start.
const queryClient = useQueryClient()
const { add } = useToast()
const { data: settings } = useSettings()
const { data: status } = useStatus()
const processing = ref(false)

const active = computed(() => settings.value?.watcherActive ?? true)
const state = computed(() => watcherState(active.value, status.value))

async function toggle() {
  processing.value = true
  try {
    queryClient.setQueryData(keys.settings, unwrap(await commands.setWatcherActive(!active.value)))
  } catch (error) {
    add({ type: 'error', title: 'Could not switch the watcher', message: errorMessage(error) })
  } finally {
    processing.value = false
  }
}
</script>

<template>
  <section class="flex flex-col gap-4 rounded-lg border bg-card p-4">
    <div class="space-y-1">
      <h2 class="text-sm font-semibold">Watcher</h2>
      <p class="text-xs text-muted-foreground">
        Control the file system watcher that monitors log files and triggers ingestion.
      </p>
    </div>
    <div class="flex items-center justify-between gap-4">
      <div>
        <Label>File Watcher</Label>
        <p class="text-sm text-muted-foreground">Monitors log files and triggers ingestion automatically.</p>
        <p v-if="!state.canStart" class="text-sm text-destructive">File paths must be valid to enable the watcher.</p>
      </div>
      <div class="flex items-center gap-3">
        <Badge :variant="state.running ? 'default' : 'secondary'">{{ state.badge }}</Badge>
        <Button variant="outline" size="sm" :disabled="!state.canStart || processing" @click="toggle">
          <Spinner v-if="processing" />
          {{ processing ? 'Processing...' : state.button }}
        </Button>
      </div>
    </div>
  </section>
</template>
