import { afterEach, describe, expect, it, vi } from 'vitest'
import type {
  CrossDraftStatsView,
  DraftHeaderView,
  DraftPickView,
  LimitedCardView,
  LimitedIndexRowView,
  RunMatchView,
  SeenWheelView,
} from '@/bindings'
import {
  avgPickText,
  avgRecordText,
  cardFor,
  colourLetters,
  crossDraftRows,
  draftSubtitle,
  formatSeconds,
  LIMITED_TIMEFRAMES,
  limitedMatchPath,
  limitedPath,
  limitedQueryFromUrl,
  limitedUrlQuery,
  marginText,
  NO_VALUE,
  NOTE_MAX_LENGTH,
  NOTE_SAVE_DELAY_MS,
  noteStatusIcon,
  noteStatusText,
  ordinalLabel,
  percentText,
  pickBadgeTime,
  pickGroups,
  pickHeading,
  PickNoteSaver,
  pickTiles,
  recordTone,
  refreshedNote,
  rowInfo,
  rowLinks,
  selectPick,
  signalText,
  signalWidth,
  signedSeconds,
  stateBadgeTint,
  stepPick,
  timeLabel,
  versionsText,
  wheelSummary,
  withOpponentColours,
} from './limited'

function row(overrides: Partial<LimitedIndexRowView> = {}): LimitedIndexRowView {
  return {
    runKey: 'draft:Q-token',
    draftToken: 'Q-token',
    title: 'HOB Draft',
    setCode: 'HOB',
    kind: 'draft',
    state: 'Complete',
    stateVariant: 'success',
    startedAtMs: 1_000,
    wins: 2,
    losses: 1,
    results: ['W', 'L', 'W'],
    picksMade: 42,
    picksExpected: 42,
    packs: null,
    deckRegistered: true,
    versionCount: 2,
    avgPickSeconds: 26,
    opponents: ['Qorvath', 'Qamurai'],
    matches: [],
    gameWins: 4,
    gameLosses: 3,
    onPlayRecord: { wins: 1, losses: 1 },
    onDrawRecord: { wins: 1, losses: 0 },
    note: null,
    linked: true,
    inLeague: true,
    classification: 'FINISH',
    liveRound: null,
    avgMatchSeconds: null,
    timeOfDay: null,
    userState: null,
    autoState: 'complete',
    notes: null,
    untrackedMatches: 0,
    ...overrides,
  }
}

function card(catalogId: number, name: string): LimitedCardView {
  return {
    catalogId,
    name,
    resolved: true,
    type: 'Creature',
    subType: null,
    rarity: 'common',
    colors: 'G',
    manaCost: '{G}',
    cmc: 1,
    image: null,
    localImage: null,
    artCrop: null,
    localArtCrop: null,
    oracleId: `oracle-${catalogId}`,
  }
}

function pick(overrides: Partial<DraftPickView> = {}): DraftPickView {
  return {
    ordinal: 1,
    packNumber: 1,
    pickNumber: 1,
    label: 'P1p1',
    packId: 7,
    direction: 1,
    available: [10, 11, 12],
    pickedCatalogId: 11,
    reservations: [],
    elapsedSeconds: 12,
    marginSeconds: 43,
    indecisive: false,
    shownAtMs: 0,
    deadlineAtMs: 55_000,
    pickedAtMs: 12_000,
    note: null,
    noteSavedAtMs: null,
    wheelReturnOrdinal: null,
    wheeledIds: [],
    takenIds: [],
    ...overrides,
  }
}

function fact(overrides: Partial<SeenWheelView> = {}): SeenWheelView {
  return {
    seenCount: 1,
    firstSeenOrdinal: 1,
    wheeled: false,
    wheeledToMe: false,
    pickedCount: 0,
    passedCount: 1,
    ...overrides,
  }
}

function header(overrides: Partial<DraftHeaderView> = {}): DraftHeaderView {
  return {
    seatIndex: 2,
    seatCount: 8,
    boosterCatalogId: 154228,
    packSize: 14,
    picksMade: 42,
    picksExpected: 42,
    startedAtMs: null,
    endedAtMs: null,
    durationMinutes: 38,
    avgPickSeconds: 26,
    avgMarginSeconds: 43,
    indecisiveCount: 3,
    fastestPack: 3,
    slowestPack: 1,
    colorsPicked: [],
    state: 'finished',
    ...overrides,
  }
}

describe('seconds', () => {
  it('reads seconds under a minute as Ns and longer as m:ss (0.x formatSeconds)', () => {
    expect(formatSeconds(null)).toBe(NO_VALUE)
    expect(formatSeconds(0)).toBe('0s')
    expect(formatSeconds(26)).toBe('26s')
    expect(formatSeconds(59)).toBe('59s')
    expect(formatSeconds(60)).toBe('1:00')
    expect(formatSeconds(125)).toBe('2:05')
  })

  it('signs a negative margin by hand', () => {
    expect(signedSeconds(null)).toBe(NO_VALUE)
    expect(signedSeconds(43)).toBe('43s')
    expect(signedSeconds(-90)).toBe('-1:30')
    expect(signedSeconds(-5)).toBe('-5s')
  })

  it('uses a plain dash for no value', () => {
    expect(NO_VALUE).toBe('-')
  })
})

describe('index filters', () => {
  it('reads set, kind and timeframe from the URL, unknowns falling back', () => {
    expect(limitedQueryFromUrl({})).toEqual({ set: null, kind: null, timeframe: 'alltime' })
    expect(limitedQueryFromUrl({ set: 'HOB', kind: 'sealed', timeframe: 'week' })).toEqual({
      set: 'HOB',
      kind: 'sealed',
      timeframe: 'week',
    })
    expect(limitedQueryFromUrl({ set: ' ', kind: 'cube', timeframe: 'decade' })).toEqual({
      set: null,
      kind: null,
      timeframe: 'alltime',
    })
  })

  it('leaves defaults out of the URL', () => {
    expect(limitedUrlQuery({ set: null, kind: null, timeframe: 'alltime' })).toEqual({})
    expect(limitedUrlQuery({ set: 'HOB', kind: 'draft', timeframe: 'monthly' })).toEqual({
      set: 'HOB',
      kind: 'draft',
      timeframe: 'monthly',
    })
  })

  it('offers 0.x timeframes', () => {
    expect(LIMITED_TIMEFRAMES).toEqual([
      { value: 'alltime', label: 'All time' },
      { value: 'monthly', label: '30d' },
      { value: 'week', label: '7d' },
    ])
  })
})

describe('paths', () => {
  it('URL-encodes run keys', () => {
    expect(limitedPath('0x:12', 'draft')).toBe('/limited/0x%3A12/draft')
    expect(limitedPath('Q/tok en', 'deck')).toBe('/limited/Q%2Ftok%20en/deck')
    expect(limitedMatchPath('0x:12', 'a/b c')).toBe('/limited/0x%3A12/matches/a%2Fb%20c')
  })
})

describe('KPI text', () => {
  it('shows a percent or a dash', () => {
    expect(percentText(null)).toBe(NO_VALUE)
    expect(percentText(57)).toBe('57%')
  })

  it('shows the average record or a dash', () => {
    expect(avgRecordText(null, null)).toBe(NO_VALUE)
    expect(avgRecordText(2.3, 1.7)).toBe('2.3-1.7')
  })
})

describe('index row', () => {
  it('links the title to the draft review, sealed to the deck, unlinked nowhere', () => {
    const key = encodeURIComponent('draft:Q-token')
    expect(rowLinks(row())).toEqual({
      draft: `/limited/${key}/draft`,
      deck: `/limited/${key}/deck`,
      title: `/limited/${key}/draft`,
    })
    expect(rowLinks(row({ kind: 'sealed', packs: 6 }))).toEqual({
      draft: null,
      deck: `/limited/${key}/deck`,
      title: `/limited/${key}/deck`,
    })
    expect(rowLinks(row({ runKey: null, linked: false }))).toEqual({ draft: null, deck: null, title: null })
  })

  it('builds the info line', () => {
    expect(rowInfo(row())).toEqual({
      count: '42/42 picks',
      versions: '2 deck versions',
      avgPick: 'avg 26s / pick',
      opponents: 'vs Qorvath · Qamurai',
      note: null,
    })
    expect(rowInfo(row({ kind: 'sealed', packs: 6, versionCount: 1, avgPickSeconds: null }))).toMatchObject({
      count: '6 packs',
      versions: '1 deck version',
      avgPick: null,
    })
  })

  it('shows the note only without opponents, and versions only once a deck is registered', () => {
    const info = rowInfo(
      row({ opponents: [], note: 'Draft finished, league ended without play', deckRegistered: false, picksMade: 3 }),
    )
    expect(info).toEqual({
      count: '3/42 picks',
      versions: null,
      avgPick: 'avg 26s / pick',
      opponents: null,
      note: 'Draft finished, league ended without play',
    })
    expect(rowInfo(row({ note: 'ignored' })).note).toBeNull()
  })

  it('pluralises versions', () => {
    expect(versionsText(0)).toBe('0 deck versions')
    expect(versionsText(1)).toBe('1 deck version')
    expect(versionsText(3)).toBe('3 deck versions')
  })

  it('reads the average pick time', () => {
    expect(avgPickText(null)).toBeNull()
    expect(avgPickText(75)).toBe('avg 1:15 / pick')
  })

  it('tints state badges by variant', () => {
    expect(stateBadgeTint('success')).toBe('border-emerald-500/30 text-emerald-300')
    expect(stateBadgeTint('destructive')).toBe('border-rose-500/30 text-rose-300')
    expect(stateBadgeTint('warning')).toBe('border-amber-500/30 text-amber-300')
    expect(stateBadgeTint('default')).toBe('border-sky-400/30 text-sky-300')
    expect(stateBadgeTint('other')).toBe('')
  })
})

describe('event sidebar', () => {
  it('colours the record: muted unplayed, emerald when level or ahead, rose behind', () => {
    expect(recordTone(0, 0)).toBe('text-muted-foreground')
    expect(recordTone(1, 1)).toBe('text-emerald-400')
    expect(recordTone(3, 0)).toBe('text-emerald-400')
    expect(recordTone(0, 1)).toBe('text-rose-400')
  })
})

describe('cards and colours', () => {
  it('falls back to #id without card data', () => {
    expect(cardFor({ 5: card(5, 'Qelf') }, 5).name).toBe('Qelf')
    expect(cardFor({}, 9)).toEqual({
      catalogId: 9,
      name: '#9',
      resolved: false,
      type: null,
      subType: null,
      rarity: null,
      colors: '',
      manaCost: null,
      cmc: null,
      image: null,
      localImage: null,
      artCrop: null,
      localArtCrop: null,
      oracleId: null,
    })
  })

  it('adds the colours an expanded row read to its matches', () => {
    const match = (token: string): RunMatchView => ({
      token,
      result: 'W',
      opponentName: 'Qopponent',
      games: [],
      startedAtMs: 1_000,
      durationSeconds: null,
      notes: null,
      manual: false,
      opponentColors: null,
      opponentArchetype: null,
      opponentArchetypeId: null,
    })
    const matches = [match('q-m1'), match('q-m2')]
    // Not read yet: the rows as they are.
    expect(withOpponentColours(matches, undefined)).toBe(matches)
    expect(withOpponentColours(matches, null)).toBe(matches)
    expect(
      withOpponentColours(matches, [
        { token: 'q-m2', opponentColors: 'B,G' },
        { token: 'q-other', opponentColors: 'W' },
      ]).map((row) => [row.token, row.opponentColors]),
    ).toEqual([
      ['q-m1', null],
      ['q-m2', 'B,G'],
    ])
  })

  it('reads colour letters in WUBRG order, colourless when none', () => {
    expect(colourLetters('G,U')).toEqual(['U', 'G'])
    expect(colourLetters('')).toEqual([])
    expect(colourLetters(null)).toEqual([])
    expect(colourLetters('B,G')).toEqual(['B', 'G'])
  })
})

describe('draft header', () => {
  it('reads seat, booster, set, pack size and duration', () => {
    expect(draftSubtitle(header(), 'HOB')).toBe('Seat 3 of 8 · booster 154228 · HOB × 3 · 14 cards per pack · 38 min')
    expect(draftSubtitle(header({ seatIndex: null, boosterCatalogId: null, durationMinutes: null }), null)).toBe(
      '? × 3 · 14 cards per pack',
    )
  })

  it('reads signals', () => {
    expect(signalText({ color: 'G', wheeled: 2, seenTwice: 3, score: 5, share: 1 })).toBe('2 wheeled · 3 seen 2+')
    expect(signalWidth(1)).toBe('100%')
    expect(signalWidth(0.5)).toBe('50%')
    expect(signalWidth(0)).toBe('4%')
    expect(signalWidth(null)).toBe('4%')
  })
})

describe('pick labels', () => {
  it('labels ordinals by pack size', () => {
    expect(ordinalLabel(1, 14)).toBe('P1p1')
    expect(ordinalLabel(15, 14)).toBe('P2p1')
    expect(ordinalLabel(42, 14)).toBe('P3p14')
    expect(ordinalLabel(16, 15)).toBe('P2p1')
  })

  it('heads the pick select', () => {
    expect(pickHeading(pick({ packNumber: 2, pickNumber: 5 }))).toBe('Pack 2 · Pick 5')
  })

  it('groups picks by pack in order', () => {
    const picks = [
      pick({ ordinal: 15, packNumber: 2, pickNumber: 1 }),
      pick({ ordinal: 1, packNumber: 1, pickNumber: 1 }),
      pick({ ordinal: 2, packNumber: 1, pickNumber: 2 }),
    ]
    expect(pickGroups(picks).map(([pack, list]) => [pack, list.map((p) => p.ordinal)])).toEqual([
      [1, [1, 2]],
      [2, [15]],
    ])
  })

  it('selects the asked pick when the draft has it, else the first', () => {
    const picks = [pick({ ordinal: 1 }), pick({ ordinal: 2 })]
    expect(selectPick(picks, 2)).toBe(2)
    expect(selectPick(picks, 9)).toBe(1)
    expect(selectPick(picks, null)).toBe(1)
    expect(selectPick([], 1)).toBeNull()
  })

  it('steps to the previous and next pick', () => {
    const picks = [pick({ ordinal: 1 }), pick({ ordinal: 2 }), pick({ ordinal: 3 })]
    expect(stepPick(picks, 2, -1)).toBe(1)
    expect(stepPick(picks, 2, 1)).toBe(3)
    expect(stepPick(picks, 1, -1)).toBeNull()
    expect(stepPick(picks, 3, 1)).toBeNull()
  })

  it('reads the time badge: seconds and spare or over', () => {
    expect(marginText(null)).toBeNull()
    expect(marginText(43)).toBe('43s spare')
    expect(marginText(0)).toBe('0s spare')
    expect(marginText(-75)).toBe('1:15 over')
    expect(pickBadgeTime(pick())).toBe('12s · 43s spare')
    expect(pickBadgeTime(pick({ marginSeconds: null }))).toBe('12s')
    expect(pickBadgeTime(pick({ elapsedSeconds: null }))).toBeNull()
  })
})

describe('pick grid', () => {
  const cards = { 10: card(10, 'Qa'), 11: card(11, 'Qb'), 12: card(12, 'Qc') }

  it('leads with the pick, keeps pack order, and pads to the pack size', () => {
    const { tiles, fillers } = pickTiles(pick(), cards, {}, 14)
    expect(tiles.map((tile) => tile.card.catalogId)).toEqual([11, 10, 12])
    expect(tiles[0].picked).toBe(true)
    expect(fillers).toBe(11)
  })

  it('marks wheeled and gone cards and shows seen and passed counts off the pick', () => {
    const seen = { 10: fact({ seenCount: 2, passedCount: 2 }), 12: fact({ seenCount: 3, passedCount: 3 }) }
    const { tiles } = pickTiles(
      pick({ wheelReturnOrdinal: 9, wheeledIds: [12], takenIds: [10] }),
      cards,
      seen,
      3,
    )
    const byId = Object.fromEntries(tiles.map((tile) => [tile.card.catalogId, tile]))
    expect(byId[10]).toMatchObject({ picked: false, wheeled: false, gone: true, seen: 'seen 2', passed: 'passed ×2' })
    expect(byId[12]).toMatchObject({ wheeled: true, gone: false, seen: null, passed: null })
    expect(byId[11]).toMatchObject({ picked: true, seen: null, passed: null })
  })

  it('never dims cards when the pack does not come back', () => {
    const { tiles } = pickTiles(pick({ takenIds: [10] }), cards, {}, 3)
    expect(tiles.every((tile) => !tile.gone)).toBe(true)
  })

  it('keys duplicate cards apart', () => {
    const { tiles } = pickTiles(pick({ available: [10, 10], pickedCatalogId: null }), cards, {}, 2)
    expect(tiles.map((tile) => tile.key)).toEqual(['10-0', '10-1'])
  })

  it('summarises the wheel', () => {
    const seen = { 12: fact({ wheeledToMe: true }) }
    expect(wheelSummary(pick(), cards, seen)).toBeNull()
    expect(wheelSummary(pick({ wheelReturnOrdinal: 9, wheeledIds: [12, 99], takenIds: [10] }), cards, seen)).toBe(
      'Wheeled at pick 9: Qc (you took it), #99. Gone: Qa.',
    )
    expect(wheelSummary(pick({ wheelReturnOrdinal: 9 }), cards, seen)).toBe('Wheeled at pick 9: nothing. Gone: nothing.')
  })
})

describe('cross-draft card', () => {
  const stat: CrossDraftStatsView = {
    oracleId: 'o',
    drafts: 4,
    timesTaken: 2,
    avgOrdinal: 16.4,
    timesPassed: 3,
    timesWheeled: 1,
    madeDeck: 2,
  }

  it('lists taken, passed, wheeled and made deck', () => {
    expect(crossDraftRows(stat, 14)).toEqual([
      { label: 'Taken', value: '2×', extra: '· avg P2p2' },
      { label: 'Seen & passed', value: '3×', extra: null },
      { label: 'Wheeled', value: '1 of 4 drafts', extra: null },
      { label: 'Made your deck', value: '2 of 4', extra: null },
    ])
  })

  it('shows a dash when it never made the deck and no average without takes', () => {
    const rows = crossDraftRows({ ...stat, avgOrdinal: null, madeDeck: 0 }, 14)
    expect(rows[0].extra).toBeNull()
    expect(rows[3].value).toBe(NO_VALUE)
  })
})

describe('pick note status', () => {
  it('reads 0.x status texts', () => {
    expect(noteStatusText('error', 1)).toBe('not saved')
    expect(noteStatusText('dirty', 1)).toBe('unsaved')
    expect(noteStatusText('idle', null)).toBeNull()
    expect(noteStatusText('saving', null)).toBeNull()
    const at = new Date(2026, 9, 8, 9, 5).getTime()
    expect(noteStatusText('saved', at)).toBe('saved 09:05')
    expect(noteStatusText('idle', at)).toBe('saved 09:05')
  })

  it('shows a spinner while saving and a check once saved', () => {
    expect(noteStatusIcon('saving', null)).toBe('spinner')
    expect(noteStatusIcon('saved', 1)).toBe('check')
    expect(noteStatusIcon('idle', 1)).toBe('check')
    expect(noteStatusIcon('idle', null)).toBeNull()
    expect(noteStatusIcon('dirty', 1)).toBeNull()
    expect(noteStatusIcon('error', 1)).toBeNull()
  })

  it('formats HH:MM in local time', () => {
    expect(timeLabel(null)).toBe(NO_VALUE)
    expect(timeLabel(new Date(2026, 0, 1, 18, 7).getTime())).toBe('18:07')
  })

  it('keeps 0.x limits', () => {
    expect(NOTE_SAVE_DELAY_MS).toBe(600)
    expect(NOTE_MAX_LENGTH).toBe(2000)
  })
})

describe('pick note saver', () => {
  afterEach(() => {
    vi.useRealTimers()
  })

  type Call = { ordinal: number; note: string }

  function saver(options: { fail?: boolean; ordinal?: number; savedAtMs?: number | null } = {}) {
    const calls: Call[] = []
    const resolvers: (() => void)[] = []
    const states: { status: string; savedAtMs: number | null }[] = []
    const instance = new PickNoteSaver({
      ordinal: options.ordinal ?? 1,
      savedAtMs: options.savedAtMs ?? null,
      save: (ordinal, note) => {
        calls.push({ ordinal, note })
        return new Promise<void>((resolve, reject) => {
          resolvers.push(() => (options.fail ? reject(new Error('This pick no longer exists.')) : resolve()))
        })
      },
      now: () => 42_000,
      onChange: (state) => states.push({ ...state }),
    })
    return { instance, calls, resolvers, states }
  }

  it('starts idle with the stored save time', () => {
    const { instance } = saver({ savedAtMs: 7 })
    expect(instance.state).toEqual({ status: 'idle', savedAtMs: 7 })
  })

  it('saves 600 ms after the last keystroke', async () => {
    vi.useFakeTimers()
    const { instance, calls, resolvers } = saver()
    instance.input('Q')
    expect(instance.state.status).toBe('dirty')
    vi.advanceTimersByTime(400)
    instance.input('Qu')
    vi.advanceTimersByTime(599)
    expect(calls).toEqual([])
    vi.advanceTimersByTime(1)
    expect(calls).toEqual([{ ordinal: 1, note: 'Qu' }])
    expect(instance.state.status).toBe('saving')
    resolvers[0]()
    await vi.runAllTimersAsync()
    expect(instance.state).toEqual({ status: 'saved', savedAtMs: 42_000 })
  })

  it('flushes on blur, and only when there is unsaved text', async () => {
    vi.useFakeTimers()
    const { instance, calls } = saver()
    instance.flush()
    expect(calls).toEqual([])
    instance.input('Qx')
    instance.flush()
    expect(calls).toEqual([{ ordinal: 1, note: 'Qx' }])
    vi.advanceTimersByTime(5_000)
    expect(calls).toHaveLength(1)
  })

  it('flushes to the old pick on a pick change, then resets to the new pick', async () => {
    vi.useFakeTimers()
    const { instance, calls, resolvers } = saver()
    instance.input('Qold')
    instance.switchTo(2, 99)
    expect(calls).toEqual([{ ordinal: 1, note: 'Qold' }])
    expect(instance.state).toEqual({ status: 'idle', savedAtMs: 99 })
    // The old pick's answer must not mark the new pick saved.
    resolvers[0]()
    await vi.runAllTimersAsync()
    expect(instance.state).toEqual({ status: 'idle', savedAtMs: 99 })
    instance.input('Qnew')
    vi.advanceTimersByTime(NOTE_SAVE_DELAY_MS)
    expect(calls[1]).toEqual({ ordinal: 2, note: 'Qnew' })
  })

  it('shows not saved when the save is refused', async () => {
    vi.useFakeTimers()
    const { instance, resolvers } = saver({ fail: true })
    instance.input('Q')
    instance.flush()
    resolvers[0]()
    await vi.runAllTimersAsync()
    expect(instance.state.status).toBe('error')
  })

  it('starts over from a refetched note once idle', () => {
    const { instance, states } = saver({ savedAtMs: 7 })
    instance.adopt(90)
    expect(instance.state).toEqual({ status: 'idle', savedAtMs: 90 })
    expect(states.at(-1)).toEqual({ status: 'idle', savedAtMs: 90 })
  })

  it('reports each state change', () => {
    vi.useFakeTimers()
    const { instance, states } = saver()
    instance.input('Q')
    instance.flush()
    expect(states.map((state) => state.status)).toEqual(['dirty', 'saving'])
  })
})

describe('refetched pick note', () => {
  const idle = { status: 'idle' as const, savedAtMs: null }

  it('replaces a stale note in an idle editor with a newer saved one', () => {
    expect(refreshedNote('', idle, { note: 'Qsaved', savedAtMs: 50 })).toBe('Qsaved')
    expect(refreshedNote('Qold', { status: 'idle', savedAtMs: 10 }, { note: 'Qsaved', savedAtMs: 50 })).toBe(
      'Qsaved',
    )
    // A note deleted elsewhere clears the box.
    expect(refreshedNote('Qold', { status: 'saved', savedAtMs: 10 }, { note: null, savedAtMs: null })).toBe('')
  })

  it('never overwrites typing that is not saved yet', () => {
    const incoming = { note: 'Qsaved', savedAtMs: 50 }
    expect(refreshedNote('Qtyping', { status: 'dirty', savedAtMs: 10 }, incoming)).toBeNull()
    expect(refreshedNote('Qtyping', { status: 'saving', savedAtMs: 10 }, incoming)).toBeNull()
    expect(refreshedNote('Qtyping', { status: 'error', savedAtMs: 10 }, incoming)).toBeNull()
  })

  it('ignores a refetch older than what this editor saved, or the same text', () => {
    expect(refreshedNote('Qnew', { status: 'saved', savedAtMs: 90 }, { note: 'Qold', savedAtMs: 50 })).toBeNull()
    expect(refreshedNote('Qsame', idle, { note: 'Qsame', savedAtMs: 50 })).toBeNull()
    expect(refreshedNote('', idle, { note: null, savedAtMs: null })).toBeNull()
  })
})
