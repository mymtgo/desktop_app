<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from 'vue'
import RecorderNotice from '@/components/settings/RecorderNotice.vue'
import { Button } from '@/components/ui/button'
import { Label } from '@/components/ui/label'
import { Switch } from '@/components/ui/switch'
import { commands, type CommandError, type RecorderStatusView } from '@/bindings'
import {
  recorderFileUrl,
  recorderStatusAction,
  recorderStatusText,
  recorderSwitchOn,
  UNSUPPORTED_TEXT,
} from '@/lib/recorder'
import { useAppInfo } from '@/lib/queries'

// The Replays settings page (spec 2026-11-09 section 4). There is no push
// event from the recorder, so the status is polled while the page is open.
const POLL_MS = 5000

const { data: info } = useAppInfo()
const view = ref<RecorderStatusView | null>(null)
const noticeOpen = ref(false)
const busy = ref(false)
const problem = ref<string | null>(null)
/** A failed accept, shown inside the notice (the page behind it is hidden). */
const noticeProblem = ref<string | null>(null)
let timer: ReturnType<typeof setInterval> | null = null

const windows = computed(() => info.value?.os === 'windows')
const statusText = computed(() => {
  if (!windows.value) return UNSUPPORTED_TEXT
  return view.value ? recorderStatusText(view.value.status) : ''
})
const action = computed(() => (windows.value && view.value ? recorderStatusAction(view.value.status) : null))
const switchOn = computed(() => (view.value ? recorderSwitchOn(view.value.status) : false))
const noBuild = computed(() => view.value?.status === 'noBuild')

function describe(error: CommandError): string {
  return typeof error === 'string' ? error : JSON.stringify(error)
}

/** Run a command; its failure (an error answer or a rejected invoke) comes back as text, `null` on success. */
async function run(
  call: () => Promise<{ status: 'ok'; data: RecorderStatusView } | { status: 'error'; error: CommandError }>,
): Promise<string | null> {
  busy.value = true
  problem.value = null
  try {
    const answer = await call()
    if (answer.status === 'ok') {
      view.value = answer.data
      return null
    }
    problem.value = describe(answer.error)
  } catch (error) {
    problem.value = String(error)
  } finally {
    busy.value = false
  }
  return problem.value
}

async function poll() {
  try {
    const answer = await commands.recorderStatus()
    if (answer.status === 'ok') view.value = answer.data
  } catch {
    // The next poll tries again.
  }
}

function toggle(on: boolean) {
  if (!on) {
    void run(() => commands.setRecorderEnabled(false))
  } else if (noBuild.value) {
    // The switch is disabled; nothing to download, so the notice never opens.
  } else if (view.value?.needsAcceptance) {
    noticeProblem.value = null
    noticeOpen.value = true
  } else {
    void run(() => commands.setRecorderEnabled(true))
  }
}

async function accept() {
  noticeProblem.value = null
  const failed = await run(() => commands.acceptRecorderNotice())
  if (failed === null) noticeOpen.value = false
  else noticeProblem.value = failed
}

function openLink(file: string) {
  if (view.value) void commands.openExternal(recorderFileUrl(view.value.version, file))
}

function retry() {
  void run(() => commands.retryRecorder())
}

function openFolder() {
  void commands.openRecorderFolder()
}

onMounted(() => {
  void poll()
  timer = setInterval(() => void poll(), POLL_MS)
})
onUnmounted(() => {
  if (timer !== null) clearInterval(timer)
})
</script>

<template>
  <div class="flex flex-col gap-3">
    <section class="flex flex-col gap-4 rounded-lg border bg-card p-4">
      <div class="space-y-1">
        <h2 class="text-sm font-semibold">Replays</h2>
        <p class="text-xs text-muted-foreground">
          An optional recorder saves a replay of each MTGO match you play. MyMTGO works fully without it.
        </p>
      </div>
      <div class="flex items-center justify-between gap-4">
        <div class="space-y-1">
          <Label>Record MTGO replays</Label>
          <p v-if="view" class="text-sm text-muted-foreground" data-testid="recorder-status">{{ statusText }}</p>
        </div>
        <Switch
          :model-value="switchOn"
          :disabled="!windows || view === null || noBuild || busy"
          @update:model-value="toggle"
        />
      </div>
      <div v-if="action" class="flex">
        <Button variant="outline" size="sm" :disabled="busy" @click="retry">{{ action }}</Button>
      </div>
      <p v-if="problem" class="text-xs text-destructive">{{ problem }}</p>
    </section>

    <section v-if="view" class="flex flex-col gap-3 rounded-lg border bg-card p-4">
      <div class="flex items-center justify-between gap-4">
        <div class="min-w-0 space-y-1">
          <Label>Recorder folder</Label>
          <p class="break-all text-sm text-muted-foreground">{{ view.folder }}</p>
        </div>
        <Button variant="outline" size="sm" @click="openFolder">Open folder</Button>
      </div>
      <p class="text-sm text-muted-foreground">
        Replays imported: <span class="font-medium text-foreground">{{ view.imported }}</span>
      </p>
      <div class="flex flex-wrap gap-x-4 gap-y-1">
        <Button variant="link" size="sm" class="h-auto p-0" @click="openLink('NOTICE')">Recorder NOTICE</Button>
        <Button variant="link" size="sm" class="h-auto p-0" @click="openLink('THIRD-PARTY-NOTICES.md')">
          Recorder third-party notices
        </Button>
      </div>
    </section>

    <RecorderNotice
      v-model:open="noticeOpen"
      :version="view?.version ?? ''"
      :submitting="busy"
      :error="noticeProblem"
      @accept="accept"
    />
  </div>
</template>
