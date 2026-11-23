<script setup lang="ts">
import { Activity, Coins, DoorOpen, Swords, Target, Trophy, Users } from 'lucide-vue-next'
import { computed } from 'vue'
import type { LeagueKpisView } from '@/bindings'
import { Card, CardContent } from '@/components/ui/card'
import { avgFinishText, dropRateText, rateText } from '@/lib/leagues'

/** The KPI strip over every filtered run (0.x `LeagueKpis`). */
const props = defineProps<{ kpis: LeagueKpisView }>()

const avgFinish = computed(() => avgFinishText(props.kpis.avgFinish))
</script>

<template>
  <div class="grid grid-cols-2 gap-4 md:grid-cols-4 lg:grid-cols-7">
    <Card class="gap-0 py-0">
      <CardContent class="flex flex-col gap-0.5 p-3">
        <span class="inline-flex items-center gap-1 text-xs tracking-wide text-muted-foreground uppercase">
          <Activity class="size-3" /> Runs
        </span>
        <span class="text-3xl font-bold tabular-nums">{{ kpis.total }}</span>
        <span class="text-sm text-muted-foreground">{{ kpis.completed }} finished · {{ kpis.live }} live</span>
      </CardContent>
    </Card>

    <Card class="gap-0 py-0">
      <CardContent class="flex flex-col gap-0.5 p-3">
        <span class="inline-flex items-center gap-1 text-xs tracking-wide text-muted-foreground uppercase">
          <Trophy class="size-3" /> Trophies
        </span>
        <span class="text-3xl font-bold tabular-nums">{{ kpis.trophies }}</span>
        <span class="text-sm text-muted-foreground">5-0 finishes</span>
      </CardContent>
    </Card>

    <Card class="gap-0 py-0">
      <CardContent class="flex flex-col gap-0.5 p-3">
        <span class="inline-flex items-center gap-1 text-xs tracking-wide text-muted-foreground uppercase">
          <Target class="size-3" /> Trophy Rate
        </span>
        <span class="text-3xl font-bold tabular-nums">{{ rateText(kpis.trophyRate) }}</span>
        <span class="text-sm text-muted-foreground">across {{ kpis.completed }} runs</span>
      </CardContent>
    </Card>

    <Card class="gap-0 py-0">
      <CardContent class="flex flex-col gap-0.5 p-3">
        <span class="inline-flex items-center gap-1 text-xs tracking-wide text-muted-foreground uppercase">
          <Coins class="size-3" /> Cash Rate
        </span>
        <span class="text-3xl font-bold tabular-nums">{{ rateText(kpis.cashRate) }}</span>
        <span class="text-sm text-muted-foreground">4-1 or better</span>
      </CardContent>
    </Card>

    <Card class="gap-0 py-0">
      <CardContent class="flex flex-col gap-0.5 p-3">
        <span class="inline-flex items-center gap-1 text-xs tracking-wide text-muted-foreground uppercase">
          <DoorOpen class="size-3" /> Dropped
        </span>
        <span class="text-3xl font-bold tabular-nums">{{ kpis.dropped }}</span>
        <span class="text-sm text-muted-foreground">{{ dropRateText(kpis.dropRate) }}</span>
      </CardContent>
    </Card>

    <Card class="gap-0 py-0">
      <CardContent class="flex flex-col gap-0.5 p-3">
        <span class="inline-flex items-center gap-1 text-xs tracking-wide text-muted-foreground uppercase">
          <Users class="size-3" /> Avg Finish
        </span>
        <span class="text-3xl font-bold tabular-nums">{{ avgFinish.value }}</span>
        <span class="text-sm text-muted-foreground">{{ avgFinish.caption }}</span>
      </CardContent>
    </Card>

    <Card class="gap-0 py-0">
      <CardContent class="flex flex-col gap-0.5 p-3">
        <span class="inline-flex items-center gap-1 text-xs tracking-wide text-muted-foreground uppercase">
          <Swords class="size-3" /> Top Matchup
        </span>
        <template v-if="kpis.topMatchup">
          <span class="truncate text-base leading-9 font-semibold">{{ kpis.topMatchup.archetype }}</span>
          <span class="text-sm text-muted-foreground tabular-nums">
            {{ kpis.topMatchup.wins }}-{{ kpis.topMatchup.losses }} · {{ kpis.topMatchup.count }} played
          </span>
        </template>
        <template v-else>
          <span class="text-base leading-9 font-semibold text-muted-foreground">-</span>
          <span class="text-sm text-muted-foreground">Not enough data</span>
        </template>
      </CardContent>
    </Card>
  </div>
</template>
