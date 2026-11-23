<script setup lang="ts">
import { TriangleAlert } from 'lucide-vue-next'
import { computed } from 'vue'
import { useRouter } from 'vue-router'
import type { ArchetypeHeaderView } from '@/bindings'
import { usePlatformContext } from '@/composables/usePlatformContext'
import ManaColors from '@/components/mana/ManaColors.vue'
import MatchRecord from '@/components/decks/MatchRecord.vue'
import SegmentedControl from '@/components/SegmentedControl.vue'
import { DECK_INDEX_TABS, type DeckIndexTab, deckCount, deckIndexTabRoute, rateTone } from '@/lib/deckIndex'

/**
 * The band between the toolbar and the grid while one archetype (or
 * Unclassified) is chosen (0.x `DeckArchetypeHeader`): its record and deck
 * count (the record follows the tab's timeframe, all time on the Decks
 * tab), the best and worst matchups, and the tabs
 * Decks, Matches, Matchups, Trends and Card Stats. A fallback has no tabs.
 */
const props = withDefaults(
  defineProps<{
    header: ArchetypeHeaderView
    tab?: DeckIndexTab
    timeframe?: string
  }>(),
  { tab: 'decks', timeframe: 'alltime' },
)

const router = useRouter()
const { platform } = usePlatformContext()

const showTabs = computed(() => props.header.archetype !== null && !props.header.archetype.isFallback)
const tabOptions = DECK_INDEX_TABS.map((item) => ({ value: item.key, label: item.label }))

function openTab(value: string) {
  const archetype = props.header.archetype
  if (archetype === null || value === props.tab) return
  void router.push(deckIndexTabRoute(value as DeckIndexTab, archetype.id, props.timeframe))
}
</script>

<template>
  <section class="flex shrink-0 flex-col gap-2 border-t border-b border-white/5 border-b-black/60 bg-background/40 px-4 py-3">
    <div class="flex items-center gap-3">
      <template v-if="header.archetype">
        <ManaColors v-if="header.archetype.colors.length" :colors="header.archetype.colors" class="shrink-0" />
        <h2 class="truncate text-lg font-semibold">{{ header.archetype.name }}</h2>
      </template>
      <template v-else>
        <TriangleAlert class="size-4 shrink-0 text-warning" />
        <h2 class="text-lg font-semibold">Unclassified</h2>
        <span class="ml-auto text-xs text-muted-foreground">Select decks and assign them an archetype to classify them</span>
      </template>
    </div>

    <div class="flex flex-wrap items-baseline gap-x-2 text-sm tabular-nums">
      <span :class="['text-2xl leading-none font-bold', rateTone(header.record)]">
        {{ header.record.total === 0 ? '-' : `${header.record.winRate}%` }}
      </span>
      <span class="text-muted-foreground">{{ platform !== 'mtgo' ? 'All win rate' : 'win rate' }}</span>
      <span class="text-muted-foreground">·</span>
      <MatchRecord :record="header.record" />
      <span class="text-muted-foreground">·</span>
      <span class="text-muted-foreground">{{ header.record.total }} matches</span>
      <span class="text-muted-foreground">·</span>
      <span class="text-muted-foreground">{{ deckCount(header.deckCount) }}</span>
    </div>

    <div v-if="header.archetype" class="flex flex-wrap gap-x-6 gap-y-1 text-xs">
      <template v-if="header.best && header.worst">
        <span class="flex items-center gap-1.5">
          <span class="text-muted-foreground">Best matchup</span>
          <ManaColors v-if="header.best.colors.length" :colors="header.best.colors" />
          <span class="font-medium">{{ header.best.name }}</span>
          <span :class="['tabular-nums', header.best.winRate >= 50 ? 'text-success' : 'text-destructive']">{{ header.best.winRate }}%</span>
          <span class="text-muted-foreground tabular-nums">({{ header.best.matches }} matches)</span>
        </span>
        <span class="flex items-center gap-1.5">
          <span class="text-muted-foreground">Worst matchup</span>
          <ManaColors v-if="header.worst.colors.length" :colors="header.worst.colors" />
          <span class="font-medium">{{ header.worst.name }}</span>
          <span :class="['tabular-nums', header.worst.winRate >= 50 ? 'text-success' : 'text-destructive']">
            {{ header.worst.winRate }}%
          </span>
          <span class="text-muted-foreground tabular-nums">({{ header.worst.matches }} matches)</span>
        </span>
      </template>
      <span v-else class="text-muted-foreground">Not enough matchup data</span>
    </div>

    <SegmentedControl v-if="showTabs" class="mt-1" :model-value="tab" :options="tabOptions" @update:model-value="openTab" />
  </section>
</template>
