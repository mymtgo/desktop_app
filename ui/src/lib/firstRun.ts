import type { FirstRunState, ImportProgress } from '@/bindings'
import { importRunning } from '@/lib/legacyImport'

/** The first run's 0.x question, as release spec section 13.2 words it. */
export const FIRST_RUN_TEXT = {
  found: 'MyMTGO found data from MyMTGO 0.x on this computer.',
  migrate: 'Migrate from MyMTGO 0.x',
  fresh: 'Start fresh',
  retry: 'Try again',
  // Not in the spec: short text for leaving a failed migration (fix round 1).
  dismiss: 'Continue without importing',
} as const

/** The page a failed migration's error points at: it shows itself, not the failure. */
export const IMPORT_PATH = '/import'

/** What the main window shows instead of its page, or null for the page. */
export type FirstRunScreen = 'ask' | 'importing' | 'failed' | null

/** Why the migration failed or did not start, or null. */
export function failureMessage(state: FirstRunState, progress: ImportProgress | null | undefined): string | null {
  if (progress?.phase === 'failed') return progress.message
  return state.error
}

/**
 * The first-run screen: the question while the app asks, the import while a
 * migration runs (its error with "Try again" when it failed), else the page.
 * A failed migration leaves the Import page (`path`) to itself, so its 0.x
 * section can choose the file and import again.
 */
export function firstRunScreen(
  state: FirstRunState | undefined,
  progress: ImportProgress | null | undefined,
  infoRunning: boolean,
  path = '',
): FirstRunScreen {
  if (state === undefined) return null
  if (state.decision === 'ask') return 'ask'
  if (state.decision !== 'migrate') return null
  if (importRunning(progress, infoRunning)) return 'importing'
  if (progress?.phase === 'finished') return null
  if (failureMessage(state, progress) !== null) return path === IMPORT_PATH ? null : 'failed'
  return 'importing'
}
