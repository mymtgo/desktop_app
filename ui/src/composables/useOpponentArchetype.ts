import { useQuery, useQueryClient } from '@tanstack/vue-query'
import { computed, ref, type Ref } from 'vue'
import { commands } from '@/bindings'
import { useToast } from '@/composables/useToast'
import { errorMessage, unwrap } from '@/lib/commands'
import { keys } from '@/lib/keys'

/** The match pages' queries (`matchDetailKeys.all`). */
const MATCH_DETAIL = ['matchDetail'] as const

/** The picker's options of a format (a match's format code or label; null for every archetype). */
export function archetypeOptionsKey(format: string | null) {
  return ['archetypeOptions', format] as const
}

/**
 * The archetypes a picker offers for `format`, read while `enabled` (a
 * dialog only asks once it opens).
 */
export function useArchetypeOptions(format: Ref<string | null>, enabled: Ref<boolean>) {
  return useQuery({
    queryKey: computed(() => archetypeOptionsKey(format.value)),
    queryFn: async () => unwrap(await commands.archetypeOptions(format.value)),
    enabled,
  })
}

/** The server's archetypes of a format v1 has not met (#489); null for a bulk set over formats. */
export function archetypeCatalogKey(format: string | null) {
  return ['archetypeCatalog', format] as const
}

/**
 * The server's archetypes of `format` that v1 does not hold, which the
 * picker lists with the held ones (#489). Empty offline; never asked
 * without a format.
 */
export function useArchetypeCatalog(format: Ref<string | null>) {
  return useQuery({
    queryKey: computed(() => archetypeCatalogKey(format.value)),
    queryFn: async () => unwrap(await commands.archetypeCatalog(format.value)),
    enabled: computed(() => format.value !== null),
  })
}

/**
 * Set or clear the opponent archetype of matches (0.x
 * `UpdateArchetypeController`, `BulkUpdateArchetypeController`). Every
 * list, deck, league, dashboard and match page query is refetched once the
 * tracker has committed; a refusal or failure shows as a toast.
 */
export function useSetOpponentArchetype() {
  const queryClient = useQueryClient()
  const { add } = useToast()
  const saving = ref(false)

  async function setArchetype(tokens: string[], archetypeId: number | null): Promise<boolean> {
    saving.value = true
    try {
      unwrap(await commands.setOpponentArchetype(tokens, archetypeId))
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: keys.allMatches }),
        queryClient.invalidateQueries({ queryKey: keys.allDecks }),
        queryClient.invalidateQueries({ queryKey: keys.allLeagues }),
        queryClient.invalidateQueries({ queryKey: keys.allDashboard }),
        queryClient.invalidateQueries({ queryKey: MATCH_DETAIL }),
      ])
      return true
    } catch (error) {
      add({ type: 'error', title: 'Could not set the archetype', message: errorMessage(error) })
      return false
    } finally {
      saving.value = false
    }
  }

  return { saving, setArchetype }
}
