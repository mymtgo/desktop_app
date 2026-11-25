import type { BestOfView, CardStatsQuery, DeckListQuery, LeagueListQuery, LimitedIndexQuery, MatchListQuery } from '@/bindings'
import type { CardsState } from './cards'
import type { OpponentsState } from './opponents'

export const keys = {
  /** The decks of the main list's rows (not under `['matches']`: the live client reads those keys as pages). */
  matchDecks: (tokens: string[]) => ['decks', 'matchDecks', tokens] as const,
  allMatches: ['matches'] as const,
  /** The main list's pages; a page's key starts with this, whatever its filters. */
  matches: (page: number) => ['matches', page] as const,
  /** One page of the main list, filtered and sorted (`keys.matches(query.page)` prefixes it). */
  matchList: (query: MatchListQuery) => ['matches', query.page, query] as const,
  status: ['status'] as const,
  /** Wrapped's year, dismissed flag and bundle (`wrapped_state`). */
  wrappedState: ['wrappedState'] as const,
  /** The Settings "Card data" card's counts; `CardsChanged` refetches them. */
  cardData: ['cardData'] as const,
  /** The Settings "Card images" card: the switch, usage and folder. */
  cardImages: ['cardImages'] as const,
  /** The Cards page's grid and archetype sheet; `CardsChanged` refetches them. */
  allCardsPage: ['cardsPage'] as const,
  cardsPage: (state: CardsState) => ['cardsPage', 'grid', state] as const,
  cardArchetypes: (group: string, format: string | null) => ['cardsPage', 'archetypes', group, format] as const,
  settings: ['settings'] as const,
  /** The header's switcher and the Accounts card (`accounts_list`); `AccountChanged` refetches it with every page. */
  accounts: ['accounts'] as const,
  /** The platform the pages read and which platforms have data; `AccountChanged` refetches it. */
  platformContext: ['platformContext'] as const,
  /** The Storage page's File Paths statuses (chosen folders counted); a save refetches them. */
  folderStatuses: ['folderStatuses'] as const,
  /** The Integrations page: both switches and the Endstep connection. */
  integrations: ['integrations'] as const,
  appInfo: ['appInfo'] as const,
  updates: ['updates'] as const,
  legacyImport: ['legacyImport'] as const,
  legacyImportProgress: ['legacyImportProgress'] as const,
  /** The first run's 0.x question (release spec 13.2); an import's end refetches it. */
  firstRun: ['firstRun'] as const,
  /** How many history imports are stored; `HistoryImportsChanged` pushes it. */
  historyImportedCount: ['historyImportedCount'] as const,
  /** Every deck query starts with this, so `DecksChanged` refetches them all. */
  allDecks: ['decks'] as const,
  deckList: (query: DeckListQuery) => ['decks', 'list', query] as const,
  /** The import page's deck versions; under `['decks']` so `DecksChanged` refetches them. */
  historyDeckVersions: ['decks', 'historyVersions'] as const,
  /** `timeframe`: the page's key, or null on pages without one. */
  deckHeader: (id: number, timeframe: string | null, bestOf: BestOfView | null) =>
    ['decks', 'header', id, timeframe, bestOf] as const,
  deckDashboard: (id: number, version: number | null, timeframe: string, bestOf: BestOfView | null) =>
    ['decks', 'dashboard', id, version, timeframe, bestOf] as const,
  /** Under `['decks']`, so deck, card and match events refetch it. */
  deckCardStats: (id: number, version: number | null, query: CardStatsQuery, bestOf: BestOfView | null) =>
    ['decks', 'cardStats', id, version, query, bestOf] as const,
  deckDecklist: (id: number, version: number | null) => ['decks', 'decklist', id, version] as const,
  /** Every deck's match list pages: a notes edit refetches them. */
  allDeckMatches: ['decks', 'matches'] as const,
  deckMatches: (id: number, version: number | null, timeframe: string, query: MatchListQuery, bestOf: BestOfView | null) =>
    ['decks', 'matches', id, version, timeframe, query, bestOf] as const,
  // The deck pages' league queries sit under `['decks']` too, so both
  // `DecksChanged` and `LeaguesChanged` refetch them.
  deckLeagues: (id: number, version: number | null, timeframe: string) =>
    ['decks', 'leagues', id, version, timeframe] as const,
  deckTournaments: (id: number, version: number | null, timeframe: string) =>
    ['decks', 'tournaments', id, version, timeframe] as const,
  deckLeaguePanels: (id: number, version: number | null) => ['decks', 'leaguePanels', id, version] as const,
  /** Every Arena deck's rank card; `RankChanged` refetches them (and `DecksChanged`, being under `['decks']`). */
  allDeckRank: ['decks', 'rank'] as const,
  /** An Arena deck's rank line and climb this season. */
  deckRank: (id: number, bestOf: BestOfView | null) => ['decks', 'rank', id, bestOf] as const,
  /** A deck's Matchups page and drawer (archetypes spec section 12). */
  deckMatchups: (id: number, version: number | null, timeframe: string, bestOf: BestOfView | null) =>
    ['decks', 'matchups', id, version, timeframe, bestOf] as const,
  deckMatchupDetail: (
    id: number,
    archetypeId: number,
    version: number | null,
    timeframe: string,
    bestOf: BestOfView | null,
  ) => ['decks', 'matchupDetail', id, archetypeId, version, timeframe, bestOf] as const,
  /** `opponent`: an archetype uuid, or null for every opponent. */
  deckGameStats: (id: number, timeframe: string, opponent: string | null) =>
    ['decks', 'gameStats', id, timeframe, opponent] as const,
  /** Undetected complete matches of a deck (the "Detect archetypes" banner). */
  deckPendingArchetypes: (id: number) => ['decks', 'pendingArchetypes', id] as const,
  /** The deck index's archetype tabs: the shell (sidebar and header) and each tab's data. */
  deckArchetypeShell: (archetypeId: number, format: string | null, timeframe: string) =>
    ['decks', 'archetypeShell', archetypeId, format, timeframe] as const,
  archetypeDeckMatches: (archetypeId: number, format: string | null, timeframe: string, query: MatchListQuery) =>
    ['decks', 'archetypeMatches', archetypeId, format, timeframe, query] as const,
  archetypeDeckMatchups: (archetypeId: number, format: string | null, timeframe: string) =>
    ['decks', 'archetypeMatchups', archetypeId, format, timeframe] as const,
  archetypeDeckTrends: (archetypeId: number, format: string | null, timeframe: string) =>
    ['decks', 'archetypeTrends', archetypeId, format, timeframe] as const,
  archetypeDeckCardStats: (archetypeId: number, format: string | null, query: CardStatsQuery) =>
    ['decks', 'archetypeCardStats', archetypeId, format, query] as const,
  /** Every Leagues page query starts with this, so `LeaguesChanged` refetches them all. */
  allLeagues: ['leagues'] as const,
  leagueList: (query: LeagueListQuery) => ['leagues', 'list', query] as const,
  leagueAvailableMatches: (runKey: string) => ['leagues', 'available', runKey] as const,
  /** Where a match is and the leagues it may move to ("Move to league..."). */
  matchLeagueTargets: (token: string) => ['leagues', 'targets', token] as const,
  /** Settings, Archives: under `leagues`, so league and match edits refetch it. */
  deletedItems: ['leagues', 'deleted'] as const,
  /** The leagues the "Add manual match" sheet offers, for a deck or for none (`0`). */
  manualLeagueOptions: (deckId: number) => ['leagues', 'manualOptions', deckId] as const,
  /** The rank gaps an Arena deck's manual match can claim; under `allDeckRank`, so `RankChanged` and `DecksChanged` refetch them. */
  manualRankGaps: (deckId: number) => ['decks', 'rank', 'manualGaps', deckId] as const,
  /**
   * Every Limited page query starts with this, so `LimitedChanged` (and the
   * match, league and import events) refetch them all.
   */
  allLimited: ['limited'] as const,
  limitedIndex: (query: LimitedIndexQuery) => ['limited', 'index', query] as const,
  /** The opponents' colours of a run's matches, read when its index row is expanded. */
  limitedRunColours: (key: string) => ['limited', 'runColours', key] as const,
  /** An event's sidebar (0.x shared props); `null` data for an unknown run. */
  limitedEvent: (key: string) => ['limited', 'event', key] as const,
  /** The whole draft review; the selected pick lives in the URL, not the key. */
  limitedDraft: (key: string) => ['limited', 'draft', key] as const,
  limitedCrossDrafts: (key: string) => ['limited', 'crossDrafts', key] as const,
  limitedDeck: (key: string) => ['limited', 'deck', key] as const,
  limitedCards: (key: string) => ['limited', 'cards', key] as const,
  limitedMatches: (key: string) => ['limited', 'matches', key] as const,
  /** One of the run's matches; `null` data when the run does not hold it. */
  limitedMatch: (key: string, token: string) => ['limited', 'match', key, token] as const,
  /** Every dashboard query starts with this, so match, deck and league changes refetch them all. */
  allDashboard: ['dashboard'] as const,
  /** The home dashboard for a timeframe and format (dashboard redesign spec 5.1). */
  homeDashboard: (timeframe: string, format: string) => ['dashboard', 'home', timeframe, format] as const,
  /** Most picked's picks for a set; `null` for every set. */
  dashboardPicks: (set: string | null) => ['dashboard', 'picks', set] as const,
  /**
   * One Opponents page. Under `['dashboard']` on purpose: the same match, deck,
   * league and import events that refresh the dashboard change these records.
   */
  opponentList: (query: OpponentsState) => ['dashboard', 'opponents', query] as const,
}
