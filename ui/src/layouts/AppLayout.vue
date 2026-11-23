<script setup lang="ts">
import AppHeader from '@/components/AppHeader.vue'
import UpdateBanner from '@/components/UpdateBanner.vue'
import StatusBar from '@/components/StatusBar.vue'
import ToastContainer from '@/components/ToastContainer.vue'
import UpdateInstallingOverlay from '@/components/UpdateInstallingOverlay.vue'
</script>

<template>
  <div class="texture-bg flex h-screen flex-col bg-background text-foreground">
    <AppHeader />
    <UpdateBanner />
    <div class="flex min-h-0 flex-1 flex-col overflow-y-auto">
      <slot />
    </div>
    <StatusBar />
    <ToastContainer />
    <UpdateInstallingOverlay />
  </div>
</template>
