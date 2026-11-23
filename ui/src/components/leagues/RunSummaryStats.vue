<script setup lang="ts">
import { computed } from 'vue'
import type { WinLossView } from '@/bindings'

/** The game-level headline for one league run, above its matches table. */
const props = defineProps<{
  gameWins: number
  gameLosses: number
  onPlayRecord: WinLossView
  onDrawRecord: WinLossView
}>()

const onPlayTotal = computed(() => props.onPlayRecord.wins + props.onPlayRecord.losses)
const onDrawTotal = computed(() => props.onDrawRecord.wins + props.onDrawRecord.losses)
</script>

<template>
  <div class="grid grid-cols-2 gap-4 sm:grid-cols-4">
    <div>
      <div class="text-xl font-semibold text-emerald-400 tabular-nums">{{ gameWins }}</div>
      <div class="text-xs text-muted-foreground">Game wins</div>
    </div>
    <div>
      <div class="text-xl font-semibold text-destructive tabular-nums">{{ gameLosses }}</div>
      <div class="text-xs text-muted-foreground">Game losses</div>
    </div>
    <div>
      <div class="text-xl font-semibold tabular-nums">{{ onPlayTotal }}</div>
      <div class="text-xs text-muted-foreground">On the play</div>
    </div>
    <div>
      <div class="text-xl font-semibold tabular-nums">{{ onDrawTotal }}</div>
      <div class="text-xs text-muted-foreground">On the draw</div>
    </div>
  </div>
</template>
