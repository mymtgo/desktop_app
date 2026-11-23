<script setup lang="ts">
import { Plus, Trophy } from 'lucide-vue-next'
import { computed, ref, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import type { LeagueListQuery, RunCardView } from '@/bindings'
import LeagueCard from '@/components/leagues/LeagueCard.vue'
import LeagueFilters from '@/components/leagues/LeagueFilters.vue'
import LeagueKpis from '@/components/leagues/LeagueKpis.vue'
import ManualLeagueDialog from '@/components/leagues/ManualLeagueDialog.vue'
import ManualMatchSheet from '@/components/matches/ManualMatchSheet.vue'
import { Button } from '@/components/ui/button'
import PagedNav from '@/components/PagedNav.vue'
import { Skeleton } from '@/components/ui/skeleton'
import { leagueQueryFromUrl, leagueUrlQuery } from '@/lib/leagues'
import { useLeagueList } from '@/lib/queries'

// Every filter lives in the URL (0.x kept them in the query string too), so
// back and forward step through them.
const route = useRoute()
const router = useRouter()

const query = computed<LeagueListQuery>(() => leagueQueryFromUrl(route.query))

const { data: list, error, isPending, isPlaceholderData, dataUpdatedAt } = useLeagueList(query)

function setQuery(next: LeagueListQuery) {
  void router.push({ query: leagueUrlQuery(next) })
}

function goToPage(page: number) {
  setQuery({ ...query.value, page })
}

// A page past the end shows the last page: follow it rather than keep a page
// number that is not shown.
watch(list, (shown) => {
  if (!shown || isPlaceholderData.value || shown.page === query.value.page) return
  void router.replace({ query: leagueUrlQuery({ ...query.value, page: shown.page }) })
})

const createDialog = ref<InstanceType<typeof ManualLeagueDialog> | null>(null)
const manualMatchSheet = ref<InstanceType<typeof ManualMatchSheet> | null>(null)

/** A run card's "Add manual match": the sheet opens on that run and its deck. */
function createMatchFor(run: RunCardView) {
  manualMatchSheet.value?.open({ league: run.key, deckId: run.deck?.id ?? null })
}
</script>

<template>
  <div class="flex flex-col gap-4 p-3 lg:p-4">
    <div class="flex items-center justify-between gap-2">
      <h1 class="text-base font-semibold tracking-tight">Leagues</h1>
      <div class="flex items-center gap-2">
        <Button size="sm" variant="outline" @click="manualMatchSheet?.open()">
          <Plus class="size-4" />
          Add manual match
        </Button>
        <Button size="sm" @click="createDialog?.open()">
          <Plus class="size-4" />
          Create league
        </Button>
      </div>
    </div>

    <p v-if="error" class="text-sm text-destructive">Could not load leagues: {{ error.message }}</p>

    <template v-else-if="isPending || !list">
      <div class="grid grid-cols-2 gap-4 md:grid-cols-3 lg:grid-cols-6">
        <Skeleton v-for="n in 6" :key="n" class="h-24 w-full" />
      </div>
      <Skeleton v-for="n in 4" :key="n" class="h-16 w-full" />
    </template>

    <template v-else>
      <LeagueKpis :kpis="list.kpis" />

      <LeagueFilters
        :query="query"
        :formats="list.formatOptions"
        :decks="list.deckOptions"
        :archetypes="list.deckArchetypes"
        @change="setQuery"
      />

      <div v-if="!list.runs.length" class="flex flex-col items-center gap-2 py-16 text-center">
        <Trophy class="size-10 text-muted-foreground/40" />
        <p class="font-medium">No league runs match.</p>
        <p class="text-sm text-muted-foreground">Adjust filters or wait for new runs to ingest.</p>
      </div>

      <div v-else class="flex flex-col gap-3">
        <LeagueCard
          v-for="(run, index) in list.runs"
          :key="run.key"
          :league="run"
          :default-expanded="index === 0"
          :now="dataUpdatedAt"
          can-create-match
          @create-match="createMatchFor"
        />
      </div>

      <PagedNav
        v-if="list.lastPage > 1"
        class="justify-end px-2 py-2"
        :per-page="list.perPage"
        :total="list.total"
        :page="list.page"
        @update:page="goToPage"
      />
    </template>

    <ManualLeagueDialog ref="createDialog" :decks="list?.manualDeckOptions ?? []" />
    <ManualMatchSheet ref="manualMatchSheet" :decks="list?.manualDeckOptions ?? []" />
  </div>
</template>
