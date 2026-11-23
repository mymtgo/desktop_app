import { describe, expect, it } from 'vitest'
import type { Frame } from '../capture/format'
import type { UnitAction } from '../capture/units'
import { recoverOnStart } from './delivery'
import { Engine, type EngineDeps, type TrackerLike } from './engine'
import { MemoryStore, type CaptureStore } from './store'

const frame: Frame = { at: 1, src: 'rest', dir: 'in', url: '/x', text: '{}' }
const open = (unitId: string): UnitAction => ({ kind: 'open', unitId, unitKind: 'match', tabId: 1, at: 1 })
const append = (unitId: string): UnitAction => ({ kind: 'append', unitId, frame })
const close = (unitId: string): UnitAction => ({ kind: 'close', unitId })

// A tracker that returns the next scripted list of actions for each event.
function scripted(...batches: UnitAction[][]): TrackerLike {
  const queue = [...batches]
  return { onEvent: () => queue.shift() ?? [], onTabClosed: () => [] }
}

function deps(store: CaptureStore, tracker: TrackerLike, extra: Partial<EngineDeps> = {}) {
  const storeErrors: { message: string; at: number }[] = []
  const base: EngineDeps = {
    store: Promise.resolve(store),
    newTracker: () => tracker,
    send: async () => ({ ok: true }),
    sendParts: async () => ({ ok: true }),
    captureEnabled: async () => true,
    version: { extension: '0.1.0', browser: 'chrome' },
    now: () => 5,
    recordDrop: async () => {},
    recordStoreError: async (e) => void storeErrors.push(e),
    clearStoreError: async () => {},
    recordDelivered: async () => {},
    ping: async () => {},
  }
  return { deps: { ...base, ...extra }, storeErrors }
}

const event = {} as never

describe('store queue', () => {
  it('runs_jobs_in_arrival_order_even_when_one_fails', async () => {
    const { deps: d } = deps(new MemoryStore(), scripted())
    const engine = new Engine(d)
    const order: number[] = []
    const first = engine.enqueue(async () => {
      await new Promise((r) => setTimeout(r, 20))
      order.push(1)
    })
    const second = engine.enqueue(async () => {
      order.push(2)
      throw new Error('boom')
    })
    const third = engine.enqueue(async () => void order.push(3))
    await Promise.all([first, second, third])
    expect(order).toEqual([1, 2, 3])
  })

  it('an_append_never_runs_before_its_open', async () => {
    const store = new MemoryStore()
    const slowOpen = store.open.bind(store)
    store.open = async (unit) => {
      await new Promise((r) => setTimeout(r, 20))
      return slowOpen(unit)
    }
    const calls: string[] = []
    const appendOriginal = store.append.bind(store)
    store.append = async (id, line) => {
      calls.push(`append:${(await store.openUnits()).includes(id)}`)
      return appendOriginal(id, line)
    }
    const { deps: d } = deps(store, scripted([open('u1')], [append('u1')]))
    const engine = new Engine(d)
    await Promise.all([engine.handleHookEvent(1, event), engine.handleHookEvent(1, event)])
    expect(calls).toEqual(['append:true'])
  })
})

describe('event time', () => {
  it('a_hook_event_keeps_the_time_it_arrived_not_the_time_its_job_ran', async () => {
    const store = new MemoryStore()
    let clock = 100
    const seen: number[] = []
    const tracker: TrackerLike = {
      onEvent: (_tab, _event, now) => {
        seen.push(now)
        return []
      },
      onTabClosed: () => [],
    }
    const { deps: d } = deps(store, tracker, { now: () => clock })
    const engine = new Engine(d)
    const slow = engine.enqueue(async () => {
      await new Promise((r) => setTimeout(r, 20))
      clock = 5000
    })
    const hook = engine.handleHookEvent(1, event)
    await Promise.all([slow, hook])
    expect(seen).toEqual([100])
  })
})

describe('store errors', () => {
  it('many_failed_actions_in_one_job_record_one_error', async () => {
    const store = new MemoryStore()
    store.append = async () => {
      throw new Error('full')
    }
    const { deps: d, storeErrors } = deps(store, scripted([open('u1'), append('u1'), append('u1'), append('u1')]))
    await new Engine(d).handleHookEvent(1, event)
    expect(storeErrors).toEqual([{ message: 'append: full', at: 5 }])
  })
})

describe('startUp', () => {
  it('recovery_runs_once_though_called_concurrently', async () => {
    const store = new MemoryStore()
    let recoveries = 0
    const original = store.openUnits.bind(store)
    store.openUnits = async () => {
      recoveries += 1
      return original()
    }
    const { deps: d } = deps(store, scripted())
    const engine = new Engine(d)
    await Promise.all([engine.startUp(), engine.startUp(), engine.startUp()])
    expect(recoveries).toBe(1)
  })
})

describe('deliver', () => {
  it('a_request_during_a_pass_is_coalesced_into_one_more_pass_not_lost', async () => {
    const store = new MemoryStore()
    await store.open({ unitId: 'u1', unitKind: 'session', tabId: 1, startedAt: 1 })
    await store.append('u1', JSON.stringify({ at: 1, src: 'rest', dir: 'in', url: '/x', text: 'a' }))
    await store.markClosed('u1')
    let passes = 0
    let active = 0
    let maxActive = 0
    const original = store.closedUnits.bind(store)
    store.closedUnits = async () => {
      passes += 1
      active += 1
      maxActive = Math.max(maxActive, active)
      await new Promise((r) => setTimeout(r, 20))
      active -= 1
      return original()
    }
    const { deps: d } = deps(store, scripted())
    const engine = new Engine(d)
    const first = engine.deliver()
    await new Promise((r) => setTimeout(r, 5))
    await engine.deliver()
    await engine.deliver()
    await first
    expect(maxActive).toBe(1)
    expect(passes).toBe(2)
  })

  it('a_request_during_a_pass_that_throws_still_runs_another_pass', async () => {
    const store = new MemoryStore()
    let passes = 0
    store.closedUnits = async () => {
      passes += 1
      await new Promise((r) => setTimeout(r, 20))
      throw new Error('storage gone')
    }
    const { deps: d } = deps(store, scripted())
    const engine = new Engine(d)
    const first = engine.deliver()
    await new Promise((r) => setTimeout(r, 5))
    await engine.deliver()
    await first
    expect(passes).toBe(2)
  })
})

describe('a failed unit open', () => {
  it('skips_the_later_appends_and_close_and_records_the_error', async () => {
    const store = new MemoryStore()
    store.open = async () => {
      throw new Error('quota')
    }
    const closes: string[] = []
    store.markClosed = async (id) => void closes.push(id)
    let sends = 0
    const { deps: d, storeErrors } = deps(store, scripted([open('u1')], [append('u1'), close('u1')]), {
      send: async () => {
        sends += 1
        return { ok: true }
      },
    })
    const engine = new Engine(d)
    await engine.handleHookEvent(1, event)
    await engine.handleHookEvent(1, event)
    expect(await store.lines('u1')).toEqual([])
    expect(closes).toEqual([])
    expect(sends).toBe(0)
    expect(storeErrors).toEqual([{ message: 'open: quota', at: 5 }])
  })

  it('startup_recovery_deletes_frame_rows_without_a_unit', async () => {
    const store = new MemoryStore()
    await store.append('orphan', 'line')
    await store.open({ unitId: 'u2', unitKind: 'match', tabId: 1, startedAt: 1 })
    await store.append('u2', 'kept')
    await recoverOnStart(store)
    expect(await store.lines('orphan')).toEqual([])
    expect(await store.lines('u2')).toEqual(['kept'])
  })
})

describe('store error clearing', () => {
  it('a_clean_job_after_an_error_clears_it_and_later_clean_jobs_make_no_call', async () => {
    const store = new MemoryStore()
    const realAppend = store.append.bind(store)
    let failNext = true
    store.append = async (id, line) => {
      if (failNext) {
        failNext = false
        throw new Error('full')
      }
      return realAppend(id, line)
    }
    let clears = 0
    const { deps: d, storeErrors } = deps(store, scripted([open('u1'), append('u1')], [append('u1')], [append('u1')], [append('u1')]), {
      clearStoreError: async () => void (clears += 1),
    })
    const engine = new Engine(d)
    await engine.handleHookEvent(1, event)
    expect(storeErrors).toHaveLength(1)
    expect(clears).toBe(0)
    await engine.handleHookEvent(1, event)
    expect(clears).toBe(1)
    await engine.handleHookEvent(1, event)
    await engine.handleHookEvent(1, event)
    expect(clears).toBe(1)
  })
})

describe('pings', () => {
  const unit = async (store: MemoryStore) => {
    await store.open({ unitId: 'u1', unitKind: 'match', tabId: 1, startedAt: 1 })
    await store.append('u1', '{"at":1}')
    await store.markClosed('u1')
  }

  it('pings_on_start_and_after_delivery', async () => {
    const store = new MemoryStore()
    const pings: string[] = []
    const { deps: d } = deps(store, scripted(), { ping: async () => void pings.push('ping') })
    const engine = new Engine(d)
    await engine.startUp()
    expect(pings).toEqual(['ping'])
    await unit(store)
    await engine.deliver()
    expect(pings).toEqual(['ping', 'ping'])
  })

  it('a_failing_ping_never_blocks_delivery_or_throws', async () => {
    const store = new MemoryStore()
    await unit(store)
    const sent: object[] = []
    const { deps: d } = deps(store, scripted(), {
      ping: async () => {
        throw new Error('no helper')
      },
      send: async (msg) => {
        sent.push(msg)
        return { ok: true }
      },
    })
    const engine = new Engine(d)
    await engine.startUp()
    expect(sent.length).toBe(1)
  })
})

describe('startUp ping', () => {
  it('delivers_without_waiting_for_the_ping', async () => {
    const store = new MemoryStore()
    await store.open({ unitId: 'u1', unitKind: 'match', tabId: 1, startedAt: 1 })
    await store.append('u1', '{"at":1}')
    await store.markClosed('u1')
    const sent: object[] = []
    const { deps: d } = deps(store, scripted(), {
      // Never answers.
      ping: () => new Promise(() => {}),
      send: async (msg) => {
        sent.push(msg)
        return { ok: true }
      },
    })
    await new Engine(d).startUp()
    expect(sent.length).toBe(1)
  })
})
