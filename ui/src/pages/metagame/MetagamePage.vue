<script setup lang="ts">
// The Metagame page (archetypes spec section 9): one tile per opponent
// archetype faced in the format and window, the player's share beside the
// website's, shareable as an image. The global column fills in when the
// archetypes worker pushes `MetagameSharesReady`.
import { Search, Settings2, Shapes } from 'lucide-vue-next'
import { computed, ref, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import SegmentedControl from '@/components/SegmentedControl.vue'
import ArchetypeTile from '@/components/metagame/ArchetypeTile.vue'
import CopyImageButton from '@/components/metagame/CopyImageButton.vue'
import MonthSwitch from '@/components/metagame/MonthSwitch.vue'
import RankTabs from '@/components/metagame/RankTabs.vue'
import TimeframeSwitch from '@/components/metagame/TimeframeSwitch.vue'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Skeleton } from '@/components/ui/skeleton'
import { usePlatformContext } from '@/composables/usePlatformContext'
import { MANAGE_PATH } from '@/lib/archetypes'
import {
  arenaSelection,
  windowLabel as windowLabelText,
  globalNotice,
  headerTitle,
  imageName,
  rangeText,
  chosenTimeframe,
  filterTiles,
  searchQuery,
  readRememberedFormat,
  readRememberedTimeframe,
  useMetagame,
  withoutArchetypeText,
  writeRememberedFormat,
  writeRememberedTimeframe,
} from '@/lib/metagame'

const route = useRoute()
const router = useRouter()

const routeFormat = computed(() => {
  const value = route.params.format
  return typeof value === 'string' && value !== '' ? value : null
})
const format = computed(() => routeFormat.value ?? readRememberedFormat(window.localStorage))
// The timeframe is remembered, so coming back keeps "All time" (issue #326).
const timeframe = computed(() => chosenTimeframe(route.query.timeframe, readRememberedTimeframe(window.localStorage)))
// Arena: the month and rank live in the route query and are not remembered
// (the page opens on the current month, All). MTGO sends neither.
const { platform } = usePlatformContext()
const isArena = computed(() => platform.value === 'arena')
const selection = computed(() => arenaSelection(platform.value, route.query.month, route.query.rank))
const { data: page, error, isPending } = useMetagame(
  format,
  timeframe,
  ref(true),
  computed(() => selection.value.month),
  computed(() => selection.value.rank),
)

// The search lives in the route query (`q`) beside the timeframe, so going
// into an archetype and back keeps it.
const search = ref(searchQuery(route.query.q))
watch(search, (text) => {
  const { q: _previous, ...rest } = route.query
  void router.replace({ query: text === '' ? rest : { ...rest, q: text } })
})
watch(
  () => route.query.q,
  (value) => {
    if (searchQuery(value) !== search.value) search.value = searchQuery(value)
  },
)
const tiles = computed(() => filterTiles(page.value?.tiles ?? [], search.value))

// The format shown goes into the URL and is remembered.
watch(page, (shown) => {
  if (!shown?.format) return
  writeRememberedFormat(window.localStorage, shown.format)
  if (shown.format !== routeFormat.value) {
    void router.replace({ path: `/metagame/${shown.format}`, query: route.query })
  }
})

function showFormat(key: string) {
  void router.push({ path: `/metagame/${key}`, query: route.query })
}

const timeframeModel = computed({
  get: () => timeframe.value,
  set: (key: string) => {
    writeRememberedTimeframe(window.localStorage, key)
    void router.push({ query: { ...route.query, timeframe: key } })
  },
})

const monthModel = computed({
  get: () => page.value?.month ?? '',
  // The new month may have no matches in the chosen band, so the rank resets.
  set: (key: string) => {
    const { rank: _previous, ...rest } = route.query
    void router.push({ query: { ...rest, month: key } })
  },
})
const rankModel = computed({
  get: () => selection.value.rank,
  set: (key: string | null) => {
    const { rank: _previous, ...rest } = route.query
    void router.push({ query: key === null ? rest : { ...rest, rank: key } })
  },
})
// The window's name in the header: the month on Arena, the timeframe on MTGO.
const windowLabel = computed(() =>
  windowLabelText(isArena.value, page.value?.months ?? [], page.value?.month ?? null, page.value?.timeframeLabel ?? ''),
)

const grid = ref<HTMLElement | null>(null)
const notice = computed(() => (page.value && !isArena.value ? globalNotice(page.value.globalState) : null))

// Archetype changes refetch this page through `onArchetypesChanged`
// (debounced), and the global shares through `onMetagameSharesReady`, never
// cancelling a running fetch.
</script>

<template>
  <div class="flex flex-col gap-4 p-3 lg:p-4">
    <div class="flex flex-wrap items-center justify-between gap-3">
      <SegmentedControl
        :model-value="page?.format ?? ''"
        :options="(page?.formats ?? []).map((option) => ({ value: option.key, label: option.label }))"
        @update:model-value="showFormat"
      />
      <div class="flex flex-wrap items-center gap-2">
        <div class="relative w-48">
          <Search class="pointer-events-none absolute top-1/2 left-2 size-3.5 -translate-y-1/2 text-muted-foreground" />
          <Input v-model="search" placeholder="Search archetypes..." aria-label="Search archetypes" class="h-8 pl-7 text-xs" />
        </div>
        <MonthSwitch v-if="page && isArena" v-model="monthModel" :months="page.months" />
        <TimeframeSwitch v-else-if="page" v-model="timeframeModel" :timeframes="page.timeframes" />
        <CopyImageButton
          v-if="page?.format"
          :target="grid"
          :file-name="imageName(page.format, isArena ? (page.month ?? page.timeframe) : page.timeframe)"
        />
        <Button size="sm" variant="outline" as-child>
          <RouterLink :to="MANAGE_PATH">
            <Settings2 class="mr-1.5 size-3.5" />
            Manage archetypes
          </RouterLink>
        </Button>
      </div>
    </div>

    <RankTabs v-if="page && isArena" v-model="rankModel" :bands="page.bands" />

    <p v-if="error" class="text-sm text-destructive">Could not load the metagame: {{ error.message }}</p>

    <template v-else-if="isPending || !page">
      <Skeleton class="h-10 w-72" />
      <div class="grid grid-cols-1 gap-3 sm:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4">
        <Skeleton v-for="n in 8" :key="n" class="h-80 w-full" />
      </div>
    </template>

    <div v-else-if="!page.format" class="flex flex-col items-center gap-2 py-16 text-center">
      <Shapes class="size-10 text-muted-foreground/40" />
      <p class="font-medium">No constructed matches yet</p>
      <p class="text-sm text-muted-foreground">Play a constructed match and its metagame shows here.</p>
    </div>

    <template v-else>
      <div ref="grid" class="flex flex-col gap-3 rounded-lg bg-background p-3">
        <header class="flex flex-wrap items-baseline justify-between gap-2">
          <h1 class="text-base font-semibold tracking-tight">
            {{ headerTitle(page.formatLabel ?? '', windowLabel) }}
          </h1>
          <p class="text-xs text-muted-foreground">
            <span v-if="!isArena && rangeText(page.rangeFrom, page.rangeTo)">{{ rangeText(page.rangeFrom, page.rangeTo) }}</span>
            <span v-if="page.account"> · {{ page.account }}</span>
            <span> · {{ page.totalMatches }} {{ page.totalMatches === 1 ? 'match' : 'matches' }}</span>
          </p>
        </header>
        <p v-if="page.tiles.length === 0" class="py-8 text-center text-sm text-muted-foreground">
          No opponent archetypes in this window.
        </p>
        <p v-else-if="tiles.length === 0" class="py-8 text-center text-sm text-muted-foreground">
          No archetypes match "{{ search.trim() }}".
        </p>
        <div v-else class="grid grid-cols-1 gap-3 sm:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4">
          <ArchetypeTile v-for="tile in tiles" :key="tile.archetypeId" :tile="tile" :global-state="page.globalState" :timeframe="timeframe" :search="search" :show-global="!isArena" :month="selection.month" :rank="selection.rank" />
        </div>
        <footer class="flex flex-col gap-0.5 text-xs text-muted-foreground">
          <span v-if="notice">{{ notice }}</span>
          <span v-if="withoutArchetypeText(page.withoutArchetype)">{{ withoutArchetypeText(page.withoutArchetype) }}</span>
        </footer>
      </div>
    </template>
  </div>
</template>
