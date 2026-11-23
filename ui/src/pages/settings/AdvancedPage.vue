<script setup lang="ts">
import AboutCard from '@/components/settings/AboutCard.vue'
import DataConsoleCard from '@/components/settings/DataConsoleCard.vue'
import LogsCard from '@/components/settings/LogsCard.vue'
import { useAppInfo } from '@/lib/queries'

// 0.x `pages/settings/Advanced.vue`; Logs and About sit here (spec ruling 4). The
// Debug Mode card waits for W9 (deferred by the owner, #286).
const { data: info } = useAppInfo()
</script>

<template>
  <div v-if="info" class="flex flex-col gap-3">
    <LogsCard :data-dir="info.dataDir" />
    <DataConsoleCard />
    <AboutCard :version="info.version" />
    <p class="text-xs text-muted-foreground">MyMTGO v{{ info.version }}</p>
  </div>
</template>
