//! The platform the pages read: MTGO, Arena or Endstep (Arena spec 15). The header's
//! platform switch stores the `platform_context` setting through the
//! tracker's account editor, like the account switch.

use serde::{Deserialize, Serialize};
use store::accounts::AccountEdit;
use store::{Connection, StoreError};
use tauri::{AppHandle, Manager};

use crate::accounts::Switched;
use crate::bridge::lock;
use crate::commands::{AppState, CommandError, Startup, ready, storage};
use crate::logging::Timer;

/// The route a platform switch opens: the dashboard.
const PLATFORM_REDIRECT: &str = "/";

/// `store::Platform` as the UI sees it (`"mtgo"`, `"arena"` or `"endstep"`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "lowercase")]
pub enum PlatformView {
    Mtgo,
    Arena,
    Endstep,
}

impl From<store::Platform> for PlatformView {
    fn from(platform: store::Platform) -> PlatformView {
        match platform {
            store::Platform::Mtgo => PlatformView::Mtgo,
            store::Platform::Arena => PlatformView::Arena,
            store::Platform::Endstep => PlatformView::Endstep,
        }
    }
}

impl From<PlatformView> for store::Platform {
    fn from(platform: PlatformView) -> store::Platform {
        match platform {
            PlatformView::Mtgo => store::Platform::Mtgo,
            PlatformView::Arena => store::Platform::Arena,
            PlatformView::Endstep => store::Platform::Endstep,
        }
    }
}

/// `store::BestOf` as the UI sees it: an Arena match's kind (`"bo1"` or
/// `"bo3"`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub enum BestOfView {
    Bo1,
    Bo3,
}

impl From<store::BestOf> for BestOfView {
    fn from(best_of: store::BestOf) -> BestOfView {
        match best_of {
            store::BestOf::One => BestOfView::Bo1,
            store::BestOf::Three => BestOfView::Bo3,
        }
    }
}

impl From<BestOfView> for store::BestOf {
    fn from(best_of: BestOfView) -> store::BestOf {
        match best_of {
            BestOfView::Bo1 => store::BestOf::One,
            BestOfView::Bo3 => store::BestOf::Three,
        }
    }
}

/// The platform the pages read, and which platforms the header shows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct PlatformContextView {
    pub current: PlatformView,
    pub mtgo: bool,
    pub arena: bool,
    pub endstep: bool,
    /// Two or more platforms are shown, so the header shows the switch.
    pub switchable: bool,
}

/// A match in progress and its platform (`live_matches`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct LiveMatchView {
    pub token: String,
    pub platform: PlatformView,
}

/// What a page command reads: the context's platform and that platform's
/// current account (`None` when it has none).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PageScope {
    pub platform: store::Platform,
    pub account: Option<i64>,
}

/// The context, then that platform's current account.
pub(crate) fn page_scope(conn: &Connection) -> Result<PageScope, StoreError> {
    let platform = store::platform::current_platform(conn)?;
    let account = store::accounts::current_account_id(conn, platform)?;
    Ok(PageScope { platform, account })
}

/// The context the header shows. A platform is in the switch only when it
/// has data and its Integrations switch is on (MTGO always). The page
/// platform is one of those (`store::platform::current_platform`, which
/// `page_scope` reads too), so a switched-off platform is never current.
fn context_view(conn: &Connection) -> Result<PlatformContextView, StoreError> {
    let shown = store::platform::shown_platforms(conn)?;
    let current = store::platform::page_platform(store::platform::stored_platform(conn)?, shown);
    let shown_count = [shown.mtgo, shown.arena, shown.endstep]
        .into_iter()
        .filter(|on| *on)
        .count();
    Ok(PlatformContextView {
        current: current.into(),
        mtgo: shown.mtgo,
        arena: shown.arena,
        endstep: shown.endstep,
        switchable: shown_count >= 2,
    })
}

pub fn read_context(state: &AppState) -> Result<PlatformContextView, CommandError> {
    context_view(&lock(&state.reader)).map_err(storage)
}

/// Stores the platform choice. Held under the settings lock, so a settings
/// save cannot interleave with it.
pub fn switch(state: &AppState, platform: PlatformView) -> Result<Switched, CommandError> {
    let _one_save = lock(&state.settings_lock);
    let editor = state.tracker_part(tracker::TrackerHandle::account_editor)?;
    let outcome = editor
        .edit(AccountEdit::SwitchPlatform {
            platform: platform.into(),
        })
        .map_err(CommandError::Tracker)?;
    match outcome.refusal() {
        Some(message) => Err(CommandError::Refused(message.to_string())),
        None => Ok(Switched {
            redirect: PLATFORM_REDIRECT.to_string(),
        }),
    }
}

#[tauri::command]
#[specta::specta]
pub fn platform_context(
    startup: tauri::State<'_, Startup>,
) -> Result<PlatformContextView, CommandError> {
    let _timer = Timer::start("platform_context");
    read_context(ready(&startup)?)
}

pub fn read_live_matches(
    state: &AppState,
    now_ms: i64,
) -> Result<Vec<LiveMatchView>, CommandError> {
    let live = store::platform::live_matches(&lock(&state.reader), now_ms).map_err(storage)?;
    Ok(live
        .into_iter()
        .map(|(token, platform)| LiveMatchView {
            token,
            platform: platform.into(),
        })
        .collect())
}

/// Every match in progress on any platform, so the header's live dots are
/// right before the first push (pushes keep them after).
#[tauri::command]
#[specta::specta]
pub fn live_matches(
    startup: tauri::State<'_, Startup>,
) -> Result<Vec<LiveMatchView>, CommandError> {
    let _timer = Timer::start("live_matches");
    read_live_matches(ready(&startup)?, crate::wall_clock_ms())
}

/// Async, and the wait for the tracker runs on a blocking thread.
#[tauri::command]
#[specta::specta]
pub async fn switch_platform(
    app: AppHandle,
    platform: PlatformView,
) -> Result<Switched, CommandError> {
    let _timer = Timer::start("switch_platform");
    crate::commands::off_async_workers(move || {
        let startup = app.state::<Startup>();
        switch(ready(&startup)?, platform)
    })
    .await
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::commands::tests::ready_state;
    use fixture_tools::TestDir;

    /// Seeds an MTGO account and/or the Arena account straight into the
    /// database (a second connection, as other app tests do).
    pub(crate) fn seed(state: &AppState, mtgo: bool, arena: bool) {
        let conn = store::open_writer(&state.db_path).expect("writer");
        if mtgo {
            conn.execute(
                "INSERT INTO accounts (username, first_seen_at) VALUES ('Qplayer', 0)",
                [],
            )
            .expect("mtgo account");
        }
        if arena {
            conn.execute(
                "INSERT INTO accounts (username, first_seen_at, platform, arena_user_id)
                 VALUES ('Qarena#12345', 0, 'arena', 'Qarena-id')",
                [],
            )
            .expect("arena account");
        }
    }

    /// Both platforms with data: an MTGO and an Arena account, each with a
    /// deck and one complete match against its own opponent. Returns the MTGO
    /// deck id and the Arena deck id.
    pub(crate) fn seed_both_platforms(state: &AppState) -> (i64, i64) {
        seed(state, true, true);
        let conn = store::open_writer(&state.db_path).expect("writer");
        let account = |platform: &str| -> i64 {
            conn.query_row(
                "SELECT id FROM accounts WHERE platform = ?1",
                [platform],
                |row| row.get(0),
            )
            .expect("account")
        };
        let mut deck_ids = Vec::new();
        for (platform, format, deck, opponent) in [
            ("mtgo", "CMODERN", "Qmtgodeck", "Qmtgoopp"),
            ("arena", "CPIONEER", "Qarenadeck", "Qarenaopp"),
        ] {
            let account_id = account(platform);
            conn.execute(
                "INSERT INTO decks (mtgo_id, account_id, name, format, created_at, updated_at, source)
                 VALUES (?1, ?2, ?3, ?4, 0, 0, 'file')",
                store::params![format!("q-{deck}"), account_id, deck, format],
            )
            .expect("deck");
            let deck_id = conn.last_insert_rowid();
            conn.execute(
                "INSERT INTO deck_versions (deck_id, signature, modified_at) VALUES (?1, 'Q', 0)",
                [deck_id],
            )
            .expect("version");
            let version_id = conn.last_insert_rowid();
            conn.execute(
                "INSERT INTO opponents (username, first_seen_at) VALUES (?1, 0)",
                [opponent],
            )
            .expect("opponent");
            let opponent_id = conn.last_insert_rowid();
            conn.execute(
                "INSERT INTO matches (token, account_id, opponent_id, source, format, state,
                     outcome, started_at, last_activity_at, revision, derived_version,
                     deck_version_id)
                 VALUES (?1, ?2, ?3, 'log', ?4, 'complete', 'win', ?5, ?5, 1, 1, ?6)",
                store::params![
                    format!("q-match-{platform}"),
                    account_id,
                    opponent_id,
                    format,
                    NOW,
                    version_id
                ],
            )
            .expect("match");
            deck_ids.push(deck_id);
        }
        (deck_ids[0], deck_ids[1])
    }

    pub(crate) fn stop(state: &AppState) {
        lock(&state.tracker).take().expect("tracker").stop();
    }

    #[test]
    fn the_context_is_mtgo_with_no_data() {
        let dir = TestDir::new("platform-none");
        let (state, _events) = ready_state(&dir);
        assert_eq!(
            read_context(&state).expect("read"),
            PlatformContextView {
                current: PlatformView::Mtgo,
                mtgo: false,
                arena: false,
                endstep: false,
                switchable: false,
            }
        );
        stop(&state);
    }

    #[test]
    fn an_arena_account_alone_makes_the_context_arena() {
        let dir = TestDir::new("platform-arena-alone");
        let (state, _events) = ready_state(&dir);
        seed(&state, false, true);
        assert_eq!(
            read_context(&state).expect("read"),
            PlatformContextView {
                current: PlatformView::Arena,
                mtgo: false,
                arena: true,
                endstep: false,
                switchable: false,
            }
        );
        stop(&state);
    }

    #[test]
    fn switching_the_viewed_platform_off_moves_to_mtgo() {
        let dir = TestDir::new("platform-switch-off-viewed");
        let (state, _events) = ready_state(&dir);
        seed(&state, true, true);
        switch(&state, PlatformView::Arena).expect("switched");
        assert_eq!(
            read_context(&state).expect("read").current,
            PlatformView::Arena
        );

        let (view, _effect) =
            crate::integrations::apply(&state, crate::integrations::IntegrationView::Arena, false)
                .expect("switched off");

        assert!(!view.arena);
        let context = read_context(&state).expect("read");
        assert_eq!(context.current, PlatformView::Mtgo);
        assert!(!context.arena);
        assert!(!context.switchable);
        assert_eq!(
            page_scope(&lock(&state.reader)).expect("scope").platform,
            store::Platform::Mtgo
        );
        stop(&state);
    }

    /// Writes a settings row straight into the database (no `set_integration`,
    /// so nothing moves the stored platform choice).
    fn set_setting(state: &AppState, key: &str, value: &str) {
        let conn = store::open_writer(&state.db_path).expect("writer");
        conn.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT (key) DO UPDATE SET value = excluded.value",
            [key, value],
        )
        .expect("setting");
    }

    #[test]
    fn the_only_platform_with_data_switched_off_is_never_current() {
        let dir = TestDir::new("platform-only-data-off");
        let (state, _events) = ready_state(&dir);
        seed(&state, false, true);
        set_setting(&state, crate::settings::ARENA_ENABLED, "false");
        let context = read_context(&state).expect("read");
        assert_eq!(context.current, PlatformView::Mtgo);
        assert!(!context.arena);
        assert_eq!(
            page_scope(&lock(&state.reader)).expect("scope").platform,
            store::Platform::Mtgo,
            "the pages agree with the header"
        );
        stop(&state);
    }

    #[test]
    fn endstep_is_in_the_context_only_with_data_and_switched_on() {
        let dir = TestDir::new("platform-endstep");
        let (state, _events) = ready_state(&dir);
        store::open_writer(&state.db_path)
            .expect("writer")
            .execute(
                "INSERT INTO accounts (username, first_seen_at, platform, display_name)
                 VALUES ('endstep:1', 0, 'endstep', 'Player')",
                [],
            )
            .expect("endstep account");
        let off = read_context(&state).expect("read");
        assert_eq!(off.current, PlatformView::Mtgo);
        assert!(!off.endstep);
        assert_eq!(
            page_scope(&lock(&state.reader)).expect("scope").platform,
            store::Platform::Mtgo
        );

        set_setting(&state, crate::settings::ENDSTEP_ENABLED, "true");
        assert_eq!(
            read_context(&state).expect("read"),
            PlatformContextView {
                current: PlatformView::Endstep,
                mtgo: false,
                arena: false,
                endstep: true,
                switchable: false,
            }
        );
        let scope = page_scope(&lock(&state.reader)).expect("scope");
        assert_eq!(scope.platform, store::Platform::Endstep);
        assert!(scope.account.is_some(), "its only account is current");

        seed(&state, true, false);
        let both = read_context(&state).expect("read");
        assert!(both.switchable, "MTGO and Endstep both shown");
        assert_eq!(both.current, PlatformView::Mtgo, "no choice stored yet");
        switch(&state, PlatformView::Endstep).expect("switched");
        assert_eq!(
            read_context(&state).expect("read").current,
            PlatformView::Endstep
        );
        stop(&state);
    }

    #[test]
    fn live_matches_name_each_match_in_progress_and_its_platform() {
        let dir = TestDir::new("platform-live");
        let (state, _events) = ready_state(&dir);
        seed(&state, false, true);
        assert_eq!(read_live_matches(&state, 1).expect("read"), Vec::new());
        store::open_writer(&state.db_path)
            .expect("writer")
            .execute(
                "INSERT INTO matches (token, account_id, source, state, outcome, started_at,
                     last_activity_at, revision, derived_version)
                 VALUES ('q-live', (SELECT id FROM accounts WHERE platform = 'arena'), 'log',
                     'in_progress', 'unknown', 1, 1, 1, 1)",
                [],
            )
            .expect("a live Arena match");
        assert_eq!(
            read_live_matches(&state, 1).expect("read"),
            vec![LiveMatchView {
                token: "q-live".to_string(),
                platform: PlatformView::Arena,
            }]
        );
        stop(&state);
    }

    #[test]
    fn switch_platform_stores_the_choice_and_redirects_home() {
        let dir = TestDir::new("platform-switch");
        let (state, _events) = ready_state(&dir);
        seed(&state, true, true);
        let before = read_context(&state).expect("read");
        assert_eq!(before.current, PlatformView::Mtgo);
        assert!(before.switchable);
        let switched = switch(&state, PlatformView::Arena).expect("switched");
        assert_eq!(switched.redirect, "/");
        assert_eq!(
            read_context(&state).expect("read").current,
            PlatformView::Arena
        );
        stop(&state);
    }

    #[test]
    fn page_scope_pairs_the_context_with_its_account() {
        let dir = TestDir::new("platform-scope");
        let (state, _events) = ready_state(&dir);
        seed(&state, true, true);
        let mtgo_id: i64 = lock(&state.reader)
            .query_row(
                "SELECT id FROM accounts WHERE platform = 'mtgo'",
                [],
                |row| row.get(0),
            )
            .expect("mtgo id");
        let arena_id: i64 = lock(&state.reader)
            .query_row(
                "SELECT id FROM accounts WHERE platform = 'arena'",
                [],
                |row| row.get(0),
            )
            .expect("arena id");
        let scope = page_scope(&lock(&state.reader)).expect("scope");
        assert_eq!(scope.platform, store::Platform::Mtgo);
        assert_eq!(scope.account, Some(mtgo_id));
        switch(&state, PlatformView::Arena).expect("switched");
        let scope = page_scope(&lock(&state.reader)).expect("scope");
        assert_eq!(scope.platform, store::Platform::Arena);
        assert_eq!(scope.account, Some(arena_id));
        stop(&state);
    }

    const NOW: i64 = 1_700_000_000_000;

    fn deck_query() -> crate::decks::DeckListQuery {
        crate::decks::DeckListQuery {
            format: None,
            recent: false,
            search: None,
            hide_archived: false,
            sort: crate::decks::DeckSortKey::Name,
            page: 1,
            per_page: 12,
            archetype: None,
        }
    }

    /// What the four page reads show: (match tokens, deck ids, opponents,
    /// the dashboard's recent match tokens).
    fn page_reads(state: &AppState) -> (Vec<String>, Vec<i64>, Vec<String>, Vec<String>) {
        let matches =
            crate::commands::matches_page(state, &crate::matches::MatchListQuery::newest_first(1))
                .expect("matches");
        let tokens = matches.rows.into_iter().map(|row| row.token).collect();
        let conn = lock(&state.reader);
        let decks = crate::decks::list(&conn, &deck_query()).expect("decks");
        let deck_ids = decks.decks.into_iter().map(|deck| deck.id).collect();
        let opponents = crate::opponents::list(
            &conn,
            None,
            crate::opponents::OpponentSortKey::MostPlayed,
            None,
            1,
        )
        .expect("opponents");
        let names = opponents.rows.into_iter().map(|row| row.username).collect();
        let home = crate::dashboard::home_dashboard_view(&conn, "all", "", NOW, chrono_tz::UTC)
            .expect("home");
        let recent = home
            .recent_matches
            .unwrap_or_default()
            .into_iter()
            .map(|recent| recent.summary.token)
            .collect();
        (tokens, deck_ids, names, recent)
    }

    #[test]
    fn arena_context_pages_read_arena_rows() {
        let dir = TestDir::new("platform-pages-arena");
        let (state, _events) = ready_state(&dir);
        let (mtgo_deck, arena_deck) = seed_both_platforms(&state);
        switch(&state, PlatformView::Arena).expect("switched");
        let (tokens, decks, opponents, recent) = page_reads(&state);
        assert_eq!(tokens, ["q-match-arena"]);
        assert_eq!(decks, [arena_deck]);
        assert!(!decks.contains(&mtgo_deck));
        assert_eq!(opponents, ["Qarenaopp"]);
        assert_eq!(recent, ["q-match-arena"]);
        stop(&state);
    }

    #[test]
    fn mtgo_context_is_unchanged() {
        let dir = TestDir::new("platform-pages-mtgo");
        let (state, _events) = ready_state(&dir);
        let (mtgo_deck, _arena_deck) = seed_both_platforms(&state);
        let (tokens, decks, opponents, recent) = page_reads(&state);
        assert_eq!(tokens, ["q-match-mtgo"]);
        assert_eq!(decks, [mtgo_deck]);
        assert_eq!(opponents, ["Qmtgoopp"]);
        assert_eq!(recent, ["q-match-mtgo"]);
        stop(&state);
    }

    #[test]
    fn the_arena_dashboard_has_no_league_cards() {
        let dir = TestDir::new("platform-dashboard-arena");
        let (state, _events) = ready_state(&dir);
        seed_both_platforms(&state);
        switch(&state, PlatformView::Arena).expect("switched");
        let home = crate::dashboard::home_dashboard_view(
            &lock(&state.reader),
            "all",
            "",
            NOW,
            chrono_tz::UTC,
        )
        .expect("home");
        assert!(home.league.is_none());
        assert!(home.league_results.is_none());
        stop(&state);
    }

    #[test]
    fn the_deck_sync_list_is_every_platform_in_both_contexts() {
        let dir = TestDir::new("platform-deck-sync");
        let (state, _events) = ready_state(&dir);
        let (mtgo_deck, arena_deck) = seed_both_platforms(&state);
        let ids = |state: &AppState| -> Vec<i64> {
            let mut ids: Vec<i64> = crate::decks::deck_sync_rows(&lock(&state.reader))
                .expect("rows")
                .into_iter()
                .map(|row| row.id)
                .collect();
            ids.sort();
            ids
        };
        let mut both = vec![mtgo_deck, arena_deck];
        both.sort();
        assert_eq!(ids(&state), both);
        switch(&state, PlatformView::Arena).expect("switched");
        assert_eq!(ids(&state), both);
        stop(&state);
    }
}
