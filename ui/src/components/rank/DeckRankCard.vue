<script setup lang="ts">
import { computed } from 'vue'
import type { DeckRankView } from '@/bindings'
import { Card, CardContent } from '@/components/ui/card'
import { climbSentence } from '@/lib/rank'
import { rankIconUrl } from '@/lib/rankIcons'
import RankChart from './RankChart.vue'

/**
 * An Arena deck's dashboard card: how far the deck moved the constructed rank, over the season's line.
 * `rank` is null when the account has no constructed reading this season; `error` is a failed read.
 */
const props = defineProps<{ rank: DeckRankView | null; error?: string | null }>()

const currentIcon = computed(() => {
  const points = props.rank?.line.points ?? []
  const last = points[points.length - 1]
  return last && props.rank ? rankIconUrl(props.rank.line.ladder, last.steps) : ''
})
const sentence = computed(() => (props.rank ? climbSentence(props.rank.climb) : ''))
</script>

<template>
  <Card class="gap-0 py-0">
    <CardContent class="flex flex-col gap-3 p-4">
      <div class="flex items-baseline justify-between gap-2">
        <h3 class="text-sm font-medium">Constructed rank</h3>
        <span v-if="rank?.line.current" class="flex items-center gap-1.5 text-xs font-medium">
          <img v-if="currentIcon" :src="currentIcon" alt="" class="h-6 w-auto" />
          {{ rank.line.current }}
        </span>
      </div>
      <p v-if="error" class="text-sm text-destructive">Could not load the rank: {{ error }}</p>
      <template v-else-if="rank">
        <p class="text-sm text-muted-foreground">{{ sentence }}</p>
        <RankChart v-if="rank.line.points.length > 0" :line="rank.line" highlight-deck :height="280" />
      </template>
      <p v-else class="py-6 text-center text-sm text-muted-foreground">No rank reading this season yet.</p>
    </CardContent>
  </Card>
</template>
