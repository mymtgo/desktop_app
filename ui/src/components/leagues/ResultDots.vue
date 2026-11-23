<script setup lang="ts">
import { computed } from 'vue'
import type { MarkView } from '@/bindings'
import { resultDots } from '@/lib/leagues'

/** One dot per round: green win, red loss, an empty ring for a round not played yet. */
const props = defineProps<{ results: (MarkView | null)[] }>()

const dots = computed(() => resultDots(props.results))
</script>

<template>
  <div class="flex items-center gap-1">
    <template v-for="(dot, index) in dots" :key="index">
      <div v-if="dot === 'open'" class="size-2 rounded-full border border-muted-foreground/40" />
      <div v-else class="flex items-center">
        <div class="bevel block h-2 w-2 rounded-full" :class="dot === 'win' ? 'bg-success' : 'bg-destructive'" />
        <span class="sr-only">{{ dot === 'win' ? 'Win' : 'Loss' }}</span>
      </div>
    </template>
  </div>
</template>
