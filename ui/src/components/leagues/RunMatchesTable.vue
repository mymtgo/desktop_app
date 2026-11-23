<script setup lang="ts">
import { Trash2 } from 'lucide-vue-next'
import { computed } from 'vue'
import { useRouter } from 'vue-router'
import type { RunMatchView } from '@/bindings'
import ManaColors from '@/components/mana/ManaColors.vue'
import ManualMatchBadge from '@/components/matches/ManualMatchBadge.vue'
import ResultBadge from '@/components/ResultBadge.vue'
import { Button } from '@/components/ui/button'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { durationText, gameCell, NO_VALUE } from '@/lib/leagues'
import { colourLetters } from '@/lib/limited'

/**
 * The matches of one league run: a handful of rows, fixed order, no sorting
 * (0.x `RunMatchesTable`): the opponent and their archetype, or a limited
 * run's opponent colours.
 */
const props = withDefaults(
  defineProps<{
    matches: RunMatchView[]
    /** Every run, constructed or limited: "Remove from league" (league self-management spec 4.2). */
    canUnlink?: boolean
    /** True while an unlink is being saved. */
    busy?: boolean
    /**
     * A limited run: a draft seat has no archetype, so the opponent's colours
     * are its only label (0.x `showArchetype=false`).
     */
    showColours?: boolean
    /** Where a row click goes; without it the rows are not links (the Leagues pages). */
    matchRoute?: (token: string) => string
  }>(),
  { canUnlink: false, busy: false, showColours: false, matchRoute: undefined },
)

const emit = defineEmits<{ unlink: [token: string] }>()

const router = useRouter()

function openMatch(token: string) {
  if (props.matchRoute) void router.push(props.matchRoute(token))
}

/** Each match with its three game cells (null: not played). */
const rows = computed(() =>
  props.matches.map((match) => ({ match, games: [0, 1, 2].map((index) => gameCell(match.games[index])) })),
)
</script>

<template>
  <div class="isolate overflow-hidden rounded-md border border-border bg-card">
    <Table class="table-fixed">
      <TableHeader class="!static !backdrop-blur-none">
        <TableRow>
          <TableHead class="w-[60px] text-center">Match</TableHead>
          <TableHead class="w-[110px]">Result</TableHead>
          <TableHead :class="showColours ? 'w-[160px]' : ''">Opponent</TableHead>
          <TableHead v-if="showColours">Colours</TableHead>
          <TableHead v-else>Archetype</TableHead>
          <TableHead class="w-[120px] text-center">Game 1</TableHead>
          <TableHead class="w-[120px] text-center">Game 2</TableHead>
          <TableHead class="w-[120px] text-center">Game 3</TableHead>
          <TableHead class="w-[80px] text-right">Time</TableHead>
          <TableHead v-if="canUnlink" class="w-[60px]"></TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        <TableRow
          v-for="({ match, games }, matchIndex) in rows"
          :key="match.token"
          :class="matchRoute ? 'cursor-pointer' : ''"
          @click="openMatch(match.token)"
        >
          <TableCell class="text-center text-sm text-muted-foreground tabular-nums">
            {{ matchIndex + 1 }}
          </TableCell>
          <TableCell>
            <ResultBadge
              :text="match.result === 'W' ? 'Win' : 'Loss'"
              :tone="match.result === 'W' ? 'win' : 'loss'"
              :provisional="false"
            />
          </TableCell>
          <TableCell class="truncate font-medium">
            <span v-if="match.opponentName" class="inline-flex max-w-full items-center gap-1.5">
              <span class="truncate">{{ match.opponentName }}</span>
              <ManualMatchBadge v-if="match.manual" />
            </span>
            <span v-else class="text-muted-foreground">{{ NO_VALUE }}</span>
          </TableCell>
          <TableCell v-if="showColours">
            <ManaColors
              v-if="colourLetters(match.opponentColors).length"
              :colors="colourLetters(match.opponentColors)"
            />
            <span v-else class="text-xs text-muted-foreground">{{ NO_VALUE }}</span>
          </TableCell>
          <TableCell v-else class="truncate">
            <span v-if="match.opponentArchetype" class="text-sm">{{ match.opponentArchetype }}</span>
            <span v-else class="text-xs text-muted-foreground">Unknown</span>
          </TableCell>
          <TableCell v-for="(game, index) in games" :key="index" class="text-center text-sm">
            <template v-if="game">
              <span :class="game.tone">{{ game.text }}</span>
              <span v-if="game.side" class="ml-1 text-xs text-muted-foreground">({{ game.side }})</span>
            </template>
            <span v-else class="text-muted-foreground">{{ NO_VALUE }}</span>
          </TableCell>
          <TableCell class="text-right text-muted-foreground tabular-nums">
            {{ durationText(match.durationSeconds) }}
          </TableCell>
          <TableCell v-if="canUnlink" class="text-right">
            <Button
              size="icon"
              variant="ghost"
              class="size-7 text-muted-foreground hover:text-destructive"
              title="Remove from league"
              :disabled="busy"
              @click.stop="emit('unlink', match.token)"
            >
              <Trash2 class="size-3.5" />
            </Button>
          </TableCell>
        </TableRow>
      </TableBody>
    </Table>
  </div>
</template>
