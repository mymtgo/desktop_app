<script setup lang="ts">
import { computed } from 'vue'
import type { BestOfView, MatchupView } from '@/bindings'
import ManaColors from '@/components/mana/ManaColors.vue'
import MatchupTrendChart from '@/components/decks/MatchupTrendChart.vue'
import { Sheet, SheetContent, SheetDescription, SheetHeader, SheetTitle } from '@/components/ui/sheet'
import { Skeleton } from '@/components/ui/skeleton'
import { deckLinkQuery, deckMatchPath } from '@/lib/deckRoute'
import { winRateTone } from '@/lib/decks'
import { winRateTextClass } from '@/lib/matchups'
import { useDeckMatchupDetail } from '@/lib/queries'

/**
 * One matchup in detail (0.x `MatchupDrawer`): win rate by game, on the
 * play and on the draw, average turns and mulligans, the on-play rate and
 * the match history. Read when a row is chosen.
 */
const props = defineProps<{
  deckId: number
  matchup: MatchupView | null
  timeframe: string
  version: number | null
  bestOf: BestOfView | null
}>()

const emit = defineEmits<{ close: [] }>()

const archetypeId = computed(() => props.matchup?.archetypeId ?? null)
const { data: detail, isFetching } = useDeckMatchupDetail(
  computed(() => props.deckId),
  archetypeId,
  computed(() => props.version),
  computed(() => props.timeframe),
  computed(() => props.bestOf),
)

/** A match of the history opens inside the deck (issue #332), keeping the version and timeframe. */
function matchPath(token: string): string {
  return deckMatchPath(props.deckId, token, deckLinkQuery({ version: props.version, timeframe: props.timeframe }, true))
}

function barColor(rate: number): string {
  const tone = winRateTone(rate)
  if (tone === 'win') return 'bg-success'
  if (tone === 'loss') return 'bg-destructive'
  return 'bg-muted-foreground'
}

function dotClass(result: boolean | null | undefined): string {
  if (result === true) return 'bg-success'
  if (result === false) return 'bg-destructive'
  return 'border border-muted-foreground/30'
}
</script>

<template>
  <Sheet :open="matchup !== null" @update:open="(open) => !open && emit('close')">
    <SheetContent side="right" class="w-[560px] overflow-y-auto sm:max-w-[560px]">
      <SheetHeader v-if="matchup">
        <div class="flex items-center gap-2">
          <ManaColors :colors="matchup.colors" class="shrink-0" />
          <SheetTitle>{{ matchup.name }}</SheetTitle>
        </div>
        <SheetDescription class="flex gap-4 text-sm">
          <span>
            Record: <span class="font-medium" :class="winRateTextClass(matchup.matchWinRate)">{{ matchup.matchRecord }}</span>
          </span>
          <span>
            Win Rate: <span class="font-medium" :class="winRateTextClass(matchup.matchWinRate)">{{ matchup.matchWinRate }}%</span>
          </span>
          <span>{{ matchup.matches }} matches</span>
        </SheetDescription>
      </SheetHeader>

      <div v-if="isFetching && !detail" class="flex flex-col gap-4 p-6">
        <Skeleton class="h-6 w-48" />
        <div class="grid grid-cols-2 gap-3">
          <Skeleton class="h-24" />
          <Skeleton class="h-24" />
        </div>
        <div class="grid grid-cols-3 gap-3">
          <Skeleton class="h-20" />
          <Skeleton class="h-20" />
          <Skeleton class="h-20" />
        </div>
      </div>

      <div v-else-if="detail" class="flex flex-col gap-0">
        <div class="border-b border-border px-6 py-3">
          <h3 class="mb-2 text-xs font-semibold tracking-wide text-muted-foreground uppercase">Win Rate Trend</h3>
          <MatchupTrendChart :history="detail.history" />
        </div>

        <div class="border-b border-border px-6 py-3">
          <h3 class="mb-2 text-xs font-semibold tracking-wide text-muted-foreground uppercase">Win Rate by Game</h3>
          <div class="grid gap-3" :class="detail.perGame.length === 3 ? 'grid-cols-3' : 'grid-cols-2'">
            <div v-for="game in detail.perGame" :key="game.gameNumber" class="rounded-lg border border-border bg-card p-3">
              <div class="text-xs tracking-wide text-muted-foreground uppercase">Game {{ game.gameNumber }}</div>
              <div class="mt-1.5 text-2xl font-semibold tabular-nums" :class="winRateTextClass(game.winRate)">{{ game.winRate }}%</div>
              <div class="mt-0.5 text-xs text-muted-foreground tabular-nums">{{ game.record }}</div>
            </div>
          </div>
        </div>

        <div class="border-b border-border px-6 py-3">
          <h3 class="mb-2 text-xs font-semibold tracking-wide text-muted-foreground uppercase">Play / Draw Breakdown</h3>
          <div class="grid grid-cols-2 gap-3">
            <div
              v-for="side in [
                { label: 'On the Play', rate: detail.otpWinRate, record: detail.otpRecord },
                { label: 'On the Draw', rate: detail.otdWinRate, record: detail.otdRecord },
              ]"
              :key="side.label"
              class="rounded-lg border border-border bg-card p-3"
            >
              <div class="mb-2 flex items-center justify-between">
                <span class="text-xs font-medium text-muted-foreground">{{ side.label }}</span>
                <span class="text-lg font-semibold tabular-nums" :class="winRateTextClass(side.rate)">{{ side.rate }}%</span>
              </div>
              <div class="mb-2 h-1.5 overflow-hidden rounded-full bg-muted">
                <div class="h-full rounded-full transition-all" :class="barColor(side.rate)" :style="{ width: side.rate + '%' }" />
              </div>
              <div class="text-xs text-muted-foreground tabular-nums">{{ side.record }} games</div>
            </div>
          </div>
        </div>

        <div class="border-b border-border px-6 py-3">
          <h3 class="mb-2 text-xs font-semibold tracking-wide text-muted-foreground uppercase">Game Stats</h3>
          <div class="grid grid-cols-3 gap-3">
            <div class="rounded-lg border border-border bg-card p-3 text-center">
              <div class="text-xl font-semibold tabular-nums">{{ detail.avgTurns ?? '-' }}</div>
              <div class="mt-1 text-xs tracking-wide text-muted-foreground uppercase">Avg Turns</div>
            </div>
            <div class="rounded-lg border border-border bg-card p-3 text-center">
              <div class="text-xl font-semibold tabular-nums">{{ detail.avgMulligans ?? '-' }}</div>
              <div class="mt-1 text-xs tracking-wide text-muted-foreground uppercase">Avg Mulligans</div>
            </div>
            <div class="rounded-lg border border-border bg-card p-3 text-center">
              <div class="text-xl font-semibold tabular-nums">{{ detail.onPlayRate }}%</div>
              <div class="mt-1 text-xs tracking-wide text-muted-foreground uppercase">On Play Rate</div>
            </div>
          </div>
        </div>

        <div class="px-6 py-3">
          <h3 class="mb-2 text-xs font-semibold tracking-wide text-muted-foreground uppercase">Match History</h3>
          <div class="flex flex-col">
            <RouterLink
              v-for="match in detail.history"
              :key="match.token"
              :to="matchPath(match.token)"
              class="flex items-center gap-3 border-b border-border py-2 last:border-b-0 hover:bg-muted/30"
            >
              <span class="w-12 shrink-0 text-xs text-muted-foreground">{{ match.dateLabel }}</span>
              <span class="min-w-0 flex-1 truncate text-xs">{{ match.opponent }}</span>
              <span class="flex shrink-0 gap-1">
                <span v-for="i in 3" :key="i" class="size-2 rounded-full" :class="dotClass(match.gameResults[i - 1])" />
              </span>
              <span
                class="w-8 shrink-0 text-right text-sm font-semibold tabular-nums"
                :class="match.outcome === 'win' ? 'text-success' : match.outcome === 'loss' ? 'text-destructive' : ''"
              >
                {{ match.score }}
              </span>
            </RouterLink>
          </div>
        </div>
      </div>
    </SheetContent>
  </Sheet>
</template>
