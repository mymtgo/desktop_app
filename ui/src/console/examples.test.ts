import { describe, expect, it } from 'vitest'
import { examples } from './examples'

describe('examples', () => {
  it('read only the v_ views', () => {
    for (const example of examples) {
      const names = [...example.sql.matchAll(/\b(?:FROM|JOIN)\s+([A-Za-z_]\w*)/gi)].map((match) => match[1])
      expect(names.length, example.name).toBeGreaterThan(0)
      for (const name of names) expect(name, example.name).toMatch(/^v_/)
    }
  })

  it('have unique names', () => {
    const names = examples.map((example) => example.name)
    expect(new Set(names).size).toBe(names.length)
  })

  it('include the two comparisons', () => {
    const archetypes = examples.find((example) => example.name === 'Compare card stats between two archetypes')
    expect(archetypes?.sql).toContain(':first_archetype')
    expect(archetypes?.sql).toContain(':second_archetype')
    const decks = examples.find((example) => example.name === 'Compare card stats between two decks')
    expect(decks?.sql).toContain(':first_deck')
    expect(decks?.sql).toContain(':second_deck')
    expect(decks?.sql).toContain('GROUP BY card_name, deck_name')
  })
})
