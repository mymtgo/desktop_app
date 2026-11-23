import { shallowRef } from 'vue'
import {
  commands,
  events,
  type AppNotification,
  type ImportProgress,
  type LiveMatchView,
  type MatchPage,
  type MatchSummary,
  type PlatformContextView,
  type PlatformView,
  type Settings,
} from '@/bindings'
import { queryClient } from '@/queryClient'
import { keys } from './keys'
import { LiveClient } from './live'
import { guideKeys } from './guides'
import { matchDetailKeys } from './matchDetail'
import { archetypeKeys } from './archetypes'
import { metagameKeys } from './metagame'
import { pushCounter } from './newerPush'
import { useToast } from '@/composables/useToast'

/** Rows being updated right now: what they will look like once committed. */
export const previews = shallowRef(new Map<string, MatchSummary>())

/**
 * The platform of every match in progress, by token, whichever platform the
 * pages read: the header's dot on the other platform's logo reads it.
 */
export const livePlatforms = shallowRef(new Map<string, PlatformView>())

/**
 * Tokens pushed while a read of the matches in progress runs: the read never
 * overrides them. Kept only while a read runs (`liveReads`), so the set never
 * outgrows the matches pushed during one read.
 */
const pushedTokens = new Set<string>()
let liveReads = 0

/** Remember a pushed token if a read is running; a read started later already sees the push. */
function notePushed(token: string): void {
  if (liveReads > 0) {
    pushedTokens.add(token)
  }
}

/**
 * Add the matches in progress when the window starts (`liveMatches`), so the
 * header's dot shows before the first push. A token a push already set or
 * cleared keeps what the push said: it is newer.
 */
function seedLivePlatforms(live: LiveMatchView[]): void {
  const next = new Map(livePlatforms.value)
  for (const match of live) {
    if (!pushedTokens.has(match.token)) {
      next.set(match.token, match.platform)
    }
  }
  livePlatforms.value = next
}

/** Pushes this window has received, so a fetch never overwrites a newer push (see `fetchUnlessPushed`). */
export const statusPushes = pushCounter()
export const updatePushes = pushCounter()

/** The window's one live client: pushes patch cached pages through it. */
export const liveClient = new LiveClient({
  currentPlatform: () => queryClient.getQueryData<PlatformContextView>(keys.platformContext)?.current ?? null,
  schedule: (run) => requestAnimationFrame(() => run()),
  pages: () =>
    queryClient
      .getQueriesData<MatchPage>({ queryKey: keys.allMatches })
      .flatMap(([key, page]) => (page ? [{ key, page }] : [])),
  setPage: (key, page) => queryClient.setQueryData(key, page),
  // Never cancel a fetch already running: during a rebuild that would starve it.
  refetchFirstPage: () => void queryClient.invalidateQueries({ queryKey: keys.matches(1) }, { cancelRefetch: false }),
  // As above: a rebuild removes matches on many frames, so never cancel a running fetch.
  invalidateLaterPages: () =>
    void queryClient.invalidateQueries(
      {
        queryKey: keys.allMatches,
        predicate: (query) => query.queryKey[1] !== 1,
      },
      { cancelRefetch: false },
    ),
  refetchPage: (key) => void queryClient.invalidateQueries({ queryKey: key, exact: true }, { cancelRefetch: false }),
  resetPages: () => void queryClient.resetQueries({ queryKey: keys.allMatches }),
  setPreviews: (value) => {
    previews.value = value
  },
})

/** Read the matches in progress into `livePlatforms`; a failed read leaves the pushes to fill it. */
export function loadLivePlatforms(read: typeof commands.liveMatches = commands.liveMatches): Promise<void> {
  liveReads += 1
  return read()
    .then((result) => {
      if (result.status === 'ok') {
        seedLivePlatforms(result.data)
      }
    })
    .finally(() => {
      liveReads -= 1
      if (liveReads === 0) {
        pushedTokens.clear()
      }
    })
}

/** Listen to every pushed event for the window's lifetime. Returns a stop function. */
export async function startLiveEvents(): Promise<() => void> {
  const stops = await Promise.all([
    events.matchUpdated.listen((e) => onMatchUpdated(e.payload.summary)),
    events.matchPreviewed.listen((e) => liveClient.push({ kind: 'previewed', summary: e.payload.summary })),
    events.matchRemoved.listen((e) => onMatchRemoved(e.payload.token)),
    events.matchAttention.listen((e) =>
      liveClient.push({ kind: 'attention', token: e.payload.token, reason: e.payload.reason }),
    ),
    events.statusChanged.listen((e) => {
      statusPushes.note()
      queryClient.setQueryData(keys.status, e.payload.status)
    }),
    events.trackerRestarting.listen(() => {
      livePlatforms.value = new Map()
      pushedTokens.clear()
      void loadLivePlatforms()
      liveClient.restart()
    }),
    events.settingsChanged.listen((e) => onSettingsChanged(e.payload.settings)),
    events.updateStatusChanged.listen((e) => {
      updatePushes.note()
      queryClient.setQueryData(keys.updates, e.payload.status)
    }),
    events.importProgressChanged.listen((e) => onImportProgress(e.payload.progress)),
    events.decksChanged.listen(() => onDecksChanged()),
    events.accountChanged.listen(() => onAccountChanged()),
    events.leaguesChanged.listen(() => onLeaguesChanged()),
    events.limitedChanged.listen(() => onLimitedChanged()),
    events.historyImportsChanged.listen((e) => onHistoryImportsChanged(e.payload.importedCount)),
    events.appNotification.listen((e) => onAppNotification(e.payload)),
    events.cardsChanged.listen(() => onCardsChanged()),
    events.guidesChanged.listen(() => onGuidesChanged()),
    events.matchArchetypesChanged.listen(() => onArchetypesChanged()),
    events.archetypesChanged.listen(() => onArchetypesChanged()),
    events.metagameSharesReady.listen(() => onMetagameSharesReady()),
    events.rankChanged.listen(() => onRankChanged()),
  ])
  // Listening first, so a push during the read wins over it.
  void loadLivePlatforms()
  return () => stops.forEach((stop) => stop())
}

/**
 * The settings changed: take the pushed value, and recheck the folder statuses
 * (a folder choice made elsewhere, or by an import, moves them).
 */
export function onSettingsChanged(settings: Settings): void {
  queryClient.setQueryData(keys.settings, settings)
  void queryClient.invalidateQueries({ queryKey: keys.folderStatuses })
  // The Endstep helper registration finishes after the switch returns.
  void queryClient.invalidateQueries({ queryKey: keys.integrations })
}

/** A toast the app raised (0.x `AppNotification`): the sync run's "Synced with MyMTGO", a live match's "Match recorded 2-1". */
export function onAppNotification(notification: AppNotification): void {
  useToast().add({
    type: notification.kind,
    title: notification.title,
    message: notification.message,
    route: notification.route,
  })
}

/**
 * A 0.x import moved to its next phase. When it ends, refetch the match pages
 * and match detail pages, status, import card, settings (0.x's deck settings),
 * the dashboard (its layout may come from 0.x), and the archetype and Metagame
 * pages (0.x's archetypes, match archetypes, guides and notes): the import
 * added rows no other event announced.
 * A failed import may have committed earlier batches, so it refetches too.
 */
export function onImportProgress(progress: ImportProgress): void {
  queryClient.setQueryData(keys.legacyImportProgress, progress)
  if (progress.phase === 'finished' || progress.phase === 'failed') {
    void queryClient.invalidateQueries({ queryKey: keys.allMatches })
    void queryClient.invalidateQueries({ queryKey: keys.status })
    void queryClient.invalidateQueries({ queryKey: keys.legacyImport })
    // A first-run migration that finished gives way to the app.
    void queryClient.invalidateQueries({ queryKey: keys.firstRun })
    void queryClient.invalidateQueries({ queryKey: keys.settings })
    void queryClient.invalidateQueries({ queryKey: keys.allDashboard })
    // Each match's 0.x detail: it changes no match revision or row.
    void queryClient.invalidateQueries({ queryKey: matchDetailKeys.all })
    void queryClient.invalidateQueries({ queryKey: archetypeKeys.all })
    void queryClient.invalidateQueries({ queryKey: metagameKeys.all })
    scheduleLimitedRefresh()
  }
}

/**
 * MTGO history matches were imported or deleted: keep the import page's count,
 * and refetch the match pages and the dashboard, which no other event
 * announces in bulk. The tracker sends `DecksChanged` and `LeaguesChanged` for
 * the deck and league pages.
 */
export function onHistoryImportsChanged(importedCount: number): void {
  queryClient.setQueryData(keys.historyImportedCount, importedCount)
  void queryClient.invalidateQueries({ queryKey: keys.allMatches })
  void queryClient.invalidateQueries({ queryKey: keys.allDashboard })
  scheduleLimitedRefresh()
}

/**
 * The current account may have changed (a Login line, a switch from the
 * header, a tracking change; release spec 5): every page reads it, so every
 * query refetches but the pushed status and settings, never cancelling a
 * fetch already running.
 */
export function onAccountChanged(): void {
  void queryClient.invalidateQueries(
    {
      // Kept current by their own pushes, and the same for every account.
      predicate: (query) => !PUSHED_KEYS.includes(String(query.queryKey[0])),
    },
    { cancelRefetch: false },
  )
}

/** Caches the app keeps current by pushes (`StatusChanged`, `SettingsChanged`). */
const PUSHED_KEYS: string[] = [keys.status[0], keys.settings[0]]

/**
 * Stored decks changed (a sync, an edit, an import or a new match link):
 * refetch every deck query, and the league queries (their cards show deck
 * names and colours, and a new link can move a match between runs), never
 * cancelling a fetch already running. The dashboard follows once things go
 * quiet.
 */
export function onDecksChanged(): void {
  void queryClient.invalidateQueries({ queryKey: keys.allDecks }, { cancelRefetch: false })
  void queryClient.invalidateQueries({ queryKey: keys.allLeagues }, { cancelRefetch: false })
  scheduleDashboardRefresh()
}

/**
 * A sideboard guide or matchup note was created, saved or deleted: refetch
 * the deck guide pages and the archetype page's block. The editor keeps its
 * own selection, so a refetch never undoes a change being typed.
 */
export function onGuidesChanged(): void {
  void queryClient.invalidateQueries({ queryKey: guideKeys.all }, { cancelRefetch: false })
}

/**
 * Card data was filled (names, details, images). Refetch the card data
 * counts, the Cards page (its grid and archetype sheet), the deck pages and
 * the match detail pages, which show card names, the archetype and Metagame
 * pages (their decklists read mana costs from the card data, issue #326;
 * the live decklists come from the cache, so no API request), and the
 * Limited pages once things go quiet. A fetch already running is never
 * cancelled.
 */
export function onCardsChanged(): void {
  void queryClient.invalidateQueries({ queryKey: keys.cardData }, { cancelRefetch: false })
  void queryClient.invalidateQueries({ queryKey: keys.allCardsPage }, { cancelRefetch: false })
  void queryClient.invalidateQueries({ queryKey: keys.allDecks }, { cancelRefetch: false })
  void queryClient.invalidateQueries({ queryKey: matchDetailKeys.all }, { cancelRefetch: false })
  void queryClient.invalidateQueries({ queryKey: archetypeKeys.all }, { cancelRefetch: false })
  void queryClient.invalidateQueries({ queryKey: metagameKeys.all }, { cancelRefetch: false })
  scheduleLimitedRefresh()
}

/**
 * The archetypes worker has the global metagame shares: refetch the Metagame
 * and archetype pages, which show them. A fetch already running is never
 * cancelled.
 */
export function onMetagameSharesReady(): void {
  void queryClient.invalidateQueries({ queryKey: metagameKeys.all }, { cancelRefetch: false })
}

/** How long archetype-showing queries wait after the last archetype event before refetching. */
export const ARCHETYPE_REFRESH_DELAY_MS = 500

let archetypeRefreshTimer: ReturnType<typeof setTimeout> | null = null

/**
 * Archetypes changed: detection (the backfill, a just-finished match, the
 * deck "Detect archetypes" banner), a hand edit, a definitions check or a
 * 0.x import. Refetch every page that shows archetype names or counts once
 * things go quiet: the match lists and match pages, the deck pages (matchups,
 * card stats options, guides), the leagues, the dashboard cards and the
 * Opponents page, the Cards page popularity, the archetype and Metagame pages.
 * A backfill sends one event per slice, and a fetch already running is never
 * cancelled.
 */
export function onArchetypesChanged(): void {
  if (archetypeRefreshTimer !== null) clearTimeout(archetypeRefreshTimer)
  archetypeRefreshTimer = setTimeout(() => {
    archetypeRefreshTimer = null
    const quiet = { cancelRefetch: false }
    void queryClient.invalidateQueries({ queryKey: keys.allMatches }, quiet)
    void queryClient.invalidateQueries({ queryKey: matchDetailKeys.all }, quiet)
    void queryClient.invalidateQueries({ queryKey: keys.allDecks }, quiet)
    void queryClient.invalidateQueries({ queryKey: keys.allLeagues }, quiet)
    void queryClient.invalidateQueries({ queryKey: keys.allDashboard }, quiet)
    void queryClient.invalidateQueries({ queryKey: keys.allCardsPage }, quiet)
    void queryClient.invalidateQueries({ queryKey: archetypeKeys.all }, quiet)
    void queryClient.invalidateQueries({ queryKey: metagameKeys.all }, quiet)
  }, ARCHETYPE_REFRESH_DELAY_MS)
}

/**
 * Arena rank readings were stored (a ranked match, a manual match's gap
 * claim): refetch the Arena deck rank cards, and the dashboard's rank cards
 * once things go quiet. A fetch already running is never cancelled.
 */
export function onRankChanged(): void {
  void queryClient.invalidateQueries({ queryKey: keys.allDeckRank }, { cancelRefetch: false })
  scheduleDashboardRefresh()
}

/** How long dashboard queries wait after the last match, deck or league event before refetching. */
export const DASHBOARD_REFRESH_DELAY_MS = 500

let dashboardRefreshTimer: ReturnType<typeof setTimeout> | null = null

/**
 * Refetch every dashboard query once things go quiet: its cards count
 * matches, decks and league runs, and a rebuild or a live league sends a
 * burst of events. A dashboard fetch already running is never cancelled.
 */
function scheduleDashboardRefresh(): void {
  if (dashboardRefreshTimer !== null) clearTimeout(dashboardRefreshTimer)
  dashboardRefreshTimer = setTimeout(() => {
    dashboardRefreshTimer = null
    void queryClient.invalidateQueries({ queryKey: keys.allDashboard }, { cancelRefetch: false })
  }, DASHBOARD_REFRESH_DELAY_MS)
}

/** How long league and deck queries wait after the last `LeaguesChanged` before refetching. */
export const LEAGUE_REFRESH_DELAY_MS = 500

let leagueRefreshTimer: ReturnType<typeof setTimeout> | null = null

/**
 * League runs or tournaments changed (a league or tournament match, a league
 * event, an edit or an import). Refetch the league and deck queries once
 * things go quiet: a live league match sends one event per tick, and a fetch
 * already running is never cancelled.
 */
export function onLeaguesChanged(): void {
  if (leagueRefreshTimer !== null) clearTimeout(leagueRefreshTimer)
  leagueRefreshTimer = setTimeout(() => {
    leagueRefreshTimer = null
    void queryClient.invalidateQueries({ queryKey: keys.allLeagues }, { cancelRefetch: false })
    void queryClient.invalidateQueries({ queryKey: keys.allDecks }, { cancelRefetch: false })
  }, LEAGUE_REFRESH_DELAY_MS)
  scheduleLimitedRefresh()
  scheduleDashboardRefresh()
}

/** How long Limited queries wait after the last Limited, match, league or import event. */
export const LIMITED_REFRESH_DELAY_MS = 500

let limitedRefreshTimer: ReturnType<typeof setTimeout> | null = null

/**
 * Refetch every Limited query once things go quiet: a live draft sends an
 * event per pick, and a league match changes a limited run's record. A
 * fetch already running is never cancelled.
 */
function scheduleLimitedRefresh(): void {
  if (limitedRefreshTimer !== null) clearTimeout(limitedRefreshTimer)
  limitedRefreshTimer = setTimeout(() => {
    limitedRefreshTimer = null
    void queryClient.invalidateQueries({ queryKey: keys.allLimited }, { cancelRefetch: false })
  }, LIMITED_REFRESH_DELAY_MS)
}

/**
 * Drafts, picks or pick notes changed (or 0.x Limited data was imported).
 * The dashboard's limited cards count drafts and runs, so it refreshes too.
 */
export function onLimitedChanged(): void {
  scheduleLimitedRefresh()
  scheduleDashboardRefresh()
}

/** How long deck queries wait after the last match event before refetching. */
export const DECK_REFRESH_DELAY_MS = 500

let deckRefreshTimer: ReturnType<typeof setTimeout> | null = null

/**
 * Refetch every deck query once things go quiet. A rebuild sends a burst of
 * match events: the trailing delay turns it into one refetch, and a deck fetch
 * already running is never cancelled.
 */
function scheduleDeckRefresh(): void {
  if (deckRefreshTimer !== null) clearTimeout(deckRefreshTimer)
  deckRefreshTimer = setTimeout(() => {
    deckRefreshTimer = null
    void queryClient.invalidateQueries({ queryKey: keys.allDecks }, { cancelRefetch: false })
  }, DECK_REFRESH_DELAY_MS)
}

/**
 * Keep `livePlatforms` (in progress: set; otherwise: delete). A summary of a
 * platform the cached context reports without data makes the context
 * refetch: the switch appears with the first Arena match.
 */
function noteLivePlatform(summary: MatchSummary): void {
  notePushed(summary.token)
  const next = new Map(livePlatforms.value)
  if (summary.state === 'in_progress') {
    next.set(summary.token, summary.platform)
  } else {
    next.delete(summary.token)
  }
  livePlatforms.value = next
  const context = queryClient.getQueryData<PlatformContextView>(keys.platformContext)
  if (context !== undefined && !context[summary.platform]) {
    void queryClient.invalidateQueries({ queryKey: keys.platformContext })
  }
}

/**
 * A match's committed result changed. Patch the match pages; once the match
 * is no longer in progress its result can count in deck and dashboard numbers
 * (record, win rate, last played), which no deck event announces, so refresh
 * them too.
 */
export function onMatchUpdated(summary: MatchSummary): void {
  noteLivePlatform(summary)
  liveClient.push({ kind: 'updated', summary })
  if (summary.state !== 'in_progress') {
    scheduleDeckRefresh()
    scheduleLimitedRefresh()
    scheduleDashboardRefresh()
  }
}

/** A match was removed (a rebuild or a merge): it may have counted in deck and dashboard numbers. */
export function onMatchRemoved(token: string): void {
  notePushed(token)
  if (livePlatforms.value.has(token)) {
    const next = new Map(livePlatforms.value)
    next.delete(token)
    livePlatforms.value = next
  }
  liveClient.push({ kind: 'removed', token })
  scheduleDeckRefresh()
  scheduleLimitedRefresh()
  scheduleDashboardRefresh()
}
