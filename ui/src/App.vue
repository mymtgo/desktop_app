<script setup lang="ts">
import FatalScreen from '@/components/FatalScreen.vue'
import FirstRunChoice from '@/components/FirstRunChoice.vue'
import { useWindowTitle } from '@/composables/useWindowTitle'
import AppLayout from '@/layouts/AppLayout.vue'
import { onMounted } from 'vue'
import { useRouter } from 'vue-router'
import { askWhatsNewOnStart } from '@/composables/useWhatsNew'
import { useLeaveHiddenPages } from '@/composables/usePlatformContext'
import { useAppInfo } from '@/lib/queries'

const { data: info } = useAppInfo()
const router = useRouter()

// Only the main window mounts this component: the overlay and replay
// windows are other entries and never ask (release spec section 7).
onMounted(() => void askWhatsNewOnStart(router))
useWindowTitle()
useLeaveHiddenPages()
</script>

<template>
  <FatalScreen v-if="info?.fatal" :info="info" />
  <AppLayout v-else>
    <!-- The first run's 0.x question and migration cover the page until settled (release spec 13.2). -->
    <FirstRunChoice>
      <RouterView />
    </FirstRunChoice>
  </AppLayout>
</template>
