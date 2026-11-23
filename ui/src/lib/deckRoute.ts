/**
 * The deck pages' shared query string (0.x kept both in the URL too):
 * `?version=<id>` picks one version (none: all versions) and `?timeframe=`
 * the window on the pages that have one (Dashboard, Matches, Leagues,
 * Tournaments).
 */

/** The version id of a `version` query value; null for all versions or anything else. */
export function deckVersionParam(value: unknown): number | null {
  if (typeof value !== 'string' || !/^\d+$/.test(value)) return null
  const id = Number(value)
  return id > 0 ? id : null
}

/**
 * The query of a link to a deck page. The version follows the user across
 * pages; the timeframe only onto the pages that use one (0.x added it to the
 * Dashboard, Matches, Leagues and Tournaments links only). Defaults are left out.
 */
export function deckLinkQuery(
  state: { version: number | null; timeframe: string },
  withTimeframe: boolean,
): Record<string, string> {
  const query: Record<string, string> = {}
  if (state.version !== null) query.version = String(state.version)
  if (withTimeframe && state.timeframe !== 'alltime') query.timeframe = state.timeframe
  return query
}

/** The version select shows from two versions on; the list starts with "All versions". */
export function showVersionSelect(versions: { id: number | null }[]): boolean {
  return versions.length > 2
}

/** "v2 · Current · Oct 03 - now", as the 0.x select item reads. */
export function versionOptionLabel(version: { label: string; isCurrent: boolean; dateLabel: string | null }): string {
  const parts = [version.label]
  if (version.isCurrent) parts.push('Current')
  if (version.dateLabel) parts.push(version.dateLabel)
  return parts.join(' · ')
}

/** Pages that show the current version when none is chosen (0.x `DecklistController`, Settings). */
const CURRENT_VERSION_PAGES = ['deck-decklist', 'deck-settings']

/**
 * The version the select shows: the one the page displays. With none chosen,
 * Decklist and Settings show the current version (0.x preselected it there:
 * `GetDeckViewSharedProps` `currentVersionId`); Dashboard and Matches count
 * all versions.
 */
export function shownVersion(version: number | null, currentVersionId: number | null, page: string): number | null {
  if (version !== null) return version
  return CURRENT_VERSION_PAGES.includes(page) ? currentVersionId : null
}

/** A query string for a path: `?a=1&b=2`, empty without values. */
function queryString(query: Record<string, string>): string {
  const text = new URLSearchParams(query).toString()
  return text ? `?${text}` : ''
}

/**
 * One match inside a deck's layout (issue #332): the deck sidebar stays,
 * with Matches highlighted, and the top nav keeps Decks active. `query` is
 * the deck's `deckLinkQuery`, so the sidebar keeps the version and timeframe.
 */
export function deckMatchPath(deckId: number, token: string, query: Record<string, string> = {}): string {
  return `/decks/${deckId}/matches/${encodeURIComponent(token)}${queryString(query)}`
}

/** A deck's Matches page, where "Back to matches" goes without history. */
export function deckMatchesPath(deckId: number, query: Record<string, string> = {}): string {
  return `/decks/${deckId}/matches${queryString(query)}`
}

/**
 * Where a row of a list in the Decks area opens: inside its deck's layout
 * when the match has a deck, else the match page on its own.
 */
export function matchPathInDecks(deckId: number | null | undefined, token: string): string {
  return deckId ? deckMatchPath(deckId, token) : `/matches/${encodeURIComponent(token)}`
}

/** The route names of the deck layout's pages that sit under another sidebar item. */
const SIDEBAR_PARENTS: Record<string, string> = {
  'deck-sideboard-guide': 'deck-sideboard-guides',
  'deck-match': 'deck-matches',
}

/** The sidebar item a deck page highlights: a guide's editor sits under Sideboard Guides, a match under Matches. */
export function deckSidebarPage(routeName: string): string {
  return SIDEBAR_PARENTS[routeName] ?? routeName
}
