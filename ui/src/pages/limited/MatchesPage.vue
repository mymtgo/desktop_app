<script setup lang="ts">
import { Clock, Swords, Target, Timer } from 'lucide-vue-next'
import { computed, ref } from 'vue'
import MatchesTable from '@/components/MatchesTable.vue'
import { Card, CardContent } from '@/components/ui/card'
import { Skeleton } from '@/components/ui/skeleton'
import { errorMessage } from '@/lib/commands'
import { formatSeconds, limitedMatchPath } from '@/lib/limited'
import { avgMatchText, matchesHeaderText, sortLimitedMatches, totalMinutesText } from '@/lib/limitedPages'
import { nextSort, type SortColumn, type SortDir } from '@/lib/matchList'
import { useLimitedMatches } from '@/lib/queries'
import { useLimitedKey } from '@/pages/limited/useLimitedKey'

/**
 * The event's league matches (0.x `pages/limited/Matches.vue`): the header
 * line, the record, average match, play and draw and total time tiles, and
 * the matches with the opponents' colours. The whole run is in the payload,
 * so sorting stays on the client; a row opens the match inside the event.
 */
const key = useLimitedKey()
const { data: page, error } = useLimitedMatches(key)

const sortBy = ref<SortColumn | null>(null)
const sortDir = ref<SortDir>('desc')

function updateSort(column: SortColumn) {
  const next = nextSort({ sortBy: sortBy.value, sortDir: sortDir.value }, column)
  sortBy.value = next.sortBy
  sortDir.value = next.sortDir
}

const sorted = computed(() => sortLimitedMatches(page.value?.matches ?? [], sortBy.value, sortDir.value))
const header = computed(() => (page.value ? matchesHeaderText(page.value.matches.length, page.value.kpis) : ''))

function matchRoute(token: string): string {
  return limitedMatchPath(key.value, token)
}
</script>

<template>
  <div class="flex flex-col gap-4 p-3 lg:p-4">
    <p v-if="error" class="text-sm text-destructive">Could not load the matches: {{ errorMessage(error) }}</p>
    <template v-else-if="page === undefined">
      <Skeleton class="h-10 w-80" />
      <div class="grid grid-cols-2 gap-4 lg:grid-cols-4">
        <Skeleton v-for="index in 4" :key="index" class="h-24 w-full" />
      </div>
      <Skeleton class="h-64 w-full" />
    </template>
    <template v-else-if="page">
      <div class="flex flex-col">
        <h1 class="text-base font-semibold tracking-tight">Matches</h1>
        <p class="text-sm text-muted-foreground">{{ header }}</p>
      </div>

      <div class="grid grid-cols-2 gap-4 lg:grid-cols-4">
        <Card class="gap-0 py-0">
          <CardContent class="flex flex-col gap-0.5 p-3">
            <span class="inline-flex items-center gap-1 text-xs tracking-wide text-muted-foreground uppercase">
              <Target class="size-3" /> Record
            </span>
            <span class="text-3xl font-bold tabular-nums">{{ page.kpis.wins }}-{{ page.kpis.losses }}</span>
            <span class="text-sm text-muted-foreground">games {{ page.kpis.gameWins }}-{{ page.kpis.gameLosses }}</span>
          </CardContent>
        </Card>
        <Card class="gap-0 py-0">
          <CardContent class="flex flex-col gap-0.5 p-3">
            <span class="inline-flex items-center gap-1 text-xs tracking-wide text-muted-foreground uppercase">
              <Timer class="size-3" /> Avg match
            </span>
            <span class="text-3xl font-bold tabular-nums">{{ avgMatchText(page.kpis.avgMatchSeconds) }}</span>
            <span class="text-sm text-muted-foreground">{{ formatSeconds(page.kpis.avgMatchSeconds) }}</span>
          </CardContent>
        </Card>
        <Card class="gap-0 py-0">
          <CardContent class="flex flex-col gap-0.5 p-3">
            <span class="inline-flex items-center gap-1 text-xs tracking-wide text-muted-foreground uppercase">
              <Swords class="size-3" /> On the play
            </span>
            <span class="text-3xl font-bold tabular-nums">{{ page.kpis.onPlay.wins }}-{{ page.kpis.onPlay.losses }}</span>
            <span class="text-sm text-muted-foreground">
              on the draw {{ page.kpis.onDraw.wins }}-{{ page.kpis.onDraw.losses }}
            </span>
          </CardContent>
        </Card>
        <Card class="gap-0 py-0">
          <CardContent class="flex flex-col gap-0.5 p-3">
            <span class="inline-flex items-center gap-1 text-xs tracking-wide text-muted-foreground uppercase">
              <Clock class="size-3" /> Total time
            </span>
            <span class="text-3xl font-bold tabular-nums">{{ totalMinutesText(page.kpis.totalMinutes) }}</span>
            <span class="text-sm text-muted-foreground">first queue to last result</span>
          </CardContent>
        </Card>
      </div>

      <div v-if="!page.matches.length" class="flex flex-col items-center gap-2 py-16 text-center">
        <Swords class="size-10 text-muted-foreground/40" />
        <p class="font-medium">No league matches yet.</p>
        <p class="text-sm text-muted-foreground">Matches attach here as soon as they finish.</p>
      </div>
      <Card v-else class="gap-0 overflow-hidden p-0">
        <CardContent class="px-0">
          <MatchesTable
            :rows="sorted"
            sortable
            :sort-by="sortBy"
            :sort-dir="sortDir"
            show-colours
            :match-route="matchRoute"
            @sort="updateSort"
          />
        </CardContent>
      </Card>
    </template>
  </div>
</template>
