import { describe, expect, it } from 'vitest'
import type { FirstRunState, ImportProgress } from '@/bindings'
import { FIRST_RUN_TEXT, failureMessage, firstRunScreen } from './firstRun'

function state(decision: FirstRunState['decision'], error: string | null = null): FirstRunState {
  return { decision, error }
}

const writing: ImportProgress = { phase: 'writing', done: 1, total: 4 }
const failed: ImportProgress = { phase: 'failed', message: 'the 0.x data could not be read' }

// Release spec section 13.2.
describe('the first-run 0.x screen', () => {
  it('has the spec strings', () => {
    expect(FIRST_RUN_TEXT.found).toBe('MyMTGO found data from MyMTGO 0.x on this computer.')
    expect(FIRST_RUN_TEXT.migrate).toBe('Migrate from MyMTGO 0.x')
    expect(FIRST_RUN_TEXT.fresh).toBe('Start fresh')
    expect(FIRST_RUN_TEXT.retry).toBe('Try again')
    expect(FIRST_RUN_TEXT.dismiss).toBe('Continue without importing')
  })

  it('asks when the app asks', () => {
    expect(firstRunScreen(state('ask'), null, false)).toBe('ask')
  })

  it('shows the import while a migration runs, and its error with Try again', () => {
    expect(firstRunScreen(state('migrate'), null, true)).toBe('importing')
    expect(firstRunScreen(state('migrate'), writing, false)).toBe('importing')
    expect(firstRunScreen(state('migrate'), failed, false)).toBe('failed')
    expect(firstRunScreen(state('migrate', 'no MyMTGO 0.x database was found'), null, false)).toBe('failed')
    expect(failureMessage(state('migrate'), failed)).toBe('the 0.x data could not be read')
    // The Import page shows itself under a failure, never under the import.
    expect(firstRunScreen(state('migrate'), failed, false, '/import')).toBeNull()
    expect(firstRunScreen(state('migrate'), writing, false, '/import')).toBe('importing')
    expect(failureMessage(state('migrate', 'not started'), null)).toBe('not started')
  })

  it('gives way to the app once the migration finished, or for fresh and nothing', () => {
    expect(firstRunScreen(state('migrate'), { phase: 'finished', summary: {} } as ImportProgress, false)).toBeNull()
    expect(firstRunScreen(state('fresh'), null, false)).toBeNull()
    expect(firstRunScreen(state('nothing'), null, false)).toBeNull()
    expect(firstRunScreen(undefined, null, false)).toBeNull()
  })
})
