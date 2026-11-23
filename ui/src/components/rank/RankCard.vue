<script setup lang="ts">
import { computed } from 'vue'
import type { RankLineView } from '@/bindings'
import HomeCard from '@/components/dashboard/HomeCard.vue'
import { rankIconUrl } from '@/lib/rankIcons'
import RankChart from './RankChart.vue'

/** The home dashboard's rank card on Arena: one ladder's current rank and its season. */
const props = defineProps<{ line: RankLineView }>()

const currentIcon = computed(() => {
  const last = props.line.points[props.line.points.length - 1]
  return last ? rankIconUrl(props.line.ladder, last.steps) : ''
})
const title = computed(() => (props.line.ladder === 'limited' ? 'Limited rank' : 'Constructed rank'))
</script>

<template>
  <HomeCard :title="title">
    <template #aside>
      <span v-if="line.current" class="flex items-center gap-1.5 font-medium text-foreground">
        <img v-if="currentIcon" :src="currentIcon" alt="" class="h-7 w-auto" />
        {{ line.current }}
      </span>
    </template>
    <RankChart v-if="line.points.length > 0" :line="line" :height="280" />
    <p v-else class="py-6 text-center text-sm text-muted-foreground">No rank reading this season yet.</p>
  </HomeCard>
</template>
