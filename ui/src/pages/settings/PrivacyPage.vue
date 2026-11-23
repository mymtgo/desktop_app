<script setup lang="ts">
import ApiStatusCard from '@/components/settings/ApiStatusCard.vue'
import OfflineModeCard from '@/components/settings/OfflineModeCard.vue'
</script>

<template>
  <div class="flex flex-col gap-3">

    <OfflineModeCard />
    <ApiStatusCard />
  </div>
</template>
