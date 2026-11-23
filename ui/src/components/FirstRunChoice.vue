<script setup lang="ts">
import { DatabaseBackup, TriangleAlert } from 'lucide-vue-next'
import { computed, ref } from 'vue'
import { RouterLink, useRoute } from 'vue-router'
import { commands, type FirstRunChoice } from '@/bindings'
import { Button } from '@/components/ui/button'
import { Empty, EmptyContent, EmptyDescription, EmptyHeader, EmptyMedia, EmptyTitle } from '@/components/ui/empty'
import { errorMessage, unwrap } from '@/lib/commands'
import { FIRST_RUN_TEXT, IMPORT_PATH, failureMessage, firstRunScreen } from '@/lib/firstRun'
import { keys } from '@/lib/keys'
import { progressLine, progressPercent } from '@/lib/legacyImport'
import { useFirstRun, useLegacyImport, useLegacyImportProgress } from '@/lib/queries'
import { queryClient } from '@/queryClient'

// The first run's 0.x question (release spec section 13.2) in place of the
// page: the in-app choice, then the migration with its progress, error and
// "Try again". Once settled, the page (the default slot) shows. A failed
// migration leaves the Import page to itself and can be left for this run.

const { data: firstRun } = useFirstRun()
const { data: progress } = useLegacyImportProgress()
const { data: legacy } = useLegacyImport()
const route = useRoute()
const busy = ref(false)
const problem = ref<string | null>(null)

const screen = computed(() =>
  firstRunScreen(firstRun.value, progress.value, legacy.value?.running === true, route.path),
)
const failure = computed(() =>
  problem.value ?? (firstRun.value ? failureMessage(firstRun.value, progress.value) : null),
)
const percent = computed(() => (progress.value ? progressPercent(progress.value) : null))

async function choose(choice: FirstRunChoice) {
  busy.value = true
  problem.value = null
  try {
    if (choice === 'migrate') queryClient.setQueryData(keys.legacyImportProgress, { phase: 'reading' })
    queryClient.setQueryData(keys.firstRun, unwrap(await commands.chooseFirstRun(choice)))
  } catch (error) {
    if (choice === 'migrate') queryClient.setQueryData(keys.legacyImportProgress, null)
    // The answer may be saved with the import refused: show what the app has.
    void queryClient.invalidateQueries({ queryKey: keys.firstRun })
    problem.value = errorMessage(error)
  } finally {
    busy.value = false
  }
}

async function dismiss() {
  busy.value = true
  problem.value = null
  try {
    queryClient.setQueryData(keys.firstRun, unwrap(await commands.dismissFirstRun()))
  } catch (error) {
    problem.value = errorMessage(error)
  } finally {
    busy.value = false
  }
}
</script>

<template>
  <Empty v-if="screen === 'ask'" class="m-auto max-w-lg">
    <EmptyHeader>
      <EmptyMedia variant="icon"><DatabaseBackup /></EmptyMedia>
      <EmptyTitle>Welcome to MyMTGO</EmptyTitle>
      <EmptyDescription>{{ FIRST_RUN_TEXT.found }}</EmptyDescription>
    </EmptyHeader>
    <EmptyContent>
      <div class="flex gap-2">
        <Button :disabled="busy" @click="choose('migrate')">{{ FIRST_RUN_TEXT.migrate }}</Button>
        <Button variant="outline" :disabled="busy" @click="choose('fresh')">{{ FIRST_RUN_TEXT.fresh }}</Button>
      </div>
      <p v-if="problem" class="text-xs text-destructive">{{ problem }}</p>
    </EmptyContent>
  </Empty>
  <Empty v-else-if="screen === 'importing'" class="m-auto max-w-lg">
    <EmptyHeader>
      <EmptyMedia variant="icon"><DatabaseBackup /></EmptyMedia>
      <EmptyTitle>Importing from MyMTGO 0.x</EmptyTitle>
      <EmptyDescription>{{ progress ? progressLine(progress) : 'Reading 0.x data...' }}</EmptyDescription>
    </EmptyHeader>
    <EmptyContent v-if="percent !== null">
      <div class="h-1.5 w-full overflow-hidden rounded-full bg-muted">
        <div class="h-full bg-primary" :style="{ width: `${percent}%` }" />
      </div>
    </EmptyContent>
  </Empty>
  <Empty v-else-if="screen === 'failed'" class="m-auto max-w-lg">
    <EmptyHeader>
      <EmptyMedia variant="icon"><TriangleAlert /></EmptyMedia>
      <EmptyTitle>The 0.x import did not finish</EmptyTitle>
      <EmptyDescription class="text-destructive">{{ failure }}</EmptyDescription>
    </EmptyHeader>
    <EmptyContent>
      <div class="flex gap-2">
        <Button :disabled="busy" @click="choose('migrate')">{{ FIRST_RUN_TEXT.retry }}</Button>
        <Button variant="outline" :disabled="busy" @click="dismiss">{{ FIRST_RUN_TEXT.dismiss }}</Button>
      </div>
      <p v-if="problem" class="text-xs text-destructive">{{ problem }}</p>
      <p class="text-xs text-muted-foreground">
        You can also import from the
        <RouterLink :to="IMPORT_PATH" class="underline">Import</RouterLink> page.
      </p>
    </EmptyContent>
  </Empty>
  <slot v-else />
</template>
