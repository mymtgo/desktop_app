import type { ArchetypeLook, Art, DeckCard, KeyCard, WrappedBundle, WrappedCard } from '@/bindings'

/**
 * Hand-made Wrapped bundles for the component and page tests only; the app
 * never imports them. Card and archetype names are real Magic names; no
 * player name appears anywhere.
 */
type TestProfile = 'light' | 'medium' | 'grinder' | 'limited' | 'weekender'

/** Scryfall art crops by set and number (the API redirects to the image), so the cards show real art while iterating. */
// A card's full image, by exact name.
function cardImage(name: string): Art {
  const remote = `https://api.scryfall.com/cards/named?exact=${encodeURIComponent(name)}&format=image&version=normal`
  return { remote, local: null, custom: null }
}

// A card's own art crop, by exact name.
function cardArt(name: string): Art {
  const remote = `https://api.scryfall.com/cards/named?exact=${encodeURIComponent(name)}&format=image&version=art_crop`
  return { remote, local: null, custom: null }
}

function keyCard(name: string): KeyCard {
  return { name, image: cardImage(name) }
}

// An opponent archetype's look: the art of its first card, its colours and
// its key cards.
function look(colors: string[], names: string[]): ArchetypeLook {
  return { art: cardArt(names[0]), colors, cards: names.map(keyCard) }
}

const BURN: Pick<DeckCard, 'colors' | 'cards' | 'rank'> = {
  colors: ['R'],
  rank: null,
  cards: ['Lightning Bolt', 'Chain Lightning', 'Rift Bolt', 'Lava Dart', 'Fireblast'].map(keyCard),
}

function scryfall(set: string, number: string): Art {
  return { remote: `https://api.scryfall.com/cards/${set}/${number}?format=image&version=art_crop`, local: null, custom: null }
}

/** 2026 in London: 1 Jan 00:00 to 1 Dec 00:00. */
const WINDOW = { startMs: 1_767_225_600_000, endMs: 1_796_083_200_000 }
const BUILT_AT = 1_796_090_000_000

// Every bundle opens with the year in Magic and its bans.
function bundle(totals: WrappedBundle['totals'], cards: WrappedCard[]): WrappedBundle {
  return { version: 1, year: 2026, window: WINDOW, builtAtMs: BUILT_AT, firstYear: true, totals, cards: [{ kind: 'magicYear' }, { kind: 'banned' }, ...cards] }
}

// 1 January to 30 November 2026.
const DAYS = 334

// Matches per day for the activity grid: `days` played days picked by a
// small seeded generator (weekends twice as likely when `weekends`), with
// `matches` spread over them, more in the later months.
function daily(matches: number, days: number, seed: number, weekends = false): number[] {
  let state = seed
  const next = () => {
    state = (state * 16_807) % 2_147_483_647
    return state / 2_147_483_647
  }
  const byDay = Array.from({ length: DAYS }, () => 0)
  const played: number[] = []
  while (played.length < Math.min(days, DAYS)) {
    const day = Math.floor(next() * DAYS)
    // 1 January 2026 is a Thursday: index 2 and 3 of each week are Saturday and Sunday.
    const weekend = (day + 3) % 7 >= 5
    const keep = weekends ? (weekend ? 1 : 0.4) : 0.4 + (day / DAYS) * 0.6
    if (!played.includes(day) && next() < keep) played.push(day)
  }
  played.forEach((day) => (byDay[day] = 1))
  for (let left = matches - played.length; left > 0; left -= 1) {
    const day = played[Math.floor(next() ** 0.6 * played.length)]
    byDay[day] += 1
  }
  return byDay
}

function hourStrip(peak: number): number[] {
  const hours: number[] = []
  for (let hour = 0; hour < 24; hour += 1) {
    const evening = hour >= 17 ? (hour - 16) * 6 : 0
    const day = hour >= 8 && hour < 17 ? 4 : 0
    hours.push(hour === peak ? 70 : evening + day)
  }
  return hours
}

const light = bundle({ matches: 34, games: 81, wins: 18, losses: 16, draws: 0, hours: 14 }, [
  { kind: 'yearInNumbers', matches: 34, games: 81, hours: 14, daysPlayed: 9, byDay: daily(34, 9, 11), topPercent: null },
  {
    kind: 'deckOfYear',
    name: 'Mono Red Burn',
    formatLabel: 'Pauper',
    matches: 21,
    wins: 10,
    losses: 11,
    winRate: null,
    trophies: 0,
    art: scryfall('m10', '146'),
    ...BURN,
  },
  { kind: 'playerType', label: 'deckLoyalist', pills: ['34 matches', 'Mono Red Burn'] },
  { kind: 'wrap' },
])

const medium = bundle({ matches: 296, games: 702, wins: 158, losses: 136, draws: 2, hours: 118 }, [
  { kind: 'yearInNumbers', matches: 296, games: 702, hours: 118, daysPlayed: 71, byDay: daily(296, 71, 23), topPercent: 34 },
  {
    kind: 'formats',
    favourite: { label: 'Modern', matches: 201, percent: 68, art: scryfall('m12', '186') },
    mentions: [{ label: 'Pioneer', matches: 95, percent: 32 }],
  },
  { kind: 'whenYouPlay', peakWeekday: 5, peakHour: 21, busiestMonth: 3, byHour: hourStrip(21), period: 'evening', weekends: false },
  {
    kind: 'deckOfYear',
    name: 'Amulet Titan',
    formatLabel: 'Modern',
    matches: 164,
    wins: 90,
    losses: 74,
    winRate: 55,
    trophies: 0,
    art: scryfall('m12', '186'),
    colors: ['G'],
    cards: ['Amulet of Vigor', 'Primeval Titan', "Summoner's Pact", 'Arboreal Grazer', 'Scapeshift'].map(keyCard),
    rank: { archetype: 'Amulet Titan', topPercent: 18 },
  },
  {
    kind: 'mvpCard',
    name: 'Primeval Titan',
    cast: 142,
    games: 120,
    winRate: 61,
    impact: 6,
    art: cardArt('Primeval Titan'),
    image: cardImage('Primeval Titan'),
    runnersUp: [{ name: 'Amulet of Vigor', value: 4 }, { name: 'Arboreal Grazer', value: 2 }],
  },
  {
    kind: 'mostFaced',
    archetype: 'Boros Energy',
    matches: 38,
    uniqueArchetypes: 41,
    look: look(['R', 'W'], ["Phlage, Titan of Fire's Fury", 'Ocelot Pride', 'Guide of Souls', 'Ragavan, Nimble Pilferer', 'Galvanic Discharge']),
  },
  { kind: 'comebacks', winsAfterMulligan: 22, quickestWinTurns: 3 },
  { kind: 'playerType', label: 'allRounder', pills: ['296 matches', 'Amulet Titan', '2 formats'] },
  { kind: 'wrap' },
])

const grinder = bundle({ matches: 1284, games: 3046, wins: 712, losses: 566, draws: 6, hours: 461 }, [
  { kind: 'yearInNumbers', matches: 1284, games: 3046, hours: 461, daysPlayed: 214, byDay: daily(1284, 214, 37), topPercent: 3 },
  {
    kind: 'formats',
    favourite: { label: 'Modern', matches: 899, percent: 70, art: scryfall('mh3', '255') },
    mentions: [
      { label: 'Pauper', matches: 231, percent: 18 },
      { label: 'Limited', matches: 116, percent: 9 },
      { label: 'Arena Standard', matches: 38, percent: 3 },
    ],
  },
  { kind: 'whenYouPlay', peakWeekday: 1, peakHour: 23, busiestMonth: 8, byHour: hourStrip(23), period: 'night', weekends: false },
  {
    kind: 'deckOfYear',
    name: 'Boros Energy (the one with too many Phlage)',
    formatLabel: 'Modern',
    matches: 412,
    wins: 239,
    losses: 173,
    winRate: 58,
    trophies: 1,
    art: cardArt("Phlage, Titan of Fire's Fury"),
    colors: ['R', 'W'],
    cards: ['Ragavan, Nimble Pilferer', 'Guide of Souls', "Phlage, Titan of Fire's Fury", 'Ocelot Pride', 'Galvanic Discharge'].map(keyCard),
    rank: { archetype: 'Boros Energy', topPercent: 5 },
  },
  {
    kind: 'bestDeck',
    name: 'Dimir Faeries',
    formatLabel: 'Pauper',
    matches: 64,
    wins: 41,
    losses: 23,
    winRate: 64,
    trophies: 1,
    art: scryfall('lrw', '63'),
    colors: ['U', 'B'],
    cards: ['Spellstutter Sprite', 'Faerie Seer', 'Ninja of the Deep Hours', 'Counterspell', 'Brainstorm'].map(keyCard),
    rank: { archetype: 'Dimir Faeries', topPercent: 40 },
  },
  {
    kind: 'mvpCard',
    name: 'Ragavan, Nimble Pilferer',
    cast: 631,
    games: 540,
    winRate: 64,
    impact: 9,
    art: cardArt('Ragavan, Nimble Pilferer'),
    image: cardImage('Ragavan, Nimble Pilferer'),
    runnersUp: [
      { name: "Phlage, Titan of Fire's Fury", value: 7 },
      { name: 'Ocelot Pride', value: 5 },
      { name: 'Guide of Souls', value: 3 },
    ],
  },
  {
    kind: 'openingHand',
    name: 'Guide of Souls',
    count: 488,
    games: 1190,
    art: cardArt('Guide of Souls'),
    image: cardImage('Guide of Souls'),
    runnersUp: [
      { name: 'Ocelot Pride', value: 431 },
      { name: 'Galvanic Discharge', value: 402 },
      { name: 'Ragavan, Nimble Pilferer', value: 366 },
    ],
  },
  {
    kind: 'sideboardHero',
    name: 'Wear // Tear',
    count: 97,
    games: 640,
    art: cardArt('Wear // Tear'),
    image: cardImage('Wear // Tear'),
    runnersUp: [
      { name: 'Blood Moon', value: 71 },
      { name: 'Surgical Extraction', value: 58 },
      { name: 'Wrath of the Skies', value: 32 },
    ],
  },
  {
    kind: 'mostFaced',
    archetype: 'Domain Zoo',
    matches: 131,
    uniqueArchetypes: 88,
    look: look(['W', 'U', 'R', 'G'], ['Scion of Draco', 'Territorial Kavu', 'Leyline Binding', 'Ragavan, Nimble Pilferer', 'Stubborn Denial']),
  },
  {
    kind: 'bestMatchup',
    archetype: 'Living End',
    matches: 24,
    wins: 19,
    winRate: 79,
    look: look(['B', 'R', 'G'], ['Living End', 'Shardless Agent', 'Violent Outburst', 'Architects of Will', 'Striped Riverwinder']),
  },
  {
    kind: 'toughestRival',
    archetype: 'Eldrazi Ramp',
    matches: 41,
    wins: 17,
    look: look(['G'], ['Emrakul, the Promised End', 'Ulamog, the Defiler', 'Devourer of Destiny', 'Eldrazi Temple', "Kozilek's Command"]),
  },
  { kind: 'comebacks', winsAfterMulligan: 93, quickestWinTurns: 3 },
  { kind: 'playDraw', side: 'play', winRate: 61, games: 1530 },
  { kind: 'trophies', leagueTrophies: 2, leagueRuns: 151, top8s: 1, bestFinish: 'Top 8, Modern Challenge, 14 Sep' },
  { kind: 'limited', drafts: 29, colourPair: 'Golgari', bestRecord: '3-0' },
  { kind: 'playerType', label: 'grinder', pills: ['1,284 matches', '2 trophies', 'Boros Energy'] },
  { kind: 'wrap' },
])

const limited = bundle({ matches: 120, games: 271, wins: 66, losses: 54, draws: 0, hours: 63 }, [
  { kind: 'yearInNumbers', matches: 120, games: 271, hours: 63, daysPlayed: 37, byDay: daily(120, 37, 41, true), topPercent: 41 },
  {
    kind: 'formats',
    favourite: { label: 'Duskmourn draft', matches: 72, percent: 60, art: scryfall('lrw', '63') },
    mentions: [
      { label: 'Bloomburrow draft', matches: 36, percent: 30 },
      { label: 'Arena Premier Draft', matches: 12, percent: 10 },
    ],
  },
  { kind: 'whenYouPlay', peakWeekday: 6, peakHour: 15, busiestMonth: 10, byHour: hourStrip(15), period: 'afternoon', weekends: true },
  { kind: 'trophies', leagueTrophies: 0, leagueRuns: 34, top8s: 0, bestFinish: null },
  { kind: 'limited', drafts: 41, colourPair: 'Selesnya', bestRecord: '3-0' },
  { kind: 'playerType', label: 'limitedLover', pills: ['41 drafts', 'Selesnya', '3-0'] },
  { kind: 'wrap' },
])

// A morning, weekend player: the "Early bird" scene.
const weekender = bundle({ matches: 88, games: 204, wins: 47, losses: 41, draws: 0, hours: 36 }, [
  { kind: 'yearInNumbers', matches: 88, games: 204, hours: 36, daysPlayed: 30, byDay: daily(88, 30, 53, true), topPercent: null },
  {
    kind: 'whenYouPlay',
    peakWeekday: 5,
    peakHour: 8,
    busiestMonth: 2,
    byHour: [0, 0, 0, 0, 0, 0, 4, 12, 20, 16, 10, 6, 3, 2, 2, 1, 1, 2, 2, 2, 1, 0, 0, 0],
    period: 'morning',
    weekends: true,
  },
  {
    kind: 'deckOfYear',
    name: 'Mono Red Burn',
    formatLabel: 'Pauper',
    matches: 61,
    wins: 34,
    losses: 27,
    winRate: 56,
    trophies: 0,
    art: scryfall('m10', '146'),
    ...BURN,
  },
  { kind: 'playerType', label: 'deckLoyalist', pills: ['88 matches', 'Saturday mornings'] },
  { kind: 'wrap' },
])

export const TEST_BUNDLES: Record<TestProfile, WrappedBundle> = { light, medium, grinder, limited, weekender }
