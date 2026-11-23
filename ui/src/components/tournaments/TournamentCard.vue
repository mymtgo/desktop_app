<script setup lang="ts">
import { Calendar, ChevronDown, Clock } from 'lucide-vue-next'
import { computed, ref } from 'vue'
import type { TournamentView } from '@/bindings'
import ResultBadge from '@/components/ResultBadge.vue'
import ResultDots from '@/components/leagues/ResultDots.vue'
import { Badge } from '@/components/ui/badge'
import { Card } from '@/components/ui/card'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { lastPlayedText } from '@/lib/decks'
import { avgMinutes, NO_VALUE, startedDayMonth } from '@/lib/leagues'
import { liveText, roundRows, roundsText, sideTotal } from '@/lib/tournaments'

/**
 * One tournament on a deck's Tournaments tab (0.x `TournamentCard` with
 * `hide-deck-identity`): the W-L box, the result dots with a pulsing dot per
 * live match, and, expanded, the game stats and the rounds table.
 */
const props = withDefaults(
  defineProps<{
    tournament: TournamentView
    defaultExpanded?: boolean
    /** "Now" for the relative start (the data's fetch time); the render time when left out. */
    now?: number
  }>(),
  { defaultExpanded: false, now: undefined },
)

const expanded = ref(props.defaultExpanded)

const startedHuman = computed(() => lastPlayedText(props.tournament.startedAtMs, props.now ?? Date.now()))
const startedDay = computed(() => startedDayMonth(props.tournament.startedAtMs))
const avgMin = computed(() => avgMinutes(props.tournament.avgMatchSeconds))
const rows = computed(() => roundRows(props.tournament))
</script>

<template>
  <Card class="gap-0 overflow-hidden p-0">
    <button
      type="button"
      class="flex w-full items-center gap-4 px-4 py-3 text-left hover:bg-muted/40"
      :aria-expanded="expanded"
      @click="expanded = !expanded"
    >
      <div
        class="relative flex h-16 w-24 shrink-0 flex-col items-center justify-center rounded-md border border-muted-foreground/30 bg-muted/40 tabular-nums text-muted-foreground"
      >
        <span class="text-2xl leading-none font-bold">{{ tournament.wins }}-{{ tournament.losses }}</span>
      </div>

      <div class="flex min-w-0 flex-1 flex-col gap-1">
        <div class="flex flex-wrap items-center gap-2 text-sm">
          <span class="truncate font-medium">{{ tournament.name }}</span>
          <Badge v-if="tournament.format" variant="outline" class="shrink-0 text-[10px] tracking-wider uppercase">
            {{ tournament.format }}
          </Badge>
          <span v-if="tournament.versionLabel" class="text-xs text-muted-foreground">
            {{ tournament.versionLabel }}
          </span>
          <div class="flex items-center gap-1">
            <ResultDots :results="tournament.results" />
            <span
              v-for="i in tournament.inProgressCount"
              :key="`p${i}`"
              class="size-3 animate-pulse rounded-full bg-amber-400/70"
              aria-label="Match in progress"
            />
          </div>
        </div>

        <div class="flex flex-wrap items-center gap-x-4 gap-y-1 text-xs text-muted-foreground">
          <span class="inline-flex items-center gap-1">
            <Calendar class="size-3" />
            {{ startedHuman }}
            <span> · {{ startedDay }}</span>
          </span>
          <span v-if="avgMin" class="inline-flex items-center gap-1">
            <Clock class="size-3" /> avg
            <span class="font-medium text-foreground">{{ avgMin }}m</span>
          </span>
          <span class="inline-flex items-center gap-1 tabular-nums">{{ roundsText(tournament.matchesCount) }}</span>
          <span
            v-if="tournament.inProgressCount > 0"
            class="inline-flex items-center gap-1 rounded-full bg-amber-500/15 px-2 py-0.5 text-[10px] font-medium tracking-wider text-amber-400 uppercase"
          >
            {{ liveText(tournament.inProgressCount) }}
          </span>
        </div>
      </div>

      <div class="shrink-0">
        <ChevronDown class="size-4 text-muted-foreground transition-transform" :class="{ 'rotate-180': expanded }" />
      </div>
    </button>

    <div v-if="expanded" class="flex flex-col gap-4 border-t border-border p-4">
      <div class="grid grid-cols-2 gap-4 sm:grid-cols-4">
        <div>
          <div class="text-xl font-semibold text-emerald-400 tabular-nums">{{ tournament.gameWins }}</div>
          <div class="text-xs text-muted-foreground">Game wins</div>
        </div>
        <div>
          <div class="text-xl font-semibold text-destructive tabular-nums">{{ tournament.gameLosses }}</div>
          <div class="text-xs text-muted-foreground">Game losses</div>
        </div>
        <div>
          <div class="text-xl font-semibold tabular-nums">{{ sideTotal(tournament.onPlayRecord) }}</div>
          <div class="text-xs text-muted-foreground">On the play</div>
        </div>
        <div>
          <div class="text-xl font-semibold tabular-nums">{{ sideTotal(tournament.onDrawRecord) }}</div>
          <div class="text-xs text-muted-foreground">On the draw</div>
        </div>
      </div>

      <div class="isolate overflow-hidden rounded-md border border-border bg-card">
        <Table class="table-fixed">
          <TableHeader class="!static !backdrop-blur-none">
            <TableRow>
              <TableHead class="w-[60px]">Round</TableHead>
              <TableHead class="w-[100px]">Result</TableHead>
              <TableHead class="w-[160px]">Opponent</TableHead>
              <TableHead>Vs</TableHead>
              <TableHead class="w-[120px] text-center">Game 1</TableHead>
              <TableHead class="w-[120px] text-center">Game 2</TableHead>
              <TableHead class="w-[120px] text-center">Game 3</TableHead>
              <TableHead class="w-[80px] text-right">Time</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            <TableRow v-for="row in rows" :key="row.token">
              <TableCell class="font-mono tabular-nums">{{ row.round }}</TableCell>
              <TableCell>
                <span v-if="row.live" class="inline-flex items-center gap-1.5 text-xs font-medium text-amber-400">
                  <span class="size-2 animate-pulse rounded-full bg-amber-400" />
                  Live
                </span>
                <ResultBadge
                  v-else
                  :text="row.won ? 'Win' : 'Loss'"
                  :tone="row.won ? 'win' : 'loss'"
                  :provisional="false"
                />
              </TableCell>
              <TableCell class="truncate font-medium">
                <span v-if="row.opponent">{{ row.opponent }}</span>
                <span v-else class="text-muted-foreground">{{ NO_VALUE }}</span>
              </TableCell>
              <TableCell class="truncate">
                <span v-if="row.archetype" class="text-sm">{{ row.archetype }}</span>
                <span v-else class="text-xs text-muted-foreground">Unknown</span>
              </TableCell>
              <TableCell v-for="(cell, i) in row.games" :key="i" class="text-center text-sm">
                <template v-if="cell">
                  <span :class="cell.tone">{{ cell.text }}</span>
                  <span v-if="cell.side" class="ml-1 text-xs text-muted-foreground">({{ cell.side }})</span>
                </template>
                <span v-else class="text-muted-foreground">{{ NO_VALUE }}</span>
              </TableCell>
              <TableCell class="text-right text-muted-foreground tabular-nums">{{ row.time }}</TableCell>
            </TableRow>
          </TableBody>
        </Table>
      </div>
    </div>
  </Card>
</template>
