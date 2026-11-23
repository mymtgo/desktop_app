import type { ConnectionView, FileView, PlatformView, StatusView } from '@/bindings'

export type Tone = 'ok' | 'warn' | 'error' | 'idle'

/** The logs of the platform the app shows: `mtgo.log` files or Arena's `Player.log`. */
export function platformFiles(status: StatusView, platform: PlatformView): FileView[] {
  switch (platform) {
    case 'mtgo':
      return status.files
    case 'arena':
      return status.arenaFiles
    case 'endstep':
      // Endstep reads browser captures, not log files.
      return []
  }
}

/** The watcher dot and its label (spec 6 status bar), for the platform the app shows. */
export function trackingState(
  status: StatusView,
  platform: PlatformView,
  /** Endstep's browser connection, when known: the extension reads nothing while it is not connected. */
  endstepConnection?: ConnectionView,
): { tone: Tone; text: string } {
  if (status.restarting !== null) {
    return { tone: 'error', text: 'Tracking paused, restarting' }
  }
  // The watcher is off (release spec 4.1): logs are found but not read.
  if (status.paused) {
    return { tone: 'error', text: 'Stopped' }
  }
  if (!status.started) {
    return { tone: 'idle', text: 'Starting' }
  }
  if (platform === 'endstep') {
    if (endstepConnection?.kind === 'neverSeen' || endstepConnection?.kind === 'stale') {
      return { tone: 'warn', text: 'Endstep not connected' }
    }
    return { tone: 'ok', text: 'Reading Endstep captures' }
  }
  const count = platformFiles(status, platform).length
  if (count === 0) {
    return { tone: 'warn', text: platform === 'arena' ? 'Arena not found' : 'MTGO not found' }
  }
  return { tone: 'ok', text: count === 1 ? 'Watching 1 log' : `Watching ${count} logs` }
}

export function newestEventMs(status: StatusView, platform: PlatformView): number | null {
  let newest: number | null = null
  for (const file of platformFiles(status, platform)) {
    if (file.lastLiveGrowthMs !== null && (newest === null || file.lastLiveGrowthMs > newest)) {
      newest = file.lastLiveGrowthMs
    }
  }
  return newest
}

const relative = new Intl.RelativeTimeFormat('en', { numeric: 'always' })

export function sinceText(ms: number | null, nowMs: number): string {
  if (ms === null) {
    return 'No live events yet'
  }
  const seconds = Math.max(0, Math.round((nowMs - ms) / 1000))
  if (seconds < 5) {
    return 'Last event just now'
  }
  if (seconds < 60) {
    return `Last event ${relative.format(-seconds, 'second')}`
  }
  const minutes = Math.floor(seconds / 60)
  if (minutes < 60) {
    return `Last event ${relative.format(-minutes, 'minute')}`
  }
  const hours = Math.floor(minutes / 60)
  if (hours < 24) {
    return `Last event ${relative.format(-hours, 'hour')}`
  }
  return `Last event ${relative.format(-Math.floor(hours / 24), 'day')}`
}

export function statusWarnings(status: StatusView, platform: PlatformView): string[] {
  const warnings: string[] = []
  if (platform === 'mtgo' && status.twoClients) {
    warnings.push('Two MTGO clients are running')
  }
  if (platform === 'mtgo' && status.awaitingIdentity > 0) {
    warnings.push('Waiting to identify your MTGO username')
  }
  if (status.quarantined > 0) {
    const noun = status.quarantined === 1 ? 'event' : 'events'
    warnings.push(`${status.quarantined} log ${noun} skipped after repeated errors`)
  }
  if (status.failedMatches > 0) {
    const many = status.failedMatches > 1
    warnings.push(
      `${status.failedMatches} ${many ? 'matches' : 'match'} could not be read from the log; a future update will retry ${many ? 'them' : 'it'}`,
    )
  }
  if (status.timezoneWarning !== null) {
    warnings.push(status.timezoneWarning)
  }
  if (platformFiles(status, platform).some((file) => file.badTimezone)) {
    warnings.push('A log file has an unknown timezone; its times use yours')
  }
  return warnings
}

export type FirstRun = 'not_found' | 'awaiting_identity' | 'waiting_for_match' | null

/** Which first-run state replaces an empty list (spec 6), in the spec's order. */
export function firstRunState(
  status: StatusView | undefined,
  total: number | undefined,
  /** Rows on the page: a live row can arrive before the stored total counts it. */
  rows = 0,
): FirstRun {
  if (status === undefined || !status.started || total === undefined) {
    return null
  }
  // First-run states are for a first run: stored matches always show.
  if (total > 0 || rows > 0) {
    return null
  }
  if (status.files.length === 0) {
    return 'not_found'
  }
  return status.awaitingIdentity > 0 ? 'awaiting_identity' : 'waiting_for_match'
}
