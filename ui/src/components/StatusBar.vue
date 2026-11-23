<script setup lang="ts">
import { useIntervalFn, useNow } from '@vueuse/core'
import { TriangleAlert } from 'lucide-vue-next'
import { computed } from 'vue'
import HelpPopover from '@/components/HelpPopover.vue'
import StatusBarVersion from '@/components/StatusBarVersion.vue'
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from '@/components/ui/tooltip'
import { useOfflineMode } from '@/composables/useOfflineMode'
import { usePlatformContext } from '@/composables/usePlatformContext'
import { useWhatsNewUrl } from '@/composables/useWhatsNew'
import { useIntegrationsQuery, useStatus } from '@/lib/queries'
import { newestEventMs, sinceText, statusWarnings, trackingState } from '@/lib/status'

const { data: status } = useStatus()
const { data: integrations } = useIntegrationsQuery()
const offlineMode = useOfflineMode()
const { platform } = usePlatformContext()
const whatsNewUrl = useWhatsNewUrl()
const now = useNow({ scheduler: (update) => useIntervalFn(update, 1000) })

const state = computed(() => (status.value ? trackingState(status.value, platform.value, integrations?.value?.endstepStatus.connection) : { tone: 'idle' as const, text: 'Starting' }))
const since = computed(() => (status.value ? sinceText(newestEventMs(status.value, platform.value), now.value.getTime()) : ''))
const warnings = computed(() => (status.value ? statusWarnings(status.value, platform.value) : []))
const dot = computed(
  () =>
    ({ ok: 'bg-success', warn: 'bg-warning', error: 'bg-destructive', idle: 'bg-muted-foreground' })[state.value.tone],
)
</script>

<template>
  <footer class="flex h-7 shrink-0 items-center gap-4 border-t bg-muted/30 px-3 text-xs text-muted-foreground">
    <div class="flex items-center gap-1.5">
      <div class="size-1.5 rounded-full" :class="dot" />
      <span>{{ state.text }}</span>
    </div>
    <template v-if="offlineMode">
      <div class="h-3 w-px bg-border" />
      <div class="flex items-center gap-1.5">
        <div class="size-1.5 rounded-full bg-warning" />
        <span>Offline mode</span>
      </div>
    </template>
    <div class="h-3 w-px bg-border" />
    <span>{{ since }}</span>
    <template v-if="warnings.length > 0">
      <div class="h-3 w-px bg-border" />
      <TooltipProvider>
        <Tooltip>
          <TooltipTrigger as-child>
            <span class="inline-flex items-center gap-1 text-warning">
              <TriangleAlert class="size-3.5" />
              {{ warnings[0] }}<template v-if="warnings.length > 1"> (+{{ warnings.length - 1 }})</template>
            </span>
          </TooltipTrigger>
          <TooltipContent side="top" class="max-w-sm">
            <p v-for="warning in warnings" :key="warning" class="text-xs">{{ warning }}</p>
          </TooltipContent>
        </Tooltip>
      </TooltipProvider>
    </template>
    <div class="flex-1" />
    <StatusBarVersion :whats-new-url="whatsNewUrl" />
    <div class="h-3 w-px bg-border" />
    <HelpPopover />
  </footer>
</template>
