<script setup lang="ts">
import { useQuery, useQueryClient } from '@tanstack/vue-query'
import { computed, ref } from 'vue'
import { Button } from '@/components/ui/button'
import { Label } from '@/components/ui/label'
import { Spinner } from '@/components/ui/spinner'
import { commands } from '@/bindings'
import { MISSING_FOLDER_TEXT, cardImagesPollInterval, folderButtonLabel } from '@/lib/cards'
import { unwrap, type CommandResult } from '@/lib/commands'
import { keys } from '@/lib/keys'

// 0.x LocalImagesCard. Card images are always saved on this machine (the
// cards worker downloads them, and pages fall back to the remote image for a
// card that has none yet); the card shows where they are and moves them. The
// folder move runs on the same worker; the card polls while it runs.
const queryClient = useQueryClient()
const { data: status } = useQuery({
  queryKey: keys.cardImages,
  queryFn: async () => unwrap(await commands.cardImagesStatus()),
  refetchInterval: (query) => cardImagesPollInterval(query.state.data),
})
const choosing = ref(false)
const folderFailure = ref<string | null>(null)
const moving = computed(() => status.value?.folder.moving ?? false)
const busy = computed(() => moving.value || choosing.value)
const folderError = computed(() => folderFailure.value ?? status.value?.folder.error ?? null)

async function moveWith(send: () => Promise<CommandResult<unknown>>) {
  folderFailure.value = null
  const answer = await send()
  if (answer.status === 'error') folderFailure.value = answer.error.message
  await queryClient.invalidateQueries({ queryKey: keys.cardImages })
}

async function browse() {
  choosing.value = true
  try {
    const chosen = await commands.chooseCardImagesFolder()
    if (chosen) await moveWith(() => commands.moveCardImages(chosen))
  } finally {
    choosing.value = false
  }
}

async function reset() {
  choosing.value = true
  try {
    await moveWith(() => commands.resetCardImagesPath())
  } finally {
    choosing.value = false
  }
}
</script>

<template>
  <section class="flex flex-col gap-4 rounded-lg border bg-card p-4">
    <div class="space-y-1">
      <h2 class="text-sm font-semibold">Card images</h2>
      <p class="text-xs text-muted-foreground">Where card imagery is stored on your machine.</p>
    </div>
    <p class="text-sm text-muted-foreground">
      Card images are saved on your machine, so they load fast and work offline. Cards that are not saved yet show
      from the web until they are.
    </p>
    <p v-if="status" class="text-sm text-muted-foreground">Current usage: {{ status.usage }}</p>

    <div v-if="status" class="flex flex-col gap-2">
      <Label>Image folder</Label>
      <p class="text-sm text-muted-foreground">
        Store images on another drive if your system drive is short on space. Existing images are moved across.
      </p>
      <div class="flex items-center gap-2">
        <code
          class="min-w-0 flex-1 truncate rounded-md border border-border bg-muted/40 px-3 py-2 text-sm"
          :title="status.folder.path"
        >
          {{ status.folder.path }}
        </code>
        <Button variant="outline" class="shrink-0" :disabled="busy" @click="browse">
          <Spinner v-if="busy" />
          {{ folderButtonLabel(moving) }}
        </Button>
        <Button v-if="!status.folder.isDefault" variant="ghost" class="shrink-0" :disabled="busy" @click="reset">
          Reset to default
        </Button>
      </div>
      <div v-if="status.folder.missing" class="flex items-center gap-2">
        <div class="size-2 shrink-0 rounded-full bg-destructive" />
        <span class="text-sm text-destructive">{{ MISSING_FOLDER_TEXT }}</span>
      </div>
      <p v-if="folderError" class="text-sm text-destructive">{{ folderError }}</p>
    </div>
  </section>
</template>
