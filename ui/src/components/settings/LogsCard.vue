<script setup lang="ts">
import { ref } from 'vue'
import { commands } from '@/bindings'
import SettingsCard from '@/components/SettingsCard.vue'
import { Button } from '@/components/ui/button'
import { useToast } from '@/composables/useToast'
import { errorMessage, unwrap } from '@/lib/commands'
import { logFolder } from '@/lib/paths'

// v1's log folder card (Advanced page, spec ruling 4).
defineProps<{ dataDir: string }>()

const { add } = useToast()
const opening = ref(false)

async function open() {
  opening.value = true
  try {
    unwrap(await commands.openLogFolder())
  } catch (error) {
    add({ type: 'error', title: 'Could not open the folder', message: errorMessage(error) })
  } finally {
    opening.value = false
  }
}
</script>

<template>
  <SettingsCard
    title="Logs"
    description="If something goes wrong, send us the files in this folder with a description of what happened."
  >
    <template #details>
      <code class="text-xs break-all select-all">{{ logFolder(dataDir) }}</code>
    </template>
    <Button size="sm" variant="outline" :disabled="opening" @click="open">Open folder</Button>
  </SettingsCard>
</template>
