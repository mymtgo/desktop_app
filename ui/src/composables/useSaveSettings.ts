import { useQueryClient } from '@tanstack/vue-query'
import { ref } from 'vue'
import { commands, type Settings } from '@/bindings'
import { useToast } from '@/composables/useToast'
import { errorMessage, unwrap } from '@/lib/commands'
import { keys } from '@/lib/keys'

/** Save settings through the tracker, or choose a log folder and save it. A failure becomes a toast. */
export function useSaveSettings() {
  const queryClient = useQueryClient()
  const { add } = useToast()
  const saving = ref(false)

  async function save(next: Settings): Promise<boolean> {
    saving.value = true
    try {
      queryClient.setQueryData(keys.settings, unwrap(await commands.saveSettings(next)))
      return true
    } catch (error) {
      add({ type: 'error', title: 'Could not save settings', message: errorMessage(error) })
      return false
    } finally {
      saving.value = false
    }
  }

  async function chooseLogFolder(current: Settings): Promise<boolean> {
    let folder: string | null
    try {
      folder = await commands.chooseLogFolder()
    } catch (error) {
      add({ type: 'error', title: 'Could not open the folder picker', message: errorMessage(error) })
      return false
    }
    if (folder === null) {
      return false
    }
    // The dialog can stay open a long time: build from what is cached now, not from the click.
    const latest = queryClient.getQueryData<Settings>(keys.settings) ?? current
    return save({ ...latest, logFolder: folder })
  }

  return { saving, save, chooseLogFolder }
}
