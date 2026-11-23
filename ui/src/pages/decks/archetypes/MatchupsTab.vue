<script setup lang="ts">
import { ref } from 'vue'
import DeckIndexLayout from '@/components/decks/DeckIndexLayout.vue'
import MatchupSpread from '@/components/decks/MatchupSpread.vue'
import TimeframeFilter from '@/components/decks/TimeframeFilter.vue'
import { Skeleton } from '@/components/ui/skeleton'
import { useArchetypeTab } from '@/composables/useArchetypeTab'
import { errorMessage } from '@/lib/commands'
import { useArchetypeDeckMatchups } from '@/lib/queries'

// The deck index's archetype Matchups tab (0.x `decks/archetypes/Matchups`):
// the spread of every deck of the archetype.
const { archetypeId, format, timeframe, settings, shell, error, setTimeframe } = useArchetypeTab()
const { data: matchups, error: matchupsError } = useArchetypeDeckMatchups(archetypeId, format, timeframe)
const search = ref('')
</script>

<template>
  <div class="flex min-h-0 flex-1 flex-col">
    <p v-if="error" class="p-4 text-sm text-destructive">Could not load the archetype: {{ errorMessage(error) }}</p>
    <DeckIndexLayout
      v-else-if="shell && settings"
      v-model:search="search"
      tab="matchups"
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

      <p v-if="matchupsError" class="text-sm text-destructive">
        Could not load the matchups: {{ errorMessage(matchupsError) }}
      </p>
      <MatchupSpread v-else-if="matchups" :matchups="matchups" />
      <div v-else class="space-y-2">
        <Skeleton v-for="n in 3" :key="n" class="h-8 w-full" />
      </div>
    </DeckIndexLayout>
    <div v-else class="space-y-2 p-4">
      <Skeleton v-for="n in 6" :key="n" class="h-8 w-full" />
    </div>
  </div>
</template>
