<script setup lang="ts">
import { X } from 'lucide-vue-next'
import { computed } from 'vue'
import type { ArchetypeCountView, BestOfView } from '@/bindings'
import { usePlatformContext } from '@/composables/usePlatformContext'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import {
  dateRange,
  bestOfOptions,
  isFiltered,
  kindOptions,
  RESULT_OPTIONS,
  type KindFilter,
  type MatchListState,
  type ResultFilter,
} from '@/lib/matchList'

/**
 * The match list's "Showing" line and filters (0.x `DeckMatches`): date from
 * and to (local days, both inclusive), result, type, the Arena kind (Bo1 or Bo3) and the opponent
 * archetype (0.x `filter_archetype`: "Unknown" for none, else one of the
 * scope's archetypes with how many matches faced it). Every change goes back
 * to the first page.
 */
const props = withDefaults(
  defineProps<{
    state: MatchListState
    /** "Showing 1-50 of 120 matches", empty when there are none. */
    summary: string
    /** The archetype filter's options; none hides the filter. */
    archetypes?: ArchetypeCountView[]
    /** Matches with no opponent archetype (0.x `unknownArchetypeCount`). */
    unknownCount?: number
    /** Hide the Bo1/Bo3 select (a deck's page has its own switch). */
    hideBestOf?: boolean
  }>(),
  { archetypes: () => [], unknownCount: 0, hideBestOf: false },
)

const { platform } = usePlatformContext()
const kinds = computed(() => kindOptions(platform.value))
const showBestOf = computed(() => platform.value !== 'mtgo' && !props.hideBestOf)
const bestOfs = bestOfOptions()

const emit = defineEmits<{ change: [MatchListState] }>()

function change(next: Partial<MatchListState>) {
  emit('change', { ...props.state, ...next, page: 1 })
}

function setDays(from: unknown, to: unknown) {
  change(dateRange(from, to))
}

function inputValue(event: Event): string {
  return (event.target as HTMLInputElement).value
}

function setResult(value: unknown) {
  change({ result: value === 'all' ? null : (value as ResultFilter) })
}

function setKind(value: unknown) {
  change({ kind: value === 'all' ? null : (value as KindFilter) })
}

function setBestOf(value: unknown) {
  change({ bestOf: value === 'all' ? null : (value as BestOfView) })
}

function setArchetype(value: unknown) {
  change({ archetype: value === 'all' ? null : String(value) })
}

function clearFilters() {
  change({ from: null, to: null, result: null, kind: null, bestOf: null, archetype: null })
}
</script>

<template>
  <div class="flex flex-wrap items-center justify-between gap-3">
    <p v-if="summary" class="text-xs text-muted-foreground">{{ summary }}</p>
    <div v-else />
    <div class="flex shrink-0 flex-wrap items-center gap-3">
      <!-- Actions before the filters (a deck's "Add manual match"). -->
      <slot />
      <div class="flex items-center gap-1.5">
        <Input
          type="date"
          aria-label="From"
          class="h-8 w-36 text-xs"
          :model-value="state.from ?? ''"
          :max="state.to ?? undefined"
          @change="setDays(inputValue($event), state.to)"
        />
        <span class="text-xs text-muted-foreground">to</span>
        <Input
          type="date"
          aria-label="To"
          class="h-8 w-36 text-xs"
          :model-value="state.to ?? ''"
          :min="state.from ?? undefined"
          @change="setDays(state.from, inputValue($event))"
        />
      </div>
      <Select :model-value="state.result ?? 'all'" @update:model-value="setResult">
        <SelectTrigger class="h-8 w-28 text-xs">
          <SelectValue placeholder="Result" />
        </SelectTrigger>
        <SelectContent>
          <SelectItem v-for="option in RESULT_OPTIONS" :key="option.value" :value="option.value" class="text-xs">
            {{ option.label }}
          </SelectItem>
        </SelectContent>
      </Select>
      <Select :model-value="state.kind ?? 'all'" @update:model-value="setKind">
        <SelectTrigger class="h-8 w-28 text-xs">
          <SelectValue placeholder="Type" />
        </SelectTrigger>
        <SelectContent>
          <SelectItem v-for="option in kinds" :key="option.value" :value="option.value" class="text-xs">
            {{ option.label }}
          </SelectItem>
        </SelectContent>
      </Select>
      <Select v-if="showBestOf" :model-value="state.bestOf ?? 'all'" @update:model-value="setBestOf">
        <SelectTrigger class="h-8 w-28 text-xs" aria-label="Best of">
          <SelectValue placeholder="Kind" />
        </SelectTrigger>
        <SelectContent>
          <SelectItem v-for="option in bestOfs" :key="option.value" :value="option.value" class="text-xs">
            {{ option.label }}
          </SelectItem>
        </SelectContent>
      </Select>
      <Select
        v-if="archetypes.length || unknownCount > 0"
        :model-value="state.archetype ?? 'all'"
        @update:model-value="setArchetype"
      >
        <SelectTrigger class="h-8 w-44 text-xs" aria-label="Archetype">
          <SelectValue placeholder="Archetype" />
        </SelectTrigger>
        <SelectContent>
          <SelectItem value="all" class="text-xs">All Archetypes</SelectItem>
          <SelectItem value="none" class="text-xs">Unknown ({{ unknownCount }})</SelectItem>
          <SelectItem
            v-for="archetype in archetypes"
            :key="archetype.id"
            :value="String(archetype.id)"
            class="text-xs"
          >
            {{ archetype.name }} ({{ archetype.matchCount }})
          </SelectItem>
        </SelectContent>
      </Select>
      <Button
        v-if="isFiltered(state)"
        variant="ghost"
        size="sm"
        class="h-8 gap-1.5 text-xs text-muted-foreground"
        @click="clearFilters"
      >
        <X class="size-3.5" />
        Clear
      </Button>
    </div>
  </div>
</template>
