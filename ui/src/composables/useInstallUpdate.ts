import { ref } from 'vue'
import { commands } from '@/bindings'
import { useToast } from '@/composables/useToast'
import { errorMessage, unwrap } from '@/lib/commands'

/** How long the installing overlay shows before the install starts (0.x). */
export const OVERLAY_BEFORE_INSTALL_MS = 3_000

// Module scope: one install and one overlay whichever surface started it
// (banner, Updates card, status bar), as 0.x.
const installing = ref(false)

/**
 * Install the downloaded update after the overlay has been on screen for a
 * moment. On success the app restarts, so `installing` stays true.
 */
export function useInstallUpdate() {
  const { add } = useToast()

  async function install() {
    if (installing.value) return
    installing.value = true
    await new Promise((resolve) => setTimeout(resolve, OVERLAY_BEFORE_INSTALL_MS))
    try {
      unwrap(await commands.installUpdate())
    } catch (error) {
      installing.value = false
      add({ type: 'error', title: 'Could not install the update', message: errorMessage(error) })
    }
  }

  return { installing, install }
}
