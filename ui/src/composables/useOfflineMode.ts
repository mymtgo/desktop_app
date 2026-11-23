import { computed, ref, type ComputedRef } from 'vue'
import { commands, type OfflineModeState } from '@/bindings'
import { errorMessage, unwrap } from '@/lib/commands'
import { useStatus } from '@/lib/queries'

/** Offline mode is on, as the status says (0.x's shared `offlineMode` prop). */
export function useOfflineMode(): ComputedRef<boolean> {
  const { data: status } = useStatus()
  return computed(() => status.value?.offlineMode === true)
}

/**
 * The offline mode switch (0.x OfflineModeCard). Turning it on is never
 * gated behind a dialog: privacy should not need confirming. Leaving starts
 * a cooldown, so the rejoin dialog asks first. `onChanged` runs after every
 * switch, refused or not.
 */
export function useOfflineModeSwitch(onChanged: () => void | Promise<void> = () => {}) {
  const state = ref<OfflineModeState | null>(null)
  const dialogOpen = ref(false)
  const switching = ref(false)
  const problem = ref<string | null>(null)

  async function load() {
    try {
      state.value = unwrap(await commands.offlineMode())
    } catch (error) {
      problem.value = errorMessage(error)
    }
  }

  async function send(enabled: boolean) {
    problem.value = null
    switching.value = true
    try {
      state.value = unwrap(await commands.setOfflineMode(enabled))
    } catch (error) {
      problem.value = errorMessage(error)
      await load()
    } finally {
      switching.value = false
    }
    // Rejoining downloads the community data, so what hangs on it reloads.
    await onChanged()
  }

  async function toggle(on: boolean) {
    if (!on) {
      dialogOpen.value = true
      return
    }
    await send(true)
  }

  async function confirmRejoin() {
    dialogOpen.value = false
    await send(false)
  }

  return { state, dialogOpen, switching, problem, load, toggle, confirmRejoin }
}
