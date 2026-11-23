<script setup lang="ts">
import type { LastSessionView } from '@/bindings'
import HomeCard from '@/components/dashboard/HomeCard.vue'
import { markClass, sessionMark } from '@/lib/homeDashboard'

/**
 * Last session (dashboard redesign spec 4.9): the latest matches with less
 * than 2 hours between them, in the format, whatever the timeframe. An
 * unknown outcome is a neutral ?, a draw D.
 */
defineProps<{ session: LastSessionView }>()
</script>

<template>
  <HomeCard title="Last session">
    <template #aside>
      <span class="truncate">{{ session.startedLabel }} · {{ session.duration }}</span>
    </template>
    <div class="flex flex-col gap-3">
      <span class="text-2xl font-semibold tabular-nums">{{ session.record }}</span>
      <ul class="flex flex-col gap-1">
        <li v-for="match in session.matches" :key="match.token" class="flex items-center gap-2 text-sm">
          <span
            class="session-mark flex size-6 shrink-0 items-center justify-center rounded-sm text-xs font-semibold"
            :class="markClass(sessionMark(match.outcome).tone)"
          >
            {{ sessionMark(match.outcome).text }}
          </span>
          <span class="flex-1 truncate">{{ match.opponentArchetype }}</span>
          <span class="text-xs text-muted-foreground tabular-nums">{{ match.gamesWon }}-{{ match.gamesLost }}</span>
        </li>
      </ul>
    </div>
  </HomeCard>
</template>
