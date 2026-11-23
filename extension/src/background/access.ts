// The endstep.cc host permission. Firefox lets the user take it away after install; without it
// the page hook and relay do not run, so the popup says so.
export const ENDSTEP_ORIGINS = ['https://endstep.cc/*']

export interface PermissionsApi {
  contains(query: { origins: string[] }): Promise<boolean>
}

// False when the permission is missing. A browser that cannot answer counts as "has access",
// so the popup never shows a warning it cannot back up.
export async function hasHostAccess(api: PermissionsApi): Promise<boolean> {
  try {
    return await api.contains({ origins: ENDSTEP_ORIGINS })
  } catch {
    return true
  }
}

// Open tabs have no relay after the user grants access again, so it is re-run then.
export function watchGrants(
  onAdded: { addListener(listener: (granted: { origins?: string[] }) => void): void },
  reinject: () => Promise<unknown>,
): void {
  onAdded.addListener((granted) => {
    if (granted.origins?.some((origin) => ENDSTEP_ORIGINS.includes(origin))) {
      reinject().catch(() => {})
    }
  })
}
