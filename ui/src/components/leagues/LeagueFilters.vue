<script setup lang="ts">
import { Search } from 'lucide-vue-next'
import type {
  LeagueArchetypeOption,
  LeagueDeckOption,
  LeagueFormatOption,
  LeagueListQuery,
  LeagueSort,
  LeagueStateChip,
} from '@/bindings'
import SegmentedControl from '@/components/SegmentedControl.vue'
import { Input } from '@/components/ui/input'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { useDebouncedSearch } from '@/composables/useDebouncedSearch'
import { LEAGUE_SORTS, STATE_CHIPS } from '@/lib/leagues'

/**
 * State chips, format, deck archetype, deck, a search over opponents and
 * their archetypes, and sort (0.x `LeagueFilters`). Every change goes back
 * to the first page.
 */
const props = withDefaults(
  defineProps<{
    query: LeagueListQuery
    formats: LeagueFormatOption[]
    decks: LeagueDeckOption[]
    /** The archetypes of the listed runs' decks (0.x `deckArchetypes`). */
    archetypes?: LeagueArchetypeOption[]
  }>(),
  { archetypes: () => [] },
)

const emit = defineEmits<{ change: [LeagueListQuery] }>()

const ALL_FORMATS = '__all_formats__'
const ALL_DECKS = '__all_decks__'
const ALL_ARCHETYPES = '__all_archetypes__'

function change(next: Partial<LeagueListQuery>) {
  emit('change', { ...props.query, ...next, page: 1 })
}

function setChip(state: LeagueStateChip) {
  change({ state })
}

function setFormat(value: unknown) {
  change({ format: value === ALL_FORMATS ? null : String(value) })
}

function setDeck(value: unknown) {
  change({ deckId: value === ALL_DECKS ? null : Number(value) })
}

function setArchetype(value: unknown) {
  change({ archetypeId: value === ALL_ARCHETYPES ? null : Number(value) })
}

function setSort(value: unknown) {
  change({ sort: value as LeagueSort })
}

// Search waits 250 ms after the last key (0.x), and follows the URL when it
// changes from outside (back and forward).
// Typing still waiting to be sent wins over the URL. A blank search is no
// search; any other is sent as typed.
const search = useDebouncedSearch({
  current: () => props.query.search ?? '',
  apply: (next) => change({ search: next === '' ? null : next }),
  delayMs: 250,
  normalise: (value) => (value.trim() === '' ? '' : value),
})
</script>

<template>
  <div class="flex flex-wrap items-center gap-2">
    <SegmentedControl
      :model-value="query.state"
      :options="STATE_CHIPS.map((chip) => ({ value: chip.key, label: chip.label }))"
      @update:model-value="(value) => setChip(value as LeagueStateChip)"
    />

    <Select :model-value="query.format ?? ALL_FORMATS" @update:model-value="setFormat">
      <SelectTrigger class="h-8 w-32">
        <SelectValue placeholder="Format" />
      </SelectTrigger>
      <SelectContent>
        <SelectItem :value="ALL_FORMATS">All formats</SelectItem>
        <SelectItem v-for="format in formats" :key="format.code" :value="format.code">{{ format.label }}</SelectItem>
      </SelectContent>
    </Select>

    <Select
      v-if="archetypes.length"
      :model-value="query.archetypeId == null ? ALL_ARCHETYPES : String(query.archetypeId)"
      @update:model-value="setArchetype"
    >
      <SelectTrigger class="h-8 w-40" aria-label="Archetype">
        <SelectValue placeholder="Archetype" />
      </SelectTrigger>
      <SelectContent>
        <SelectItem :value="ALL_ARCHETYPES">All archetypes</SelectItem>
        <SelectItem v-for="archetype in archetypes" :key="archetype.id" :value="String(archetype.id)">
          {{ archetype.name }}
        </SelectItem>
      </SelectContent>
    </Select>

    <Select :model-value="query.deckId === null ? ALL_DECKS : String(query.deckId)" @update:model-value="setDeck">
      <SelectTrigger class="h-8 w-40">
        <SelectValue placeholder="Deck" />
      </SelectTrigger>
      <SelectContent>
        <SelectItem :value="ALL_DECKS">All decks</SelectItem>
        <SelectItem v-for="deck in decks" :key="deck.id" :value="String(deck.id)">
          {{ deck.name }}
        </SelectItem>
      </SelectContent>
    </Select>

    <div class="ml-auto flex flex-wrap items-center gap-2">
      <div class="relative">
        <Search class="absolute top-1/2 left-2 size-3.5 -translate-y-1/2 text-muted-foreground" />
        <Input
          v-model="search"
          placeholder="Search opponent or archetype"
          aria-label="Search opponent or archetype" class="h-8 w-full pl-7 sm:w-64" />
      </div>
      <Select :model-value="query.sort" @update:model-value="setSort">
        <SelectTrigger class="h-8 w-32">
          <SelectValue />
        </SelectTrigger>
        <SelectContent>
          <SelectItem v-for="option in LEAGUE_SORTS" :key="option.value" :value="option.value">
            {{ option.label }}
          </SelectItem>
        </SelectContent>
      </Select>
    </div>
  </div>
</template>
