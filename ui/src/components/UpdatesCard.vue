<script setup lang="ts">
import { useIntervalFn, useNow } from '@vueuse/core'
import { computed, ref } from 'vue'
import { RouterLink } from 'vue-router'
import { Button } from '@/components/ui/button'
import { Separator } from '@/components/ui/separator'
import { Switch } from '@/components/ui/switch'
import { commands } from '@/bindings'
import { useInstallUpdate } from '@/composables/useInstallUpdate'
import { useToast } from '@/composables/useToast'
import { errorMessage, unwrap } from '@/lib/commands'
import { queryClient } from '@/queryClient'
import { keys } from '@/lib/keys'
import { useAppInfo, useSettings, useUpdateStatus } from '@/lib/queries'
import { checkedAgo, updateDot, updateLabel } from '@/lib/updates'

// 0.x `UpdatesCard.vue` plus the update channel (release spec 6.1, #221)
// and the What's new link (6.2; the page comes with What's new).
const props = withDefaults(defineProps<{ whatsNewUrl?: string | null }>(), { whatsNewUrl: null })

const { data: update } = useUpdateStatus()
const { data: settings } = useSettings()
const { data: info } = useAppInfo()
const { add } = useToast()
const choosing = ref(false)
const { installing, install } = useInstallUpdate()
const now = useNow({ scheduler: (tick) => useIntervalFn(tick, 30_000) })
const checking = ref(false)
const inFlight = computed(() => update.value?.phase === 'checking' || update.value?.phase === 'downloading')
// The product name is read at runtime (release spec 13.1); the brand until it arrives.
const productName = computed(() => info.value?.productName ?? 'MyMTGO')
const versionLine = computed(() =>
  update.value ? `${productName.value} v${update.value.current} (${update.value.channel})` : '',
)

async function check() {
  checking.value = true
  try {
    queryClient.setQueryData(keys.updates, await commands.checkForUpdates())
  } finally {
    checking.value = false
  }
}

// Its own command, as the Watcher's: a settings save from a page read
// earlier keeps the stored choice, so it can never undo this switch.
async function setBetaUpdates(on: boolean) {
  choosing.value = true
  try {
    queryClient.setQueryData(keys.settings, unwrap(await commands.setBetaUpdates(on)))
  } catch (error) {
    add({ type: 'error', title: 'Could not save settings', message: errorMessage(error) })
  } finally {
    choosing.value = false
  }
}
</script>

<template>
  <section v-if="update" class="space-y-4 rounded-lg border bg-card p-4">
    <div class="flex items-start justify-between gap-6">
      <div class="space-y-1">
        <h2 class="text-sm font-semibold">Updates</h2>
        <p class="text-xs text-muted-foreground">{{ versionLine }}</p>
        <p class="flex items-center gap-2 text-xs text-muted-foreground">
          <span class="size-2 rounded-full" :class="updateDot(update)" />
          {{ updateLabel(update) }}
        </p>
        <p v-if="update.phase === 'error' && update.error" class="text-xs text-muted-foreground">{{ update.error }}</p>
        <p v-if="checkedAgo(update.checkedAtMs, now.getTime())" class="text-xs text-muted-foreground">
          {{ checkedAgo(update.checkedAtMs, now.getTime()) }}
        </p>
        <RouterLink v-if="props.whatsNewUrl" :to="props.whatsNewUrl" class="text-xs text-primary hover:underline"
          >What's new</RouterLink
        >
      </div>
      <div class="flex shrink-0 items-center gap-2">
        <Button v-if="update.phase === 'ready'" size="sm" :disabled="installing" @click="install"
          >Install & restart</Button
        >
        <Button v-else size="sm" variant="outline" :disabled="!update.active || checking || inFlight" @click="check">
          Check for updates
        </Button>
      </div>
    </div>
    <Separator />
    <div class="flex items-center justify-between gap-6">
      <div class="space-y-1">
        <p class="text-sm font-medium">Enable edge updates</p>
        <p class="text-xs text-muted-foreground">Get new versions before everyone else. Edge builds can have rough edges.</p>
      </div>
      <Switch
        :model-value="update.channel === 'edge'"
        :disabled="choosing || !settings"
        @update:model-value="setBetaUpdates"
      />
    </div>
  </section>
</template>
