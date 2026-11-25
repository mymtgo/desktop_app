/**
 * The Magic sets each Wrapped year saw, for the opening card's timeline
 * (Wrapped spec). Hard coded: the main expansions released between
 * 1 January and 30 November, from Scryfall's set list. Their symbols are
 * Scryfall's set SVGs, unmodified, one per set code in
 * `@/assets/wrapped/sets/`. A set without one shows its code.
 */
export type SetRelease = {
  code: string
  name: string
  released: string
  /** The set's face on the opening card: its most played card on MTGO this year (or its face planeswalker), by printing, with its artist. */
  art: { set: string; number: string; card: string; artist: string }
}

function face(set: string, number: string, card: string, artist: string): SetRelease['art'] {
  return { set, number, card, artist }
}

const SET_RELEASES: Record<number, SetRelease[]> = {
  2026: [
    { code: 'ecl', name: 'Lorwyn Eclipsed', released: '2026-01-23', art: face('ecl', '176', 'Formidable Speaker', 'Aurore Folny') },
    { code: 'tmt', name: 'Teenage Mutant Ninja Turtles', released: '2026-03-06', art: face('tmt', '178', 'Skateboard', 'Hokyoung Kim') },
    { code: 'sos', name: 'Secrets of Strixhaven', released: '2026-04-24', art: face('sos', '49', 'Flow State', 'Genel Jumalon') },
    { code: 'msh', name: 'Marvel Super Heroes', released: '2026-06-26', art: face('msh', '187', 'Shang-Chi, Master of Kung Fu', 'Lee Woo-chul') },
    { code: 'hob', name: 'The Hobbit', released: '2026-08-14', art: face('hob', '173', "Giant's Boulder", 'Miklós Ligeti') },
    { code: 'fra', name: 'Reality Fracture', released: '2026-10-02', art: face('fra', '43', 'The Theorist, Jace Beleren', 'Ekaterina Burmak') },
    { code: 'trk', name: 'Star Trek', released: '2026-11-13', art: face('trk', '200', 'Genesis Device', 'Cliff Nielsen') },
  ],
}

const ICONS = import.meta.glob<string>('@/assets/wrapped/sets/*.svg', { eager: true, query: '?url', import: 'default' })

/** A set's symbol URL, or null when it has none bundled. */
export function setIcon(code: string): string | null {
  for (const [path, url] of Object.entries(ICONS)) {
    if (path.endsWith(`/${code}.svg`)) return url
  }
  return null
}

/** The year's sets in release order; empty for a year with none listed. */
export function setReleases(year: number): SetRelease[] {
  return SET_RELEASES[year] ?? []
}
