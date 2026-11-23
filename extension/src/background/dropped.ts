// The popup's "Skipped" counter. Recording a drop reads the count then writes it back, so every
// change goes through one queue: a reset can never be overwritten by a drop that was in progress.
import type { DropRecord } from './delivery'

export interface CounterStorage {
  get(key: string): Promise<Record<string, unknown>>
  set(items: Record<string, unknown>): Promise<void>
  remove(keys: string[]): Promise<void>
}

export class DroppedCounter {
  private queue: Promise<void> = Promise.resolve()

  constructor(
    private storage: CounterStorage,
    private now: () => number,
  ) {}

  private serial(job: () => Promise<void>): Promise<void> {
    const run = this.queue.then(job)
    // The next job runs even when this one failed; the caller still sees the failure.
    this.queue = run.catch(() => {})
    return run
  }

  record(drop: DropRecord): Promise<void> {
    return this.serial(async () => {
      const { droppedCount } = await this.storage.get('droppedCount')
      const count = typeof droppedCount === 'number' ? droppedCount : 0
      await this.storage.set({ droppedCount: count + 1, lastDrop: { ...drop, at: this.now() } })
    })
  }

  reset(): Promise<void> {
    return this.serial(() => this.storage.remove(['droppedCount', 'lastDrop']))
  }
}
