<script setup lang="ts">
import { computed } from 'vue'
import type { MatchupDetailView, MatchupView } from '@/bindings'
import ManaColors from '@/components/mana/ManaColors.vue'
import { winRateTone } from '@/lib/decks'
import { screenshotGames } from '@/lib/matchups'

/**
 * The card "Copy screenshot" renders off screen (0.x `MatchupScreenshot`):
 * the matchup's record, games 1 to 3, on the play, average turns and
 * mulligans. Inline styles, so the capture does not depend on the page.
 */
const props = defineProps<{
  matchup: MatchupView
  detail: MatchupDetailView
  timeframeLabel: string
}>()

const colors = {
  bg: '#111111',
  text: '#ffffff',
  muted: 'rgba(255, 255, 255, 0.5)',
  subtle: 'rgba(255, 255, 255, 0.04)',
  win: '#22c55e',
  loss: '#ef4444',
  border: 'rgba(255, 255, 255, 0.08)',
}

function rateColor(rate: number): string {
  const tone = winRateTone(rate)
  if (tone === 'win') return colors.win
  if (tone === 'loss') return colors.loss
  return colors.text
}

function oneDecimal(value: number | null): string {
  return value === null ? '-' : value.toFixed(1)
}

const games = computed(() => screenshotGames(props.detail.perGame))
const stats = computed(() => [
  { label: 'OTP', value: `${props.detail.otpWinRate}%`, color: rateColor(props.detail.otpWinRate) },
  { label: 'Avg turns', value: oneDecimal(props.detail.avgTurns), color: colors.text },
  { label: 'Avg mulls', value: oneDecimal(props.detail.avgMulligans), color: colors.text },
])
</script>

<template>
  <div
    :style="{
      width: '480px',
      backgroundColor: colors.bg,
      color: colors.text,
      fontFamily: 'system-ui, -apple-system, sans-serif',
      padding: '18px 22px',
      borderRadius: '12px',
    }"
  >
    <div :style="{ display: 'flex', justifyContent: 'space-between', alignItems: 'flex-start' }">
      <div>
        <div :style="{ display: 'flex', alignItems: 'center', gap: '8px' }">
          <span :style="{ fontSize: '17px', fontWeight: '700', lineHeight: '1.2' }">{{ matchup.name }}</span>
          <ManaColors v-if="matchup.colors.length" :colors="matchup.colors" />
        </div>
        <div :style="{ fontSize: '12px', color: colors.muted, marginTop: '2px' }">
          {{ matchup.matches }} matches · {{ timeframeLabel }}
        </div>
      </div>
      <div :style="{ textAlign: 'right' }">
        <div
          :style="{
            fontSize: '24px',
            fontWeight: '700',
            lineHeight: '1',
            color: rateColor(matchup.matchWinRate),
            fontVariantNumeric: 'tabular-nums',
          }"
        >
          {{ matchup.matchWinRate }}%
        </div>
        <div :style="{ fontSize: '12px', color: colors.muted, marginTop: '2px', fontVariantNumeric: 'tabular-nums' }">
          {{ matchup.matchRecord }}
        </div>
      </div>
    </div>

    <div :style="{ height: '1px', backgroundColor: colors.border, margin: '12px 0' }" />

    <div :style="{ display: 'grid', gridTemplateColumns: 'repeat(3, 1fr)', gap: '8px' }">
      <div
        v-for="entry in games"
        :key="entry.gameNumber"
        :style="{ backgroundColor: colors.subtle, padding: '10px', borderRadius: '6px', textAlign: 'center' }"
      >
        <div :style="{ fontSize: '11px', color: colors.muted }">Game {{ entry.gameNumber }}</div>
        <div
          :style="{
            fontSize: '18px',
            fontWeight: '600',
            marginTop: '4px',
            color: entry.game ? rateColor(entry.game.winRate) : colors.muted,
            fontVariantNumeric: 'tabular-nums',
          }"
        >
          {{ entry.game ? `${entry.game.winRate}%` : '-' }}
        </div>
        <div :style="{ fontSize: '11px', color: colors.muted, fontVariantNumeric: 'tabular-nums' }">
          {{ entry.game ? entry.game.record : '-' }}
        </div>
      </div>
    </div>

    <div :style="{ height: '1px', backgroundColor: colors.border, margin: '12px 0' }" />

    <div :style="{ display: 'flex' }">
      <div
        v-for="stat in stats"
        :key="stat.label"
        :style="{ display: 'flex', flexDirection: 'column', alignItems: 'center', padding: '8px 0', flex: '1' }"
      >
        <div :style="{ fontSize: '22px', fontWeight: '700', color: stat.color, fontVariantNumeric: 'tabular-nums' }">
          {{ stat.value }}
        </div>
        <div
          :style="{
            fontSize: '10px',
            textTransform: 'uppercase',
            letterSpacing: '0.05em',
            color: colors.muted,
            marginTop: '2px',
          }"
        >
          {{ stat.label }}
        </div>
      </div>
    </div>

    <div :style="{ marginTop: '12px', textAlign: 'right', fontSize: '10px', color: colors.muted }">mymtgo.com</div>
  </div>
</template>
