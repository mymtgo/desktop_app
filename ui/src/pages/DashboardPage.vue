<script setup lang="ts">
import { LayoutDashboard } from 'lucide-vue-next'
import { computed } from 'vue'
import { RouterLink, useRoute, useRouter } from 'vue-router'
import BreakdownCard from '@/components/dashboard/BreakdownCard.vue'
import DashboardWrappedCard from '@/components/dashboard/DashboardWrappedCard.vue'
import LastSessionCard from '@/components/dashboard/LastSessionCard.vue'
import LeagueCard from '@/components/dashboard/LeagueCard.vue'
import LeagueResultsCard from '@/components/dashboard/LeagueResultsCard.vue'
import MostPickedCard from '@/components/dashboard/MostPickedCard.vue'
import RecentCard from '@/components/dashboard/RecentCard.vue'
import RecentMatchesCard from '@/components/dashboard/RecentMatchesCard.vue'
import ResultsCard from '@/components/dashboard/ResultsCard.vue'
import TopMatchupsCard from '@/components/dashboard/TopMatchupsCard.vue'
import YourColoursCard from '@/components/dashboard/YourColoursCard.vue'
import RankCard from '@/components/rank/RankCard.vue'
import TimeframeFilter from '@/components/decks/TimeframeFilter.vue'
import FirstRunState from '@/components/FirstRunState.vue'
import SegmentedControl from '@/components/SegmentedControl.vue'
import { Skeleton } from '@/components/ui/skeleton'
import { usePlatformContext } from '@/composables/usePlatformContext'
import { errorMessage } from '@/lib/commands'
import { askedFormat, formatChoices, noMatchesText, showFormatSwitch, withQueryValue } from '@/lib/homeDashboard'
import { offersImportHint } from '@/lib/legacyImport'
import { hasFeature } from '@/lib/platformFeatures'
import { useHomeDashboard, useLegacyImport, useStatus } from '@/lib/queries'
import { firstRunState } from '@/lib/status'
import { normalizeTimeframe } from '@/lib/timeframes'
import { useWrapped } from '@/lib/wrapped/useWrapped'

/**
 * The home page (dashboard redesign spec): one fixed page of cards for the
 * timeframe (`?timeframe=`, all time when absent) and the format
 * (`?format=`, else the format last played, which the backend resolves).
 */
const route = useRoute()
const router = useRouter()

const timeframe = computed(() => normalizeTimeframe(route.query.timeframe))
const asked = computed(() => askedFormat(route.query.format))
const { data: dashboard, dataUpdatedAt, error, isPlaceholderData } = useHomeDashboard(timeframe, asked)
// The cards read the shown data; the switch follows the click at once, and
// otherwise shows the format the backend resolved (the one last played).
const format = computed(() => dashboard.value?.format ?? asked.value)
const switchFormat = computed(() => (isPlaceholderData.value && asked.value !== '' ? asked.value : format.value))
const { platform } = usePlatformContext()
// Arena has ranks where MTGO has leagues (arena spec section 15).
const ranking = computed(() => hasFeature(platform.value, 'ranking'))
const leagues = computed(() => hasFeature(platform.value, 'leagues'))
const choices = computed(() => formatChoices(dashboard.value?.formats ?? [], platform.value))
// Arena hides the switch once the dashboard shows the account has one format.
const formatSwitch = computed(() => !dashboard.value || showFormatSwitch(dashboard.value.formats, platform.value))

// This year's Wrapped, above Results while it is out (Wrapped spec).
const { bundle: wrapped, dismissed: wrappedDismissed, dismiss: dismissWrapped } = useWrapped()

const { data: status } = useStatus()
const { data: legacy } = useLegacyImport()
const importHint = computed(() => offersImportHint(legacy.value, dashboard.value?.hasMatches !== false))
// A fresh install gets v1's first-run help (finding MTGO, the 0.x import)
// in place of an empty dashboard.
// Arena needs none of it: its log is found by its own setup (arena spec section 15).
const firstRun = computed(() =>
  dashboard.value && hasFeature(platform.value, 'logTrackerStatus') ? firstRunState(status.value, dashboard.value.hasMatches ? 1 : 0) : null,
)

function setTimeframe(value: string) {
  void router.push({ query: withQueryValue(route.query, 'timeframe', value, 'alltime') })
}

function chooseFormat(value: string) {
  void router.push({ query: withQueryValue(route.query, 'format', value, '') })
}
</script>

<template>
  <div class="flex flex-col gap-4 p-3 lg:p-4">
    <div class="flex flex-wrap items-center justify-between gap-3">
      <TimeframeFilter :model-value="timeframe" @update:model-value="setTimeframe" />
      <SegmentedControl
        v-if="formatSwitch"
        class="flex-wrap"
        :model-value="switchFormat"
        :options="choices"
        :disabled="!dashboard"
        @update:model-value="chooseFormat"
      />
    </div>

    <p v-if="importHint" class="text-center text-sm text-muted-foreground">
      Used MyMTGO 0.x?
      <RouterLink to="/import" class="underline">Import your data</RouterLink>
    </p>

    <p v-if="error" class="text-sm text-destructive">Could not load the dashboard: {{ errorMessage(error) }}</p>

    <div v-else-if="!dashboard" class="grid grid-cols-1 gap-4 lg:grid-cols-3">
      <div class="flex flex-col gap-4 lg:col-span-2">
        <Skeleton class="h-96 rounded-xl bg-muted" />
        <Skeleton class="h-48 rounded-xl bg-muted" />
      </div>
      <div class="flex flex-col gap-4">
        <Skeleton v-for="n in 3" :key="n" class="h-40 rounded-xl bg-muted" />
      </div>
    </div>

    <FirstRunState v-else-if="firstRun && status" :state="firstRun" :status="status" />

    <div v-else-if="!dashboard.hasMatches" class="flex flex-col items-center gap-2 py-16 text-center">
      <LayoutDashboard class="size-10 text-muted-foreground/40" />
      <p class="font-medium">No match data yet</p>
      <p class="text-sm text-muted-foreground">Start the file watcher in Settings to begin tracking your MTGO matches.</p>
    </div>

    <div v-else class="grid grid-cols-1 items-start gap-4 lg:grid-cols-3">
      <DashboardWrappedCard v-if="wrapped && !wrappedDismissed" class="lg:col-span-3" :bundle="wrapped" @dismiss="dismissWrapped" />
      <div class="flex flex-col gap-4 lg:col-span-2">
        <ResultsCard
          v-if="dashboard.results"
          :results="dashboard.results"
          :timeframe="timeframe"
          :empty-text="noMatchesText(format, dashboard.formats, platform)"
        />
        <template v-if="ranking">
          <RankCard v-for="line in dashboard.rank ?? []" :key="line.ladder" :line="line" />
        </template>
        <LeagueResultsCard
          v-if="dashboard.leagueResults && leagues"
          :view="dashboard.leagueResults"
          :limited="format === 'limited'"
        />
        <TopMatchupsCard v-if="dashboard.matchups" :matchups="dashboard.matchups" />
        <YourColoursCard v-if="dashboard.yourColours" :colours="dashboard.yourColours" />
        <!-- On Arena the rank cards take the main column, so recent matches move right. -->
        <RecentMatchesCard
          v-if="dashboard.recentMatches && !ranking"
          :matches="dashboard.recentMatches"
          :now="dataUpdatedAt || undefined"
        />
      </div>
      <div class="flex flex-col gap-4">
        <RecentMatchesCard
          v-if="dashboard.recentMatches && ranking"
          :matches="dashboard.recentMatches"
          :now="dataUpdatedAt || undefined"
        />
        <LeagueCard v-if="dashboard.league && leagues" :league="dashboard.league" :now="dataUpdatedAt || undefined" />
        <RecentCard v-if="dashboard.recent" :entries="dashboard.recent" :format="format" :now="dataUpdatedAt || undefined" />
        <MostPickedCard v-if="dashboard.mostPicked" :key="format" :view="dashboard.mostPicked" />
        <LastSessionCard v-if="dashboard.lastSession" :session="dashboard.lastSession" />
        <BreakdownCard v-if="dashboard.breakdown" :breakdown="dashboard.breakdown" />
      </div>
    </div>
  </div>
</template>
