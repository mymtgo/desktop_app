<script setup lang="ts">
import { FolderSearch, Hourglass, UserSearch } from 'lucide-vue-next'
import { RouterLink } from 'vue-router'
import type { StatusView } from '@/bindings'
import { Button } from '@/components/ui/button'
import { Empty, EmptyContent, EmptyDescription, EmptyHeader, EmptyMedia, EmptyTitle } from '@/components/ui/empty'
import { useSaveSettings } from '@/composables/useSaveSettings'
import { useToast } from '@/composables/useToast'
import { offersFirstImport } from '@/lib/legacyImport'
import { useAppInfo, useLegacyImport, useSettings } from '@/lib/queries'
import type { FirstRun } from '@/lib/status'

defineProps<{ state: Exclude<FirstRun, null>; status: StatusView }>()

const { data: settings } = useSettings()
const { saving, chooseLogFolder } = useSaveSettings()
const { add } = useToast()
const { data: legacy } = useLegacyImport()
const { data: info } = useAppInfo()

async function chooseFolder() {
  if (settings.value === undefined) return
  if (await chooseLogFolder(settings.value)) {
    add({ type: 'success', title: 'Log folder saved', message: 'Looking for mtgo.log there now.' })
  }
}
</script>

<template>
  <Empty class="m-auto max-w-lg">
    <EmptyHeader>
      <EmptyMedia variant="icon">
        <FolderSearch v-if="state === 'not_found'" />
        <UserSearch v-else-if="state === 'awaiting_identity'" />
        <Hourglass v-else />
      </EmptyMedia>
      <template v-if="state === 'not_found'">
        <EmptyTitle>MTGO not found</EmptyTitle>
        <EmptyDescription>No mtgo.log was found in these locations:</EmptyDescription>
      </template>
      <template v-else-if="state === 'awaiting_identity'">
        <EmptyTitle>Waiting to identify your MTGO username</EmptyTitle>
        <EmptyDescription>
          Your matches will appear once MTGO logs your sign-in. Signing out of MTGO and back in does it straight away.
        </EmptyDescription>
      </template>
      <template v-else>
        <EmptyTitle>Waiting for your first match</EmptyTitle>
        <EmptyDescription>MyMTGO is watching your MTGO log. Play a match and it will appear here.</EmptyDescription>
      </template>
    </EmptyHeader>
    <EmptyContent v-if="state === 'not_found'">
      <ul class="w-full space-y-1 text-left text-xs text-muted-foreground">
        <li v-for="folder in status.searched" :key="folder"><code class="select-all break-all">{{ folder }}</code></li>
        <li v-if="status.searched.length === 0">No locations to search on this system.</li>
      </ul>
      <p v-if="info?.os === 'macos'" class="text-xs text-muted-foreground">
        Running MTGO in a Windows virtual machine? Share its MTGO folder with your Mac and choose it here.
      </p>
      <Button :disabled="saving" @click="chooseFolder">Choose folder</Button>
    </EmptyContent>
    <p v-if="offersFirstImport(legacy)" class="text-xs text-muted-foreground">
      You used MyMTGO 0.x on this computer. Import your matches on the
      <RouterLink to="/import" class="underline">Import</RouterLink> page.
    </p>
  </Empty>
</template>
