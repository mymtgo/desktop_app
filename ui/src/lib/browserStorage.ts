/**
 * The window's remembered preferences in `localStorage`. Storage can be
 * blocked or full: a read then finds nothing and a write only loses the
 * preference, never an error.
 */
export type KeyValueStorage = Pick<Storage, 'getItem' | 'setItem'>

export function readStored(storage: KeyValueStorage, key: string): string | null {
  try {
    return storage.getItem(key)
  } catch {
    return null
  }
}

export function writeStored(storage: KeyValueStorage, key: string, value: string): void {
  try {
    storage.setItem(key, value)
  } catch {
    // A full or blocked store only loses the preference.
  }
}

/** The stored JSON value, `null` when missing or junk. */
export function readStoredJson(storage: KeyValueStorage, key: string): unknown {
  const raw = readStored(storage, key)
  if (raw === null) return null
  try {
    return JSON.parse(raw) as unknown
  } catch {
    return null
  }
}

export function writeStoredJson(storage: KeyValueStorage, key: string, value: unknown): void {
  writeStored(storage, key, JSON.stringify(value))
}
