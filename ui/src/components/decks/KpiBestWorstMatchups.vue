<script setup lang="ts">
import { computed } from 'vue'
import type { MatchupView } from '@/bindings'
import KpiBar from '@/components/decks/KpiBar.vue'
import KpiCard from '@/components/decks/KpiCard.vue'
import ManaColors from '@/components/mana/ManaColors.vue'
import { MIN_MATCHES, rankMatchups } from '@/lib/matchups'

/**
 * The deck dashboard's "Matchup spread" card (0.x `KpiBestWorstMatchups`):
 * the best, the median and the worst archetype faced 3 times or more.
 */
const props = defineProps<{ spread: MatchupView[] }>()

const ranked = computed(() => rankMatchups(props.spread))
const bars = computed(() => {
  const { best, middle, worst } = ranked.value
  const shown: { matchup: MatchupView; tone: 'success' | 'muted' | 'destructive' }[] = []
  if (best) shown.push({ matchup: best, tone: 'success' })
  if (middle) shown.push({ matchup: middle, tone: 'muted' })
  if (worst) shown.push({ matchup: worst, tone: 'destructive' })
  return shown
})
</script>

<template>
  <KpiCard label="Matchup spread" :scope="ranked.best ? `${MIN_MATCHES}+ matches` : null">
    <div v-if="ranked.best" class="flex flex-col gap-3">
      <KpiBar
        v-for="bar in bars"
        :key="bar.matchup.archetypeId"
        :rate="bar.matchup.matchWinRate"
        :record="bar.matchup.matchRecord"
        :tone="bar.tone"
      >
        <template #label>
          <span class="flex min-w-0 items-center gap-1.5">
            <ManaColors :colors="bar.matchup.colors" class="shrink-0" />
            <span class="truncate">{{ bar.matchup.name }}</span>
          </span>
        </template>
      </KpiBar>

      <p v-if="!ranked.worst" class="text-xs text-muted-foreground">
        Only one archetype has {{ MIN_MATCHES }} or more matches so far.
      </p>
    </div>

    <p v-else class="py-4 text-sm text-muted-foreground">
      No archetype has {{ MIN_MATCHES }} matches yet, which is the fewest that says anything.
    </p>
  </KpiCard>
</template>
