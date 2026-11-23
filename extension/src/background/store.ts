export interface NewUnit {
  unitId: string
  unitKind: 'match' | 'session'
  tabId: number
  startedAt: number
}

export interface StoredUnit extends NewUnit {
  counter: number
  closed: boolean
  error?: string
}

// Where units live until the helper confirms they are on disk.
export interface CaptureStore {
  // Stores a new open unit and returns its capture counter (one higher per open, per browser profile).
  open(unit: NewUnit): Promise<number>
  // Stores one stripped frame line (frameLine output). Raw frames are never stored (spec 4.4).
  append(unitId: string, line: string): Promise<void>
  markClosed(unitId: string): Promise<void>
  // Open units, only those of `kind` when given.
  openUnits(kind?: NewUnit['unitKind']): Promise<string[]>
  closedUnits(): Promise<StoredUnit[]>
  lines(unitId: string): Promise<string[]>
  setError(unitId: string, error: string): Promise<void>
  remove(unitId: string): Promise<void>
  // Deletes frame rows whose unit row does not exist.
  removeOrphanFrames(): Promise<void>
}

// In-memory store for tests.
export class MemoryStore implements CaptureStore {
  private counter = 0
  private units = new Map<string, StoredUnit>()
  private rows: { unitId: string; line: string }[] = []

  async open(unit: NewUnit): Promise<number> {
    this.counter += 1
    this.units.set(unit.unitId, { ...unit, counter: this.counter, closed: false })
    return this.counter
  }

  async append(unitId: string, line: string): Promise<void> {
    this.rows.push({ unitId, line })
  }

  async markClosed(unitId: string): Promise<void> {
    const unit = this.units.get(unitId)
    if (unit) unit.closed = true
  }

  async openUnits(kind?: NewUnit['unitKind']): Promise<string[]> {
    return [...this.units.values()]
      .filter((u) => !u.closed && (kind === undefined || u.unitKind === kind))
      .map((u) => u.unitId)
  }

  async closedUnits(): Promise<StoredUnit[]> {
    return [...this.units.values()].filter((u) => u.closed).map((u) => ({ ...u }))
  }

  async lines(unitId: string): Promise<string[]> {
    return this.rows.filter((r) => r.unitId === unitId).map((r) => r.line)
  }

  async setError(unitId: string, error: string): Promise<void> {
    const unit = this.units.get(unitId)
    if (unit) unit.error = error
  }

  async remove(unitId: string): Promise<void> {
    this.units.delete(unitId)
    this.rows = this.rows.filter((r) => r.unitId !== unitId)
  }

  async removeOrphanFrames(): Promise<void> {
    this.rows = this.rows.filter((r) => this.units.has(r.unitId))
  }
}
