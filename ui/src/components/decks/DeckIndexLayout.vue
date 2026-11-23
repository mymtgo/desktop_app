<script setup lang="ts">
import { useQueryClient } from '@tanstack/vue-query'
import { Search } from 'lucide-vue-next'
import { SplitterGroup, SplitterPanel, SplitterResizeHandle } from 'reka-ui'
import { computed } from 'vue'
import { useRouter } from 'vue-router'
import type { ArchetypeHeaderView, DeckArchetypeOptionView, FormatOptionView, Settings } from '@/bindings'
import DeckArchetypeHeader from '@/components/decks/DeckArchetypeHeader.vue'
import DeckIndexSidebar from '@/components/decks/DeckIndexSidebar.vue'
import { Input } from '@/components/ui/input'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { useSaveSettings } from '@/composables/useSaveSettings'
import { type DeckIndexTab, deckIndexTabRoute, toggleArchetype } from '@/lib/deckIndex'
import { ALL_FORMATS, deckFilters, formatSelectOptions } from '@/lib/decks'
import { keys } from '@/lib/keys'

/**
 * The deck index shell (0.x `DeckIndexLayout`): the sidebar, the top bar
 * (search and format, then the page's toolbar), the archetype header and
 * the page body, shared by the deck grid and the archetype tabs.
 * A sidebar choice changes the remembered filters (`deckFilters`); on a
 * stats tab, choosing another archetype stays on that tab, and anything
 * else (Unclassified, recently played, a format, a search) goes back to the
 * grid (0.x `applyFilters`).
 */
const props = withDefaults(
  defineProps<{
    settings: Settings
    tab: DeckIndexTab
    timeframe?: string
    search: string
    formatOptions: FormatOptionView[]
    archetypeOptions: DeckArchetypeOptionView[]
    unclassifiedCount: number
    archetypeHeader: ArchetypeHeaderView | null
    /** Whether "recently played" applies (the grid's answer). */
    recent: boolean
    recentUnavailable?: boolean
    selectedCount?: number
    selectionFormat?: string | null
    /** The format of the decks being dragged; null when no drag is on. */
    dragFormat?: string | null
  }>(),
  { timeframe: 'alltime', recentUnavailable: false, selectedCount: 0, selectionFormat: null, dragFormat: null },
)

const emit = defineEmits<{
  'update:search': [value: string]
  /** A filter changed: the grid starts again from page 1 and drops its selection. */
  filtered: []
  /** Decks dropped or chosen (none given: the selection) for an archetype, or null to clear. */
  assign: [deckIds: number[], archetypeId: number | null]
}>()

const router = useRouter()
const queryClient = useQueryClient()
const { save } = useSaveSettings()

function toGrid() {
  emit('filtered')
  if (props.tab !== 'decks') void router.push({ name: 'decks' })
}

const formatRows = computed(() => formatSelectOptions(props.formatOptions))

function setFormat(value: unknown) {
  const next = String(value ?? '')
  deckFilters.format = next === '' || next === ALL_FORMATS ? null : next
  // The archetype rows are per format: a chosen one would be stranded.
  deckFilters.archetype = null
  toGrid()
}

function setRecent(value: boolean) {
  deckFilters.recent = value
  if (value) deckFilters.archetype = null
  toGrid()
}

/** "All decks": every deck, neither recent nor one archetype. */
function showAll() {
  deckFilters.recent = false
  deckFilters.archetype = null
  toGrid()
}

function setArchetype(value: string) {
  const next = toggleArchetype(deckFilters.archetype, value)
  deckFilters.archetype = next
  if (props.tab !== 'decks' && next !== null && next !== 'none') {
    emit('filtered')
    void router.push(deckIndexTabRoute(props.tab, Number(next), props.timeframe))
    return
  }
  toGrid()
}

function setSearch(value: string) {
  emit('update:search', value)
  if (props.tab !== 'decks' && value.trim() !== '') {
    void router.push({ name: 'decks', query: { search: value } })
  }
}

async function saveSetting(next: Partial<Settings>) {
  if (await save({ ...props.settings, ...next })) {
    // The archetype tabs read the archived setting on the backend.
    void queryClient.invalidateQueries({ queryKey: keys.allDecks })
  }
}
</script>

<template>
  <!-- Sidebar width is user-draggable; reka keeps it under the autoSaveId. -->
  <SplitterGroup direction="horizontal" auto-save-id="decks-index-sidebar" class="flex min-h-0 flex-1">
    <SplitterPanel :default-size="18" :min-size="12" :max-size="40" class="shrink-0">
      <DeckIndexSidebar
        :format="deckFilters.format ?? ''"
        :recent="recent"
        :recent-unavailable="recentUnavailable"
        :compact-cards="settings.deckCardSize === 'compact'"
        :show-archived="!settings.hideArchivedDecks"
        :archetype="deckFilters.archetype"
        :archetype-options="archetypeOptions"
        :unclassified-count="unclassifiedCount"
        :selected-count="selectedCount"
        :selection-format="selectionFormat"
        :drag-format="dragFormat"
        @update:recent="setRecent"
        @update:archetype="setArchetype"
        @show-all="showAll"
        @update:compact-cards="(value) => saveSetting({ deckCardSize: value ? 'compact' : 'large' })"
        @update:show-archived="(value) => saveSetting({ hideArchivedDecks: !value })"
        @assign="(ids, id) => emit('assign', ids, id)"
      />
    </SplitterPanel>

    <SplitterResizeHandle
      class="-mr-1 w-1 cursor-col-resize bg-transparent transition-colors hover:bg-primary/40 data-[state=drag]:bg-primary/60"
    />

    <SplitterPanel class="flex min-h-0 flex-col border-l border-white/5">
      <header class="flex items-center gap-2 border-b border-black/60 bg-background/40 px-4 py-2">
        <div class="relative w-56 shrink-0">
          <Search class="pointer-events-none absolute top-1/2 left-2 size-3.5 -translate-y-1/2 text-muted-foreground" />
          <Input
            :model-value="search"
            placeholder="Search decks..."
            class="h-8 pl-7 text-xs"
            @update:model-value="(value) => setSearch(String(value))"
          />
        </div>
        <Select :model-value="deckFilters.format ?? ALL_FORMATS" @update:model-value="setFormat">
          <SelectTrigger size="sm" class="w-40 text-xs">
            <SelectValue placeholder="All formats" />
          </SelectTrigger>
          <SelectContent>
            <SelectItem v-for="row in formatRows" :key="row.value" :value="row.value">
              {{ row.label }}
              <template #trailing>
                <span class="ml-auto text-xs text-muted-foreground tabular-nums">{{ row.count }}</span>
              </template>
            </SelectItem>
          </SelectContent>
        </Select>
        <slot name="toolbar" />
      </header>

      <!-- Outside the scroll container so the body scrolls under it. -->
      <DeckArchetypeHeader
        v-if="archetypeHeader"
        :header="archetypeHeader"
        :tab="tab"
        :timeframe="timeframe"
      />

      <!-- Children keep their height (shrink-0) so they overflow and scroll here:
           a Card with overflow-hidden would otherwise shrink to fit and clip its rows. -->
      <div class="flex min-h-0 flex-1 flex-col gap-4 overflow-y-auto border-t border-white/5 p-3 *:shrink-0 lg:p-4">
        <slot />
      </div>

      <slot name="footer" />
    </SplitterPanel>
  </SplitterGroup>
</template>
