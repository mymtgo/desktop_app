import { describe, expect, it } from 'vitest'
import type {
  CardRowsSummaryView,
  EvolutionSummaryView,
  LimitedCardRowView,
  LimitedCardView,
  LimitedMatchesKpisView,
  MatchSummary,
  PoolView,
  RegisteredVersionView,
} from '@/bindings'
import {
  cardFilterOptions,
  cardsEmptyText,
  cardsSummaryText,
  cardSubline,
  curveAndColours,
  deckChanges,
  deckSummaryText,
  imageCard,
  mainGroups,
  matchesHeaderText,
  avgMatchText,
  poolGroups,
  poolStatusLabel,
  poolStatusTint,
  priorDraftsText,
  shownVersion,
  sortLimitedMatches,
  totalMinutesText,
  versionLine,
  versionOptions,
  wholePool,
  winPctText,
  winPctTone,
} from './limitedPages'

function card(catalogId: number, overrides: Partial<LimitedCardView> = {}): LimitedCardView {
  return {
    catalogId,
    name: `Qard ${catalogId}`,
    resolved: true,
    type: 'Creature',
    subType: null,
    rarity: 'common',
    colors: 'G',
    manaCost: '{G}',
    cmc: 2,
    image: `img-${catalogId}`,
    localImage: null,
    artCrop: `art-${catalogId}`,
    localArtCrop: null,
    oracleId: `oracle-${catalogId}`,
    ...overrides,
  }
}

function version(index: number, overrides: Partial<RegisteredVersionView> = {}): RegisteredVersionView {
  return {
    index,
    signature: `sig-${index}`,
    capturedAtMs: Date.UTC(2026, 7, 22, 11, 0),
    matchTokens: [],
    matchLabels: [],
    main: 40,
    side: 5,
    colors: 'G,U',
    diffMain: { added: [], removed: [] },
    diffSide: { added: [], removed: [] },
    isCurrent: false,
    pool: { groups: [] },
    mainCards: [],
    sideCards: [],
    ...overrides,
  }
}

function summary(overrides: Partial<EvolutionSummaryView> = {}): EvolutionSummaryView {
  return {
    drafted: 42,
    added: 0,
    mainSpells: 23,
    basics: 17,
    sideboard: 19,
    versionCount: 2,
    firstRegisteredAtMs: null,
    lastRegisteredAtMs: null,
    ...overrides,
  }
}

function cardRow(catalogId: number, overrides: Partial<LimitedCardRowView> = {}): LimitedCardRowView {
  return {
    catalogId,
    oracleId: null,
    ordinals: [1],
    labels: ['P1p1'],
    status: 'main',
    added: false,
    gamesCast: 0,
    castWon: 0,
    castLost: 0,
    winPctCast: null,
    seenCount: 1,
    wheeled: false,
    priorTaken: 0,
    priorAvgOrdinal: null,
    priorWheeled: 0,
    priorDrafts: 0,
    ...overrides,
  }
}

function match(token: string, overrides: Partial<MatchSummary> = {}): MatchSummary {
  return {
    id: null,
    token,
    opponent: 'Qpponent',
    format: 'Limited',
    matchType: null,
    platform: 'mtgo',
    bestOf: null,
    state: 'complete',
    outcome: 'win',
    tentativeOutcome: null,
    wins: 2,
    losses: 0,
    needsAttention: false,
    attentionReason: null,
    startedAtMs: 1_000,
    endedAtMs: 61_000,
    revision: 1,
    notes: null,
    gameResults: [],
    manual: false,
    limited: false,
    opponentColors: null,
    opponentArchetype: null,
    ...overrides,
  }
}

function kpis(overrides: Partial<LimitedMatchesKpisView> = {}): LimitedMatchesKpisView {
  return {
    wins: 1,
    losses: 2,
    gameWins: 3,
    gameLosses: 4,
    avgMatchSeconds: null,
    onPlay: { wins: 0, losses: 0 },
    onDraw: { wins: 0, losses: 0 },
    queuedAtMs: null,
    finishedAtMs: null,
    totalMinutes: null,
    ...overrides,
  }
}

/** "HH:MM" in the test's local timezone, as `timeLabel` shows it. */
function hm(ms: number): string {
  const date = new Date(ms)
  return `${String(date.getHours()).padStart(2, '0')}:${String(date.getMinutes()).padStart(2, '0')}`
}

describe('pool status', () => {
  it("uses 0.x's labels and tints, unknown statuses as they are", () => {
    expect(['main', 'side', 'pool', 'cut'].map(poolStatusLabel)).toEqual(['Main', 'Side', 'Pool', 'Cut'])
    expect(poolStatusLabel('other')).toBe('other')
    expect(poolStatusTint('main')).toBe('border-sky-400/30 text-sky-300')
    expect(poolStatusTint('side')).toBe('border-border text-muted-foreground')
    expect(poolStatusTint('cut')).toBe('border-rose-500/30 text-rose-300')
    expect(poolStatusTint('pool')).toBe('border-border text-muted-foreground')
  })
})

describe('deck page', () => {
  const cards = { 1: card(1), 2: card(2, { type: 'Instant', cmc: 1, colors: 'U' }), 3: card(3, { type: 'Basic Land', cmc: 0, colors: '' }) }

  it('turns a card into a decklist card, `#id` and no art without card data', () => {
    expect(imageCard(cards, 2, 3)).toEqual({
      catalogId: 2,
      name: 'Qard 2',
      typeLine: 'Instant',
      colors: ['U'],
      image: 'img-2',
      localImage: null,
      artCrop: 'art-2',
      localArtCrop: null,
      cmc: 1,
      manaCost: '{G}',
      quantity: 3,
      sideboard: false,
      known: true,
    })
    expect(imageCard(cards, 99, 1)).toEqual({
      catalogId: 99,
      name: '#99',
      typeLine: null,
      colors: [],
      image: null,
      localImage: null,
      artCrop: null,
      localArtCrop: null,
      cmc: null,
      manaCost: null,
      quantity: 1,
      sideboard: false,
      known: false,
    })
  })

  it('shows the selected version, else the current one', () => {
    const versions = [version(1), version(2, { isCurrent: true })]
    expect(shownVersion(versions, null)?.index).toBe(2)
    expect(shownVersion(versions, 1)?.index).toBe(1)
    expect(shownVersion(versions, 7)?.index).toBe(2)
    expect(shownVersion([], null)).toBeNull()
  })

  it('groups the main deck by type in decklist order, by curve then name', () => {
    const shown = version(1, {
      mainCards: [
        { catalogId: 3, quantity: 8 },
        { catalogId: 1, quantity: 2 },
        { catalogId: 2, quantity: 1 },
        { catalogId: 99, quantity: 1 },
      ],
    })
    const groups = mainGroups(shown, cards)
    expect(groups.map((group) => [group.label, group.cards.map((c) => c.catalogId)])).toEqual([
      ['Creature', [1]],
      ['Instant', [2]],
      ['Land', [3]],
      ['Other', [99]],
    ])
    expect(mainGroups(null, cards)).toEqual([])
  })

  it('lists every pool copy for the whole-pool curve', () => {
    const pool: PoolView = {
      groups: [
        {
          key: 'G',
          label: 'Green',
          count: 3,
          cards: [{ catalogId: 1, quantity: 3, added: 0, status: 'main', mainQty: 2, sideQty: 1 }],
        },
      ],
    }
    expect(wholePool(pool, cards).map((c) => [c.catalogId, c.quantity])).toEqual([[1, 3]])
  })

  it("puts what the build left out in colour groups, the added booster's copies first", () => {
    const pool: PoolView = {
      groups: [
        {
          key: 'G',
          label: 'Green',
          count: 4,
          cards: [
            { catalogId: 1, quantity: 4, added: 1, status: 'main', mainQty: 2, sideQty: 2 },
            { catalogId: 99, quantity: 1, added: 0, status: 'side', mainQty: 0, sideQty: 1 },
          ],
        },
        {
          key: 'U',
          label: 'Blue',
          count: 1,
          cards: [{ catalogId: 2, quantity: 1, added: 0, status: 'main', mainQty: 1, sideQty: 0 }],
        },
      ],
    }
    const groups = poolGroups(version(1, { pool }), { groups: [] }, cards)
    expect(groups.map((group) => [group.key, group.label, group.cards.map((c) => [c.catalogId, c.quantity])])).toEqual([
      ['added', 'Added booster', [[1, 1]]],
      ['G', 'Green', [[99, 1], [1, 1]]],
    ])
    // No version yet: the run's own pool.
    expect(poolGroups(null, pool, cards)[0].key).toBe('added')
  })

  it('counts the curve and colours of nonland cards, 7+ in one bucket', () => {
    const list = [
      imageCard({ 1: card(1, { cmc: 2, colors: 'G,U' }) }, 1, 2),
      imageCard({ 4: card(4, { cmc: 7, colors: 'R' }) }, 4, 1),
      imageCard({ 5: card(5, { cmc: 9, colors: 'R' }) }, 5, 1),
      imageCard(cards, 3, 8),
      imageCard(cards, 99, 1),
    ]
    expect(curveAndColours(list)).toEqual({
      curve: [
        { cmc: 0, count: 1 },
        { cmc: 2, count: 2 },
        { cmc: 7, count: 2 },
      ],
      colours: [
        { colour: 'W', count: 0 },
        { colour: 'U', count: 2 },
        { colour: 'B', count: 0 },
        { colour: 'R', count: 2 },
        { colour: 'G', count: 2 },
        { colour: 'C', count: 1 },
      ],
      total: 5,
    })
  })

  it("writes 0.x's summary line", () => {
    expect(deckSummaryText(summary({ versionCount: 1 }), false)).toBe(
      '42 drafted · 23 spells registered + 17 basics · 1 registered version',
    )
    const first = Date.UTC(2026, 7, 22, 11, 5)
    const last = Date.UTC(2026, 7, 22, 12, 40)
    expect(
      deckSummaryText(summary({ added: 3, firstRegisteredAtMs: first, lastRegisteredAtMs: last }), true),
    ).toBe(`42 in pool (3 from the added booster) · 23 spells registered + 17 basics · 2 registered versions · deck built ${hm(first)} → ${hm(last)}`)
  })

  it('names each version in the strip', () => {
    const at = Date.UTC(2026, 7, 22, 11, 0)
    expect(versionOptions([version(1), version(2)])).toEqual([
      { value: '1', label: 'v1' },
      { value: '2', label: 'v2' },
    ])
    expect(versionLine(version(2, { isCurrent: true, matchLabels: ['Match 2', 'Match 3'], capturedAtMs: at }))).toEqual({
      title: 'Registered build v2 · current',
      matches: 'Match 2, Match 3',
      time: hm(at),
      counts: '40 main · 5 side',
    })
    expect(versionLine(version(1)).matches).toBe('unplayed')
    expect(versionLine(version(1)).title).toBe('Registered build v1')
  })

  it('lists the changes against the version before', () => {
    const versions = [
      version(1),
      version(2, {
        diffMain: { added: [{ catalogId: 2, quantity: 1 }], removed: [{ catalogId: 99, quantity: 2 }] },
        diffSide: { added: [], removed: [] },
      }),
      version(3),
    ]
    expect(deckChanges(versions, cards, null)).toEqual({ state: 'none', previous: null, sections: [] })
    expect(deckChanges(versions, cards, 1)).toEqual({ state: 'first', previous: null, sections: [] })
    expect(deckChanges(versions, cards, 3)).toEqual({ state: 'identical', previous: 2, sections: [] })
    const changed = deckChanges(versions, cards, 2)
    expect(changed.state).toBe('changed')
    expect(changed.previous).toBe(1)
    expect(changed.sections).toEqual([
      {
        key: 'main',
        label: 'Main deck',
        rows: [
          {
            key: 'm+2',
            sign: '+',
            quantity: 1,
            name: 'Qard 2',
            image: 'img-2',
            localImage: null,
            artCrop: 'art-2',
            localArtCrop: null,
            colors: 'U',
          },
          {
            key: 'm−99',
            sign: '−',
            quantity: 2,
            name: '#99',
            image: null,
            localImage: null,
            artCrop: null,
            localArtCrop: null,
            colors: '',
          },
        ],
      },
    ])
  })
})

describe('cards page', () => {
  it('offers Pool only once a row has it', () => {
    expect(cardFilterOptions([cardRow(1), cardRow(2, { status: 'cut' })])).toEqual([
      { value: 'all', label: 'All' },
      { value: 'main', label: 'Main' },
      { value: 'side', label: 'Side' },
      { value: 'cut', label: 'Cut' },
    ])
    expect(cardFilterOptions([cardRow(1, { status: 'pool' })]).map((o) => o.value)).toEqual([
      'all',
      'main',
      'side',
      'pool',
      'cut',
    ])
  })

  it("writes 0.x's summary line", () => {
    const one: CardRowsSummaryView = { distinct: 1, games: 1, otherDrafts: 0 }
    expect(cardsSummaryText(one, false, 'HOB')).toBe('1 distinct card drafted · game stats from 1 game with this deck')
    const many: CardRowsSummaryView = { distinct: 30, games: 4, otherDrafts: 2 }
    expect(cardsSummaryText(many, false, 'HOB')).toBe(
      '30 distinct cards drafted · game stats from 4 games with this deck · prior columns from 2 other HOB drafts',
    )
    expect(cardsSummaryText({ distinct: 2, games: 0, otherDrafts: 1 }, true, 'HOB')).toBe(
      '2 distinct cards in pool · game stats from 0 games with this deck · prior columns from 1 other HOB draft',
    )
  })

  it('says why the table is empty', () => {
    expect(cardsEmptyText(false)).toEqual({
      title: 'No picks recorded.',
      body: 'Cards appear here once a draft has been tracked for this event.',
    })
    expect(cardsEmptyText(true)).toEqual({
      title: 'No pool recorded yet.',
      body: 'Your pool appears here once a match with this deck has started.',
    })
  })

  it('shows type and rarity under the name, nothing without card data', () => {
    expect(cardSubline(card(1))).toBe('Creature · common')
    expect(cardSubline(card(1, { rarity: null }))).toBe('Creature')
    expect(cardSubline(card(1, { type: null, rarity: null }))).toBe('')
  })

  it('shows the cast win rate', () => {
    expect(winPctText(null)).toBe('-')
    expect(winPctText(50)).toBe('50%')
    expect(winPctTone(null)).toBe('text-muted-foreground')
    expect(winPctTone(50)).toBe('text-emerald-400')
    expect(winPctTone(49)).toBe('text-rose-400')
  })

  it('sums up prior drafts', () => {
    expect(priorDraftsText(cardRow(1))).toBe('-')
    expect(priorDraftsText(cardRow(1, { priorDrafts: 3, priorTaken: 2, priorAvgOrdinal: 2.6 }))).toBe('taken 2× · avg P1p3')
    expect(priorDraftsText(cardRow(1, { priorDrafts: 3, priorTaken: 1 }))).toBe('taken 1×')
    expect(priorDraftsText(cardRow(1, { priorDrafts: 3, priorWheeled: 2 }))).toBe('wheeled in 2 of 3')
    expect(priorDraftsText(cardRow(1, { priorDrafts: 3 }))).toBe('passed')
  })
})

describe('matches page', () => {
  it('writes the header line', () => {
    expect(matchesHeaderText(1, kpis())).toBe('1 league match')
    const queued = Date.UTC(2026, 7, 22, 11, 0)
    const finished = Date.UTC(2026, 7, 22, 11, 32)
    expect(matchesHeaderText(3, kpis({ queuedAtMs: queued, finishedAtMs: finished, totalMinutes: 32 }))).toBe(
      `3 league matches · queued ${hm(queued)} · finished ${hm(finished)} · 32 min total`,
    )
  })

  it('shows minutes or a dash', () => {
    expect(avgMatchText(null)).toBe('-')
    expect(avgMatchText(1290)).toBe('22m')
    expect(totalMinutesText(null)).toBe('-')
    expect(totalMinutesText(32)).toBe('32m')
  })

  const rows = [
    match('a', { outcome: 'loss', startedAtMs: 3_000, endedAtMs: 603_000, gameResults: [{ won: false, onPlay: true }] }),
    match('b', { outcome: 'win', startedAtMs: 1_000, endedAtMs: 61_000, gameResults: [{ won: true, onPlay: true }] }),
    match('c', { outcome: 'win', startedAtMs: 2_000, endedAtMs: null, gameResults: [] }),
  ]
  const tokens = (list: MatchSummary[]) => list.map((row) => row.token)

  it("keeps the command's order unsorted", () => {
    expect(tokens(sortLimitedMatches(rows, null, 'desc'))).toEqual(['a', 'b', 'c'])
  })

  it('sorts on the client by result, game, duration and date', () => {
    expect(tokens(sortLimitedMatches(rows, 'outcome', 'desc'))).toEqual(['b', 'c', 'a'])
    expect(tokens(sortLimitedMatches(rows, 'outcome', 'asc'))).toEqual(['a', 'b', 'c'])
    expect(tokens(sortLimitedMatches(rows, 'game_1', 'desc'))).toEqual(['b', 'a', 'c'])
    expect(tokens(sortLimitedMatches(rows, 'duration', 'desc'))).toEqual(['a', 'b', 'c'])
    expect(tokens(sortLimitedMatches(rows, 'duration', 'asc'))).toEqual(['c', 'b', 'a'])
    expect(tokens(sortLimitedMatches(rows, 'started_at', 'asc'))).toEqual(['b', 'c', 'a'])
  })

  it('sorts duration by the shown label, as 0.x did, not by milliseconds', () => {
    // "5 minutes" both: the longer one by a few seconds does not move first.
    const close = [
      match('x', { startedAtMs: 0, endedAtMs: 300_000 }),
      match('y', { startedAtMs: 0, endedAtMs: 359_000 }),
      match('z', { startedAtMs: 0, endedAtMs: 3_600_000 }),
    ]
    expect(tokens(sortLimitedMatches(close, 'duration', 'desc'))).toEqual(['z', 'x', 'y'])
    expect(tokens(sortLimitedMatches(close, 'duration', 'asc'))).toEqual(['x', 'y', 'z'])
    expect(tokens(sortLimitedMatches([close[1], close[0]], 'duration', 'desc'))).toEqual(['y', 'x'])
  })
})
