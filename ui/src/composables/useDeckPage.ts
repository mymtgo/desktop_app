import { computed, type Ref } from 'vue'
import type { BestOfView } from '@/bindings'
import { useRoute, useRouter } from 'vue-router'
import { deckLinkQuery, deckVersionParam } from '@/lib/deckRoute'
import { useDeckHeader } from '@/lib/queries'
import { normalizeTimeframe } from '@/lib/timeframes'

/**
 * What every deck page reads from the URL: the deck id, the selected version
 * (`?version=`, null for all versions) and the timeframe (`?timeframe=`, all
 * time when absent). Routes whose meta has `deckTimeframe: true` (Dashboard,
 * Matches, Leagues, Tournaments) use the timeframe; the others ignore it and leave it out of links.
 */
export function useDeckPage() {
  const route = useRoute()
  const router = useRouter()

  /** 0 for an id that does not parse: the header then reads "no such deck". */
  const deckId = computed(() => deckVersionParam(route.params.id) ?? 0)
  const version = computed(() => deckVersionParam(route.query.version))
  const timeframe = computed(() => normalizeTimeframe(route.query.timeframe))
  const usesTimeframe = computed(() => route.meta.deckTimeframe === true)

  function navigate(next: { version: number | null; timeframe: string }) {
    void router.push({ query: deckLinkQuery(next, usesTimeframe.value) })
  }

  function setVersion(next: number | null) {
    navigate({ version: next, timeframe: timeframe.value })
  }

  function setTimeframe(next: string) {
    navigate({ version: version.value, timeframe: next })
  }

  return { deckId, version, timeframe, usesTimeframe, setVersion, setTimeframe }
}

/**
 * The deck's sidebar data, as the layout loads it (same query key, so a page
 * asking for it shares the layout's fetch). `null` data: no such deck.
 */
export function useCurrentDeckHeader(bestOf: Ref<BestOfView | null>) {
  const { deckId, timeframe, usesTimeframe } = useDeckPage()
  // 0.x gave the sidebar the page's window on Dashboard, Matches, Leagues and
  // Tournaments, and its
  // default (the last two months) elsewhere.
  const headerTimeframe = computed(() => (usesTimeframe.value ? timeframe.value : null))
  return useDeckHeader(deckId, headerTimeframe, bestOf)
}
