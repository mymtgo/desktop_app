<script setup lang="ts">
import { useQuery } from '@tanstack/vue-query'
import { AlertTriangle } from 'lucide-vue-next'
import { computed } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { commands } from '@/bindings'
import DeckSidebar from '@/components/decks/DeckSidebar.vue'
import MatchDetailState from '@/components/matches/MatchDetailState.vue'
import { Skeleton } from '@/components/ui/skeleton'
import { useArchetypeDetection } from '@/composables/useArchetypeDetection'
import { useLiveMatchDetail } from '@/composables/useLiveMatchDetail'
import { unwrap } from '@/lib/commands'
import { deckLinkQuery } from '@/lib/deckRoute'
import { keys } from '@/lib/keys'
import { matchesFallbackPath } from '@/lib/matchDetail'

/**
 * One match (0.x `matches/Show.vue` and `MatchDetail.vue`), opened from the
 * Matches page or anywhere outside the Decks area. A match played with a
 * deck sits beside that deck's sidebar, as 0.x's deck layout did; otherwise
 * the page stands alone. A match opened from a deck's pages has its own
 * route inside the deck layout (`deck-match`, issue #332).
 */
const route = useRoute()
const router = useRouter()

const token = computed(() => String(route.params.token ?? ''))
const { detectingTokens, detectMatches } = useArchetypeDetection()

const { data: match, error, isPending } = useLiveMatchDetail(token)

// The deck's sidebar, under the deck layout's own key (no timeframe: 0.x's
// default window), so it shares that cache.
const deckId = computed(() => match.value?.deck?.id ?? null)
const { data: deckHeader, isPending: deckHeaderPending } = useQuery({
  queryKey: computed(() => keys.deckHeader(deckId.value ?? 0, null, null)),
  queryFn: async ({ queryKey }) => unwrap(await commands.deckHeader(queryKey[2], queryKey[3], null)),
  enabled: computed(() => deckId.value !== null),
})
const deckSidebarLoading = computed(() => deckId.value !== null && deckHeaderPending.value)

/** The sidebar's version select: show that version's matches. */
function openDeckMatches(version: number | null) {
  const id = deckId.value
  if (id === null) return
  void router.push({
    name: 'deck-matches',
    params: { id },
    query: deckLinkQuery({ version, timeframe: 'alltime' }, true),
  })
}
</script>

<template>
  <div class="flex min-h-0 flex-1 flex-col">
    <div
      v-if="deckHeader?.deck.archived"
      class="flex items-center justify-center gap-2 border-b border-amber-500/30 bg-amber-500/10 px-4 py-1.5 text-xs text-amber-200"
    >
      <AlertTriangle class="size-3.5 shrink-0 text-amber-400" />
      <span>This deck has been archived. Historical match data is preserved.</span>
    </div>
    <div class="flex min-h-0 flex-1">
      <div v-if="deckHeader" class="w-56 shrink-0">
        <DeckSidebar
          :header="deckHeader"
          :version="null"
          timeframe="alltime"
          current-page="deck-matches"
          @update:version="openDeckMatches"
        />
      </div>
      <div v-else-if="deckSidebarLoading" class="w-56 shrink-0 border-r border-black/80 p-3">
        <Skeleton class="h-24 w-full" />
      </div>

      <div class="flex min-h-0 flex-1 flex-col overflow-y-auto" :class="deckHeader ? 'border-l border-white/5' : ''">
        <MatchDetailState
          :match="match"
          :error="error"
          :pending="isPending"
          :fallback-path="matchesFallbackPath(match?.deck ?? null)"
          detectable
          :detecting="detectingTokens.includes(token)"
          @detect="detectMatches([token])"
        />
      </div>
    </div>
  </div>
</template>
