use std::path::PathBuf;
use std::time::Instant;

use fixture_tools::TestDir;
use overlay::{GRACE_MS, LeagueStatus, LeagueTheme, LiveMatch, OverlayArtwork, OverlaySize, Rect};
use serde_json::{Value, json};
use store::params;

use super::*;
use crate::overlay::game_net::GameNet;
use crate::overlay::windows::testing::{Call, FakeWindows};
use crate::overlay::worker::{OverlayMsg, Worker};
use crate::overlay::{LEAGUE_OVERLAY, MAIN_WINDOW};
use crate::settings::overlay_background_file_row;

/// 2026-07-01T10:00:00Z.
const NOW: i64 = 1_782_900_000_000;
const MINUTE: i64 = 60_000;
const KARN: &str = "https://cards.example/karn.jpg";

fn db(label: &str) -> (TestDir, Connection) {
    let dir = TestDir::new(label);
    let conn = store::open_writer(&dir.join("mymtgo.sqlite")).expect("open");
    conn.execute(
        "INSERT INTO accounts (id, username, first_seen_at) VALUES (1, 'Qlocal', 0)",
        [],
    )
    .expect("account");
    deck(&conn, 1, "Qtron v3 final", 5001, KARN);
    (dir, conn)
}

/// A deck with one version (the same id) and a cover with a remote art crop.
fn deck(conn: &Connection, id: i64, name: &str, cover: u32, art: &str) {
    conn.execute(
        "INSERT INTO cards (catalog_id, oracle_id, name, type, art_crop)
         VALUES (?1, 'oracle-' || ?1, 'Qcover', 'Artifact', ?2)",
        params![cover, art],
    )
    .expect("card");
    conn.execute(
        "INSERT INTO decks (id, mtgo_id, name, format, color_identity, cover_catalog_id,
             created_at, updated_at, source)
         VALUES (?1, 'q-deck-' || ?1, ?2, 'CMODERN', 'G', ?3, 1, 1, 'file')",
        params![id, name, cover],
    )
    .expect("deck");
    conn.execute(
        "INSERT INTO deck_versions (id, deck_id, signature, modified_at) VALUES (?1, ?1, 'SIG', 1)",
        [id],
    )
    .expect("version");
}

/// An active Modern league run of the local account, deck 1.
fn run(conn: &Connection, id: i64, state: &str, completed_at: Option<i64>) {
    conn.execute(
        "INSERT INTO leagues (id, run_key, account_id, source, kind, state, token, name, format,
             deck_id, started_at, completed_at, created_at)
         VALUES (?1, 'run-' || ?1, 1, 'log', 'constructed', ?2, 'q-league-' || ?1,
             'Modern League 14-09-2026 11:01am', 'CMODERN', 1, ?3, ?4, 1)",
        params![id, state, NOW - 60 * MINUTE, completed_at],
    )
    .expect("run");
}

/// A log match of the local account with deck version `version`, in `league`.
fn a_match(
    conn: &Connection,
    id: i64,
    league: Option<i64>,
    state: &str,
    outcome: &str,
    started_at: i64,
    version: i64,
) {
    conn.execute(
        "INSERT INTO matches (id, token, account_id, source, format, state, outcome, started_at,
             last_activity_at, revision, derived_version, league_token, league_id,
             deck_version_id)
         VALUES (?1, 'q-match-' || ?1, 1, 'log', 'CMODERN', ?2, ?3, ?4, ?4, 1, 1,
             CASE WHEN ?5 IS NULL THEN NULL ELSE 'q-league-' || ?5 END, ?5, ?6)",
        params![id, state, outcome, started_at, league, version],
    )
    .expect("match");
}

fn game(conn: &Connection, match_id: i64, number: i64, result: &str) {
    conn.execute(
        "INSERT INTO games (match_id, game_number, result, end_reason) VALUES (?1, ?2, ?3, 'unknown')",
        params![match_id, number, result],
    )
    .expect("game");
}

fn token(id: i64) -> String {
    format!("q-match-{id}")
}

fn live(id: i64, state: &str) -> LiveMatch {
    LiveMatch {
        token: token(id),
        source: "log".to_string(),
        state: state.to_string(),
        game_count: 1,
        format: Some("CMODERN".to_string()),
        hidden: false,
        needs_attention: false,
        started_at: NOW - MINUTE,
    }
}

fn updated(id: i64, state: &str, league: bool) -> OverlayMsg {
    OverlayMsg::MatchUpdated {
        live: Box::new(live(id, state)),
        league,
    }
}

fn settings() -> Settings {
    Settings {
        league_window: true,
        ..Settings::default()
    }
}

fn wake(
    worker: &mut Worker,
    conn: &Connection,
    windows: &mut FakeWindows,
    messages: Vec<OverlayMsg>,
    now_ms: i64,
) {
    for message in messages {
        worker.handle(conn, message);
    }
    worker.update(conn, windows, now_ms);
}

/// The league cards pushed to the league window, in order.
fn cards(windows: &FakeWindows) -> Vec<Value> {
    windows
        .emitted()
        .into_iter()
        .filter(|(label, event)| label == LEAGUE_OVERLAY && event.name == "league-overlay-changed")
        .map(|(_, event)| event.payload["state"].clone())
        .collect()
}

/// A worker that has pushed its first card.
fn started(conn: &Connection, windows: &mut FakeWindows) -> Worker {
    let mut worker = Worker::new(settings(), false, true);
    worker.update(conn, windows, NOW);
    assert_eq!(cards(windows).len(), 1, "the first card");
    worker
}

// ---- 0.x LeagueOverlayChangedTest: which tracker events recompute the card ----

#[test]
fn recomputes_when_a_league_match_completes() {
    let (_dir, conn) = db("league-card-completes");
    run(&conn, 1, "active", None);
    a_match(&conn, 1, Some(1), "in_progress", "unknown", NOW - MINUTE, 1);
    let mut windows = FakeWindows::default();
    let mut worker = started(&conn, &mut windows);
    assert_eq!(cards(&windows)[0]["status"], "in_game");

    conn.execute(
        "UPDATE matches SET state = 'complete', outcome = 'loss' WHERE id = 1",
        [],
    )
    .expect("complete");
    wake(
        &mut worker,
        &conn,
        &mut windows,
        vec![updated(1, "complete", true)],
        NOW,
    );
    let card = cards(&windows).pop().expect("a new card");
    assert_eq!(card["status"], "waiting");
    assert_eq!(card["record"], json!({"wins": 0, "losses": 1}));
}

#[test]
fn recomputes_when_a_match_is_linked_to_a_league() {
    let (_dir, conn) = db("league-card-linked");
    run(&conn, 1, "active", None);
    a_match(&conn, 1, Some(1), "complete", "win", NOW - 20 * MINUTE, 1);
    a_match(&conn, 2, None, "in_progress", "unknown", NOW - MINUTE, 1);
    let mut windows = FakeWindows::default();
    let mut worker = started(&conn, &mut windows);
    assert_eq!(cards(&windows)[0]["status"], "waiting");

    conn.execute("UPDATE matches SET league_id = 1 WHERE id = 2", [])
        .expect("link");
    wake(
        &mut worker,
        &conn,
        &mut windows,
        vec![OverlayMsg::LeaguesChanged],
        NOW,
    );
    let card = cards(&windows).pop().expect("a new card");
    assert_eq!(card["status"], "in_game");
    assert_eq!(card["match"]["number"], 2);
}

#[test]
fn recomputes_when_a_match_is_unlinked_from_a_league() {
    let (_dir, conn) = db("league-card-unlinked");
    run(&conn, 1, "active", None);
    a_match(&conn, 1, Some(1), "complete", "win", NOW - 20 * MINUTE, 1);
    a_match(&conn, 2, Some(1), "in_progress", "unknown", NOW - MINUTE, 1);
    let mut windows = FakeWindows::default();
    let mut worker = started(&conn, &mut windows);
    assert_eq!(cards(&windows)[0]["status"], "in_game");

    conn.execute("UPDATE matches SET league_id = NULL WHERE id = 2", [])
        .expect("unlink");
    wake(
        &mut worker,
        &conn,
        &mut windows,
        vec![OverlayMsg::LeaguesChanged],
        NOW,
    );
    assert_eq!(
        cards(&windows).pop().expect("a new card")["status"],
        "waiting"
    );
}

#[test]
fn a_non_league_match_does_not_recompute_the_card() {
    let (_dir, conn) = db("league-card-non-league");
    let mut windows = FakeWindows::default();
    let mut worker = started(&conn, &mut windows);
    assert_eq!(cards(&windows)[0]["status"], "idle");

    // A newer match with another deck: a recompute would show that deck.
    deck(
        &conn,
        2,
        "Qother deck",
        5002,
        "https://cards.example/other.jpg",
    );
    a_match(&conn, 9, None, "complete", "win", NOW, 2);
    wake(
        &mut worker,
        &conn,
        &mut windows,
        vec![updated(9, "complete", false)],
        NOW,
    );
    assert_eq!(cards(&windows).len(), 1, "no new card");
}

#[test]
fn a_league_match_change_the_card_does_not_show_pushes_nothing() {
    let (_dir, conn) = db("league-card-ignored-match");
    run(&conn, 1, "active", None);
    a_match(&conn, 1, Some(1), "in_progress", "unknown", NOW - MINUTE, 1);
    let mut windows = FakeWindows::default();
    let mut worker = started(&conn, &mut windows);

    conn.execute("UPDATE matches SET notes = 'tight games' WHERE id = 1", [])
        .expect("notes");
    wake(
        &mut worker,
        &conn,
        &mut windows,
        vec![updated(1, "in_progress", true), OverlayMsg::LeaguesChanged],
        NOW,
    );
    assert_eq!(
        cards(&windows).len(),
        1,
        "the same card is not pushed again"
    );
}

#[test]
fn recomputes_when_a_league_game_gets_a_result() {
    let (_dir, conn) = db("league-card-game-result");
    run(&conn, 1, "active", None);
    a_match(&conn, 1, Some(1), "in_progress", "unknown", NOW - MINUTE, 1);
    game(&conn, 1, 1, "unknown");
    let mut windows = FakeWindows::default();
    let mut worker = started(&conn, &mut windows);
    assert_eq!(cards(&windows)[0]["match"]["games"], json!([{"won": null}]));

    conn.execute("UPDATE games SET result = 'win' WHERE match_id = 1", [])
        .expect("result");
    wake(
        &mut worker,
        &conn,
        &mut windows,
        vec![OverlayMsg::LeaguesChanged],
        NOW,
    );
    let card = cards(&windows).pop().expect("a new card");
    assert_eq!(card["match"]["games"], json!([{"won": true}]));
    assert_eq!(card["match"]["gamesWon"], 1);
}

#[test]
fn recomputes_when_a_league_game_is_created() {
    let (_dir, conn) = db("league-card-game-created");
    run(&conn, 1, "active", None);
    a_match(&conn, 1, Some(1), "in_progress", "unknown", NOW - MINUTE, 1);
    let mut windows = FakeWindows::default();
    let mut worker = started(&conn, &mut windows);
    assert_eq!(cards(&windows)[0]["match"]["games"], json!([]));

    game(&conn, 1, 1, "unknown");
    wake(
        &mut worker,
        &conn,
        &mut windows,
        vec![updated(1, "in_progress", true)],
        NOW,
    );
    assert_eq!(
        cards(&windows).pop().expect("a new card")["match"]["games"],
        json!([{"won": null}])
    );
}

#[test]
fn a_game_in_a_non_league_match_does_not_recompute_the_card() {
    let (_dir, conn) = db("league-card-non-league-game");
    run(&conn, 1, "active", None);
    a_match(&conn, 1, Some(1), "in_progress", "unknown", NOW - MINUTE, 1);
    a_match(&conn, 2, None, "in_progress", "unknown", NOW - MINUTE, 1);
    let mut windows = FakeWindows::default();
    let mut worker = started(&conn, &mut windows);

    // The league game changes too, unseen: only the other match was told.
    game(&conn, 1, 1, "loss");
    game(&conn, 2, 1, "loss");
    wake(
        &mut worker,
        &conn,
        &mut windows,
        vec![updated(2, "in_progress", false)],
        NOW,
    );
    assert_eq!(cards(&windows).len(), 1, "no new card");
}

#[test]
fn a_league_game_change_the_card_does_not_show_pushes_nothing() {
    let (_dir, conn) = db("league-card-ignored-game");
    run(&conn, 1, "active", None);
    a_match(&conn, 1, Some(1), "in_progress", "unknown", NOW - MINUTE, 1);
    game(&conn, 1, 1, "unknown");
    let mut windows = FakeWindows::default();
    let mut worker = started(&conn, &mut windows);

    conn.execute("UPDATE games SET turn_count = 7 WHERE match_id = 1", [])
        .expect("turns");
    wake(
        &mut worker,
        &conn,
        &mut windows,
        vec![OverlayMsg::LeaguesChanged],
        NOW,
    );
    assert_eq!(
        cards(&windows).len(),
        1,
        "the same card is not pushed again"
    );
}

// 0.x "waits for the transaction to commit so the overlay never reloads
// stale data": a match update can reach the worker before its batch
// commits; the league sync's `LeaguesChanged`, told after its commit, reads
// the card again.
#[test]
fn a_change_told_before_its_commit_is_read_again_after_it() {
    let (_dir, conn) = db("league-card-after-commit");
    run(&conn, 1, "active", None);
    a_match(&conn, 1, Some(1), "in_progress", "unknown", NOW - MINUTE, 1);
    let mut windows = FakeWindows::default();
    let mut worker = started(&conn, &mut windows);

    // Told, not yet stored: the card is the same.
    wake(
        &mut worker,
        &conn,
        &mut windows,
        vec![updated(1, "complete", true)],
        NOW,
    );
    assert_eq!(cards(&windows).len(), 1);
    conn.execute(
        "UPDATE matches SET state = 'complete', outcome = 'win' WHERE id = 1",
        [],
    )
    .expect("commit");
    wake(
        &mut worker,
        &conn,
        &mut windows,
        vec![OverlayMsg::LeaguesChanged],
        NOW,
    );
    let card = cards(&windows).pop().expect("the committed card");
    assert_eq!(card["record"], json!({"wins": 1, "losses": 0}));
}

#[test]
fn live_events_of_the_cards_live_match_recompute_it() {
    let (_dir, conn) = db("league-card-live-events");
    run(&conn, 1, "active", None);
    a_match(&conn, 1, Some(1), "in_progress", "unknown", NOW - MINUTE, 1);
    let mut windows = FakeWindows::default();
    let mut worker = started(&conn, &mut windows);
    game(&conn, 1, 1, "unknown");
    let announced = |tokens: Vec<String>| OverlayMsg::LiveEventsStored {
        tokens,
        detected_at: Instant::now(),
    };
    wake(
        &mut worker,
        &conn,
        &mut windows,
        vec![announced(vec!["q-other".to_string()])],
        NOW,
    );
    assert_eq!(cards(&windows).len(), 1, "another match's frames");
    wake(
        &mut worker,
        &conn,
        &mut windows,
        vec![announced(vec![token(1)])],
        NOW,
    );
    assert_eq!(cards(&windows).len(), 2, "the card's live match");
}

#[test]
fn the_card_goes_idle_when_a_finished_runs_minutes_on_it_end() {
    let (_dir, conn) = db("league-card-grace");
    run(&conn, 1, "complete", Some(NOW));
    a_match(&conn, 1, Some(1), "complete", "win", NOW - 2 * MINUTE, 1);
    let mut windows = FakeWindows::default();
    let mut worker = started(&conn, &mut windows);
    assert_eq!(cards(&windows)[0]["status"], "complete");
    assert_eq!(
        worker.next_wake(NOW),
        Some(std::time::Duration::from_millis((GRACE_MS + 1) as u64))
    );
    worker.update(&conn, &mut windows, NOW + GRACE_MS);
    assert_eq!(cards(&windows).len(), 1, "still on the card at its last ms");
    worker.update(&conn, &mut windows, NOW + GRACE_MS + 1);
    assert_eq!(cards(&windows).pop().expect("idle")["status"], "idle");
    assert_eq!(
        worker.next_wake(NOW + GRACE_MS + 1),
        None,
        "nothing more to wait for"
    );
}

#[test]
fn the_chosen_finished_minutes_take_the_run_off_the_card_sooner() {
    let (_dir, conn) = db("league-card-finished-minutes");
    run(&conn, 1, "complete", Some(NOW));
    a_match(&conn, 1, Some(1), "complete", "win", NOW - 2 * MINUTE, 1);
    let mut windows = FakeWindows::default();
    let mut worker = started(&conn, &mut windows);

    let saved = Settings {
        overlay_finished_minutes: 1,
        ..settings()
    };
    wake(
        &mut worker,
        &conn,
        &mut windows,
        vec![OverlayMsg::SettingsChanged {
            settings: Box::new(Settings::from_rows(&saved.to_rows())),
        }],
        NOW,
    );
    assert_eq!(
        worker.next_wake(NOW),
        Some(std::time::Duration::from_millis((MINUTE + 1) as u64))
    );
    worker.update(&conn, &mut windows, NOW + MINUTE + 1);
    assert_eq!(cards(&windows).pop().expect("idle")["status"], "idle");
}

// ---- Where the card goes ----

#[test]
fn the_card_is_pushed_to_the_league_window_and_the_main_window() {
    let (_dir, conn) = db("league-card-targets");
    let mut windows = FakeWindows::default();
    started(&conn, &mut windows);
    let labels: Vec<String> = windows
        .emitted()
        .into_iter()
        .filter(|(_, event)| event.name == "league-overlay-changed")
        .map(|(label, _)| label)
        .collect();
    assert_eq!(
        labels,
        vec![LEAGUE_OVERLAY.to_string(), MAIN_WINDOW.to_string()]
    );
}

#[test]
fn still_pushes_the_league_card_when_overlays_are_unavailable() {
    let (_dir, conn) = db("league-card-unavailable");
    run(&conn, 1, "active", None);
    a_match(&conn, 1, Some(1), "in_progress", "unknown", NOW - MINUTE, 1);
    let mut worker = Worker::new(settings(), false, false);
    let mut windows = FakeWindows::default();
    worker.update(&conn, &mut windows, NOW);
    assert!(
        windows.opened_labels().is_empty(),
        "no window on native Wayland"
    );
    let main: Vec<Value> = windows
        .emitted()
        .into_iter()
        .filter(|(label, _)| label == MAIN_WINDOW)
        .map(|(_, event)| event.payload["state"].clone())
        .collect();
    assert_eq!(main.len(), 1);
    assert_eq!(main[0]["status"], "in_game");
    assert_eq!(
        worker.league_state().map(|state| state.status),
        Some(LeagueStatus::InGame)
    );
}

// ---- 0.x OverlayCardSettingsTest ----

#[test]
fn persists_artwork_and_size_and_pushes_a_card_reload() {
    let (_dir, conn) = db("league-card-look");
    a_match(&conn, 1, None, "complete", "win", NOW - 30 * MINUTE, 1);
    let mut windows = FakeWindows::default();
    let mut worker = started(&conn, &mut windows);
    assert_eq!(
        cards(&windows)[0]["art"],
        json!({"url": KARN, "thumbnail": false})
    );

    let saved = Settings {
        overlay_artwork: Some(OverlayArtwork::NoArt),
        overlay_size: OverlaySize::Compact,
        ..settings()
    };
    let stored = Settings::from_rows(&saved.to_rows());
    assert_eq!(stored.overlay_artwork, Some(OverlayArtwork::NoArt));
    assert_eq!(stored.overlay_size, OverlaySize::Compact);
    wake(
        &mut worker,
        &conn,
        &mut windows,
        vec![OverlayMsg::SettingsChanged {
            settings: Box::new(stored),
        }],
        NOW,
    );
    let card = cards(&windows).pop().expect("a new card");
    assert_eq!(card["size"], "compact");
    assert_eq!(card["art"], Value::Null);
}

// A saved display choice reaches the pushed card (league card options spec
// section 3): the window shows it, and the changed card is republished.
#[test]
fn a_display_choice_reaches_the_pushed_card() {
    let (_dir, conn) = db("league-card-display");
    a_match(&conn, 1, None, "complete", "win", NOW - 30 * MINUTE, 1);
    let mut windows = FakeWindows::default();
    let mut worker = started(&conn, &mut windows);
    assert_eq!(cards(&windows)[0]["display"]["games"], "dots");

    let mut saved = settings();
    saved.overlay_league_display.games = overlay::LeagueGames::Letters;
    saved.overlay_league_display.record = false;
    let stored = Settings::from_rows(&saved.to_rows());
    wake(
        &mut worker,
        &conn,
        &mut windows,
        vec![OverlayMsg::SettingsChanged {
            settings: Box::new(stored),
        }],
        NOW,
    );
    let card = cards(&windows).pop().expect("a new card");
    assert_eq!(card["display"]["games"], "letters");
    assert_eq!(card["display"]["record"], false);
}

#[test]
fn rejects_unknown_artwork_and_size_values() {
    let mut payload = serde_json::to_value(Settings::default()).expect("settings as JSON");
    payload["overlayArtwork"] = json!("banner");
    assert!(serde_json::from_value::<Settings>(payload.clone()).is_err());
    payload["overlayArtwork"] = json!("deck");
    payload["overlaySize"] = json!("huge");
    assert!(serde_json::from_value::<Settings>(payload.clone()).is_err());
    payload["overlaySize"] = json!("compact");
    assert!(serde_json::from_value::<Settings>(payload).is_ok());
}

fn league_window_open() -> FakeWindows {
    let mut windows = FakeWindows::default();
    windows.open.insert(
        LEAGUE_OVERLAY.to_string(),
        Rect {
            x: 40,
            y: 40,
            width: 300,
            height: 80,
        },
    );
    windows
}

fn resizes(windows: &FakeWindows) -> Vec<(String, f64, f64)> {
    windows
        .calls
        .iter()
        .filter_map(|call| match call {
            Call::Resized(label, width, height) => Some((label.clone(), *width, *height)),
            _ => None,
        })
        .collect()
}

fn size_saved(size: OverlaySize) -> OverlayMsg {
    OverlayMsg::SettingsChanged {
        settings: Box::new(Settings {
            overlay_size: size,
            ..settings()
        }),
    }
}

#[test]
fn resizes_an_open_league_overlay_when_the_size_changes() {
    let (_dir, conn) = db("league-card-resize");
    let mut windows = league_window_open();
    let mut worker = started(&conn, &mut windows);
    wake(
        &mut worker,
        &conn,
        &mut windows,
        vec![size_saved(OverlaySize::Compact)],
        NOW,
    );
    assert_eq!(
        resizes(&windows),
        vec![(LEAGUE_OVERLAY.to_string(), 240.0, 58.0)]
    );
    // The same size again resizes nothing.
    wake(
        &mut worker,
        &conn,
        &mut windows,
        vec![size_saved(OverlaySize::Compact)],
        NOW,
    );
    assert_eq!(resizes(&windows).len(), 1);
    assert_eq!(cards(&windows).pop().expect("the card")["size"], "compact");
}

fn theme_saved(theme: LeagueTheme) -> OverlayMsg {
    OverlayMsg::SettingsChanged {
        settings: Box::new(Settings {
            overlay_league_theme: theme,
            ..settings()
        }),
    }
}

#[test]
fn resizes_an_open_league_overlay_when_the_theme_changes() {
    let (_dir, conn) = db("league-card-theme-resize");
    let mut windows = league_window_open();
    let mut worker = started(&conn, &mut windows);
    wake(
        &mut worker,
        &conn,
        &mut windows,
        vec![theme_saved(LeagueTheme::Versus)],
        NOW,
    );
    assert_eq!(
        resizes(&windows),
        vec![(LEAGUE_OVERLAY.to_string(), 580.0, 104.0)]
    );
    assert_eq!(cards(&windows).pop().expect("the card")["theme"], "versus");
}

#[test]
fn does_not_resize_when_the_overlay_window_is_closed() {
    let (_dir, conn) = db("league-card-no-resize");
    let mut windows = FakeWindows::default();
    let mut worker = Worker::new(Settings::default(), false, true);
    worker.update(&conn, &mut windows, NOW);
    wake(
        &mut worker,
        &conn,
        &mut windows,
        vec![size_saved(OverlaySize::Compact)],
        NOW,
    );
    assert!(resizes(&windows).is_empty());
}

fn set_background_row(conn: &Connection, name: Option<&str>) {
    let (key, value) = overlay_background_file_row(name);
    conn.execute(
        "INSERT INTO settings (key, value) VALUES (?1, ?2)
         ON CONFLICT (key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )
    .expect("row");
}

#[test]
fn pushes_a_card_reload_after_choosing_and_removing_a_background() {
    let (dir, conn) = db("league-card-background");
    a_match(&conn, 1, None, "complete", "win", NOW - 30 * MINUTE, 1);
    let folder: PathBuf = dir.join("overlay");
    let mut windows = FakeWindows::default();
    let mut worker = Worker::new(settings(), false, true).with_overlay_folder(folder.clone());
    worker.update(&conn, &mut windows, NOW);
    assert_eq!(cards(&windows).len(), 1);

    std::fs::create_dir_all(&folder).expect("folder");
    std::fs::write(folder.join("background-77.png"), b"q").expect("file");
    set_background_row(&conn, Some("background-77.png"));
    wake(
        &mut worker,
        &conn,
        &mut windows,
        vec![OverlayMsg::BackgroundChanged],
        NOW,
    );
    std::fs::remove_file(folder.join("background-77.png")).expect("delete");
    set_background_row(&conn, None);
    wake(
        &mut worker,
        &conn,
        &mut windows,
        vec![OverlayMsg::BackgroundChanged],
        NOW,
    );

    let pushed = cards(&windows);
    assert_eq!(pushed.len(), 3, "the first card, then one per change");
    assert_eq!(pushed[1]["art"]["url"], background_url("background-77.png"));
    assert_eq!(pushed[2]["art"], json!({"url": KARN, "thumbnail": false}));
}

/// A live league match against Qrival (opponent 1) with a manual archetype.
fn live_league_match_against_a_pick(conn: &Connection) {
    run(conn, 1, "active", None);
    a_match(conn, 1, Some(1), "in_progress", "unknown", NOW - MINUTE, 1);
    conn.execute(
        "INSERT INTO opponents (id, username, first_seen_at) VALUES (1, 'Qrival', 0)",
        [],
    )
    .expect("opponent");
    conn.execute("UPDATE matches SET opponent_id = 1 WHERE id = 1", [])
        .expect("opponent");
    conn.execute(
        "INSERT INTO archetypes (id, uuid, name, format, color_identity, source, created_at, updated_at)
         VALUES (101, 'q-tempo', 'Dimir Tempo', 'modern', 'U,B', 'downloaded', 1, 1)",
        [],
    )
    .expect("archetype");
    conn.execute(
        "INSERT INTO match_archetypes (match_id, is_opponent, archetype_id, confidence, manual)
         VALUES (1, 1, 101, 1.0, 1)",
        [],
    )
    .expect("pick");
}

fn opponent_name(card: &LeagueCard) -> Option<String> {
    card.state
        .live_match
        .as_ref()
        .and_then(|live| live.opponent_archetype.clone())
}

#[test]
fn the_live_matchs_opponent_archetype_is_on_the_card() {
    let (_dir, conn) = db("league-card-opponent");
    live_league_match_against_a_pick(&conn);
    let card = league_card(
        &conn,
        &Settings::default(),
        None,
        NOW,
        None,
        &mut GameNet::default(),
    )
    .expect("card");
    assert_eq!(opponent_name(&card).as_deref(), Some("Dimir Tempo"));
    let json = serde_json::to_value(&card.state).expect("json");
    assert_eq!(json["match"]["opponentArchetype"], "Dimir Tempo");
}

#[test]
fn the_card_uses_the_game_overlays_answer_for_the_live_match() {
    let (_dir, conn) = db("league-card-opponent-known");
    live_league_match_against_a_pick(&conn);
    let known = KnownOpponent {
        token: token(1),
        archetype: "Izzet Murktide".to_string(),
    };
    let card = league_card(
        &conn,
        &Settings::default(),
        None,
        NOW,
        Some(&known),
        &mut GameNet::default(),
    )
    .expect("card");
    assert_eq!(opponent_name(&card).as_deref(), Some("Izzet Murktide"));
}

#[test]
fn a_live_match_with_no_known_archetype_has_none() {
    let (_dir, conn) = db("league-card-opponent-unknown");
    live_league_match_against_a_pick(&conn);
    conn.execute("DELETE FROM match_archetypes", [])
        .expect("no pick");
    let card = league_card(
        &conn,
        &Settings::default(),
        None,
        NOW,
        None,
        &mut GameNet::default(),
    )
    .expect("card");
    assert!(card.state.live_match.is_some());
    assert_eq!(opponent_name(&card), None);
}

#[test]
fn a_card_with_no_live_match_names_no_opponent() {
    let (_dir, conn) = db("league-card-opponent-idle");
    live_league_match_against_a_pick(&conn);
    conn.execute(
        "UPDATE matches SET state = 'complete', outcome = 'win' WHERE id = 1",
        [],
    )
    .expect("over");
    let card = league_card(
        &conn,
        &Settings::default(),
        None,
        NOW,
        None,
        &mut GameNet::default(),
    )
    .expect("card");
    assert_eq!(card.state.live_match, None);
}

#[test]
fn a_changed_pick_pushes_the_card_again_with_the_game_overlay_off() {
    let (_dir, conn) = db("league-card-opponent-push");
    live_league_match_against_a_pick(&conn);
    let mut windows = FakeWindows::default();
    let mut worker = started(&conn, &mut windows);
    assert_eq!(
        cards(&windows)[0]["match"]["opponentArchetype"],
        "Dimir Tempo"
    );

    conn.execute(
        "INSERT INTO archetypes (id, uuid, name, format, source, created_at, updated_at)
         VALUES (102, 'q-blink', 'Esper Blink', 'modern', 'downloaded', 1, 1)",
        [],
    )
    .expect("archetype");
    conn.execute("UPDATE match_archetypes SET archetype_id = 102", [])
        .expect("repick");
    wake(
        &mut worker,
        &conn,
        &mut windows,
        vec![OverlayMsg::MatchArchetypesChanged { match_ids: vec![1] }],
        NOW,
    );
    assert_eq!(
        cards(&windows).pop().expect("a new card")["match"]["opponentArchetype"],
        "Esper Blink"
    );
}

// 0.x "passes artwork, size and deck art to the settings page": the card
// (the settings preview's real deck) has the default look and the last
// played deck's art.
#[test]
fn the_idle_card_has_the_default_look_and_the_last_played_decks_art() {
    let (_dir, conn) = db("league-card-idle");
    a_match(&conn, 1, None, "complete", "win", NOW - MINUTE, 1);
    let card = league_card(
        &conn,
        &Settings::default(),
        None,
        NOW,
        None,
        &mut GameNet::default(),
    )
    .expect("card");
    assert_eq!(card.state.status, LeagueStatus::Idle);
    assert_eq!(card.state.size, OverlaySize::Full);
    assert_eq!(card.state.art.map(|art| art.url).as_deref(), Some(KARN));
    assert_eq!(
        card.state.deck.map(|deck| deck.name).as_deref(),
        Some("Qtron v3 final")
    );
    assert!(card.live_tokens.is_empty());
    assert_eq!(card.changes_at_ms, None);
}

#[test]
fn the_card_shows_the_decks_own_cover_only_when_its_file_exists() {
    let (dir, conn) = db("league-card-cover-file");
    a_match(&conn, 1, None, "complete", "win", NOW - MINUTE, 1);
    conn.execute("UPDATE decks SET cover_file = '1-5.png' WHERE id = 1", [])
        .expect("cover");
    let overlay = dir.join("overlay");
    let art = |conn: &Connection| {
        league_card(
            conn,
            &Settings::default(),
            Some(&overlay),
            NOW,
            None,
            &mut GameNet::default(),
        )
        .expect("card")
        .state
        .art
        .map(|art| art.url)
    };

    assert_eq!(art(&conn).as_deref(), Some(KARN), "no file: the printing");

    let covers = crate::decks::cover::folder(dir.path());
    std::fs::create_dir_all(&covers).expect("folder");
    std::fs::write(covers.join("1-5.png"), b"q").expect("file");
    assert_eq!(
        art(&conn),
        Some(format!("{}1-5.png", scheme_base("deckcover")))
    );
}

#[test]
fn a_tournament_decks_missing_cover_file_is_cleared() {
    let deck = |id: i64| overlay::OverlayDeck {
        id,
        name: "Qdeck".to_string(),
        archetype: None,
        color_identity: None,
        label: None,
        cover_art_crop: Some(KARN.to_string()),
        cover_local_art_crop: None,
        cover_file: Some(format!("{id}-5.png")),
    };
    let member = overlay::LeagueMatchInput {
        id: 1,
        token: "q-t".to_string(),
        state: mtgo_match::MatchState::Complete,
        outcome: mtgo_match::Outcome::Win,
        started_at_ms: NOW,
        ended_at_ms: None,
        games: Vec::new(),
        sideboarding_events: Vec::new(),
        newest_game_start: None,
        first_frame: None,
        newest_frame: None,
    };
    let mut input = LeagueCardInput {
        tournaments: vec![overlay::TournamentInput {
            mtgo_event_id: 700,
            name: None,
            format: None,
            status: leagues::standing::StandingStatus::WaitingForRound,
            finish: None,
            round: Some(2),
            stage: leagues::standing::StandingStage::Swiss,
            swiss_rounds: None,
            cut_size: None,
            rank: None,
            players: None,
            matches_left: None,
            byes: 0,
            place: None,
            starts_at_ms: None,
            round_result_at_ms: None,
            finished_at_ms: None,
            last_transition_at_ms: NOW,
            last_event_at_ms: NOW,
            matches: vec![overlay::TournamentMatchInput {
                round: Some(1),
                deck: Some(deck(1)),
                member,
            }],
            registered_deck: Some(deck(2)),
            format_deck: Some(deck(3)),
        }],
        ..LeagueCardInput::default()
    };
    drop_missing_covers(&mut input, None);
    let tournament = &input.tournaments[0];
    assert_eq!(
        tournament.matches[0]
            .deck
            .as_ref()
            .and_then(|deck| deck.cover_file.clone()),
        None
    );
    assert_eq!(
        tournament
            .registered_deck
            .as_ref()
            .and_then(|deck| deck.cover_file.clone()),
        None
    );
    assert_eq!(
        tournament
            .format_deck
            .as_ref()
            .and_then(|deck| deck.cover_file.clone()),
        None
    );
}

#[test]
fn the_card_look_takes_the_background_only_when_its_file_exists() {
    let dir = TestDir::new("league-card-look-file");
    let folder = dir.join("overlay");
    let rows = vec![overlay_background_file_row(Some("background-5.gif"))];
    let look = card_look(&Settings::default(), &rows, Some(&folder));
    assert_eq!(look.background_url, None, "no file yet");
    assert_eq!(look.card_file_base, scheme_base("card"));
    assert_eq!(look.deck_cover_base, scheme_base("deckcover"));
    std::fs::create_dir_all(&folder).expect("folder");
    std::fs::write(folder.join("background-5.gif"), b"q").expect("file");
    let look = card_look(&Settings::default(), &rows, Some(&folder));
    assert_eq!(
        look.background_url,
        Some(background_url("background-5.gif"))
    );
    assert_eq!(
        card_look(&Settings::default(), &rows, None).background_url,
        None
    );
    let compact = Settings {
        overlay_size: OverlaySize::Compact,
        overlay_artwork: Some(OverlayArtwork::Deck),
        ..Settings::default()
    };
    let look = card_look(&compact, &[], Some(&folder));
    assert_eq!(
        (look.size, look.artwork),
        (OverlaySize::Compact, Some(OverlayArtwork::Deck))
    );
}

// ---- The label command (0.x UpdateOverlayDeckLabelTest, through the tracker) ----

#[test]
fn the_label_command_saves_through_the_tracker_and_refuses_an_unknown_deck() {
    let dir = TestDir::new("league-card-label");
    let (state, _events) = crate::commands::tests::ready_state(&dir);
    {
        let writer = store::open_writer(&state.db_path).expect("writer");
        deck(&writer, 1, "Qtron v3 final", 5001, KARN);
    }
    set_label(&state, 1, Some("  Qstream name  ".to_string())).expect("saved");
    let label: Option<String> = lock(&state.reader)
        .query_row("SELECT overlay_label FROM decks WHERE id = 1", [], |row| {
            row.get(0)
        })
        .expect("label");
    assert_eq!(label.as_deref(), Some("Qstream name"));
    assert_eq!(
        set_label(&state, 999, Some("Qnone".to_string())),
        Err(CommandError::Refused(
            "This deck no longer exists.".to_string()
        ))
    );
    crate::commands::stop_tracker(&state);
}
