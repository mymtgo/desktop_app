<script setup lang="ts">
import { computed, ref } from 'vue'
import { RouterLink } from 'vue-router'
import licenceText from '../../../../LICENSE?raw'
import SettingsCard from '@/components/SettingsCard.vue'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { channelOf } from '@/lib/licences'
import { useUpdateStatus } from '@/lib/queries'

// The About card on the Advanced page (release spec section 9); the notice
// line is 0.x's `NOTICE`. The channel is the update channel (W4, the user's
// choice); the build's own channel stands in until the status loads.
const props = defineProps<{ version: string }>()

const { data: update } = useUpdateStatus()
const licenceOpen = ref(false)
const channel = computed(() => update.value?.channel ?? channelOf(props.version))
const versionLine = computed(() => `MyMTGO v${props.version} (${channel.value})`)
</script>

<template>
  <SettingsCard title="About" :description="versionLine">
    <template #details>
      <p class="text-xs text-muted-foreground">
        MyMTGO is not affiliated with, endorsed by, or sponsored by Wizards of the Coast.
      </p>
    </template>
    <Button size="sm" variant="outline" @click="licenceOpen = true">Licence</Button>
    <Button size="sm" variant="outline" as-child>
      <RouterLink to="/settings/advanced/licences">Third-party licences</RouterLink>
    </Button>
  </SettingsCard>

  <Dialog :open="licenceOpen" @update:open="licenceOpen = $event">
    <DialogContent class="max-w-2xl">
      <DialogHeader>
        <DialogTitle>Licence</DialogTitle>
        <DialogDescription>The licence MyMTGO is published under.</DialogDescription>
      </DialogHeader>
      <pre class="max-h-[60vh] overflow-auto text-xs whitespace-pre-wrap select-text">{{ licenceText }}</pre>
    </DialogContent>
  </Dialog>
</template>
