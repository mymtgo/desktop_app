import { setReleases, type SetRelease } from '@/lib/wrapped/sets'

/**
 * The year in Magic itself, for the opening cards (Wrapped spec): the sets,
 * the big events' winning decks as one sentence, and the cards banned or
 * restricted. Hard coded per year from Wizards' announcements and coverage;
 * a year with nothing listed shows only what it has.
 */
export type BanChange = { format: string; cards: string[]; date: string; restricted?: boolean }

/** A banned card with every format it left and when it first went, as shown under its image. */
export type BannedCard = { name: string; formats: string[]; date: string }

/** The winners sentence: plain text and winning decks (never players), in order. */
export type WinnersPart = string | { deck: string }

export type MagicYear = {
  sets: SetRelease[]
  winners: WinnersPart[]
  bans: BanChange[]
}

const YEARS: Record<number, Omit<MagicYear, 'sets'>> = {
  // Winners: magic.gg coverage of the three Pro Tours and World
  // Championship 32. Banned and restricted: magic.wizards.com, 18 May,
  // 29 Jun and 10 Aug 2026 (MTGO formats only).
  2026: {
    winners: [
      { deck: 'Dimir Excruciator' },
      ' and ',
      { deck: 'Selesnya Landfall' },
      ' won the Standard Pro Tours, ',
      { deck: 'Simic Neoform' },
      ' took the Modern one, and ',
      { deck: 'Izzet Mission' },
      ' won the World Championship.',
    ],
    bans: [
      { format: 'Standard', cards: ['Badgermole Cub', "Stormchaser's Talent", 'Gran-Gran'], date: '2026-08-10' },
      { format: 'Pioneer', cards: ['Cori-Steel Cutter'], date: '2026-05-18' },
      { format: 'Modern', cards: ["Phlage, Titan of Fire's Fury", 'Lotus Field'], date: '2026-05-18' },
      { format: 'Legacy', cards: ['Undercity Informer', 'Candelabra of Tawnos', 'The Fantasticar'], date: '2026-08-10' },
      { format: 'Vintage', cards: ['The Fantasticar'], date: '2026-08-10', restricted: true },
      { format: 'Pauper', cards: ['Seeker of Skybreak'], date: '2026-06-29' },
    ],
  },
}

export function magicYear(year: number): MagicYear {
  const facts = YEARS[year] ?? { winners: [], bans: [] }
  return { sets: setReleases(year), ...facts }
}

/** Each banned card once, in announcement order, with all its formats. */
export function bannedCards(year: number): BannedCard[] {
  const out: BannedCard[] = []
  for (const ban of magicYear(year).bans) {
    const format = ban.restricted ? `${ban.format} (restricted)` : ban.format
    for (const name of ban.cards) {
      const known = out.find((card) => card.name === name)
      if (known) {
        known.formats.push(format)
        if (ban.date < known.date) known.date = ban.date
      }
      else out.push({ name, formats: [format], date: ban.date })
    }
  }
  return out
}
