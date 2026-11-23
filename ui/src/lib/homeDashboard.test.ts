import { describe, expect, it } from 'vitest'
import type { BreakdownRowView, FormatSwitchOptionView, LeagueRunBarView, LeagueRunsView, RecentEntryView } from '@/bindings'
import {
  ALL_SETS,
  shareText,
  breakdownDescription,
  breakdownRows,
  segmentClass,
  segmentWidth,
  askedFormat,
  deltaClass,
  deltaText,
  formatChoices,
  ladderMax,
  leagueLadders,
  leagueRunsHeader,
  limitedKindLabel,
  limitedPickName,
  limitedSetTitle,
  markClass,
  noMatchesText,
  showFormatSwitch,
  pickSetChoices,
  recentLink,
  recentMatchEvent,
  recentSessionLabel,
  recentTitle,
  sessionMark,
  winRateClass,
  withQueryValue,
} from './homeDashboard'

const MODERN: FormatSwitchOptionView = { key: 'modern', label: 'Modern' }

function bar(fields: Partial<LeagueRunBarView>): LeagueRunBarView {
  return {
    key: 'q-run',
    kind: 'constructed',
    rounds: 5,
    wins: 4,
    losses: 1,
    state: 'complete',
    name: 'Qleague',
    subject: 'Qburn',
    startedAtMs: 0,
    height: 0.8,
    tone: 'winning',
    deckId: 7,
    ...fields,
  }
}

describe('the format switch', () => {
  it('offers Constructed and Limited, then the formats played, never All', () => {
    expect(formatChoices([MODERN]).map((choice) => choice.value)).toEqual(['constructed', 'limited', 'modern'])
    expect(formatChoices([]).map((choice) => choice.label)).toEqual(['Constructed', 'Limited'])
    expect(formatChoices([MODERN], 'arena').map((choice) => choice.value)).toEqual(['constructed', 'modern'])
  })

  it('labels the combined Arena choice All formats and leaves MTGO alone', () => {
    expect(formatChoices([MODERN], 'arena').map((choice) => choice.label)).toEqual(['All formats', 'Modern'])
    expect(formatChoices([MODERN], 'mtgo').map((choice) => choice.label)).toEqual(['Constructed', 'Limited', 'Modern'])
  })

  it('reads Endstep like Arena: no Limited, one combined choice', () => {
    expect(formatChoices([MODERN], 'endstep').map((choice) => choice.label)).toEqual(['All formats', 'Modern'])
    expect(showFormatSwitch([MODERN], 'endstep')).toBe(false)
    expect(showFormatSwitch([MODERN, { key: 'historic', label: 'Historic' }], 'endstep')).toBe(true)
    expect(noMatchesText('constructed', [MODERN], 'endstep')).toBe('No matches in this timeframe')
  })

  it('hides the Arena switch for an account with one format or none', () => {
    expect(showFormatSwitch([MODERN], 'arena')).toBe(false)
    expect(showFormatSwitch([], 'arena')).toBe(false)
    expect(showFormatSwitch([MODERN, { key: 'historic', label: 'Historic' }], 'arena')).toBe(true)
    expect(showFormatSwitch([MODERN], 'mtgo')).toBe(true)
    expect(showFormatSwitch([], 'mtgo')).toBe(true)
  })

  it('asks for the URL format, else empty (the format last played)', () => {
    expect(askedFormat('modern')).toBe('modern')
    expect(askedFormat(undefined)).toBe('')
    expect(askedFormat('  ')).toBe('')
    expect(askedFormat(['modern'])).toBe('')
  })

  it('writes a value into the query and leaves the default out', () => {
    expect(withQueryValue({ timeframe: 'week' }, 'format', 'modern', 'all')).toEqual({ timeframe: 'week', format: 'modern' })
    expect(withQueryValue({ timeframe: 'week', format: 'modern' }, 'format', 'all', 'all')).toEqual({ timeframe: 'week' })
  })

  it('names the format in the empty Results text', () => {
    expect(noMatchesText('all', [MODERN])).toBe('No matches in this timeframe')
    expect(noMatchesText('limited', [MODERN])).toBe('No limited matches in this timeframe')
    expect(noMatchesText('constructed', [MODERN])).toBe('No constructed matches in this timeframe')
    expect(noMatchesText('modern', [MODERN])).toBe('No Modern matches in this timeframe')
  })

  it('reads No matches for the combined Arena choice', () => {
    expect(noMatchesText('constructed', [MODERN], 'arena')).toBe('No matches in this timeframe')
    expect(noMatchesText('modern', [MODERN], 'arena')).toBe('No Modern matches in this timeframe')
  })
})

describe('League results', () => {
  it('heads the card with runs, trophies and average wins', () => {
    const view: LeagueRunsView = { runs: [bar({}), bar({ key: 'q-2' })], trophies: 1, avgWins: 2.4, axisRounds: null }
    expect(leagueRunsHeader(view)).toBe('2 leagues · 1 trophy · 2.4 avg wins')
    expect(leagueRunsHeader({ runs: [bar({})], trophies: 0, avgWins: null, axisRounds: 5 })).toBe('1 league · 0 trophies')
  })
})

describe('marks and numbers', () => {
  it('marks an unknown outcome ? and a draw D, never L', () => {
    expect(sessionMark('win')).toEqual({ text: 'W', tone: 'win' })
    expect(sessionMark('loss')).toEqual({ text: 'L', tone: 'loss' })
    expect(sessionMark('draw')).toEqual({ text: 'D', tone: 'neutral' })
    expect(sessionMark('unknown')).toEqual({ text: '?', tone: 'neutral' })
    expect(markClass('neutral')).toBe('bg-muted text-muted-foreground')
  })

  it('colours changes and results only', () => {
    expect(deltaText(5)).toBe('+5%')
    expect(deltaText(-3)).toBe('-3%')
    expect(deltaText(0)).toBe('0%')
    expect(deltaClass(4)).toBe('bg-success/15 text-success')
    expect(deltaClass(-1)).toBe('bg-destructive/15 text-destructive')
    expect(deltaClass(0)).toBe('bg-muted text-muted-foreground')
    expect(winRateClass(50)).toBe('text-success')
    expect(winRateClass(49)).toBe('text-destructive')
  })
})

describe('Recent sessions labels', () => {
  it('says Today and Yesterday, then the weekday, then the date', () => {
    expect(recentSessionLabel({ day: '2026-11-23', daysAgo: 0 })).toBe('Today')
    expect(recentSessionLabel({ day: '2026-11-22', daysAgo: 1 })).toBe('Yesterday')
    // 2026-11-20 is a Friday.
    expect(recentSessionLabel({ day: '2026-11-20', daysAgo: 3 })).toBe('Friday')
    expect(recentSessionLabel({ day: '2026-11-17', daysAgo: 6 })).toBe('Tuesday')
  })

  it('names a day a week or more back by its date', () => {
    expect(recentSessionLabel({ day: '2026-11-16', daysAgo: 7 })).toBe('16 Nov')
    expect(recentSessionLabel({ day: '2026-10-03', daysAgo: 51 })).toBe('3 Oct')
  })

  it('falls back to the day text when it is not a day', () => {
    expect(recentSessionLabel({ day: 'nonsense', daysAgo: 3 })).toBe('nonsense')
  })
})

describe('the side cards', () => {
  it('titles Recent by the format', () => {
    expect(recentTitle('limited')).toBe('Recent drafts')
    expect(recentTitle('all')).toBe('Recent decks and drafts')
    expect(recentTitle('modern')).toBe('Recent decks')
  })

  it('tags a recent match with its format and event', () => {
    expect(recentMatchEvent({ runKey: null, match: { format: 'Modern', matchType: 'League', limited: false } })).toBe(
      'Modern · League',
    )
    expect(recentMatchEvent({ runKey: null, match: { format: 'Pauper', matchType: null, limited: false } })).toBe('Pauper')
    expect(recentMatchEvent({ runKey: '0x:4', match: { format: 'Dqnwqnwqnw', matchType: 'League', limited: true } })).toBe(
      'League',
    )
    expect(recentMatchEvent({ runKey: null, match: { format: null, matchType: null, limited: true } })).toBe('Limited')
  })

  it('links a recent deck or draft', () => {
    const deck = { type: 'deck', id: 3 } as RecentEntryView
    const draft = { type: 'draft', key: '0x:4' } as RecentEntryView
    expect(recentLink(deck)).toBe('/decks/3')
    expect(recentLink(draft)).toBe('/limited/0x%3A4')
  })

  it('offers the drafted sets, then All', () => {
    expect(pickSetChoices([{ code: 'QNW', name: 'Q New Set' }])).toEqual([
      { value: 'QNW', label: 'Q New Set' },
      { value: ALL_SETS, label: 'All sets' },
    ])
  })

  it('keeps the limited text helpers', () => {
    expect(limitedSetTitle(null, 'HOB')).toBe('HOB')
    expect(limitedSetTitle(null, null)).toBe('Unknown set')
    expect(limitedKindLabel('sealed')).toBe('Sealed')
    expect(limitedPickName({ catalogId: 7, name: null, image: null, localImage: null, count: 2 })).toBe('Card #7')
  })
})

describe('League results ladders', () => {
  const labels = (rows: { label: string }[]) => rows.map((row) => row.label)

  it('lists every finish with zero rows, then Dropped, and no Unfinished without one', () => {
    const [ladder] = leagueLadders([
      bar({ key: '1', wins: 5, losses: 0, tone: 'trophy' }),
      bar({ key: '2', wins: 3, losses: 2 }),
      bar({ key: '3', wins: 3, losses: 2 }),
      bar({ key: '4', wins: 1, losses: 2, state: 'dropped', tone: 'dropped' }),
    ])
    expect(ladder.label).toBeNull()
    expect(labels(ladder.rows)).toEqual(['5-0', '4-1', '3-2', '2-3', '1-4', '0-5', 'Dropped'])
    expect(ladder.rows.map((row) => row.count)).toEqual([1, 0, 2, 0, 0, 0, 1])
    expect(ladder.rows.map((row) => row.tone)).toEqual(['trophy', 'empty', 'winning', 'empty', 'empty', 'empty', 'dropped'])
    expect(ladderMax(ladder)).toBe(2)
  })

  it('adds Unfinished only when a run ended early, and leaves out active runs', () => {
    const [ladder] = leagueLadders([
      bar({ key: '1', state: 'partial', tone: 'partial' }),
      bar({ key: '2', state: 'active', tone: 'active' }),
    ])
    expect(labels(ladder.rows).slice(-2)).toEqual(['Dropped', 'Unfinished'])
    expect(ladder.rows.reduce((total, row) => total + row.count, 0)).toBe(1)
  })

  it('puts an odd record in the bucket of its wins, clamped', () => {
    const [ladder] = leagueLadders([
      bar({ key: '1', wins: 7, losses: 0, tone: 'trophy' }),
      bar({ key: '2', wins: 0, losses: 0, tone: 'losing' }),
    ])
    expect(ladder.rows[0]).toEqual({ label: '5-0', count: 1, tone: 'trophy' })
    expect(ladder.rows[5]).toEqual({ label: '0-5', count: 1, tone: 'losing' })
    expect(ladderMax(leagueLadders([])[0] ?? { kind: '', label: null, rows: [] })).toBe(1)
  })

  it('gives each kind its own ladder, named when mixed, in constructed, draft, sealed order', () => {
    const ladders = leagueLadders([
      bar({ key: '1', kind: 'sealed', rounds: 5, wins: 4, losses: 1 }),
      bar({ key: '2', kind: 'draft', rounds: 3, wins: 3, losses: 0, tone: 'trophy' }),
      bar({ key: '3' }),
    ])
    expect(ladders.map((ladder) => ladder.label)).toEqual(['Constructed', 'Draft', 'Sealed'])
    expect(labels(ladders[1].rows)).toEqual(['3-0', '2-1', '1-2', '0-3', 'Dropped'])
    expect(labels(ladders[2].rows)[0]).toBe('5-0')
    expect(leagueLadders([])).toEqual([])
  })
})

describe('the Formats and Sets card', () => {
  const row = (key: string, matches: number, share: number): BreakdownRowView => ({
    key,
    label: key,
    matches,
    share,
    record: { wins: matches, losses: 0, draws: 0, total: matches, winRate: 100, label: `${matches} - 0` },
  })

  it('gives each segment a distinct, stable colour by order', () => {
    const colours = [0, 1, 2, 3, 4, 5].map(segmentClass)
    expect(new Set(colours).size).toBe(6)
    expect(segmentClass(1)).toBe(colours[1])
    expect(segmentClass(6)).toBe(colours[0])
  })

  it('keeps a sliver for a tiny share', () => {
    expect(segmentWidth(0)).toBe('1%')
    expect(segmentWidth(43)).toBe('43%')
  })

  it('describes the split by format or by set', () => {
    expect(breakdownDescription('formats')).toBe('Share of your matches by format, and your match win rate in each.')
    expect(breakdownDescription('sets')).toBe('Share of your matches by set, and your match win rate in each.')
  })

  it('lists every row up to five, and folds the zero shares into Other past five', () => {
    const five = [row('a', 50, 50), row('b', 49, 49), row('c', 1, 0), row('d', 1, 0), row('e', 1, 0)]
    expect(breakdownRows(five)).toEqual(five)
    const six = [...five, row('f', 1, 0)]
    const shown = breakdownRows(six)
    expect(shown.map((r) => r.label)).toEqual(['a', 'b', 'Other'])
    expect(shown[2].matches).toBe(4)
    expect(shown[2].record.winRate).toBe(100)
  })
})

describe('shareText', () => {
  it('says the count and its share', () => {
    expect(shareText(24, 'matches', 4, 'matches')).toBe('24 matches · 4% of matches')
    expect(shareText(1, 'matches', 1, 'matches')).toBe('1 match · 1% of matches')
    expect(shareText(1, 'runs', 10, 'drafts')).toBe('1 run · 10% of drafts')
  })
})
