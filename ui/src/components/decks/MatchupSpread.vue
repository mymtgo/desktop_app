<script setup lang="ts">
import { Camera, ChevronDown, ChevronsUpDown, ChevronUp } from 'lucide-vue-next'
import { ContextMenuContent, ContextMenuItem, ContextMenuPortal, ContextMenuRoot, ContextMenuTrigger } from 'reka-ui'
import { computed, ref } from 'vue'
import type { MatchupView } from '@/bindings'
import ManaColors from '@/components/mana/ManaColors.vue'
import { Card, CardContent } from '@/components/ui/card'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import {
  DEFAULT_SPREAD_SORT,
  LOW_DATA_THRESHOLD,
  nextSpreadSort,
  spreadColumns,
  sortMatchups,
  splitByData,
  type SpreadSort,
  type SpreadSortKey,
  winRateTextClass,
} from '@/lib/matchups'

/**
 * A deck's results against each opponent archetype (0.x
 * `MatchupSpreadTable`): sortable columns, archetypes faced fewer than 4
 * times under a divider. With `selectable`, a row click emits `select`
 * (the deck page opens the drawer); with `screenshots`, a right click
 * offers "Copy screenshot".
 */
const props = withDefaults(
  defineProps<{ matchups: MatchupView[]; selectable?: boolean; screenshots?: boolean; bo1?: boolean }>(),
  { selectable: false, screenshots: false, bo1: false },
)

const emit = defineEmits<{ select: [row: MatchupView]; screenshot: [row: MatchupView] }>()

const itemClass =
  'focus:bg-accent focus:text-accent-foreground relative flex cursor-default items-center gap-2 rounded-sm px-2 py-1.5 text-sm outline-hidden select-none [&_svg]:size-4 [&_svg]:shrink-0'

const sort = ref<SpreadSort>({ ...DEFAULT_SPREAD_SORT })

function toggle(key: SpreadSortKey) {
  sort.value = nextSpreadSort(sort.value, key)
}

function icon(key: SpreadSortKey) {
  if (sort.value.key !== key) return ChevronsUpDown
  return sort.value.desc ? ChevronDown : ChevronUp
}

const split = computed(() => splitByData(props.matchups))
const main = computed(() => sortMatchups(split.value.main, sort.value))
const lowData = computed(() => sortMatchups(split.value.lowData, sort.value))

const columns = computed(() => spreadColumns(props.bo1))
</script>

<template>
  <Card class="gap-0 overflow-hidden p-0">
    <CardContent class="px-0">
      <p v-if="!matchups.length" class="py-8 text-center text-sm text-muted-foreground">No matchup data yet.</p>
      <Table v-else>
        <TableHeader class="sticky top-0 z-10 backdrop-blur-sm">
          <TableRow>
            <TableHead class="w-8 pr-0 pl-3" />
            <TableHead class="cursor-pointer select-none" @click="toggle('name')">
              <span class="inline-flex items-center gap-1">Archetype <component :is="icon('name')" class="size-3" /></span>
            </TableHead>
            <TableHead
              v-for="column in columns"
              :key="column.label"
              class="cursor-pointer select-none"
              :class="column.class"
              @click="toggle(column.key)"
            >
              <span class="inline-flex items-center justify-end gap-1">
                {{ column.label }} <component :is="icon(column.key)" class="size-3" />
              </span>
            </TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          <template v-for="(group, groupIndex) in [main, lowData]" :key="groupIndex">
            <TableRow v-if="groupIndex === 1 && group.length" class="pointer-events-none">
              <TableCell :colspan="columns.length + 2" class="py-1.5">
                <div class="flex items-center gap-3">
                  <div class="h-px flex-1 bg-border" />
                  <span class="text-xs text-muted-foreground/60">Less than {{ LOW_DATA_THRESHOLD }} matches</span>
                  <div class="h-px flex-1 bg-border" />
                </div>
              </TableCell>
            </TableRow>
            <ContextMenuRoot v-for="matchup in group" :key="matchup.archetypeId" :modal="false">
              <ContextMenuTrigger as-child :disabled="!screenshots">
                <TableRow
                  :class="[selectable && 'cursor-pointer', groupIndex === 1 && 'opacity-60']"
                  @click="selectable && emit('select', matchup)"
                >
                  <TableCell class="w-8 pr-0 pl-3">
                    <ManaColors :colors="matchup.colors" class="shrink-0" />
                  </TableCell>
                  <TableCell class="truncate">{{ matchup.name }}</TableCell>
                  <TableCell class="text-right text-muted-foreground tabular-nums">{{ matchup.matchRecord }}</TableCell>
                  <TableCell class="text-right text-muted-foreground tabular-nums">{{ matchup.matches }}</TableCell>
                  <TableCell class="text-right">
                    <span class="font-medium tabular-nums" :class="winRateTextClass(matchup.matchWinRate)">{{ matchup.matchWinRate }}%</span>
                  </TableCell>
                  <template v-if="!bo1">
                    <TableCell class="border-l border-border text-right text-muted-foreground tabular-nums">
                      {{ matchup.gameRecord }}
                    </TableCell>
                    <TableCell class="text-right">
                      <span class="font-medium tabular-nums" :class="winRateTextClass(matchup.gameWinRate)">{{ matchup.gameWinRate }}%</span>
                    </TableCell>
                  </template>
                  <TableCell class="text-right">
                    <span class="font-medium tabular-nums" :class="winRateTextClass(matchup.otpWinRate)">{{ matchup.otpWinRate }}%</span>
                  </TableCell>
                  <TableCell class="text-right text-muted-foreground tabular-nums">{{ matchup.avgTurns ?? '-' }}</TableCell>
                </TableRow>
              </ContextMenuTrigger>
              <ContextMenuPortal>
                <ContextMenuContent
                  class="z-50 min-w-[10rem] overflow-hidden rounded-md border bg-popover p-1 text-popover-foreground shadow-md"
                >
                  <ContextMenuItem :class="itemClass" @select="emit('screenshot', matchup)">
                    <Camera class="text-muted-foreground" />
                    Copy screenshot
                  </ContextMenuItem>
                </ContextMenuContent>
              </ContextMenuPortal>
            </ContextMenuRoot>
          </template>
        </TableBody>
      </Table>
    </CardContent>
  </Card>
</template>
