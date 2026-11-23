// A tab that was open on endstep.cc before the extension was installed has no page hook, so
// nothing from it is captured until it is reloaded. The popup asks for the active tab's state.
import { INSTALL_MARKER } from '../page/hook'
import { ENDSTEP_PATTERN } from './reinject'

export interface TabHookApi {
  // Id of the active tab in the focused window when it is on endstep.cc, else null.
  activeEndstepTab(urlPattern: string): Promise<number | null>
  // True when the page's own world carries the hook's install marker.
  hasHook(tabId: number, marker: string): Promise<boolean>
}

export const PAGE_CHECK_TIMEOUT_MS = 1000

// True only when the active tab is an Endstep tab that is known to have no hook. Any doubt gives
// false, including a page that does not answer in time (busy, paused or still loading), so the
// popup never waits on a tab.
export async function activeTabNeedsReload(api: TabHookApi, timeoutMs: number = PAGE_CHECK_TIMEOUT_MS): Promise<boolean> {
  let timer: ReturnType<typeof setTimeout> | undefined
  try {
    const tabId = await api.activeEndstepTab(ENDSTEP_PATTERN)
    if (tabId === null) return false
    const silent = new Promise<boolean>((resolve) => {
      timer = setTimeout(() => resolve(true), timeoutMs)
    })
    // Resolves true when the hook is there or the page stayed silent.
    return !(await Promise.race([api.hasHook(tabId, INSTALL_MARKER), silent]))
  } catch {
    return false
  } finally {
    clearTimeout(timer)
  }
}
