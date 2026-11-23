import type { PlatformView, StatusView } from '@/bindings'
import { platformFiles } from '@/lib/status'

/** What the Watcher card shows (0.x `WatcherCard.vue`, release spec 4.1). */
export type WatcherState = {
  /** On, and at least one `mtgo.log` is found. */
  running: boolean
  /** Start needs a found log: an `mtgo.log` (0.x: valid paths) or Arena's `Player.log`. */
  canStart: boolean
  badge: 'Running' | 'Stopped'
  button: 'Start' | 'Stop'
}

export function watcherState(active: boolean, status: StatusView | undefined): WatcherState {
  const canStart = (status?.files.length ?? 0) + (status?.arenaFiles.length ?? 0) > 0
  const running = active && canStart
  return { running, canStart, badge: running ? 'Running' : 'Stopped', button: running ? 'Stop' : 'Start' }
}

/** The platform's log status while it is left on automatic: v1's own. */
export type AutomaticLogStatus = {
  /** `searching` until the first status arrives. */
  state: 'searching' | 'found' | 'missing'
  message: string
  /** The logs found. */
  paths: string[]
  /** Every location looked in, shown on request when nothing is found. */
  searched: string[]
}

export function automaticLogStatus(status: StatusView | undefined, platform: PlatformView): AutomaticLogStatus {
  const name = platform === 'arena' ? 'Player.log' : 'mtgo.log'
  if (!status) {
    return { state: 'searching', message: `Searching for ${name}...`, paths: [], searched: [] }
  }
  const searched = (platform === 'arena' ? status.arenaSearched : status.searched) ?? []
  const paths = platformFiles(status, platform).map((file) => file.path)
  if (paths.length === 0) {
    const places = searched.length === 1 ? 'location' : 'locations'
    return {
      state: 'missing',
      message: `Can't find ${name}. Checked ${searched.length} possible ${places}.`,
      paths,
      searched,
    }
  }
  const message = paths.length === 1 ? `Found ${name}` : `Found ${paths.length} logs`
  return { state: 'found', message, paths, searched }
}
