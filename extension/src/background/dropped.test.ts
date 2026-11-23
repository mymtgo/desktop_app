import { describe, expect, it } from 'vitest'
import { DroppedCounter, type CounterStorage } from './dropped'

// A storage whose reads and writes each take a moment, so interleaving would show.
function slowStorage() {
  const data: Record<string, unknown> = {}
  const wait = () => new Promise((resolve) => setTimeout(resolve, 5))
  const storage: CounterStorage = {
    get: async (key) => {
      await wait()
      return { [key]: data[key] }
    },
    set: async (items) => {
      await wait()
      Object.assign(data, items)
    },
    remove: async (keys) => {
      await wait()
      for (const key of keys) delete data[key]
    },
  }
  return { data, storage }
}

const drop = { kind: 'match', reason: 'bot' } as const

describe('DroppedCounter', () => {
  it('counts_every_drop_even_when_they_arrive_together', async () => {
    const { data, storage } = slowStorage()
    const counter = new DroppedCounter(storage, () => 9)
    await Promise.all([counter.record(drop), counter.record(drop), counter.record(drop)])
    expect(data.droppedCount).toBe(3)
  })

  it('a_reset_is_not_overwritten_by_a_drop_in_progress', async () => {
    const { data, storage } = slowStorage()
    const counter = new DroppedCounter(storage, () => 9)
    await counter.record(drop)
    const inFlight = counter.record(drop)
    await counter.reset()
    await inFlight
    // The drop ran first, then the reset cleared everything.
    expect(data.droppedCount).toBeUndefined()
    expect(data.lastDrop).toBeUndefined()
  })

  it('a_drop_after_a_reset_starts_from_one', async () => {
    const { data, storage } = slowStorage()
    const counter = new DroppedCounter(storage, () => 9)
    await counter.record(drop)
    await counter.reset()
    await counter.record(drop)
    expect(data.droppedCount).toBe(1)
  })
})
