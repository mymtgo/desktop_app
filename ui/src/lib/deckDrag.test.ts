import { describe, expect, it } from 'vitest'
import { DECK_DRAG_TYPE, acceptsDrag, deckDragLabel, dragIds, hasDeckDrag, readDeckDrag, writeDeckDrag } from './deckDrag'

/** A DataTransfer stand-in: jsdom has none. */
function transfer(): DataTransfer {
  const data = new Map<string, string>()
  return {
    effectAllowed: 'none',
    get types() {
      return [...data.keys()]
    },
    setData: (type: string, value: string) => void data.set(type, value),
    getData: (type: string) => data.get(type) ?? '',
  } as unknown as DataTransfer
}

describe('dragging decks onto archetype rows (0.x lib/deckDrag)', () => {
  it('carries the deck ids under a private type', () => {
    const moving = transfer()
    writeDeckDrag(moving, [3, 5])
    expect(moving.types).toEqual([DECK_DRAG_TYPE])
    expect(moving.effectAllowed).toBe('move')
    expect(hasDeckDrag(moving)).toBe(true)
    expect(readDeckDrag(moving)).toEqual([3, 5])
  })

  it('ignores other drags and broken payloads', () => {
    const text = transfer()
    text.setData('text/plain', 'Qtext')
    expect(hasDeckDrag(text)).toBe(false)
    expect(hasDeckDrag(null)).toBe(false)
    expect(readDeckDrag(text)).toEqual([])
    const broken = transfer()
    broken.setData(DECK_DRAG_TYPE, '[1, "x", 2')
    expect(readDeckDrag(broken)).toEqual([])
    const mixed = transfer()
    mixed.setData(DECK_DRAG_TYPE, '[1, "x", 2]')
    expect(readDeckDrag(mixed)).toEqual([1, 2])
    expect(readDeckDrag(null)).toEqual([])
  })

  it('drags the whole selection from a selected card, else the one deck', () => {
    expect(dragIds(4, true, [2, 4])).toEqual([2, 4])
    expect(dragIds(4, false, [2])).toEqual([4])
    expect(deckDragLabel(2, 'Qburn')).toBe('2 decks')
    expect(deckDragLabel(1, 'Qburn')).toBe('Qburn')
  })

  it('takes a drop only on a row of the dragged format; unclassified and Other take any', () => {
    expect(acceptsDrag(null, 'modern')).toBe(true)
    expect(acceptsDrag('CMODERN', null)).toBe(true)
    expect(acceptsDrag('CMODERN', 'modern')).toBe(true)
    expect(acceptsDrag('CPAUPER', 'modern')).toBe(false)
  })
})
