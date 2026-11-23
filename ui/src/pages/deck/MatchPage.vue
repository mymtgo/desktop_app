<script setup lang="ts">
import { computed, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import MatchDetailState from '@/components/matches/MatchDetailState.vue'
import { useArchetypeDetection } from '@/composables/useArchetypeDetection'
import { useDeckPage } from '@/composables/useDeckPage'
import { useLiveMatchDetail } from '@/composables/useLiveMatchDetail'
import { deckLinkQuery, deckMatchesPath, matchPathInDecks } from '@/lib/deckRoute'

/**
 * One match opened from a deck's pages (issue #332): the match page's detail
 * inside the deck layout, so the deck sidebar stays (Matches highlighted) and
 * the top nav keeps Decks active. "Back to matches" steps back, or goes to
 * the deck's Matches page with the same version and timeframe. A match whose
 * deck is another one (its deck was just changed) moves to that deck; one
 * without a deck moves to the match page on its own.
 */
const route = useRoute()
const router = useRouter()
const { deckId, version, timeframe } = useDeckPage()

const token = computed(() => String(route.params.token ?? ''))
const { detectingTokens, detectMatches } = useArchetypeDetection()
const { data: match, error, isPending } = useLiveMatchDetail(token)

const fallbackPath = computed(() =>
  deckMatchesPath(deckId.value, deckLinkQuery({ version: version.value, timeframe: timeframe.value }, true)),
)

watch(match, (shown) => {
  if (!shown || shown.deck?.id === deckId.value) return
  void router.replace(matchPathInDecks(shown.deck?.id, shown.token))
})
</script>

<template>
  <MatchDetailState
    :match="match"
    :error="error"
    :pending="isPending"
    :fallback-path="fallbackPath"
    detectable
    :detecting="detectingTokens.includes(token)"
    @detect="detectMatches([token])"
  />
</template>
