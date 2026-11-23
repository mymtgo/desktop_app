<script setup lang="ts">
import { computed } from 'vue'
import { useRoute } from 'vue-router'
import MatchDetailState from '@/components/matches/MatchDetailState.vue'
import { useDetailRefresh } from '@/composables/useDetailRefresh'
import { keys } from '@/lib/keys'
import { limitedPath } from '@/lib/limited'
import { useLimitedMatch } from '@/lib/queries'
import { useLimitedKey } from '@/pages/limited/useLimitedKey'

/**
 * One of the run's matches inside the event's pages (0.x
 * `pages/limited/Match.vue`): the match page's detail, with "Back to
 * matches" falling back to the event's Matches page. A match the run does
 * not hold is not shown (0.x's 404).
 */
const route = useRoute()
const key = useLimitedKey()
const token = computed(() => String(route.params.token ?? ''))

const { data: match, error, isPending } = useLimitedMatch(key, token)
// Refreshed as the match page is.
useDetailRefresh(token, () => keys.limitedMatch(key.value, token.value))

const fallbackPath = computed(() => limitedPath(key.value, 'matches'))
</script>

<template>
  <MatchDetailState :match="match" :error="error" :pending="isPending" :fallback-path="fallbackPath" />
</template>
