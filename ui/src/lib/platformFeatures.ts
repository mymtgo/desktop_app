import type { PlatformView } from '@/bindings'

/** A part of the app that only one platform has (arena spec section 15). */
export type Feature =
  | 'leagues'
  | 'limited'
  | 'tournaments'
  | 'ranking'
  | 'cardStats'
  | 'gameStats'
  | 'sideboardGuides'
  | 'cardsPage'
  | 'import'
  | 'matchDetail'
  | 'overlays'
  | 'logTrackerStatus'

/** What Arena has no data for. */
const ARENA_HIDDEN: Feature[] = [
  'leagues',
  'tournaments',
  'import',
  // The MTGO overlays read MTGO's game state; Arena gets its own later.
  'overlays',
  // The first-run state reads MTGO's log tracker status.
  'logTrackerStatus',
]

/** What Endstep has no data for yet (later plans remove the ones they build). */
const ENDSTEP_HIDDEN: Feature[] = [
  'leagues',
  'limited',
  'tournaments',
  'ranking',
  'import',
  'overlays',
  'logTrackerStatus',
]

/** What MTGO has no data for. */
const MTGO_HIDDEN: Feature[] = ['ranking']

function hiddenFeatures(platform: PlatformView): Feature[] {
  switch (platform) {
    case 'mtgo':
      return MTGO_HIDDEN
    case 'arena':
      return ARENA_HIDDEN
    case 'endstep':
      return ENDSTEP_HIDDEN
  }
}

export function hasFeature(platform: PlatformView, feature: Feature): boolean {
  return !hiddenFeatures(platform).includes(feature)
}

/** The feature a route needs, by route name; routes not listed need none. */
export const ROUTE_FEATURES: Record<string, Feature> = {
  leagues: 'leagues',
  'deck-leagues': 'leagues',
  limited: 'limited',
  'limited-draft': 'limited',
  'limited-deck': 'limited',
  'limited-matches': 'limited',
  'limited-match': 'limited',
  'limited-cards': 'limited',
  'deck-tournaments': 'tournaments',
  'deck-card-stats': 'cardStats',
  'deck-archetype-card-stats': 'cardStats',
  'deck-game-stats': 'gameStats',
  'deck-sideboard-guides': 'sideboardGuides',
  'deck-sideboard-guide': 'sideboardGuides',
  cards: 'cardsPage',
  import: 'import',
  'settings-overlays': 'overlays',
}

export function routeAllowed(routeName: string | symbol | null | undefined, platform: PlatformView): boolean {
  if (typeof routeName !== 'string') return true
  const feature = ROUTE_FEATURES[routeName]
  return feature === undefined || hasFeature(platform, feature)
}
