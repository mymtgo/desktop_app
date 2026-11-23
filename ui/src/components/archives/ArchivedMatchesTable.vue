<script setup lang="ts">
import { ref } from 'vue'
import type { DeletedMatchView } from '@/bindings'
import ResultBadge from '@/components/ResultBadge.vue'
import { Button } from '@/components/ui/button'
import { Empty, EmptyHeader, EmptyTitle } from '@/components/ui/empty'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { useLeagueManage } from '@/composables/useLeagueManage'
import { useMatchEdit } from '@/composables/useMatchEdit'
import { deckCell, deletedResultText, resultTone } from '@/lib/archives'
import { matchDateText } from '@/lib/matchList'

/**
 * Settings, Archives, Matches (league self-management spec 5.5): the
 * deleted matches, newest deleted first as the query orders them, each with
 * Restore. A restored match leaves the table when the query refetches.
 *
 * A match deleted together with its league comes back only with the league
 * (issue #418): its row says so and offers "Restore league", the Leagues
 * tab's Restore, which refreshes both tabs (one query).
 */
defineProps<{ matches: DeletedMatchView[] }>()

const { restore } = useMatchEdit()
const { restoreLeague } = useLeagueManage()

/** The tokens being restored: only their buttons are busy. */
const restoring = ref<string[]>([])

async function restoreRow(row: DeletedMatchView) {
  const token = row.token
  restoring.value = [...restoring.value, token]
  try {
    if (row.deletedWithLeague) await restoreLeague(row.deletedWithLeague.runKey)
    else await restore(token)
  } finally {
    restoring.value = restoring.value.filter((each) => each !== token)
  }
}
</script>

<template>
  <Empty v-if="matches.length === 0" class="rounded-md border">
    <EmptyHeader>
      <EmptyTitle class="text-sm">No deleted matches</EmptyTitle>
    </EmptyHeader>
  </Empty>
  <div v-else class="isolate overflow-hidden rounded-md border border-border bg-card">
    <Table>
      <TableHeader class="!static !backdrop-blur-none">
        <TableRow>
          <TableHead>Date</TableHead>
          <TableHead>Deck</TableHead>
          <TableHead>League</TableHead>
          <TableHead>Opponent</TableHead>
          <TableHead>Result</TableHead>
          <TableHead>Deleted</TableHead>
          <TableHead class="w-[90px]"></TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        <TableRow v-for="row in matches" :key="row.token">
          <TableCell class="text-sm tabular-nums">{{ matchDateText(row.startedAtMs) }}</TableCell>
          <TableCell class="truncate text-sm" data-cell="deck" :class="{ 'text-muted-foreground': deckCell(row.deckName, null).muted }">
            {{ deckCell(row.deckName, null).text }}
          </TableCell>
          <TableCell class="truncate text-sm" data-cell="league">
            <template v-if="row.leagueName">
              {{ row.leagueName }}
              <span v-if="row.leagueRemoved" class="text-xs text-muted-foreground">(deleted)</span>
            </template>
            <span v-else class="text-muted-foreground">-</span>
          </TableCell>
          <TableCell class="truncate font-medium">
            <span v-if="row.opponent">{{ row.opponent }}</span>
            <span v-else class="text-muted-foreground">Unknown opponent</span>
          </TableCell>
          <TableCell>
            <ResultBadge :text="deletedResultText(row.result)" :tone="resultTone(row.result)" :provisional="false" />
          </TableCell>
          <TableCell class="text-sm text-muted-foreground tabular-nums">{{ matchDateText(row.hiddenAtMs) }}</TableCell>
          <TableCell class="text-right">
            <div v-if="row.deletedWithLeague" class="flex items-center justify-end gap-2">
              <span class="text-xs whitespace-nowrap text-muted-foreground">Deleted with its league</span>
              <Button
                variant="outline"
                size="sm"
                data-restore="match-league"
                :disabled="restoring.includes(row.token)"
                @click="restoreRow(row)"
              >
                Restore league
              </Button>
            </div>
            <Button
              v-else
              variant="outline"
              size="sm"
              data-restore="match"
              :disabled="restoring.includes(row.token)"
              @click="restoreRow(row)"
            >
              Restore
            </Button>
          </TableCell>
        </TableRow>
      </TableBody>
    </Table>
  </div>
</template>
