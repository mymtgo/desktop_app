import { keepPreviousData, useQuery, type QueryClient } from '@tanstack/vue-query'
import { computed, type MaybeRef, type Ref } from 'vue'
import {
  commands,
  type BestOfView,
  type CardStatsQuery,
  type DeckListQuery,
  type ImportProgress,
  type LeagueListQuery,
  type LimitedIndexQuery,
  type MatchListQuery,
  type StatusView,
  type UpdateStatus,
} from '@/bindings'
import { unwrap } from './commands'
import { keys } from './keys'
import { liveClient, statusPushes, updatePushes } from './liveEvents'
import { fetchUnlessPushed } from './newerPush'
import type { CardsState } from './cards'
import type { OpponentsState } from './opponents'

export { keys }

/** One page of the main match list, filtered and sorted; pushes patch it through the live client. */
export function useMatchesPage(query: Ref<MatchListQuery>) {
  return useQuery({
    queryKey: computed(() => keys.matchList(query.value)),
    queryFn: async ({ queryKey }) => liveClient.reconcile(unwrap(await commands.listMatches(queryKey[2]))),
    placeholderData: keepPreviousData,
  })
}

/** The decks of the listed matches, for the Matches page's Deck column. */
export function useMatchDecks(tokens: Ref<string[]>) {
  return useQuery({
    queryKey: computed(() => keys.matchDecks(tokens.value)),
    queryFn: async ({ queryKey }) => unwrap(await commands.listedMatchDecks(queryKey[2])),
    enabled: computed(() => tokens.value.length > 0),
    placeholderData: keepPreviousData,
  })
}

/** Only keys a disabled deck list query. */
const emptyDeckQuery: DeckListQuery = {
  format: null,
  recent: false,
  search: null,
  hideArchived: true,
  sort: 'last_played',
  page: 1,
  perPage: 12,
  archetype: null,
}

/** One page of the deck list; waits until the query can be built (it needs the settings). */
export function useDeckList(query: Ref<DeckListQuery | null>) {
  return useQuery({
    queryKey: computed(() => keys.deckList(query.value ?? emptyDeckQuery)),
    queryFn: async ({ queryKey }) => unwrap(await commands.listDecks(queryKey[2])),
    enabled: computed(() => query.value !== null),
    placeholderData: keepPreviousData,
  })
}

// The deck queries keep showing the previous data while a new timeframe or
// version loads, but not across decks: another deck's numbers would be
// wrong, not just stale.

/** A deck's sidebar (name, versions); `null` data when the deck does not exist. */
export function useDeckHeader(id: Ref<number>, timeframe: Ref<string | null>, bestOf: Ref<BestOfView | null>) {
  return useQuery({
    queryKey: computed(() => keys.deckHeader(id.value, timeframe.value, bestOf.value)),
    queryFn: async ({ queryKey }) => unwrap(await commands.deckHeader(queryKey[2], queryKey[3], queryKey[4])),
    placeholderData: (previous, previousQuery) => (previousQuery?.queryKey[2] === id.value ? previous : undefined),
  })
}

/**
 * The home dashboard for a timeframe and format: every card in one read.
 * A new timeframe or format keeps the previous page on screen while it loads.
 */
export function useHomeDashboard(timeframe: Ref<string>, format: Ref<string>) {
  return useQuery({
    queryKey: computed(() => keys.homeDashboard(timeframe.value, format.value)),
    queryFn: async ({ queryKey }) => unwrap(await commands.homeDashboard(queryKey[2], queryKey[3])),
    placeholderData: keepPreviousData,
  })
}

/** Most picked's picks for another set (`null`: every set); read only when `enabled`. */
export function useDashboardPicks(set: Ref<string | null>, enabled: Ref<boolean>) {
  return useQuery({
    queryKey: computed(() => keys.dashboardPicks(set.value)),
    queryFn: async ({ queryKey }) => unwrap(await commands.dashboardPicks(queryKey[2])),
    enabled,
    placeholderData: keepPreviousData,
  })
}

/** A deck's dashboard numbers for one version (null: all) and timeframe. */
export function useDeckDashboard(
  id: Ref<number>,
  version: Ref<number | null>,
  timeframe: Ref<string>,
  bestOf: Ref<BestOfView | null>,
) {
  return useQuery({
    queryKey: computed(() => keys.deckDashboard(id.value, version.value, timeframe.value, bestOf.value)),
    queryFn: async ({ queryKey }) => unwrap(await commands.deckDashboard(queryKey[2], queryKey[3], queryKey[4], queryKey[5])),
    placeholderData: (previous, previousQuery) => (previousQuery?.queryKey[2] === id.value ? previous : undefined),
  })
}

/** A deck's Card Stats tab for one version (null: all) and the filters. */
export function useDeckCardStats(
  id: Ref<number>,
  version: Ref<number | null>,
  query: Ref<CardStatsQuery>,
  bestOf: Ref<BestOfView | null>,
) {
  return useQuery({
    queryKey: computed(() => keys.deckCardStats(id.value, version.value, query.value, bestOf.value)),
    queryFn: async ({ queryKey }) => unwrap(await commands.deckCardStats(queryKey[2], queryKey[3], { ...queryKey[4], bestOf: queryKey[5] })),
    placeholderData: (previous, previousQuery) => (previousQuery?.queryKey[2] === id.value ? previous : undefined),
  })
}

/** A deck version's list (null: the latest version), grouped, with its curve and colours. */
export function useDeckDecklist(id: Ref<number>, version: Ref<number | null>) {
  return useQuery({
    queryKey: computed(() => keys.deckDecklist(id.value, version.value)),
    queryFn: async ({ queryKey }) => unwrap(await commands.deckDecklist(queryKey[2], queryKey[3])),
    placeholderData: (previous, previousQuery) => (previousQuery?.queryKey[2] === id.value ? previous : undefined),
  })
}

/** One page of a deck's complete matches for a version (null: all) and timeframe, filtered and sorted. */
export function useDeckMatches(
  id: Ref<number>,
  version: Ref<number | null>,
  timeframe: Ref<string>,
  query: Ref<MatchListQuery>,
  bestOf: Ref<BestOfView | null>,
) {
  return useQuery({
    queryKey: computed(() => keys.deckMatches(id.value, version.value, timeframe.value, query.value, bestOf.value)),
    queryFn: async ({ queryKey }) =>
      unwrap(await commands.deckMatches(queryKey[2], queryKey[3], queryKey[4], queryKey[5], queryKey[6])),
    placeholderData: (previous, previousQuery) => (previousQuery?.queryKey[2] === id.value ? previous : undefined),
  })
}

/** A deck's Leagues tab: its runs in the timeframe (version null: all) and their KPIs. */
export function useDeckLeagues(id: Ref<number>, version: Ref<number | null>, timeframe: Ref<string>) {
  return useQuery({
    queryKey: computed(() => keys.deckLeagues(id.value, version.value, timeframe.value)),
    queryFn: async ({ queryKey }) => unwrap(await commands.deckLeagues(queryKey[2], queryKey[3], queryKey[4])),
    placeholderData: (previous, previousQuery) => (previousQuery?.queryKey[2] === id.value ? previous : undefined),
  })
}

/** A deck's Tournaments tab: tournaments with a complete deck match in the timeframe. */
export function useDeckTournaments(id: Ref<number>, version: Ref<number | null>, timeframe: Ref<string>) {
  return useQuery({
    queryKey: computed(() => keys.deckTournaments(id.value, version.value, timeframe.value)),
    queryFn: async ({ queryKey }) => unwrap(await commands.deckTournaments(queryKey[2], queryKey[3], queryKey[4])),
    placeholderData: (previous, previousQuery) => (previousQuery?.queryKey[2] === id.value ? previous : undefined),
  })
}

/** A deck dashboard's league panels: finishes and the current or latest run. */
export function useDeckLeaguePanels(id: Ref<number>, version: Ref<number | null>, enabled: MaybeRef<boolean> = true) {
  return useQuery({
    queryKey: computed(() => keys.deckLeaguePanels(id.value, version.value)),
    enabled,
    queryFn: async ({ queryKey }) => unwrap(await commands.deckLeaguePanels(queryKey[2], queryKey[3])),
    placeholderData: (previous, previousQuery) => (previousQuery?.queryKey[2] === id.value ? previous : undefined),
  })
}

/** An Arena deck's rank card (null: no constructed reading this season); only read where ranking is on. */
export function useDeckRank(id: Ref<number>, enabled: Ref<boolean>, bestOf: Ref<BestOfView | null>) {
  return useQuery({
    queryKey: computed(() => keys.deckRank(id.value, bestOf.value)),
    queryFn: async ({ queryKey }) => unwrap(await commands.deckRank(queryKey[2], queryKey[3])),
    enabled,
    placeholderData: (previous, previousQuery) => (previousQuery?.queryKey[2] === id.value ? previous : undefined),
  })
}

/** A deck's Matchups page: its results against each opponent archetype. */
export function useDeckMatchups(
  id: Ref<number>,
  version: Ref<number | null>,
  timeframe: Ref<string>,
  bestOf: Ref<BestOfView | null>,
) {
  return useQuery({
    queryKey: computed(() => keys.deckMatchups(id.value, version.value, timeframe.value, bestOf.value)),
    queryFn: async ({ queryKey }) => unwrap(await commands.deckMatchups(queryKey[2], queryKey[3], queryKey[4], queryKey[5])),
    placeholderData: (previous, previousQuery) => (previousQuery?.queryKey[2] === id.value ? previous : undefined),
  })
}

/** The Matchups drawer for one archetype, read while one is open. */
export function useDeckMatchupDetail(
  id: Ref<number>,
  archetypeId: Ref<number | null>,
  version: Ref<number | null>,
  timeframe: Ref<string>,
  bestOf: Ref<BestOfView | null>,
) {
  return useQuery({
    queryKey: computed(() =>
      keys.deckMatchupDetail(id.value, archetypeId.value ?? 0, version.value, timeframe.value, bestOf.value),
    ),
    queryFn: async ({ queryKey }) =>
      unwrap(await commands.deckMatchupDetail(queryKey[2], queryKey[3], queryKey[4], queryKey[5], queryKey[6])),
    enabled: computed(() => archetypeId.value !== null),
  })
}

/** A deck's Game Stats page. */
export function useDeckGameStats(id: Ref<number>, timeframe: Ref<string>, opponent: Ref<string | null>) {
  return useQuery({
    queryKey: computed(() => keys.deckGameStats(id.value, timeframe.value, opponent.value)),
    queryFn: async ({ queryKey }) => unwrap(await commands.deckGameStats(queryKey[2], queryKey[3], queryKey[4])),
    placeholderData: (previous, previousQuery) => (previousQuery?.queryKey[2] === id.value ? previous : undefined),
  })
}

/**
 * A deck's complete matches still waiting for detection; read again every
 * 3 s while any wait (0.x `ArchetypeDetectionBanner` polling).
 */
export function useDeckPendingArchetypes(id: Ref<number>) {
  return useQuery({
    queryKey: computed(() => keys.deckPendingArchetypes(id.value)),
    queryFn: async ({ queryKey }) => unwrap(await commands.pendingArchetypeCount(queryKey[2])),
    refetchInterval: (query) => ((query.state.data ?? 0) > 0 ? 3000 : false),
  })
}

/** The sidebar and header around an archetype tab. */
export function useDeckArchetypeShell(
  archetypeId: Ref<number>,
  format: Ref<string | null>,
  timeframe: Ref<string>,
) {
  return useQuery({
    queryKey: computed(() => keys.deckArchetypeShell(archetypeId.value, format.value, timeframe.value)),
    queryFn: async ({ queryKey }) =>
      unwrap(await commands.deckArchetypeShell(queryKey[2], queryKey[3], queryKey[4])),
    placeholderData: keepPreviousData,
  })
}

/** The archetype Matches tab. */
export function useArchetypeDeckMatches(
  archetypeId: Ref<number>,
  format: Ref<string | null>,
  timeframe: Ref<string>,
  query: Ref<MatchListQuery>,
) {
  return useQuery({
    queryKey: computed(() => keys.archetypeDeckMatches(archetypeId.value, format.value, timeframe.value, query.value)),
    queryFn: async ({ queryKey }) =>
      unwrap(await commands.archetypeDeckMatches(queryKey[2], queryKey[3], queryKey[4], queryKey[5])),
    placeholderData: keepPreviousData,
  })
}

/** The archetype Matchups tab. */
export function useArchetypeDeckMatchups(archetypeId: Ref<number>, format: Ref<string | null>, timeframe: Ref<string>) {
  return useQuery({
    queryKey: computed(() => keys.archetypeDeckMatchups(archetypeId.value, format.value, timeframe.value)),
    queryFn: async ({ queryKey }) =>
      unwrap(await commands.archetypeDeckMatchups(queryKey[2], queryKey[3], queryKey[4])),
    placeholderData: keepPreviousData,
  })
}

/** The archetype Trends tab. */
export function useArchetypeDeckTrends(archetypeId: Ref<number>, format: Ref<string | null>, timeframe: Ref<string>) {
  return useQuery({
    queryKey: computed(() => keys.archetypeDeckTrends(archetypeId.value, format.value, timeframe.value)),
    queryFn: async ({ queryKey }) =>
      unwrap(await commands.archetypeDeckTrends(queryKey[2], queryKey[3], queryKey[4])),
    placeholderData: keepPreviousData,
  })
}

/** The archetype Card stats tab. */
export function useArchetypeDeckCardStats(
  archetypeId: Ref<number>,
  format: Ref<string | null>,
  query: Ref<CardStatsQuery>,
) {
  return useQuery({
    queryKey: computed(() => keys.archetypeDeckCardStats(archetypeId.value, format.value, query.value)),
    queryFn: async ({ queryKey }) =>
      unwrap(await commands.archetypeDeckCardStats(queryKey[2], queryKey[3], queryKey[4])),
    placeholderData: keepPreviousData,
  })
}

/** One Leagues page. */
export function useLeagueList(query: Ref<LeagueListQuery>) {
  return useQuery({
    queryKey: computed(() => keys.leagueList(query.value)),
    queryFn: async ({ queryKey }) => unwrap(await commands.listLeagues(queryKey[2])),
    placeholderData: keepPreviousData,
  })
}

/** The Limited index for a set of filters. */
export function useLimitedIndex(query: Ref<LimitedIndexQuery>) {
  return useQuery({
    queryKey: computed(() => keys.limitedIndex(query.value)),
    queryFn: async ({ queryKey }) => unwrap(await commands.limitedIndex(queryKey[2])),
    placeholderData: keepPreviousData,
  })
}

/** The opponents' colours of an index row's matches; waits until the row is expanded. */
export function useLimitedRunColours(key: Ref<string | null>, expanded: Ref<boolean>) {
  return useQuery({
    queryKey: computed(() => keys.limitedRunColours(key.value ?? '')),
    queryFn: async ({ queryKey }) => unwrap(await commands.limitedRunColours(queryKey[2])),
    enabled: computed(() => expanded.value && key.value !== null),
  })
}

// The event queries keep nothing across runs: another run's numbers would be
// wrong, not just stale.

/** An event's shared props (sidebar); `null` data for an unknown or constructed run. */
export function useLimitedEvent(key: Ref<string>) {
  return useQuery({
    queryKey: computed(() => keys.limitedEvent(key.value)),
    queryFn: async ({ queryKey }) => unwrap(await commands.limitedEvent(queryKey[2])),
  })
}

/** The whole draft review of a run (the selected pick is chosen on the page). */
export function useLimitedDraft(key: Ref<string>) {
  return useQuery({
    queryKey: computed(() => keys.limitedDraft(key.value)),
    queryFn: async ({ queryKey }) => unwrap(await commands.limitedDraft(queryKey[2], null)),
  })
}

/** The stats of every card of a draft across the user's other drafts of the set: one read per draft. */
export function useLimitedCrossDrafts(key: Ref<string>) {
  return useQuery({
    queryKey: computed(() => keys.limitedCrossDrafts(key.value)),
    queryFn: async ({ queryKey }) => unwrap(await commands.limitedCrossDrafts(queryKey[2])),
  })
}

/** The Deck page of a run; `null` data for an unknown or constructed run. */
export function useLimitedDeck(key: Ref<string>, enabled: MaybeRef<boolean> = true) {
  return useQuery({
    queryKey: computed(() => keys.limitedDeck(key.value)),
    enabled,
    queryFn: async ({ queryKey }) => unwrap(await commands.limitedDeck(queryKey[2])),
  })
}

/** The Cards page of a run. */
export function useLimitedCards(key: Ref<string>) {
  return useQuery({
    queryKey: computed(() => keys.limitedCards(key.value)),
    queryFn: async ({ queryKey }) => unwrap(await commands.limitedCards(queryKey[2])),
  })
}

/** The Matches page of a run. */
export function useLimitedMatches(key: Ref<string>) {
  return useQuery({
    queryKey: computed(() => keys.limitedMatches(key.value)),
    queryFn: async ({ queryKey }) => unwrap(await commands.limitedMatches(queryKey[2])),
  })
}

/** One of the run's matches (Plan K's detail); `null` data when the run does not hold it. */
export function useLimitedMatch(key: Ref<string>, token: Ref<string>) {
  return useQuery({
    queryKey: computed(() => keys.limitedMatch(key.value, token.value)),
    queryFn: async ({ queryKey }) => unwrap(await commands.limitedMatch(queryKey[2], queryKey[3])),
  })
}

/** One Opponents page. */
export function useOpponents(query: Ref<OpponentsState>) {
  return useQuery({
    queryKey: computed(() => keys.opponentList(query.value)),
    queryFn: async ({ queryKey }) => {
      const { search, sort, format, page } = queryKey[2]
      return unwrap(await commands.opponents(search === '' ? null : search, sort, format, page))
    },
    placeholderData: keepPreviousData,
  })
}

/** The matches a manual league can link; waits for a run key. */
export function useLeagueAvailableMatches(runKey: Ref<string | null>) {
  return useQuery({
    queryKey: computed(() => keys.leagueAvailableMatches(runKey.value ?? '')),
    queryFn: async ({ queryKey }) => unwrap(await commands.leagueAvailableMatches(queryKey[2])),
    enabled: computed(() => runKey.value !== null),
  })
}

/** Where a match is and the leagues it may move to; waits for a token. */
export function useMatchLeagueTargets(token: Ref<string | null>) {
  return useQuery({
    queryKey: computed(() => keys.matchLeagueTargets(token.value ?? '')),
    queryFn: async ({ queryKey }) => unwrap(await commands.matchLeagueTargets(queryKey[2])),
    enabled: computed(() => token.value !== null),
  })
}

/** The deleted leagues and matches (Settings, Archives). */
export function useDeletedItems() {
  return useQuery({
    queryKey: keys.deletedItems,
    queryFn: async () => unwrap(await commands.deletedItems()),
  })
}

/** The leagues a manual match can join, for a deck or for none; waits for `enabled`. */
export function useManualLeagueOptions(deckId: Ref<number | null>, enabled: Ref<boolean>) {
  return useQuery({
    queryKey: computed(() => keys.manualLeagueOptions(deckId.value ?? 0)),
    queryFn: async ({ queryKey }) => unwrap(await commands.manualLeagueOptions(queryKey[2] === 0 ? null : queryKey[2])),
    enabled,
  })
}

/** The rank gaps the "Add manual match" sheet offers for an Arena deck, newest first. */
export function useManualRankGaps(deckId: Ref<number | null>, enabled: Ref<boolean>) {
  return useQuery({
    queryKey: computed(() => keys.manualRankGaps(deckId.value ?? 0)),
    queryFn: async ({ queryKey }) => unwrap(await commands.manualRankGaps(queryKey[3])),
    enabled: computed(() => enabled.value && deckId.value !== null),
  })
}

/** The update status, unless a push arrived while it was fetched. */
export function fetchUpdateStatus(client: QueryClient): Promise<UpdateStatus> {
  return fetchUnlessPushed(
    updatePushes.count,
    () => commands.getUpdateStatus(),
    () => client.getQueryData<UpdateStatus>(keys.updates),
  )
}

export function useUpdateStatus() {
  return useQuery({
    queryKey: keys.updates,
    queryFn: ({ client }) => fetchUpdateStatus(client),
  })
}

export function useLegacyImport() {
  return useQuery({
    queryKey: keys.legacyImport,
    queryFn: async () => unwrap(await commands.getLegacyImport()),
  })
}

/** The first run's 0.x question (release spec 13.2): ask, migrating, or settled. */
export function useFirstRun() {
  return useQuery({
    queryKey: keys.firstRun,
    queryFn: () => commands.firstRunState(),
  })
}

/** The latest progress event of the running (or just finished) import. */
export function useLegacyImportProgress() {
  return useQuery({
    queryKey: keys.legacyImportProgress,
    queryFn: () => null as ImportProgress | null,
    staleTime: Infinity,
    gcTime: Infinity,
  })
}

/** How many history imports are stored (0.x `importedCount`); pushes keep it current. */
export function useHistoryImportedCount() {
  return useQuery({
    queryKey: keys.historyImportedCount,
    queryFn: async () => unwrap(await commands.historyImportedCount()),
  })
}

/**
 * The deck versions the import page and "Set deck" offer, newest first,
 * archived decks too; read while `enabled` (a dialog only asks once it opens).
 */
export function useHistoryDeckVersions(enabled: MaybeRef<boolean> = true) {
  return useQuery({
    queryKey: keys.historyDeckVersions,
    queryFn: async () => unwrap(await commands.historyDeckVersions()),
    enabled,
  })
}

export function useStatus() {
  return useQuery({
    queryKey: keys.status,
    queryFn: ({ client }) =>
      fetchUnlessPushed(
        statusPushes.count,
        async () => unwrap(await commands.getStatus()),
        () => client.getQueryData<StatusView>(keys.status),
      ),
  })
}

/** Every account and the current one (release spec 5): the header and the Accounts card. */
export function useAccounts() {
  return useQuery({
    queryKey: keys.accounts,
    queryFn: async () => unwrap(await commands.accountsList()),
  })
}

/** The platform context (arena spec section 15): the one query fn the router guard and the header share. */
export async function fetchPlatformContext() {
  return unwrap(await commands.platformContext())
}

export function usePlatformContextQuery() {
  return useQuery({
    queryKey: keys.platformContext,
    queryFn: fetchPlatformContext,
  })
}

/** The Integrations page: Arena and Endstep switches and the Endstep connection. */
export function useIntegrationsQuery() {
  return useQuery({
    queryKey: keys.integrations,
    queryFn: async () => unwrap(await commands.integrations()),
  })
}

export function useSettings() {
  return useQuery({
    queryKey: keys.settings,
    queryFn: async () => unwrap(await commands.getSettings()),
  })
}

/** The chosen folders' file counts (0.x `ValidatePath`); the command counts on a blocking thread. */
export function useFolderStatuses() {
  return useQuery({
    queryKey: keys.folderStatuses,
    queryFn: async () => unwrap(await commands.folderStatuses()),
  })
}

export function useAppInfo() {
  return useQuery({
    queryKey: keys.appInfo,
    queryFn: () => commands.appInfo(),
    staleTime: Infinity,
  })
}

/** One page of the Cards page's grid. */
export function useCardsPage(state: Ref<CardsState>) {
  return useQuery({
    queryKey: computed(() => keys.cardsPage(state.value)),
    queryFn: async ({ queryKey }) => {
      const { search, format, hiddenTypes, groupPrintings, page } = queryKey[2]
      return unwrap(
        await commands.cardsPage({
          search: search === '' ? null : search,
          format,
          hiddenTypes,
          groupPrintings,
          page,
        }),
      )
    },
    placeholderData: keepPreviousData,
  })
}

/** The archetypes holding a card's group, read when its sheet opens. */
export function useCardArchetypes(group: Ref<string | null>, format: Ref<string | null>) {
  return useQuery({
    queryKey: computed(() => keys.cardArchetypes(group.value ?? '', format.value)),
    queryFn: async ({ queryKey }) => unwrap(await commands.cardArchetypes(queryKey[2], queryKey[3])),
    enabled: computed(() => group.value !== null),
  })
}
