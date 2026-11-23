<script setup lang="ts">
import { useIntervalFn, useNow } from '@vueuse/core'
import { computed, ref } from 'vue'
import SettingsCard from '@/components/SettingsCard.vue'
import { Button } from '@/components/ui/button'
import { commands } from '@/bindings'
import { queryClient } from '@/queryClient'
import { errorMessage, unwrap } from '@/lib/commands'
import { keys } from '@/lib/keys'
import {
  accountsLine,
  apiLine,
  ARCHETYPE_WAIT_NOTE,
  archetypesLine,
  deckSkipsLine,
  decksLine,
  leagueSkipsLine,
  leaguesLine,
  limitedLine,
  limitedSkipsLine,
  overlayLine,
  pathLine,
  importRunning,
  progressLine,
  progressPercent,
  replaysLine,
  skipsLine,
  summaryLines,
} from '@/lib/legacyImport'
import { useLegacyImport, useLegacyImportProgress } from '@/lib/queries'

const { data: info } = useLegacyImport()
const { data: progress } = useLegacyImportProgress()
const now = useNow({ scheduler: (tick) => useIntervalFn(tick, 30_000) })
const busy = ref(false)
const problem = ref<string | null>(null)

const path = computed(() => info.value?.chosenPath ?? info.value?.detectedPath ?? null)
const running = computed(() => importRunning(progress.value, info.value?.running === true))
const failure = computed(() => problem.value ?? (progress.value?.phase === 'failed' ? progress.value.message : null))
const percent = computed(() => (progress.value ? progressPercent(progress.value) : null))

async function run(action: () => Promise<void>) {
  busy.value = true
  problem.value = null
  try {
    await action()
  } catch (error) {
    problem.value = errorMessage(error)
  } finally {
    busy.value = false
  }
}

function chooseFile() {
  return run(async () => {
    const picked = await commands.pickLegacyDb()
    if (picked === null) return
    queryClient.setQueryData(keys.legacyImport, unwrap(await commands.chooseLegacyDb(picked)))
  })
}

function useDetected() {
  return run(async () => {
    queryClient.setQueryData(keys.legacyImport, unwrap(await commands.chooseLegacyDb(null)))
  })
}

function startImport() {
  return run(async () => {
    queryClient.setQueryData(keys.legacyImportProgress, { phase: 'reading' })
    try {
      unwrap(await commands.startLegacyImport())
    } catch (error) {
      queryClient.setQueryData(keys.legacyImportProgress, null)
      throw error
    }
  })
}
</script>

<template>
  <section v-if="info" class="space-y-2">
    <h2 class="text-sm font-semibold">MyMTGO 0.x</h2>
    <SettingsCard
      title="Import from MyMTGO 0.x"
      description="Bring your matches, decks, leagues and drafts over from the old app. Matches tracked from your MTGO logs, decks read from your MTGO deck files, and deck edits, league notes, links, drops and pick notes made here are never overwritten."
    >
      <template #details>
        <p class="text-xs">
          <code class="break-all select-all">{{ pathLine(info) }}</code>
        </p>
        <template v-if="running && progress">
          <p class="text-xs text-muted-foreground">{{ progressLine(progress) }}</p>
          <div v-if="percent !== null" class="h-1.5 w-full overflow-hidden rounded-full bg-muted">
            <div class="h-full bg-primary" :style="{ width: `${percent}%` }" />
          </div>
        </template>
        <p v-else-if="running" class="text-xs text-muted-foreground">Reading 0.x data...</p>
        <p v-if="running || info.lastImport" class="text-xs text-muted-foreground">{{ ARCHETYPE_WAIT_NOTE }}</p>
        <p v-if="failure" class="text-xs text-destructive">{{ failure }}</p>
        <template v-if="info.lastImport">
          <p class="text-xs text-muted-foreground">{{ summaryLines(info.lastImport, now.getTime()) }}</p>
          <p v-if="skipsLine(info.lastImport)" class="text-xs text-muted-foreground">{{ skipsLine(info.lastImport) }}</p>
          <p v-if="accountsLine(info.lastImport)" class="text-xs text-muted-foreground">
            {{ accountsLine(info.lastImport) }}
          </p>
          <p v-if="decksLine(info.lastImport)" class="text-xs text-muted-foreground">{{ decksLine(info.lastImport) }}</p>
          <p v-if="deckSkipsLine(info.lastImport)" class="text-xs text-muted-foreground">
            {{ deckSkipsLine(info.lastImport) }}
          </p>
          <p v-if="leaguesLine(info.lastImport)" class="text-xs text-muted-foreground">{{ leaguesLine(info.lastImport) }}</p>
          <p v-if="leagueSkipsLine(info.lastImport)" class="text-xs text-muted-foreground">
            {{ leagueSkipsLine(info.lastImport) }}
          </p>
          <p v-if="limitedLine(info.lastImport)" class="text-xs text-muted-foreground">{{ limitedLine(info.lastImport) }}</p>
          <p v-if="limitedSkipsLine(info.lastImport)" class="text-xs text-muted-foreground">
            {{ limitedSkipsLine(info.lastImport) }}
          </p>
          <p v-if="apiLine(info.lastImport)" class="text-xs text-muted-foreground">{{ apiLine(info.lastImport) }}</p>
          <p v-if="archetypesLine(info.lastImport)" class="text-xs text-muted-foreground">
            {{ archetypesLine(info.lastImport) }}
          </p>
          <p v-if="overlayLine(info.lastImport)" class="text-xs text-muted-foreground">
            {{ overlayLine(info.lastImport) }}
          </p>
          <p v-if="replaysLine(info.lastImport)" class="text-xs text-muted-foreground">
            {{ replaysLine(info.lastImport) }}
          </p>
          <p class="text-xs text-muted-foreground">
            You can uninstall MyMTGO 0.x now. Its data stays on this computer, so you can import again.
          </p>
        </template>
      </template>
      <Button size="sm" variant="outline" :disabled="busy || running" @click="chooseFile">Choose file</Button>
      <Button v-if="info.chosenPath" size="sm" variant="ghost" :disabled="busy || running" @click="useDetected">
        Use detected
      </Button>
      <Button size="sm" :disabled="busy || running || path === null" @click="startImport">Import</Button>
    </SettingsCard>
  </section>
</template>
