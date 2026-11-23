<script setup lang="ts">
import type { RunCardView } from '@/bindings'
import LeaguesPageCard from '@/components/leagues/LeagueCard.vue'

/**
 * League in progress, else the latest league (dashboard redesign spec 4.6):
 * the Leagues page's run row for either kind, compact (the header alone).
 */
defineProps<{
  league: RunCardView
  /** "Now" for the run's relative start (the data's fetch time). */
  now?: number
}>()
</script>

<template>
  <LeaguesPageCard :league="league" :now="now" compact />
</template>
