import { describe, expect, it } from 'vitest'
import { hasTitleStrip, isTransparent, overlayView, viewTitle } from './views'

describe('overlayView', () => {
  it('picks the page from ?view=', () => {
    expect(overlayView('?view=game')).toBe('game')
    expect(overlayView('?view=draft-notes')).toBe('draft-notes')
    expect(overlayView('?view=league')).toBe('league')
    expect(overlayView('?other=1&view=league')).toBe('league')
  })

  it('has no page for a missing or unknown view', () => {
    expect(overlayView('')).toBeNull()
    expect(overlayView('?view=debug')).toBeNull()
    expect(overlayView('?view=')).toBeNull()
  })

  it('titles each page as its window', () => {
    expect(viewTitle('game')).toBe('Game overlay')
    expect(viewTitle('draft-notes')).toBe('Draft notes')
    expect(viewTitle('league')).toBe('League Overlay')
  })
})

describe('hasTitleStrip', () => {
  it('leaves the strip off the league window, which the card fills', () => {
    expect(hasTitleStrip('league')).toBe(false)
  })

  it('leaves the strip off the game overlay, whose header drags it (0.x)', () => {
    expect(hasTitleStrip('game')).toBe(false)
    expect(hasTitleStrip('draft-notes')).toBe(true)
    expect(hasTitleStrip(null)).toBe(true)
  })
})

describe('isTransparent', () => {
  it('makes only the league window see-through', () => {
    expect(isTransparent('league')).toBe(true)
    expect(isTransparent('game')).toBe(false)
    expect(isTransparent('draft-notes')).toBe(false)
    expect(isTransparent(null)).toBe(false)
  })
})
