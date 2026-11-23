import { disable, enable, isEnabled } from '@tauri-apps/plugin-autostart'
import { ref } from 'vue'
import { commands } from '@/bindings'
import { useToast } from '@/composables/useToast'
import { errorMessage } from '@/lib/commands'

/** The OS "launch at login" switch. `changing` is true while a change runs. */
export function useLaunchAtLogin() {
  const { add } = useToast()
  const enabled = ref(false)
  const changing = ref(false)

  async function load() {
    try {
      enabled.value = await isEnabled()
    } catch (error) {
      add({ type: 'error', title: 'Could not read launch at login', message: errorMessage(error) })
    }
  }

  async function set(on: boolean) {
    if (changing.value) return
    changing.value = true
    try {
      if (on) await enable()
      else await disable()
      enabled.value = await isEnabled()
    } catch (error) {
      add({ type: 'error', title: 'Could not change launch at login', message: errorMessage(error) })
      return
    } finally {
      changing.value = false
    }
    await recordChange()
  }

  function warnNotRecorded(message: string) {
    console.warn('launch at login change not recorded', message)
    add({ type: 'error', title: 'Could not remember your launch at login choice', message })
  }

  /** The 0.x import never replaces a choice made here (release spec section 13 item 4). */
  async function recordChange() {
    try {
      const result = await commands.launchAtLoginChanged()
      if (result.status === 'error') warnNotRecorded(errorMessage(result.error))
    } catch (error) {
      warnNotRecorded(errorMessage(error))
    }
  }

  return { enabled, changing, load, set }
}
