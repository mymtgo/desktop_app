import { onUnmounted, ref } from 'vue'
import { commands, type ReplayShareState } from '@/bindings'
import { COPIED_MS } from '@/replay/share/types'

type ShareResult = { status: 'ok'; data: ReplayShareState } | { status: 'error'; error: string }

/** v1's line when the clipboard refuses the link (0.x let the failure throw unseen). */
export const COPY_FAILED = "Couldn't copy the link."

function message(failure: unknown): string {
  return failure instanceof Error ? failure.message : String(failure)
}

/**
 * The share dialog's state for the game on screen: the state the app reports
 * (`replay_share_state`), a busy flag while `share_replay` or `revoke_replay`
 * runs, the message of a refused command as the error line, and "Copied".
 * The app refuses with a ready-made message (0.x's), shown as it comes.
 */
export function useReplayShareState(gameId: () => number) {
  const state = ref<ReplayShareState | null>(null)
  const busy = ref(false)
  const error = ref<string | null>(null)
  const copied = ref(false)
  let timer: ReturnType<typeof setTimeout> | null = null
  // Only the newest read counts when the game changes quickly.
  let reads = 0
  let loadedGame: number | null = null

  function resetCopied() {
    if (timer) clearTimeout(timer)
    timer = null
    copied.value = false
  }

  /** Reads the state again: after sign-in, offline changes, or another game. */
  async function refresh() {
    const mine = ++reads
    const game = gameId()
    if (loadedGame !== null && loadedGame !== game) {
      // Another game: its error and copy are not this one's.
      error.value = null
      resetCopied()
    }
    loadedGame = game
    let result: ShareResult
    try {
      result = (await commands.replayShareState(game)) as ShareResult
    } catch (failure) {
      result = { status: 'error', error: message(failure) }
    }
    if (mine !== reads) return
    if (result.status === 'ok') {
      state.value = result.data
    } else {
      error.value = result.error
    }
  }

  async function run(command: (game: number) => Promise<ShareResult>) {
    if (busy.value) return
    busy.value = true
    error.value = null
    const game = gameId()
    let result: ShareResult
    try {
      result = await command(game)
    } catch (failure) {
      result = { status: 'error', error: message(failure) }
    }
    busy.value = false
    if (gameId() !== game) {
      // Another game is on screen now: its link ends with its own number,
      // so read its state rather than show this game's.
      void refresh()
      return
    }
    // A read started during the command holds the state from before it.
    reads++
    if (result.status === 'ok') {
      state.value = result.data
    } else {
      error.value = result.error
    }
  }

  const share = () => run((game) => commands.shareReplay(game) as Promise<ShareResult>)
  const revoke = () => run((game) => commands.revokeReplay(game) as Promise<ShareResult>)

  async function copy() {
    const url = state.value?.url
    if (!url) return
    try {
      await navigator.clipboard.writeText(url)
    } catch {
      resetCopied()
      error.value = COPY_FAILED
      return
    }
    if (error.value === COPY_FAILED) error.value = null
    copied.value = true
    if (timer) clearTimeout(timer)
    timer = setTimeout(resetCopied, COPIED_MS)
  }

  /** The dialog was opened: nothing is copied yet, and the state may have moved. */
  async function reopened() {
    resetCopied()
    error.value = null
    await refresh()
  }

  onUnmounted(resetCopied)

  return { state, busy, error, copied, refresh, share, revoke, copy, reopened }
}
