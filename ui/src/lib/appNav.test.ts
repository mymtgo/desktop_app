import { describe, expect, it } from 'vitest'
import { navItemActive } from './appNav'

const NAV = ['/', '/matches', '/decks', '/leagues', '/limited', '/opponents', '/metagame', '/cards']

function active(path: string): string[] {
  return NAV.filter((to) => navItemActive(path, to))
}

describe('navItemActive (issue #332)', () => {
  it('keeps Decks active on a match opened inside a deck', () => {
    expect(active('/decks/3/matches/q-token')).toEqual(['/decks'])
    expect(active('/decks/3/matches')).toEqual(['/decks'])
    expect(active('/decks/archetypes/4/matches')).toEqual(['/decks'])
  })

  it('keeps Matches active on a match opened from the Matches page', () => {
    expect(active('/matches/q-token')).toEqual(['/matches'])
    expect(active('/matches')).toEqual(['/matches'])
  })

  it('leaves the Dashboard to its own page', () => {
    expect(active('/')).toEqual(['/'])
    expect(active('/settings/general')).toEqual([])
  })

  it('does not take a page whose path only starts with the same letters', () => {
    expect(navItemActive('/decksx', '/decks')).toBe(false)
  })
})
