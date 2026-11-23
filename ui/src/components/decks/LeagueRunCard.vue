<script setup lang="ts">
import { Camera, Download } from 'lucide-vue-next'
import { computed } from 'vue'
import type { PanelRunView } from '@/bindings'
import LeagueRunHeader from '@/components/leagues/LeagueRunHeader.vue'
import RunSummaryStats from '@/components/leagues/RunSummaryStats.vue'
import LeagueScreenshot from '@/components/screenshots/LeagueScreenshot.vue'
import { Button } from '@/components/ui/button'
import { Card, CardContent } from '@/components/ui/card'
import { useOffscreenScreenshot } from '@/composables/useOffscreenScreenshot'
import {
  gameRecordText,
  oddsNote,
  roundsLeftText,
  runCardTitle,
  signedNumber,
  winsForPrizeText,
} from '@/lib/leagueRunCard'
import { runCardHeader } from '@/lib/leagueRunHeader'
import { runRecord } from '@/lib/leagues'
import { screenshotFileName } from '@/lib/screenshots'

/**
 * The deck dashboard's league run (0.x `LeagueRunCard`): the run being played
 * with its odds and EV, or else the latest finished run with its tix. The
 * header is the Leagues page's (`LeagueRunHeader`), labelled "League in
 * progress" or "Latest league", with the rounds left and wins for prize.
 */
const props = withDefaults(
  defineProps<{
    panel: PanelRunView
    /** "Now" for the relative start (the data's fetch time); the render time when left out. */
    now?: number
  }>(),
  { now: undefined },
)

const run = computed(() => props.panel.run)
const live = computed(() => run.value.state === 'active')
const odds = computed(() => (live.value ? (props.panel.inProgress?.odds ?? null) : null))
const record = computed(() => runRecord(run.value.results))
const winsForPrize = computed(() => winsForPrizeText(odds.value?.winsForPrize ?? null))
const note = computed(() => {
  const progress = props.panel.inProgress
  return progress ? oddsNote(progress.oddsSource, progress.winProbability) : null
})
const header = computed(() => runCardHeader(run.value, props.now ?? Date.now()))

// "Copy screenshot of league" (0.x `LeagueRunCard`: no cover art here).
const screenshot = useOffscreenScreenshot<true>()
const { shot, busy: capturing, saveOffered, saving } = screenshot
const shotTarget = screenshot.target

function copyScreenshot() {
  void screenshot.take(async () => true, screenshotFileName(run.value.deck?.name, 'league'))
}
</script>

<template>
  <Card class="gap-0 overflow-hidden p-0">
    <LeagueRunHeader :header="header" :now="now">
      <template #title-extra>
        <span class="text-xs text-muted-foreground">{{ runCardTitle(run.state) }}</span>
      </template>
      <template #info-extra>
        <span v-if="odds">
          {{ roundsLeftText(odds.roundsLeft) }}
          <template v-if="winsForPrize">
            <span class="mx-0.5">&middot;</span>
            <span class="font-medium text-foreground">{{ winsForPrize }}</span>
            for prize
          </template>
        </span>
      </template>
      <template #actions>
        <Button
          v-if="saveOffered"
          variant="outline"
          size="sm"
          class="h-7"
          :disabled="saving"
          @click.stop="screenshot.save"
        >
          <Download class="mr-1.5 size-3.5" />
          Save image
        </Button>
        <Button
          variant="ghost"
          size="icon"
          class="size-7 shrink-0"
          :disabled="capturing"
          aria-label="Copy screenshot of league"
          @click.stop="copyScreenshot"
        >
          <Camera class="size-3.5" />
        </Button>
      </template>
    </LeagueRunHeader>

    <CardContent class="flex flex-col p-0">
      <div class="border-t px-4 py-3">
        <RunSummaryStats
          :game-wins="run.gameWins"
          :game-losses="run.gameLosses"
          :on-play-record="run.onPlayRecord"
          :on-draw-record="run.onDrawRecord"
        />
      </div>

      <div v-if="run.matches.length" class="divide-y divide-border/60 border-t">
        <!-- 0.x opened the match; v1 has no match page yet. -->
        <div
          v-for="(match, index) in run.matches"
          :key="match.token"
          class="flex w-full items-center gap-2.5 px-4 py-2 text-left text-sm"
        >
          <span class="w-3 shrink-0 text-xs text-muted-foreground tabular-nums">{{ index + 1 }}</span>
          <div
            class="bevel block h-2 w-2 shrink-0 rounded-full"
            :class="match.result === 'W' ? 'bg-success' : 'bg-destructive'"
          >
            <span class="sr-only">{{ match.result === 'W' ? 'Win' : 'Loss' }}</span>
          </div>
          <span class="shrink-0 font-medium">{{ match.opponentName ?? 'Unknown' }}</span>
          <span class="ml-auto shrink-0 text-xs text-muted-foreground tabular-nums">{{ gameRecordText(match.games) }}</span>
        </div>
      </div>

      <div v-if="odds && odds.chanceOfPrize !== null" class="border-t px-4 py-3">
        <p class="text-xs text-muted-foreground">Expected finish from {{ record.wins }}&ndash;{{ record.losses }}</p>
        <p class="mt-1 text-sm tabular-nums">
          <span class="font-semibold text-success">{{ odds.chanceOfPrize }}%</span>
          <span class="text-muted-foreground"> to reach {{ odds.prizeWins }} wins or better</span>
          <template v-if="odds.expectedTix !== null">
            <span class="mx-1 text-muted-foreground">&middot;</span>
            <span class="font-semibold" :class="odds.expectedTix >= 0 ? 'text-warning' : 'text-destructive'">
              {{ signedNumber(odds.expectedTix) }}
            </span>
            <span class="text-muted-foreground"> tix EV</span>
          </template>
        </p>
        <p v-if="note" class="mt-1 text-xs text-muted-foreground">Projected {{ note }}.</p>
      </div>
    </CardContent>
  </Card>

  <div v-if="shot" style="position: fixed; top: -9999px; left: -9999px; pointer-events: none">
    <LeagueScreenshot ref="shotTarget" :league="run" />
  </div>
</template>
