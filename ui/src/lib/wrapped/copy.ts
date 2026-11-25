import afternoonArt from '@/assets/wrapped/scenes/afternoon.webp'
import eveningArt from '@/assets/wrapped/scenes/evening.webp'
import morningArt from '@/assets/wrapped/scenes/morning.webp'
import nightArt from '@/assets/wrapped/scenes/night.webp'
import type { Art, FormatShare, KeyCard, PlayerType, PlayPeriod, RunnerUp, WrappedBundle, WrappedCard } from '@/bindings'
import { bannedCards, magicYear, type BannedCard, type MagicYear } from '@/lib/wrapped/magicYear'

/**
 * Every Wrapped card's words in one place (Wrapped spec: feel-good only).
 * `cardView` turns a bundle card into what the one card template draws.
 */
export type Accent = 'blue' | 'green' | 'purple' | 'amber' | 'pink'

export type CardStat = { value: string; label: string; good?: boolean }

export type CardFact = { label: string; value: string; detail?: string }

export type CardVisual =
  | { kind: 'mentions'; rows: FormatShare[] }
  | { kind: 'hours'; byHour: number[]; peakHour: number }
  | { kind: 'magicYear'; year: MagicYear }
  | { kind: 'banned'; cards: BannedCard[] }
  | { kind: 'days'; byDay: number[] }

export type CardView = {
  label: string
  /** A big number, when the card leads with one. */
  big?: string
  /** A name or headline, when the card leads with one. */
  title?: string
  lines: string[]
  /** Large headline numbers under the visual, side by side. */
  figures: CardStat[]
  /** A small heading over the figures. */
  figuresHeading: string | null
  /** A quiet brag line under the figures. */
  badge: string | null
  pills: string[]
  art: Art | null
  accent: Accent
  visual: CardVisual | null
  /** `spotlight`: the name, one big number, a line and the numbers over
   * faint art, with the full card on the right when `spotlight` is set. */
  layout: 'plain' | 'spotlight'
  /** The full card image, on the right of a spotlight layout. */
  spotlight: Art | null
  /** Who made the art, shown on it when the art is ours to pick. */
  credit: string | null
  /** Words beside the big number. */
  bigLabel: string | null
  /** A deck or an opponent's archetype: drawn with its art behind, its mana
   * symbols by the name and its key cards in a row. */
  deck: { colors: string[]; cards: KeyCard[] } | null
  /** The part of the day whose land art fills the card, for "When you play". */
  scene: PlayPeriod | null
  /** A larger line under the title. */
  lead: string | null
  /** Small labelled tiles at the foot of the card. */
  facts: CardFact[]
  /** Honourable mentions under a hairline: the next cards and their numbers. */
  runnersUp: { heading: string; rows: { name: string; value: string }[] } | null
  /** `full`: a spotlight card's art at full strength, dark only behind the
   * words; `faint` (the default) keeps it low behind everything. */
  artStrength: 'faint' | 'full'
  /** The trophy cabinet: a trophy for each league won, a medal for each Top 8. */
  cabinet: { trophies: number; top8s: number } | null
  /** The Limited card: drafts, the favourite colour pair as mana symbols, the best run. */
  limited: { drafts: number; pair: string | null; colors: string[]; record: string | null } | null
  /** The player type card's own art and its credit. */
  persona: { art: Art; credit: string } | null
  /** The closing card's recap line under the title. */
  recap: CardStat[]
  /** The closing card: shows "Watch again". */
  closing: boolean
}

export const FIRST_YEAR_NOTE =
  "This is your first MyMTGO Wrapped. It might look a little light, but next year's will know a lot more about you."

export const PLAYER_TYPES: Record<PlayerType, { name: string; line: string }> = {
  grinder: { name: 'The Grinder', line: 'One more queue? Always. You put the hours in this year.' },
  deckLoyalist: { name: 'The Deck Loyalist', line: 'You found your deck and you stuck with it.' },
  comebackKid: { name: 'The Comeback Kid', line: 'Mulligans, bad starts, close games. You kept winning anyway.' },
  limitedLover: { name: 'The Limited Lover', line: 'Fresh packs, new sets, every draft a new puzzle.' },
  formatExplorer: { name: 'The Format Explorer', line: 'Why play one format when you can play them all?' },
  allRounder: { name: 'The All-Rounder', line: 'A bit of everything, and good at all of it.' },
}

export const PLAY_PERIODS: Record<PlayPeriod, { name: string; line: string }> = {
  morning: { name: 'Early bird', line: 'Coffee in one hand, a league in the other. Most of your Magic happened before noon.' },
  afternoon: { name: 'Daylight duellist', line: 'While the sun was up, so were your queues.' },
  evening: { name: 'Prime-time player', line: 'Dinner done, decks out. Evenings were when you played most.' },
  night: { name: 'Night owl', line: 'One more match before bed? It was never just one. Most of your Magic happened after 10pm.' },
}

// The comebacks card's art: a goblin clawing his way back out of the
// ground (Persist, art by Milivoj Ćeran).
const COMEBACK_ART: Art = {
  remote: 'https://api.scryfall.com/cards/ecc/83?format=image&version=art_crop',
  local: null,
  custom: null,
}

// A card's art crop by printing, for the art Wrapped picks itself.
function printingArt(set: string, number: string): Art {
  return { remote: `https://api.scryfall.com/cards/${set}/${number}?format=image&version=art_crop`, local: null, custom: null }
}

// A full-size art file bundled with the app, for art shown large enough
// that Scryfall's art crop (about 626 pixels wide) looks blocky.
function bundledArt(url: string): Art {
  return { remote: url, local: null, custom: null }
}

// The "When you play" card's art: a basic land for each part of the day,
// bundled at full size (the scene fills the whole card). Printings:
// Mountain pana 270, Island sos 275, Plains tla 287, Island thb 281.
export const PERIOD_ART: Record<PlayPeriod, { art: Art; credit: string }> = {
  morning: { art: bundledArt(morningArt), credit: 'Mountain · art by Shahab Alizadeh' },
  afternoon: { art: bundledArt(afternoonArt), credit: 'Island · art by Constantin Marin' },
  evening: { art: bundledArt(eveningArt), credit: 'Plains · art by Salvatorre Zee Yazzie' },
  night: { art: bundledArt(nightArt), credit: 'Island · art by Sam Burley' },
}

// The play or draw card's art: seizing the first move, or drawing into it
// (Seize the Initiative, art by Steve Argyle; Divination, art by Willian Murai).
const PLAY_DRAW_ART: Record<'play' | 'draw', { art: Art; credit: string }> = {
  play: { art: printingArt('som', '20'), credit: 'Seize the Initiative · art by Steve Argyle' },
  draw: { art: printingArt('m19', '51'), credit: 'Divination · art by Willian Murai' },
}

// The trophy cabinet's art (Arena of Glory, art by Jorge Jacinto).
const TROPHY_ART = { art: printingArt('mh3', '215'), credit: 'Arena of Glory · art by Jorge Jacinto' }

// Colour pairs (and three-colour groups) by name, for mana symbols.
const COLOUR_GROUPS: Record<string, string[]> = {
  Azorius: ['W', 'U'], Dimir: ['U', 'B'], Rakdos: ['B', 'R'], Gruul: ['R', 'G'], Selesnya: ['G', 'W'],
  Orzhov: ['W', 'B'], Izzet: ['U', 'R'], Golgari: ['B', 'G'], Boros: ['R', 'W'], Simic: ['G', 'U'],
  Bant: ['G', 'W', 'U'], Esper: ['W', 'U', 'B'], Grixis: ['U', 'B', 'R'], Jund: ['B', 'R', 'G'], Naya: ['R', 'G', 'W'],
  Abzan: ['W', 'B', 'G'], Jeskai: ['U', 'R', 'W'], Sultai: ['B', 'G', 'U'], Mardu: ['R', 'W', 'B'], Temur: ['G', 'U', 'R'],
}

// The Limited card's art (Cogwork Librarian, art by Dan Murayama Scott).
const LIMITED_ART = { art: printingArt('cns', '58'), credit: 'Cogwork Librarian · art by Dan Murayama Scott' }

// Each player type's art, picked for its name.
export const PERSONA_ART: Record<PlayerType, { art: Art; credit: string }> = {
  grinder: { art: printingArt('hou', '155'), credit: 'Grind // Dust · art by Josh Hass' },
  deckLoyalist: { art: printingArt('msc', '136'), credit: 'Loyal Retainers · art by Tyler Walpole' },
  comebackKid: { art: printingArt('ulg', '41'), credit: 'Second Chance · art by Mark Tedin' },
  limitedLover: { art: printingArt('und', '33'), credit: 'Booster Tutor · art by Heather Hudson' },
  formatExplorer: { art: printingArt('tle', '259'), credit: 'Explore · art by Maojin Lee' },
  allRounder: { art: printingArt('otc', '83'), credit: 'Mirror Entity · art by Zoltan Boros & Gabor Szikszai' },
}

const WEEKDAYS = ['Monday', 'Tuesday', 'Wednesday', 'Thursday', 'Friday', 'Saturday', 'Sunday']

export function count(value: number): string {
  return value.toLocaleString('en-GB')
}

/** `part` as a whole percentage of `whole`; 0 when there is no whole. */
function share(part: number, whole: number): number {
  return whole === 0 ? 0 : Math.round((part / whole) * 100)
}

function plural(value: number, one: string, many = `${one}s`): string {
  return `${count(value)} ${value === 1 ? one : many}`
}

/** `11pm`, `3pm`, `midnight`, `noon`. */
export function hourText(hour: number): string {
  if (hour === 0) return 'midnight'
  if (hour === 12) return 'noon'
  return hour < 12 ? `${hour}am` : `${hour - 12}pm`
}

function runnersUp(heading: string, rows: RunnerUp[], value: (row: RunnerUp) => string): CardView['runnersUp'] {
  return rows.length > 0 ? { heading, rows: rows.map((row) => ({ name: row.name, value: value(row) })) } : null
}

function base(label: string, accent: Accent): CardView {
  return { label, lines: [], figures: [], figuresHeading: null, badge: null, scene: null, lead: null, facts: [], deck: null, layout: 'plain', spotlight: null, bigLabel: null, credit: null, pills: [], art: null, accent, visual: null, runnersUp: null, artStrength: 'faint', cabinet: null, limited: null, persona: null, recap: [], closing: false }
}

/**
 * The page backdrop: the deck of the year's art, blurred behind every card.
 * Null when the bundle has no deck of the year or it has no art.
 */
export function backdropArt(bundle: WrappedBundle): Art | null {
  for (const card of bundle.cards) {
    if (card.kind !== 'deckOfYear') continue
    const { remote, local, custom } = card.art
    return remote !== null || local !== null || custom !== null ? card.art : null
  }
  return null
}

export function cardView(card: WrappedCard, bundle: WrappedBundle): CardView {
  switch (card.kind) {
    case 'magicYear': {
      const year = magicYear(bundle.year)
      const sets = year.sets.length
      return {
        ...base(`${bundle.year} in Magic`, 'blue'),
        title: sets > 0 ? `This year saw ${plural(sets, 'set release')}` : `A year of Magic`,
        visual: { kind: 'magicYear', year },
      }
    }
    case 'banned': {
      const cards = bannedCards(bundle.year)
      return {
        ...base(`${bundle.year} in Magic`, 'blue'),
        title: cards.length > 0 ? 'A moment of silence...' : 'A year without bans',
        visual: { kind: 'banned', cards },
      }
    }
    case 'yearInNumbers':
      return {
        ...base(`Your ${bundle.year} in Magic`, 'blue'),
        title: 'Through it all, you kept queueing',
        visual: { kind: 'days', byDay: card.byDay },
        figures: [
          { value: count(card.matches), label: card.matches === 1 ? 'match' : 'matches' },
          { value: count(card.games), label: card.games === 1 ? 'game' : 'games' },
          ...(card.hours > 0 ? [{ value: count(card.hours), label: card.hours === 1 ? 'hour' : 'hours' }] : []),
          { value: count(card.daysPlayed), label: card.daysPlayed === 1 ? 'day' : 'days' },
        ],
        badge: card.topPercent !== null && card.topPercent <= 50 ? `You're in the top ${card.topPercent}% of MyMTGO players` : null,
      }
    case 'formats':
      return {
        ...base('Your format of choice', 'blue'),
        title: card.favourite.label,
        lines: [`${plural(card.favourite.matches, 'match', 'matches')}, ${card.favourite.percent}% of your year.`],
        art: card.favourite.art,
        visual: card.mentions.length > 0 ? { kind: 'mentions', rows: card.mentions } : null,
      }
    case 'whenYouPlay':
      return {
        ...base('When you play', 'purple'),
        title: PLAY_PERIODS[card.period].name,
        lead: PLAY_PERIODS[card.period].line,
        visual: { kind: 'hours', byHour: card.byHour, peakHour: card.peakHour },
        scene: card.period,
        facts: [
          { label: 'Your peak', value: `${hourText(card.peakHour)} on ${WEEKDAYS[card.peakWeekday] ?? 'weekday'}s` },
          card.weekends
            ? { label: 'Your week', value: 'Weekend warrior', detail: 'Saturdays and Sundays were your busiest days.' }
            : { label: 'Your week', value: 'Weekday regular', detail: 'Monday to Friday is when you played most.' },
        ],
      }
    case 'deckOfYear':
    case 'bestDeck':
      return {
        ...base(card.kind === 'deckOfYear' ? 'Your deck of the year' : 'Your best deck', card.kind === 'deckOfYear' ? 'pink' : 'green'),
        title: card.name,
        lines: [card.kind === 'deckOfYear' ? `Your go-to in ${card.formatLabel} all year.` : `Your highest win rate in ${card.formatLabel}.`],
        // Wins and win rate only at 50% or better (the bundle leaves the
        // rate out below that), then trophies.
        figures: [
          { value: count(card.matches), label: card.matches === 1 ? 'match' : 'matches' },
          ...(card.winRate !== null
            ? [
                { value: count(card.wins), label: card.wins === 1 ? 'win' : 'wins' },
                { value: `${card.winRate}%`, label: 'win rate' },
              ]
            : []),
          ...(card.trophies > 0 ? [{ value: count(card.trophies), label: card.trophies === 1 ? 'trophy' : 'trophies' }] : []),
        ],
        art: card.art,
        deck: { colors: card.colors, cards: card.cards },
        badge:
          card.rank !== null && card.rank.topPercent <= 25
            ? `You're in the top ${card.rank.topPercent}% of MyMTGO ${card.rank.archetype} players by matches played.`
            : null,
      }
    case 'mvpCard':
      return {
        ...base('Your MVP', 'green'),
        title: card.name,
        big: `+${card.impact}`,
        bigLabel: 'points of impact',
        lines: [
          card.winRate !== null
            ? `When you cast it you won ${card.winRate}% of games, ${plural(card.impact, 'point')} above your usual.`
            : `Casting it lifted your win rate by ${plural(card.impact, 'point')}.`,
        ],
        figures: [
          { value: count(card.cast), label: card.cast === 1 ? 'cast' : 'casts' },
          { value: count(card.games), label: card.games === 1 ? 'game' : 'games' },
          ...(card.winRate !== null ? [{ value: `${card.winRate}%`, label: 'won when cast' }] : []),
        ],
        art: card.art,
        layout: 'spotlight',
        spotlight: card.image,
        runnersUp: runnersUp('Also pulling their weight', card.runnersUp, (row) => `+${row.value}`),
      }
    case 'openingHand':
      return {
        ...base('Always in your opener', 'blue'),
        title: card.name,
        big: `${share(card.count, card.games)}%`,
        bigLabel: 'of your opening hands',
        lines: [`It was there to start ${plural(card.count, 'game')}.`],
        figures: [
          { value: count(card.count), label: card.count === 1 ? 'opening hand' : 'opening hands' },
          { value: count(card.games), label: card.games === 1 ? 'game' : 'games' },
        ],
        art: card.art,
        layout: 'spotlight',
        spotlight: card.image,
        runnersUp: runnersUp('Also in your opening hands', card.runnersUp, (row) => `${share(row.value, card.games)}%`),
      }
    case 'sideboardHero':
      return {
        ...base('Sideboard hero', 'purple'),
        title: card.name,
        big: count(card.count),
        bigLabel: card.count === 1 ? 'time brought in' : 'times brought in',
        lines: [`Your answer when the plan needed changing, in for ${share(card.count, card.games)}% of your games after sideboarding.`],
        figures: [
          { value: count(card.count), label: card.count === 1 ? 'time sided in' : 'times sided in' },
          { value: count(card.games), label: card.games === 1 ? 'game after sideboarding' : 'games after sideboarding' },
        ],
        art: card.art,
        layout: 'spotlight',
        spotlight: card.image,
        runnersUp: runnersUp('Also off the bench', card.runnersUp, (row) => plural(row.value, 'time')),
      }
    case 'mostFaced':
      return {
        ...base('Your most faced deck', 'blue'),
        title: card.archetype,
        lines: ['Wherever you queued, it was waiting.'],
        figures: [
          { value: count(card.matches), label: card.matches === 1 ? 'match against it' : 'matches against it' },
          { value: count(card.uniqueArchetypes), label: card.uniqueArchetypes === 1 ? 'deck faced in all' : 'different decks faced' },
        ],
        art: card.look.art,
        deck: { colors: card.look.colors, cards: card.look.cards },
      }
    case 'bestMatchup':
      return {
        ...base('Your best matchup', 'green'),
        title: card.archetype,
        lines: ['They never saw you coming.'],
        figures: [
          { value: count(card.wins), label: card.wins === 1 ? 'win' : 'wins' },
          { value: count(card.matches), label: card.matches === 1 ? 'match' : 'matches' },
          { value: `${card.winRate}%`, label: 'win rate' },
        ],
        art: card.look.art,
        deck: { colors: card.look.colors, cards: card.look.cards },
      }
    case 'toughestRival':
      return {
        ...base('Your toughest rival', 'amber'),
        title: card.archetype,
        lines: [`They gave you a hard time, and you still beat them ${plural(card.wins, 'time')}.`],
        figures: [
          { value: count(card.wins), label: card.wins === 1 ? 'win' : 'wins' },
          { value: count(card.matches), label: card.matches === 1 ? 'match' : 'matches' },
        ],
        art: card.look.art,
        deck: { colors: card.look.colors, cards: card.look.cards },
      }
    case 'comebacks':
      return {
        ...base('Never give up, never surrender', 'amber'),
        title: 'Back from the brink',
        big: count(card.winsAfterMulligan),
        bigLabel: card.winsAfterMulligan === 1 ? 'game won after a mulligan' : 'games won after a mulligan',
        lines: ['Seven cards are overrated anyway.'],
        figures: card.quickestWinTurns !== null ? [{ value: `Turn ${card.quickestWinTurns}`, label: 'your quickest win' }] : [],
        art: COMEBACK_ART,
        layout: 'spotlight',
        credit: 'Persist · art by Milivoj Ćeran',
      }
    case 'playDraw': {
      const play = card.side === 'play'
      return {
        ...base(play ? 'On the play' : 'On the draw', 'blue'),
        title: play ? 'First to the table' : 'Patience pays',
        big: `${card.winRate}%`,
        bigLabel: play ? 'of games won on the play' : 'of games won on the draw',
        lines: [play ? 'Going first suited you. You set the pace and made them answer.' : 'An extra card was all you needed. You took your time and won anyway.'],
        figures: [{ value: count(card.games), label: card.games === 1 ? `game ${play ? 'on the play' : 'on the draw'}` : `games ${play ? 'on the play' : 'on the draw'}` }],
        art: PLAY_DRAW_ART[card.side].art,
        artStrength: 'full',
        layout: 'spotlight',
        credit: PLAY_DRAW_ART[card.side].credit,
      }
    }
    case 'trophies': {
      let title = 'Every run counts'
      if (card.leagueTrophies > 0) title = `You won ${plural(card.leagueTrophies, 'league')}!`
      else if (card.top8s > 0) title = `${count(card.top8s)} Top 8${card.top8s === 1 ? '' : 's'}!`
      let lines = ['Win or learn, every league made you better.']
      if (card.bestFinish) lines = [`Best finish: ${card.bestFinish}.`]
      else if (card.leagueTrophies > 0 || card.top8s > 0) lines = []
      return {
        ...base('Trophy cabinet', 'amber'),
        title,
        big: count(card.leagueRuns),
        bigLabel: card.leagueRuns === 1 ? 'league run' : 'league runs',
        lines,
        figures: [
          ...(card.leagueTrophies > 0
            ? [{ value: count(card.leagueTrophies), label: card.leagueTrophies === 1 ? 'league trophy' : 'league trophies' }]
            : []),
          ...(card.top8s > 0 ? [{ value: count(card.top8s), label: card.top8s === 1 ? 'Top 8' : 'Top 8s' }] : []),
        ],
        art: TROPHY_ART.art,
        artStrength: 'full',
        cabinet: card.leagueTrophies > 0 || card.top8s > 0 ? { trophies: card.leagueTrophies, top8s: card.top8s } : null,
        layout: 'spotlight',
        credit: TROPHY_ART.credit,
      }
    }
    case 'limited':
      return {
        ...base('Limited', 'purple'),
        title: card.drafts === 1 ? 'Your first draft' : 'Fresh packs, every time',
        big: count(card.drafts),
        bigLabel: card.drafts === 1 ? 'draft this year' : 'drafts this year',
        limited: {
          drafts: card.drafts,
          pair: card.colourPair,
          colors: card.colourPair ? (COLOUR_GROUPS[card.colourPair] ?? []) : [],
          record: card.bestRecord,
        },
        art: LIMITED_ART.art,
        credit: LIMITED_ART.credit,
      }
    case 'playerType':
      return {
        ...base('Your player type', 'purple'),
        title: PLAYER_TYPES[card.label].name,
        lines: [PLAYER_TYPES[card.label].line],
        pills: card.pills,
        persona: PERSONA_ART[card.label],
      }
    case 'wrap':
      return {
        ...base("That's a wrap", 'blue'),
        title: `See you in ${bundle.year + 1}`,
        lines: ['Thanks for playing with MyMTGO.'],
        recap: [
          { value: count(bundle.totals.matches), label: bundle.totals.matches === 1 ? 'match' : 'matches' },
          { value: count(bundle.totals.games), label: bundle.totals.games === 1 ? 'game' : 'games' },
          { value: count(bundle.totals.hours), label: bundle.totals.hours === 1 ? 'hour' : 'hours' },
        ],
        closing: true,
      }
  }
}
