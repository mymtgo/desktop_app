import { describe, expect, it } from 'vitest'
import {
  ALL_ENABLED,
  FILTER_KEYS,
  loadTypeFilters,
  passesTypeFilter,
  presentFilterKeys,
  reenableVanished,
  TYPE_FILTER_STORAGE_KEY,
} from './typeFilter'

const stat = (type: string | null, isSideboard = false) => ({ type, isSideboard })

describe('filter keys', () => {
  it('lists the types then Sideboard', () => {
    expect(FILTER_KEYS).toEqual([
      'Creature',
      'Instant',
      'Sorcery',
      'Enchantment',
      'Artifact',
      'Land',
      'Planeswalker',
      'Battle',
      'Sideboard',
    ])
    expect(TYPE_FILTER_STORAGE_KEY).toBe('cardStatsTypeFilters')
  })
})

describe('passesTypeFilter', () => {
  it('lets a sideboard card follow only the Sideboard toggle', () => {
    const noCreatures = { ...ALL_ENABLED, Creature: false }
    expect(passesTypeFilter(stat('Creature', true), noCreatures)).toBe(true)
    expect(passesTypeFilter(stat('Creature', true), { ...ALL_ENABLED, Sideboard: false })).toBe(false)
  })

  it('hides a maindeck card whose category is off', () => {
    expect(passesTypeFilter(stat('Artifact Creature'), { ...ALL_ENABLED, Creature: false })).toBe(false)
    expect(passesTypeFilter(stat('Artifact Creature'), { ...ALL_ENABLED, Artifact: false })).toBe(true)
  })

  it('never hides a card with no recognised type', () => {
    const none = Object.fromEntries(FILTER_KEYS.map((key) => [key, false])) as typeof ALL_ENABLED
    expect(passesTypeFilter(stat(null), none)).toBe(true)
    expect(passesTypeFilter(stat('Token'), none)).toBe(true)
  })
})

describe('presentFilterKeys', () => {
  it('lists only the types present, with Sideboard for a sideboard card', () => {
    const present = presentFilterKeys([stat('Basic Land'), stat('Instant', true), stat(null)])
    expect(present).toEqual(new Set(['Land', 'Instant', 'Sideboard']))
  })
})

describe('reenableVanished', () => {
  it('re-enables a hidden type that is no longer present', () => {
    const filters = { ...ALL_ENABLED, Land: false, Creature: false }
    const next = reenableVanished(filters, new Set(['Creature']))
    expect(next).toEqual({ ...ALL_ENABLED, Creature: false })
  })

  it('returns null when nothing changes', () => {
    expect(reenableVanished({ ...ALL_ENABLED, Creature: false }, new Set(['Creature']))).toBeNull()
  })
})

describe('loadTypeFilters', () => {
  const store = (value: string | null) => ({ getItem: () => value, setItem: () => {} })

  it('defaults to every type on', () => {
    expect(loadTypeFilters(store(null))).toEqual(ALL_ENABLED)
  })

  it('uses stored filters only when every key is present', () => {
    const stored = { ...ALL_ENABLED, Land: false }
    expect(loadTypeFilters(store(JSON.stringify(stored)))).toEqual(stored)
    expect(loadTypeFilters(store(JSON.stringify({ Land: false })))).toEqual(ALL_ENABLED)
    expect(loadTypeFilters(store('{broken'))).toEqual(ALL_ENABLED)
  })
})
