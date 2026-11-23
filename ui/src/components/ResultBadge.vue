<script setup lang="ts">
defineProps<{ text: string; tone: 'win' | 'loss' | 'draw' | 'muted'; provisional: boolean }>()
</script>

<template>
  <div
    class="flex items-center gap-2 text-sm font-medium"
    :class="{
      'text-success': tone === 'win',
      'text-destructive': tone === 'loss',
      'text-warning': tone === 'draw',
      'text-muted-foreground': tone === 'muted',
      'opacity-70': provisional,
    }"
  >
    <div
      class="bevel block h-2 w-2 rounded-full"
      :class="{
        'bg-success': tone === 'win',
        'bg-destructive': tone === 'loss',
        'bg-warning': tone === 'draw',
        'bg-muted-foreground': tone === 'muted',
      }"
    />
    <span>{{ text }}<template v-if="provisional && tone !== 'muted'"> so far</template></span>
  </div>
</template>
