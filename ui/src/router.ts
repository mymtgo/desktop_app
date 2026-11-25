import { createRouter, createWebHistory } from 'vue-router'
import CardsPage from '@/pages/CardsPage.vue'
import DeckLayout from '@/layouts/DeckLayout.vue'
import DashboardPage from '@/pages/DashboardPage.vue'
import WrappedPage from '@/pages/WrappedPage.vue'
import DecksPage from '@/pages/DecksPage.vue'
import ArchetypeCardStatsTab from '@/pages/decks/archetypes/CardStatsTab.vue'
import ArchetypeMatchesTab from '@/pages/decks/archetypes/MatchesTab.vue'
import ArchetypeMatchupsTab from '@/pages/decks/archetypes/MatchupsTab.vue'
import ArchetypeTrendsTab from '@/pages/decks/archetypes/TrendsTab.vue'
import DeckCardStatsPage from '@/pages/deck/CardStatsPage.vue'
import DeckDashboardPage from '@/pages/deck/DashboardPage.vue'
import DeckDecklistPage from '@/pages/deck/DecklistPage.vue'
import DeckGameStatsPage from '@/pages/deck/GameStatsPage.vue'
import DeckLeaguesPage from '@/pages/deck/LeaguesPage.vue'
import DeckMatchesPage from '@/pages/deck/MatchesPage.vue'
import DeckMatchPage from '@/pages/deck/MatchPage.vue'
import DeckMatchupsPage from '@/pages/deck/MatchupsPage.vue'
import DeckSettingsPage from '@/pages/deck/SettingsPage.vue'
import DeckSideboardGuideEditPage from '@/pages/deck/SideboardGuideEditPage.vue'
import DeckSideboardGuidesPage from '@/pages/deck/SideboardGuidesPage.vue'
import DeckTournamentsPage from '@/pages/deck/TournamentsPage.vue'
import ImportPage from '@/pages/ImportPage.vue'
import LeaguesPage from '@/pages/LeaguesPage.vue'
import type { LimitedPage } from '@/bindings'
import LimitedCardsPage from '@/pages/limited/CardsPage.vue'
import LimitedDeckPage from '@/pages/limited/DeckPage.vue'
import LimitedDraftPage from '@/pages/limited/DraftPage.vue'
import LimitedEventLayout from '@/pages/limited/LimitedEventLayout.vue'
import LimitedIndexPage from '@/pages/limited/LimitedIndexPage.vue'
import LimitedMatchesPage from '@/pages/limited/MatchesPage.vue'
import LimitedMatchPage from '@/pages/limited/MatchPage.vue'
import MatchesPage from '@/pages/MatchesPage.vue'
import MatchPage from '@/pages/MatchPage.vue'
import ArchetypeFormPage from '@/pages/metagame/ArchetypeFormPage.vue'
import ArchetypePage from '@/pages/metagame/ArchetypePage.vue'
import ManagePage from '@/pages/metagame/ManagePage.vue'
import MetagamePage from '@/pages/metagame/MetagamePage.vue'
import OpponentsPage from '@/pages/OpponentsPage.vue'
import SettingsLayout from '@/layouts/SettingsLayout.vue'
import AccountPage from '@/pages/settings/AccountPage.vue'
import { trackUsage } from '@/lib/usage'
import AdvancedPage from '@/pages/settings/AdvancedPage.vue'
import ArchivesPage from '@/pages/settings/ArchivesPage.vue'
import GeneralPage from '@/pages/settings/GeneralPage.vue'
import IntegrationsPage from '@/pages/settings/IntegrationsPage.vue'
import OverlaysPage from '@/pages/settings/OverlaysPage.vue'
import PrivacyPage from '@/pages/settings/PrivacyPage.vue'
import ReplaysPage from '@/pages/settings/ReplaysPage.vue'
import StoragePage from '@/pages/settings/StoragePage.vue'
import WhatsNewPage from '@/pages/WhatsNewPage.vue'
import { keys } from '@/lib/keys'
import { routeAllowed } from '@/lib/platformFeatures'
import { fetchPlatformContext } from '@/lib/queries'
import { queryClient } from '@/queryClient'

declare module 'vue-router' {
  interface RouteMeta {
    /** A deck page that reads `?timeframe=` (the sidebar carries it onto these links). */
    deckTimeframe?: boolean
    /** A Limited event page: the sidebar entry it highlights. */
    limitedPage?: LimitedPage
  }
}

export const router = createRouter({
  history: createWebHistory(),
  routes: [
    { path: '/', name: 'dashboard', component: DashboardPage },
    { path: '/wrapped', name: 'wrapped', component: WrappedPage },
    { path: '/matches', name: 'matches', component: MatchesPage },
    { path: '/matches/:token', name: 'match', component: MatchPage },
    { path: '/decks', name: 'decks', component: DecksPage },
    // The deck index's archetype tabs (archetypes spec section 12).
    { path: '/decks/archetypes/:archetypeId/matches', name: 'deck-archetype-matches', component: ArchetypeMatchesTab },
    { path: '/decks/archetypes/:archetypeId/matchups', name: 'deck-archetype-matchups', component: ArchetypeMatchupsTab },
    { path: '/decks/archetypes/:archetypeId/trends', name: 'deck-archetype-trends', component: ArchetypeTrendsTab },
    {
      path: '/decks/archetypes/:archetypeId/card-stats',
      name: 'deck-archetype-card-stats',
      component: ArchetypeCardStatsTab,
    },
    { path: '/cards', name: 'cards', component: CardsPage },
    {
      path: '/decks/:id',
      component: DeckLayout,
      children: [
        { path: '', redirect: (to) => ({ name: 'deck-dashboard', params: to.params, query: to.query }) },
        { path: 'dashboard', name: 'deck-dashboard', component: DeckDashboardPage, meta: { deckTimeframe: true } },
        { path: 'card-stats', name: 'deck-card-stats', component: DeckCardStatsPage, meta: { deckTimeframe: true } },
        {
          path: 'game-stats',
          name: 'deck-game-stats',
          component: DeckGameStatsPage,
          meta: { deckTimeframe: true },
        },
        { path: 'matches', name: 'deck-matches', component: DeckMatchesPage, meta: { deckTimeframe: true } },
        // A match opened from the deck's pages stays in the deck layout, so
        // the top nav keeps Decks active (issue #332). It carries the
        // version and timeframe back to the list.
        { path: 'matches/:token', name: 'deck-match', component: DeckMatchPage, meta: { deckTimeframe: true } },
        { path: 'leagues', name: 'deck-leagues', component: DeckLeaguesPage, meta: { deckTimeframe: true } },
        {
          path: 'tournaments',
          name: 'deck-tournaments',
          component: DeckTournamentsPage,
          meta: { deckTimeframe: true },
        },
        { path: 'matchups', name: 'deck-matchups', component: DeckMatchupsPage, meta: { deckTimeframe: true } },
        { path: 'sideboard-guides', name: 'deck-sideboard-guides', component: DeckSideboardGuidesPage },
        { path: 'sideboard-guides/:guideId', name: 'deck-sideboard-guide', component: DeckSideboardGuideEditPage },
        { path: 'decklist', name: 'deck-decklist', component: DeckDecklistPage },
        { path: 'settings', name: 'deck-settings', component: DeckSettingsPage },
      ],
    },
    { path: '/leagues', name: 'leagues', component: LeaguesPage },
    { path: '/limited', name: 'limited', component: LimitedIndexPage },
    {
      // `key` is a run key, URL-encoded (see `limitedPath`).
      path: '/limited/:key',
      component: LimitedEventLayout,
      children: [
        { path: '', redirect: (to) => ({ name: 'limited-draft', params: to.params }) },
        { path: 'draft', name: 'limited-draft', component: LimitedDraftPage, meta: { limitedPage: 'draft' } },
        { path: 'deck', name: 'limited-deck', component: LimitedDeckPage, meta: { limitedPage: 'deck' } },
        { path: 'matches', name: 'limited-matches', component: LimitedMatchesPage, meta: { limitedPage: 'matches' } },
        // One of the run's matches sits under Matches in the sidebar.
        {
          path: 'matches/:token',
          name: 'limited-match',
          component: LimitedMatchPage,
          meta: { limitedPage: 'matches' },
        },
        { path: 'cards', name: 'limited-cards', component: LimitedCardsPage, meta: { limitedPage: 'cards' } },
      ],
    },
    { path: '/opponents', name: 'opponents', component: OpponentsPage },
    { path: '/metagame/manage', name: 'metagame-manage', component: ManagePage },
    { path: '/metagame/archetypes/new', name: 'archetype-create', component: ArchetypeFormPage },
    { path: '/metagame/archetypes/:id/edit', name: 'archetype-edit', component: ArchetypeFormPage },
    // One archetype on a tab (issue #326): Decklist when the tab is left out.
    {
      path: '/metagame/archetypes/:id(\\d+)/:tab(decklist|matches|sideboard-guides)?',
      name: 'archetype',
      component: ArchetypePage,
    },
    // The Metagame page (archetypes spec section 9); `format` is an archetypes
    // key, which may hold an underscore (Arena's `historic_brawl`).
    { path: '/metagame/:format([a-z_]+)?', name: 'metagame', component: MetagamePage },
    { path: '/import', name: 'import', component: ImportPage },
    // What's new (release spec section 7): opened once after a minor update.
    { path: '/whats-new', name: 'whats-new', component: WhatsNewPage },
    {
      // The settings area in 0.x's layout and order (release spec section 4).
      path: '/settings',
      component: SettingsLayout,
      children: [
        { path: '', name: 'settings', redirect: { name: 'settings-general' } },
        { path: 'general', name: 'settings-general', component: GeneralPage },
        { path: 'account', name: 'settings-account', component: AccountPage },
        { path: 'overlays', name: 'settings-overlays', component: OverlaysPage },
        { path: 'storage', name: 'settings-storage', component: StoragePage },
        { path: 'archives', name: 'settings-archives', component: ArchivesPage, meta: { wide: true } },
        { path: 'replays', name: 'settings-replays', component: ReplaysPage },
        { path: 'integrations', name: 'settings-integrations', component: IntegrationsPage },
        { path: 'privacy', name: 'settings-privacy', component: PrivacyPage },
        { path: 'advanced', name: 'settings-advanced', component: AdvancedPage },
        // Loaded on demand: the generated licence list is large (release spec section 9).
        {
          path: 'advanced/licences',
          name: 'settings-licences',
          component: () => import('@/pages/settings/LicencesPage.vue'),
        },
      ],
    },
  ],
})

/**
 * The platform context for the guard: the cached one, unless an event
 * invalidated it (an account or platform change), which is read again first.
 */
function guardContext() {
  const options = { queryKey: keys.platformContext, queryFn: fetchPlatformContext, retry: false } as const
  if (queryClient.getQueryState(keys.platformContext)?.isInvalidated) {
    return queryClient.fetchQuery(options)
  }
  return queryClient.ensureQueryData(options)
}

// A page the current platform has no data for (arena spec section 15) lands
// on the dashboard. A failed context read allows the route: the page shows its own error.
router.beforeEach(async (to) => {
  try {
    const context = await guardContext()
    return routeAllowed(to.name, context.current) ? true : { name: 'dashboard' }
  } catch {
    return true
  }
})

// Anonymous usage counts (usage analytics spec): one page view per route,
// not per filter or query change on the same page.
router.afterEach((to, from, failure) => {
  if (failure || typeof to.name !== 'string' || to.name === from.name) return
  trackUsage({ kind: 'pageViewed', page: to.name })
})
