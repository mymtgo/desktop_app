/**
 * Whether a top nav item is active on `path`: its own page and every page
 * under it. A deck's pages, a match opened inside a deck included, sit under
 * /decks and keep Decks active; a match opened from the Matches page sits
 * under /matches (issue #332). The Dashboard is active only on its own page.
 */
export function navItemActive(path: string, to: string): boolean {
  return path === to || (to !== '/' && path.startsWith(`${to}/`))
}
