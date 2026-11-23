<script setup lang="ts">
import { ref } from 'vue'
import { useRouter } from 'vue-router'
import ArchetypeTrendChart from '@/components/decks/ArchetypeTrendChart.vue'
import DeckIndexLayout from '@/components/decks/DeckIndexLayout.vue'
import TimeframeFilter from '@/components/decks/TimeframeFilter.vue'
import { Skeleton } from '@/components/ui/skeleton'
import { useArchetypeTab } from '@/composables/useArchetypeTab'
import { errorMessage } from '@/lib/commands'
import { useArchetypeDeckTrends } from '@/lib/queries'

// The deck index's archetype Trends tab: the archetype's sessions and trend,
// or a trend line per deck.
const router = useRouter()
const { archetypeId, format, timeframe, settings, shell, error, setTimeframe } = useArchetypeTab()
const { data: trends, error: trendsError } = useArchetypeDeckTrends(archetypeId, format, timeframe)
const search = ref('')
</script>

<template>
  <div class="flex min-h-0 flex-1 flex-col">
    <p v-if="error" class="p-4 text-sm text-destructive">Could not load the archetype: {{ errorMessage(error) }}</p>
    <DeckIndexLayout
      v-else-if="shell && settings"
      v-model:search="search"
      tab="trends"
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

      <p v-if="trendsError" class="text-sm text-destructive">
        Could not load the trends: {{ errorMessage(trendsError) }}
      </p>
      <ArchetypeTrendChart
        v-else-if="trends"
        :overall="trends.overall"
        :decks="trends.decks"
        @open-deck="(id) => router.push(`/decks/${id}`)"
      />
      <Skeleton v-else class="h-[400px] w-full" />
    </DeckIndexLayout>
    <div v-else class="space-y-2 p-4">
      <Skeleton v-for="n in 6" :key="n" class="h-8 w-full" />
    </div>
  </div>
</template>
