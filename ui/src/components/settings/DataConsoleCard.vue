<script setup lang="ts">
import { ref } from 'vue'
import { commands } from '@/bindings'
import SettingsCard from '@/components/SettingsCard.vue'
import { Button } from '@/components/ui/button'
import { useToast } from '@/composables/useToast'
import { errorMessage } from '@/lib/commands'

// The data console entry (spec 2026-11-20-data-console-design.md, section 5.6).
// These two commands return a plain string error.
const { add } = useToast()
const busy = ref(false)

type Result = { status: 'ok'; data: null } | { status: 'error'; error: string }

async function run(open: () => Promise<Result>, failure: string) {
  busy.value = true
  try {
    const result = await open()
    if (result.status === 'error') throw new Error(result.error)
  } catch (error) {
    add({ type: 'error', title: failure, message: errorMessage(error) })
  } finally {
    busy.value = false
  }
}

const openFolder = () => run(() => commands.openDatabaseFolder(), 'Could not open the folder')
const openConsole = () => run(() => commands.openConsoleWindow(), 'Could not open the data console')
</script>

<template>
  <SettingsCard
    title="Data console"
    description="Run read-only SQL over your own data, chart the results and export them. Your data is never changed."
  >
    <Button
      size="sm"
      variant="outline"
      :disabled="busy"
      @click="openFolder"
    >
      Open database folder
    </Button>
    <Button
      size="sm"
      :disabled="busy"
      @click="openConsole"
    >
      Open data console
    </Button>
  </SettingsCard>
</template>
