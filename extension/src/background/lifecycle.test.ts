import { describe, expect, it } from 'vitest'
import { onInstalled, onWorkerLoad } from './lifecycle'

function recorder() {
  const calls: string[] = []
  const call = (name: string) => async () => void calls.push(name)
  return { calls, deps: { reinject: call('reinject'), startUp: call('startUp'), ensureAlarm: call('alarm') } }
}

describe('onWorkerLoad', () => {
  it('sets_the_alarm_reinjects_the_relay_and_starts_up', () => {
    const { calls, deps } = recorder()
    onWorkerLoad(deps)
    expect(calls.sort()).toEqual(['alarm', 'reinject', 'startUp'])
  })

  it('a_failing_step_does_not_stop_the_others', async () => {
    const { calls, deps } = recorder()
    onWorkerLoad({ ...deps, reinject: () => Promise.reject(new Error('no tabs')) })
    await new Promise((resolve) => setTimeout(resolve, 0))
    expect(calls.sort()).toEqual(['alarm', 'startUp'])
  })
})

describe('onInstalled', () => {
  it.each(['install', 'update'])('reinjects_the_relay_after_%s', (reason) => {
    const { calls, deps } = recorder()
    onInstalled({ reason }, deps)
    expect(calls).toEqual(['reinject', 'startUp'])
  })

  it.each(['browser_update', 'chrome_update', 'shared_module_update'])('does_not_reinject_after_%s', (reason) => {
    const { calls, deps } = recorder()
    onInstalled({ reason }, deps)
    expect(calls).toEqual(['startUp'])
  })
})
