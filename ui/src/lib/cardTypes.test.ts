import { describe, expect, it } from 'vitest'
import { CANONICAL_TYPES, normalizeType, TYPE_ORDER } from './cardTypes'

describe('normalizeType', () => {
  it('keeps a canonical type', () => {
    for (const type of CANONICAL_TYPES) {
      expect(normalizeType(type)).toBe(type)
    }
  })

  it('takes the first canonical type a type line contains', () => {
    expect(normalizeType('Artifact Creature')).toBe('Creature')
    expect(normalizeType('Legendary Planeswalker')).toBe('Planeswalker')
    expect(normalizeType('Kindred Instant')).toBe('Instant')
    expect(normalizeType('Enchantment Land')).toBe('Enchantment')
    expect(normalizeType('Basic Land')).toBe('Land')
  })

  it('leaves any other type as it is', () => {
    expect(normalizeType('Unknown')).toBe('Unknown')
    expect(normalizeType('Conspiracy')).toBe('Conspiracy')
  })
})

describe('TYPE_ORDER', () => {
  it('orders the types as 0.x lists them', () => {
    expect(CANONICAL_TYPES).toEqual(['Creature', 'Planeswalker', 'Battle', 'Instant', 'Sorcery', 'Enchantment', 'Artifact', 'Land'])
    expect(TYPE_ORDER.Creature).toBe(0)
    expect(TYPE_ORDER.Land).toBe(7)
    expect(TYPE_ORDER.Unknown).toBeUndefined()
  })
})
