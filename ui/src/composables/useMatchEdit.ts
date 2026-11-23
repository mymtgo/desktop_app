import { useQueryClient } from '@tanstack/vue-query'
import { ref } from 'vue'
import {
  commands,
  type LeagueRefView,
  type ManualEditView,
  type ManualHandInput,
  type ManualMatchInput,
  type ManualMatchSaveView,
  type ManualRevealInput,
  type ManualSideboardChangeInput,
  type MatchCorrectionInput,
  type MatchEditView,
} from '@/bindings'
import { UNDO_TOAST_MS } from '@/composables/useLeagueManage'
import { useToast } from '@/composables/useToast'
import { errorMessage, unwrap, type CommandResult } from '@/lib/commands'
import { keys } from '@/lib/keys'
import { movedFromText } from '@/lib/leagues'
import type { FieldErrors } from '@/lib/manualMatch'

/** The match pages' queries (`matchDetailKeys.all`, from the match page's module). */
const MATCH_DETAIL = ['matchDetail'] as const

/** Caches kept current by their own pushes (`StatusChanged`, `SettingsChanged`). */
const PUSHED_KEYS: string[] = [keys.status[0], keys.settings[0]]


/** A manual form's save: done, or refused with 0.x's messages by field (empty after a toast). */
export type ManualOutcome = { ok: true } | { ok: false; errors: FieldErrors }

/**
 * A match's notes, "Delete match" with Undo, restore and "Move to league..." (0.x `UpdateNotesController`,
 * `DeleteController`). Notes push no event, so a save refetches every match
 * list (the main list, each deck's list, the dashboard's recent matches, the
 * Limited pages) and the match pages. Hiding pushes `MatchRemoved` and
 * `DecksChanged`; the lists are refetched straight away too, as the deck push
 * follows on a later tick. A refusal or failure shows as a toast. `saving` is
 * true until the refetch is done.
 *
 * The manual match saves (0.x `StoreController` and the `Games/Update*`
 * controllers) refetch the match pages, every match list, every deck, league
 * and Limited query and the dashboard straight away (the tracker's pushes
 * follow on a later tick), and show 0.x's flash message as a success toast.
 * A form the rules refuse comes back with its field errors and refetches
 * nothing. "Set deck" (issue #332) refetches as a manual save does.
 *
 * "Edit match" (edit a match spec) fixes any match in place. Its result feeds
 * nearly every page (lists, decks, leagues, the dashboard, opponents,
 * matchups, card stats, the archetype, Metagame, Cards and Limited pages), so
 * a save refetches every query but the pushed status and settings. A log
 * match's fix pushes no `MatchUpdated`; the tracker's `DecksChanged` and
 * `LeaguesChanged` refresh the overlays.
 */
export function useMatchEdit() {
  const queryClient = useQueryClient()
  const { add } = useToast()
  const saving = ref(false)

  async function run(title: string, action: () => Promise<CommandResult<null>>): Promise<boolean> {
    saving.value = true
    try {
      unwrap(await action())
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: keys.allMatches }),
        queryClient.invalidateQueries({ queryKey: keys.allDeckMatches }),
        queryClient.invalidateQueries({ queryKey: keys.allDashboard }),
        queryClient.invalidateQueries({ queryKey: MATCH_DETAIL }),
        queryClient.invalidateQueries({ queryKey: keys.allLimited }),
      ])
      return true
    } catch (error) {
      add({ type: 'error', title, message: errorMessage(error) })
      return false
    } finally {
      saving.value = false
    }
  }

  function saveNotes(token: string, notes: string): Promise<boolean> {
    return run('Could not save notes', () => commands.setMatchNotes(token, notes))
  }

  /** "Delete match" (league self-management spec 5.4): a toast offers Undo. */
  async function hide(token: string): Promise<boolean> {
    const done = await run('Could not delete the match', () => commands.hideMatch(token))
    if (done) {
      add({
        type: 'success',
        title: 'Match deleted',
        message: 'You can restore it from Settings, Archives.',
        duration: UNDO_TOAST_MS,
        action: { label: 'Undo', run: () => void restore(token) },
      })
    }
    return done
  }

  /**
   * Restore a deleted match (Undo, Settings, Archives): back in every list,
   * deck and league. A match deleted with its league is refused while the
   * league stays deleted (issue #418): Restore the league instead. A match
   * deleted on its own whose league is deleted since comes back as a
   * non-league match (issue #416).
   */
  async function restore(token: string): Promise<boolean> {
    saving.value = true
    try {
      unwrap(await commands.restoreMatch(token))
      await refetchAfterManualSave()
      return true
    } catch (error) {
      add({ type: 'error', title: 'Could not restore the match', message: errorMessage(error) })
      return false
    } finally {
      saving.value = false
    }
  }

  /**
   * "Move to league..." (spec 5.4): into the league `runKey`, or out of
   * every league for `null`. The toast names where it came from (`from`).
   */
  async function moveToLeague(token: string, runKey: string | null, from: LeagueRefView | null): Promise<boolean> {
    saving.value = true
    try {
      unwrap(await commands.moveMatchToLeague(token, runKey))
      await refetchAfterManualSave()
      add({
        type: 'success',
        title: runKey === null ? 'Removed from its league' : 'Match moved',
        message: movedFromText(from),
      })
      return true
    } catch (error) {
      add({ type: 'error', title: 'Could not move the match', message: errorMessage(error) })
      return false
    } finally {
      saving.value = false
    }
  }

  /**
   * "Set deck" on the Matches page (issue #332): the matches were played
   * with deck version `versionId`. Refetches as a manual save does (the
   * match moves between decks, leagues and the dashboard's numbers).
   */
  async function setDeck(tokens: string[], versionId: number): Promise<boolean> {
    saving.value = true
    try {
      unwrap(await commands.setMatchDeck(tokens, versionId))
      await refetchAfterManualSave()
      const what = tokens.length === 1 ? 'The match was moved' : `${tokens.length} matches were moved`
      add({ type: 'success', title: 'Deck set', message: `${what} to the chosen deck.` })
      return true
    } catch (error) {
      add({ type: 'error', title: 'Could not set the deck', message: errorMessage(error) })
      return false
    } finally {
      saving.value = false
    }
  }

  async function refetchAfterManualSave() {
    await Promise.all([
      queryClient.invalidateQueries({ queryKey: keys.allMatches }),
      queryClient.invalidateQueries({ queryKey: keys.allDecks }),
      queryClient.invalidateQueries({ queryKey: keys.allLeagues }),
      queryClient.invalidateQueries({ queryKey: keys.allDashboard }),
      queryClient.invalidateQueries({ queryKey: MATCH_DETAIL }),
      queryClient.invalidateQueries({ queryKey: keys.allLimited }),
    ])
  }

  /** Every query a match's result feeds: all but the caches kept current by their own pushes. */
  async function refetchAfterCorrection() {
    await queryClient.invalidateQueries({
      predicate: (query) => !PUSHED_KEYS.includes(String(query.queryKey[0])),
    })
  }

  async function runManual<T extends ManualEditView | ManualMatchSaveView>(
    title: string,
    action: () => Promise<CommandResult<T>>,
    refetch: () => Promise<void> = refetchAfterManualSave,
  ): Promise<{ ok: true; answer: T } | { ok: false; errors: FieldErrors }> {
    saving.value = true
    try {
      const answer = unwrap(await action())
      if (answer.kind === 'invalid') return { ok: false, errors: answer.errors }
      await refetch()
      add({ type: 'success', title: 'Success', message: answer.message })
      return { ok: true, answer }
    } catch (error) {
      add({ type: 'error', title, message: errorMessage(error) })
      return { ok: false, errors: {} }
    } finally {
      saving.value = false
    }
  }

  async function createManual(
    input: ManualMatchInput,
  ): Promise<{ ok: true; token: string } | { ok: false; errors: FieldErrors }> {
    const outcome = await runManual('Could not add the match', () => commands.createManualMatch(input))
    if (!outcome.ok) return outcome
    return { ok: true, token: outcome.answer.kind === 'created' ? outcome.answer.token : '' }
  }

  async function edit(title: string, action: () => Promise<CommandResult<ManualEditView>>): Promise<ManualOutcome> {
    const outcome = await runManual(title, action)
    return outcome.ok ? { ok: true } : outcome
  }

  function saveManualHand(token: string, gameNumber: number, hand: ManualHandInput): Promise<ManualOutcome> {
    return edit('Could not save the hand', () => commands.setManualHand(token, gameNumber, hand))
  }

  function saveManualSideboard(
    token: string,
    gameNumber: number,
    changes: ManualSideboardChangeInput[],
  ): Promise<ManualOutcome> {
    return edit('Could not save the sideboard', () => commands.setManualSideboard(token, gameNumber, changes))
  }

  function saveManualReveals(token: string, gameNumber: number, cards: ManualRevealInput[]): Promise<ManualOutcome> {
    return edit('Could not save the revealed cards', () => commands.setManualReveals(token, gameNumber, cards))
  }

  /** What the "Edit match" sheet opens with; `null` (after a toast) when it cannot be read. */
  async function loadEditView(matchId: number): Promise<MatchEditView | null> {
    try {
      return unwrap(await commands.matchEditView(matchId))
    } catch (error) {
      add({ type: 'error', title: 'Could not open the match', message: errorMessage(error) })
      return null
    }
  }

  /** "Edit match": save the player's fix of match row `matchId`. */
  async function correctMatch(matchId: number, correction: MatchCorrectionInput): Promise<ManualOutcome> {
    const outcome = await runManual(
      'Could not save the match',
      () => commands.correctMatch(matchId, correction),
      refetchAfterCorrection,
    )
    return outcome.ok ? { ok: true } : outcome
  }

  return {
    saving,
    saveNotes,
    hide,
    restore,
    moveToLeague,
    setDeck,
    createManual,
    saveManualHand,
    saveManualSideboard,
    saveManualReveals,
    loadEditView,
    correctMatch,
  }
}
