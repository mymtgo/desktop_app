<script setup lang="ts">
import { RouterLink } from 'vue-router'
import type { TopMatchupRowView } from '@/bindings'
import HomeCard from '@/components/dashboard/HomeCard.vue'
import MatchRecord from '@/components/decks/MatchRecord.vue'
import WinRateBar from '@/components/decks/WinRateBar.vue'
import { shareText } from '@/lib/homeDashboard'

/**
 * Top matchups (dashboard redesign spec 4.3): the five opponent archetypes
 * met most in the window; the bar is the win rate, the share is text.
 */
defineProps<{ matchups: TopMatchupRowView[] }>()
</script>

<template>
  <HomeCard title="Top matchups">
    <ul class="flex flex-col">
      <li v-for="row in matchups" :key="row.archetypeId">
        <RouterLink
          :to="`/metagame/archetypes/${row.archetypeId}`"
          class="grid grid-cols-[minmax(0,1fr)_6rem_10rem] items-center gap-3 rounded-sm px-2 py-1.5 text-sm hover:bg-muted/40"
        >
          <span class="flex min-w-0 flex-col">
            <span class="truncate font-medium">{{ row.name }}</span>
            <span class="share-text truncate text-xs text-muted-foreground">
              {{ shareText(row.record.total, 'matches', row.share, 'matches') }}
            </span>
          </span>
          <MatchRecord :record="row.record" />
          <WinRateBar :winrate="row.record.winRate" size="sm" />
        </RouterLink>
      </li>
    </ul>
  </HomeCard>
</template>
