import { describe, expect, it } from 'vitest'
import { archiveTabLabel, deckCell, deletedResultText, recordText, resultTone } from './archives'

describe('the Archives tables', () => {
  it('reads a match result as a word', () => {
    expect(deletedResultText('win')).toBe('Win')
    expect(deletedResultText('loss')).toBe('Loss')
    expect(deletedResultText('draw')).toBe('Draw')
    expect(deletedResultText('unknown')).toBe('-')
  })

  it('colours a match result, unknown muted', () => {
    expect(resultTone('win')).toBe('win')
    expect(resultTone('loss')).toBe('loss')
    expect(resultTone('draw')).toBe('draw')
    expect(resultTone('unknown')).toBe('muted')
  })

  it('writes a league record as wins-losses', () => {
    expect(recordText(4, 1)).toBe('4-1')
  })

  it('puts the count in a tab label', () => {
    expect(archiveTabLabel('Matches', 3)).toBe('Matches (3)')
    expect(archiveTabLabel('Leagues', 0)).toBe('Leagues (0)')
  })

  it('shows the deck, else the format muted, else a muted dash', () => {
    expect(deckCell('Qburn', 'Modern')).toEqual({ text: 'Qburn', muted: false })
    expect(deckCell(null, 'Modern')).toEqual({ text: 'Modern', muted: true })
    expect(deckCell(null, null)).toEqual({ text: '-', muted: true })
  })
})
