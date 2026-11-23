<script setup lang="ts">
import TimeframeFilter from '@/components/decks/TimeframeFilter.vue'
import LeagueCard from '@/components/leagues/LeagueCard.vue'
import { Card, CardContent } from '@/components/ui/card'
import { Skeleton } from '@/components/ui/skeleton'
import { useDeckPage } from '@/composables/useDeckPage'
import { errorMessage } from '@/lib/commands'
import { useDeckLeagues } from '@/lib/queries'

// 0.x `pages/decks/Leagues.vue` and `DeckLeagues.vue`: the deck's runs started
// in the timeframe (of the chosen version), newest first, cards without the
// deck's identity, the first one expanded.
const { deckId, version, timeframe, setTimeframe } = useDeckPage()
const { data: leagues, error, dataUpdatedAt } = useDeckLeagues(deckId, version, timeframe)
</script>

<template>
  <div class="space-y-4 p-3 lg:p-4">
    <TimeframeFilter :model-value="timeframe" @update:model-value="setTimeframe" />

    <p v-if="error" class="text-sm text-destructive">Could not load leagues: {{ errorMessage(error) }}</p>

    <div v-else-if="leagues" class="flex flex-col gap-3">
      <p v-if="!leagues.runs.length" class="py-8 text-center text-sm text-muted-foreground">
        No league runs found for this deck.
      </p>
      <LeagueCard
        v-for="(run, index) in leagues.runs"
        :key="run.key"
        :league="run"
        hide-deck-identity
        :default-expanded="index === 0"
        :now="dataUpdatedAt"
      />
    </div>

    <div v-else class="flex flex-col gap-3">
      <Card v-for="i in 3" :key="i" class="gap-0 py-0">
        <CardContent class="p-4"><Skeleton class="h-16 w-full" /></CardContent>
      </Card>
    </div>
  </div>
</template>
