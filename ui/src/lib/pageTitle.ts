/** The window title of every page, in one place (0.x set `<Head title>`; Limited as `{event} · {page}`). */
/** A page's name by route name. */
export const PAGES: Record<string, string> = {
  dashboard: 'Dashboard',
  matches: 'Matches',
  match: 'Match',
  decks: 'Decks',
  'deck-archetype-matches': 'Decks',
  'deck-archetype-matchups': 'Decks',
  'deck-archetype-trends': 'Decks',
  'deck-archetype-card-stats': 'Decks',
  cards: 'Cards',
  'deck-dashboard': 'Dashboard',
  'deck-card-stats': 'Card Stats',
  'deck-game-stats': 'Game Stats',
  'deck-matches': 'Matches',
  'deck-match': 'Match',
  'deck-leagues': 'Leagues',
  'deck-tournaments': 'Tournaments',
  'deck-matchups': 'Matchups',
  'deck-sideboard-guides': 'Sideboard Guides',
  'deck-sideboard-guide': 'Sideboard Guide',
  'deck-decklist': 'Decklist',
  'deck-settings': 'Settings',
  leagues: 'Leagues',
  limited: 'Limited',
  'limited-draft': 'Draft',
  'limited-deck': 'Deck',
  'limited-matches': 'Matches',
  'limited-match': 'Match',
  'limited-cards': 'Cards',
  opponents: 'Opponents',
  'metagame-manage': 'Manage archetypes',
  'archetype-create': 'New archetype',
  'archetype-edit': 'Edit archetype',
  archetype: 'Archetype',
  metagame: 'Metagame',
  import: 'Import',
  settings: 'Settings',
  'settings-general': 'General',
  'settings-storage': 'Storage',
  'settings-archives': 'Archives',
  'settings-replays': 'Replays',
  'settings-integrations': 'Integrations',
  'settings-advanced': 'Advanced',
  'settings-licences': 'Licences',
  'whats-new': "What's new",
  'settings-account': 'Account',
  'settings-privacy': 'Data & Privacy',
  'settings-overlays': 'Overlays',
}

/**
 * The title for a route. `detail` is what the page loaded (a deck or Limited
 * event name): it leads, as `{detail} · {page}`. `appName` is the product
 * name read at runtime (release spec 13.1), never written here.
 */
export function pageTitle(routeName: string | null, detail: string | null, appName: string): string {
  const page = routeName === null ? undefined : PAGES[routeName]
  if (!page) return appName
  const named = detail ? `${detail} · ${page}` : page
  return `${named} - ${appName}`
}
