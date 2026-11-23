<script setup lang="ts">
import CardDataCard from '@/components/settings/CardDataCard.vue'
import FilePathsCard from '@/components/settings/FilePathsCard.vue'
import LocalImagesCard from '@/components/settings/LocalImagesCard.vue'
import WatcherCard from '@/components/settings/WatcherCard.vue'

// 0.x `pages/settings/Storage.vue`: card images, card data, file paths and
// the watcher (release spec section 4). Deleted items are on Settings,
// Archives (league self-management spec 5.5).
</script>

<template>
  <div class="flex flex-col gap-3">
    <LocalImagesCard />

    <CardDataCard />

    <FilePathsCard />

    <WatcherCard />
  </div>
</template>
