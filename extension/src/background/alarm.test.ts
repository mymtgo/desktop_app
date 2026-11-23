import { describe, expect, it } from 'vitest'
import { ensureAlarm } from './alarm'

describe('ensureAlarm', () => {
  it('creates_the_alarm_when_there_is_none', async () => {
    const created: unknown[] = []
    await ensureAlarm({ get: async () => undefined, create: (...args) => void created.push(args) }, 'ping', 30)
    expect(created).toEqual([['ping', { periodInMinutes: 30 }]])
  })

  it('leaves_an_existing_alarm_alone', async () => {
    const created: unknown[] = []
    await ensureAlarm({ get: async () => ({ name: 'ping' }), create: (...args) => void created.push(args) }, 'ping', 30)
    expect(created).toEqual([])
  })

  it('never_throws', async () => {
    await expect(
      ensureAlarm({ get: async () => Promise.reject(new Error('no alarms')), create: () => {} }, 'ping', 30),
    ).resolves.toBeUndefined()
  })
})
