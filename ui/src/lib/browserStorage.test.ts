import { describe, expect, it } from 'vitest'
import { readStored, readStoredJson, writeStored, writeStoredJson, type KeyValueStorage } from './browserStorage'

function memory(): KeyValueStorage & { data: Map<string, string> } {
  const data = new Map<string, string>()
  return {
    data,
    getItem: (key) => data.get(key) ?? null,
    setItem: (key, value) => void data.set(key, value),
  }
}

/** A store that refuses everything (blocked, or full). */
const blocked: KeyValueStorage = {
  getItem: () => {
    throw new Error('blocked')
  },
  setItem: () => {
    throw new Error('quota exceeded')
  },
}

describe('browser storage', () => {
  it('writes and reads text', () => {
    const storage = memory()
    expect(readStored(storage, 'k')).toBeNull()
    writeStored(storage, 'k', 'grid')
    expect(readStored(storage, 'k')).toBe('grid')
  })

  it('writes and reads JSON, junk reading as null', () => {
    const storage = memory()
    expect(readStoredJson(storage, 'k')).toBeNull()
    writeStoredJson(storage, 'k', { a: 1 })
    expect(storage.data.get('k')).toBe('{"a":1}')
    expect(readStoredJson(storage, 'k')).toEqual({ a: 1 })
    storage.data.set('k', '{not json')
    expect(readStoredJson(storage, 'k')).toBeNull()
  })

  it('treats a blocked or full store as empty and never throws', () => {
    expect(readStored(blocked, 'k')).toBeNull()
    expect(readStoredJson(blocked, 'k')).toBeNull()
    expect(() => writeStored(blocked, 'k', 'v')).not.toThrow()
    expect(() => writeStoredJson(blocked, 'k', { a: 1 })).not.toThrow()
  })
})
