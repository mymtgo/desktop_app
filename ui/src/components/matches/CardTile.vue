<script setup lang="ts">
import CardImage from '@/components/CardImage.vue'
/** A card image, or its name on a muted tile when there is no image ("Card <id>" for an unknown card). */
defineProps<{
  name: string
  image: string | null
  local?: string | null
}>()
</script>

<template>
  <CardImage v-if="image" :src="image" :local="local" :alt="name" class="h-full w-full object-cover" />
  <div v-else class="flex h-full w-full items-center justify-center bg-muted p-1.5 text-center">
    <span class="text-xs leading-tight text-muted-foreground">{{ name }}</span>
  </div>
</template>
