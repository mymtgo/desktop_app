<script setup lang="ts">
import { computed } from 'vue'
import FirstRunState from '@/components/FirstRunState.vue'
import MatchListFilters from '@/components/matches/MatchListFilters.vue'
import MatchesTable from '@/components/MatchesTable.vue'
import { Card, CardContent } from '@/components/ui/card'
import PagedNav from '@/components/PagedNav.vue'
import { Skeleton } from '@/components/ui/skeleton'
import { usePlatformContext } from '@/composables/usePlatformContext'
import { useArchetypeDetection } from '@/composables/useArchetypeDetection'
import { useUrlMatchList } from '@/composables/useUrlMatchList'
import { isFiltered, noMatchesRecordedText } from '@/lib/matchList'
import { showingText } from '@/lib/paging'
import { hasFeature } from '@/lib/platformFeatures'
import { decksByToken } from '@/lib/format'
import { useMatchDecks, useMatchesPage, useStatus } from '@/lib/queries'
import { firstRunState } from '@/lib/status'

// The filters, sort and page live in the URL, as on a deck's Matches page.
const { list, query, show, sort, stepBackFromEmptyPage } = useUrlMatchList({ keepOtherQuery: false })
const { data: matches, isPending, error } = useMatchesPage(query)
const { data: status } = useStatus()
const { platform } = usePlatformContext()
const { detectingTokens, detectMatches } = useArchetypeDetection()
// The Deck column (0.x's table has it as an option); a match with no deck says Unknown.
const { data: rowDecks } = useMatchDecks(computed(() => matches.value?.rows.map((row) => row.token) ?? []))
const decks = computed(() => decksByToken(rowDecks.value ?? []))
// A filter that finds nothing is not a first run; the first-run screens are about MTGO only.
const firstRun = computed(() =>
  isFiltered(list.value) || !hasFeature(platform.value, 'logTrackerStatus') ? null : firstRunState(status.value, matches.value?.total, matches.value?.rows.length),
)

stepBackFromEmptyPage(matches)
</script>

<template>
  <div class="flex flex-1 flex-col gap-4 p-4">
    <FirstRunState v-if="firstRun && status" :state="firstRun" :status="status" />
    <template v-else>
      <p v-if="error" class="text-sm text-destructive">Could not load matches: {{ error.message }}</p>
      <div v-else-if="isPending" class="space-y-2">
        <Skeleton v-for="n in 8" :key="n" class="h-8 w-full" />
      </div>
      <template v-else-if="matches">
        <MatchListFilters
          :state="list"
          :summary="showingText(matches)"
          :archetypes="matches.archetypes"
          :unknown-count="matches.unknownArchetypeCount"
          @change="show"
        />
        <Card class="gap-0 overflow-hidden p-0">
          <CardContent class="px-0">
            <p v-if="!matches.rows.length" class="py-8 text-center text-sm text-muted-foreground">{{ noMatchesRecordedText(platform) }}</p>
            <MatchesTable
              v-else
              :rows="matches.rows"
              :decks="decks"
              sortable
              selectable
              detectable
              :detecting="detectingTokens"
              :sort-by="list.sortBy"
              :sort-dir="list.sortDir"
              @sort="sort"
              @detect="detectMatches"
            />
          </CardContent>
          <div v-if="matches.total > matches.perPage" class="justify-end py-2 text-right">
            <PagedNav
              show-ellipsis
              :per-page="matches.perPage"
              :total="matches.total"
              :page="matches.page"
              @update:page="(page: number) => show({ ...list, page })"
            />
          </div>
        </Card>
      </template>
    </template>
  </div>
</template>
