<script setup lang="ts">
import { useQueryClient } from '@tanstack/vue-query'
import { computed, type Ref, ref, watch } from 'vue'
import { commands, type PathStatus, type Settings } from '@/bindings'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Separator } from '@/components/ui/separator'
import { Spinner } from '@/components/ui/spinner'
import { usePlatformContext } from '@/composables/usePlatformContext'
import { useSaveSettings } from '@/composables/useSaveSettings'
import { keys } from '@/lib/keys'
import { useAppInfo, useFolderStatuses, useIntegrationsQuery, useSettings, useStatus } from '@/lib/queries'
import { automaticLogStatus } from '@/lib/storage'
import AutomaticLogLine from './AutomaticLogLine.vue'

// 0.x FilePathsCard. Each folder saves on Enter, Save or a Browse pick; a
// blank folder (or "Use automatic", v1's) goes back to detection. Saving a
// path never stops the watcher (release spec ruling 6).
type FieldKey = 'logFolder' | 'mtgoDataFolder'

const fields: Array<{ key: FieldKey; label: string; hint: string }> = [
  { key: 'logFolder', label: 'Log File Directory', hint: 'Contains mtgo.log files' },
  { key: 'mtgoDataFolder', label: 'Game Data Directory', hint: 'Contains Match_GameLog_* and deck XML files' },
]

const queryClient = useQueryClient()
const { data: settings } = useSettings()
const { data: status } = useStatus()
const { data: statuses } = useFolderStatuses()
const { save } = useSaveSettings()
const { platform } = usePlatformContext()
const { data: integrations } = useIntegrationsQuery()
const { data: info } = useAppInfo()
// The Arena block shows only while Arena is on (spec 8.1); on until the answer arrives.
const arenaOn = computed(() => integrations.value?.arena !== false)

const inputs: Record<FieldKey, Ref<string>> = {
  logFolder: ref(''),
  mtgoDataFolder: ref(''),
}
const processing = ref<FieldKey | null>(null)
// The folder dialog is open: the field is busy, but nothing is being saved.
const browsing = ref<FieldKey | null>(null)

function busy(key: FieldKey): boolean {
  return processing.value === key || browsing.value === key
}

function saved(key: FieldKey): string {
  return settings.value?.[key] ?? ''
}

// A field follows its stored folder whenever that changes (a save, a push).
for (const field of fields) {
  watch(
    () => saved(field.key),
    (folder) => {
      inputs[field.key].value = folder
    },
    { immediate: true },
  )
}

function chosenStatus(key: FieldKey): PathStatus | null {
  const current = statuses.value
  if (!current) return null
  return key === 'logFolder' ? current.log : current.data
}

async function saveField(key: FieldKey, folder: string) {
  // Build from what is cached now: another save may have landed since this page read it.
  const latest = queryClient.getQueryData<Settings>(keys.settings) ?? settings.value
  if (!latest) return
  processing.value = key
  try {
    await save({ ...latest, [key]: folder.trim() === '' ? null : folder })
    await queryClient.invalidateQueries({ queryKey: keys.folderStatuses })
  } finally {
    processing.value = null
  }
}

async function browse(key: FieldKey) {
  browsing.value = key
  let picked: string | null = null
  try {
    picked = await commands.chooseFolder(inputs[key].value || null)
  } catch {
    picked = null
  }
  browsing.value = null
  if (picked) {
    inputs[key].value = picked
    await saveField(key, picked)
  }
}
</script>

<template>
  <section class="flex flex-col gap-4 rounded-lg border bg-card p-4">
    <div class="space-y-1">
      <h2 class="text-sm font-semibold">File Paths</h2>
      <p v-if="platform === 'arena' && arenaOn" class="text-xs text-muted-foreground">
        Where MTG Arena's log is found. It is found automatically for standard installs.
      </p>
      <template v-else>
        <p class="text-xs text-muted-foreground">
          Where to look for MTGO log files and game data. Defaults are set automatically for standard installs.
        </p>
        <p v-if="info?.os === 'macos'" class="text-xs text-muted-foreground">
          MTGO under CrossOver, Whisky or another Wine wrapper is found automatically. In a Windows virtual machine,
          share its MTGO folder with your Mac and choose it below.
        </p>
      </template>
    </div>
    <div v-if="platform === 'arena' && arenaOn" class="flex flex-col gap-2">
      <Label>Arena Log File</Label>
      <p class="text-sm text-muted-foreground">Player.log, written by MTG Arena while it runs</p>
      <AutomaticLogLine :status="automaticLogStatus(status, 'arena')" />
    </div>
    <template v-for="(field, index) in fields" v-else :key="field.key">
      <Separator v-if="index > 0" />
      <div class="flex flex-col gap-2">
        <Label>{{ field.label }}</Label>
        <p class="text-sm text-muted-foreground">{{ field.hint }}</p>
        <div class="flex gap-2">
          <Input
            v-model="inputs[field.key].value"
            placeholder="Automatic"
            :disabled="busy(field.key)"
            @keydown.enter="saveField(field.key, inputs[field.key].value)"
          />
          <Button variant="outline" :disabled="busy(field.key)" @click="browse(field.key)">Browse</Button>
          <Button
            variant="outline"
            :disabled="busy(field.key) || inputs[field.key].value === saved(field.key)"
            @click="saveField(field.key, inputs[field.key].value)"
          >
            <Spinner v-if="processing === field.key" />
            {{ processing === field.key ? 'Saving...' : 'Save' }}
          </Button>
          <Button
            v-if="saved(field.key)"
            variant="ghost"
            :disabled="busy(field.key)"
            @click="saveField(field.key, '')"
          >
            Use automatic
          </Button>
        </div>
        <div v-if="saved(field.key) && chosenStatus(field.key)" class="flex items-center gap-2">
          <div
            class="size-2 shrink-0 rounded-full"
            :class="chosenStatus(field.key)?.valid ? 'bg-primary' : 'bg-destructive'"
          />
          <span class="text-sm" :class="chosenStatus(field.key)?.valid ? 'text-muted-foreground' : 'text-destructive'">
            {{ chosenStatus(field.key)?.message }}
          </span>
        </div>
        <AutomaticLogLine
          v-else-if="!saved(field.key) && field.key === 'logFolder'"
          :status="automaticLogStatus(status, 'mtgo')"
        />
      </div>
    </template>
  </section>
</template>
