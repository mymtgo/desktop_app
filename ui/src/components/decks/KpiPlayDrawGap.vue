<script setup lang="ts">
import { computed } from 'vue'
import type { PlayDrawView } from '@/bindings'
import KpiBar from '@/components/decks/KpiBar.vue'
import KpiCard from '@/components/decks/KpiCard.vue'

const props = defineProps<{
  playDraw: PlayDrawView
  /** Every game in range, logged or not, for the scope note. */
  totalGames: number
}>()

const gap = computed(() => props.playDraw.otpRate - props.playDraw.otdRate)
</script>

<template>
  <KpiCard label="Play / draw gap" :scope="`${playDraw.playDrawGames} of ${totalGames} games`">
    <template v-if="playDraw.playDrawGames > 0">
      <div class="flex items-baseline gap-2">
        <span class="text-4xl font-bold tabular-nums text-warning">{{ gap }}</span>
        <span class="text-sm text-muted-foreground">pts</span>
      </div>

      <div class="flex flex-col gap-2.5">
        <KpiBar
          label="On the play"
          :rate="playDraw.otpRate"
          :record="`${playDraw.otpWon}–${playDraw.otpLost}`"
          :tone="playDraw.otpRate >= 50 ? 'success' : 'destructive'"
        />
        <KpiBar
          label="On the draw"
          :rate="playDraw.otdRate"
          :record="`${playDraw.otdWon}–${playDraw.otdLost}`"
          :tone="playDraw.otdRate >= 50 ? 'success' : 'destructive'"
        />
      </div>
    </template>

    <p v-else class="py-4 text-sm text-muted-foreground">
      Who was on the play is only known from a game log. None of the {{ totalGames }} games in this range has one.
    </p>
  </KpiCard>
</template>
