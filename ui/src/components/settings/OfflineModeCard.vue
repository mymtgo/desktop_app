<script setup lang="ts">
import { useIntervalFn, useNow } from '@vueuse/core'
import { computed, onMounted, ref } from 'vue'
import DataDisclosure from '@/components/settings/DataDisclosure.vue'
import OfflineArchetypes from '@/components/settings/OfflineArchetypes.vue'
import OfflineModeRejoinDialog from '@/components/settings/OfflineModeRejoinDialog.vue'
import { Button } from '@/components/ui/button'
import { Label } from '@/components/ui/label'
import { Separator } from '@/components/ui/separator'
import { Spinner } from '@/components/ui/spinner'
import { Switch } from '@/components/ui/switch'
import { commands, events, type PendingReports } from '@/bindings'
import { useOfflineModeSwitch } from '@/composables/useOfflineMode'
import { useSaveSettings } from '@/composables/useSaveSettings'
import { useTauriEvents } from '@/composables/useTauriEvents'
import {
  archetypeCatalogWarning,
  OFFLINE_COOLDOWN_TEXT,
  OFFLINE_COOLDOWN_TITLE,
  pendingMatchesText,
  rejoinBlocked,
  showPendingMatches,
  submitPendingLabel,
} from '@/lib/api'
import { useSettings } from '@/lib/queries'

// 0.x OfflineModeCard, with its warning while no downloaded archetype is
// held (0.x `hasArchetypeCatalog`), and offline the archetype download
// (`OfflineArchetypes`) that fixes it. The pending matches line and its submit button show only while match reports
// are live (MATCH_REPORTS_LIVE, on since #9) and offline mode is off.
// "Share anonymous usage data" (usage analytics spec): a stored setting,
// on by default; offline mode stops it as well.
const { data: settings } = useSettings()
const { saving: savingUsage, save } = useSaveSettings()

function setShareUsage(on: boolean) {
  if (settings.value) void save({ ...settings.value, shareUsage: on })
}

const now = useNow({ scheduler: (update) => useIntervalFn(update, 60_000) })

const blocked = computed(() => rejoinBlocked(state.value, now.value.getTime()))

const hasArchetypes = ref<boolean | null>(null)
const catalogWarning = computed(() => archetypeCatalogWarning(hasArchetypes.value, state.value?.enabled ?? false))

const heldOffline = ref<boolean | null>(null)

async function loadArchetypes() {
  const answer = await commands.hasArchetypeDefinitions()
  hasArchetypes.value = answer.status === 'ok' ? answer.data : null
  const held = await commands.hasOfflineArchetypes()
  heldOffline.value = held.status === 'ok' ? held.data : null
}

// The warning reloads after every switch: rejoining downloads the archetypes.
const { state, dialogOpen, switching, problem, load, toggle, confirmRejoin } = useOfflineModeSwitch(loadArchetypes)

const pending = ref<PendingReports | null>(null)
const submitting = ref(false)
const pendingShown = computed(() => showPendingMatches(pending.value, state.value?.enabled ?? true))

async function loadPending() {
  const answer = await commands.pendingMatchReports()
  pending.value = answer.status === 'ok' ? answer.data : null
}

async function submitPending() {
  submitting.value = true
  try {
    await commands.submitPendingMatches()
    await loadPending()
  } finally {
    submitting.value = false
  }
}

const listening = useTauriEvents(() => [
  events.offlineModeChanged.listen((event) => {
    state.value = event.payload.state
    void loadArchetypes()
  }),
])
onMounted(async () => {
  if (!(await listening.ready)) return
  await load()
  await loadPending()
  await loadArchetypes()
})
</script>

<template>
  <section class="flex flex-col gap-4 rounded-lg border bg-card p-4">
    <div class="space-y-1">
      <h2 class="text-sm font-semibold">Offline mode</h2>
      <p class="text-xs text-muted-foreground">Control what data is collected from your use of the app.</p>
    </div>
    <div class="flex items-center justify-between gap-4">
      <div class="space-y-1">
        <Label>Offline mode</Label>
        <p class="text-sm text-muted-foreground">
          Your matches stay private and no community data comes in, so you won't get archetype estimates or updates (you can
          download the archetypes once instead), metagame shares, opponent scouting, tournament names, replay sharing, the published overlay or cloud sync. Card data still refreshes so your decks keep working, and
          update checks and error reports still go out.
        </p>
      </div>
      <Switch
        :model-value="state?.enabled ?? false"
        :disabled="state === null || switching || blocked"
        :title="blocked ? OFFLINE_COOLDOWN_TITLE : undefined"
        @update:model-value="toggle"
      />
    </div>
    <p v-if="blocked" class="text-xs text-warning">{{ OFFLINE_COOLDOWN_TEXT }}</p>
    <p v-if="catalogWarning" class="text-xs text-warning">{{ catalogWarning }}</p>
    <p v-if="problem" class="text-xs text-destructive">{{ problem }}</p>
    <OfflineArchetypes :offline="state?.enabled ?? false" :held-offline="heldOffline" @downloaded="loadArchetypes" />
    <div v-if="pendingShown && pending" class="flex items-center justify-between">
      <p class="text-sm text-muted-foreground">{{ pendingMatchesText(pending.count) }}</p>
      <Button variant="outline" size="sm" :disabled="submitting || pending.count === 0" @click="submitPending">
        <Spinner v-if="submitting" />
        {{ submitPendingLabel(pending.count, submitting) }}
      </Button>
    </div>
    <div v-if="settings" class="flex items-center justify-between gap-4">
      <div class="space-y-1">
        <Label>Share anonymous usage data</Label>
        <p class="text-sm text-muted-foreground">
          Counts which pages and features get used, to help decide what to improve. No account, username, deck or match data is
          ever sent, and nothing at all while offline mode is on.
        </p>
      </div>
      <Switch :model-value="settings.shareUsage" :disabled="savingUsage" @update:model-value="setShareUsage" />
    </div>
    <Separator />
    <DataDisclosure />

    <OfflineModeRejoinDialog v-model:open="dialogOpen" :submitting="switching" @confirm="confirmRejoin" />
  </section>
</template>
