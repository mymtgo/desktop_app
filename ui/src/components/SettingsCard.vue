<script setup lang="ts">
defineProps<{ title: string; description: string }>()
</script>

<template>
  <section class="flex items-start justify-between gap-6 rounded-lg border bg-card p-4">
    <div class="space-y-1">
      <h2 class="text-sm font-semibold">{{ title }}</h2>
      <p class="text-xs text-muted-foreground">{{ description }}</p>
      <slot name="details" />
    </div>
    <div class="flex shrink-0 items-center gap-2">
      <slot />
    </div>
  </section>
</template>
