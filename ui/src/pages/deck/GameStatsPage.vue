<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import TimeframeFilter from '@/components/decks/TimeframeFilter.vue'
import { Card } from '@/components/ui/card'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Skeleton } from '@/components/ui/skeleton'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { useDeckPage } from '@/composables/useDeckPage'
import { errorMessage } from '@/lib/commands'
import { formatAverage, formatPercent, gameStatGroups, hasGames, winRateTextClass } from '@/lib/matchups'
import { useDeckGameStats } from '@/lib/queries'

// The deck Game Stats page (0.x `decks/GameStats`): every game and games 1
// to 3, overall, on the play and on the draw, against every opponent or one
// archetype.
const { deckId, timeframe, setTimeframe } = useDeckPage()

const ALL_OPPONENTS = '__all__'
const opponent = ref<string | null>(null)
watch(deckId, () => {
  opponent.value = null
})
const { data: stats, error } = useDeckGameStats(deckId, timeframe, opponent)

const groups = computed(() => gameStatGroups(stats.value?.rows ?? []))

function setOpponent(value: unknown) {
  const next = String(value ?? ALL_OPPONENTS)
  opponent.value = next === ALL_OPPONENTS ? null : next
}

function rateClass(value: number | null): string {
  if (value === null) return 'text-muted-foreground'
  return winRateTextClass(value)
}
</script>

<template>
  <div class="space-y-4 p-3 lg:p-4">
    <div class="flex flex-wrap items-center gap-3">
      <TimeframeFilter :model-value="timeframe" @update:model-value="setTimeframe" />

      <Select v-if="stats && stats.opponents.length > 0" :model-value="opponent ?? ALL_OPPONENTS" @update:model-value="setOpponent">
        <SelectTrigger class="h-9 w-56 text-sm">
          <SelectValue />
        </SelectTrigger>
        <SelectContent>
          <SelectItem :value="ALL_OPPONENTS">All Opponents</SelectItem>
          <SelectItem v-for="option in stats.opponents" :key="option.uuid" :value="option.uuid">
            {{ option.name }}
          </SelectItem>
        </SelectContent>
      </Select>
    </div>

    <p v-if="error" class="text-sm text-destructive">Could not load the game stats: {{ errorMessage(error) }}</p>

    <template v-else-if="stats">
      <div
        v-if="!hasGames(stats.rows)"
        class="rounded-md border border-dashed border-black/40 bg-muted/20 p-8 text-center text-sm text-muted-foreground"
      >
        No games yet. Play some matches with this deck to see stats here.
      </div>

      <Card v-else class="overflow-hidden py-0">
        <Table>
          <TableHeader>
            <TableRow class="bg-muted/30">
              <TableHead class="w-36" />
              <TableHead class="w-24" />
              <TableHead class="text-right">Wins</TableHead>
              <TableHead class="text-right">Losses</TableHead>
              <TableHead class="text-right">Win%</TableHead>
              <TableHead class="border-l border-border/60 text-right">Mulls/G</TableHead>
              <TableHead class="text-right">Opp Mulls/G</TableHead>
              <TableHead class="border-l border-border/60 text-right">Turns/G</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            <template v-for="(group, groupIndex) in groups" :key="group.key">
              <TableRow
                v-for="(row, index) in group.rows"
                :key="`${group.key}-${row.split}`"
                :class="[index === 0 && groupIndex > 0 ? 'border-t-2 border-border' : '', index === 0 ? 'bg-muted/10' : '']"
              >
                <TableCell class="py-2 align-middle">
                  <span v-if="index === 0" class="text-sm font-semibold tracking-tight">{{ group.label }}</span>
                </TableCell>
                <TableCell class="py-2 text-xs text-muted-foreground">{{ row.label }}</TableCell>
                <TableCell class="py-2 text-right tabular-nums">
                  <span :class="row.wins > 0 ? 'font-medium text-success' : 'text-muted-foreground'">{{ row.wins }}</span>
                </TableCell>
                <TableCell class="py-2 text-right tabular-nums">
                  <span :class="row.losses > 0 ? 'font-medium text-destructive' : 'text-muted-foreground'">{{ row.losses }}</span>
                </TableCell>
                <TableCell class="py-2 text-right tabular-nums">
                  <span class="font-medium" :class="rateClass(row.winRate)">{{ formatPercent(row.winRate) }}</span>
                </TableCell>
                <TableCell class="border-l border-border/60 py-2 text-right text-muted-foreground tabular-nums">
                  {{ formatAverage(row.mulligans) }}
                </TableCell>
                <TableCell class="py-2 text-right text-muted-foreground tabular-nums">
                  {{ formatAverage(row.opponentMulligans) }}
                </TableCell>
                <TableCell class="border-l border-border/60 py-2 text-right text-muted-foreground tabular-nums">
                  {{ formatAverage(row.turns) }}
                </TableCell>
              </TableRow>
            </template>
          </TableBody>
        </Table>
      </Card>
    </template>

    <div v-else class="space-y-2">
      <Skeleton v-for="n in 12" :key="n" class="h-8 w-full" />
    </div>
  </div>
</template>
