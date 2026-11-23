<script setup lang="ts">
import { computed } from 'vue'
import KpiBestWorstMatchups from '@/components/decks/KpiBestWorstMatchups.vue'
import KpiBoardingSplit from '@/components/decks/KpiBoardingSplit.vue'
import KpiMatchWinRate from '@/components/decks/KpiMatchWinRate.vue'
import KpiPlayDrawGap from '@/components/decks/KpiPlayDrawGap.vue'
import LeagueFinishes from '@/components/decks/LeagueFinishes.vue'
import LeagueRunCard from '@/components/decks/LeagueRunCard.vue'
import MatchHistoryChart from '@/components/decks/MatchHistoryChart.vue'
import StandoutCards from '@/components/decks/StandoutCards.vue'
import TimeframeFilter from '@/components/decks/TimeframeFilter.vue'
import DeckRankCard from '@/components/rank/DeckRankCard.vue'
import { Card, CardContent } from '@/components/ui/card'
import { Skeleton } from '@/components/ui/skeleton'
import { useDeckBestOfContext } from '@/composables/useDeckBestOf'
import { useDeckPage } from '@/composables/useDeckPage'
import { usePlatformContext } from '@/composables/usePlatformContext'
import { errorMessage } from '@/lib/commands'
import { useDeckDashboard, useDeckLeaguePanels, useDeckRank } from '@/lib/queries'
import { dashboardTimeframeLabel } from '@/lib/timeframes'

const { deckId, version, timeframe, setTimeframe } = useDeckPage()
const { bestOf, bo1Mode } = useDeckBestOfContext()
const { data: dashboard, error } = useDeckDashboard(deckId, version, timeframe, bestOf)
// Arena decks show their rank where MTGO decks show their leagues; each
// platform reads only its own column.
const { hasFeature } = usePlatformContext()
const ranking = computed(() => hasFeature('ranking'))
const leagues = computed(() => hasFeature('leagues'))
// All time, as 0.x: the finishes and the current run ignore the timeframe.
const { data: panels, dataUpdatedAt: panelsUpdatedAt } = useDeckLeaguePanels(deckId, version, leagues)
const { data: rank, isPending: rankPending, error: rankError } = useDeckRank(deckId, ranking, bestOf)

const timeframeLabel = computed(() => dashboardTimeframeLabel(timeframe.value))
const totalGames = computed(() => (dashboard.value ? dashboard.value.games.won + dashboard.value.games.lost : 0))
</script>

<template>
  <div class="space-y-4 p-3 lg:p-4">
    <TimeframeFilter :model-value="timeframe" @update:model-value="setTimeframe" />

    <p v-if="error" class="text-sm text-destructive">Could not load the dashboard: {{ errorMessage(error) }}</p>

    <div v-else-if="dashboard" class="space-y-4">
      <!-- KPI Cards -->
      <div class="grid grid-cols-1 gap-4 md:grid-cols-2 xl:grid-cols-4">
        <KpiMatchWinRate
          :record="dashboard.record"
          :delta="dashboard.delta"
          :sparkline="dashboard.sparkline"
          :timeframe-label="timeframeLabel"
        />
        <KpiBoardingSplit v-if="!bo1Mode" :split="dashboard.boarding" />
        <KpiPlayDrawGap :play-draw="dashboard.playDraw" :total-games="totalGames" />
        <KpiBestWorstMatchups :spread="dashboard.matchupSpread" />
      </div>

      <!-- Chart + League Finishes and the current run -->
      <div class="grid grid-cols-3 gap-4">
        <Card :class="ranking ? 'col-span-3' : 'col-span-2'">
          <CardContent>
            <MatchHistoryChart
              v-if="dashboard.daily.length"
              :data="dashboard.daily"
              :timeframe="timeframe"
              :peer="dashboard.peerChart"
            />
            <p v-else class="py-12 text-center text-sm text-muted-foreground">No match data for this period.</p>
          </CardContent>
        </Card>

        <!-- Arena: the deck's rank line, full width under the match chart. -->
        <div v-if="ranking" class="col-span-3">
          <Card v-if="rankPending && !rankError" class="gap-0 py-0">
            <CardContent class="p-4"><Skeleton class="h-72 w-full" /></CardContent>
          </Card>
          <!-- No constructed reading yet (null) still shows the card, with a short line. -->
          <DeckRankCard v-else :rank="rank ?? null" :error="rankError ? errorMessage(rankError) : null" />
        </div>
        <div v-else class="flex flex-col gap-4">
          <template v-if="panels">
            <LeagueFinishes :finishes="panels.finishes" />
            <!-- The run being played, or the last one finished. -->
            <LeagueRunCard v-if="panels.current" :panel="panels.current" :now="panelsUpdatedAt" />
          </template>
          <template v-else>
            <Card class="gap-0 p-0">
              <CardContent class="flex flex-col gap-2 p-4">
                <Skeleton class="h-6 w-full" />
                <Skeleton class="h-6 w-full" />
                <Skeleton class="h-6 w-3/4" />
              </CardContent>
            </Card>
            <Card class="gap-0 py-0">
              <CardContent class="p-4"><Skeleton class="h-40 w-full" /></CardContent>
            </Card>
          </template>
        </div>
      </div>

      <!-- Standout cards: all games of the version, whatever the timeframe -->
      <StandoutCards :standout="dashboard.standout" />
    </div>

    <div v-else class="space-y-4">
      <div class="grid grid-cols-1 gap-4 md:grid-cols-2 xl:grid-cols-4">
        <Card v-for="i in 4" :key="i" class="gap-0 py-0">
          <CardContent class="p-4"><Skeleton class="h-28 w-full" /></CardContent>
        </Card>
      </div>
      <div class="grid grid-cols-3 gap-4">
        <Card class="col-span-2">
          <CardContent><Skeleton class="h-[400px] w-full" /></CardContent>
        </Card>
        <Card class="gap-0 p-0">
          <CardContent class="p-4"><Skeleton class="h-40 w-full" /></CardContent>
        </Card>
      </div>
    </div>
  </div>
</template>
