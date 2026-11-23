import { describe, expect, it } from 'vitest'
import { groupByType, typeOf } from './useCardTypeGroups'

describe('groupByType', () => {
  it('orders the groups Creature to Land, then the rest', () => {
    const cards = [
      { name: 'Qland', type: 'Basic Land' },
      { name: 'Qtribal', type: 'Kindred Instant' },
      { name: 'Qart', type: 'Artifact' },
      { name: 'Qdude', type: 'Legendary Creature \u2014 Eldrazi' },
      { name: 'Qodd', type: 'Conspiracy' },
      { name: 'Qnone', type: null },
      { name: 'Qwalker', type: 'Legendary Planeswalker' },
    ]
    const groups = groupByType(cards, (card) => card.type)
    expect(Object.keys(groups)).toEqual(['Creature', 'Planeswalker', 'Instant', 'Artifact', 'Land', 'Conspiracy', 'Other'])
    expect(groups.Creature.map((card) => card.name)).toEqual(['Qdude'])
  })

  it('keeps the callers order within a type', () => {
    const groups = groupByType(
      [
        { name: 'Qb', type: 'Instant' },
        { name: 'Qa', type: 'Instant' },
      ],
      (card) => card.type,
    )
    expect(groups.Instant.map((card) => card.name)).toEqual(['Qb', 'Qa'])
  })

  it('names a card without a type Other, an artifact creature a creature', () => {
    expect(typeOf(null)).toBe('Other')
    expect(typeOf('')).toBe('Other')
    expect(typeOf('Artifact Creature')).toBe('Creature')
  })
})
