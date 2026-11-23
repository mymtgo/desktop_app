<script setup lang="ts">
import { Download, ExternalLink } from 'lucide-vue-next'
import { Button } from '@/components/ui/button'
import type { ReportBundleError } from '@/composables/useReportBundle'

/** Get help's content (0.x `HelpPopover`), apart from the popover frame. */
defineProps<{ busy: boolean; error: ReportBundleError | null }>()
const emit = defineEmits<{ openDiscord: []; download: []; openSettings: [] }>()
</script>

<template>
  <div class="space-y-3">
    <div class="space-y-1">
      <p class="text-sm font-semibold">Need help?</p>
      <p class="text-xs text-muted-foreground">
        Hop into our Discord server to report bugs or ask questions. Including your report data helps a lot when
        reporting an issue.
      </p>
    </div>

    <div class="flex flex-col gap-2">
      <Button type="button" class="w-full" @click="emit('openDiscord')">
        <ExternalLink class="size-4" />
        Open Discord
      </Button>

      <Button type="button" variant="outline" class="w-full" :disabled="busy" @click="emit('download')">
        <Download class="size-4" />
        {{ busy ? 'Preparing…' : 'Download report data' }}
      </Button>

      <p v-if="error" class="text-xs text-destructive">
        {{ error.message }}
        <button v-if="error.showSettingsLink" type="button" class="underline" @click="emit('openSettings')">Open settings</button>
      </p>
      <p v-else class="text-xs text-muted-foreground">
        This zips your MTGO and Arena logs and MyMTGO's recent logs to help with debugging. You can optionally upload
        it in Discord. It includes your MTGO and Arena usernames, the names of players you met, and file paths that may
        show your computer's account name.
      </p>
    </div>
  </div>
</template>
