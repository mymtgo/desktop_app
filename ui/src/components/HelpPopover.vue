<script setup lang="ts">
import { LifeBuoy } from 'lucide-vue-next'
import { useRouter } from 'vue-router'
import { commands } from '@/bindings'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { useReportBundle } from '@/composables/useReportBundle'
import HelpPopoverBody from './HelpPopoverBody.vue'

/** MyMTGO's Discord invite (0.x `config/support.php`). */
const DISCORD_INVITE_URL = 'https://discord.gg/KuFv4Jcm38'
/** Where the log path is set (spec section 4: the Storage settings page, W1). */
const LOG_PATH_SETTINGS = { name: 'settings-storage' }

const router = useRouter()
const { busy, error, download } = useReportBundle()

async function openDiscord(): Promise<void> {
  const result = await commands.openExternal(DISCORD_INVITE_URL)
  if (result.status === 'error') console.error('Could not open Discord', result.error)
}

function openSettings(): void {
  void router.push(LOG_PATH_SETTINGS)
}
</script>

<template>
  <Popover>
    <PopoverTrigger as-child>
      <button type="button" class="flex items-center gap-1.5 text-xs text-muted-foreground hover:text-foreground">
        <LifeBuoy class="size-3.5" />
        <span>Get help</span>
      </button>
    </PopoverTrigger>
    <PopoverContent side="top" align="end" class="w-80">
      <HelpPopoverBody
        :busy="busy"
        :error="error"
        @open-discord="openDiscord"
        @download="download"
        @open-settings="openSettings"
      />
    </PopoverContent>
  </Popover>
</template>
