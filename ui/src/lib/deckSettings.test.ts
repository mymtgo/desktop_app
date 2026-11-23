import { describe, expect, it } from 'vitest'
import {
  DECK_NAME_INPUT_MAX,
  IDENTITY_OPTIONS,
  canArchive,
  clampDeckName,
  coverAfterOptions,
  coverSettingsKey,
  deckNameProblem,
  identityChanged,
  orderIdentity,
  renameTarget,
  toggleIdentity,
} from './deckSettings'

describe('deck name', () => {
  it('needs 1 to 255 characters after trimming', () => {
    const message = 'A deck name needs 1 to 255 characters.'
    expect(deckNameProblem('Qburn')).toBeNull()
    expect(deckNameProblem('  Qburn  ')).toBeNull()
    expect(deckNameProblem('')).toBe(message)
    expect(deckNameProblem('   ')).toBe(message)
    expect(deckNameProblem('q'.repeat(255))).toBeNull()
    expect(deckNameProblem('q'.repeat(256))).toBe(message)
  })

  it('counts characters, not UTF-16 units, as the app does', () => {
    expect(deckNameProblem('\u{1F525}'.repeat(255))).toBeNull()
    expect(deckNameProblem('\u{1F525}'.repeat(256))).toBe('A deck name needs 1 to 255 characters.')
  })

  it('clamps typing to 255 characters, counting an astral character once', () => {
    expect(clampDeckName('Qburn')).toBe('Qburn')
    expect(clampDeckName('q'.repeat(256))).toBe('q'.repeat(255))
    const fire = '\u{1F525}'.repeat(255)
    expect(clampDeckName(fire)).toBe(fire)
    expect(clampDeckName(fire + 'q')).toBe(fire)
  })

  it('lets an input hold the longest valid name in UTF-16 units', () => {
    expect('\u{1F525}'.repeat(255).length).toBeLessThanOrEqual(DECK_NAME_INPUT_MAX)
  })

  it('renames only to a valid name that differs from the current one', () => {
    expect(renameTarget('  Qtempo  ', 'Qburn')).toBe('Qtempo')
    expect(renameTarget('Qburn', 'Qburn')).toBeNull()
    expect(renameTarget(' Qburn ', 'Qburn')).toBeNull()
    expect(renameTarget('   ', 'Qburn')).toBeNull()
    expect(renameTarget('q'.repeat(256), 'Qburn')).toBeNull()
  })
})

describe('colour identity', () => {
  it('offers the five colours and colourless in 0.x order', () => {
    expect(IDENTITY_OPTIONS.map((option) => option.key)).toEqual(['W', 'U', 'B', 'R', 'G', 'C'])
    expect(IDENTITY_OPTIONS.map((option) => option.label)).toEqual([
      'White',
      'Blue',
      'Black',
      'Red',
      'Green',
      'Colorless',
    ])
  })

  it('orders letters WUBRGC without repeats', () => {
    expect(orderIdentity(['R', 'U', 'R', 'C', 'W'])).toEqual(['W', 'U', 'R', 'C'])
    expect(orderIdentity([])).toEqual([])
  })

  it('toggles a colour on or off and keeps the order', () => {
    expect(toggleIdentity(['R'], 'U')).toEqual(['U', 'R'])
    expect(toggleIdentity(['U', 'R'], 'U')).toEqual(['R'])
    expect(toggleIdentity([], 'C')).toEqual(['C'])
    expect(toggleIdentity(['G', 'W'], 'B')).toEqual(['W', 'B', 'G'])
  })

  it('is changed only when the set of colours differs', () => {
    expect(identityChanged(['U', 'R'], ['U', 'R'])).toBe(false)
    expect(identityChanged(['U', 'R'], ['R', 'U'])).toBe(false)
    expect(identityChanged(['U'], ['U', 'R'])).toBe(true)
    expect(identityChanged([], [])).toBe(false)
    expect(identityChanged(['C'], [])).toBe(true)
  })
})

describe('archive confirmation', () => {
  it('needs DELETE, trimmed, in capitals', () => {
    expect(canArchive('DELETE')).toBe(true)
    expect(canArchive('  DELETE ')).toBe(true)
    expect(canArchive('delete')).toBe(false)
    expect(canArchive('DELET')).toBe(false)
    expect(canArchive('')).toBe(false)
  })
})

describe('cover printing', () => {
  const option = (catalogId: number) => ({ catalogId, setName: null, setCode: null, artCrop: `https://q/${catalogId}.jpg`, localArtCrop: null })

  it('picks the only printing there is', () => {
    expect(coverAfterOptions([option(7)], null)).toBe(7)
    expect(coverAfterOptions([option(7)], 3)).toBe(7)
  })

  it('keeps the chosen printing when the name still has it, else none', () => {
    expect(coverAfterOptions([option(1), option(2)], 2)).toBe(2)
    expect(coverAfterOptions([option(1), option(2)], 3)).toBeNull()
    expect(coverAfterOptions([option(1), option(2)], null)).toBeNull()
    expect(coverAfterOptions([], 2)).toBeNull()
  })
})

describe('cover settings key', () => {
  it('sits under the deck queries, so deck changes refresh it', () => {
    expect(coverSettingsKey(5)).toEqual(['decks', 'cover-settings', 5])
  })
})
