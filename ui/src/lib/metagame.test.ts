import { describe, expect, it } from 'vitest'
import type { MetagameTileView, SeenCardView } from '@/bindings'
import {
  ARCHETYPE_TABS,
  arenaSelection,
  rankIconFile,
  windowLabel,
  metagameKeys,
  METAGAME_FORMAT_KEY,
  METAGAME_TIMEFRAME_KEY,
  archetypeLocation,
  archetypeTab,
  chosenTimeframe,
  filterTiles,
  searchQuery,
  metagameLocation,
  readRememberedTimeframe,
  sidebarFilters,
  writeRememberedTimeframe,
  dayText,
  differenceText,
  differenceTone,
  globalNotice,
  globalText,
  headerTitle,
  imageName,
  rangeText,
  readRememberedFormat,
  shareText,
  seenCaption,
  splitSeen,
  withoutArchetypeText,
  writeRememberedFormat,
  tileBackdrop,
} from './metagame'

function tile(overrides: Partial<MetagameTileView> = {}): MetagameTileView {
  return {
    archetypeId: 1,
    name: 'Qstorm',
    colors: ['R'],
    source: 'downloaded',
    retired: false,
    offlineHeld: false,
    matches: 4,
    wins: 2,
    losses: 1,
    draws: 1,
    winRate: 50,
    record: '2 - 1 - 1',
    share: 10,
    comparable: true,
    globalShare: 6.2,
    difference: 3.8,
    cards: [],
    ...overrides,
  }
}

describe('the tiles (spec section 9)', () => {
  it('fills a tile with the art of its first card that has some', () => {
    const card = (name: string, art: string | null, localArt: string | null = null) => ({
      name,
      image: `https://cards.scryfall.io/${name}.jpg`,
      localImage: null,
      art,
      localArt,
    })
    expect(tileBackdrop(tile())).toBeNull()
    expect(tileBackdrop(tile({ cards: [card('a', null), card('b', 'https://cards.scryfall.io/b-art.jpg')] }))?.name).toBe('b')
    expect(tileBackdrop(tile({ cards: [card('a', null, '12_art.jpg')] }))?.name).toBe('a')
  })

  it('shows shares with one decimal', () => {
    expect(shareText(10)).toBe('10.0%')
    expect(shareText(33.3)).toBe('33.3%')
  })

  it('shows the global share and a signed difference in points', () => {
    expect(globalText(tile(), 'ready')).toBe('6.2%')
    expect(differenceText(tile(), 'ready')).toBe('+3.8 pts')
    expect(differenceText(tile({ difference: -2.3 }), 'ready')).toBe('-2.3 pts')
    expect(differenceText(tile({ difference: 0 }), 'ready')).toBe('0.0 pts')
    expect(differenceTone(tile(), 'ready')).toBe('up')
    expect(differenceTone(tile({ difference: -2.3 }), 'ready')).toBe('down')
    expect(differenceTone(tile({ difference: 0 }), 'ready')).toBe('none')
  })

  it('shows "-" for manual, fallback, retired and unlisted archetypes', () => {
    const manual = tile({ comparable: false, globalShare: null, difference: null })
    expect(globalText(manual, 'ready')).toBe('-')
    expect(differenceText(manual, 'ready')).toBe('-')
    // Still "-" offline: the website never lists it.
    expect(globalText(manual, 'unavailable')).toBe('-')
    const unlisted = tile({ globalShare: null, difference: null })
    expect(globalText(unlisted, 'ready')).toBe('-')
    expect(differenceText(unlisted, 'ready')).toBe('-')
  })

  it('leaves the global column blank without the shares', () => {
    expect(globalText(tile(), 'unavailable')).toBe('')
    expect(differenceText(tile(), 'loading')).toBe('')
    expect(differenceTone(tile(), 'loading')).toBe('none')
    expect(globalNotice('unavailable')).toBe('Global shares need a connection.')
    expect(globalNotice('loading')).toBe('Loading global shares…')
    expect(globalNotice('ready')).toBeNull()
  })

  it('counts matches without an opponent archetype', () => {
    expect(withoutArchetypeText(0)).toBeNull()
    expect(withoutArchetypeText(1)).toBe('1 match without an opponent archetype')
    expect(withoutArchetypeText(3)).toBe('3 matches without an opponent archetype')
  })
})

describe('the timeframes and the header', () => {
  it('titles the shareable header', () => {
    expect(headerTitle('Modern', 'Last 30 days')).toBe('Modern metagame · Last 30 days')
    expect(rangeText('2027-02-09', '2027-03-10')).toBe('9 Feb 2027 to 10 Mar 2027')
    expect(rangeText(null, null)).toBeNull()
    expect(dayText('2027-12-01')).toBe('1 Dec 2027')
    expect(imageName('modern', 'last7')).toBe('modern-metagame-last7')
  })

  it('remembers the last format', () => {
    const rows = new Map<string, string>()
    const storage = {
      getItem: (key: string) => rows.get(key) ?? null,
      setItem: (key: string, value: string) => void rows.set(key, value),
    }
    expect(readRememberedFormat(storage)).toBeNull()
    writeRememberedFormat(storage, 'pioneer')
    expect(rows.get(METAGAME_FORMAT_KEY)).toBe('pioneer')
    expect(readRememberedFormat(storage)).toBe('pioneer')
    rows.set(METAGAME_FORMAT_KEY, '<junk>')
    expect(readRememberedFormat(storage)).toBeNull()
    writeRememberedFormat(storage, 'historic_brawl')
    expect(readRememberedFormat(storage)).toBe('historic_brawl')
  })

  it('forgets nothing loudly when storage is blocked or full', () => {
    const blocked = {
      getItem: (): string | null => {
        throw new Error('blocked')
      },
      setItem: () => {
        throw new Error('quota exceeded')
      },
    }
    expect(readRememberedFormat(blocked)).toBeNull()
    expect(readRememberedTimeframe(blocked)).toBeNull()
    expect(() => writeRememberedFormat(blocked, 'pioneer')).not.toThrow()
    expect(() => writeRememberedTimeframe(blocked, 'last7')).not.toThrow()
  })
})

describe('cards seen (spec section 10)', () => {
  it('sets the cards in no typical list apart', () => {
    const card = (name: string, inTypical: boolean): SeenCardView => ({
      key: name,
      catalogId: 1,
      name,
      matches: 1,
      mostCopies: 1,
      inTypical,
      image: null,
      localImage: null,
    })
    const split = splitSeen([card('Qbolt', true), card('Qtech', false), card('Qritual', true)])
    expect(split.typical.map((c) => c.name)).toEqual(['Qbolt', 'Qritual'])
    expect(split.other.map((c) => c.name)).toEqual(['Qtech'])
  })

  it('captions each card with its most copies and its matches', () => {
    expect(seenCaption({ mostCopies: 4, matches: 3 })).toBe('up to 4 · 3 matches')
    expect(seenCaption({ mostCopies: 1, matches: 1 })).toBe('up to 1 · 1 match')
  })
})

describe('the remembered metagame filters (issue #326)', () => {
  function memory(): { getItem(key: string): string | null; setItem(key: string, value: string): void; data: Map<string, string> } {
    const data = new Map<string, string>()
    return { data, getItem: (key) => data.get(key) ?? null, setItem: (key, value) => void data.set(key, value) }
  }

  it('keeps "All time" when the page is opened again without a query', () => {
    const storage = memory()
    writeRememberedTimeframe(storage, 'alltime')
    expect(storage.data.get(METAGAME_TIMEFRAME_KEY)).toBe('alltime')
    expect(chosenTimeframe(undefined, readRememberedTimeframe(storage))).toBe('alltime')
  })

  it("lets the route's timeframe win, and falls back to the default", () => {
    expect(chosenTimeframe('last7', 'alltime')).toBe('last7')
    expect(chosenTimeframe('junk', 'alltime')).toBe('alltime')
    expect(chosenTimeframe(undefined, null)).toBe('last30')
  })

  it('ignores junk in storage and never writes junk', () => {
    const storage = memory()
    storage.setItem(METAGAME_TIMEFRAME_KEY, 'forever')
    expect(readRememberedTimeframe(storage)).toBeNull()
    writeRememberedTimeframe(storage, 'nonsense')
    expect(storage.data.get(METAGAME_TIMEFRAME_KEY)).toBe('forever')
  })

  it('goes back to the metagame with the format and timeframe', () => {
    expect(metagameLocation('modern', 'alltime')).toEqual({ path: '/metagame/modern', query: { timeframe: 'alltime' } })
    expect(metagameLocation(null, 'last7')).toEqual({ path: '/metagame', query: { timeframe: 'last7' } })
  })
})

describe('the archetype tabs (issue #326)', () => {
  it('reads the tab from the route, Decklist by default', () => {
    expect(ARCHETYPE_TABS.map((tab) => tab.label)).toEqual(['Decklist', 'Matches', 'Sideboard guides'])
    expect(archetypeTab('matches')).toBe('matches')
    expect(archetypeTab('sideboard-guides')).toBe('sideboard-guides')
    expect(archetypeTab(undefined)).toBe('decklist')
    expect(archetypeTab('nonsense')).toBe('decklist')
  })

  it('links to another archetype on the same tab and timeframe', () => {
    expect(archetypeLocation(7, 'matches', 'alltime')).toEqual({
      name: 'archetype',
      params: { id: '7', tab: 'matches' },
      query: { timeframe: 'alltime' },
    })
    expect(archetypeLocation(7, 'decklist', 'last30', 'q-two').query).toEqual({ timeframe: 'last30', version: 'q-two' })
  })
})

describe('the archetype side menu filters (issue #326)', () => {
  it("shares the manage screen's remembered filters, else opens on the archetype's format", () => {
    expect(sidebarFilters({ search: 'qstorm', format: '' }, 'modern')).toEqual({ search: 'qstorm', format: '' })
    expect(sidebarFilters(null, 'modern')).toEqual({ search: '', format: 'modern' })
    expect(sidebarFilters(null, null)).toEqual({ search: '', format: '' })
  })
})

describe('metagame search', () => {
  const tiles = [{ name: 'Izzet Prowess' }, { name: 'Boros Energy (Provisional)' }, { name: 'Amulet Titan' }]

  it('matches anywhere in the name, ignoring case', () => {
    expect(filterTiles(tiles, 'PROW').map((t) => t.name)).toEqual(['Izzet Prowess'])
    expect(filterTiles(tiles, 'tan').map((t) => t.name)).toEqual(['Amulet Titan'])
  })

  it('ignores the Provisional suffix and keeps everything for blank text', () => {
    expect(filterTiles(tiles, 'provisional')).toEqual([])
    expect(filterTiles(tiles, 'energy')).toHaveLength(1)
    expect(filterTiles(tiles, '  ')).toHaveLength(3)
  })

  it('keeps the search in the route query for the way in and back', () => {
    expect(metagameLocation('modern', 'last7', 'tit').query).toEqual({ timeframe: 'last7', q: 'tit' })
    expect(metagameLocation('modern', 'last7').query).toEqual({ timeframe: 'last7' })
    expect(archetypeLocation(4, 'decklist', 'last7', null, 'tit').query).toEqual({ timeframe: 'last7', q: 'tit' })
    expect(searchQuery('tit')).toBe('tit')
    expect(searchQuery(undefined)).toBe('')
  })
})

describe('the Arena month and rank selection', () => {
  it('sends the route month and rank only on Arena', () => {
    expect(arenaSelection('arena', '2026-11', 'gold')).toEqual({ month: '2026-11', rank: 'gold' })
    expect(arenaSelection('arena', '2026-11', 'spark')).toEqual({ month: '2026-11', rank: 'spark' })
    expect(arenaSelection('arena', undefined, undefined)).toEqual({ month: null, rank: null })
    expect(arenaSelection('arena', ['x'], 'bogus')).toEqual({ month: null, rank: null })
  })

  it('leaves the MTGO request unchanged', () => {
    expect(arenaSelection('mtgo', '2026-11', 'gold')).toEqual({ month: null, rank: null })
  })

  it('keys the page query by month and rank', () => {
    expect(metagameKeys.page('standard', 'last30', '2026-11', 'gold')).not.toEqual(
      metagameKeys.page('standard', 'last30', null, null),
    )
  })

  it('names the rank icon files', () => {
    expect(rankIconFile('gold')).toBe('constructed_gold_t1')
    expect(rankIconFile('mythic')).toBe('constructed_mythic')
    expect(rankIconFile('spark')).toBe('constructed_spark_t1')
    expect(rankIconFile('unranked')).toBe('constructed_unranked')
  })
})

describe('Arena month and rank in links', () => {
  it('writes month and rank into the metagame and archetype links only when set', () => {
    expect(metagameLocation('standard', 'last30', '', { month: '2026-11', rank: 'gold' }).query).toEqual({
      timeframe: 'last30',
      month: '2026-11',
      rank: 'gold',
    })
    expect(metagameLocation('standard', 'last30', '', { month: null, rank: null }).query).toEqual({ timeframe: 'last30' })
    expect(archetypeLocation(3, 'decklist', 'last30', null, 'q', { month: '2026-10', rank: null }).query).toEqual({
      timeframe: 'last30',
      q: 'q',
      month: '2026-10',
    })
    expect(archetypeLocation(3, 'decklist', 'last30').query).toEqual({ timeframe: 'last30' })
  })
})

describe('the window label', () => {
  const months = [{ key: '2026-11', label: 'November 2026' }]
  it('is the month on Arena and the timeframe on MTGO', () => {
    expect(windowLabel(true, months, '2026-11', 'Last 30 days')).toBe('November 2026')
    expect(windowLabel(true, months, '2026-09', 'Last 30 days')).toBe('2026-09')
    expect(windowLabel(true, months, null, 'Last 30 days')).toBe('Last 30 days')
    expect(windowLabel(false, months, '2026-11', 'Last 30 days')).toBe('Last 30 days')
  })
})
