<script setup lang="ts">
/**
 * The offline archetype download (2026-10-27 offline download design), on
 * the offline mode card while offline mode is on. Both buttons do their work
 * only when pressed: Download archetypes fetches the whole archetype catalog
 * once and keeps it as Custom archetypes the player owns; Run detection asks
 * for every match's archetypes again against them.
 */
import { computed, ref, watch } from 'vue'
import { Button } from '@/components/ui/button'
import { Spinner } from '@/components/ui/spinner'
import { commands } from '@/bindings'

// `heldOffline`: archetypes from an offline download are held, so detection
// can use them (a catalog met online is not enough). The card reloads it
// after a download and after every switch.
const props = defineProps<{ offline: boolean; heldOffline: boolean | null }>()

const emit = defineEmits<{ downloaded: [] }>()

const downloading = ref(false)
const downloadNote = ref<string | null>(null)
const downloadProblem = ref<string | null>(null)

const detecting = ref(false)
const detectNote = ref<string | null>(null)
const detectProblem = ref<string | null>(null)

const detectionShown = computed(() => props.heldOffline === true)

// Going online releases every held row: coming back offline starts over.
watch(
  () => props.offline,
  (offline) => {
    if (offline) return
    downloadNote.value = null
    downloadProblem.value = null
    detectNote.value = null
    detectProblem.value = null
  },
)

async function download() {
  downloading.value = true
  downloadNote.value = null
  downloadProblem.value = null
  try {
    const answer = await commands.downloadOfflineArchetypes()
    if (answer.status === 'ok') {
      const held = answer.data.added + answer.data.converted
      downloadNote.value = `Downloaded ${held} archetypes. They are now Custom archetypes you can edit.`
      emit('downloaded')
    } else {
      downloadProblem.value = answer.error.message
    }
  } finally {
    downloading.value = false
  }
}

async function detect() {
  detecting.value = true
  detectNote.value = null
  detectProblem.value = null
  try {
    const answer = await commands.redetectOfflineMatches()
    if (answer.status === 'ok') {
      detectNote.value = `Detection queued for ${answer.data} matches.`
    } else {
      detectProblem.value = answer.error.message
    }
  } finally {
    detecting.value = false
  }
}
</script>

<template>
  <div v-if="props.offline" class="flex flex-col gap-3">
    <div class="flex items-center justify-between gap-4">
      <div class="space-y-1">
        <p class="text-sm font-medium">Archetypes</p>
        <p class="text-sm text-muted-foreground">
          Download every archetype once so detection works offline. They become Custom archetypes you can edit, and only your
          device key is sent to get them.
        </p>
      </div>
      <Button variant="outline" size="sm" :disabled="downloading" @click="download">
        <Spinner v-if="downloading" />
        {{ downloading ? 'Downloading archetypes...' : 'Download archetypes' }}
      </Button>
    </div>
    <p v-if="downloadNote" class="text-xs text-muted-foreground">{{ downloadNote }}</p>
    <p v-if="downloadProblem" class="text-xs text-destructive">{{ downloadProblem }}</p>

    <div v-if="detectionShown" class="flex items-center justify-between gap-4">
      <p class="text-sm text-muted-foreground">Archetypes you set by hand are never changed.</p>
      <Button variant="outline" size="sm" :disabled="detecting" @click="detect">
        <Spinner v-if="detecting" />
        Run detection against your matches now
      </Button>
    </div>
    <p v-if="detectNote" class="text-xs text-muted-foreground">{{ detectNote }}</p>
    <p v-if="detectProblem" class="text-xs text-destructive">{{ detectProblem }}</p>
  </div>
</template>
