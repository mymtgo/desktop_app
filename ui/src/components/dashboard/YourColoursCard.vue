<script setup lang="ts">
import type { ColourShareView } from '@/bindings'
import HomeCard from '@/components/dashboard/HomeCard.vue'
import MatchRecord from '@/components/decks/MatchRecord.vue'
import WinRateBar from '@/components/decks/WinRateBar.vue'
import ManaColors from '@/components/mana/ManaColors.vue'
import { shareText } from '@/lib/homeDashboard'

/**
 * Your colours (dashboard redesign spec 4.4): the colours you drafted; the
 * bar is the win rate, the share of drafts is text.
 */
defineProps<{ colours: ColourShareView[] }>()
</script>

<template>
  <HomeCard title="Your colours">
    <ul class="flex flex-col">
      <li
        v-for="row in colours"
        :key="row.colors.join('')"
        class="grid grid-cols-[minmax(0,1fr)_6rem_10rem] items-center gap-3 px-2 py-1.5 text-sm"
      >
        <span class="flex min-w-0 items-center gap-2">
          <ManaColors :colors="row.colors" size="sm" />
          <span class="flex min-w-0 flex-col">
            <span class="truncate font-medium">{{ row.name }}</span>
            <span class="share-text truncate text-xs text-muted-foreground">
              {{ shareText(row.runs, 'runs', row.share, 'drafts') }}
            </span>
          </span>
        </span>
        <MatchRecord :record="row.record" />
        <WinRateBar :winrate="row.record.winRate" size="sm" />
      </li>
    </ul>
  </HomeCard>
</template>
