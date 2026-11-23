import { describe, expect, it } from 'vitest'
import {
  deckLinkQuery,
  deckMatchesPath,
  deckMatchPath,
  deckSidebarPage,
  deckVersionParam,
  matchPathInDecks,
  showVersionSelect,
  shownVersion,
  versionOptionLabel,
} from './deckRoute'

describe('deckVersionParam', () => {
  it('reads a version id from the query string', () => {
    expect(deckVersionParam('12')).toBe(12)
  })

  it('is null for all versions or anything that is not an id', () => {
    expect(deckVersionParam(undefined)).toBeNull()
    expect(deckVersionParam(null)).toBeNull()
    expect(deckVersionParam('')).toBeNull()
    expect(deckVersionParam('0')).toBeNull()
    expect(deckVersionParam('-3')).toBeNull()
    expect(deckVersionParam('1.5')).toBeNull()
    expect(deckVersionParam('abc')).toBeNull()
    expect(deckVersionParam(['12'])).toBeNull()
  })
})

describe('deckLinkQuery', () => {
  it('carries the version and a timeframe other than all time', () => {
    expect(deckLinkQuery({ version: 4, timeframe: 'monthly' }, true)).toEqual({ version: '4', timeframe: 'monthly' })
  })

  it('leaves out all versions and all time', () => {
    expect(deckLinkQuery({ version: null, timeframe: 'alltime' }, true)).toEqual({})
  })

  it('leaves the timeframe off pages without one', () => {
    expect(deckLinkQuery({ version: 4, timeframe: 'monthly' }, false)).toEqual({ version: '4' })
  })
})

describe('showVersionSelect', () => {
  const version = (id: number | null) => ({ id })

  it('shows once the deck has two versions besides "All versions"', () => {
    expect(showVersionSelect([version(null), version(1), version(2)])).toBe(true)
  })

  it('hides for a single version', () => {
    expect(showVersionSelect([version(null), version(1)])).toBe(false)
    expect(showVersionSelect([version(null)])).toBe(false)
  })
})

describe('versionOptionLabel', () => {
  it('joins the label, the current mark and the dates', () => {
    expect(versionOptionLabel({ label: 'v2', isCurrent: true, dateLabel: 'Oct 03 - now' })).toBe('v2 · Current · Oct 03 - now')
    expect(versionOptionLabel({ label: 'v1', isCurrent: false, dateLabel: 'Sep 12 - Oct 03' })).toBe('v1 · Sep 12 - Oct 03')
    expect(versionOptionLabel({ label: 'All versions', isCurrent: false, dateLabel: null })).toBe('All versions')
  })
})

describe('the version the select shows', () => {
  it('is the chosen version on every page', () => {
    for (const page of ['deck-dashboard', 'deck-matches', 'deck-decklist', 'deck-settings']) {
      expect(shownVersion(4, 9, page)).toBe(4)
    }
  })

  it('is the current version on Decklist and Settings, which show it when none is chosen (0.x)', () => {
    expect(shownVersion(null, 9, 'deck-decklist')).toBe(9)
    expect(shownVersion(null, 9, 'deck-settings')).toBe(9)
  })

  it('is all versions on Dashboard and Matches, which count every version when none is chosen', () => {
    expect(shownVersion(null, 9, 'deck-dashboard')).toBeNull()
    expect(shownVersion(null, 9, 'deck-matches')).toBeNull()
  })

  it('is all versions when the deck has none', () => {
    expect(shownVersion(null, null, 'deck-decklist')).toBeNull()
  })
})

describe('a match inside a deck (issue #332)', () => {
  it('opens under the deck with the deck query', () => {
    expect(deckMatchPath(3, 'q/token')).toBe('/decks/3/matches/q%2Ftoken')
    expect(deckMatchPath(3, 'q-token', { version: '4', timeframe: 'monthly' })).toBe(
      '/decks/3/matches/q-token?version=4&timeframe=monthly',
    )
  })

  it('goes back to the deck matches with the same query', () => {
    expect(deckMatchesPath(3)).toBe('/decks/3/matches')
    expect(deckMatchesPath(3, { version: '4' })).toBe('/decks/3/matches?version=4')
  })

  it('opens a listed match in its deck, or on its own without one', () => {
    expect(matchPathInDecks(5, 'q-token')).toBe('/decks/5/matches/q-token')
    expect(matchPathInDecks(null, 'q-token')).toBe('/matches/q-token')
    expect(matchPathInDecks(undefined, 'q-token')).toBe('/matches/q-token')
  })

  it('highlights Matches in the sidebar on a match and Sideboard Guides on a guide', () => {
    expect(deckSidebarPage('deck-match')).toBe('deck-matches')
    expect(deckSidebarPage('deck-sideboard-guide')).toBe('deck-sideboard-guides')
    expect(deckSidebarPage('deck-dashboard')).toBe('deck-dashboard')
  })
})
