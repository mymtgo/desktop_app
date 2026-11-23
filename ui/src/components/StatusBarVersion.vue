<script setup lang="ts">
import { computed } from 'vue'
import { RouterLink } from 'vue-router'
import { useInstallUpdate } from '@/composables/useInstallUpdate'
import { useAppInfo, useUpdateStatus } from '@/lib/queries'

// 0.x `StatusBarVersion.vue` (release spec 6.2): the ready update (click
// installs), the download, or the running version, linked to What's new
// when that page exists.
const props = withDefaults(defineProps<{ whatsNewUrl?: string | null }>(), { whatsNewUrl: null })

const { data: update } = useUpdateStatus()
const { data: info } = useAppInfo()
const { installing, install } = useInstallUpdate()

const current = computed(() => update.value?.current ?? info.value?.version ?? null)
</script>

<template>
  <button
    v-if="update?.phase === 'ready'"
    type="button"
    class="flex cursor-pointer items-center gap-1.5 rounded-sm bg-primary px-2 py-0.5 font-medium text-primary-foreground hover:bg-primary/90 disabled:opacity-60"
    :disabled="installing"
    @click="install"
  >
    Update ready: v{{ update.available }}
  </button>
  <span v-else-if="update?.phase === 'downloading'">Downloading update...</span>
  <RouterLink v-else-if="current && props.whatsNewUrl" :to="props.whatsNewUrl" class="cursor-pointer hover:text-foreground"
    >v{{ current }}</RouterLink
  >
  <span v-else-if="current">v{{ current }}</span>
</template>
