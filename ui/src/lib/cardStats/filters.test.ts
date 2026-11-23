import { describe, expect, it } from 'vitest'
import { ALL, DEFAULT_FILTERS, effectiveFilters, filtersDiffer, filtersToQuery } from './filters'

describe('filtersToQuery', () => {
  it('sends no filter for the all values', () => {
    expect(filtersToQuery(DEFAULT_FILTERS, 'alltime')).toEqual({
      timeframe: 'alltime',
      board: null,
      playDraw: null,
      archetype: null,
      perspective: 'mine',
    })
  })

  it('sends the chosen board, side, archetype and perspective', () => {
    const query = filtersToQuery(
      { perspective: 'theirs', board: 'postboard', playDraw: 'draw', archetype: '7' },
      'week',
    )
    expect(query).toEqual({ timeframe: 'week', board: 'postboard', playDraw: 'draw', archetype: 7, perspective: 'theirs' })
  })

  it('drops an archetype that is not a number', () => {
    expect(filtersToQuery({ ...DEFAULT_FILTERS, archetype: 'x' }, 'alltime').archetype).toBeNull()
    expect(ALL).toBe('__all__')
  })
})

describe('filtersDiffer', () => {
  it('is false for the defaults', () => {
    expect(filtersDiffer(DEFAULT_FILTERS)).toBe(false)
  })

  it('is true for any filter off its default, so the bar stays to undo it', () => {
    expect(filtersDiffer({ ...DEFAULT_FILTERS, perspective: 'theirs' })).toBe(true)
    expect(filtersDiffer({ ...DEFAULT_FILTERS, board: 'game1' })).toBe(true)
    expect(filtersDiffer({ ...DEFAULT_FILTERS, playDraw: 'draw' })).toBe(true)
    expect(filtersDiffer({ ...DEFAULT_FILTERS, archetype: '12' })).toBe(true)
  })
})

describe('effectiveFilters', () => {
  const chosen = { perspective: 'mine' as const, board: 'postboard', playDraw: 'play', archetype: '7' }

  it('keeps the chosen filters out of Bo1', () => {
    expect(effectiveFilters(chosen, false)).toEqual(chosen)
  })

  it('drops a board chosen on Bo3 in Bo1 (no postboard)', () => {
    expect(effectiveFilters(chosen, true)).toEqual({ ...chosen, board: ALL })
  })
})
