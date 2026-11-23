<script setup lang="ts">
import { Plus } from 'lucide-vue-next'
import { computed, ref } from 'vue'
import { useRouter } from 'vue-router'
import ArchetypeDetectionBanner from '@/components/decks/ArchetypeDetectionBanner.vue'
import TimeframeFilter from '@/components/decks/TimeframeFilter.vue'
import ManualMatchSheet from '@/components/matches/ManualMatchSheet.vue'
import MatchListFilters from '@/components/matches/MatchListFilters.vue'
import MatchesTable from '@/components/MatchesTable.vue'
import { Button } from '@/components/ui/button'
import { Card, CardContent } from '@/components/ui/card'
import PagedNav from '@/components/PagedNav.vue'
import { Skeleton } from '@/components/ui/skeleton'
import { useArchetypeDetection } from '@/composables/useArchetypeDetection'
import { useDeckBestOfContext } from '@/composables/useDeckBestOf'
import { useDeckPage } from '@/composables/useDeckPage'
import { useUrlMatchList } from '@/composables/useUrlMatchList'
import { usePlatformContext } from '@/composables/usePlatformContext'
import { errorMessage } from '@/lib/commands'
import { deckLinkQuery, deckMatchPath } from '@/lib/deckRoute'
import { NO_DECK_MATCHES_TEXT, withListQuery } from '@/lib/matchList'
import { showingText } from '@/lib/paging'
import { canAddManualMatch } from '@/lib/platform'
import { useDeckHeader, useDeckMatches } from '@/lib/queries'

const router = useRouter()
const { deckId, version, timeframe } = useDeckPage()
const { bestOf, bo1Mode } = useDeckBestOfContext()

// The filters, sort and page sit in the URL beside `?version=` and
// `?timeframe=`, so going back from a match finds the list as it was. A new
// version (the sidebar) starts the list afresh; a new timeframe keeps the
// filters and goes back to the first page.
const { list, query, show, sort, stepBackFromEmptyPage } = useUrlMatchList({ singleResult: bo1Mode })
const { data: matches, error } = useDeckMatches(deckId, version, timeframe, query, bestOf)
// A refresh can empty the page being viewed.
stepBackFromEmptyPage(matches)

function setTimeframe(next: string) {
  const deckQuery = deckLinkQuery({ version: version.value, timeframe: next }, true)
  void router.push({ query: withListQuery(deckQuery, { ...list.value, page: 1 }) })
}

/** A row opens inside the deck, carrying the version and timeframe. */
function matchRoute(token: string): string {
  return deckMatchPath(deckId.value, token, deckLinkQuery({ version: version.value, timeframe: timeframe.value }, true))
}

const { platform } = usePlatformContext()
const manualMatchSheet = ref<InstanceType<typeof ManualMatchSheet> | null>(null)
// The deck's format, for the manual match's archetype picker.
const { data: deckHeader } = useDeckHeader(deckId, computed(() => null), bestOf)

// "Detect" on a row, and the banner while the archetype filter is set.
const { detectingTokens, detectMatches } = useArchetypeDetection()
</script>

<template>
  <div class="space-y-4 p-3 lg:p-4">
    <TimeframeFilter :model-value="timeframe" @update:model-value="setTimeframe" />

    <p v-if="error" class="text-sm text-destructive">Could not load matches: {{ errorMessage(error) }}</p>

    <div v-else-if="matches" class="flex flex-col gap-4">
      <ArchetypeDetectionBanner
        v-if="list.archetype"
        :deck-id="deckId"
        :filter="list.archetype"
        :archived="deckHeader?.deck.archived ?? false"
      />

      <MatchListFilters
        :state="list"
        hide-best-of
        :summary="showingText(matches)"
        :archetypes="matches.archetypes"
        :unknown-count="matches.unknownArchetypeCount"
        @change="show"
      >
        <Button
          v-if="canAddManualMatch(platform)"
          size="sm"
          variant="outline"
          class="h-8 text-xs"
          @click="manualMatchSheet?.open()"
        >
          <Plus class="size-3.5" />
          Add manual match
        </Button>
      </MatchListFilters>

      <Card class="gap-0 overflow-hidden p-0">
        <CardContent class="px-0">
          <p v-if="!matches.total" class="py-8 text-center text-sm text-muted-foreground">{{ NO_DECK_MATCHES_TEXT }}</p>
          <MatchesTable
            v-else
            :rows="matches.rows"
            sortable
            :single-result="bo1Mode"
            hide-kind
            selectable
            detectable
            :detecting="detectingTokens"
            :match-route="matchRoute"
            :sort-by="list.sortBy"
            :sort-dir="list.sortDir"
            @sort="sort"
            @detect="detectMatches"
          />
        </CardContent>

        <div v-if="matches.total > matches.perPage" class="justify-end py-2 text-right">
          <PagedNav
            :per-page="matches.perPage"
            :total="matches.total"
            :page="matches.page"
            @update:page="(page: number) => show({ ...list, page })"
          />
        </div>
      </Card>
    </div>

    <div v-else class="space-y-2">
      <Skeleton v-for="n in 8" :key="n" class="h-8 w-full" />
    </div>

    <ManualMatchSheet ref="manualMatchSheet" :deck-id="deckId" :format="deckHeader?.deck.format ?? null" />
  </div>
</template>
