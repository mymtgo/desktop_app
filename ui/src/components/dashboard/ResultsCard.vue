<script setup lang="ts">
import { computed } from 'vue'
import type { ResultsView } from '@/bindings'
import HomeCard from '@/components/dashboard/HomeCard.vue'
import MatchHistoryChart from '@/components/decks/MatchHistoryChart.vue'
import MatchRecord from '@/components/decks/MatchRecord.vue'
import { deltaClass, deltaText, recentSessionLabel, winRateClass } from '@/lib/homeDashboard'
import { NO_VALUE } from '@/lib/leagues'

/**
 * Results (dashboard redesign spec 4.1): match and game win rates with
 * their change against the previous window (none for all time), play and
 * draw, the streak, the chart, and the recent sessions strip.
 */
const props = defineProps<{ results: ResultsView; timeframe: string; emptyText: string }>()

const deltas = computed(() => props.timeframe !== 'alltime')
</script>

<template>
  <HomeCard title="Results">
    <p v-if="results.matches === 0" class="py-8 text-center text-sm text-muted-foreground">{{ emptyText }}</p>
    <div v-else class="flex flex-col gap-6">
      <div class="grid grid-cols-2 gap-4 lg:grid-cols-4">
        <div class="flex flex-col gap-1">
          <span class="text-xs text-muted-foreground">Match win rate</span>
          <span class="flex items-baseline gap-2">
            <span class="text-3xl font-semibold tabular-nums">{{ results.record.winRate }}%</span>
            <span
              v-if="deltas"
              class="rounded-full px-1.5 py-0.5 text-[10px] font-medium tabular-nums"
              :class="deltaClass(results.matchWinRateDelta)"
            >
              {{ deltaText(results.matchWinRateDelta) }}
            </span>
          </span>
          <MatchRecord :record="results.record" />
        </div>
        <div class="flex flex-col gap-1">
          <span class="text-xs text-muted-foreground">Game win rate</span>
          <span class="flex items-baseline gap-2">
            <span class="text-2xl font-semibold tabular-nums">{{ results.games.winRate }}%</span>
            <span
              v-if="deltas"
              class="rounded-full px-1.5 py-0.5 text-[10px] font-medium tabular-nums"
              :class="deltaClass(results.gameWinRateDelta)"
            >
              {{ deltaText(results.gameWinRateDelta) }}
            </span>
          </span>
          <span class="text-xs tabular-nums">
            <span class="text-success">{{ results.games.won }}W</span>
            {{ ' ' }}
            <span class="text-destructive">{{ results.games.lost }}L</span>
          </span>
        </div>
        <div class="flex flex-col gap-1">
          <span class="text-xs text-muted-foreground">Game win rate, play / draw</span>
          <span class="text-2xl font-semibold tabular-nums">
            {{ results.playDraw.otpWinRate }}%
            <span class="text-muted-foreground/60">/</span>
            {{ results.playDraw.otdWinRate }}%
          </span>
          <span class="text-xs text-muted-foreground">On the play / on the draw</span>
        </div>
        <div class="flex flex-col gap-1">
          <span class="text-xs text-muted-foreground">Current streak</span>
          <span class="text-2xl font-semibold tabular-nums">{{ results.streak.current ?? NO_VALUE }}</span>
          <span class="text-xs text-muted-foreground tabular-nums">Best {{ results.streak.bestWin }}W</span>
        </div>
      </div>

      <MatchHistoryChart
        :data="results.daily"
        :timeframe="timeframe"
        title=""
        storage-key="dashboard:results-chart-mode"
        :height="260"
      />

      <div v-if="results.recentSessions.length" class="flex flex-col gap-2">
        <span class="text-xs text-muted-foreground">Recent sessions</span>
        <div class="flex flex-wrap gap-2">
          <div
            v-for="session in results.recentSessions"
            :key="session.day"
            class="recent-session flex min-w-24 flex-1 flex-col gap-0.5 rounded-md border border-border px-3 py-2"
          >
            <span class="text-xs text-muted-foreground">{{ recentSessionLabel(session) }}</span>
            <span class="text-lg font-semibold tabular-nums" :class="winRateClass(session.record.winRate)">
              {{ session.record.winRate }}%
            </span>
            <span class="text-xs text-muted-foreground tabular-nums">{{ session.record.label }}</span>
          </div>
        </div>
      </div>
    </div>
  </HomeCard>
</template>
