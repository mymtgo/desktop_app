<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import CurveAndColours from '@/components/decks/CurveAndColours.vue'
import CardImageGroups from '@/components/limited/CardImageGroups.vue'
import DeckChanges from '@/components/limited/DeckChanges.vue'
import VersionStrip from '@/components/limited/VersionStrip.vue'
import SegmentedControl from '@/components/SegmentedControl.vue'
import { Skeleton } from '@/components/ui/skeleton'
import { errorMessage } from '@/lib/commands'
import { curveAndColours, deckSummaryText, mainGroups, poolGroups, shownVersion, wholePool } from '@/lib/limitedPages'
import { useLimitedDeck, useLimitedEvent } from '@/lib/queries'
import { useLimitedKey } from '@/pages/limited/useLimitedKey'

/**
 * The event's deck (0.x `pages/limited/Deck.vue`): the summary line, the
 * registered builds, the main deck or the rest of the pool as card images,
 * the curve and colours of both, and what changed from the build before.
 */
const key = useLimitedKey()
const { data: event } = useLimitedEvent(key)
const { data: evolution, error } = useLimitedDeck(key)

const current = computed(() => evolution.value?.versions.find((version) => version.isCurrent) ?? null)

/** The build whose cards are shown; the current one until another is picked. */
const selectedIndex = ref<number | null>(null)
watch(current, (version) => (selectedIndex.value = version?.index ?? null), { immediate: true })

const selected = computed(() => shownVersion(evolution.value?.versions ?? [], selectedIndex.value))

/** Sealed has no draft: the pool is the boosters opened, plus the one added mid-run. */
const isSealed = computed(() => event.value?.kind === 'sealed')

type Tab = 'main' | 'pool'
const tab = ref<Tab>('main')
const tabs = [
  { value: 'main', label: 'Maindeck' },
  { value: 'pool', label: 'Card pool' },
]

const cards = computed(() => evolution.value?.cards ?? {})
const main = computed(() => mainGroups(selected.value, cards.value))
const pool = computed(() => poolGroups(selected.value, evolution.value?.pool ?? { groups: [] }, cards.value))
const mainStats = computed(() => curveAndColours(main.value.flatMap((group) => group.cards)))
const poolStats = computed(() => curveAndColours(wholePool(evolution.value?.pool ?? { groups: [] }, cards.value)))
const summary = computed(() => (evolution.value ? deckSummaryText(evolution.value.summary, isSealed.value) : ''))
</script>

<template>
  <div class="flex flex-col gap-4 p-3 lg:p-4">
    <div class="flex flex-wrap items-start justify-between gap-4">
      <div class="flex flex-col gap-1">
        <h1 class="text-base font-semibold tracking-tight">Deck</h1>
        <p v-if="evolution" class="text-xs text-muted-foreground">{{ summary }}</p>
        <Skeleton v-else-if="!error" class="h-4 w-96" />
      </div>
    </div>

    <p v-if="error" class="text-sm text-destructive">Could not load the deck: {{ errorMessage(error) }}</p>
    <template v-else-if="evolution === undefined">
      <Skeleton class="h-10 w-full" />
      <div class="grid gap-4 lg:grid-cols-[1fr_20rem]">
        <Skeleton class="h-96 w-full" />
        <Skeleton class="h-64 w-full" />
      </div>
    </template>
    <template v-else-if="evolution">
      <VersionStrip
        :versions="evolution.versions"
        :selected="selected?.index ?? null"
        @select="(index) => (selectedIndex = index)"
      />
      <div class="grid gap-4 lg:grid-cols-[1fr_20rem]">
        <div class="flex min-w-0 flex-col gap-4">
          <SegmentedControl
            :model-value="tab"
            :options="tabs"
            class="self-start"
            @update:model-value="(value) => (tab = value as Tab)"
          />
          <CardImageGroups v-if="tab === 'main'" :groups="main" empty-text="No registered deck yet." />
          <CardImageGroups v-else :groups="pool" empty-text="Every card in the pool is in the main deck." />
        </div>
        <div class="flex flex-col gap-6 self-start">
          <CurveAndColours
            :curve="mainStats.curve"
            :colours="mainStats.colours"
            :total="mainStats.total"
            scope="maindeck, nonland"
          />
          <CurveAndColours
            :curve="poolStats.curve"
            :colours="poolStats.colours"
            :total="poolStats.total"
            scope="card pool, nonland"
          />
          <DeckChanges :versions="evolution.versions" :cards="evolution.cards" :selected="selected?.index ?? null" />
        </div>
      </div>
    </template>
  </div>
</template>
