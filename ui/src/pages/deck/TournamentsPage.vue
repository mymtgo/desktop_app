<script setup lang="ts">
import TimeframeFilter from '@/components/decks/TimeframeFilter.vue'
import TournamentCard from '@/components/tournaments/TournamentCard.vue'
import { Card, CardContent } from '@/components/ui/card'
import { Skeleton } from '@/components/ui/skeleton'
import { useDeckPage } from '@/composables/useDeckPage'
import { errorMessage } from '@/lib/commands'
import { useDeckTournaments } from '@/lib/queries'

// 0.x `pages/decks/Tournaments.vue` and `DeckTournaments.vue`: the deck's
// tournaments with a complete match in the timeframe (of the chosen version),
// newest first, the first one expanded.
const { deckId, version, timeframe, setTimeframe } = useDeckPage()
const { data: tournaments, error, dataUpdatedAt } = useDeckTournaments(deckId, version, timeframe)
</script>

<template>
  <div class="space-y-4 p-3 lg:p-4">
    <TimeframeFilter :model-value="timeframe" @update:model-value="setTimeframe" />

    <p v-if="error" class="text-sm text-destructive">Could not load tournaments: {{ errorMessage(error) }}</p>

    <div v-else-if="tournaments" class="flex flex-col gap-3">
      <p v-if="!tournaments.length" class="py-8 text-center text-sm text-muted-foreground">
        No tournaments found for this deck.
      </p>
      <TournamentCard
        v-for="(tournament, index) in tournaments"
        :key="tournament.eventId"
        :tournament="tournament"
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
