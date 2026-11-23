<script setup lang="ts">
import { computed } from 'vue'
import { RouterLink } from 'vue-router'
import type { RecentEntryView } from '@/bindings'
import CardImage from '@/components/CardImage.vue'
import HomeCard from '@/components/dashboard/HomeCard.vue'
import Sparkline from '@/components/decks/Sparkline.vue'
import ManaColors from '@/components/mana/ManaColors.vue'
import { Badge } from '@/components/ui/badge'
import { deckCover, lastPlayedText } from '@/lib/decks'
import { limitedSetTitle, recentLink, recentTitle } from '@/lib/homeDashboard'
import { NO_VALUE } from '@/lib/leagues'

/**
 * Recent decks, Recent drafts, or both in All mode (dashboard redesign spec
 * 4.7): the two played last, whatever the timeframe, with all-time boxes.
 */
const props = defineProps<{ entries: RecentEntryView[]; format: string; now?: number }>()

const nowMs = computed(() => props.now ?? Date.now())
</script>

<template>
  <HomeCard :title="recentTitle(format)" :to="format === 'limited' ? '/limited' : '/decks'" link-label="Open the list">
    <ul class="flex flex-col gap-4">
      <li v-for="entry in entries" :key="entry.type === 'deck' ? `deck-${entry.id}` : `draft-${entry.key}`">
        <RouterLink :to="recentLink(entry)" class="flex flex-col gap-3 rounded-md p-1 hover:bg-muted/30">
          <div class="flex items-center gap-3">
            <CardImage
              v-if="deckCover(entry).hasCover"
              :src="entry.coverArt"
              :local="entry.localCoverArt"
              :custom="deckCover(entry).custom"
              alt=""
              class="size-10 shrink-0 rounded-md object-cover"
            />
            <div class="flex min-w-0 flex-1 flex-col">
              <template v-if="entry.type === 'deck'">
                <span class="flex min-w-0 items-center gap-2">
                  <span class="truncate text-sm font-medium">{{ entry.name }}</span>
                  <Badge variant="outline">{{ entry.formatLabel }}</Badge>
                </span>
                <span class="text-xs text-muted-foreground">{{ lastPlayedText(entry.lastPlayedAtMs, nowMs) }}</span>
              </template>
              <template v-else>
                <span class="flex min-w-0 items-center gap-2">
                  <span class="truncate text-sm font-medium">{{ limitedSetTitle(entry.setName, entry.setCode) }}</span>
                  <ManaColors v-if="entry.colors" :colors="entry.colors" size="xs" />
                </span>
                <span class="text-xs text-muted-foreground">{{ lastPlayedText(entry.startedAtMs, nowMs) }}</span>
              </template>
            </div>
          </div>
          <div class="grid grid-cols-3 gap-2 text-center">
            <template v-if="entry.type === 'deck'">
              <div class="rounded-md bg-muted/30 px-2 py-1.5">
                <div class="text-sm font-semibold tabular-nums">{{ entry.record.winRate }}%</div>
                <div class="text-[10px] text-muted-foreground">Matches</div>
              </div>
              <div class="rounded-md bg-muted/30 px-2 py-1.5">
                <div class="text-sm font-semibold tabular-nums">{{ entry.games.winRate }}%</div>
                <div class="text-[10px] text-muted-foreground">Games</div>
              </div>
              <div class="rounded-md bg-muted/30 px-2 py-1.5">
                <div class="text-sm font-semibold tabular-nums">{{ entry.record.wins }}-{{ entry.record.losses }}</div>
                <div class="text-[10px] text-muted-foreground">Record</div>
              </div>
            </template>
            <template v-else>
              <div class="rounded-md bg-muted/30 px-2 py-1.5">
                <div class="text-sm font-semibold tabular-nums">{{ entry.wins }}-{{ entry.losses }}</div>
                <div class="text-[10px] text-muted-foreground">Record</div>
              </div>
              <div class="rounded-md bg-muted/30 px-2 py-1.5">
                <div class="text-sm font-semibold tabular-nums">{{ entry.gameWins }}-{{ entry.gameLosses }}</div>
                <div class="text-[10px] text-muted-foreground">Games</div>
              </div>
              <div class="min-w-0 rounded-md bg-muted/30 px-2 py-1.5">
                <div class="truncate text-sm font-semibold">{{ entry.firstPick ?? NO_VALUE }}</div>
                <div class="text-[10px] text-muted-foreground">First pick</div>
              </div>
            </template>
          </div>
          <Sparkline v-if="entry.type === 'deck'" :series="entry.sparkline" :positive="entry.record.winRate >= 50" />
        </RouterLink>
      </li>
    </ul>
  </HomeCard>
</template>
