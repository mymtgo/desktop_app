<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import CardStatsView from '@/components/cards/CardStatsView.vue'
import TrustSliderPopover from '@/components/cards/TrustSliderPopover.vue'
import DeckIndexLayout from '@/components/decks/DeckIndexLayout.vue'
import TimeframeFilter from '@/components/decks/TimeframeFilter.vue'
import { Card, CardContent } from '@/components/ui/card'
import { Skeleton } from '@/components/ui/skeleton'
import { useArchetypeTab } from '@/composables/useArchetypeTab'
import { useTrustSetting } from '@/composables/useTrustSetting'
import { DEFAULT_FILTERS, filtersToQuery, type CardStatsFilters } from '@/lib/cardStats/filters'
import { trustMax } from '@/lib/cardStats/format'
import { errorMessage } from '@/lib/commands'
import { useArchetypeDeckCardStats } from '@/lib/queries'

// The deck index's archetype Card stats tab (0.x `decks/archetypes/
// CardStats`): Plan S's card stats over every deck of the archetype.
const { archetypeId, format, timeframe, settings, shell, error, setTimeframe } = useArchetypeTab()

const filters = ref<CardStatsFilters>({ ...DEFAULT_FILTERS })
watch(archetypeId, () => {
  filters.value = { ...DEFAULT_FILTERS }
})
const query = computed(() => filtersToQuery(filters.value, timeframe.value))
const { data: payload, error: statsError } = useArchetypeDeckCardStats(archetypeId, format, query)

const trust = useTrustSetting()
watch(
  () => payload.value?.trust,
  (stored) => {
    if (stored !== undefined) trust.seed(stored)
  },
  { immediate: true },
)
const max = computed(() => trustMax(payload.value?.deckWinrate.games ?? 0))
const search = ref('')
</script>

<template>
  <div class="flex min-h-0 flex-1 flex-col">
    <p v-if="error" class="p-4 text-sm text-destructive">Could not load the archetype: {{ errorMessage(error) }}</p>
    <DeckIndexLayout
      v-else-if="shell && settings"
      v-model:search="search"
      tab="card-stats"
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
        <div class="ml-auto">
          <TrustSliderPopover :model-value="trust.value.value" :max="max" @update:model-value="trust.setValue" @reset="trust.reset" />
        </div>
      </template>

      <p v-if="statsError" class="text-sm text-destructive">Could not load card stats: {{ errorMessage(statsError) }}</p>
      <CardStatsView v-else-if="payload" v-model:filters="filters" :payload="payload" :trust="trust.value.value" />
      <Card v-else class="gap-0 overflow-hidden p-0">
        <CardContent class="flex flex-col gap-2 px-4 py-4">
          <Skeleton class="h-8 w-full" />
          <Skeleton class="h-8 w-full" />
          <Skeleton class="h-8 w-3/4" />
        </CardContent>
      </Card>
    </DeckIndexLayout>
    <div v-else class="space-y-2 p-4">
      <Skeleton v-for="n in 6" :key="n" class="h-8 w-full" />
    </div>
  </div>
</template>
