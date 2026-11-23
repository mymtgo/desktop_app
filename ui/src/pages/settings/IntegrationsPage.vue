<script setup lang="ts">
import { useIntervalFn, useNow } from '@vueuse/core'
import { computed, ref } from 'vue'
import { commands, type IntegrationView } from '@/bindings'
import { Button } from '@/components/ui/button'
import { Label } from '@/components/ui/label'
import { Switch } from '@/components/ui/switch'
import { useToast } from '@/composables/useToast'
import { errorMessage, unwrap } from '@/lib/commands'
import {
  ARENA_OFF_TEXT,
  ARENA_ON_TEXT,
  browserInstalls,
  endstepStatusText,
  firstMatchText,
  FLATPAK_TEXT,
  STORE_LINKS,
  type StoreLinks,
} from '@/lib/integrations'
import { keys } from '@/lib/keys'
import { onAccountChanged } from '@/lib/liveEvents'
import { useIntegrationsQuery } from '@/lib/queries'
import { queryClient } from '@/queryClient'

// The Integrations settings page (spec 2026-11-10 section 8.1): one card per
// platform. MTGO is the app's core and is not listed.
const props = withDefaults(defineProps<{ storeLinks?: StoreLinks }>(), { storeLinks: () => STORE_LINKS })
const { data: view, refetch } = useIntegrationsQuery()
const { add } = useToast()
const now = useNow({ scheduler: (tick) => useIntervalFn(tick, 30_000) })
const busy = ref(false)

const arenaText = computed(() => (view.value?.arena ? ARENA_ON_TEXT : ARENA_OFF_TEXT))
const endstepText = computed(() =>
  view.value ? endstepStatusText(view.value.endstep, view.value.endstepStatus, now.value.getTime()) : '',
)
const installs = computed(() => browserInstalls(props.storeLinks))
const showInstallSteps = computed(() => !!view.value && view.value.endstep && !view.value.endstepStatus.helperMissing)
const showCheckAgain = computed(() => showInstallSteps.value && view.value?.endstepStatus.connection.kind === 'neverSeen')
const firstMatch = computed(() =>
  view.value && view.value.endstep && !view.value.endstepStatus.helperMissing
    ? firstMatchText(view.value.endstepStatus)
    : null,
)

// The backend only emits SettingsChanged, which does not refetch the platform
// context, so the header's platform switch is refreshed here.
async function toggle(integration: IntegrationView, on: boolean) {
  busy.value = true
  try {
    queryClient.setQueryData(keys.integrations, unwrap(await commands.setIntegration(integration, on)))
    await queryClient.invalidateQueries({ queryKey: keys.platformContext })
    // The platform being viewed may have just been switched off: refetch the
    // pages the way an account change does.
    if (!on) onAccountChanged()
  } catch (error) {
    add({ type: 'error', title: 'Could not save the integration', message: errorMessage(error) })
  } finally {
    busy.value = false
  }
}

async function openListing(url: string) {
  try {
    unwrap(await commands.openExternal(url))
  } catch (error) {
    add({ type: 'error', title: 'Could not open the store page', message: errorMessage(error) })
  }
}

function checkAgain() {
  void refetch()
}
</script>

<template>
  <div v-if="view" class="flex flex-col gap-3">
    <section class="flex flex-col gap-2 rounded-lg border bg-card p-4">
      <div class="flex items-center justify-between gap-4">
        <div class="space-y-1">
          <Label>MTG Arena</Label>
          <p class="text-sm text-muted-foreground" data-testid="arena-status">{{ arenaText }}</p>
        </div>
        <Switch :model-value="view.arena" :disabled="busy" @update:model-value="(on: boolean) => toggle('arena', on)" />
      </div>
    </section>

    <section class="flex flex-col gap-3 rounded-lg border bg-card p-4">
      <div class="flex items-center justify-between gap-4">
        <div class="space-y-1">
          <Label>Endstep</Label>
          <p class="text-sm text-muted-foreground" data-testid="endstep-status">{{ endstepText }}</p>
          <p v-if="firstMatch" class="text-sm text-muted-foreground" data-testid="endstep-first-match">{{ firstMatch }}</p>
        </div>
        <Switch
          :model-value="view.endstep"
          :disabled="busy"
          @update:model-value="(on: boolean) => toggle('endstep', on)"
        />
      </div>
      <template v-if="showInstallSteps">
        <div v-if="showCheckAgain" class="flex">
          <Button variant="outline" size="sm" @click="checkAgain">Check again</Button>
        </div>
        <div v-for="install in installs" :key="install.title" class="space-y-1 text-sm">
          <p class="font-medium">{{ install.title }}</p>
          <div v-if="install.store" class="flex items-center gap-2">
            <Button size="sm" @click="openListing(install.store.url)">{{ install.store.label }}</Button>
            <span v-if="install.store.hint" class="text-xs text-muted-foreground">{{ install.store.hint }}</span>
          </div>
          <ol v-else class="list-decimal space-y-0.5 pl-5 text-muted-foreground">
            <li v-for="step in install.steps" :key="step">{{ step }}</li>
          </ol>
        </div>
        <p class="text-xs text-muted-foreground">{{ FLATPAK_TEXT }}</p>
      </template>
    </section>
  </div>
</template>
