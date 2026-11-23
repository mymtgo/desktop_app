<script setup lang="ts">
import CardImage from '@/components/CardImage.vue'
import { ArrowDown, ArrowUp, Check, Layers } from 'lucide-vue-next'
import { computed, ref } from 'vue'
import SegmentedControl from '@/components/SegmentedControl.vue'
import { Badge } from '@/components/ui/badge'
import { Skeleton } from '@/components/ui/skeleton'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { errorMessage } from '@/lib/commands'
import { cardFor, NO_VALUE } from '@/lib/limited'
import { nextCardSort, sortCardRows, visibleColumns, type CardSort, type SortKey } from '@/lib/limitedCardColumns'
import {
  cardFilterOptions,
  cardsEmptyText,
  cardsSummaryText,
  cardSubline,
  poolStatusLabel,
  poolStatusTint,
  priorDraftsText,
  winPctText,
  winPctTone,
} from '@/lib/limitedPages'
import { useLimitedCards, useLimitedEvent } from '@/lib/queries'
import { useLimitedKey } from '@/pages/limited/useLimitedKey'

/**
 * Every card of the event (0.x `pages/limited/Cards.vue`): when it was
 * picked, whether it made the deck, how it did when cast, how often it was
 * seen and wheeled, and how it went in the user's other drafts of the set.
 * Sealed has no picks, so the pick columns go.
 */
const key = useLimitedKey()
const { data: event } = useLimitedEvent(key)
const { data: table, error } = useLimitedCards(key)

const isSealed = computed(() => event.value?.kind === 'sealed')
const columns = computed(() => visibleColumns(isSealed.value))

const filter = ref('all')
const sort = ref<CardSort>({ key: event.value?.kind === 'sealed' ? 'status' : 'pick', dir: 'asc' })

const filterOptions = computed(() => cardFilterOptions(table.value?.rows ?? []))
const summary = computed(() =>
  table.value ? cardsSummaryText(table.value.summary, isSealed.value, event.value?.setLabel ?? null) : '',
)
const empty = computed(() => cardsEmptyText(isSealed.value))

function toggleSort(column: SortKey) {
  sort.value = nextCardSort(sort.value, column)
}

const rows = computed(() => {
  if (!table.value) return []
  const cards = table.value.cards
  return sortCardRows(table.value.rows, filter.value, sort.value, (id) => cardFor(cards, id).name).map((row) => ({
    row,
    card: cardFor(cards, row.catalogId),
  }))
})

function alignClass(align: 'left' | 'right' | 'center'): string {
  if (align === 'right') return 'text-right'
  return align === 'center' ? 'text-center' : ''
}
</script>

<template>
  <div class="flex flex-col gap-4 p-3 lg:p-4">
    <div class="flex flex-wrap items-start justify-between gap-4">
      <div class="flex flex-col gap-1">
        <h1 class="text-base font-semibold tracking-tight">Cards</h1>
        <p v-if="table" class="text-xs text-muted-foreground">{{ summary }}</p>
        <Skeleton v-else-if="!error" class="h-4 w-96" />
      </div>
      <SegmentedControl
        v-if="table"
        :model-value="filter"
        :options="filterOptions"
        @update:model-value="(value) => (filter = value)"
      />
    </div>

    <p v-if="error" class="text-sm text-destructive">Could not load the cards: {{ errorMessage(error) }}</p>
    <Skeleton v-else-if="table === undefined" class="h-96 w-full" />
    <template v-else-if="table">
      <div v-if="!table.rows.length" class="flex flex-col items-center gap-2 py-16 text-center">
        <Layers class="size-10 text-muted-foreground/40" />
        <p class="font-medium">{{ empty.title }}</p>
        <p class="text-sm text-muted-foreground">{{ empty.body }}</p>
      </div>
      <div v-else class="overflow-x-auto rounded-lg border border-black/60">
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead
                v-for="col in columns"
                :key="col.key"
                class="cursor-pointer whitespace-nowrap select-none"
                :class="alignClass(col.align)"
                @click="toggleSort(col.key)"
              >
                <span class="inline-flex items-center gap-1">
                  {{ col.label }}
                  <component :is="sort.dir === 'asc' ? ArrowUp : ArrowDown" v-if="sort.key === col.key" class="size-3" />
                </span>
              </TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            <TableRow v-for="{ row, card } in rows" :key="row.catalogId">
              <TableCell>
                <div class="flex items-center gap-2">
                  <CardImage
                    v-if="card.artCrop"
                    :src="card.artCrop"
                    :local="card.localArtCrop"
                    class="size-7 shrink-0 rounded object-cover"
                    :alt="card.name"
                  />
                  <span v-else class="size-7 shrink-0 rounded bg-muted" />
                  <div class="flex flex-col leading-tight">
                    <span class="inline-flex items-center gap-1.5">
                      {{ card.name }}
                      <Badge v-if="row.added" variant="secondary" class="px-1.5 py-0 text-[10px]">Added booster</Badge>
                    </span>
                    <span class="text-[11px] text-muted-foreground">{{ cardSubline(card) }}</span>
                  </div>
                </div>
              </TableCell>
              <TableCell v-if="!isSealed" class="whitespace-nowrap tabular-nums">{{ row.labels.join(' · ') }}</TableCell>
              <TableCell class="text-center">
                <Badge variant="outline" :class="poolStatusTint(row.status)">{{ poolStatusLabel(row.status) }}</Badge>
              </TableCell>
              <TableCell class="text-right tabular-nums">{{ row.gamesCast }}</TableCell>
              <TableCell class="text-right tabular-nums" :class="winPctTone(row.winPctCast)">
                {{ winPctText(row.winPctCast) }}
              </TableCell>
              <template v-if="!isSealed">
                <TableCell class="text-right tabular-nums">{{ row.seenCount }}</TableCell>
                <TableCell class="text-center">
                  <Check v-if="row.wheeled" class="mx-auto size-4 text-muted-foreground" />
                  <span v-else class="text-muted-foreground">{{ NO_VALUE }}</span>
                </TableCell>
                <TableCell class="text-right whitespace-nowrap text-muted-foreground tabular-nums">
                  {{ priorDraftsText(row) }}
                </TableCell>
              </template>
            </TableRow>
          </TableBody>
        </Table>
      </div>
    </template>
  </div>
</template>
