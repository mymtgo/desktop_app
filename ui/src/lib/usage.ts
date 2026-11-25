import { commands, type UsageEvent } from '@/bindings'

/**
 * Count one usage event (usage analytics spec). Fire and forget: the app
 * decides in Rust whether anything is sent (a built-in key, "Share anonymous
 * usage data" on, offline mode off), and a failure never reaches the page.
 */
export function trackUsage(event: UsageEvent): void {
  void commands.trackUsage(event).catch(() => {})
}
