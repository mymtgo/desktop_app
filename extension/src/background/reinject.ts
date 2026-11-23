// After an install or update the browser does not run the declared content scripts in tabs that
// were already open, so their relay is dead and frames are dropped. Running relay.js again in the
// isolated world gives the old page hook a live relay. The page's own world is never touched.
export const ENDSTEP_PATTERN = 'https://endstep.cc/*'
export const RELAY_FILE = 'relay.js'

export interface ReinjectApi {
  // Ids of the open tabs matching the url pattern.
  queryTabs(urlPattern: string): Promise<number[]>
  // Runs a packaged file in the tab's top frame, isolated world.
  runFile(tabId: number, file: string): Promise<void>
}

// Returns how many tabs the relay reached. Never throws: a tab that closed or cannot be reached is skipped.
export async function reinjectRelay(api: ReinjectApi): Promise<number> {
  let tabIds: number[]
  try {
    tabIds = await api.queryTabs(ENDSTEP_PATTERN)
  } catch {
    return 0
  }
  let reached = 0
  for (const tabId of tabIds) {
    try {
      await api.runFile(tabId, RELAY_FILE)
      reached += 1
    } catch {
      // closed, discarded or no access
    }
  }
  return reached
}
