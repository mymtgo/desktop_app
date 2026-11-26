import { describe, expect, it } from 'vitest'
import type { DeckArchetypeOptionView, DeckView, RecordView } from '@/bindings'
import {
  DECK_INDEX_TABS,
  archetypeRate,
  archetypeRowTitle,
  canSelectDeck,
  dimsInSelection,
  assignableDeckIds,
  canSetArchetypeByHand,
  selectionBlockedTitle,
  deckCount,
  deckIndexTabRoute,
  rateTone,
  selectionFormat,
  toggleArchetype,
  visibleArchetypes,
} from './deckIndex'

function record(wins: number, losses: number, draws = 0): RecordView {
  const total = wins + losses + draws
  return {
    wins,
    losses,
    draws,
    total,
    winRate: total ? Math.round((100 * wins) / total) : 0,
    label: draws ? `${wins} - ${losses} - ${draws}` : `${wins} - ${losses}`,
  }
}

function option(name: string, rec: RecordView): DeckArchetypeOptionView {
  return { id: 1, name, colors: [], format: 'modern', deckCount: 2, record: rec, isCustom: false }
}

function deck(id: number, format: string, commanderFormat = false): DeckView {
  return {
    id,
    name: `Qdeck ${id}`,
    originalName: null,
    format,
    formatLabel: format,
    colors: [],
    coverArt: null,
    localCoverArt: null,
    customCoverArt: null,
    archived: false,
    record: record(0, 0),
    kinds: [],
    lastPlayedAtMs: null,
    archetype: null,
    commanderFormat,
  }
}

describe('the deck index archetype rows (0.x DeckIndexSidebar)', () => {
  it('toggles a row off when clicked again', () => {
    expect(toggleArchetype(null, '5')).toBe('5')
    expect(toggleArchetype('5', '5')).toBeNull()
    expect(toggleArchetype('5', 'none')).toBe('none')
  })

  it('shows a dash before any match, else the rate coloured at 50', () => {
    expect(archetypeRate(record(0, 0))).toBe('-')
    expect(archetypeRate(record(3, 1))).toBe('75%')
    expect(rateTone(record(0, 0))).toBe('text-muted-foreground')
    expect(rateTone(record(1, 1))).toBe('text-success')
    expect(rateTone(record(1, 2))).toBe('text-destructive')
  })

  it('titles a row with its record and counts decks', () => {
    expect(archetypeRowTitle(option('Qtron', record(2, 1)))).toBe('Qtron · 2 - 1 · 3 matches')
    expect(deckCount(1)).toBe('1 deck')
    expect(deckCount(3)).toBe('3 decks')
  })
})

describe('the deck selection (0.x Index selectionFormat)', () => {
  it('scopes the picker to the selection when it shares one format', () => {
    const decks = [deck(1, 'CMODERN'), deck(2, 'CMODERN'), deck(3, 'CPAUPER')]
    expect(selectionFormat([1, 2], decks)).toBe('CMODERN')
    expect(selectionFormat([1, 3], decks)).toBeNull()
    expect(selectionFormat([], decks)).toBeNull()
  })

  it('only lets decks of the selected format join the selection', () => {
    expect(canSelectDeck(deck(1, 'CMODERN'), null)).toBe(true)
    expect(canSelectDeck(deck(1, 'CMODERN'), 'CMODERN')).toBe(true)
    expect(canSelectDeck(deck(3, 'CPAUPER'), 'CMODERN')).toBe(false)
  })

  it('keeps commander format decks out of the selection and the set-archetype dialog', () => {
    const brawl = deck(4, 'ARENA_HISTORIC_BRAWL', true)
    expect(canSelectDeck(brawl, null)).toBe(false)
    expect(canSelectDeck(brawl, 'ARENA_HISTORIC_BRAWL')).toBe(false)
    expect(canSetArchetypeByHand(brawl)).toBe(false)
    expect(canSetArchetypeByHand(deck(5, 'ARENA_STANDARD'))).toBe(true)
    // The backend decides: an MTGO Brawl deck is not a commander format deck.
    expect(canSetArchetypeByHand(deck(6, 'CBRAWL'))).toBe(true)
    expect(canSelectDeck(deck(6, 'CBRAWL'), 'CBRAWL')).toBe(true)
  })

  it('dims only decks of another format while a selection is on, a Brawl deck like any other', () => {
    const brawl = deck(4, 'ARENA_HISTORIC_BRAWL', true)
    expect(dimsInSelection(brawl, null)).toBe(false)
    expect(dimsInSelection(deck(1, 'CMODERN'), null)).toBe(false)
    expect(dimsInSelection(deck(1, 'CMODERN'), 'CMODERN')).toBe(false)
    expect(dimsInSelection(deck(3, 'CPAUPER'), 'CMODERN')).toBe(true)
    expect(dimsInSelection(brawl, 'CMODERN')).toBe(true)
  })

  it('drops commander format decks from every assign, dropped or selected', () => {
    const decks = [deck(1, 'CMODERN'), deck(4, 'ARENA_HISTORIC_BRAWL', true), deck(5, 'ARENA_BRAWL', true)]
    expect(assignableDeckIds([1, 4, 5], decks)).toEqual([1])
    expect(assignableDeckIds([4], decks)).toEqual([])
    expect(assignableDeckIds([1, 9], decks)).toEqual([1, 9])
  })

  it('tells a commander deck why it cannot be selected', () => {
    expect(selectionBlockedTitle(deck(4, 'ARENA_BRAWL', true))).toBe("Brawl decks take their commander's archetype")
    expect(selectionBlockedTitle(deck(1, 'CMODERN'))).toBe('Only decks of one format can be selected together')
  })
})

describe('the archetype tabs (0.x deckIndexTabs)', () => {
  it('lists the five tabs', () => {
    expect(DECK_INDEX_TABS.map((tab) => tab.label)).toEqual(['Decks', 'Matches', 'Matchups', 'Trends', 'Card Stats'])
  })

  it('links the grid with the archetype filter and the stats tabs with the timeframe', () => {
    expect(deckIndexTabRoute('decks', 7, 'week')).toEqual({ name: 'decks' })
    expect(deckIndexTabRoute('matches', 7, 'alltime')).toEqual({
      name: 'deck-archetype-matches',
      params: { archetypeId: '7' },
      query: {},
    })
    expect(deckIndexTabRoute('card-stats', 7, 'week')).toEqual({
      name: 'deck-archetype-card-stats',
      params: { archetypeId: '7' },
      query: { timeframe: 'week' },
    })
  })
})

describe('visibleArchetypes', () => {
  const played = { ...option('Qplayed', record(3, 1)), id: 1 }
  const empty = { ...option('Qempty', record(0, 0)), id: 2 }
  const other = { ...option('Qother', record(0, 0)), id: 3 }

  it('hides archetypes without matches and counts them', () => {
    const { shown, hiddenCount } = visibleArchetypes([played, empty, other], { showAll: false, selected: null })
    expect(shown).toEqual([played])
    expect(hiddenCount).toBe(2)
  })

  it('keeps the order and shows everything when asked', () => {
    const { shown, hiddenCount } = visibleArchetypes([empty, played, other], { showAll: true, selected: null })
    expect(shown).toEqual([empty, played, other])
    expect(hiddenCount).toBe(2)
  })

  it('keeps the selected archetype even without matches', () => {
    const { shown, hiddenCount } = visibleArchetypes([played, empty, other], { showAll: false, selected: '2' })
    expect(shown).toEqual([played, empty])
    expect(hiddenCount).toBe(1)
  })

  it('hides nothing when no archetype has matches', () => {
    const { shown, hiddenCount } = visibleArchetypes([empty, other], { showAll: false, selected: null })
    expect(shown).toEqual([empty, other])
    expect(hiddenCount).toBe(0)
  })

  it('has no hidden rows when every archetype has matches', () => {
    expect(visibleArchetypes([played], { showAll: false, selected: null }).hiddenCount).toBe(0)
  })
})
