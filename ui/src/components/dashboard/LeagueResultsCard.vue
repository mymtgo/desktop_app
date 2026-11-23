<script setup lang="ts">
import { computed } from 'vue'
import type { LeagueRunsView } from '@/bindings'
import HomeCard from '@/components/dashboard/HomeCard.vue'
import MeterBar from '@/components/decks/MeterBar.vue'
import {
  ladderBarClass,
  ladderMax,
  leagueLadders,
  leagueRunsHeader,
} from '@/lib/homeDashboard'

/**
 * League results (dashboard redesign spec 4.2): how the runs started in the
 * window finished, one ladder per kind (every finish, even with none).
 */
const props = defineProps<{ view: LeagueRunsView; limited: boolean }>()

const header = computed(() => leagueRunsHeader(props.view))
const ladders = computed(() => leagueLadders(props.view.runs))
function fill(count: number, max: number): number {
  return (count / max) * 100
}
</script>

<template>
  <HomeCard title="League results" :to="limited ? '/limited' : '/leagues'" link-label="Open leagues">
    <template #aside>
      <span class="truncate tabular-nums">{{ header }}</span>
    </template>
    <div class="flex flex-col gap-4 py-1.5">
      <div v-for="ladder in ladders" :key="ladder.kind" class="flex flex-col gap-1.5">
        <p v-if="ladder.label" class="ladder-heading text-xs text-muted-foreground">{{ ladder.label }}</p>
        <div
          v-for="row in ladder.rows"
          :key="row.label"
          class="ladder-row grid grid-cols-[4.5rem_minmax(0,1fr)_2rem] items-center gap-3 text-xs"
        >
          <span class="ladder-label tabular-nums" :class="row.count === 0 ? 'text-muted-foreground' : ''">
            {{ row.label }}
          </span>
          <MeterBar :value="fill(row.count, ladderMax(ladder))" :fill-class="ladderBarClass(row.tone)" />
          <span class="ladder-count text-right tabular-nums" :class="row.count === 0 ? 'text-muted-foreground' : ''">
            {{ row.count }}
          </span>
        </div>
      </div>
    </div>
  </HomeCard>
</template>
