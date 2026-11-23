import { DB_VERSION, upgradeSchema } from './idbSchema'
import type { CaptureStore, NewUnit, StoredUnit } from './store'

// IndexedDB store, opened only by the background script (spec 4.5).
// The object stores and their versions are described in idbSchema.ts.
const DB_NAME = 'mymtgo-endstep'
const COUNTER_KEY = 'counter'

// Only the stripped line is stored, never the raw frame (spec 4.4).
interface FrameRow {
  unitId: string
  line: string
}

function done<T>(request: IDBRequest<T>): Promise<T> {
  return new Promise((resolve, reject) => {
    request.onsuccess = () => resolve(request.result)
    request.onerror = () => reject(request.error)
  })
}

function committed(tx: IDBTransaction): Promise<void> {
  const finished = new Promise<void>((resolve, reject) => {
    tx.oncomplete = () => resolve()
    tx.onerror = () => reject(tx.error)
    tx.onabort = () => reject(tx.error ?? new Error('transaction aborted'))
  })
  // A failed request inside the transaction is reported by that request; this keeps the
  // transaction's own rejection from also surfacing as unhandled. Awaiting it still throws.
  finished.catch(() => {})
  return finished
}

export class IdbStore implements CaptureStore {
  private constructor(private db: IDBDatabase) {}

  // Rejects when IndexedDB cannot be opened (Firefox "never remember history" mode).
  static open(): Promise<IdbStore> {
    return new Promise((resolve, reject) => {
      let request: IDBOpenDBRequest
      try {
        request = indexedDB.open(DB_NAME, DB_VERSION)
      } catch (error) {
        reject(error)
        return
      }
      // The versioning rules and steps are in idbSchema.ts.
      request.onupgradeneeded = (event) => {
        const db = request.result
        upgradeSchema(
          {
            createStore(name, options, indexes = []) {
              const store = db.createObjectStore(name, options)
              for (const index of indexes) store.createIndex(index, index)
            },
          },
          event.oldVersion,
          event.newVersion ?? DB_VERSION,
        )
      }
      request.onsuccess = () => resolve(new IdbStore(request.result))
      request.onerror = () => reject(request.error ?? new Error('IndexedDB failed to open'))
      request.onblocked = () => reject(new Error('IndexedDB open was blocked'))
    })
  }

  async open(unit: NewUnit): Promise<number> {
    const tx = this.db.transaction(['units', 'meta'], 'readwrite')
    const finished = committed(tx)
    const meta = tx.objectStore('meta')
    const previous = await done(meta.get(COUNTER_KEY))
    const counter = (typeof previous === 'number' ? previous : 0) + 1
    meta.put(counter, COUNTER_KEY)
    const stored: StoredUnit = { ...unit, counter, closed: false }
    tx.objectStore('units').put(stored)
    await finished
    return counter
  }

  async append(unitId: string, line: string): Promise<void> {
    const tx = this.db.transaction('frames', 'readwrite')
    const row: FrameRow = { unitId, line }
    tx.objectStore('frames').add(row)
    await committed(tx)
  }

  async markClosed(unitId: string): Promise<void> {
    await this.updateUnit(unitId, (unit) => {
      unit.closed = true
    })
  }

  async setError(unitId: string, error: string): Promise<void> {
    await this.updateUnit(unitId, (unit) => {
      unit.error = error
    })
  }

  async openUnits(kind?: NewUnit['unitKind']): Promise<string[]> {
    const units = await this.allUnits()
    return units.filter((u) => !u.closed && (kind === undefined || u.unitKind === kind)).map((u) => u.unitId)
  }

  async closedUnits(): Promise<StoredUnit[]> {
    const units = await this.allUnits()
    return units.filter((u) => u.closed)
  }

  async lines(unitId: string): Promise<string[]> {
    const rows = await this.rows(unitId)
    return rows.map((r) => r.line)
  }

  async remove(unitId: string): Promise<void> {
    const tx = this.db.transaction(['units', 'frames'], 'readwrite')
    const finished = committed(tx)
    tx.objectStore('units').delete(unitId)
    const keys = await done(tx.objectStore('frames').index('unitId').getAllKeys(unitId))
    const frames = tx.objectStore('frames')
    for (const key of keys) frames.delete(key)
    await finished
  }

  async removeOrphanFrames(): Promise<void> {
    const tx = this.db.transaction(['units', 'frames'], 'readwrite')
    const finished = committed(tx)
    const units = tx.objectStore('units')
    const frames = tx.objectStore('frames')
    const index = frames.index('unitId')
    // Walk distinct unit ids (not every row); one request serves every step of a key cursor.
    const ids: string[] = []
    const request = index.openKeyCursor(null, 'nextunique')
    let cursor = await done(request)
    while (cursor) {
      ids.push(cursor.key as string)
      const step = done(request)
      cursor.continue()
      cursor = await step
    }
    for (const unitId of ids) {
      if ((await done(units.getKey(unitId))) !== undefined) continue
      for (const key of await done(index.getAllKeys(unitId))) frames.delete(key)
    }
    await finished
  }

  private async allUnits(): Promise<StoredUnit[]> {
    const tx = this.db.transaction('units', 'readonly')
    return (await done(tx.objectStore('units').getAll())) as StoredUnit[]
  }

  // Rows sorted by index key, then primary key: the frames of one unit in arrival order.
  private async rows(unitId: string): Promise<FrameRow[]> {
    const tx = this.db.transaction('frames', 'readonly')
    return (await done(tx.objectStore('frames').index('unitId').getAll(unitId))) as FrameRow[]
  }

  private async updateUnit(unitId: string, change: (unit: StoredUnit) => void): Promise<void> {
    const tx = this.db.transaction('units', 'readwrite')
    const finished = committed(tx)
    const units = tx.objectStore('units')
    const unit = (await done(units.get(unitId))) as StoredUnit | undefined
    if (unit) {
      change(unit)
      units.put(unit)
    }
    await finished
  }
}
