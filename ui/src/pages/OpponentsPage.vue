<script setup lang="ts">
import { Search, Skull, Swords } from 'lucide-vue-next'
import { computed, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import SegmentedControl from '@/components/SegmentedControl.vue'
import WinRateBar from '@/components/decks/WinRateBar.vue'
import ManaColors from '@/components/mana/ManaColors.vue'
import MatchRecord from '@/components/decks/MatchRecord.vue'
import { Card } from '@/components/ui/card'
import { Input } from '@/components/ui/input'
import PagedNav from '@/components/PagedNav.vue'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Skeleton } from '@/components/ui/skeleton'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from '@/components/ui/tooltip'
import { useDebouncedSearch } from '@/composables/useDebouncedSearch'
import {
  OPPONENT_SORTS,
  clampedPage,
  opponentTag,
  opponentsFromUrl,
  opponentsUrlQuery,
  relativeTime,
  tagTooltip,
  type OpponentsState,
} from '@/lib/opponents'
import { useOpponents } from '@/lib/queries'

// 0.x Opponents page: search, sort and format filter, the table with the
// Nemesis and Rival tags, the archetypes seen (three shown, the rest in a
// tooltip), and pages of 25.
// Everything lives in the URL, so back and forward step through it.
/** Archetype chips shown before "+n more" (0.x). */
const VISIBLE_ARCHETYPES = 3

const route = useRoute()
const router = useRouter()

const query = computed<OpponentsState>(() => opponentsFromUrl(route.query))
const { data, error, isPending, isPlaceholderData, dataUpdatedAt } = useOpponents(query)

function setQuery(next: OpponentsState) {
  void router.push({ query: opponentsUrlQuery(next) })
}

// Every filter change goes back to the first page.
function change(next: Partial<OpponentsState>) {
  setQuery({ ...query.value, ...next, page: 1 })
}

const search = useDebouncedSearch({
  current: () => query.value.search,
  apply: (next) => change({ search: next }),
})

// A page past the end shows the last page: follow it.
watch(data, (shown) => {
  if (!shown || isPlaceholderData.value) return
  const page = clampedPage(query.value.page, shown.total, shown.perPage)
  if (page === null) return
  void router.replace({ query: opponentsUrlQuery({ ...query.value, page }) })
})

const lastPage = computed(() => (data.value ? Math.max(1, Math.ceil(data.value.total / data.value.perPage)) : 1))
</script>

<template>
  <div class="flex flex-col gap-4 p-3 lg:p-4">
    <h1 class="text-base font-semibold tracking-tight">Opponents</h1>

    <div class="flex flex-wrap items-center gap-3">
      <div class="relative">
        <Search class="absolute top-1/2 left-2 size-3.5 -translate-y-1/2 text-muted-foreground" />
        <Input v-model="search" placeholder="Search opponents..." aria-label="Search opponents" class="h-8 w-56 pl-7" />
      </div>

      <Select :model-value="query.sort" @update:model-value="(value) => change({ sort: value as OpponentsState['sort'] })">
        <SelectTrigger class="h-8 w-48">
          <SelectValue />
        </SelectTrigger>
        <SelectContent>
          <SelectItem v-for="option in OPPONENT_SORTS" :key="option.value" :value="option.value">
            {{ option.label }}
          </SelectItem>
        </SelectContent>
      </Select>

      <SegmentedControl
        class="ml-auto"
        :model-value="query.format ?? ''"
        :options="[{ value: '', label: 'All' }, ...(data?.formats ?? []).map((format) => ({ value: format.code, label: format.name }))]"
        @update:model-value="(value) => change({ format: value === '' ? null : value })"
      />
    </div>

    <p v-if="error" class="text-sm text-destructive">Could not load opponents: {{ error.message }}</p>

    <template v-else-if="isPending || !data">
      <Skeleton v-for="n in 6" :key="n" class="h-12 w-full" />
    </template>

    <Card v-else class="overflow-hidden">
      <Table>
        <TableHeader>
          <TableRow>
            <TableHead class="w-0"></TableHead>
            <TableHead>Opponent</TableHead>
            <TableHead>Archetypes Seen</TableHead>
            <TableHead>Record</TableHead>
            <TableHead>Formats</TableHead>
            <TableHead>Last Played</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          <TableRow v-if="data.rows.length === 0">
            <TableCell colspan="6" class="py-12 text-center">
              <div class="flex flex-col items-center gap-2">
                <Swords class="size-10 text-muted-foreground/40" />
                <p class="font-medium">No opponents found</p>
                <p class="text-sm text-muted-foreground">Play some matches and your opponents will show up here.</p>
              </div>
            </TableCell>
          </TableRow>
          <TableRow v-for="opponent in data.rows" :key="opponent.opponentId">
            <TableCell>
              <TooltipProvider v-if="opponentTag(opponent.record)">
                <Tooltip>
                  <TooltipTrigger as-child>
                    <span
                      v-if="opponentTag(opponent.record) === 'nemesis'"
                      class="inline-flex items-center gap-1 rounded-full border border-red-500/50 px-2 py-0.5 text-xs font-medium text-red-500"
                    >
                      <Skull class="size-3" />
                      Nemesis
                    </span>
                    <span
                      v-else
                      class="inline-flex items-center gap-1 rounded-full border border-indigo-500/50 px-2 py-0.5 text-xs font-medium text-indigo-500"
                    >
                      <Swords class="size-3" />
                      Rival
                    </span>
                  </TooltipTrigger>
                  <TooltipContent>{{ tagTooltip(opponentTag(opponent.record)!) }}</TooltipContent>
                </Tooltip>
              </TooltipProvider>
            </TableCell>
            <TableCell>
              <span class="font-medium">{{ opponent.username }}</span>
            </TableCell>
            <TableCell>
              <TooltipProvider v-if="opponent.archetypes.length > 0">
                <div class="flex flex-wrap items-center gap-1.5">
                  <Tooltip v-for="archetype in opponent.archetypes.slice(0, VISIBLE_ARCHETYPES)" :key="archetype.name">
                    <TooltipTrigger>
                      <div class="flex items-center gap-1 rounded border px-1.5 py-0.5 text-xs">
                        <ManaColors v-if="archetype.colors.length" :colors="archetype.colors" />
                        <span class="max-w-24 truncate">{{ archetype.name }}</span>
                      </div>
                    </TooltipTrigger>
                    <TooltipContent>{{ archetype.name }}</TooltipContent>
                  </Tooltip>
                  <Tooltip v-if="opponent.archetypes.length > VISIBLE_ARCHETYPES">
                    <TooltipTrigger>
                      <span class="text-xs text-muted-foreground">
                        +{{ opponent.archetypes.length - VISIBLE_ARCHETYPES }} more
                      </span>
                    </TooltipTrigger>
                    <TooltipContent>
                      <div class="flex flex-col gap-0.5">
                        <span v-for="archetype in opponent.archetypes.slice(VISIBLE_ARCHETYPES)" :key="archetype.name">
                          {{ archetype.name }}
                        </span>
                      </div>
                    </TooltipContent>
                  </Tooltip>
                </div>
              </TooltipProvider>
              <span v-else class="text-sm text-muted-foreground">Unknown</span>
            </TableCell>
            <TableCell>
              <div class="flex flex-col gap-1">
                <WinRateBar :winrate="opponent.record.winRate" size="sm" />
                <MatchRecord :record="opponent.record" />
              </div>
            </TableCell>
            <TableCell class="text-sm text-muted-foreground">{{ opponent.formats.join(', ') }}</TableCell>
            <TableCell class="text-sm whitespace-nowrap text-muted-foreground">
              {{ relativeTime(opponent.lastPlayedAtMs, dataUpdatedAt) }}
            </TableCell>
          </TableRow>
        </TableBody>
      </Table>

      <PagedNav
        v-if="lastPage > 1"
        class="justify-end px-2 py-2"
        :per-page="data.perPage"
        :total="data.total"
        :page="data.page"
        @update:page="(page) => setQuery({ ...query, page })"
      />
    </Card>
  </div>
</template>
