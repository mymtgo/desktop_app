<script setup lang="ts">
import { computed } from 'vue'
import type { GamePip, LeagueGames } from '@/bindings'
import { PIP_LETTERS, pips, pipTone, type PipTone } from '@/lib/leagueOverlayCard'

// The live match's games on the league card (0.x `OverlayGamePips.vue`):
// dots, or letters (`W / L / -`, league card options spec section 4).
const props = defineProps<{
  games: GamePip[]
  /** True only while a game is being played (status in_game). */
  live: boolean
  mode: LeagueGames
}>()

const shown = computed(() => pips(props.games, props.live).map((pip) => ({ ...pip, tone: pipTone(pip) })))

const LETTER_CLASS: Record<PipTone, string> = {
  won: 'text-emerald-300',
  lost: 'text-red-300',
  drawn: 'text-white/70',
  live: 'animate-pulse text-amber-300',
  unplayed: 'text-white/35',
}
</script>

<template>
  <span v-if="mode === 'letters'" class="inline-flex shrink-0 items-center gap-1 font-bold tabular-nums">
    <template v-for="(pip, index) in shown" :key="pip.key">
      <span v-if="index > 0" class="text-white/35">/</span>
      <span :class="LETTER_CLASS[pip.tone]">{{ PIP_LETTERS[pip.tone] }}</span>
    </template>
  </span>
  <span v-else class="inline-flex shrink-0 items-center gap-1">
    <span
      v-for="pip in shown"
      :key="pip.key"
      class="block size-2 rounded-full ring-1 ring-black/60"
      :class="{
        'bevel bg-success': pip.tone === 'won',
        'bevel bg-destructive': pip.tone === 'lost',
        'animate-pulse bg-white': pip.tone === 'live',
        'bg-white/20': pip.tone === 'drawn' || pip.tone === 'unplayed',
      }"
    />
  </span>
</template>
