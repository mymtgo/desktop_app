import { frameLine } from '../capture/format'
import type { HookEvent, UnitAction } from '../capture/units'
import { errorText, recoverOnStart, sendClosedUnits, type DeliverySettings, type DropRecord, type HostReply } from './delivery'
import type { CaptureStore } from './store'

// The tracker is only used through these two calls, so tests can pass a fake.
export interface TrackerLike {
  onEvent(tabId: number, event: HookEvent, now: number): UnitAction[]
  onTabClosed(tabId: number): UnitAction[]
}

export interface EngineDeps {
  store: Promise<CaptureStore | null>
  newTracker: () => TrackerLike
  send: (msg: object) => Promise<HostReply>
  sendParts: (messages: Iterable<object>) => Promise<HostReply>
  captureEnabled: () => Promise<boolean>
  version: { extension: string; browser: string }
  now: () => number
  recordDrop: (drop: DropRecord) => Promise<void>
  recordStoreError: (error: { message: string; at: number }) => Promise<void>
  clearStoreError: () => Promise<void>
  recordDelivered: (at: number) => Promise<void>
  // Tells the helper the extension is alive (it writes last_seen.json). Failures are swallowed here.
  ping: () => Promise<unknown>
}

// The background script's logic without any chrome wiring: the store queue, startup recovery
// and the delivery passes.
export class Engine {
  private tracker: TrackerLike
  // Units whose `open` failed: their later appends and close are skipped, so no frame row is
  // written without a unit row.
  private failedUnits = new Set<string>()
  // One queue for every store change, so they run strictly in arrival order
  // (an append never runs before its unit's open). The returned promise never rejects.
  private queue: Promise<void> = Promise.resolve()
  private delivering = false
  private deliverAgain = false
  private started: Promise<void> | null = null
  // Whether a store error may be persisted. Starts true because the worker may have been
  // restarted since one was written; the first clean job clears it, later ones make no call.
  private storeErrorRecorded = true

  constructor(private deps: EngineDeps) {
    this.tracker = deps.newTracker()
  }

  enqueue(job: () => Promise<void>): Promise<void> {
    this.queue = this.queue.then(job).catch((error: unknown) => console.warn('mymtgo-endstep: store job failed', error))
    return this.queue
  }

  // Applies the tracker's actions in order. Returns whether any unit was closed.
  private async applyActions(store: CaptureStore, actions: UnitAction[]): Promise<boolean> {
    let closed = false
    let lastError: { message: string; at: number } | null = null
    for (const action of actions) {
      try {
        if (action.kind === 'open') {
          const { unitId, unitKind, tabId, at } = action
          try {
            await store.open({ unitId, unitKind, tabId, startedAt: at })
          } catch (error) {
            this.failedUnits.add(unitId)
            throw error
          }
        } else if (action.kind === 'append') {
          if (this.failedUnits.has(action.unitId)) continue
          await store.append(action.unitId, frameLine(action.frame))
        } else {
          if (this.failedUnits.delete(action.unitId)) continue
          await store.markClosed(action.unitId)
          closed = true
        }
      } catch (error) {
        console.warn('mymtgo-endstep: could not store', action.kind, error)
        lastError = { message: `${action.kind}: ${errorText(error)}`, at: this.deps.now() }
      }
    }
    // At most one storage write per job, so a store that fails every append stays cheap.
    if (lastError) {
      this.storeErrorRecorded = true
      await this.deps.recordStoreError(lastError).catch(() => {})
    } else if (actions.length > 0 && this.storeErrorRecorded) {
      this.storeErrorRecorded = false
      await this.deps.clearStoreError().catch(() => {
        this.storeErrorRecorded = true
      })
    }
    return closed
  }

  // `now` is read on arrival, before the queue, so a slow job ahead does not delay event times.
  handleHookEvent(tabId: number, event: HookEvent, now: number = this.deps.now()): Promise<void> {
    return this.enqueue(async () => {
      const store = await this.deps.store
      if (!store || !(await this.deps.captureEnabled())) return
      const closed = await this.applyActions(store, this.tracker.onEvent(tabId, event, now))
      if (closed) void this.deliver()
    })
  }

  handleTabClosed(tabId: number): Promise<void> {
    return this.enqueue(async () => {
      const store = await this.deps.store
      if (!store) return
      const closed = await this.applyActions(store, this.tracker.onTabClosed(tabId))
      if (closed) void this.deliver()
    })
  }

  // One delivery pass at a time; a request during a pass runs one more pass after it.
  async deliver(): Promise<void> {
    if (this.delivering) {
      this.deliverAgain = true
      return
    }
    this.delivering = true
    try {
      // A failed pass must not lose a request that arrived during it, so the catch is inside the loop.
      do {
        this.deliverAgain = false
        try {
          const store = await this.deps.store
          if (!store) return
          const settings: DeliverySettings = { ...this.deps.version }
          const transport = { send: this.deps.send, sendParts: this.deps.sendParts }
          const report = await sendClosedUnits(store, transport, settings, this.deps.recordDrop)
          if (report.sent.length > 0) {
            await this.deps.recordDelivered(this.deps.now())
            void this.ping()
          }
        } catch (error) {
          console.warn('mymtgo-endstep: delivery failed', error)
        }
      } while (this.deliverAgain)
    } finally {
      this.delivering = false
    }
  }

  // A missing helper must never block delivery or throw out of the engine.
  private async ping(): Promise<void> {
    try {
      await this.deps.ping()
    } catch (error) {
      console.warn('mymtgo-endstep: ping failed', error)
    }
  }

  // Closes units a crash or restart left open, then sends. Runs once per background script load:
  // queued before any message, so it never closes a unit opened in this lifetime.
  startUp(): Promise<void> {
    this.started ??= this.enqueue(async () => {
      const store = await this.deps.store
      if (store) await recoverOnStart(store)
    })
      .then(() => {
        // The helper may be slow or missing: the ping runs alongside delivery, never before it.
        void this.ping()
        return this.deliver()
      })
    return this.started
  }

  // Capture switched off: forget the tracker's units and close them in the store.
  stopCapture(): Promise<void> {
    return this.enqueue(async () => {
      this.tracker = this.deps.newTracker()
      this.failedUnits.clear()
      const store = await this.deps.store
      if (store) await recoverOnStart(store)
    }).then(() => void this.deliver())
  }
}
