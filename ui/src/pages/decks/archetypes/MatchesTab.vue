<script setup lang="ts">
import { computed, ref } from 'vue'
import { useRouter } from 'vue-router'
import DeckIndexLayout from '@/components/decks/DeckIndexLayout.vue'
import TimeframeFilter from '@/components/decks/TimeframeFilter.vue'
import MatchListFilters from '@/components/matches/MatchListFilters.vue'
import MatchesTable from '@/components/MatchesTable.vue'
import { Card, CardContent } from '@/components/ui/card'
import PagedNav from '@/components/PagedNav.vue'
import { Skeleton } from '@/components/ui/skeleton'
import { useArchetypeDetection } from '@/composables/useArchetypeDetection'
import { useArchetypeTab } from '@/composables/useArchetypeTab'
import { useUrlMatchList } from '@/composables/useUrlMatchList'
import { errorMessage } from '@/lib/commands'
import { matchPathInDecks } from '@/lib/deckRoute'
import { decksByToken } from '@/lib/format'
import { NO_DECK_MATCHES_TEXT, withListQuery } from '@/lib/matchList'
import { showingText } from '@/lib/paging'
import { useArchetypeDeckMatches } from '@/lib/queries'

// The deck index's archetype Matches tab (0.x `decks/archetypes/Matches`):
// the complete matches of every deck of the archetype, with their deck,
// through the match list's filters (kept in the URL).
const router = useRouter()
const { archetypeId, format, timeframe, settings, shell, error } = useArchetypeTab()

const { list, query, show, sort, stepBackFromEmptyPage } = useUrlMatchList()
const { data: found, error: matchesError } = useArchetypeDeckMatches(archetypeId, format, timeframe, query)
const matches = computed(() => found.value?.page ?? null)
// A refresh can empty the page being viewed.
stepBackFromEmptyPage(matches)

/** Each row's deck, for the Deck column. */
const decks = computed(() => decksByToken(found.value?.decks ?? []))

/** A row opens inside its deck's layout, so Decks stays active (issue #332). */
function matchRoute(token: string): string {
  return matchPathInDecks(decks.value[token]?.id, token)
}

function setTimeframe(next: string) {
  const base: Record<string, string> = next === 'alltime' ? {} : { timeframe: next }
  void router.push({ query: withListQuery(base, { ...list.value, page: 1 }) })
}

const { detectingTokens, detectMatches } = useArchetypeDetection()
const search = ref('')
</script>

<template>
  <div class="flex min-h-0 flex-1 flex-col">
    <p v-if="error" class="p-4 text-sm text-destructive">Could not load the archetype: {{ errorMessage(error) }}</p>
    <DeckIndexLayout
      v-else-if="shell && settings"
      v-model:search="search"
      tab="matches"
      :timeframe="timeframe"
      :settings="settings"
      :format-options="shell.formatOptions"
      :archetype-options="shell.archetypeOptions"
      :unclassified-count="shell.unclassifiedCount"
      :archetype-header="shell.archetypeHeader"
      :recent="false"
    >
      <template #toolbar>
        <TimeframeFilter :model-value="timeframe" @update:model-value="setTimeframe" />
      </template>

      <p v-if="matchesError" class="text-sm text-destructive">Could not load matches: {{ errorMessage(matchesError) }}</p>

      <div v-else-if="matches" class="flex flex-col gap-4">
        <MatchListFilters
          :state="list"
          :summary="showingText(matches)"
          :archetypes="matches.archetypes"
          :unknown-count="matches.unknownArchetypeCount"
          @change="show"
        />

        <Card class="gap-0 overflow-hidden p-0">
          <CardContent class="px-0">
            <p v-if="!matches.total" class="py-8 text-center text-sm text-muted-foreground">{{ NO_DECK_MATCHES_TEXT }}</p>
            <MatchesTable
              v-else
              :rows="matches.rows"
              :decks="decks"
              :match-route="matchRoute"
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
    </DeckIndexLayout>
    <div v-else class="space-y-2 p-4">
      <Skeleton v-for="n in 6" :key="n" class="h-8 w-full" />
    </div>
  </div>
</template>
