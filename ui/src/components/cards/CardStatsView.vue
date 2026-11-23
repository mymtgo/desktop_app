<script setup lang="ts">
import {
  ArrowDownUp,
  BarChart3,
  Camera,
  Check,
  ChevronDown,
  ChevronsUpDown,
  ChevronUp,
  Columns3,
  Filter,
  LayoutGrid,
  List,
  Lock,
  Search,
} from 'lucide-vue-next'
import { ContextMenuContent, ContextMenuItem, ContextMenuPortal, ContextMenuRoot, ContextMenuTrigger } from 'reka-ui'
import { computed, ref, watch } from 'vue'
import type { CardStatsPayload, CardStatView } from '@/bindings'
import CardImage from '@/components/CardImage.vue'
import CardStatsGridCard from '@/components/cards/CardStatsGridCard.vue'
import CardStatsRow from '@/components/cards/CardStatsRow.vue'
import CardTypeFilter from '@/components/cards/CardTypeFilter.vue'
import SegmentedControl from '@/components/SegmentedControl.vue'
import { Button } from '@/components/ui/button'
import { Card, CardContent } from '@/components/ui/card'
import {
  DropdownMenu,
  DropdownMenuCheckboxItem,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { Input } from '@/components/ui/input'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Skeleton } from '@/components/ui/skeleton'
import { Table, TableBody, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { TooltipProvider } from '@/components/ui/tooltip'
import { useShrinkage } from '@/composables/useShrinkage'
import {
  CARD_STATS_COLUMNS,
  configurableColumns,
  effectiveVisibility,
  loadVisibility,
  pickerGroups,
  saveVisibility,
  type CardStatsColumnKey,
  type Visibility,
} from '@/lib/cardStats/columns'
import { readStored, writeStored } from '@/lib/browserStorage'
import { ALL, filtersDiffer, type CardStatsFilters } from '@/lib/cardStats/filters'
import { DEFAULT_SORT, nextSort, sortEntries, type SortKey, type SortState } from '@/lib/cardStats/sort'
import {
  FILTER_KEYS,
  loadTypeFilters,
  passesTypeFilter,
  presentFilterKeys,
  reenableVanished,
  saveTypeFilters,
  type TypeFilters,
} from '@/lib/cardStats/typeFilter'

const props = defineProps<{
  payload: CardStatsPayload
  /** The trust setting (games to trust). */
  trust: number
  loading?: boolean
  /** Offer "Copy screenshot" on a right click (the page listens for `screenshot`). */
  screenshots?: boolean
  /** Bo1 deck: the board is overall and the sideboard columns do not apply. */
  bo1?: boolean
}>()

const emit = defineEmits<{ screenshot: [stat: CardStatView, columns: Visibility] }>()

const menuItemClass =
  'focus:bg-accent focus:text-accent-foreground relative flex cursor-default items-center gap-2 rounded-sm px-2 py-1.5 text-sm outline-hidden select-none [&_svg]:size-4 [&_svg]:shrink-0'
const menuClass = 'z-50 min-w-[10rem] overflow-hidden rounded-md border bg-popover p-1 text-popover-foreground shadow-md'

/** The server-side filters; the page reloads the stats when they change. */
const filters = defineModel<CardStatsFilters>('filters', { required: true })

const stats = computed(() => props.payload.stats)
const perspective = computed(() => props.payload.perspective)
const archetypes = computed(() => props.payload.archetypes)
/** The deck's win rate, the shrinkage prior (a half without games). */
const deckRate = computed(() => props.payload.deckWinrate.rate ?? 0.5)

function setFilter(patch: Partial<CardStatsFilters>) {
  filters.value = { ...filters.value, ...patch }
}

const searchQuery = ref('')

const shrunkStats = useShrinkage({
  stats: () => stats.value,
  prior: () => deckRate.value,
  strength: () => props.trust,
  perspective: () => perspective.value,
})

// Type filter.

const typeFilters = ref<TypeFilters>(loadTypeFilters(localStorage))

function onTypeFiltersUpdate(next: TypeFilters) {
  typeFilters.value = next
  saveTypeFilters(localStorage, next)
}

const present = computed(() => presentFilterKeys(stats.value))
const visibleFilterKeys = computed(() => FILTER_KEYS.filter((key) => present.value.has(key)))

watch(present, (now) => {
  const next = reenableVanished(typeFilters.value, now)
  if (next) onTypeFiltersUpdate(next)
})

// Columns.

const visibleColumns = ref(loadVisibility(localStorage))
const effectiveColumns = computed(() => effectiveVisibility(visibleColumns.value, perspective.value, props.bo1))
const configurable = computed(() => configurableColumns(perspective.value, props.bo1))
const groups = computed(() => pickerGroups(perspective.value, props.bo1))
const allColumnsVisible = computed(() => configurable.value.every((col) => visibleColumns.value[col.key]))
const hiddenColumnCount = computed(() => configurable.value.filter((col) => !effectiveColumns.value[col.key]).length)

function setColumnVisible(key: CardStatsColumnKey, value: boolean) {
  visibleColumns.value = { ...visibleColumns.value, [key]: value }
  saveVisibility(localStorage, visibleColumns.value)
}

function toggleAllColumns() {
  const next = !allColumnsVisible.value
  const updated = { ...visibleColumns.value }
  for (const col of configurable.value) updated[col.key] = next
  visibleColumns.value = updated
  saveVisibility(localStorage, updated)
}

// Sorting. The grid's "Sort by" menu drives the same state as the table headers.

const sort = ref<SortState>({ ...DEFAULT_SORT })

function toggleSort(key: SortKey) {
  sort.value = nextSort(sort.value, key)
}

function sortIcon(key: SortKey) {
  if (sort.value.key !== key) return ChevronsUpDown
  return sort.value.desc ? ChevronDown : ChevronUp
}

function columnLabel(key: SortKey): string {
  if (key === 'name') return 'Name'
  return CARD_STATS_COLUMNS.find((col) => col.key === key)?.label ?? 'Name'
}

const sortOptions = computed(() => {
  const options: { key: SortKey; label: string }[] = [{ key: 'name', label: 'Name' }]
  for (const col of CARD_STATS_COLUMNS) {
    if (col.key === 'type' || col.key === 'sb') continue
    if (!effectiveColumns.value[col.key]) continue
    options.push({ key: col.key, label: col.label })
  }
  return options
})

function chooseSort(key: SortKey) {
  if (sort.value.key !== key) sort.value = { key, desc: key !== 'name' }
}

// View mode.

type ViewMode = 'table' | 'grid'
const VIEW_MODE_KEY = 'cardStatsViewMode'

function loadViewMode(): ViewMode {
  const stored = readStored(localStorage, VIEW_MODE_KEY)
  return stored === 'grid' ? 'grid' : 'table'
}

const viewMode = ref<ViewMode>(loadViewMode())

function setViewMode(mode: ViewMode) {
  viewMode.value = mode
  writeStored(localStorage, VIEW_MODE_KEY, mode)
}

const rows = computed(() => {
  const query = searchQuery.value.toLowerCase()
  const shown = shrunkStats.value.filter(
    (entry) => passesTypeFilter(entry.raw, typeFilters.value) && (!query || entry.raw.name.toLowerCase().includes(query)),
  )
  return sortEntries(shown, sort.value)
})

/** The filter bar stays while any filter differs from its default, so each can be undone. */
const showFilters = computed(
  () => archetypes.value.length > 0 || stats.value.length > 0 || filtersDiffer(filters.value),
)

// Card image on hover.

const hovered = ref<CardStatView | null>(null)
const mouse = ref({ x: 0, y: 0 })

function onImageMove(event: MouseEvent) {
  mouse.value = { x: event.clientX, y: event.clientY }
}
</script>

<template>
  <div v-if="showFilters" class="flex shrink-0 flex-col gap-4">
    <div class="flex items-center gap-4">
      <div class="relative flex-1">
        <Search class="pointer-events-none absolute top-1/2 left-2 size-3.5 -translate-y-1/2 text-muted-foreground" />
        <Input v-model="searchQuery" placeholder="Search cards..." class="pl-7 text-xs" />
      </div>

      <SegmentedControl
        :model-value="filters.perspective"
        :options="[
          { value: 'mine', label: 'My Cards' },
          { value: 'theirs', label: 'Their Cards' },
        ]"
        @update:model-value="(value) => setFilter({ perspective: value === 'theirs' ? 'theirs' : 'mine' })"
      />

      <SegmentedControl
        v-if="!bo1"
        :model-value="filters.board"
        :options="[
          { value: ALL, label: 'Overall' },
          { value: 'game1', label: 'Game 1' },
          { value: 'postboard', label: 'Postboard' },
        ]"
        @update:model-value="(value) => setFilter({ board: value })"
      />

      <div class="flex items-center gap-2">
        <Select :model-value="filters.playDraw" @update:model-value="(value) => setFilter({ playDraw: String(value) })">
          <SelectTrigger>
            <SelectValue placeholder="Play / Draw" />
          </SelectTrigger>
          <SelectContent>
            <SelectItem :value="ALL">Play &amp; Draw</SelectItem>
            <SelectItem value="play">On the Play</SelectItem>
            <SelectItem value="draw">On the Draw</SelectItem>
          </SelectContent>
        </Select>

        <Select
          v-if="archetypes.length"
          :model-value="filters.archetype"
          @update:model-value="(value) => setFilter({ archetype: String(value) })"
        >
          <SelectTrigger>
            <SelectValue placeholder="All opponent archetypes" />
          </SelectTrigger>
          <SelectContent>
            <SelectItem :value="ALL">All opponent archetypes</SelectItem>
            <SelectItem v-for="archetype in archetypes" :key="archetype.id" :value="String(archetype.id)">
              {{ archetype.name }}
            </SelectItem>
          </SelectContent>
        </Select>

        <CardTypeFilter :model-value="typeFilters" :keys="visibleFilterKeys" @update:model-value="onTypeFiltersUpdate" />

        <div v-if="viewMode === 'grid'" class="flex items-center">
          <DropdownMenu>
            <DropdownMenuTrigger as-child>
              <Button
                variant="ghost"
                class="bevel gap-1.5 rounded-l-md rounded-r-none border border-r-0 border-black/60 px-3 py-2 text-xs"
              >
                <ArrowDownUp class="size-3.5" />
                <span>{{ columnLabel(sort.key) }}</span>
                <ChevronDown class="size-3" />
              </Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end" class="w-44">
              <DropdownMenuLabel class="text-xs font-semibold">Sort by</DropdownMenuLabel>
              <DropdownMenuSeparator />
              <DropdownMenuItem v-for="option in sortOptions" :key="option.key" class="text-xs" @select="chooseSort(option.key)">
                <Check v-if="sort.key === option.key" class="size-3.5 text-success" />
                <span :class="sort.key === option.key ? '' : 'pl-[1.375rem]'">{{ option.label }}</span>
              </DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>
          <Button
            variant="ghost"
            class="bevel gap-1.5 rounded-l-none rounded-r-md border border-black/60 px-2 py-2 text-xs"
            :title="sort.desc ? 'Descending' : 'Ascending'"
            @click="sort = { ...sort, desc: !sort.desc }"
          >
            <component :is="sort.desc ? ChevronDown : ChevronUp" class="size-3.5" />
          </Button>
        </div>

        <DropdownMenu>
          <DropdownMenuTrigger as-child>
            <Button variant="ghost" class="bevel gap-1.5 rounded-md border border-black/60 px-3 py-2 text-xs">
              <Columns3 class="size-3.5" />
              <span v-if="hiddenColumnCount > 0">{{ hiddenColumnCount }} hidden</span>
              <span v-else>Columns</span>
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end" class="w-[38rem]">
            <div class="flex items-center justify-between px-2 py-1.5">
              <span class="text-xs font-semibold">Visible columns</span>
              <button type="button" class="text-xs text-muted-foreground hover:text-foreground" @click="toggleAllColumns">
                {{ allColumnsVisible ? 'Hide all' : 'Show all' }}
              </button>
            </div>
            <DropdownMenuSeparator />
            <DropdownMenuLabel class="flex items-center gap-2 text-xs font-normal text-muted-foreground">
              <Lock class="size-3" />
              Card <span class="ml-auto text-[10px]">locked</span>
            </DropdownMenuLabel>
            <DropdownMenuSeparator />
            <template v-for="(group, index) in groups" :key="group.label">
              <DropdownMenuSeparator v-if="index > 0" />
              <DropdownMenuLabel class="text-[10px] font-semibold tracking-wide text-muted-foreground uppercase">
                {{ group.label }}
              </DropdownMenuLabel>
              <div class="grid grid-cols-4 gap-x-1">
                <DropdownMenuCheckboxItem
                  v-for="col in group.columns"
                  :key="col.key"
                  :model-value="visibleColumns[col.key]"
                  class="whitespace-nowrap"
                  @update:model-value="(value: boolean) => setColumnVisible(col.key, value)"
                  @select.prevent
                >
                  <template #indicator-icon><Check class="size-4 text-success" /></template>
                  {{ col.label }}
                </DropdownMenuCheckboxItem>
              </div>
            </template>
          </DropdownMenuContent>
        </DropdownMenu>

        <div class="flex items-center">
          <Button
            variant="ghost"
            class="bevel rounded-l-md rounded-r-none border border-r-0 border-black/60 px-2.5 py-2 text-xs"
            :class="viewMode === 'table' ? 'text-foreground' : 'text-muted-foreground'"
            :aria-pressed="viewMode === 'table'"
            title="Table view"
            @click="setViewMode('table')"
          >
            <List class="size-3.5" />
          </Button>
          <Button
            variant="ghost"
            class="bevel rounded-l-none rounded-r-md border border-black/60 px-2.5 py-2 text-xs"
            :class="viewMode === 'grid' ? 'text-foreground' : 'text-muted-foreground'"
            :aria-pressed="viewMode === 'grid'"
            title="Grid view"
            @click="setViewMode('grid')"
          >
            <LayoutGrid class="size-3.5" />
          </Button>
        </div>
      </div>
    </div>
  </div>

  <Card v-if="loading" class="gap-0 overflow-hidden p-0">
    <CardContent class="flex flex-col gap-2 px-4 py-4">
      <Skeleton class="h-8 w-full" />
      <Skeleton class="h-8 w-full" />
      <Skeleton class="h-8 w-full" />
      <Skeleton class="h-8 w-3/4" />
    </CardContent>
  </Card>

  <div v-else-if="!stats.length && perspective === 'theirs'" class="flex flex-col items-center gap-3 py-16 text-center">
    <BarChart3 class="size-10 text-muted-foreground/40" />
    <p class="font-medium">No opponent cards tracked yet</p>
    <p class="max-w-sm text-sm text-muted-foreground">
      Their cards will appear here as you play more games. Switch back to "My Cards" to see your own stats.
    </p>
  </div>

  <div v-else-if="!stats.length" class="flex flex-col items-center gap-3 py-16 text-center">
    <BarChart3 class="size-10 text-muted-foreground/40" />
    <p class="font-medium">No card stats yet</p>
    <p class="max-w-sm text-sm text-muted-foreground">
      Card performance stats will appear here once you've played some games. Stats may take a moment to compute after
      matches complete.
    </p>
  </div>

  <div v-else-if="!rows.length" class="flex flex-col items-center gap-3 py-16 text-center">
    <Filter class="size-10 text-muted-foreground/40" />
    <p class="font-medium">All card types are hidden</p>
    <p class="max-w-sm text-sm text-muted-foreground">Enable some card types in the filter to view stats.</p>
  </div>

  <TooltipProvider v-else>
    <Card v-if="viewMode === 'table'" class="min-h-0 flex-1 gap-0 p-0">
      <CardContent class="min-h-0 flex-1 px-0 [&_[data-slot=table-container]]:h-full [&_[data-slot=table-container]]:overflow-auto">
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead class="cursor-pointer select-none" @click="toggleSort('name')">
                <span class="inline-flex items-center gap-1">Card <component :is="sortIcon('name')" class="size-3" /></span>
              </TableHead>
              <TableHead v-if="effectiveColumns.type">Type</TableHead>
              <TableHead v-if="effectiveColumns.sb" class="w-10 text-center">SB</TableHead>
              <template v-for="col in CARD_STATS_COLUMNS" :key="col.key">
                <TableHead
                  v-if="col.key !== 'type' && col.key !== 'sb' && effectiveColumns[col.key]"
                  class="cursor-pointer text-right select-none"
                  @click="toggleSort(col.key)"
                >
                  <span class="inline-flex items-center justify-end gap-1">
                    {{ col.label }} <component :is="sortIcon(col.key)" class="size-3" />
                  </span>
                </TableHead>
              </template>
            </TableRow>
          </TableHeader>
          <TableBody>
            <ContextMenuRoot v-for="entry in rows" :key="entry.raw.key" :modal="false">
              <ContextMenuTrigger as-child :disabled="!screenshots">
                <CardStatsRow
                  :stat="entry.raw"
                  :shrunk="entry.shrunk"
                  :raw-rates="entry.rawRates"
                  :samples="entry.samples"
                  :prior="deckRate"
                  :trust="trust"
                  :visible-columns="effectiveColumns"
                  :perspective="perspective"
                  @image-enter="hovered = $event"
                  @image-move="onImageMove"
                  @image-leave="hovered = null"
                />
              </ContextMenuTrigger>
              <ContextMenuPortal>
                <ContextMenuContent :class="menuClass">
                  <ContextMenuItem :class="menuItemClass" @select="emit('screenshot', entry.raw, effectiveColumns)">
                    <Camera class="text-muted-foreground" />
                    Copy screenshot
                  </ContextMenuItem>
                </ContextMenuContent>
              </ContextMenuPortal>
            </ContextMenuRoot>
          </TableBody>
        </Table>
      </CardContent>
    </Card>

    <div
      v-else
      class="grid min-h-0 flex-1 grid-cols-1 content-start gap-3 overflow-y-auto sm:grid-cols-2 xl:grid-cols-3 2xl:grid-cols-4"
    >
      <ContextMenuRoot v-for="entry in rows" :key="entry.raw.key" :modal="false">
        <ContextMenuTrigger as-child :disabled="!screenshots">
          <CardStatsGridCard
            :stat="entry.raw"
            :shrunk="entry.shrunk"
            :raw-rates="entry.rawRates"
            :samples="entry.samples"
            :prior="deckRate"
            :trust="trust"
            :visible-columns="effectiveColumns"
            :perspective="perspective"
          />
        </ContextMenuTrigger>
        <ContextMenuPortal>
          <ContextMenuContent :class="menuClass">
            <ContextMenuItem :class="menuItemClass" @select="emit('screenshot', entry.raw, effectiveColumns)">
              <Camera class="text-muted-foreground" />
              Copy screenshot
            </ContextMenuItem>
          </ContextMenuContent>
        </ContextMenuPortal>
      </ContextMenuRoot>
    </div>
  </TooltipProvider>

  <Teleport to="body">
    <CardImage
      v-if="hovered"
      :src="hovered.image"
      :local="hovered.localImage"
      :alt="hovered.name"
      class="pointer-events-none fixed z-50 w-56 rounded-lg shadow-xl"
      :style="{ top: `${mouse.y - 160}px`, left: `${mouse.x + 16}px` }"
    />
  </Teleport>
</template>
