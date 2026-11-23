<script setup lang="ts">
import { Dices, Hand } from 'lucide-vue-next'
import { computed } from 'vue'
import type { OverlayGame } from '@/bindings'

/**
 * The strip above the opponent's name: play or draw, and the hand each
 * player kept. Each part shows once MTGO logs it; the strip is empty (and
 * hidden by the header) before then.
 */
const props = defineProps<{ game: OverlayGame }>()

const playDraw = computed(() => {
  if (props.game.onPlay === null) return null
  return props.game.onPlay ? 'On the play' : 'On the draw'
})

const hands = computed(() => {
  const { localHand, opponentHand } = props.game
  if (localHand === null && opponentHand === null) return null
  return `You ${localHand ?? '–'} · Opp ${opponentHand ?? '–'}`
})
</script>

<template>
  <div data-tauri-drag-region class="flex items-center justify-between gap-3 text-xs text-muted-foreground">
    <span v-if="playDraw" data-tauri-drag-region class="inline-flex items-center gap-1">
      <Dices class="size-3.5" aria-hidden="true" />
      {{ playDraw }}
    </span>
    <span v-if="hands" data-tauri-drag-region class="ml-auto inline-flex items-center gap-1" title="Hand size kept after mulligans">
      <Hand class="size-3.5" aria-hidden="true" />
      {{ hands }}
    </span>
  </div>
</template>
