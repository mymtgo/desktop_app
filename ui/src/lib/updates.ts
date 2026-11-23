import type { UpdateStatus } from '@/bindings'

/** What the updater is doing, as the 0.x Updates card said it. */
export function updateLabel(status: UpdateStatus): string {
  if (!status.active) return 'Updates are only checked in the installed app'
  switch (status.phase) {
    case 'checking':
      return 'Checking for updates...'
    case 'downloading':
      return `Downloading v${status.available}...`
    case 'ready':
      return `v${status.available} is ready to install`
    case 'error':
      return 'Could not check for updates'
    default:
      return "You're up to date"
  }
}

/** "Last checked 5 min ago", or null before the first check. */
export function checkedAgo(checkedAtMs: number | null, nowMs: number): string | null {
  if (checkedAtMs === null) return null
  const minutes = Math.round((nowMs - checkedAtMs) / 60_000)
  if (minutes < 1) return 'Last checked just now'
  return minutes < 60 ? `Last checked ${minutes} min ago` : `Last checked ${Math.round(minutes / 60)} h ago`
}

/** The status dot colour. */
export function updateDot(status: UpdateStatus): string {
  switch (status.phase) {
    case 'ready':
      return 'bg-primary'
    case 'checking':
    case 'downloading':
      return 'animate-pulse bg-warning'
    case 'error':
      return 'bg-destructive'
    default:
      return 'bg-success'
  }
}
