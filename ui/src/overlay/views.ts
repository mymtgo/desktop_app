/**
 * The overlay entry's pages (overlay spec 5.1): each overlay window opens
 * `overlay.html?view=…` and the entry mounts that page only, without the main
 * app's router, live match client or query preloads.
 */
export type OverlayView = 'game' | 'draft-notes' | 'league'

const VIEWS: readonly OverlayView[] = ['game', 'draft-notes', 'league']

const TITLES: Record<OverlayView, string> = {
  game: 'Game overlay',
  'draft-notes': 'Draft notes',
  league: 'League Overlay',
}

/** The page named by a location's query (`?view=game`), or null. */
export function overlayView(search: string): OverlayView | null {
  const view = new URLSearchParams(search).get('view')
  return VIEWS.find((known) => known === view) ?? null
}

/** The page's window title. */
export function viewTitle(view: OverlayView): string {
  return TITLES[view]
}

/**
 * Whether the page has the title strip that drags its window. The league
 * card fills its window and drags it itself (0.x, spec 9.2); the game
 * overlay's own header and tab strips drag it (0.x had no title strip).
 */
export function hasTitleStrip(view: OverlayView | null): boolean {
  return view !== 'league' && view !== 'game'
}

/** Whether the window is see-through (the league card's, so its rounded corners show). */
export function isTransparent(view: OverlayView | null): boolean {
  return view === 'league'
}
