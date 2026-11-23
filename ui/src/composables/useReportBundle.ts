import { ref } from 'vue'
import { commands } from '@/bindings'
import { useToast } from './useToast'

/** 0.x's HelpPopover errors (spec section 8.1, dashes rewritten). */
export const NO_LOG_FILES = "Couldn't find log files: check your log path in settings."
export const BUNDLE_FAILED = 'Something went wrong building the report bundle.'

export interface ReportBundleError {
  message: string
  /** Show "Open settings" (to Storage, where the log path is set). */
  showSettingsLink: boolean
}

/**
 * "Download report data" (0.x `HelpPopover.downloadBundle`): the app finds
 * the logs, asks where to save the zip and writes it. Busy while it runs;
 * a closed save dialog does nothing.
 */
export function useReportBundle() {
  const busy = ref(false)
  const error = ref<ReportBundleError | null>(null)
  const { add } = useToast()

  async function download(): Promise<void> {
    busy.value = true
    error.value = null
    try {
      const result = await commands.saveReportBundle()
      if (result.status === 'error') {
        error.value = { message: BUNDLE_FAILED, showSettingsLink: false }
        return
      }
      if (result.data === 'noLogFiles') {
        error.value = { message: NO_LOG_FILES, showSettingsLink: true }
        return
      }
      if (result.data === 'saved') {
        add({ type: 'success', title: 'Saved', message: 'Report data saved' })
      }
    } catch (e) {
      console.error('Failed to save the report bundle', e)
      error.value = { message: BUNDLE_FAILED, showSettingsLink: false }
    } finally {
      busy.value = false
    }
  }

  return { busy, error, download }
}
