import { useQueryClient } from '@tanstack/vue-query'
import { ref } from 'vue'
import { commands, type DeleteLeagueView, type RunStateView } from '@/bindings'
import { useToast } from '@/composables/useToast'
import { errorMessage, unwrap, type CommandResult } from '@/lib/commands'
import { keys } from '@/lib/keys'

/** How long a toast with Undo stays up, in ms. */
export const UNDO_TOAST_MS = 10_000

type Outcome<T> = { ok: true; data: T } | { ok: false }

/**
 * The player's league edits (league self-management spec 4.3 to 4.5):
 * the status, delete with its Undo toast, and restore. Each refetches
 * every league, deck, match, dashboard and Limited query straight away (a
 * delete or restore moves matches in and out of every list); the tracker's
 * pushes follow later. A refusal ("Drop it first") shows as an error toast.
 */
export function useLeagueManage() {
  const queryClient = useQueryClient()
  const { add } = useToast()
  const saving = ref(false)

  async function refetch() {
    await Promise.all([
      queryClient.invalidateQueries({ queryKey: keys.allLeagues }),
      queryClient.invalidateQueries({ queryKey: keys.allDecks }),
      queryClient.invalidateQueries({ queryKey: keys.allMatches }),
      queryClient.invalidateQueries({ queryKey: keys.allDashboard }),
      queryClient.invalidateQueries({ queryKey: keys.allLimited }),
    ])
  }

  async function attempt<T>(title: string, action: () => Promise<CommandResult<T>>): Promise<Outcome<T>> {
    saving.value = true
    try {
      const data = unwrap(await action())
      await refetch()
      return { ok: true, data }
    } catch (error) {
      add({ type: 'error', title, message: errorMessage(error) })
      return { ok: false }
    } finally {
      saving.value = false
    }
  }

  /** `null` is Automatic. */
  async function setStatus(runKey: string, status: RunStateView | null): Promise<boolean> {
    return (await attempt('Could not set the status', () => commands.setLeagueStatus(runKey, status))).ok
  }

  /**
   * Undo a delete: the league (and the matches it hid) back, then the status the delete replaced.
   * Undo names the league by its row id, which a rekey since the delete keeps, and answers the run
   * key it has now, which the status goes back under (sync identity spec 2.5).
   */
  async function undoDelete(deleted: DeleteLeagueView): Promise<boolean> {
    const undone = await attempt('Could not undo the delete', () =>
      commands.undoDeleteLeague(deleted.leagueId, deleted.removedAt),
    )
    if (!undone.ok) return false
    const runKey = undone.data
    const previous = deleted.previousStatus
    if (previous === null) return true
    return (await attempt('Could not put the status back', () => commands.setLeagueStatus(runKey, previous))).ok
  }

  async function deleteLeague(runKey: string, matches: 'keep' | 'delete'): Promise<boolean> {
    const outcome = await attempt('Could not delete the league', () => commands.deleteLeague(runKey, matches))
    if (!outcome.ok) return false
    const deleted = outcome.data
    add({
      type: 'success',
      title: 'League deleted',
      message: 'You can restore it from Settings, Archives.',
      duration: UNDO_TOAST_MS,
      action: { label: 'Undo', run: () => void undoDelete(deleted) },
    })
    return true
  }

  /** Restore a deleted league: its kept matches and those deleted with it come back too (spec 4.5). */
  async function restoreLeague(runKey: string): Promise<boolean> {
    const outcome = await attempt('Could not restore the league', () => commands.restoreLeague(runKey))
    if (outcome.ok) add({ type: 'success', title: 'League restored', message: 'Matches deleted with it are back too.' })
    return outcome.ok
  }

  return { saving, setStatus, deleteLeague, undoDelete, restoreLeague }
}
