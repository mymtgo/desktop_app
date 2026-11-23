<script setup lang="ts">
// The archetype page's side menu (issue #326; 0.x `ArchetypeSidebar.vue`):
// search and format over the held archetypes, 25 a page, each linking to
// that archetype on the open tab and timeframe. The filters are the manage
// screen's remembered ones, as 0.x's sidebar shared the index's.
import { ChevronLeft, ChevronRight, Plus, Search } from 'lucide-vue-next'
import { refDebounced } from '@vueuse/core'
import { computed, ref, watch } from 'vue'
import ManaColors from '@/components/mana/ManaColors.vue'
import { SEARCH_DEBOUNCE_MS } from '@/composables/useDebouncedSearch'
import { Input } from '@/components/ui/input'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { createArchetypeRoute, readRememberedFilters, useManageArchetypes, writeRememberedFilters } from '@/lib/archetypes'
import { ALL_FORMATS } from '@/lib/decks'
import { archetypeLocation, sidebarFilters, type ArchetypeTab } from '@/lib/metagame'

const props = defineProps<{
  selectedId: number
  /** The open archetype's format, the default filter. */
  archetypeFormat: string | null
  tab: ArchetypeTab
  timeframe: string
  /** The Metagame page's search, kept for the way back. */
  metagameSearch?: string
}>()

const remembered = readRememberedFilters(window.localStorage)
const start = sidebarFilters(remembered, props.archetypeFormat)
const search = ref(start.search)
const settledSearch = refDebounced(search, SEARCH_DEBOUNCE_MS)
const searched = computed(() => settledSearch.value.trim())
const format = ref(start.format)
function onFormatChange(value: unknown) {
  const next = String(value ?? '')
  format.value = next === ALL_FORMATS ? '' : next
}
const page = ref(1)

// With nothing remembered the menu opens on the archetype's format, which is
// known once its detail loads. Applied once, and not remembered: only the
// user's own choices are.
let applyingDefault = false
if (remembered === null && props.archetypeFormat === null) {
  watch(
    () => props.archetypeFormat,
    (archetypeFormat) => {
      if (archetypeFormat === null || format.value !== '' || searched.value !== '') return
      applyingDefault = true
      format.value = archetypeFormat
    },
    { once: true },
  )
}

watch([searched, format], ([nextSearch, nextFormat]) => {
  page.value = 1
  if (applyingDefault) {
    applyingDefault = false
    return
  }
  // The sidebar lists every source; the manage page's source filter stays remembered.
  const source = readRememberedFilters(window.localStorage)?.source ?? ''
  writeRememberedFilters(window.localStorage, { search: nextSearch, format: nextFormat, source })
})

const query = computed(() => ({
  search: searched.value || null,
  format: format.value || null,
  source: null,
  page: page.value,
}))
const { data } = useManageArchetypes(query)
// 0.x's sidebar left the system fallback (Homebrew or Rogue) out.
const rows = computed(() => (data.value?.rows ?? []).filter((row) => row.source !== 'fallback'))
const lastPage = computed(() => (data.value ? Math.max(1, Math.ceil(data.value.total / data.value.perPage)) : 1))
</script>

<template>
  <aside class="flex h-full w-full flex-col border-r border-black/60 bg-muted/20">
    <div class="flex gap-2 border-b border-black/60 p-3">
      <div class="relative basis-2/3">
        <Search class="pointer-events-none absolute top-1/2 left-2 size-3.5 -translate-y-1/2 text-muted-foreground" />
        <Input v-model="search" placeholder="Search..." aria-label="Search archetypes" class="h-8 pl-7 text-xs" />
      </div>
      <!-- The deck sidebar's format select; Select has no empty value, so All is ALL_FORMATS. -->
      <Select :model-value="format === '' ? ALL_FORMATS : format" @update:model-value="onFormatChange">
        <SelectTrigger class="h-8 basis-1/3 text-xs" aria-label="Format">
          <SelectValue />
        </SelectTrigger>
        <SelectContent>
          <SelectItem :value="ALL_FORMATS">All</SelectItem>
          <SelectItem v-for="option in data?.formats ?? []" :key="option.key" :value="option.key">{{ option.label }}</SelectItem>
        </SelectContent>
      </Select>
    </div>

    <div class="flex items-center border-b border-black/40 px-3 py-2">
      <RouterLink
        :to="createArchetypeRoute()"
        class="flex items-center gap-2 rounded-md px-2 py-1.5 text-sm font-medium text-purple-400 transition-colors hover:bg-accent/50"
      >
        <Plus class="size-4" />
        Create New
      </RouterLink>
    </div>

    <nav class="flex-1 overflow-y-auto" aria-label="Archetypes">
      <p v-if="data && rows.length === 0" class="px-3 py-6 text-center text-sm text-muted-foreground">
        No archetypes found
      </p>
      <RouterLink
        v-for="row in rows"
        :key="row.id"
        :to="archetypeLocation(row.id, tab, timeframe, null, metagameSearch ?? '')"
        class="flex items-center gap-2 border-b border-black/40 px-3 py-2.5 text-sm transition-colors hover:bg-accent/50"
        :class="{
          'border-l-2 border-l-purple-500 bg-accent/30': selectedId === row.id,
          'opacity-60': row.mergedInto || row.retired,
        }"
        :aria-current="selectedId === row.id ? 'page' : undefined"
      >
        <div class="min-w-0 flex-1">
          <div class="flex items-center gap-1.5">
            <span class="truncate font-medium text-foreground">{{ row.name }}</span>
            <span
              v-if="row.mergedInto"
              class="shrink-0 rounded bg-muted px-1.5 py-0.5 text-[10px] tracking-wide text-muted-foreground uppercase"
              title="Merged into another archetype"
            >
              Merged
            </span>
          </div>
          <div class="flex items-center gap-1.5 text-xs text-muted-foreground">
            <span v-if="row.formatLabel">{{ row.formatLabel }}</span>
            <span v-if="row.formatLabel && row.colors.length">&middot;</span>
            <ManaColors v-if="row.colors.length" :colors="row.colors" size="xs" />
          </div>
        </div>
        <span
          v-if="row.matchesFaced > 0"
          class="shrink-0 text-xs text-muted-foreground tabular-nums"
          :title="`${row.matchesFaced} ${row.matchesFaced === 1 ? 'match' : 'matches'} faced`"
          >{{ row.matchesFaced }}</span
        >
        <span
          v-if="row.variants > 0 && !row.mergedInto"
          class="size-2 shrink-0 rounded-full bg-green-500"
          title="Decklist downloaded"
        />
      </RouterLink>
    </nav>

    <div v-if="lastPage > 1" class="flex items-center justify-between border-t border-black/60 px-3 py-2">
      <button
        type="button"
        :disabled="page <= 1"
        class="rounded p-1 hover:bg-accent/50 disabled:opacity-30"
        aria-label="Previous page"
        @click="page -= 1"
      >
        <ChevronLeft class="size-4" />
      </button>
      <span class="text-xs text-muted-foreground">{{ page }} / {{ lastPage }}</span>
      <button
        type="button"
        :disabled="page >= lastPage"
        class="rounded p-1 hover:bg-accent/50 disabled:opacity-30"
        aria-label="Next page"
        @click="page += 1"
      >
        <ChevronRight class="size-4" />
      </button>
    </div>
  </aside>
</template>
