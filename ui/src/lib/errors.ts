import type { App } from 'vue'
import type { UiError } from '@/bindings'

/** At most this many reports a session. */
export const MAX_REPORTS = 20
/** The same message is sent again only after this long. */
export const REPEAT_AFTER_MS = 60_000

/**
 * Browser notices that are not errors: a ResizeObserver callback that resizes
 * what it observes makes the browser defer the rest to the next frame and
 * fire an `error` event about it. Nothing is lost.
 */
export function isBenign(message: string): boolean {
  return message.startsWith('ResizeObserver loop')
}

export type Reporter = (source: string, error: unknown, route?: string | null) => boolean

/** A message and stack for any thrown value. */
export function describeError(error: unknown): { message: string; stack: string | null } {
  if (error instanceof Error) {
    return { message: error.message || error.name, stack: error.stack ?? null }
  }
  if (typeof error === 'string') {
    return { message: error, stack: null }
  }
  try {
    return { message: JSON.stringify(error) ?? String(error), stack: null }
  } catch {
    return { message: String(error), stack: null }
  }
}

/** Sends UI errors to the app, which logs them and reports them to Sentry. Returns whether it sent. */
export function createErrorReporter(send: (report: UiError) => Promise<unknown>, now: () => number = Date.now): Reporter {
  const lastSent = new Map<string, number>()
  let sent = 0
  return (source, error, route = null) => {
    const { message, stack } = describeError(error)
    if (isBenign(message)) {
      return false
    }
    const at = now()
    const previous = lastSent.get(message)
    if (sent >= MAX_REPORTS || (previous !== undefined && at - previous < REPEAT_AFTER_MS)) {
      return false
    }
    lastSent.set(message, at)
    sent += 1
    try {
      send({ source, message, stack, route }).catch(() => {})
    } catch {
      // A send that throws at once must never fail the caller.
    }
    return true
  }
}

/** Report Vue errors, uncaught errors and unhandled promise rejections. */
export function installErrorReporting(app: App, report: Reporter, route: () => string | null) {
  app.config.errorHandler = (error, _instance, info) => {
    report(`vue: ${info}`, error, route())
    console.error(error)
  }
  window.addEventListener('error', (event) => {
    report('window', event.error ?? event.message, route())
  })
  window.addEventListener('unhandledrejection', (event) => {
    report('promise', event.reason, route())
  })
}
