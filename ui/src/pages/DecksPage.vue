<script setup lang="ts">
import { refDebounced } from '@vueuse/core'
import { ArrowUpDown, Layers, RefreshCw, Rows3 } from 'lucide-vue-next'
import { computed, ref, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { commands, type DeckListQuery, type DeckSortKey, type DeckView } from '@/bindings'
import DeckCardGrid from '@/components/decks/DeckCardGrid.vue'
import DeckIndexLayout from '@/components/decks/DeckIndexLayout.vue'
import DeckSelectionBar from '@/components/decks/DeckSelectionBar.vue'
import SetDeckArchetypeDialog from '@/components/decks/SetDeckArchetypeDialog.vue'
import { Button } from '@/components/ui/button'
import PagedNav from '@/components/PagedNav.vue'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Skeleton } from '@/components/ui/skeleton'
import { SEARCH_DEBOUNCE_MS } from '@/composables/useDebouncedSearch'
import { useDeckEdit } from '@/composables/useDeckEdit'
import { useSaveSettings } from '@/composables/useSaveSettings'
import { useToast } from '@/composables/useToast'
import { errorMessage, unwrap } from '@/lib/commands'
import { assignableDeckIds, canSetArchetypeByHand, selectionFormat } from '@/lib/deckIndex'
import { deckFilters, PER_PAGE_OPTIONS, recentRowState, SORT_OPTIONS, totalDecks } from '@/lib/decks'
import { useDeckList, useSettings } from '@/lib/queries'

const route = useRoute()
const router = useRouter()
const { data: settings } = useSettings()
const { save } = useSaveSettings()
const { add } = useToast()

// Sort and page live in the URL (back and forward keep them); format and
// recently played are remembered for the session; search is neither.
const page = computed(() => Math.max(1, Number(route.query.page ?? 1) || 1))
const sort = computed<DeckSortKey>(() => {
  const value = route.query.sort
  return SORT_OPTIONS.some((option) => option.value === value) ? (value as DeckSortKey) : 'last_played'
})

function urlQuery(next: { sort: DeckSortKey; page: number }) {
  const query: Record<string, string> = {}
  if (next.sort !== 'last_played') query.sort = next.sort
  if (next.page > 1) query.page = String(next.page)
  return { query }
}

function goToPage(next: number) {
  void router.push(urlQuery({ sort: sort.value, page: next }))
}

function setSort(value: unknown) {
  void router.push(urlQuery({ sort: value as DeckSortKey, page: 1 }))
}

/** A filter changed: start again from the first page. */
function firstPage() {
  if (page.value !== 1) void router.replace(urlQuery({ sort: sort.value, page: 1 }))
}

// A search typed on an archetype tab arrives as `?search=` (not remembered).
const searchInput = ref(typeof route.query.search === 'string' ? route.query.search : '')
const search = refDebounced(searchInput, SEARCH_DEBOUNCE_MS)
watch(search, firstPage)

const listQuery = computed<DeckListQuery | null>(() => {
  if (!settings.value) return null
  return {
    format: deckFilters.format,
    recent: deckFilters.recent,
    archetype: deckFilters.archetype,
    search: search.value.trim() === '' ? null : search.value,
    hideArchived: settings.value.hideArchivedDecks,
    sort: sort.value,
    page: page.value,
    perPage: settings.value.decksPerPage,
  }
})

const { data: list, error, isPending, isPlaceholderData, dataUpdatedAt } = useDeckList(listQuery)

// The list shows the last page when asked for one past the end (a filter or
// page size shrank it): follow it rather than keep a page number that is not shown.
watch(list, (shown) => {
  if (!shown || isPlaceholderData.value || shown.page === page.value) return
  void router.replace(urlQuery({ sort: sort.value, page: shown.page }))
})

// A remembered archetype with no deck left in scope is dropped (0.x).
watch(list, (shown) => {
  if (!shown || isPlaceholderData.value) return
  if (deckFilters.archetype !== null && shown.archetype === null) deckFilters.archetype = null
})

/** Whether "recent decks" applied: with no deck activity yet, every deck shows. */
const recentShown = computed(() => list.value?.recent ?? (deckFilters.recent && deckFilters.archetype === null))
/** The row's look; a placeholder list is the previous query's, so it says nothing yet. */
const recentRow = computed(() =>
  recentRowState(
    deckFilters.recent,
    list.value && !isPlaceholderData.value ? list.value.recent : null,
    deckFilters.archetype,
  ),
)
const anyFilter = computed(
  () => !!search.value || deckFilters.format !== null || recentShown.value || deckFilters.archetype !== null,
)
const showEmptyStateEmpty = computed(() => !!list.value && totalDecks(list.value.formatOptions) === 0 && !anyFilter.value)
const showEmptyStateFiltered = computed(() => !!list.value && list.value.total === 0 && anyFilter.value)

function clearFilters() {
  deckFilters.format = null
  deckFilters.recent = false
  deckFilters.archetype = null
  searchInput.value = ''
  firstPage()
}

/** A sidebar filter changed: back to page 1, nothing selected. */
function onFiltered() {
  selectedIds.value = []
  firstPage()
}

// Selection for the bulk archetype assign (0.x `selectedIds`): replaced
// whole on every change, dropped when the filters, sort or page change.
const selectedIds = ref<number[]>([])
const shownFormat = computed(() => selectionFormat(selectedIds.value, list.value?.decks ?? []))
watch([page, sort], () => {
  selectedIds.value = []
})

function setSelected(deckId: number, value: boolean) {
  const next = new Set(selectedIds.value)
  if (value) next.add(deckId)
  else next.delete(deckId)
  selectedIds.value = [...next]
}

const { saving: assigning, run } = useDeckEdit()

/**
 * Assign (or with null clear) the archetype of decks dropped on a sidebar
 * row, or of the selection when none are given (0.x `assignArchetype`).
 */
async function assign(deckIds: number[], archetypeId: number | null) {
  // Detection would revert a Brawl deck's archetype: no path sets it by hand.
  const ids = assignableDeckIds(deckIds.length > 0 ? deckIds : selectedIds.value, list.value?.decks ?? [])
  if (ids.length === 0 || assigning.value) return
  if (await run('Could not set the archetype', () => commands.setDeckArchetype(ids, archetypeId))) {
    selectedIds.value = []
  }
}

// One deck's archetype from its card's menu; the bulk selection is left alone.
const archetypeDeck = ref<DeckView | null>(null)

async function setOneArchetype(deck: DeckView, archetypeId: number | null) {
  if (assigning.value || !canSetArchetypeByHand(deck)) return
  if (await run('Could not set the archetype', () => commands.setDeckArchetype([deck.id], archetypeId))) {
    archetypeDeck.value = null
  }
}

/** The format of the decks being dragged onto the sidebar. */
const dragFormat = ref<string | null>(null)

// Page size is a saved setting, not a filter: the list is reached from
// every deck page, and a preference that resets on the next visit is worse
// than no preference.
function setPerPage(value: unknown) {
  if (settings.value) void save({ ...settings.value, decksPerPage: Number(value) })
}

const syncing = ref(false)

async function syncDecks() {
  if (syncing.value) return
  syncing.value = true
  try {
    unwrap(await commands.syncDecks())
    add({ type: 'success', title: 'Decks synced', message: '' })
  } catch (failure) {
    add({ type: 'error', title: 'Could not sync decks', message: errorMessage(failure) })
  } finally {
    syncing.value = false
  }
}
</script>

<template>
  <div class="flex min-h-0 flex-1 flex-col">
    <p v-if="error" class="p-4 text-sm text-destructive">Could not load decks: {{ error.message }}</p>

    <div v-else-if="isPending || !settings" class="grid grid-cols-1 gap-3 p-4 sm:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4">
      <Skeleton v-for="n in 8" :key="n" class="h-72 w-full" />
    </div>

    <div v-else-if="showEmptyStateEmpty" class="flex flex-1 flex-col items-center justify-center gap-2 py-16 text-center">
      <Layers class="size-10 text-muted-foreground/40" />
      <p class="font-medium">No decks yet</p>
      <p class="text-sm text-muted-foreground">Decks are synced automatically from MTGO once the file watcher is running.</p>
    </div>

    <DeckIndexLayout
      v-else-if="list"
      v-model:search="searchInput"
      tab="decks"
      :settings="settings"
      :format-options="list.formatOptions"
      :archetype-options="list.archetypeOptions"
      :unclassified-count="list.unclassifiedCount"
      :archetype-header="list.archetypeHeader"
      :recent="recentShown"
      :recent-unavailable="recentRow === 'unavailable'"
      :selected-count="selectedIds.length"
      :selection-format="shownFormat"
      @filtered="onFiltered"
      :drag-format="dragFormat"
      @assign="assign"
    >
      <template #toolbar>
        <Select :model-value="sort" @update:model-value="setSort">
          <SelectTrigger size="sm" class="w-36 gap-1.5 text-xs">
            <ArrowUpDown class="size-3.5 text-muted-foreground" />
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem v-for="option in SORT_OPTIONS" :key="option.value" :value="option.value" class="text-xs">
              {{ option.label }}
            </SelectItem>
          </SelectContent>
        </Select>

        <div class="ml-auto flex items-center gap-2">
          <Select :model-value="String(settings.decksPerPage)" @update:model-value="setPerPage">
            <SelectTrigger size="sm" class="w-fit gap-1.5 text-xs">
              <Rows3 class="size-3.5 text-muted-foreground" />
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem v-for="size in PER_PAGE_OPTIONS" :key="size" :value="String(size)" class="text-xs">
                {{ size }} per page
              </SelectItem>
            </SelectContent>
          </Select>
          <PagedNav
            v-if="list.total > list.perPage"
            class="mx-0 w-auto"
            :per-page="list.perPage"
            :total="list.total"
            :page="list.page"
            @update:page="goToPage"
          />

          <Button variant="outline" size="sm" class="gap-1.5 text-xs" :disabled="syncing" @click="syncDecks">
            <RefreshCw :class="['size-3.5', syncing && 'animate-spin']" />
            {{ syncing ? 'Syncing…' : 'Sync decks' }}
          </Button>
        </div>
      </template>

        <div v-if="showEmptyStateFiltered" class="flex flex-col items-center gap-2 py-12 text-center">
          <p class="text-sm text-muted-foreground">No decks match your filters.</p>
          <Button variant="outline" size="sm" @click="clearFilters">Clear filters</Button>
        </div>

        <template v-else>
          <DeckCardGrid
            :decks="list.decks"
            :card-size="settings.deckCardSize"
            :now="dataUpdatedAt"
            selectable
            :selected-ids="selectedIds"
            :selection-format="shownFormat"
            @update:selected="setSelected"
            @dragstart="(format: string) => (dragFormat = format)"
            @dragend="dragFormat = null"
            @set-archetype="(deck: DeckView) => (archetypeDeck = deck)"
            @clear-archetype="(deck: DeckView) => setOneArchetype(deck, null)"
          />

          <PagedNav
            v-if="list.total > list.perPage"
            class="justify-end"
            :per-page="list.perPage"
            :total="list.total"
            :page="list.page"
            @update:page="goToPage"
          />
        </template>

      <template #footer>
        <DeckSelectionBar
          :count="selectedIds.length"
          :busy="assigning"
          :format="shownFormat"
          @assign="(id) => assign([], id)"
          @dismiss="selectedIds = []"
        />
      </template>
    </DeckIndexLayout>

    <SetDeckArchetypeDialog
      :deck="archetypeDeck"
      :busy="assigning"
      @select="(id: number) => archetypeDeck && setOneArchetype(archetypeDeck, id)"
      @close="archetypeDeck = null"
    />
  </div>
</template>
