import { describe, expect, it } from 'vitest'
import { hasFeature, routeAllowed, ROUTE_FEATURES, type Feature } from './platformFeatures'

const ALL: Feature[] = [
  'leagues',
  'limited',
  'tournaments',
  'ranking',
  'cardStats',
  'gameStats',
  'sideboardGuides',
  'cardsPage',
  'import',
  'matchDetail',
  'overlays',
  'logTrackerStatus',
]

describe('the feature map', () => {
  it('arena hides leagues, tournaments, import and the overlays', () => {
    const hidden = ALL.filter((feature) => !hasFeature('arena', feature))
    const shown: Feature[] = ['limited', 'ranking', 'cardsPage', 'matchDetail', 'cardStats', 'gameStats', 'sideboardGuides']
    expect(hidden.sort()).toEqual(ALL.filter((feature) => !shown.includes(feature)).sort())
    expect(hasFeature('arena', 'ranking')).toBe(true)
    expect(routeAllowed('settings-overlays', 'arena')).toBe(false)
    expect(routeAllowed('settings-overlays', 'mtgo')).toBe(true)
  })

  it('limited is available on arena, routes included', () => {
    expect(hasFeature('arena', 'limited')).toBe(true)
    for (const name of ['limited', 'limited-draft', 'limited-deck', 'limited-matches', 'limited-match', 'limited-cards']) {
      expect(routeAllowed(name, 'arena')).toBe(true)
    }
  })

  it('match detail is available on arena', () => {
    expect(hasFeature('arena', 'matchDetail')).toBe(true)
  })

  it('card stats, game stats and sideboard guides are available on arena', () => {
    for (const feature of ['cardStats', 'gameStats', 'sideboardGuides'] as Feature[]) {
      expect(hasFeature('arena', feature)).toBe(true)
    }
    expect(routeAllowed('deck-card-stats', 'arena')).toBe(true)
    expect(routeAllowed('deck-game-stats', 'arena')).toBe(true)
    expect(routeAllowed('deck-sideboard-guides', 'arena')).toBe(true)
  })

  it('the cards page is available on arena', () => {
    expect(hasFeature('arena', 'cardsPage')).toBe(true)
    expect(routeAllowed('cards', 'arena')).toBe(true)
  })

  it('endstep shows matches and hides what it has no data for', () => {
    const hidden: Feature[] = [
      'leagues',
      'limited',
      'tournaments',
      'ranking',
      'import',
      'overlays',
      'logTrackerStatus',
    ]
    expect(ALL.filter((feature) => !hasFeature('endstep', feature)).sort()).toEqual([...hidden].sort())
    expect(hasFeature('endstep', 'cardsPage')).toBe(true)
    expect(hasFeature('endstep', 'leagues')).toBe(false)
  })

  it('match detail, card stats, game stats and sideboard guides are available on endstep', () => {
    expect(hasFeature('endstep', 'matchDetail')).toBe(true)
    expect(hasFeature('endstep', 'cardStats')).toBe(true)
    expect(hasFeature('endstep', 'gameStats')).toBe(true)
    expect(hasFeature('endstep', 'sideboardGuides')).toBe(true)
    for (const feature of ['overlays', 'leagues', 'ranking'] as Feature[]) {
      expect(hasFeature('endstep', feature)).toBe(false)
    }
    expect(routeAllowed('deck-card-stats', 'endstep')).toBe(true)
    expect(routeAllowed('deck-game-stats', 'endstep')).toBe(true)
    expect(routeAllowed('deck-sideboard-guides', 'endstep')).toBe(true)
    expect(routeAllowed('deck-sideboard-guide', 'endstep')).toBe(true)
  })

  it('a limited route is not allowed on endstep', () => {
    for (const name of ['limited', 'limited-draft', 'leagues', 'settings-overlays', 'import']) {
      expect(routeAllowed(name, 'endstep')).toBe(false)
    }
    expect(routeAllowed('matches', 'endstep')).toBe(true)
    expect(routeAllowed('decks', 'endstep')).toBe(true)
  })

  it('mtgo hides ranking only', () => {
    expect(ALL.filter((feature) => !hasFeature('mtgo', feature))).toEqual(['ranking'])
  })

  it('hidden_routes_redirect_to_the_dashboard', () => {
    expect(routeAllowed('deck-leagues', 'arena')).toBe(false)
    expect(routeAllowed('decks', 'arena')).toBe(true)
    expect(routeAllowed('deck-leagues', 'mtgo')).toBe(true)
  })

  it('lets unnamed routes through', () => {
    expect(routeAllowed(null, 'arena')).toBe(true)
    expect(routeAllowed(undefined, 'arena')).toBe(true)
    expect(routeAllowed(Symbol('x'), 'arena')).toBe(true)
  })

  it('names every Limited route', () => {
    const limited = Object.keys(ROUTE_FEATURES).filter((name) => name === 'limited' || name.startsWith('limited-'))
    expect(limited.sort()).toEqual(
      ['limited', 'limited-cards', 'limited-deck', 'limited-draft', 'limited-match', 'limited-matches'].sort(),
    )
  })
})
