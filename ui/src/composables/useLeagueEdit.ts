import { useQueryClient } from '@tanstack/vue-query'
import { ref } from 'vue'
import { useToast } from '@/composables/useToast'
import { errorMessage, unwrap, type CommandResult } from '@/lib/commands'
import { keys } from '@/lib/keys'


export type EditOutcome = { ok: true } | { ok: false; message: string }

/**
 * Runs one league edit (create, link, unlink, notes, drop). On success every
 * league, deck and Limited query is refetched straight away: `LeaguesChanged` only
 * follows on the tracker's next tick plus its debounce. `saving` is true
 * until the refetch is done.
 */
export function useLeagueEdit() {
  const queryClient = useQueryClient()
  const { add } = useToast()
  const saving = ref(false)

  /** The edit's outcome; a refusal comes back for the caller to show. */
  async function attempt(action: () => Promise<CommandResult<null>>): Promise<EditOutcome> {
    saving.value = true
    try {
      unwrap(await action())
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: keys.allLeagues }),
        queryClient.invalidateQueries({ queryKey: keys.allDecks }),
        queryClient.invalidateQueries({ queryKey: keys.allLimited }),
      ])
      return { ok: true }
    } catch (error) {
      return { ok: false, message: errorMessage(error) }
    } finally {
      saving.value = false
    }
  }

  /** The edit, a failure or refusal shown as a toast titled `title`. */
  async function run(title: string, action: () => Promise<CommandResult<null>>): Promise<boolean> {
    const outcome = await attempt(action)
    if (!outcome.ok) add({ type: 'error', title, message: outcome.message })
    return outcome.ok
  }

  return { saving, attempt, run }
}
