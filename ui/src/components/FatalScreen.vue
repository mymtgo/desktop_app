<script setup lang="ts">
import { CircleX } from 'lucide-vue-next'
import type { AppInfo } from '@/bindings'
import { logFolder } from '@/lib/paths'

defineProps<{ info: AppInfo }>()
</script>

<template>
  <main class="flex h-screen items-center justify-center bg-background p-8 text-foreground">
    <div class="max-w-xl space-y-3 rounded-lg border bg-card p-6">
      <div class="flex items-center gap-2 text-destructive">
        <CircleX class="size-5" />
        <h1 class="text-lg font-semibold">MyMTGO could not start</h1>
      </div>
      <p class="text-sm">{{ info.fatal }}</p>
      <p v-if="info.dataDir" class="text-xs text-muted-foreground">
        Data folder: <code class="select-all">{{ info.dataDir }}</code>
      </p>
      <p class="text-xs text-muted-foreground">
        Close MyMTGO and open it again. If this keeps happening, send us the files in
        <code v-if="info.dataDir" class="break-all select-all">{{ logFolder(info.dataDir) }}</code>
        <template v-else>the logs folder</template>.
      </p>
    </div>
  </main>
</template>
