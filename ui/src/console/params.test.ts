import { describe, expect, it } from 'vitest'
import { paramValues, pickerKind, pickerSql } from './params'

describe('pickerKind', () => {
  it('names ending in archetype or deck get a picker, any case', () => {
    expect(pickerKind('first_archetype')).toBe('archetype')
    expect(pickerKind('Second_Deck')).toBe('deck')
    expect(pickerKind('format')).toBe('text')
  })
})

describe('pickerSql', () => {
  it('reads each name once from the views, and a text field has none', () => {
    expect(pickerSql('archetype')).toBe("SELECT DISTINCT name FROM v_archetypes WHERE name <> '' ORDER BY name")
    expect(pickerSql('deck')).toBe("SELECT DISTINCT name FROM v_decks WHERE name <> '' ORDER BY name")
    expect(pickerSql('text')).toBeNull()
  })
})

describe('paramValues', () => {
  it('sends every listed name, an empty field as null', () => {
    expect(paramValues(['a', 'b', 'c'], { a: 'Modern', b: '' })).toEqual([
      { name: 'a', value: 'Modern' },
      { name: 'b', value: null },
      { name: 'c', value: null },
    ])
  })
})
