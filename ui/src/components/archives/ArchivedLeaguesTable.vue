<script setup lang="ts">
import { ref } from 'vue'
import type { DeletedLeagueView } from '@/bindings'
import { Button } from '@/components/ui/button'
import { Empty, EmptyHeader, EmptyTitle } from '@/components/ui/empty'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { useLeagueManage } from '@/composables/useLeagueManage'
import { deckCell, recordText } from '@/lib/archives'
import { startedDayMonth } from '@/lib/leagues'
import { matchDateText } from '@/lib/matchList'

/**
 * Settings, Archives, Leagues (league self-management spec 5.5): the
 * deleted leagues, newest deleted first as the query orders them, each with
 * Restore. Restore brings back the matches deleted with the league too
 * (spec 4.5), so a restored league leaves this table and its matches leave
 * the Matches tab when the query refetches.
 */
defineProps<{ leagues: DeletedLeagueView[] }>()

const { restoreLeague } = useLeagueManage()

/** The run keys being restored: only their buttons are busy. */
const restoring = ref<string[]>([])

async function restoreRow(runKey: string) {
  restoring.value = [...restoring.value, runKey]
  try {
    await restoreLeague(runKey)
  } finally {
    restoring.value = restoring.value.filter((each) => each !== runKey)
  }
}
</script>

<template>
  <Empty v-if="leagues.length === 0" class="rounded-md border">
    <EmptyHeader>
      <EmptyTitle class="text-sm">No deleted leagues</EmptyTitle>
    </EmptyHeader>
  </Empty>
  <div v-else class="flex flex-col gap-2">
    <p class="text-sm text-muted-foreground">Restore brings a league back with its matches, those deleted with it too.</p>
    <div class="isolate overflow-hidden rounded-md border border-border bg-card">
      <Table>
        <TableHeader class="!static !backdrop-blur-none">
          <TableRow>
            <TableHead>League</TableHead>
            <TableHead>Deck</TableHead>
            <TableHead>Started</TableHead>
            <TableHead>Record</TableHead>
            <TableHead>Deleted</TableHead>
            <TableHead class="w-[90px]"></TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          <TableRow v-for="league in leagues" :key="league.runKey">
            <TableCell class="truncate font-medium">{{ league.name }}</TableCell>
            <TableCell class="truncate text-sm" data-cell="deck" :class="{ 'text-muted-foreground': deckCell(league.deckName, league.format).muted }">
              {{ deckCell(league.deckName, league.format).text }}
            </TableCell>
            <TableCell class="text-sm tabular-nums">{{ startedDayMonth(league.startedAtMs) }}</TableCell>
            <TableCell class="text-sm tabular-nums">{{ recordText(league.wins, league.losses) }}</TableCell>
            <TableCell class="text-sm text-muted-foreground tabular-nums">{{ matchDateText(league.removedAtMs) }}</TableCell>
            <TableCell class="text-right">
              <Button
                variant="outline"
                size="sm"
                data-restore="league"
                :disabled="restoring.includes(league.runKey)"
                @click="restoreRow(league.runKey)"
              >
                Restore
              </Button>
            </TableCell>
          </TableRow>
        </TableBody>
      </Table>
    </div>
  </div>
</template>
