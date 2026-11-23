use std::time::Instant;

use fixture_tools::TestDir;
use overlay::{LiveMatch, OverlayArtwork, PublishError};
use store::params;

use super::*;
use crate::commands::tests::ready_state;
use crate::commands::{AppState, apply_settings, read_settings, stop_tracker};
use crate::overlay::windows::testing::FakeWindows;
use crate::overlay::worker::{OverlayMsg, Worker};
use crate::overlay::{DRAFT_NOTES, GAME_OVERLAY};
use crate::settings::{
    overlay_background_file_row, overlay_last_published_at_row, overlay_publish_error_row,
};

/// 2026-07-01T10:00:00Z.
const NOW: i64 = 1_782_900_000_000;
const DAY: i64 = 24 * 3_600_000;
const KARN: &str = "https://cards.scryfall.io/art_crop/karn.jpg";

// ---- The page's read (0.x OverlaysControllerTest, OverlayPublishControllerTest) ----

/// A database with the streamer `Qstream` (login id 4242) logged in and a
/// deck with a Scryfall cover art crop, played yesterday.
fn world(label: &str) -> (TestDir, Connection) {
    let dir = TestDir::new(label);
    let conn = store::open_writer(&dir.join("mymtgo.sqlite")).expect("open");
    conn.execute_batch(
        "INSERT INTO accounts (id, username, login_id, first_seen_at)
             VALUES (1, 'Qstream', 4242, 0);
         INSERT INTO cards (catalog_id, oracle_id, name, type, art_crop)
             VALUES (5001, 'oracle-karn', 'Qkarn', 'Planeswalker',
                 'https://cards.scryfall.io/art_crop/karn.jpg');
         INSERT INTO decks (id, mtgo_id, name, format, color_identity, cover_catalog_id,
                 created_at, updated_at, source)
             VALUES (1, 'q-deck-1', 'Mono Green Tron', 'CMODERN', 'G', 5001, 1, 1, 'file');
         INSERT INTO deck_versions (id, deck_id, signature, modified_at)
             VALUES (1, 1, 'SIG', 1);
         INSERT INTO log_files (id, path, identity_hash, head_hash, head_len, day_anchor,
                 timezone, clock_date, first_seen_at, last_seen_at)
             VALUES (1, 'Q/1.log', 'id', 'head', 0, '2026-07-01', 'UTC', '2026-07-01', 1, 1);",
    )
    .expect("world");
    conn.execute(
        "INSERT INTO matches (id, token, account_id, source, format, state, outcome,
             started_at, last_activity_at, revision, derived_version, deck_version_id)
         VALUES (1, 'q-match-1', 1, 'log', 'CMODERN', 'complete', 'win', ?1, ?1, 1, 1, 1)",
        [NOW - DAY],
    )
    .expect("match");
    let line = "12:00:00 [INF] (Login|MtGO Login Success) Username: Qstream (4242)";
    conn.execute(
        "INSERT INTO log_events (log_file_id, byte_offset, kind, logged_at, raw)
         VALUES (1, 0, 'login', ?1, ?2)",
        params![NOW - DAY, line.as_bytes().to_vec()],
    )
    .expect("login");
    (dir, conn)
}

fn save_rows(conn: &Connection, rows: Vec<(String, String)>) {
    for (key, value) in rows {
        conn.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT (key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )
        .expect("setting");
    }
}

fn context() -> PageContext {
    PageContext {
        api_base: Some("https://mymtgo.com".to_string()),
        signed_in: true,
        overlay_windows_available: true,
    }
}

#[test]
fn renders_the_overlays_settings_page_with_every_overlay_prop() {
    let dir = TestDir::new("overlay-page-empty");
    let conn = store::open_writer(&dir.join("mymtgo.sqlite")).expect("open");
    let view = settings_view(&conn, Some(&dir.join("overlay")), &context(), NOW).expect("view");
    assert_eq!(view.background_url, None);
    assert_eq!(view.deck, None, "no deck played yet");
    assert_eq!(view.deck_art_url, None);
    assert_eq!(view.artwork, OverlayArtwork::Deck);
    assert_eq!(view.public_url, None, "no MTGO login yet");
    assert_eq!(
        view.publish,
        OverlayPublishChanged {
            last_published_at_ms: None,
            error: None
        }
    );
    assert!(view.signed_in);
    assert!(view.overlay_windows_available);
    // The window settings are the typed settings' (0.x props).
    let settings = Settings::from_rows(&store::read::settings(&conn).expect("rows"));
    assert!(settings.draft_notes_window);
    assert!(!settings.game_overlay);
    assert!(!settings.league_window);
    assert!(settings.overlay_show_opponent && settings.overlay_show_sideboard);
}

#[test]
fn resolves_the_overlay_background_url_when_the_file_exists() {
    let (dir, conn) = world("overlay-page-background");
    let folder = dir.join("overlay");
    save_rows(
        &conn,
        vec![overlay_background_file_row(Some("background-1700.png"))],
    );
    let missing = settings_view(&conn, Some(&folder), &context(), NOW).expect("view");
    assert_eq!(missing.background_url, None, "the file is not there");
    assert_eq!(missing.artwork, OverlayArtwork::Deck);

    std::fs::create_dir_all(&folder).expect("folder");
    std::fs::write(folder.join("background-1700.png"), b"img").expect("file");
    let view = settings_view(&conn, Some(&folder), &context(), NOW).expect("view");
    assert_eq!(
        view.background_url,
        Some(crate::overlay::background::background_url(
            "background-1700.png"
        ))
    );
    assert_eq!(
        view.artwork,
        OverlayArtwork::Custom,
        "an existing background is the read-time default"
    );
    assert_eq!(
        view.deck_art_url.as_deref(),
        Some(KARN),
        "the deck art stays"
    );
}

#[test]
fn shares_the_publish_props_with_the_overlays_page() {
    let (dir, conn) = world("overlay-page-publish");
    save_rows(
        &conn,
        vec![
            overlay_last_published_at_row(Some(1_790_000_000_000)),
            overlay_publish_error_row(Some(PublishError::NotClaimed)),
        ],
    );
    let view = settings_view(&conn, Some(&dir.join("overlay")), &context(), NOW).expect("view");
    assert_eq!(
        view.public_url.as_deref(),
        Some("https://mymtgo.com/overlays/Qstream")
    );
    assert_eq!(
        view.publish,
        OverlayPublishChanged {
            last_published_at_ms: Some(1_790_000_000_000),
            error: Some(PublishError::NotClaimed),
        }
    );
    assert!(view.signed_in);
    let deck = view.deck.expect("the last played deck");
    assert_eq!(deck.id, 1);
    assert_eq!(deck.name, "Mono Green Tron");
    assert_eq!(deck.color_identity, vec!["G".to_string()]);
    assert_eq!(view.deck_art_url.as_deref(), Some(KARN));
}

#[test]
fn shares_the_obs_browser_source_size_for_each_card_size() {
    let (dir, conn) = world("overlay-page-obs-size");
    let view = settings_view(&conn, Some(&dir.join("overlay")), &context(), NOW).expect("view");
    // The strip switch is on by default: card + strip + 16 px on every
    // side (event overlay spec 8.2).
    assert_eq!(
        view.obs_sizes,
        ObsSizes {
            full: ObsSize {
                width: 332,
                height: 128
            },
            compact: ObsSize {
                width: 272,
                height: 106
            },
        }
    );
    // Switched off: the window sizes at the branch tip (owner, spec ruling 1).
    save_rows(
        &conn,
        vec![("overlay_event_strip".to_string(), "false".to_string())],
    );
    let view = settings_view(&conn, Some(&dir.join("overlay")), &context(), NOW).expect("view");
    assert_eq!(
        view.obs_sizes,
        ObsSizes {
            full: ObsSize {
                width: 332,
                height: 112
            },
            compact: ObsSize {
                width: 272,
                height: 90
            },
        }
    );
}

#[test]
fn shares_the_obs_browser_source_size_of_the_current_theme() {
    let (dir, conn) = world("overlay-page-obs-size-versus");
    save_rows(
        &conn,
        vec![("overlay_league_theme".to_string(), "\"versus\"".to_string())],
    );
    let view = settings_view(&conn, Some(&dir.join("overlay")), &context(), NOW).expect("view");
    assert_eq!(
        view.obs_sizes,
        ObsSizes {
            full: ObsSize {
                width: 612,
                height: 156
            },
            compact: ObsSize {
                width: 412,
                height: 112
            },
        }
    );
}

#[test]
fn the_page_says_when_signed_out_or_on_native_wayland() {
    let (dir, conn) = world("overlay-page-signed-out");
    let view = settings_view(
        &conn,
        Some(&dir.join("overlay")),
        &PageContext {
            api_base: None,
            signed_in: false,
            overlay_windows_available: false,
        },
        NOW,
    )
    .expect("view");
    assert!(!view.signed_in);
    assert!(!view.overlay_windows_available);
    assert_eq!(view.public_url, None, "no API base, no URL");
}

#[test]
fn the_public_url_encodes_the_username_as_php() {
    let (dir, conn) = world("overlay-page-encoded");
    conn.execute(
        "UPDATE log_events SET raw = ?1",
        [b"12:00:00 [INF] (Login|MtGO Login Success) Username: Q b+c (4242)".to_vec()],
    )
    .expect("login");
    let view = settings_view(
        &conn,
        Some(&dir.join("overlay")),
        &PageContext {
            api_base: Some("https://mymtgo.test/".to_string()),
            ..context()
        },
        NOW,
    )
    .expect("view");
    assert_eq!(
        view.public_url.as_deref(),
        Some("https://mymtgo.test/overlays/Q%20b%2Bc")
    );
}

// ---- Save side effects (0.x UpdateOverlaySettingsControllerTest) ----
//
// `save_settings` stores the settings through the tracker, then sends the
// worker `SettingsChanged` (ruling C7); these run both halves on a real
// app state, with the fake window manager.

struct App {
    _dir: TestDir,
    /// The bridge's end: the tracker stops when nobody listens.
    _events: std::sync::mpsc::Receiver<tracker::TrackerEvent>,
    state: AppState,
    worker: Worker,
    windows: FakeWindows,
}

impl App {
    /// A running app whose stored settings are `stored`, the worker after
    /// its start-up update.
    fn new(label: &str, stored: impl FnOnce(&mut Settings)) -> App {
        let dir = TestDir::new(label);
        let (state, events) = ready_state(&dir);
        let mut settings = read_settings(&state).expect("settings");
        stored(&mut settings);
        apply_settings(&state, settings.clone()).expect("stored");
        let writer = store::open_writer(&state.db_path).expect("writer");
        writer
            .execute(
                "INSERT INTO accounts (id, username, first_seen_at) VALUES (1, 'Qlocal', 0)",
                [],
            )
            .expect("account");
        let mut app = App {
            _dir: dir,
            _events: events,
            worker: Worker::new(read_settings(&state).expect("settings"), false, true),
            state,
            windows: FakeWindows::default(),
        };
        app.wake(Vec::new());
        app
    }

    fn wake(&mut self, messages: Vec<OverlayMsg>) {
        let reader = lock(&self.state.reader);
        for message in messages {
            self.worker.handle(&reader, message);
        }
        self.worker.update(&reader, &mut self.windows, NOW);
    }

    /// The settings page's save: the stored settings with `change`.
    fn save(&mut self, change: impl FnOnce(&mut Settings)) {
        let mut next = read_settings(&self.state).expect("settings");
        change(&mut next);
        let applied = apply_settings(&self.state, next).expect("saved");
        self.wake(vec![OverlayMsg::SettingsChanged {
            settings: Box::new(applied.settings),
        }]);
    }

    fn stored(&self) -> Settings {
        read_settings(&self.state).expect("settings")
    }

    /// A constructed match in progress whose frames the tracker announced.
    fn match_in_progress(&mut self) {
        let live = LiveMatch {
            token: "q-settings-overlay".to_string(),
            source: "log".to_string(),
            state: "in_progress".to_string(),
            game_count: 1,
            format: Some("CMODERN".to_string()),
            hidden: false,
            needs_attention: false,
            started_at: NOW - 60_000,
        };
        self.wake(vec![
            OverlayMsg::MatchUpdated {
                live: Box::new(live),
                league: false,
            },
            OverlayMsg::LiveEventsStored {
                tokens: vec!["q-settings-overlay".to_string()],
                detected_at: Instant::now(),
            },
        ]);
    }

    /// The local account's draft, picking, then the tracker's
    /// `LimitedChanged`. The draft is dated from the wall clock, as the
    /// running tracker's league sync is: dated from `NOW` it is long past
    /// its `abandons_at`, and the tracker abandons it (#824). The write takes
    /// the write lock at once, so it waits for the tracker's writes instead
    /// of failing with "database is locked" (#679).
    fn draft_picking(&mut self) {
        let now = crate::wall_clock_ms();
        let mut writer = store::open_writer(&self.state.db_path).expect("writer");
        let tx = writer
            .transaction_with_behavior(store::TransactionBehavior::Immediate)
            .expect("transaction");
        tx.execute(
            "INSERT INTO drafts (id, draft_token, account_id, source, seat_count, seat_index,
                 state, pack_size, picks_expected, started_at, imported_from)
             VALUES (1, 'q-draft-1', 1, 'imported', 8, 5, 'picking', 14, 42, ?1, 1)",
            [now - 60_000],
        )
        .expect("draft");
        store::league_sync::sync_every_owner(&tx, chrono_tz::UTC, now).expect("league sync");
        tx.commit().expect("commit");
        drop(writer);
        self.wake(vec![OverlayMsg::LimitedChanged]);
    }

    fn opened(&self, label: &str) -> usize {
        self.windows
            .opened_labels()
            .iter()
            .filter(|opened| *opened == label)
            .count()
    }

    fn closed(&self, label: &str) -> usize {
        self.windows
            .closed()
            .iter()
            .filter(|closed| *closed == label)
            .count()
    }
}

impl Drop for App {
    fn drop(&mut self) {
        stop_tracker(&self.state);
    }
}

#[test]
fn persists_league_window_setting() {
    let mut app = App::new("overlay-save-league", |_| {});
    app.save(|settings| settings.league_window = true);
    assert!(app.stored().league_window);
}

#[test]
fn validates_league_window_is_boolean() {
    let mut json = serde_json::to_value(Settings::default()).expect("json");
    json["leagueWindow"] = serde_json::json!("not-a-bool");
    assert!(serde_json::from_value::<Settings>(json).is_err());
}

#[test]
fn opens_the_game_overlay_window_when_enabled() {
    let mut app = App::new("overlay-save-game-on", |_| {});
    app.save(|settings| settings.game_overlay = true);
    assert!(app.stored().game_overlay);
}

#[test]
fn closes_the_game_overlay_window_when_disabled() {
    let mut app = App::new("overlay-save-game-off", |settings| {
        settings.game_overlay = true
    });
    app.save(|settings| settings.game_overlay = false);
    assert!(!app.stored().game_overlay);
}

#[test]
fn persists_section_toggles_without_touching_the_master() {
    let mut app = App::new("overlay-save-section", |settings| {
        settings.game_overlay = true
    });
    app.match_in_progress();
    assert_eq!(app.opened(GAME_OVERLAY), 1);
    app.save(|settings| settings.overlay_show_sideboard = false);
    let stored = app.stored();
    assert!(!stored.overlay_show_sideboard);
    assert!(stored.game_overlay);
    assert_eq!(app.closed(GAME_OVERLAY), 0, "a section never closes it");
    assert_eq!(app.opened(GAME_OVERLAY), 1, "nor reopens it");
}

#[test]
fn does_not_open_the_game_overlay_when_enabled_with_no_match_in_progress() {
    let mut app = App::new("overlay-save-game-no-match", |_| {});
    app.save(|settings| settings.game_overlay = true);
    assert!(app.stored().game_overlay);
    assert!(app.windows.opened().is_empty());
}

#[test]
fn opens_the_game_overlay_when_enabled_during_an_in_progress_match() {
    let mut app = App::new("overlay-save-game-match", |_| {});
    app.match_in_progress();
    assert_eq!(app.opened(GAME_OVERLAY), 0);
    app.save(|settings| settings.game_overlay = true);
    assert_eq!(app.opened(GAME_OVERLAY), 1);
}

#[test]
fn closes_an_open_game_overlay_when_disabled_mid_match() {
    let mut app = App::new("overlay-save-game-mid-match", |settings| {
        settings.game_overlay = true
    });
    app.match_in_progress();
    assert_eq!(app.opened(GAME_OVERLAY), 1);
    app.save(|settings| settings.game_overlay = false);
    assert!(!app.stored().game_overlay);
    assert_eq!(app.closed(GAME_OVERLAY), 1);
}

#[test]
fn persists_the_reveals_section_toggle() {
    let mut app = App::new("overlay-save-reveals", |_| {});
    app.save(|settings| settings.overlay_show_reveals = false);
    assert!(!app.stored().overlay_show_reveals);
}

#[test]
fn persists_the_draft_notes_window_setting() {
    let mut app = App::new("overlay-save-draft", |_| {});
    app.save(|settings| settings.draft_notes_window = false);
    assert!(!app.stored().draft_notes_window);
    app.save(|settings| settings.draft_notes_window = true);
    assert!(app.stored().draft_notes_window);
}

#[test]
fn validates_the_draft_notes_window_setting_is_boolean() {
    let mut json = serde_json::to_value(Settings::default()).expect("json");
    json["draftNotesWindow"] = serde_json::json!("nope");
    assert!(serde_json::from_value::<Settings>(json).is_err());
}

#[test]
fn leaves_the_draft_notes_window_setting_alone_when_the_key_is_absent() {
    let mut app = App::new("overlay-save-draft-absent", |settings| {
        settings.draft_notes_window = false
    });
    app.save(|settings| settings.game_overlay = true);
    assert!(!app.stored().draft_notes_window);
}

#[test]
fn opens_the_draft_notes_window_when_enabled_during_a_live_draft() {
    let mut app = App::new("overlay-save-draft-live", |settings| {
        settings.draft_notes_window = false
    });
    app.draft_picking();
    assert_eq!(app.opened(DRAFT_NOTES), 0);
    app.save(|settings| settings.draft_notes_window = true);
    assert_eq!(app.opened(DRAFT_NOTES), 1);
}

#[test]
fn closes_an_open_draft_notes_window_when_disabled_mid_draft() {
    let mut app = App::new("overlay-save-draft-mid", |_| {});
    app.draft_picking();
    assert_eq!(app.opened(DRAFT_NOTES), 1);
    app.save(|settings| settings.draft_notes_window = false);
    assert_eq!(app.closed(DRAFT_NOTES), 1);
}

#[test]
fn does_not_open_the_draft_notes_window_when_enabled_with_no_live_draft() {
    let mut app = App::new("overlay-save-draft-none", |settings| {
        settings.draft_notes_window = false
    });
    app.save(|settings| settings.draft_notes_window = true);
    assert!(app.windows.opened().is_empty());
}

// ---- The game overlay's pick and notes (0.x UpdateOpponentArchetypeControllerTest,
// OverlayNotesTest) ----

mod game {
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::overlay::game::LiveContext;
    use crate::overlay::game::tests::{
        LIVE, a_match, archetype, live_match, note, seed, settings as game_settings,
    };
    use crate::overlay::worker::GameSlot;

    /// The app with the tracker, the live Modern match against Qrival
    /// (deck 1) and two held archetypes, Esper Blink (101) and Ruby Storm
    /// (102).
    /// The tracker's events are kept (the tracker stops once nobody
    /// listens).
    type Events = std::sync::mpsc::Receiver<tracker::TrackerEvent>;

    fn world(label: &str) -> (TestDir, AppState, Connection, Events) {
        let dir = TestDir::new(label);
        let (state, events) = ready_state(&dir);
        let writer = store::open_writer(&state.db_path).expect("writer");
        seed(&writer);
        live_match(&writer);
        archetype(&writer, 101, "q-blink", "Esper Blink", "downloaded");
        archetype(&writer, 102, "q-storm", "Ruby Storm", "downloaded");
        (dir, state, writer, events)
    }

    fn live() -> LiveContext {
        LiveContext {
            match_id: 1,
            format: Some("CModern".to_string()),
            opponent_id: Some(1),
            deck_id: Some(1),
            archetype_id: None,
        }
    }

    fn seats(conn: &Connection) -> Vec<(i64, i64, f64, bool)> {
        let mut stmt = conn
            .prepare(
                "SELECT match_id, archetype_id, confidence, manual FROM match_archetypes
                 WHERE is_opponent = 1 ORDER BY match_id",
            )
            .expect("prepare");
        stmt.query_map([], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })
        .expect("query")
        .map(|row| row.expect("row"))
        .collect()
    }

    fn held(id: u32) -> OverlayArchetypeChoice {
        OverlayArchetypeChoice::Held { id }
    }

    #[test]
    fn stores_a_manual_archetype_for_the_live_match_opponent() {
        let (_dir, state, writer, _events) = world("overlay-pick");
        assert_eq!(
            pick_archetype(&state, Some(&live()), held(101), NOW),
            Ok(false)
        );
        assert_eq!(seats(&writer), vec![(1, 101, 1.0, true)]);
        stop_tracker(&state);
    }

    #[test]
    fn is_idempotent_across_repeat_picks() {
        let (_dir, state, writer, _events) = world("overlay-pick-twice");
        pick_archetype(&state, Some(&live()), held(101), NOW).expect("pick");
        pick_archetype(&state, Some(&live()), held(101), NOW).expect("pick");
        assert_eq!(seats(&writer).len(), 1);
        stop_tracker(&state);
    }

    #[test]
    fn stores_the_merged_into_archetype_when_the_pick_has_been_merged_away() {
        let (_dir, state, writer, _events) = world("overlay-pick-merged");
        writer
            .execute(
                "UPDATE archetypes SET merged_into_id = 101 WHERE id = 102",
                [],
            )
            .expect("merge");
        pick_archetype(&state, Some(&live()), held(102), NOW).expect("pick");
        assert_eq!(seats(&writer), vec![(1, 101, 1.0, true)]);
        stop_tracker(&state);
    }

    #[test]
    fn rejects_an_unknown_archetype() {
        let (_dir, state, writer, _events) = world("overlay-pick-unknown");
        assert_eq!(
            pick_archetype(&state, Some(&live()), held(999_999), NOW),
            Err(CommandError::Refused(
                "The selected archetype id is invalid.".to_string()
            ))
        );
        assert!(seats(&writer).is_empty());
        stop_tracker(&state);
    }

    #[test]
    fn does_nothing_when_no_match_is_live() {
        let (_dir, state, writer, _events) = world("overlay-pick-no-match");
        assert_eq!(pick_archetype(&state, None, held(101), NOW), Ok(false));
        let no_opponent = LiveContext {
            opponent_id: None,
            ..live()
        };
        assert_eq!(
            pick_archetype(&state, Some(&no_opponent), held(101), NOW),
            Ok(false)
        );
        assert!(seats(&writer).is_empty());
        stop_tracker(&state);
    }

    /// Review Focus 3: the pick lands on the match the overlay shows (the
    /// newest live one the worker computed), whatever else is in progress;
    /// the request names only the archetype.
    #[test]
    fn the_pick_targets_the_live_match_not_the_request() {
        let (_dir, state, writer, _events) = world("overlay-pick-live");
        // An older match left in progress by another client.
        a_match(
            &writer,
            2,
            "q-older",
            Some(2),
            "CModern",
            "in_progress",
            "unknown",
            NOW - 600_000,
            Some(1),
        );
        let reader = store::open_reader(&state.db_path).expect("reader");
        let slot = Arc::new(Mutex::new(GameSlot::default()));
        let mut worker =
            Worker::new(game_settings(), false, true).with_game_slot(Arc::clone(&slot));
        let mut windows = FakeWindows::default();
        let live_match = |token: &str, started_at: i64| OverlayMsg::MatchUpdated {
            live: Box::new(LiveMatch {
                token: token.to_string(),
                source: "log".to_string(),
                state: "in_progress".to_string(),
                game_count: 1,
                format: Some("CModern".to_string()),
                hidden: false,
                needs_attention: false,
                started_at,
            }),
            league: false,
        };
        for message in [
            live_match("q-older", NOW - 600_000),
            live_match(LIVE, NOW - 60_000),
            OverlayMsg::LiveEventsStored {
                tokens: vec!["q-older".to_string(), LIVE.to_string()],
                detected_at: Instant::now(),
            },
        ] {
            worker.handle(&reader, message);
        }
        worker.update(&reader, &mut windows, NOW);
        let shared = crate::bridge::lock(&slot).live.clone();
        assert_eq!(shared.as_ref().map(|live| live.match_id), Some(1));
        pick_archetype(&state, shared.as_ref(), held(102), NOW).expect("pick");
        assert_eq!(seats(&writer), vec![(1, 102, 1.0, true)]);
        stop_tracker(&state);
    }

    /// Owner answer 3: an archetype of the format's metagame v1 has never
    /// met can be picked; it becomes a stub the archetypes worker fills.
    #[test]
    fn an_unmet_archetype_can_be_picked_and_becomes_a_stub() {
        let (_dir, state, writer, _events) = world("overlay-pick-unmet");
        let choice = OverlayArchetypeChoice::Unheld {
            uuid: "q-unmet".to_string(),
            name: "Qunmet Combo".to_string(),
            colors: Some("UR".to_string()),
        };
        assert_eq!(
            pick_archetype(&state, Some(&live()), choice.clone(), NOW),
            Ok(true)
        );
        let (id, name, source, colors, defined): (i64, String, String, Option<String>, bool) =
            writer
                .query_row(
                    "SELECT id, name, source, color_identity, definition IS NOT NULL
                 FROM archetypes WHERE uuid = 'q-unmet'",
                    [],
                    |row| {
                        Ok((
                            row.get(0)?,
                            row.get(1)?,
                            row.get(2)?,
                            row.get(3)?,
                            row.get(4)?,
                        ))
                    },
                )
                .expect("stub");
        assert_eq!(
            (name.as_str(), source.as_str()),
            ("Qunmet Combo", "downloaded")
        );
        assert_eq!(colors.as_deref(), Some("U,R"));
        assert!(!defined);
        assert!(
            store::archetypes::undefined_uuids(&writer)
                .expect("read")
                .contains(&"q-unmet".to_string()),
            "waiting for the archetypes worker's definition download"
        );
        assert_eq!(seats(&writer), vec![(1, id, 1.0, true)]);
        // Picked again, the held stub is used: no second stub.
        assert_eq!(
            pick_archetype(&state, Some(&live()), choice, NOW),
            Ok(false)
        );
        stop_tracker(&state);
    }

    // ---- 0.x OverlayNotesTest ----

    fn with_archetype() -> LiveContext {
        LiveContext {
            archetype_id: Some(101),
            ..live()
        }
    }

    fn notes(conn: &Connection) -> Vec<(i64, i64, String)> {
        let mut stmt = conn
            .prepare("SELECT deck_id, archetype_id, body FROM deck_archetype_notes ORDER BY id")
            .expect("prepare");
        stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .expect("query")
            .map(|row| row.expect("row"))
            .collect()
    }

    fn guides(conn: &Connection) -> Vec<(i64, i64)> {
        let mut stmt = conn
            .prepare("SELECT deck_id, archetype_id FROM sideboard_guides ORDER BY id")
            .expect("prepare");
        stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .expect("query")
            .map(|row| row.expect("row"))
            .collect()
    }

    #[test]
    fn stores_a_note_against_the_live_match_deck_and_archetype() {
        let (_dir, state, writer, _events) = world("overlay-note");
        let changed = add_note(
            &state,
            Some(&with_archetype()),
            "Keep Rest in Peace, cut Cut Down",
            NOW,
        )
        .expect("note");
        assert_eq!(
            changed,
            Some(crate::events::GuidesChanged {
                deck_id: 1,
                archetype_id: 101
            })
        );
        assert_eq!(
            notes(&writer),
            vec![(1, 101, "Keep Rest in Peace, cut Cut Down".to_string())]
        );
        stop_tracker(&state);
    }

    #[test]
    fn requires_a_body() {
        let (_dir, state, writer, _events) = world("overlay-note-empty");
        assert_eq!(
            add_note(&state, Some(&with_archetype()), "  ", NOW),
            Err(CommandError::Refused(
                "The body field is required.".to_string()
            ))
        );
        assert!(notes(&writer).is_empty());
        stop_tracker(&state);
    }

    #[test]
    fn rejects_a_body_over_two_thousand_characters() {
        let (_dir, state, writer, _events) = world("overlay-note-long");
        assert_eq!(
            add_note(&state, Some(&with_archetype()), &"a".repeat(2001), NOW),
            Err(CommandError::Refused(
                "The body field must not be greater than 2000 characters.".to_string()
            ))
        );
        assert!(notes(&writer).is_empty());
        stop_tracker(&state);
    }

    /// 0.x "does not store a note when no archetype is resolved" (Review
    /// Focus 3).
    #[test]
    fn a_note_without_a_resolved_archetype_is_not_stored() {
        let (_dir, state, writer, _events) = world("overlay-note-no-archetype");
        assert_eq!(
            add_note(&state, Some(&live()), "Orphan note", NOW),
            Ok(None)
        );
        assert_eq!(add_note(&state, None, "Orphan note", NOW), Ok(None));
        let no_deck = LiveContext {
            deck_id: None,
            ..with_archetype()
        };
        assert_eq!(
            add_note(&state, Some(&no_deck), "Orphan note", NOW),
            Ok(None)
        );
        assert!(notes(&writer).is_empty());
        stop_tracker(&state);
    }

    #[test]
    fn deletes_a_note() {
        let (_dir, state, writer, _events) = world("overlay-note-delete");
        note(&writer, 1, 101, "Delete me", NOW);
        let id: i64 = writer
            .query_row("SELECT id FROM deck_archetype_notes", [], |row| row.get(0))
            .expect("id");
        crate::guides::delete_one_note(&state, id).expect("delete");
        assert!(notes(&writer).is_empty());
        stop_tracker(&state);
    }

    #[test]
    fn creates_a_sideboard_guide_in_the_background_when_noting_an_unguided_matchup() {
        let (_dir, state, writer, _events) = world("overlay-note-guide");
        add_note(
            &state,
            Some(&with_archetype()),
            "Board in Rest in Peace",
            NOW,
        )
        .expect("note");
        assert_eq!(guides(&writer), vec![(1, 101)]);
        stop_tracker(&state);
    }

    #[test]
    fn does_not_create_a_second_guide_when_one_already_exists_for_the_matchup() {
        let (_dir, state, writer, _events) = world("overlay-note-one-guide");
        writer
            .execute(
                "INSERT INTO sideboard_guides (deck_id, archetype_id, created_at, updated_at)
                 VALUES (1, 101, 1, 1)",
                [],
            )
            .expect("guide");
        add_note(
            &state,
            Some(&with_archetype()),
            "Board in Rest in Peace",
            NOW,
        )
        .expect("note");
        assert_eq!(guides(&writer).len(), 1);
        stop_tracker(&state);
    }

    // ---- The picker ----

    fn shares(rows: &[(&str, &str)]) -> mymtgo_api::MetagameShares {
        mymtgo_api::MetagameShares {
            format: "modern".to_string(),
            days: 90,
            window_start: "2026-04-02".to_string(),
            window_end: "2026-07-01".to_string(),
            matches: 100,
            archetypes: rows
                .iter()
                .map(|(uuid, name)| mymtgo_api::GlobalShare {
                    uuid: uuid.to_string(),
                    name: name.to_string(),
                    color_identity: Some("R".to_string()),
                    share: Some(10.0),
                    matches: 10,
                    cards: Vec::new(),
                })
                .collect(),
            other_share: None,
            unnameable_share: None,
        }
    }

    /// Spec 7.3: the format's held archetypes and, online, the 90-day
    /// metagame's unmet ones, by name, then the fallbacks.
    #[test]
    fn the_picker_lists_held_and_unmet_archetypes_then_the_fallbacks() {
        let (_dir, state, writer, _events) = world("overlay-picker");
        let offline = archetype_options(&writer, Some("CModern"), None).expect("read");
        let names: Vec<(&str, bool, bool)> = offline
            .iter()
            .map(|option| {
                (
                    option.name.as_str(),
                    option.id.is_some(),
                    option.is_fallback,
                )
            })
            .collect();
        assert_eq!(
            names[..2],
            [("Esper Blink", true, false), ("Ruby Storm", true, false)]
        );
        assert!(names[2..].iter().all(|(_, _, fallback)| *fallback));
        let online = archetype_options(
            &writer,
            Some("CModern"),
            Some(&shares(&[
                ("q-storm", "Ruby Storm"),
                ("q-unmet", "Amulet Titan"),
            ])),
        )
        .expect("read");
        let regular: Vec<(&str, Option<i64>)> = online
            .iter()
            .filter(|option| !option.is_fallback)
            .map(|option| (option.name.as_str(), option.id))
            .collect();
        assert_eq!(
            regular,
            vec![
                ("Amulet Titan", None),
                ("Esper Blink", Some(101)),
                ("Ruby Storm", Some(102))
            ]
        );
        assert_eq!(online[0].uuid, "q-unmet");
        assert_eq!(online[0].colors, vec!["R"]);
        assert!(online.last().expect("fallback").is_fallback);
        stop_tracker(&state);
    }
}
