import { computed, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { deckVersionParam } from '@/lib/deckRoute'
import { deckFilters } from '@/lib/decks'
import { useDeckArchetypeShell, useSettings } from '@/lib/queries'
import { normalizeTimeframe } from '@/lib/timeframes'

/**
 * What an archetype tab of the deck index reads (0.x
 * `ArchetypeTabController`): the archetype from the route, the remembered
 * format, the timeframe from `?timeframe=`, and the shell (sidebar and
 * header). Opening a tab remembers the archetype as the chosen row; a
 * fallback (no tabs) or an archetype without decks in scope goes back to
 * the deck grid.
 */
export function useArchetypeTab() {
  const route = useRoute()
  const router = useRouter()
  const { data: settings } = useSettings()

  const archetypeId = computed(() => deckVersionParam(route.params.archetypeId) ?? 0)
  const format = computed(() => deckFilters.format)
  const timeframe = computed(() => normalizeTimeframe(route.query.timeframe))
  const { data: shell, error } = useDeckArchetypeShell(archetypeId, format, timeframe)

  watch(
    archetypeId,
    (id) => {
      if (id > 0) deckFilters.archetype = String(id)
    },
    { immediate: true },
  )

  watch(shell, (found) => {
    if (!found) return
    const header = found.archetypeHeader
    if (header === null) {
      deckFilters.archetype = null
      void router.replace({ name: 'decks' })
    } else if (header.archetype?.isFallback) {
      void router.replace({ name: 'decks' })
    }
  })

  function setTimeframe(next: string) {
    void router.push({ query: next === 'alltime' ? {} : { timeframe: next } })
  }

  return { archetypeId, format, timeframe, settings, shell, error, setTimeframe }
}
