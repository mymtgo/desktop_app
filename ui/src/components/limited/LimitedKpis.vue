<script setup lang="ts">
import { Activity, Clock, Layers, Swords, Target, Timer } from 'lucide-vue-next'
import type { LimitedKpisView } from '@/bindings'
import { Card, CardContent } from '@/components/ui/card'
import { avgRecordText, formatSeconds, NO_VALUE, percentText } from '@/lib/limited'

/** The Limited index KPI strip (0.x `LimitedKpis`). */
defineProps<{ kpis: LimitedKpisView }>()
</script>

<template>
  <div class="grid grid-cols-2 gap-4 md:grid-cols-3 lg:grid-cols-6">
    <Card class="gap-0 py-0">
      <CardContent class="flex flex-col gap-0.5 p-3">
        <span class="inline-flex items-center gap-1 text-xs tracking-wide text-muted-foreground uppercase">
          <Activity class="size-3" /> Events
        </span>
        <span class="text-3xl font-bold tabular-nums">{{ kpis.events }}</span>
        <span class="text-sm text-muted-foreground">{{ kpis.drafts }} drafts · {{ kpis.unlinked }} unlinked</span>
      </CardContent>
    </Card>

    <Card class="gap-0 py-0">
      <CardContent class="flex flex-col gap-0.5 p-3">
        <span class="inline-flex items-center gap-1 text-xs tracking-wide text-muted-foreground uppercase">
          <Swords class="size-3" /> Match win
        </span>
        <span class="text-3xl font-bold tabular-nums">{{ percentText(kpis.matchWinPct) }}</span>
        <span class="text-sm text-muted-foreground">
          {{ kpis.matchWins }}-{{ kpis.matchLosses }} across {{ kpis.matchWins + kpis.matchLosses }} matches
        </span>
      </CardContent>
    </Card>

    <Card class="gap-0 py-0">
      <CardContent class="flex flex-col gap-0.5 p-3">
        <span class="inline-flex items-center gap-1 text-xs tracking-wide text-muted-foreground uppercase">
          <Target class="size-3" /> Avg record
        </span>
        <span class="text-3xl font-bold tabular-nums">{{ avgRecordText(kpis.avgWins, kpis.avgLosses) }}</span>
        <span class="text-sm text-muted-foreground">per completed run</span>
      </CardContent>
    </Card>

    <Card class="gap-0 py-0">
      <CardContent class="flex flex-col gap-0.5 p-3">
        <span class="inline-flex items-center gap-1 text-xs tracking-wide text-muted-foreground uppercase">
          <Layers class="size-3" /> Most drafted
        </span>
        <span class="text-3xl font-bold tabular-nums">{{ kpis.mostDraftedSet ?? NO_VALUE }}</span>
        <span class="text-sm text-muted-foreground">{{ kpis.mostDraftedCount }} runs</span>
      </CardContent>
    </Card>

    <Card class="gap-0 py-0">
      <CardContent class="flex flex-col gap-0.5 p-3">
        <span class="inline-flex items-center gap-1 text-xs tracking-wide text-muted-foreground uppercase">
          <Timer class="size-3" /> Avg pick time
        </span>
        <span class="text-3xl font-bold tabular-nums">{{ formatSeconds(kpis.avgPickSeconds) }}</span>
        <span class="text-sm text-muted-foreground">shown to committed</span>
      </CardContent>
    </Card>

    <Card class="gap-0 py-0">
      <CardContent class="flex flex-col gap-0.5 p-3">
        <span class="inline-flex items-center gap-1 text-xs tracking-wide text-muted-foreground uppercase">
          <Clock class="size-3" /> Indecision
        </span>
        <span class="text-3xl font-bold tabular-nums">{{ percentText(kpis.indecisionPct) }}</span>
        <span class="text-sm text-muted-foreground">picks with 2+ reservations</span>
      </CardContent>
    </Card>
  </div>
</template>
