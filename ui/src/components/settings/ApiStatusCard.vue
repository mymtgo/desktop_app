<script setup lang="ts">
import { onMounted, ref } from 'vue'
import { RouterLink } from 'vue-router'
import SettingsCard from '@/components/SettingsCard.vue'
import { Button } from '@/components/ui/button'
import { Spinner } from '@/components/ui/spinner'
import { commands, type ApiStatus } from '@/bindings'
import { apiStatusDot, apiStatusText, apiStatusTone } from '@/lib/api'
import { errorMessage } from '@/lib/commands'

// 0.x ApiStatusCard: checked on mount and by "Recheck", never polled.
const status = ref<ApiStatus | null>(null)
const checking = ref(false)
const reauthenticating = ref(false)

async function run(flag: typeof checking, ask: () => Promise<ApiStatus>) {
  flag.value = true
  try {
    status.value = await ask()
  } catch (error) {
    status.value = { state: 'unreachable', error: errorMessage(error) }
  } finally {
    flag.value = false
  }
}

const check = () => run(checking, commands.apiStatus)
const reauthenticate = () => run(reauthenticating, commands.reauthenticate)

onMounted(check)
</script>

<template>
  <SettingsCard title="API Status" description="Connection status to the mymtgo API.">
    <template #details>
      <p class="flex items-center gap-2 pt-1 text-sm">
        <span class="size-2 shrink-0 rounded-full" :class="apiStatusDot(status)" />
        <span :class="apiStatusTone(status)">{{ apiStatusText(status) }}</span>
      </p>
      <pre
        v-if="status?.state === 'unreachable'"
        class="max-h-40 overflow-auto rounded-md border border-destructive/40 bg-destructive/5 p-3 text-xs break-words whitespace-pre-wrap text-destructive"
      >{{ status.error }}</pre>
    </template>
    <Button v-if="status?.state === 'noauth' && status.signedIn" variant="outline" size="sm" as-child>
      <RouterLink to="/settings/account">Sign in again</RouterLink>
    </Button>
    <Button
      v-else-if="status?.state === 'noauth'"
      variant="outline"
      size="sm"
      :disabled="reauthenticating"
      @click="reauthenticate"
    >
      <Spinner v-if="reauthenticating" />
      {{ reauthenticating ? 'Reauthenticating…' : 'Reauthenticate' }}
    </Button>
    <Button variant="outline" size="sm" :disabled="checking || reauthenticating" @click="check">
      <Spinner v-if="checking" />
      {{ checking ? 'Checking…' : 'Recheck' }}
    </Button>
  </SettingsCard>
</template>
