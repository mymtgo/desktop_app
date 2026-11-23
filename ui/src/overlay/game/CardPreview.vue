<script setup lang="ts">
import CardImage from '@/components/CardImage.vue'

/** The hovered card's image, anchored to the window's right edge (0.x). */
defineProps<{
  card: { name: string; image: string | null; localImage?: string | null } | null
  top: number
}>()
</script>

<template>
  <Transition name="fade">
    <div v-if="card && (card.image || card.localImage)" class="pointer-events-none fixed right-2 z-50" :style="{ top: `${top}px` }">
      <CardImage :src="card.image" :local="card.localImage" :alt="card.name" class="w-[200px] rounded-lg shadow-xl ring-1 ring-border" />
    </div>
  </Transition>
</template>

<style scoped>
.fade-enter-active {
  transition: opacity 0.1s ease;
}
.fade-leave-active {
  transition: opacity 0.05s ease;
}
.fade-enter-from,
.fade-leave-to {
  opacity: 0;
}
</style>
