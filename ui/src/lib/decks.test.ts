import { describe, expect, it } from 'vitest'
import type { DeckView, FormatOptionView, RecordView } from '@/bindings'
import {
  ALL_FORMATS,
  activityText,
  deckCover,
  deckFilters,
  formatSelectOptions,
  lastPlayedText,
  manaColorsFor,
  manaWash,
  recentRowState,
  recordParts,
  shownManaSymbols,
  totalDecks,
  winRateTone,
  deckKindLabels,
} from './decks'

function record(overrides: Partial<RecordView> = {}): RecordView {
  return { wins: 3, losses: 1, draws: 0, total: 4, winRate: 75, label: '3 - 1', ...overrides }
}

function deck(overrides: Partial<DeckView> = {}): DeckView {
  return {
    id: 1,
    name: 'Qdeck',
    originalName: null,
    format: 'CMODERN',
    formatLabel: 'Modern',
    colors: ['U', 'R'],
    coverArt: null,
    localCoverArt: null,
    customCoverArt: null,
    archived: false,
    record: record(),
    kinds: [],
    lastPlayedAtMs: null,
    archetype: null,
    commanderFormat: false,
    ...overrides,
  }
}

function option(code: string, label: string, count: number): FormatOptionView {
  return { code, label, count }
}

// Local wall-clock times, so the calendar maths reads the same in every timezone.
const now = new Date(2026, 9, 7, 12, 0, 0).getTime()
const at = (y: number, m: number, d: number, h = 12, min = 0, s = 0) => new Date(y, m - 1, d, h, min, s).getTime()

describe('format labels', () => {
  it('puts an "All" row with every deck counted before the formats', () => {
    const options = [option('CMODERN', 'Modern', 3), option('CPAUPER', 'Pauper', 2)]
    expect(totalDecks(options)).toBe(5)
    expect(formatSelectOptions(options)).toEqual([
      { value: ALL_FORMATS, label: 'All', count: 5 },
      { value: 'CMODERN', label: 'Modern', count: 3 },
      { value: 'CPAUPER', label: 'Pauper', count: 2 },
    ])
  })

  it('counts no decks when there are no formats', () => {
    expect(totalDecks([])).toBe(0)
    expect(formatSelectOptions([])).toEqual([{ value: ALL_FORMATS, label: 'All', count: 0 }])
  })
})

describe('record label', () => {
  it('shows wins and losses as letters', () => {
    expect(recordParts(record())).toEqual([
      { text: '3W', tone: 'win' },
      { text: '1L', tone: 'loss' },
    ])
  })

  it('adds draws only when there are any', () => {
    expect(recordParts(record({ wins: 2, losses: 2, draws: 1, total: 5 }))).toEqual([
      { text: '2W', tone: 'win' },
      { text: '2L', tone: 'loss' },
      { text: '1D', tone: 'draw' },
    ])
  })

  it('tones a win rate: below half is a loss, exactly half is even', () => {
    expect(winRateTone(49)).toBe('loss')
    expect(winRateTone(50)).toBe('even')
    expect(winRateTone(51)).toBe('win')
    expect(winRateTone(0)).toBe('loss')
  })
})

describe('colour wash', () => {
  it('reads each identity letter as its mana colour', () => {
    expect(manaColorsFor(['W', 'u', 'X'])).toEqual(['#F8F6D8', '#C1D7E9'])
  })

  it('is a diagonal gradient with one stop per colour', () => {
    expect(manaWash(['U', 'R'])).toBe('linear-gradient(135deg, rgba(193, 215, 233, 0.28) 0%, rgba(228, 153, 119, 0.28) 100%)')
    expect(manaWash(['W', 'U', 'B'])).toBe(
      'linear-gradient(135deg, rgba(248, 246, 216, 0.28) 0%, rgba(193, 215, 233, 0.28) 50%, rgba(186, 177, 171, 0.28) 100%)',
    )
  })

  it('repeats a single colour and falls back to neutral grey', () => {
    expect(manaWash(['G'])).toBe('linear-gradient(135deg, rgba(163, 192, 149, 0.28) 0%, rgba(163, 192, 149, 0.28) 100%)')
    expect(manaWash([])).toBe('linear-gradient(135deg, rgba(138, 138, 138, 0.28) 0%, rgba(138, 138, 138, 0.28) 100%)')
    expect(manaWash(['G'], 0.5)).toBe('linear-gradient(135deg, rgba(163, 192, 149, 0.5) 0%, rgba(163, 192, 149, 0.5) 100%)')
  })

  it('shows symbols in WUBRG order, colourless when asked for or when there are none', () => {
    expect(shownManaSymbols(['R', 'U'])).toEqual(['U', 'R'])
    expect(shownManaSymbols(['C', 'G'])).toEqual(['G', 'C'])
    expect(shownManaSymbols([])).toEqual(['C'])
  })
})

describe('last played text', () => {
  it('is null when the deck was never played', () => {
    expect(lastPlayedText(null, now)).toBeNull()
  })

  it('counts the largest whole unit, like Carbon diffForHumans', () => {
    expect(lastPlayedText(now, now)).toBe('0 seconds ago')
    expect(lastPlayedText(at(2026, 10, 7, 11, 59, 59), now)).toBe('1 second ago')
    expect(lastPlayedText(at(2026, 10, 7, 11, 59, 1), now)).toBe('59 seconds ago')
    expect(lastPlayedText(at(2026, 10, 7, 11, 59, 0), now)).toBe('1 minute ago')
    expect(lastPlayedText(at(2026, 10, 7, 11, 0, 0), now)).toBe('1 hour ago')
    expect(lastPlayedText(at(2026, 10, 6, 12, 0, 1), now)).toBe('23 hours ago')
    expect(lastPlayedText(at(2026, 10, 6), now)).toBe('1 day ago')
    expect(lastPlayedText(at(2026, 10, 1), now)).toBe('6 days ago')
    expect(lastPlayedText(at(2026, 9, 30), now)).toBe('1 week ago')
    expect(lastPlayedText(at(2026, 9, 8), now)).toBe('4 weeks ago')
    expect(lastPlayedText(at(2026, 9, 7), now)).toBe('1 month ago')
    expect(lastPlayedText(at(2026, 8, 7), now)).toBe('2 months ago')
    expect(lastPlayedText(at(2025, 10, 8), now)).toBe('11 months ago')
    expect(lastPlayedText(at(2025, 10, 7), now)).toBe('1 year ago')
    expect(lastPlayedText(at(2024, 10, 7), now)).toBe('2 years ago')
  })

  it('counts months on the calendar, overflowing short months as PHP does', () => {
    expect(lastPlayedText(at(2026, 1, 31), at(2026, 3, 1))).toBe('4 weeks ago')
    expect(lastPlayedText(at(2026, 2, 28), at(2026, 3, 31))).toBe('1 month ago')
    expect(lastPlayedText(at(2026, 3, 1), at(2026, 3, 31))).toBe('4 weeks ago')
  })

  it('says "from now" for a time ahead of the clock', () => {
    expect(lastPlayedText(at(2026, 10, 7, 12, 0, 5), now)).toBe('5 seconds from now')
    expect(lastPlayedText(at(2026, 10, 9), now)).toBe('2 days from now')
  })

  it('ignores milliseconds', () => {
    expect(lastPlayedText(at(2026, 10, 7, 11, 59, 0), now + 999)).toBe('1 minute ago')
  })
})

describe('activity text', () => {
  it('shows the match count and when the deck was last played', () => {
    expect(activityText(deck({ lastPlayedAtMs: at(2026, 10, 6) }), now, 'never played')).toBe('4 matches · 1 day ago')
    expect(activityText(deck(), now, 'never played')).toBe('4 matches · never played')
    expect(activityText(deck(), now, 'never')).toBe('4 matches · never')
  })

  it('leaves out last played for an archived deck', () => {
    expect(activityText(deck({ archived: true, lastPlayedAtMs: at(2026, 10, 6) }), now, 'never')).toBe('4 matches')
  })
})

describe('remembered filters', () => {
  it('open on every format and the recently played decks', () => {
    expect(deckFilters.format).toBeNull()
    expect(deckFilters.recent).toBe(true)
  })
})

describe('recently played row', () => {
  it('is on or off as asked while no list says otherwise', () => {
    expect(recentRowState(true, null)).toBe('on')
    expect(recentRowState(false, null)).toBe('off')
  })

  it('follows the list once the filter applied', () => {
    expect(recentRowState(true, true)).toBe('on')
    expect(recentRowState(false, false)).toBe('off')
  })

  it('is unavailable when asked for but not applied: no deck has a complete match yet', () => {
    expect(recentRowState(true, false)).toBe('unavailable')
  })

  it('is off, never unavailable, while an archetype row is chosen (it replaces recent)', () => {
    expect(recentRowState(true, false, 'none')).toBe('off')
    expect(recentRowState(true, null, '7')).toBe('off')
    expect(recentRowState(true, false, null)).toBe('unavailable')
  })
})

describe('an Arena deck card', () => {
  it('shows the record over all matches and a tag per kind', () => {
    const arena = deck({ kinds: ['bo3', 'bo1'] })
    expect(deckKindLabels(arena)).toEqual(['Bo3', 'Bo1'])
    expect(deckKindLabels(deck({ kinds: ['bo1'] }))).toEqual(['Bo1'])
    expect(activityText(arena, now, 'never')).toBe('4 matches · never')
  })

  it('leaves an MTGO deck with no tag', () => {
    expect(deckKindLabels(deck())).toEqual([])
  })
})

describe('deckCover', () => {
  it('has a cover when the deck has its own image or a printing with art', () => {
    const none = { coverArt: null, localCoverArt: null, customCoverArt: null }
    expect(deckCover(none).hasCover).toBe(false)
    expect(deckCover({ ...none, coverArt: 'https://cards.scryfall.io/a.jpg' }).hasCover).toBe(true)
    expect(deckCover({ ...none, customCoverArt: '5-1700.png' }).hasCover).toBe(true)
  })

  it('hands the own image beside the printing, so the image can fall back', () => {
    expect(
      deckCover({ coverArt: 'https://cards.scryfall.io/a.jpg', localCoverArt: '12_art.jpg', customCoverArt: '5-1700.png' }),
    ).toEqual({ src: 'https://cards.scryfall.io/a.jpg', local: '12_art.jpg', custom: '5-1700.png', hasCover: true })
  })

  it('takes a draft entry, which has no custom image', () => {
    expect(deckCover({ coverArt: null, localCoverArt: null }).custom).toBeNull()
  })
})
