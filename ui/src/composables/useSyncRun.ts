import { onUnmounted, ref } from 'vue'
import type { SyncStatus } from '@/bindings'
import { errorMessage } from '@/lib/commands'

export const SYNC_POLL_MS = 3000
export const SYNC_POLL_DEADLINE_MS = 240_000

type Deps = {
  /** "Sync now": asks the app for a run. Rejects when the app refuses. */
  start: () => Promise<SyncStatus>
  /** Reads the sync status again; null when it cannot be read. */
  poll: () => Promise<SyncStatus | null>
  /** The run ended (or the wait did). */
  onEnd: () => void
  deadlineMs?: number
}

/**
 * The "Sync now" button's busy state (0.x SyncCard). A run is watched with a
 * poll until it ends. The busy state clears at once when the press is
 * refused, and the wait ends when the status cannot be read or the deadline
 * passes, so the button can never stay on "Syncing..." for good.
 */
export function useSyncRun(deps: Deps) {
  const running = ref(false)
  const problem = ref<string | null>(null)
  let timer: ReturnType<typeof setInterval> | null = null

  function stop() {
    if (timer !== null) {
      clearInterval(timer)
      timer = null
    }
  }

  function end() {
    stop()
    running.value = false
    deps.onEnd()
  }

  /** Watch a run, one the press started or one started elsewhere. */
  function watchRun() {
    running.value = true
    stop()
    const deadline = Date.now() + (deps.deadlineMs ?? SYNC_POLL_DEADLINE_MS)
    timer = setInterval(async () => {
      if (Date.now() > deadline) return end()
      const status = await deps.poll()
      if (status === null || !status.syncing) end()
    }, SYNC_POLL_MS)
  }

  async function press() {
    problem.value = null
    running.value = true
    try {
      await deps.start()
    } catch (error) {
      problem.value = errorMessage(error)
      running.value = false
      return
    }
    watchRun()
  }

  onUnmounted(stop)

  return { running, problem, press, watchRun, stop }
}
