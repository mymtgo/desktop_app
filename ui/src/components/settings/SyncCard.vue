<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from 'vue'
import { Button } from '@/components/ui/button'
import { Separator } from '@/components/ui/separator'
import { Spinner } from '@/components/ui/spinner'
import { commands, events, type AccountStatus, type CloudCounts, type SlotCounts, type SyncStatus } from '@/bindings'
import {
  accountDot,
  accountStateText,
  canSignIn,
  cloudText,
  deckListText,
  deckSlotsText,
  formatCounts,
  lastErrorText,
  lastSyncedText,
  notSyncedText,
  rejectionsText,
  showDeckSlots,
  signInLabel,
  syncNowLabel,
  uploadCounts,
} from '@/lib/api'
import { useSyncRun } from '@/composables/useSyncRun'
import { useTauriEvents } from '@/composables/useTauriEvents'
import { errorMessage, unwrap } from '@/lib/commands'

// 0.x SyncCard. The sign-in waits in the background; the app emits
// `apiChanged` when it starts, ends or the device signs out. A sync run is
// watched with a 3 s poll until it ends (or 240 s pass).
const emit = defineEmits<{ 'update:linked': [value: boolean] }>()

const status = ref<AccountStatus | null>(null)
const sync = ref<SyncStatus | null>(null)
const cloud = ref<CloudCounts | null>(null)
const cloudLoading = ref(false)
const loading = ref(false)
const opening = ref(false)
const signingOut = ref(false)
const problem = ref<string | null>(null)
const activityBox = ref<HTMLElement | null>(null)

watch(
  () => status.value?.linked,
  (value) => emit('update:linked', value === true),
)

/** Whether the status could be read. */
async function refresh(options: { silent?: boolean } = {}): Promise<boolean> {
  if (!options.silent) loading.value = true
  try {
    status.value = unwrap(await commands.accountStatus())
    sync.value = unwrap(await commands.syncStatus())
    return true
  } catch (error) {
    problem.value = errorMessage(error)
    return false
  } finally {
    loading.value = false
  }
}

// Asks the server what it holds; never part of the poll, so a background
// card never hammers the API.
async function fetchCloud() {
  cloudLoading.value = true
  try {
    cloud.value = unwrap(await commands.syncCloudCounts())
  } catch {
    // Keep the previous numbers.
  } finally {
    cloudLoading.value = false
  }
}

async function refreshAll() {
  await Promise.all([refresh(), fetchCloud()])
}

// The press and the poll behind "Sync now" (see `useSyncRun`).
const {
  running,
  problem: syncProblem,
  press: runSync,
  watchRun,
  stop: stopPolling,
} = useSyncRun({
  start: async () => (sync.value = unwrap(await commands.syncNow())),
  poll: async () => {
    return (await refresh({ silent: true })) ? sync.value : null
  },
  onEnd: () => void fetchCloud(),
})

async function signIn() {
  problem.value = null
  opening.value = true
  try {
    unwrap(await commands.signIn())
  } catch (error) {
    problem.value = errorMessage(error)
  } finally {
    opening.value = false
    await refresh()
  }
}

async function cancel() {
  await commands.cancelSignIn()
}

async function signOut() {
  problem.value = null
  signingOut.value = true
  try {
    unwrap(await commands.signOut())
  } catch (error) {
    problem.value = errorMessage(error)
  } finally {
    signingOut.value = false
    await refresh()
  }
}

// The server's live ledger wins over the copy the last run wrote down.
const deckSlots = computed<SlotCounts | null>(() => cloud.value?.slots ?? sync.value?.slots ?? null)

// Keep the activity console on its newest line as the poll appends.
watch(
  () => sync.value?.activity.length ?? 0,
  async () => {
    await nextTick()
    activityBox.value?.scrollTo({ top: activityBox.value.scrollHeight })
  },
)

const listening = useTauriEvents(() => [events.apiChanged.listen(() => refresh({ silent: true }))])
onMounted(async () => {
  if (!(await listening.ready)) return
  await refresh()
  void fetchCloud()
  // A run started elsewhere (the half-hourly schedule, a match completing)
  // is watched the same way.
  if (sync.value?.syncing) watchRun()
})
onUnmounted(() => stopPolling())
</script>

<template>
  <section class="flex flex-col gap-4 rounded-lg border bg-card p-4">
    <div class="space-y-1">
      <h2 class="text-sm font-semibold">MyMTGO account</h2>
      <p class="text-xs text-muted-foreground">
        Sign in to MyMTGO to keep your matches backed up and in sync across devices.
      </p>
    </div>

    <div class="flex items-center justify-between">
      <p class="flex items-center gap-2 text-sm">
        <span class="size-2 shrink-0 rounded-full" :class="accountDot(status)" />
        <span class="text-muted-foreground">{{ accountStateText(status) }}</span>
      </p>
      <div class="flex items-center gap-2">
        <template v-if="status !== null && !status.linked">
          <Button
            variant="outline"
            size="sm"
            :disabled="!canSignIn(status, opening)"
            :title="status.offline ? 'Turn off offline mode to sign in.' : undefined"
            @click="signIn"
          >
            <Spinner v-if="opening || status.signingIn" />
            {{ signInLabel(status, opening) }}
          </Button>
          <Button v-if="status.signingIn" variant="ghost" size="sm" @click="cancel">Cancel</Button>
        </template>
        <Button variant="outline" size="sm" :disabled="loading || cloudLoading" @click="refreshAll">
          <Spinner v-if="loading" />
          {{ loading ? 'Checking...' : 'Refresh' }}
        </Button>
      </div>
    </div>

    <p v-if="status !== null && !status.linked" class="text-xs text-muted-foreground">
      Signing in opens MyMTGO in your browser. Sign in or create a free account, then approve this device.
    </p>
    <p v-if="status !== null && !status.configured" class="text-xs text-muted-foreground">
      Sign-in is not configured in this build.
    </p>
    <p v-if="status?.offline" class="text-xs text-warning">
      Offline mode is on. Turn it off in Data &amp; Privacy to sync.
    </p>
    <p v-if="problem ?? syncProblem ?? status?.error" class="text-xs text-destructive">{{ problem ?? syncProblem ?? status?.error }}</p>

    <template v-if="status?.linked && sync">
      <Separator />

      <div class="flex flex-col gap-2">
        <div class="flex items-center justify-between">
          <span class="text-sm text-muted-foreground">Last synced</span>
          <span class="text-sm">{{ lastSyncedText(sync.lastSyncedAtMs) }}</span>
        </div>
        <div class="flex flex-col gap-1">
          <div class="flex items-center justify-between">
            <span class="text-sm text-muted-foreground">Deck list</span>
            <span class="text-sm">{{ deckListText(sync) }}</span>
          </div>
          <p class="text-xs text-muted-foreground">Your deck list always syncs, on every plan.</p>
        </div>
        <div class="flex items-center justify-between">
          <span class="text-sm text-muted-foreground">Ready to upload</span>
          <span class="text-sm">{{ formatCounts(uploadCounts(sync), 'Nothing') }}</span>
        </div>
        <div class="flex items-center justify-between">
          <span class="text-sm text-muted-foreground">Waiting in the cloud</span>
          <span class="text-sm">{{ cloudText(cloudLoading, cloud?.download ?? null) }}</span>
        </div>
        <div v-if="showDeckSlots(deckSlots)" class="flex flex-col gap-1">
          <div class="flex items-center justify-between">
            <span class="text-sm text-muted-foreground">Deck slots</span>
            <span class="text-sm">{{ deckSlotsText(deckSlots) }}</span>
          </div>
          <p class="text-xs text-muted-foreground">
            Your slot is for constructed decks. Choose which one syncs below. Supporter lifts the limit and adds draft
            and sealed decks.
          </p>
        </div>
        <p v-if="sync.notSynced > 0" class="text-xs text-warning">{{ notSyncedText(sync.notSynced) }}</p>
        <p v-if="sync.rejections > 0" class="text-xs text-destructive">{{ rejectionsText(sync.rejections) }}</p>
        <p v-if="sync.lastError" class="text-xs text-destructive">{{ lastErrorText(sync.lastError) }}</p>

        <div
          v-if="sync.activity.length > 0"
          ref="activityBox"
          class="max-h-40 overflow-auto rounded-lg border border-border bg-background p-3"
        >
          <pre class="text-xs leading-relaxed text-muted-foreground">{{ sync.activity.join('\n') }}</pre>
        </div>
      </div>

      <Separator />

      <div class="flex items-center justify-between">
        <Button
          variant="outline"
          size="sm"
          :disabled="running || status.offline"
          :title="status.offline ? 'Turn off offline mode to sync.' : undefined"
          @click="runSync"
        >
          <Spinner v-if="running" />
          {{ syncNowLabel(running) }}
        </Button>
        <Button variant="ghost" size="sm" :disabled="signingOut" @click="signOut">
          <Spinner v-if="signingOut" />
          {{ signingOut ? 'Signing out...' : 'Sign out' }}
        </Button>
      </div>
      <p class="text-xs text-muted-foreground">
        You can also sign this device out from your account's Devices page on the website.
      </p>
    </template>
  </section>
</template>
