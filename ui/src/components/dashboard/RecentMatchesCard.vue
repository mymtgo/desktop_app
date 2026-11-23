<script setup lang="ts">
import { computed } from 'vue'
import { RouterLink } from 'vue-router'
import type { HomeRecentMatchView } from '@/bindings'
import ArchetypeName from '@/components/archetypes/ArchetypeName.vue'
import HomeCard from '@/components/dashboard/HomeCard.vue'
import ManaColors from '@/components/mana/ManaColors.vue'
import { lastPlayedText } from '@/lib/decks'
import { markClass, recentMatchEvent, sessionMark } from '@/lib/homeDashboard'

/**
 * Recent matches (dashboard redesign spec 4.5): the latest five in the
 * window and format; a limited match shows its set and colours.
 */
const props = defineProps<{ matches: HomeRecentMatchView[]; now?: number }>()

const nowMs = computed(() => props.now ?? Date.now())
</script>

<template>
  <HomeCard title="Recent matches" to="/matches" link-label="Open matches">
    <ul class="flex flex-col">
      <li v-for="row in matches" :key="row.match.token">
        <RouterLink
          :to="`/matches/${row.match.token}`"
          class="grid grid-cols-[1.5rem_minmax(0,1fr)_minmax(0,1fr)_minmax(0,7rem)_5rem] items-center gap-3 rounded-sm px-2 py-1.5 text-sm hover:bg-muted/40"
        >
          <span
            class="flex size-6 items-center justify-center rounded-sm text-xs font-semibold"
            :class="markClass(sessionMark(row.match.outcome).tone)"
          >
            {{ sessionMark(row.match.outcome).text }}
          </span>
          <span class="flex min-w-0 items-center gap-2">
            <template v-if="row.runKey">
              <span class="truncate">{{ row.setCode ?? 'Limited' }}</span>
              <ManaColors v-if="row.colors" :colors="row.colors" size="xs" />
            </template>
            <span v-else class="truncate">{{ row.deckName ?? 'No deck' }}</span>
          </span>
          <span class="truncate text-muted-foreground">
            <ArchetypeName v-if="row.match.opponentArchetype" :archetype="row.match.opponentArchetype" />
            <template v-else>Unknown</template>
          </span>
          <span class="truncate text-xs text-muted-foreground">{{ recentMatchEvent(row) }}</span>
          <span class="text-right text-xs text-muted-foreground">{{ lastPlayedText(row.match.startedAtMs, nowMs) }}</span>
        </RouterLink>
      </li>
    </ul>
  </HomeCard>
</template>
