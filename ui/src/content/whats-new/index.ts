import { defineAsyncComponent, type Component } from 'vue'

/**
 * What's new (release spec section 7): one Vue component per minor version,
 * registered here with its `major.minor`, images beside it as Vite assets.
 * Add the next release's entry at the end.
 */
export type WhatsNewEntry = {
  /** The release's `major.minor`, for example `1.0`. */
  version: string
  component: Component
}

export const whatsNewEntries: WhatsNewEntry[] = [
  { version: '1.0', component: defineAsyncComponent(() => import('./1.0.vue')) },
  { version: '1.1', component: defineAsyncComponent(() => import('./1.1.vue')) },
  { version: '1.2', component: defineAsyncComponent(() => import('./1.2.vue')) },
  { version: '1.3', component: defineAsyncComponent(() => import('./1.3.vue')) },
  { version: '1.4', component: defineAsyncComponent(() => import('./1.4.vue')) },
]

/** The `major.minor` of a version, read as the app reads it (`whats_new::major_minor`). */
export function majorMinor(version: string): [number, number] {
  const parts = version.trim().replace(/^v+/, '').split('.')
  const leading = (part: string | undefined) => Number.parseInt(/^\d*/.exec(part ?? '')?.[0] || '0', 10)
  return [leading(parts[0]), leading(parts[1])]
}

function notNewer(entry: [number, number], running: [number, number]): boolean {
  return entry[0] < running[0] || (entry[0] === running[0] && entry[1] <= running[1])
}

function newer(a: [number, number], b: [number, number]): boolean {
  return a[0] > b[0] || (a[0] === b[0] && a[1] > b[1])
}

/** The newest entry not newer than the running version, or `null`. */
export function entryFor(running: string, entries: WhatsNewEntry[] = whatsNewEntries): WhatsNewEntry | null {
  const current = majorMinor(running)
  let best: WhatsNewEntry | null = null
  for (const entry of entries) {
    const version = majorMinor(entry.version)
    if (!notNewer(version, current)) continue
    if (best === null || newer(version, majorMinor(best.version))) best = entry
  }
  return best
}

/** The page's link when it has content for the running version (0.x `whatsNewAvailable`), else `null`. */
export function whatsNewUrl(
  running: string | null | undefined,
  entries: WhatsNewEntry[] = whatsNewEntries,
): string | null {
  if (!running) return null
  return entryFor(running, entries) ? '/whats-new' : null
}

/** Every entry's `major.minor`, for the start command. */
export function whatsNewVersions(entries: WhatsNewEntry[] = whatsNewEntries): string[] {
  return entries.map((entry) => entry.version)
}
