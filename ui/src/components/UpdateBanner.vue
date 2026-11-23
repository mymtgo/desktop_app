<script setup lang="ts">
import { Download } from 'lucide-vue-next'
import { Button } from '@/components/ui/button'
import { useInstallUpdate } from '@/composables/useInstallUpdate'
import { useUpdateStatus } from '@/lib/queries'

const { data: update } = useUpdateStatus()
const { installing, install } = useInstallUpdate()
</script>

<template>
  <div
    v-if="update?.phase === 'ready'"
    class="flex py-2 shrink-0 items-center justify-between gap-4 border-b border-black/80 bg-primary/15 px-4 text-sm"
  >
    <div class="flex items-center gap-2">
      <Download :size="14" class="text-primary" />
      <span>{{ update.available }} is ready to install.</span>
    </div>
    <Button size="sm" :disabled="installing" @click="install">Install & restart</Button>
  </div>
</template>
