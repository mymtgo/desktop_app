import { afterEach, describe, expect, it, vi } from 'vitest'
import type { ImportSummary, LiveMatchView, MatchPage, MatchSummary, PlatformContextView, Settings } from '@/bindings'
import { queryClient } from '@/queryClient'
import { keys } from './keys'
import {
  ARCHETYPE_REFRESH_DELAY_MS,
  DASHBOARD_REFRESH_DELAY_MS,
  LIMITED_REFRESH_DELAY_MS,
  liveClient,
  livePlatforms,
  onAccountChanged,
  onAppNotification,
  onArchetypesChanged,
  onCardsChanged,
  onDecksChanged,
  onGuidesChanged,
  onHistoryImportsChanged,
  onImportProgress,
  onLeaguesChanged,
  onLimitedChanged,
  onMatchRemoved,
  onMatchUpdated,
  onMetagameSharesReady,
  onRankChanged,
  onSettingsChanged,
  loadLivePlatforms,
} from './liveEvents'
import { guideKeys } from './guides'
import { matchDetailKeys } from './matchDetail'
import { archetypeKeys } from './archetypes'
import { metagameKeys } from './metagame'
import { useToast } from '@/composables/useToast'

function row(token: string): MatchSummary {
  return {
    id: null,
    token,
    opponent: 'Qopponent',
    format: 'Modern',
    matchType: 'Constructed',
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
    endedAtMs: null,
    revision: 1,
    notes: null,
    gameResults: [],
    manual: false,
    limited: false,
    opponentArchetype: null,
  }
}

function page(rows: MatchSummary[], number: number): MatchPage {
  return { rows, total: 4, page: number, perPage: 2, unknownArchetypeCount: 0, archetypes: [] }
}

afterEach(() => {
  vi.unstubAllGlobals()
  vi.restoreAllMocks()
  queryClient.clear()
})

describe('liveEvents wiring', () => {
  it('keeps the pushed settings and rechecks the folder statuses', () => {
    const invalidate = vi.spyOn(queryClient, 'invalidateQueries')
    const pushed = { closeToTray: false } as Settings
    onSettingsChanged(pushed)
    expect(queryClient.getQueryData(keys.settings)).toBe(pushed)
    expect(invalidate).toHaveBeenCalledWith({ queryKey: keys.folderStatuses })
  })

  it('refreshes later pages after a removal without cancelling a fetch already running', () => {
    queryClient.setQueryData(keys.matches(1), page([row('a'), row('b')], 1))
    queryClient.setQueryData(keys.matches(2), page([row('c'), row('d')], 2))
    const invalidate = vi.spyOn(queryClient, 'invalidateQueries')
    // Frames run by hand below.
    vi.stubGlobal('requestAnimationFrame', () => 0)

    liveClient.push({ kind: 'removed', token: 'a' })
    liveClient.flush()

    expect(invalidate).toHaveBeenCalledTimes(1)
    // A rebuild removes matches on many frames: cancelling would starve the fetch.
    expect(invalidate.mock.calls[0][1]).toEqual({ cancelRefetch: false })
  })
})

describe('import progress', () => {
  /** Watch invalidations; the returned function gives the keys so far, in call order. */
  function watchInvalidations(): () => unknown[] {
    const invalidate = vi.spyOn(queryClient, 'invalidateQueries')
    return () => invalidate.mock.calls.map((call) => (call[0] as { queryKey?: unknown } | undefined)?.queryKey)
  }

  // The import may also store 0.x's deck settings and dashboard layout, and
  // each match's 0.x detail (which bumps no match revision).
  const refreshed = [
    keys.allMatches,
    keys.status,
    keys.legacyImport,
    keys.firstRun,
    keys.settings,
    keys.allDashboard,
    matchDetailKeys.all,
    archetypeKeys.all,
    metagameKeys.all,
  ]

  it('stores the progress and refreshes nothing while the import runs', () => {
    const invalidated = watchInvalidations()
    onImportProgress({ phase: 'writing', done: 2, total: 5 })
    expect(queryClient.getQueryData(keys.legacyImportProgress)).toEqual({ phase: 'writing', done: 2, total: 5 })
    expect(invalidated()).toEqual([])
  })

  it('refreshes matches, status, the import card and settings when the import finishes', () => {
    const invalidated = watchInvalidations()
    onImportProgress({ phase: 'finished', summary: {} as ImportSummary })
    expect(invalidated()).toEqual(refreshed)
  })

  it('refreshes them after a failure too: earlier batches may have committed', () => {
    const invalidated = watchInvalidations()
    onImportProgress({ phase: 'failed', message: 'the 0.x data could not be read' })
    expect(queryClient.getQueryData(keys.legacyImportProgress)).toEqual({
      phase: 'failed',
      message: 'the 0.x data could not be read',
    })
    expect(invalidated()).toEqual(refreshed)
  })
})

describe('history imports changed', () => {
  it('refreshes the match pages and the dashboard', () => {
    const invalidate = vi.spyOn(queryClient, 'invalidateQueries')
    onHistoryImportsChanged(4)
    const invalidated = invalidate.mock.calls.map((call) => (call[0] as { queryKey?: unknown } | undefined)?.queryKey)
    expect(invalidated).toEqual([keys.allMatches, keys.allDashboard])
  })

  it("keeps the pushed count as the import page's imported count", () => {
    onHistoryImportsChanged(4)
    expect(queryClient.getQueryData(keys.historyImportedCount)).toBe(4)
  })
})

describe('cards changed', () => {
  afterEach(() => {
    vi.useRealTimers()
    vi.restoreAllMocks()
  })

  it('refetches the card counts, the Cards grid, deck, match detail, archetype and metagame pages, then Limited once quiet', () => {
    vi.useFakeTimers()
    const invalidate = vi.spyOn(queryClient, 'invalidateQueries')
    onCardsChanged()
    expect(invalidate.mock.calls.map((call) => call[0])).toEqual([
      { queryKey: keys.cardData },
      { queryKey: keys.allCardsPage },
      { queryKey: keys.allDecks },
      { queryKey: matchDetailKeys.all },
      // Issue #326: the decklists' mana costs come from the card data.
      { queryKey: archetypeKeys.all },
      { queryKey: metagameKeys.all },
    ])
    vi.advanceTimersByTime(LIMITED_REFRESH_DELAY_MS)
    expect(invalidate.mock.calls.map((call) => call[0])).toContainEqual({ queryKey: keys.allLimited })
    for (const call of invalidate.mock.calls) expect(call[1]).toEqual({ cancelRefetch: false })
  })
})

describe('archetypes changed', () => {
  afterEach(() => {
    vi.useRealTimers()
  })

  // Final review I1: background detection, the deck "Detect archetypes"
  // banner, a definitions check and the 0.x import change archetype names
  // shown on many pages. A backfill sends one event per slice: one refetch
  // once things go quiet, never cancelling a fetch already running.
  it('refetches every page showing archetypes once things go quiet', () => {
    vi.useFakeTimers()
    const invalidate = vi.spyOn(queryClient, 'invalidateQueries')
    onArchetypesChanged()
    vi.advanceTimersByTime(300)
    onArchetypesChanged()
    vi.advanceTimersByTime(ARCHETYPE_REFRESH_DELAY_MS - 1)
    expect(invalidate).not.toHaveBeenCalled()
    vi.advanceTimersByTime(1)
    expect(invalidate.mock.calls.map((call) => call[0])).toEqual([
      // Match lists and match pages (opponent archetype column, header).
      { queryKey: keys.allMatches },
      { queryKey: matchDetailKeys.all },
      // Deck pages: matchups, card stats options, guides, the sidebar.
      { queryKey: keys.allDecks },
      { queryKey: keys.allLeagues },
      // Dashboard archetype cards and the Opponents page.
      { queryKey: keys.allDashboard },
      // Cards page popularity and archetype sheet.
      { queryKey: keys.allCardsPage },
      { queryKey: archetypeKeys.all },
      { queryKey: metagameKeys.all },
    ])
    for (const call of invalidate.mock.calls) expect(call[1]).toEqual({ cancelRefetch: false })
    vi.advanceTimersByTime(5_000)
    expect(invalidate).toHaveBeenCalledTimes(8)
  })
})

describe('rank changed', () => {
  afterEach(() => {
    vi.useRealTimers()
  })

  it('refetches the deck rank cards at once and the dashboard once things go quiet', () => {
    vi.useFakeTimers()
    const invalidate = vi.spyOn(queryClient, 'invalidateQueries')
    onRankChanged()
    expect(invalidate.mock.calls).toEqual([[{ queryKey: keys.allDeckRank }, { cancelRefetch: false }]])
    expect(keys.deckRank(7, null).slice(0, 2)).toEqual([...keys.allDeckRank])
    onRankChanged()
    vi.advanceTimersByTime(DASHBOARD_REFRESH_DELAY_MS)
    expect(invalidate.mock.calls.filter((call) => (call[0] as { queryKey?: unknown } | undefined)?.queryKey === keys.allDashboard)).toEqual([
      [{ queryKey: keys.allDashboard }, { cancelRefetch: false }],
    ])
  })
})

describe('decks changed', () => {
  afterEach(() => {
    vi.useRealTimers()
  })

  it('refetches every deck and league query, then the dashboard once things go quiet', () => {
    vi.useFakeTimers()
    const invalidate = vi.spyOn(queryClient, 'invalidateQueries')
    onDecksChanged()
    // League cards show deck names, and a new link can move a match between runs.
    expect(invalidate.mock.calls.map((call) => call[0])).toEqual([
      { queryKey: keys.allDecks },
      { queryKey: keys.allLeagues },
    ])
    onDecksChanged()
    vi.advanceTimersByTime(DASHBOARD_REFRESH_DELAY_MS)
    expect(invalidate.mock.calls.map((call) => call[0])).toEqual([
      { queryKey: keys.allDecks },
      { queryKey: keys.allLeagues },
      { queryKey: keys.allDecks },
      { queryKey: keys.allLeagues },
      { queryKey: keys.allDashboard },
    ])
    // A fetch already running is never cancelled, so events cannot starve it.
    for (const call of invalidate.mock.calls) expect(call[1]).toEqual({ cancelRefetch: false })
  })
})

describe('leagues changed', () => {
  afterEach(() => {
    vi.useRealTimers()
  })

  it('refetches league and deck queries once, 500 ms after a burst', () => {
    vi.useFakeTimers()
    const invalidate = vi.spyOn(queryClient, 'invalidateQueries')

    onLeaguesChanged()
    vi.advanceTimersByTime(300)
    onLeaguesChanged()
    vi.advanceTimersByTime(499)
    expect(invalidate).not.toHaveBeenCalled()

    vi.advanceTimersByTime(1)
    expect(invalidate.mock.calls).toEqual([
      [{ queryKey: keys.allLeagues }, { cancelRefetch: false }],
      [{ queryKey: keys.allDecks }, { cancelRefetch: false }],
      [{ queryKey: keys.allLimited }, { cancelRefetch: false }],
      [{ queryKey: keys.allDashboard }, { cancelRefetch: false }],
    ])
    vi.advanceTimersByTime(5_000)
    expect(invalidate).toHaveBeenCalledTimes(4)
  })

  it('keeps the deck pages league queries under the deck key and the page under the league key', () => {
    expect(keys.deckLeagues(1, null, 'alltime')[0]).toBe(keys.allDecks[0])
    expect(keys.deckTournaments(1, 2, 'week')[0]).toBe(keys.allDecks[0])
    expect(keys.deckLeaguePanels(1, null)[0]).toBe(keys.allDecks[0])
    expect(keys.leagueAvailableMatches('manual:1')[0]).toBe(keys.allLeagues[0])
  })

  it('keeps every dashboard query under the dashboard key', () => {
    expect(keys.homeDashboard('week', 'all')[0]).toBe(keys.allDashboard[0])
    expect(keys.dashboardPicks(null)[0]).toBe(keys.allDashboard[0])
  })
})

describe('match events refresh deck numbers', () => {
  afterEach(() => {
    vi.useRealTimers()
  })

  /** Deck invalidations so far, with their options. */
  function watchDeckInvalidations(): () => unknown[][] {
    const invalidate = vi.spyOn(queryClient, 'invalidateQueries')
    return () =>
      invalidate.mock.calls
        .filter((call) => (call[0] as { queryKey?: unknown } | undefined)?.queryKey === keys.allDecks)
        .map((call) => [call[0], call[1]])
  }

  function stubFrames(): void {
    // Match pages are patched on frames; this test only looks at decks.
    vi.stubGlobal('requestAnimationFrame', () => 0)
  }

  it('refetches deck queries once, 500 ms after a burst of settled updates and removals', () => {
    vi.useFakeTimers()
    stubFrames()
    const deckCalls = watchDeckInvalidations()

    onMatchUpdated(row('a'))
    onMatchUpdated({ ...row('b'), state: 'ended' })
    onMatchRemoved('c')
    vi.advanceTimersByTime(499)
    expect(deckCalls()).toEqual([])

    onMatchUpdated({ ...row('d'), state: 'abandoned' })
    vi.advanceTimersByTime(499)
    expect(deckCalls()).toEqual([])

    vi.advanceTimersByTime(1)
    // A rebuild sends many events: never cancel a deck fetch already running.
    expect(deckCalls()).toEqual([[{ queryKey: keys.allDecks }, { cancelRefetch: false }]])

    vi.advanceTimersByTime(5_000)
    expect(deckCalls()).toHaveLength(1)
  })

  it('leaves deck queries alone while a match is still in progress', () => {
    vi.useFakeTimers()
    stubFrames()
    const deckCalls = watchDeckInvalidations()

    onMatchUpdated({ ...row('a'), state: 'in_progress' })
    vi.advanceTimersByTime(5_000)
    expect(deckCalls()).toEqual([])
  })

  it('still patches the match pages', () => {
    vi.useFakeTimers()
    stubFrames()
    const push = vi.spyOn(liveClient, 'push')

    onMatchUpdated(row('a'))
    onMatchRemoved('b')

    expect(push.mock.calls).toEqual([
      [{ kind: 'updated', summary: row('a') }],
      [{ kind: 'removed', token: 'b' }],
    ])
  })
})

describe('match events refresh the dashboard', () => {
  afterEach(() => {
    vi.useRealTimers()
  })

  /** Dashboard invalidations so far, with their options. */
  function watchDashboardInvalidations(): () => unknown[][] {
    const invalidate = vi.spyOn(queryClient, 'invalidateQueries')
    return () =>
      invalidate.mock.calls
        .filter((call) => (call[0] as { queryKey?: unknown } | undefined)?.queryKey === keys.allDashboard)
        .map((call) => [call[0], call[1]])
  }

  it('refetches the dashboard once, 500 ms after a burst of settled updates and removals', () => {
    vi.useFakeTimers()
    vi.stubGlobal('requestAnimationFrame', () => 0)
    const dashboardCalls = watchDashboardInvalidations()

    onMatchUpdated(row('a'))
    onMatchRemoved('b')
    vi.advanceTimersByTime(DASHBOARD_REFRESH_DELAY_MS - 1)
    onLeaguesChanged()
    vi.advanceTimersByTime(DASHBOARD_REFRESH_DELAY_MS - 1)
    expect(dashboardCalls()).toEqual([])

    vi.advanceTimersByTime(1)
    expect(dashboardCalls()).toEqual([[{ queryKey: keys.allDashboard }, { cancelRefetch: false }]])
    vi.advanceTimersByTime(5_000)
    expect(dashboardCalls()).toHaveLength(1)
  })

  it('leaves the dashboard alone while a match is still in progress', () => {
    vi.useFakeTimers()
    vi.stubGlobal('requestAnimationFrame', () => 0)
    const dashboardCalls = watchDashboardInvalidations()

    onMatchUpdated({ ...row('a'), state: 'in_progress' })
    vi.advanceTimersByTime(5_000)
    expect(dashboardCalls()).toEqual([])
  })
})

describe('limited changed', () => {
  afterEach(() => {
    vi.useRealTimers()
  })

  /** Limited invalidations so far, with their options. */
  function watchLimitedInvalidations(): () => unknown[][] {
    const invalidate = vi.spyOn(queryClient, 'invalidateQueries')
    return () =>
      invalidate.mock.calls
        .filter((call) => (call[0] as { queryKey?: unknown } | undefined)?.queryKey === keys.allLimited)
        .map((call) => [call[0], call[1]])
  }

  it('refetches every Limited query once, 500 ms after a burst', () => {
    vi.useFakeTimers()
    const limitedCalls = watchLimitedInvalidations()
    onLimitedChanged()
    vi.advanceTimersByTime(300)
    onLimitedChanged()
    vi.advanceTimersByTime(LIMITED_REFRESH_DELAY_MS - 1)
    expect(limitedCalls()).toEqual([])
    vi.advanceTimersByTime(1)
    expect(limitedCalls()).toEqual([[{ queryKey: keys.allLimited }, { cancelRefetch: false }]])
    vi.advanceTimersByTime(5_000)
    expect(limitedCalls()).toHaveLength(1)
  })

  it('also follows settled matches, removals, leagues and imports', () => {
    vi.useFakeTimers()
    vi.stubGlobal('requestAnimationFrame', () => 0)
    const limitedCalls = watchLimitedInvalidations()

    onMatchUpdated({ ...row('a'), state: 'in_progress' })
    vi.advanceTimersByTime(5_000)
    expect(limitedCalls()).toEqual([])

    onMatchUpdated(row('a'))
    vi.advanceTimersByTime(LIMITED_REFRESH_DELAY_MS)
    onMatchRemoved('b')
    vi.advanceTimersByTime(LIMITED_REFRESH_DELAY_MS)
    onLeaguesChanged()
    vi.advanceTimersByTime(LIMITED_REFRESH_DELAY_MS)
    onImportProgress({ phase: 'finished', summary: {} as ImportSummary })
    vi.advanceTimersByTime(LIMITED_REFRESH_DELAY_MS)
    onHistoryImportsChanged(1)
    vi.advanceTimersByTime(LIMITED_REFRESH_DELAY_MS)
    expect(limitedCalls()).toHaveLength(5)
  })

  it('also refreshes the dashboard, whose limited cards count drafts and runs', () => {
    vi.useFakeTimers()
    const invalidate = vi.spyOn(queryClient, 'invalidateQueries')
    const dashboardCalls = () =>
      invalidate.mock.calls
        .filter((call) => (call[0] as { queryKey?: unknown } | undefined)?.queryKey === keys.allDashboard)
        .map((call) => [call[0], call[1]])
    onLimitedChanged()
    vi.advanceTimersByTime(DASHBOARD_REFRESH_DELAY_MS - 1)
    onLimitedChanged()
    vi.advanceTimersByTime(DASHBOARD_REFRESH_DELAY_MS - 1)
    expect(dashboardCalls()).toEqual([])
    vi.advanceTimersByTime(1)
    expect(dashboardCalls()).toEqual([[{ queryKey: keys.allDashboard }, { cancelRefetch: false }]])
  })

  it('keeps every Limited query under the Limited key', () => {
    expect(keys.limitedIndex({ set: null, kind: null, timeframe: null })[0]).toBe(keys.allLimited[0])
    expect(keys.limitedRunColours('draft:Q')[0]).toBe(keys.allLimited[0])
    expect(keys.limitedEvent('draft:Q')[0]).toBe(keys.allLimited[0])
    expect(keys.limitedDraft('draft:Q')[0]).toBe(keys.allLimited[0])
    expect(keys.limitedCrossDrafts('draft:Q')[0]).toBe(keys.allLimited[0])
    expect(keys.limitedDeck('draft:Q')[0]).toBe(keys.allLimited[0])
    expect(keys.limitedCards('draft:Q')[0]).toBe(keys.allLimited[0])
    expect(keys.limitedMatches('draft:Q')[0]).toBe(keys.allLimited[0])
    expect(keys.limitedMatch('draft:Q', 'tok')[0]).toBe(keys.allLimited[0])
  })
})

describe('app notifications', () => {
  it('raises the sync run toast with its words and route', () => {
    const { toasts, remove } = useToast()
    onAppNotification({
      kind: 'sync',
      title: 'Synced with MyMTGO',
      message: '1 match downloaded',
      route: '/settings/account',
    })
    const toast = toasts.value[toasts.value.length - 1]
    expect(toast).toMatchObject({
      type: 'sync',
      title: 'Synced with MyMTGO',
      message: '1 match downloaded',
      route: '/settings/account',
    })
    remove(toast.id)
  })
})

describe('guides changed', () => {
  it('refetches the deck guide pages and the archetype page block, never cancelling a fetch', () => {
    const invalidate = vi.spyOn(queryClient, 'invalidateQueries')
    invalidate.mockClear()
    onGuidesChanged()
    expect(invalidate.mock.calls).toEqual([[{ queryKey: guideKeys.all }, { cancelRefetch: false }]])
    expect(guideKeys.archetype(3).slice(0, 2)).toEqual([...guideKeys.all])
    expect(guideKeys.editor(1, 2).slice(0, 2)).toEqual([...guideKeys.all])
  })
})

describe('metagame shares ready', () => {
  it('refetches the Metagame and archetype pages, never cancelling a fetch', () => {
    const invalidate = vi.spyOn(queryClient, 'invalidateQueries')
    invalidate.mockClear()
    onMetagameSharesReady()
    expect(invalidate.mock.calls).toEqual([[{ queryKey: metagameKeys.all }, { cancelRefetch: false }]])
  })
})

describe('an account change', () => {
  afterEach(() => queryClient.clear())

  // Release spec 5: one AccountChanged and every page refetches.
  it('refetches every query but the pushed status and settings', () => {
    queryClient.setQueryData(keys.settings, { closeToTray: true })
    queryClient.setQueryData(keys.status, { files: [] })
    queryClient.setQueryData(keys.allDecks, [])
    queryClient.setQueryData(['anything', 1], 1)
    onAccountChanged()
    const stale = queryClient
      .getQueryCache()
      .getAll()
      .map((query) => [query.queryKey[0], query.state.isInvalidated])
    expect(stale).toEqual([
      ['settings', false],
      ['status', false],
      ['decks', true],
      ['anything', true],
    ])
  })
})

describe('live platforms', () => {
  const context = (fields: Partial<PlatformContextView>): PlatformContextView => ({
    current: 'mtgo',
    mtgo: true,
    arena: true,
    endstep: false,
    switchable: true,
    ...fields,
  })
  const live = (token: string, platform: 'mtgo' | 'arena') => ({ ...row(token), platform, state: 'in_progress' as const })

  afterEach(() => {
    livePlatforms.value = new Map()
  })

  it('tracks in-progress matches by platform, whichever platform the pages read', () => {
    queryClient.setQueryData(keys.platformContext, context({}))
    onMatchUpdated(live('a', 'arena'))
    onMatchUpdated(live('b', 'mtgo'))
    expect([...livePlatforms.value]).toEqual([
      ['a', 'arena'],
      ['b', 'mtgo'],
    ])
    onMatchUpdated({ ...live('a', 'arena'), state: 'complete' })
    expect([...livePlatforms.value]).toEqual([['b', 'mtgo']])
    onMatchRemoved('b')
    expect(livePlatforms.value.size).toBe(0)
  })

  it('seeds the matches in progress at start, never over a push made during the read', async () => {
    queryClient.setQueryData(keys.platformContext, context({}))
    let answer: (live: LiveMatchView[]) => void = () => {}
    const read = loadLivePlatforms(
      () => new Promise((resolve) => (answer = (data) => resolve({ status: 'ok', data }))),
    )
    onMatchUpdated({ ...live('ended', 'arena'), state: 'complete' })
    answer([
      { token: 'seeded', platform: 'arena' },
      { token: 'ended', platform: 'arena' },
    ])
    await read
    expect([...livePlatforms.value]).toEqual([['seeded', 'arena']])
  })

  it('forgets the pushes once the read settles: a later read is newer than them', async () => {
    queryClient.setQueryData(keys.platformContext, context({}))
    livePlatforms.value = new Map()
    await loadLivePlatforms(async () => ({ status: 'ok', data: [] }))
    onMatchUpdated({ ...live('again', 'arena'), state: 'complete' })
    await loadLivePlatforms(async () => ({ status: 'ok', data: [{ token: 'again', platform: 'arena' }] }))
    expect([...livePlatforms.value]).toEqual([['again', 'arena']])
  })

  it('refetches the context when a push is of a platform it reports without data', () => {
    queryClient.setQueryData(keys.platformContext, context({ arena: false, switchable: false }))
    const invalidate = vi.spyOn(queryClient, 'invalidateQueries')
    onMatchUpdated(live('a', 'mtgo'))
    expect(invalidate).not.toHaveBeenCalledWith({ queryKey: keys.platformContext })
    onMatchUpdated(live('b', 'arena'))
    expect(invalidate).toHaveBeenCalledWith({ queryKey: keys.platformContext })
  })

  it('an account change refetches the context too', () => {
    queryClient.setQueryData(keys.platformContext, context({}))
    onAccountChanged()
    expect(queryClient.getQueryState(keys.platformContext)?.isInvalidated).toBe(true)
  })
})
