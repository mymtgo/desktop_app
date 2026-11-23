use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::mpsc;
use std::time::Instant;

use fixture_tools::{Fixture, TestDir, load_fixture};
use mtgo_log::Env;
use overlay::{OverlaySize, Placement, Rect};
use store::params;
use tracker::engine::POLL_ACTIVE_MS;
use tracker::{Engine, TrackerConfig};

use super::*;
use crate::overlay::LEAGUE_OVERLAY;
use crate::overlay::windows::Monitors;
use crate::overlay::windows::testing::{Call, FakeWindows, primary};

/// 2026-07-01T10:00:00Z.
const NOW: i64 = 1_782_900_000_000;
const SECOND: i64 = 1_000;
const MINUTE: i64 = 60 * SECOND;

fn db(label: &str) -> (TestDir, Connection) {
    let dir = TestDir::new(label);
    let conn = store::open_writer(&dir.join("mymtgo.sqlite")).expect("open");
    conn.execute(
        "INSERT INTO accounts (id, username, first_seen_at) VALUES (1, 'Qlocal', 0)",
        [],
    )
    .expect("account");
    (dir, conn)
}

fn settings() -> Settings {
    Settings {
        game_overlay: true,
        draft_notes_window: true,
        league_window: false,
        ..Settings::default()
    }
}

fn worker(settings: Settings) -> Worker {
    Worker::new(settings, false, true)
}

/// A log match in progress with one game.
fn live(token: &str, started_at: i64) -> LiveMatch {
    LiveMatch {
        token: token.to_string(),
        source: "log".to_string(),
        state: "in_progress".to_string(),
        game_count: 1,
        format: Some("CMODERN".to_string()),
        hidden: false,
        needs_attention: false,
        started_at,
    }
}

fn updated(live: LiveMatch) -> OverlayMsg {
    OverlayMsg::MatchUpdated {
        live: Box::new(live),
        league: false,
    }
}

fn ended(token: &str) -> OverlayMsg {
    let mut over = live(token, 0);
    over.state = "ended".to_string();
    updated(over)
}

fn announced(tokens: &[&str]) -> OverlayMsg {
    OverlayMsg::LiveEventsStored {
        tokens: tokens.iter().map(|token| token.to_string()).collect(),
        detected_at: Instant::now(),
    }
}

/// A live match the tracker announced frames of.
fn playing(token: &str, started_at: i64) -> Vec<OverlayMsg> {
    vec![updated(live(token, started_at)), announced(&[token])]
}

/// One wake: handle the messages, then update.
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

fn with_open(labels: &[&str]) -> FakeWindows {
    let mut windows = FakeWindows::default();
    for label in labels {
        windows.open.insert(
            label.to_string(),
            Rect {
                x: 10,
                y: 10,
                width: 320,
                height: 220,
            },
        );
    }
    windows
}

fn game_opens(windows: &FakeWindows) -> usize {
    windows
        .opened_labels()
        .iter()
        .filter(|label| *label == GAME_OVERLAY)
        .count()
}

// ---- The game overlay (0.x SyncGameOverlayVisibilityTest window cases) ----

#[test]
fn opens_the_overlay_when_a_match_is_in_progress_and_the_setting_is_enabled() {
    let (_dir, conn) = db("overlay-worker-opens");
    let mut worker = worker(settings());
    let mut windows = FakeWindows::default();
    wake(&mut worker, &conn, &mut windows, playing("q-live", 10), NOW);
    assert_eq!(windows.opened_labels(), vec![GAME_OVERLAY.to_string()]);
}

#[test]
fn closes_an_open_overlay_when_no_match_is_in_progress() {
    let (_dir, conn) = db("overlay-worker-none");
    let mut worker = worker(settings());
    let mut windows = with_open(&[GAME_OVERLAY]);
    wake(&mut worker, &conn, &mut windows, vec![ended("q-over")], NOW);
    assert_eq!(windows.closed(), vec![GAME_OVERLAY.to_string()]);
    assert!(windows.opened().is_empty());
}

#[test]
fn closes_an_open_overlay_when_the_setting_is_disabled() {
    let (_dir, conn) = db("overlay-worker-setting-off");
    let mut worker = worker(Settings {
        game_overlay: false,
        ..settings()
    });
    let mut windows = with_open(&[GAME_OVERLAY]);
    wake(&mut worker, &conn, &mut windows, playing("q-live", 10), NOW);
    assert_eq!(windows.closed(), vec![GAME_OVERLAY.to_string()]);
}

/// 0.x "closes the overlay when AdvanceMatchState ends the match": the
/// completion arrives as the match's update.
#[test]
fn closes_the_overlay_when_the_match_ends() {
    let (_dir, conn) = db("overlay-worker-match-ends");
    let mut worker = worker(settings());
    let mut windows = FakeWindows::default();
    wake(&mut worker, &conn, &mut windows, playing("q-live", 10), NOW);
    assert_eq!(game_opens(&windows), 1);
    let mut complete = live("q-live", 10);
    complete.state = "complete".to_string();
    wake(
        &mut worker,
        &conn,
        &mut windows,
        vec![updated(complete)],
        NOW,
    );
    assert_eq!(windows.closed(), vec![GAME_OVERLAY.to_string()]);
}

#[test]
fn closes_an_open_overlay_when_the_only_match_in_progress_is_limited() {
    let (_dir, conn) = db("overlay-worker-limited");
    let mut worker = worker(settings());
    let mut windows = with_open(&[GAME_OVERLAY]);
    let mut draft = live("q-draft-match", 10);
    draft.format = Some("DHOBHOBHOB".to_string());
    wake(
        &mut worker,
        &conn,
        &mut windows,
        vec![updated(draft), announced(&["q-draft-match"])],
        NOW,
    );
    assert_eq!(windows.closed(), vec![GAME_OVERLAY.to_string()]);
}

#[test]
fn a_failed_or_removed_match_closes_the_overlay() {
    let (_dir, conn) = db("overlay-worker-failed");
    let mut worker = worker(settings());
    let mut windows = FakeWindows::default();
    wake(&mut worker, &conn, &mut windows, playing("q-a", 10), NOW);
    let failed = OverlayMsg::MatchFailed {
        token: "q-a".to_string(),
    };
    wake(&mut worker, &conn, &mut windows, vec![failed], NOW);
    assert_eq!(windows.closed(), vec![GAME_OVERLAY.to_string()]);

    wake(&mut worker, &conn, &mut windows, playing("q-b", 20), NOW);
    let removed = OverlayMsg::MatchRemoved {
        token: "q-b".to_string(),
    };
    wake(&mut worker, &conn, &mut windows, vec![removed], NOW);
    assert_eq!(windows.closed().len(), 2);
}

// ---- Opening the game overlay (0.x OpenGameOverlayWindowTest) ----

#[test]
fn opens_an_opponent_only_overlay_short_instead_of_at_the_full_default_height() {
    let (_dir, conn) = db("overlay-worker-short");
    let mut worker = worker(Settings {
        overlay_show_opponent: true,
        overlay_show_draw_odds: false,
        overlay_show_reveals: false,
        overlay_show_sideboard: false,
        ..settings()
    });
    let mut windows = FakeWindows::default();
    wake(&mut worker, &conn, &mut windows, playing("q-live", 10), NOW);
    let spec = &windows.opened_specs()[0];
    assert_eq!(
        spec.height,
        f64::from(overlay::height::OPPONENT_HEADER_ESTIMATE)
    );
    // Low enough for the collapsed strip.
    assert_eq!(
        spec.min_height,
        f64::from(overlay::height::COLLAPSED_MIN_HEIGHT)
    );
    assert_eq!(
        (spec.width, spec.min_width, spec.max_width),
        (320.0, 300.0, Some(400.0))
    );
}

#[test]
fn opens_at_the_full_height_when_a_tab_section_is_enabled() {
    let (_dir, conn) = db("overlay-worker-tall");
    let mut worker = worker(Settings {
        overlay_show_opponent: true,
        overlay_show_draw_odds: true,
        ..settings()
    });
    let mut windows = FakeWindows::default();
    wake(&mut worker, &conn, &mut windows, playing("q-live", 10), NOW);
    assert_eq!(
        windows.opened_specs()[0].height,
        f64::from(overlay::height::OPPONENT_HEADER_ESTIMATE + overlay::height::TAB_AREA)
    );
}

// ---- The draft notes window ----

/// A draft of the local account, then the league sync at `now_ms` (as the
/// tracker runs it before the worker reads).
fn draft(conn: &Connection, id: i64, state: &str, ended_at: Option<i64>, now_ms: i64) {
    conn.execute(
        "INSERT INTO drafts (id, draft_token, account_id, source, seat_count, seat_index,
             state, pack_size, picks_expected, started_at, imported_from)
         VALUES (?1, 'q-draft-' || ?1, 1, 'imported', 8, 5, 'picking', 14, 42, ?2, ?1)",
        params![id, now_ms - MINUTE],
    )
    .expect("draft");
    set_draft(conn, id, state, ended_at, now_ms);
}

fn set_draft(conn: &Connection, id: i64, state: &str, ended_at: Option<i64>, now_ms: i64) {
    conn.execute(
        "UPDATE drafts SET state = ?2, ended_at = ?3,
             abandons_at = CASE WHEN ?2 IN ('connecting', 'picking') THEN abandons_at END
         WHERE id = ?1",
        params![id, state, ended_at],
    )
    .expect("state");
    let tx = conn.unchecked_transaction().expect("transaction");
    store::league_sync::sync_every_owner(&tx, chrono_tz::UTC, now_ms).expect("league sync");
    tx.commit().expect("commit");
}

fn draft_opens(windows: &FakeWindows) -> usize {
    windows
        .opened_labels()
        .iter()
        .filter(|label| *label == DRAFT_NOTES)
        .count()
}

// 0.x DraftNotesWindowPipelineTest, driven by `LimitedChanged`.

#[test]
fn opens_the_draft_notes_window_while_a_draft_is_picking() {
    let (_dir, conn) = db("overlay-worker-draft-opens");
    let mut worker = worker(settings());
    let mut windows = FakeWindows::default();
    wake(&mut worker, &conn, &mut windows, Vec::new(), NOW);
    assert_eq!(draft_opens(&windows), 0);
    draft(&conn, 1, "picking", None, NOW);
    wake(
        &mut worker,
        &conn,
        &mut windows,
        vec![OverlayMsg::LimitedChanged],
        NOW,
    );
    assert_eq!(windows.opened_labels(), vec![DRAFT_NOTES.to_string()]);
    let pushed = windows.emitted();
    let (label, event) = pushed.last().expect("a push");
    assert_eq!(label, DRAFT_NOTES);
    assert_eq!(event.payload["view"]["state"], "picking");
}

#[test]
fn closes_the_draft_notes_window_once_the_draft_is_abandoned() {
    let (_dir, conn) = db("overlay-worker-draft-abandoned");
    draft(&conn, 1, "picking", None, NOW);
    let mut worker = worker(settings());
    let mut windows = FakeWindows::default();
    wake(&mut worker, &conn, &mut windows, Vec::new(), NOW);
    assert_eq!(draft_opens(&windows), 1);
    set_draft(&conn, 1, "abandoned", Some(NOW), NOW);
    wake(
        &mut worker,
        &conn,
        &mut windows,
        vec![OverlayMsg::LimitedChanged],
        NOW,
    );
    assert_eq!(windows.closed(), vec![DRAFT_NOTES.to_string()]);
}

#[test]
fn does_not_open_the_draft_notes_window_when_the_setting_is_off() {
    let (_dir, conn) = db("overlay-worker-draft-off");
    draft(&conn, 1, "picking", None, NOW);
    let mut worker = worker(Settings {
        draft_notes_window: false,
        ..settings()
    });
    let mut windows = FakeWindows::default();
    wake(
        &mut worker,
        &conn,
        &mut windows,
        vec![OverlayMsg::LimitedChanged],
        NOW,
    );
    assert!(windows.opened().is_empty());
}

#[test]
fn opens_the_notes_window_for_the_hobbit_draft_while_inside_grace_and_closes_it_after() {
    let fixture = load_fixture("draft_hobbit");
    let dir = TestDir::new("overlay-worker-hobbit");
    // Recorded on a UTC+1 machine: read in that zone, as Plan N's tests do.
    let config = TrackerConfig {
        timezone: chrono_tz::Europe::London,
        ..hobbit_config(&dir, &fixture)
    };
    dir.write("logs/mtgo.log", &fixture.bytes);
    let mut engine = Engine::open(
        config.clone(),
        Env::current(),
        Arc::default(),
        Vec::new(),
        fixture.meta.now_ms(),
    )
    .expect("engine");
    let mut now = fixture.meta.now_ms();
    tick_until_read(&mut engine, now);
    for _ in 0..3 {
        now += 1_000;
        engine.tick(now).expect("tick");
    }
    engine.flush().expect("flush");
    drop(engine);
    let conn = store::open_writer(&config.db_path).expect("writer");
    // The league sync the tracker runs before the pages read.
    let tx = conn.unchecked_transaction().expect("transaction");
    store::league_sync::sync_every_owner(&tx, config.timezone, now).expect("league sync");
    tx.commit().expect("commit");
    let ended_at: i64 = conn
        .query_row(
            "SELECT ended_at FROM drafts WHERE state = 'finished'",
            [],
            |row| row.get(0),
        )
        .expect("the finished draft");

    // Ten seconds after the draft ended: the window opens.
    let mut worker = worker(settings());
    let mut windows = FakeWindows::default();
    let inside = ended_at + 10 * SECOND;
    wake(
        &mut worker,
        &conn,
        &mut windows,
        vec![OverlayMsg::LimitedChanged],
        inside,
    );
    assert_eq!(windows.opened_labels(), vec![DRAFT_NOTES.to_string()]);
    let (_, event) = windows
        .emitted()
        .into_iter()
        .rfind(|(label, _)| label == DRAFT_NOTES)
        .expect("the draft notes are pushed");
    let view = &event.payload["view"];
    assert_eq!(view["state"], "finished");
    assert_eq!(view["currentOrdinal"], 42);
    // The whole draft, oldest first, so notes can be added afterwards.
    let picks = view["picks"].as_array().expect("picks");
    assert_eq!(picks.len(), 42);
    assert_eq!(picks[0]["label"], "P1p1");
    assert_eq!(picks[41]["label"], "P3p14");
    // The grace timer is due 20 s later.
    assert_eq!(
        worker.next_wake(inside),
        Some(Duration::from_millis(20_001))
    );

    // Past grace: the timer's wake closes it, no message needed.
    wake(
        &mut worker,
        &conn,
        &mut windows,
        Vec::new(),
        ended_at + 31 * SECOND,
    );
    assert_eq!(windows.closed(), vec![DRAFT_NOTES.to_string()]);
    // The draft's timer is done. The fixture's league run, finished later,
    // keeps a timer of its own: the end of its minutes on the league card.
    let league_card_ends = Duration::from_secs(60);
    assert!(
        worker
            .next_wake(ended_at + 31 * SECOND)
            .is_none_or(|wait| wait > league_card_ends)
    );
}

#[test]
fn completes_the_wake_when_the_window_api_is_unreachable() {
    let (_dir, conn) = db("overlay-worker-draft-unreachable");
    draft(&conn, 1, "picking", None, NOW);
    let mut worker = worker(settings());
    let mut windows = FakeWindows {
        fail: true,
        ..FakeWindows::default()
    };
    wake(
        &mut worker,
        &conn,
        &mut windows,
        vec![OverlayMsg::LimitedChanged],
        NOW,
    );
    assert!(windows.open.is_empty());
}

// 0.x SyncDraftNotesWindowVisibilityTest window cases.

#[test]
fn keeps_the_window_open_for_thirty_seconds_after_the_draft_finishes() {
    let (_dir, conn) = db("overlay-worker-draft-grace");
    draft(&conn, 1, "finished", Some(NOW - 29 * SECOND), NOW);
    let mut worker = worker(settings());
    let mut windows = with_open(&[DRAFT_NOTES]);
    wake(&mut worker, &conn, &mut windows, Vec::new(), NOW);
    assert!(windows.closed().is_empty());
    assert!(windows.opened().is_empty(), "already open");
}

#[test]
fn closes_the_window_once_the_grace_period_after_finishing_has_passed() {
    let (_dir, conn) = db("overlay-worker-draft-grace-over");
    draft(&conn, 1, "finished", Some(NOW - 31 * SECOND), NOW);
    let mut worker = worker(settings());
    let mut windows = with_open(&[DRAFT_NOTES]);
    wake(&mut worker, &conn, &mut windows, Vec::new(), NOW);
    assert_eq!(windows.closed(), vec![DRAFT_NOTES.to_string()]);
}

#[test]
fn closes_the_window_when_the_draft_is_abandoned() {
    let (_dir, conn) = db("overlay-worker-draft-abandoned-open");
    draft(&conn, 1, "abandoned", Some(NOW), NOW);
    let mut worker = worker(settings());
    let mut windows = with_open(&[DRAFT_NOTES]);
    wake(&mut worker, &conn, &mut windows, Vec::new(), NOW);
    assert_eq!(windows.closed(), vec![DRAFT_NOTES.to_string()]);
}

#[test]
fn closes_an_open_window_when_the_setting_is_turned_off_mid_draft() {
    let (_dir, conn) = db("overlay-worker-draft-toggle");
    draft(&conn, 1, "picking", None, NOW);
    let mut worker = worker(settings());
    let mut windows = FakeWindows::default();
    wake(&mut worker, &conn, &mut windows, Vec::new(), NOW);
    assert_eq!(draft_opens(&windows), 1);
    let off = OverlayMsg::SettingsChanged {
        settings: Box::new(Settings {
            draft_notes_window: false,
            ..settings()
        }),
    };
    wake(&mut worker, &conn, &mut windows, vec![off], NOW);
    assert_eq!(windows.closed(), vec![DRAFT_NOTES.to_string()]);
}

#[test]
fn does_not_issue_a_close_when_nothing_is_open() {
    let (_dir, conn) = db("overlay-worker-draft-nothing-open");
    draft(&conn, 1, "abandoned", Some(NOW), NOW);
    let mut worker = worker(settings());
    let mut windows = FakeWindows::default();
    wake(&mut worker, &conn, &mut windows, Vec::new(), NOW);
    assert!(windows.closed().is_empty());
    assert!(!windows.calls.contains(&Call::Saved));
}

#[test]
fn logs_and_swallows_a_window_api_failure_instead_of_throwing() {
    let (_dir, conn) = db("overlay-worker-draft-api-down");
    draft(&conn, 1, "picking", None, NOW);
    let mut worker = worker(settings());
    let mut windows = FakeWindows {
        fail: true,
        ..FakeWindows::default()
    };
    wake(&mut worker, &conn, &mut windows, Vec::new(), NOW);
    assert!(windows.opened().is_empty());

    // Not remembered as done: once the API answers, the next wake opens it.
    windows.fail = false;
    wake(&mut worker, &conn, &mut windows, Vec::new(), NOW);
    assert_eq!(windows.opened_labels(), vec![DRAFT_NOTES.to_string()]);
}

// ---- Flips, rebuilds, positions, platforms and pushes ----

#[test]
fn a_user_closed_overlay_stays_closed_until_the_next_match() {
    let (_dir, conn) = db("overlay-worker-user-closed");
    let mut worker = worker(settings());
    let mut windows = FakeWindows::default();
    wake(
        &mut worker,
        &conn,
        &mut windows,
        playing("q-first", 10),
        NOW,
    );
    windows.user_closes(GAME_OVERLAY);
    // More frames of the same match: it stays closed.
    for _ in 0..3 {
        wake(
            &mut worker,
            &conn,
            &mut windows,
            playing("q-first", 10),
            NOW,
        );
    }
    assert_eq!(game_opens(&windows), 1);
    // The next match opens it again, even with no gap between the two.
    let mut messages = vec![ended("q-first")];
    messages.extend(playing("q-second", 20));
    wake(&mut worker, &conn, &mut windows, messages, NOW);
    assert_eq!(game_opens(&windows), 2);
}

#[test]
fn a_user_closed_overlay_stays_closed_after_an_unrelated_settings_save() {
    let (_dir, conn) = db("overlay-worker-unrelated-save");
    draft(&conn, 1, "picking", None, NOW);
    let on = Settings {
        league_window: true,
        ..settings()
    };
    let mut worker = worker(on.clone());
    let mut windows = FakeWindows::default();
    wake(&mut worker, &conn, &mut windows, playing("q-live", 10), NOW);
    assert_eq!(windows.opened().len(), 3);
    for label in [GAME_OVERLAY, DRAFT_NOTES, LEAGUE_OVERLAY] {
        windows.user_closes(label);
    }
    // A save on another settings page, and a game overlay section toggle.
    let unrelated = Settings {
        close_to_tray: !on.close_to_tray,
        overlay_show_reveals: false,
        overlay_size: OverlaySize::Compact,
        ..on.clone()
    };
    wake(
        &mut worker,
        &conn,
        &mut windows,
        vec![OverlayMsg::SettingsChanged {
            settings: Box::new(unrelated),
        }],
        NOW,
    );
    assert_eq!(windows.opened().len(), 3, "nothing reopened");
}

#[test]
fn only_a_windows_own_toggle_reopens_it() {
    let (_dir, conn) = db("overlay-worker-own-toggle");
    draft(&conn, 1, "picking", None, NOW);
    let mut worker = worker(settings());
    let mut windows = FakeWindows::default();
    wake(&mut worker, &conn, &mut windows, playing("q-live", 10), NOW);
    windows.user_closes(GAME_OVERLAY);
    windows.user_closes(DRAFT_NOTES);
    // The league window switched on: only it opens.
    let league_on = Settings {
        league_window: true,
        ..settings()
    };
    wake(
        &mut worker,
        &conn,
        &mut windows,
        vec![OverlayMsg::SettingsChanged {
            settings: Box::new(league_on.clone()),
        }],
        NOW,
    );
    assert_eq!(
        windows.opened_labels().last().map(String::as_str),
        Some(LEAGUE_OVERLAY)
    );
    assert_eq!(game_opens(&windows), 1);
    assert_eq!(draft_opens(&windows), 1);
    // The game overlay switched off and on: it opens again.
    for on in [false, true] {
        wake(
            &mut worker,
            &conn,
            &mut windows,
            vec![OverlayMsg::SettingsChanged {
                settings: Box::new(Settings {
                    game_overlay: on,
                    ..league_on.clone()
                }),
            }],
            NOW,
        );
    }
    assert_eq!(game_opens(&windows), 2);
    assert_eq!(draft_opens(&windows), 1, "the draft window stays closed");
}

#[test]
fn a_tracker_restart_forgets_the_live_matches() {
    let (_dir, conn) = db("overlay-worker-restart");
    let mut worker = worker(settings());
    let mut windows = FakeWindows::default();
    wake(&mut worker, &conn, &mut windows, playing("q-live", 10), NOW);
    wake(
        &mut worker,
        &conn,
        &mut windows,
        vec![OverlayMsg::Restarting],
        NOW,
    );
    assert_eq!(windows.closed(), vec![GAME_OVERLAY.to_string()]);
    assert_eq!(
        from_tracker(&TrackerEvent::Restarting {
            reason: "disk full".to_string()
        }),
        Some(OverlayMsg::Restarting)
    );
}

#[test]
fn a_rebuild_never_opens_an_overlay() {
    let (_dir, conn) = db("overlay-worker-rebuild");
    let mut worker = worker(settings());
    let mut windows = FakeWindows::default();
    // A rebuild or a backlog pushes match views but announces no live
    // events: a match left in progress by an earlier session stays shut.
    wake(
        &mut worker,
        &conn,
        &mut windows,
        vec![updated(live("q-stale", 10)), updated(live("q-other", 5))],
        NOW,
    );
    assert!(windows.opened().is_empty());
    // Its live frames open it.
    wake(
        &mut worker,
        &conn,
        &mut windows,
        vec![announced(&["q-stale"])],
        NOW,
    );
    assert_eq!(windows.opened_labels(), vec![GAME_OVERLAY.to_string()]);
}

fn rect(x: i32, y: i32, width: u32, height: u32) -> Rect {
    Rect {
        x,
        y,
        width,
        height,
    }
}

/// Every overlay wanted: a live match, a live draft and the league window.
fn everything(label: &str) -> (TestDir, Connection, Worker) {
    let (dir, conn) = db(label);
    draft(&conn, 1, "picking", None, NOW);
    let worker = worker(Settings {
        league_window: true,
        ..settings()
    });
    (dir, conn, worker)
}

#[test]
fn a_reopened_window_restores_its_saved_position() {
    let (_dir, conn, mut worker) = everything("overlay-worker-restore");
    let mut windows = FakeWindows {
        monitors: Monitors {
            all: vec![primary(), rect(1920, 0, 2560, 1440)],
            primary: Some(primary()),
        },
        ..FakeWindows::default()
    };
    wake(&mut worker, &conn, &mut windows, playing("q-live", 10), NOW);
    let labels = [GAME_OVERLAY, DRAFT_NOTES, LEAGUE_OVERLAY];
    let mut placed = Vec::new();
    for (index, label) in labels.iter().enumerate() {
        let to = rect(2000 + 400 * index as i32, 300, 320, 400);
        windows.user_moves(label, to);
        windows.user_closes(label);
        placed.push(to);
    }
    // Each window's own setting switched off and on again reopens it.
    let all = |on: bool| OverlayMsg::SettingsChanged {
        settings: Box::new(Settings {
            game_overlay: on,
            draft_notes_window: on,
            league_window: on,
            ..settings()
        }),
    };
    let before = windows.opened().len();
    wake(&mut worker, &conn, &mut windows, vec![all(false)], NOW);
    wake(&mut worker, &conn, &mut windows, vec![all(true)], NOW);
    let reopened: Vec<(String, Option<Placement>)> = windows.opened()[before..].to_vec();
    let mut expected: Vec<(String, Option<Placement>)> = labels
        .iter()
        .zip(placed)
        .map(|(label, at)| (label.to_string(), Some(Placement::Restore(at))))
        .collect();
    expected.sort_by(|a, b| a.0.cmp(&b.0));
    let mut reopened_sorted = reopened.clone();
    reopened_sorted.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(reopened_sorted, expected);
}

#[test]
fn a_programmatic_close_saves_the_position() {
    let (_dir, conn) = db("overlay-worker-programmatic-close");
    let mut worker = worker(settings());
    let mut windows = FakeWindows::default();
    wake(
        &mut worker,
        &conn,
        &mut windows,
        playing("q-first", 10),
        NOW,
    );
    let moved = rect(700, 50, 320, 640);
    windows.user_moves(GAME_OVERLAY, moved);
    wake(
        &mut worker,
        &conn,
        &mut windows,
        vec![ended("q-first")],
        NOW,
    );
    let last_two: Vec<Call> = windows.calls.iter().rev().take(2).rev().cloned().collect();
    assert_eq!(
        last_two,
        vec![Call::Saved, Call::Closed(GAME_OVERLAY.to_string())]
    );
    wake(
        &mut worker,
        &conn,
        &mut windows,
        playing("q-second", 20),
        NOW,
    );
    assert_eq!(
        windows.opened().last(),
        Some(&(GAME_OVERLAY.to_string(), Some(Placement::Restore(moved))))
    );
}

#[test]
fn a_window_is_centred_only_when_its_monitor_is_gone() {
    let (_dir, conn) = db("overlay-worker-centred");
    let mut worker = worker(settings());
    let mut windows = FakeWindows::default();
    // Never opened: centred on the primary monitor.
    wake(
        &mut worker,
        &conn,
        &mut windows,
        playing("q-first", 10),
        NOW,
    );
    assert_eq!(
        windows.opened()[0],
        (
            GAME_OVERLAY.to_string(),
            Some(Placement::Centre(Some(primary())))
        )
    );
    // Left on a second monitor that is then unplugged: centred again.
    windows.user_moves(GAME_OVERLAY, rect(-1800, 200, 320, 640));
    wake(
        &mut worker,
        &conn,
        &mut windows,
        vec![ended("q-first")],
        NOW,
    );
    wake(
        &mut worker,
        &conn,
        &mut windows,
        playing("q-second", 20),
        NOW,
    );
    assert_eq!(
        windows.opened()[1],
        (
            GAME_OVERLAY.to_string(),
            Some(Placement::Centre(Some(primary())))
        )
    );
    // Moved on the primary monitor: restored there.
    windows.user_moves(GAME_OVERLAY, rect(1500, 100, 320, 640));
    wake(
        &mut worker,
        &conn,
        &mut windows,
        vec![ended("q-second")],
        NOW,
    );
    wake(
        &mut worker,
        &conn,
        &mut windows,
        playing("q-third", 30),
        NOW,
    );
    assert_eq!(
        windows.opened()[2],
        (
            GAME_OVERLAY.to_string(),
            Some(Placement::Restore(rect(1500, 100, 320, 640)))
        )
    );
}

#[test]
fn no_overlay_opens_on_native_wayland() {
    let (_dir, conn) = db("overlay-worker-wayland");
    draft(&conn, 1, "picking", None, NOW);
    let mut worker = Worker::new(
        Settings {
            league_window: true,
            ..settings()
        },
        false,
        false,
    );
    let mut windows = FakeWindows::default();
    wake(&mut worker, &conn, &mut windows, playing("q-live", 10), NOW);
    wake(
        &mut worker,
        &conn,
        &mut windows,
        vec![OverlayMsg::LimitedChanged],
        NOW,
    );
    assert!(windows.opened().is_empty());
}

#[test]
fn the_league_window_opens_with_its_setting_at_the_card_size() {
    let (_dir, conn) = db("overlay-worker-league");
    let mut worker = worker(Settings {
        league_window: true,
        overlay_size: OverlaySize::Compact,
        ..settings()
    });
    let mut windows = FakeWindows::default();
    wake(&mut worker, &conn, &mut windows, Vec::new(), NOW);
    let specs = windows.opened_specs();
    assert_eq!(specs.len(), 1);
    assert_eq!(specs[0].label, LEAGUE_OVERLAY);
    assert_eq!((specs[0].width, specs[0].height), (240.0, 58.0));
}

#[test]
fn the_league_window_grows_before_the_strip_and_shrinks_after_it() {
    let (_dir, conn) = db("overlay-worker-league-strip");
    let mut worker = worker(Settings {
        league_window: true,
        ..settings()
    });
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
    worker.update(&conn, &mut windows, NOW);
    // The league window's resizes and pushes, in order.
    let league_calls = |windows: &FakeWindows| -> Vec<String> {
        windows
            .calls
            .iter()
            .filter_map(|call| match call {
                Call::Resized(label, width, height) if label == LEAGUE_OVERLAY => {
                    Some(format!("resize {width}x{height}"))
                }
                Call::Emitted(label, _) if label == LEAGUE_OVERLAY => Some("push".to_string()),
                _ => None,
            })
            .collect()
    };
    assert_eq!(league_calls(&windows), vec!["push"], "no strip, no resize");
    // A card with another event on its strip: the window grows first, so
    // the strip is never clipped.
    let mut with_strip = worker.league_state().expect("the idle card").clone();
    with_strip.size = OverlaySize::Compact;
    with_strip.other = Some(overlay::OtherEvent {
        kind: overlay::EventKind::Tournament,
        name: "Modern Challenge 64".to_string(),
        status: overlay::LeagueStatus::WaitingForRound,
        record: None,
        live_match: None,
        tournament: None,
    });
    windows.calls.clear();
    worker.show_league_card(&mut windows, with_strip);
    assert_eq!(league_calls(&windows), vec!["resize 300x96", "push"]);
    // The next card has none: pushed first, then the window shrinks.
    windows.calls.clear();
    wake(
        &mut worker,
        &conn,
        &mut windows,
        vec![OverlayMsg::LeaguesChanged],
        NOW,
    );
    assert_eq!(league_calls(&windows), vec!["push", "resize 300x80"]);
    // Still none: nothing more.
    windows.calls.clear();
    wake(
        &mut worker,
        &conn,
        &mut windows,
        vec![OverlayMsg::LeaguesChanged],
        NOW,
    );
    assert!(league_calls(&windows).is_empty());
}

#[test]
fn an_unchanged_view_is_not_emitted_again() {
    let (_dir, conn) = db("overlay-worker-unchanged");
    let mut worker = worker(settings());
    let mut windows = FakeWindows::default();
    wake(&mut worker, &conn, &mut windows, playing("q-live", 10), NOW);
    let game_pushes = |windows: &FakeWindows| {
        windows
            .emitted()
            .into_iter()
            .filter(|(label, _)| label == GAME_OVERLAY)
            .count()
    };
    let first = game_pushes(&windows);
    assert_eq!(first, 1, "the first view");
    for _ in 0..3 {
        wake(
            &mut worker,
            &conn,
            &mut windows,
            vec![announced(&["q-live"])],
            NOW,
        );
    }
    assert_eq!(game_pushes(&windows), 1, "frames that change nothing");
    // A change (a section switched off by a settings save) is pushed, with a
    // newer revision.
    let collapsed = OverlayMsg::SettingsChanged {
        settings: Box::new(Settings {
            overlay_show_reveals: false,
            ..settings()
        }),
    };
    wake(&mut worker, &conn, &mut windows, vec![collapsed], NOW);
    let revisions: Vec<u32> = windows
        .emitted()
        .into_iter()
        .filter(|(label, _)| label == GAME_OVERLAY)
        .filter_map(|(_, event)| event.payload["revision"].as_u64())
        .map(|revision| revision as u32)
        .collect();
    assert_eq!(revisions, vec![1, 2]);
    assert_eq!(worker.game_state().map(|state| state.revision), Some(2));
}

#[test]
fn the_worker_survives_a_failing_window_call() {
    let (dir, conn) = db("overlay-worker-survives");
    drop(conn);
    let path = dir.join("mymtgo.sqlite");
    let (sender, inbox) = mpsc::channel();
    let thread = std::thread::spawn(move || {
        let conn = store::open_reader(&path).expect("reader");
        let mut worker = worker(settings());
        let mut windows = FakeWindows {
            fail: true,
            ..FakeWindows::default()
        };
        run_overlay_worker(&inbox, &mut worker, &conn, &mut windows, &|| NOW);
        windows
    });
    for message in playing("q-live", 10) {
        sender.send(message).expect("send");
    }
    sender.send(announced(&["q-live"])).expect("send");
    sender.send(OverlayMsg::Stop).expect("stop");
    let windows = thread.join().expect("the worker ended normally");
    assert!(windows.opened().is_empty());
    assert!(
        !windows.emitted().is_empty(),
        "it went on computing and pushing"
    );
}

#[test]
fn the_grace_timer_wakes_the_worker_without_a_message() {
    let (dir, conn) = db("overlay-worker-timer");
    // Finished so that its grace ends 50 ms after the worker starts.
    let base = NOW;
    draft(&conn, 1, "finished", Some(base - 30 * SECOND + 50), base);
    drop(conn);
    let path = dir.join("mymtgo.sqlite");
    let started = Instant::now();
    let now = Arc::new(AtomicI64::new(base));
    let clock_now = Arc::clone(&now);
    let (sender, inbox) = mpsc::channel();
    let thread = std::thread::spawn(move || {
        let conn = store::open_reader(&path).expect("reader");
        let mut worker = worker(settings());
        let mut windows = FakeWindows::default();
        let clock = move || {
            let fake = base + started.elapsed().as_millis() as i64;
            clock_now.store(fake, Ordering::SeqCst);
            fake
        };
        run_overlay_worker(&inbox, &mut worker, &conn, &mut windows, &clock);
        windows
    });
    std::thread::sleep(Duration::from_millis(400));
    sender.send(OverlayMsg::Stop).expect("stop");
    let windows = thread.join().expect("join");
    assert_eq!(windows.opened_labels(), vec![DRAFT_NOTES.to_string()]);
    assert_eq!(windows.closed(), vec![DRAFT_NOTES.to_string()]);
}

// ---- The bridge's forwarding ----

#[test]
fn the_bridge_forwards_what_the_overlay_needs() {
    let view = crate::summary::testing::view("q-fwd", 1_000, mtgo_match::MatchState::InProgress);
    let message = from_tracker(&TrackerEvent::MatchUpdated {
        view: view.clone(),
        revision: 1,
        cause: None,
        manual: false,
        platform: store::Platform::Mtgo,
        opponent_name: None,
    });
    let Some(OverlayMsg::MatchUpdated { live, league }) = message else {
        panic!("a match update: {message:?}");
    };
    assert!(!league, "no league token");
    let mut in_tournament = view.clone();
    in_tournament.tournament_event_id = Some(12_858_947);
    assert!(
        matches!(
            from_tracker(&TrackerEvent::MatchUpdated {
                view: in_tournament,
                revision: 1,
                cause: None,
                manual: false,
                platform: store::Platform::Mtgo,
                opponent_name: None,
            }),
            Some(OverlayMsg::MatchUpdated { league: true, .. })
        ),
        "a tournament match changes the event card"
    );
    let mut in_league = view.clone();
    in_league.league_token = Some("q-league".to_string());
    assert!(matches!(
        from_tracker(&TrackerEvent::MatchUpdated {
            view: in_league,
            revision: 1,
            cause: None,
            manual: false,
            platform: store::Platform::Mtgo,
            opponent_name: None,
        }),
        Some(OverlayMsg::MatchUpdated { league: true, .. })
    ));
    assert_eq!(live.token, "q-fwd");
    assert_eq!(live.state, "in_progress");
    assert_eq!(live.source, "log");
    assert_eq!(live.game_count, view.games.len() as u32);
    assert_eq!(live.started_at, view.started_at_ms);
    let manual = TrackerEvent::MatchUpdated {
        view,
        revision: 1,
        cause: None,
        manual: true,
        platform: store::Platform::Mtgo,
        opponent_name: None,
    };
    assert_eq!(from_tracker(&manual), None, "a manual match has no overlay");
    let arena = TrackerEvent::MatchUpdated {
        view: crate::summary::testing::view("q-arena", 1_000, mtgo_match::MatchState::InProgress),
        revision: 1,
        cause: None,
        manual: false,
        platform: store::Platform::Arena,
        opponent_name: Some("Opponent".to_string()),
    };
    assert_eq!(from_tracker(&arena), None, "an Arena match has no overlay");
    let at = Instant::now();
    assert_eq!(
        from_tracker(&TrackerEvent::LiveEventsStored {
            tokens: vec!["q-fwd".to_string()],
            detected_at: at,
        }),
        Some(OverlayMsg::LiveEventsStored {
            tokens: vec!["q-fwd".to_string()],
            detected_at: at,
        })
    );
    for (event, message) in [
        (TrackerEvent::DecksChanged, OverlayMsg::DecksChanged),
        (TrackerEvent::LeaguesChanged, OverlayMsg::LeaguesChanged),
        (
            TrackerEvent::TournamentsChanged,
            OverlayMsg::TournamentsChanged,
        ),
        (TrackerEvent::LimitedChanged, OverlayMsg::LimitedChanged),
        // The live read's candidates join card rows (U12 review I2).
        (TrackerEvent::CardsChanged, OverlayMsg::CardsChanged),
    ] {
        assert_eq!(from_tracker(&event), Some(message));
    }
    assert_eq!(
        from_tracker(&TrackerEvent::CardStubsAdded { count: 1 }),
        None
    );
}

// ---- The event card's tournaments (event overlay plan B6) ----

/// A stored standing of account 1 for `event`, waiting for round 5.
fn store_standing(conn: &Connection, event: i64, matches_left: i64) {
    store::tournament_sync::write_standing(
        conn,
        &store::tournament_sync::StandingRow {
            account_id: 1,
            mtgo_event_id: event,
            token: format!("q-token-{event}"),
            name: Some("Modern Challenge 64".to_string()),
            format: Some("CMODERN".to_string()),
            cut_size: Some(8),
            swiss_rounds: Some(6),
            round_minutes: Some(50),
            starts_at: Some(NOW - 3 * 60 * MINUTE),
            status: leagues::standing::StandingStatus::WaitingForRound,
            round: Some(4),
            stage: leagues::standing::StandingStage::Swiss,
            in_playoffs: false,
            round_started_at: Some(NOW - 20 * MINUTE),
            round_result_at: None,
            rank: Some(12),
            players: Some(64),
            matches_left: Some(matches_left),
            byes: 0,
            finish: None,
            place: None,
            finished_at: None,
            last_transition_at: NOW - 5 * MINUTE,
            last_event_at: NOW - MINUTE,
        },
    )
    .expect("standing");
}

/// The league card pushes to the league window.
fn league_pushes(windows: &FakeWindows) -> usize {
    windows
        .calls
        .iter()
        .filter(|call| matches!(call, Call::Emitted(label, _) if label == LEAGUE_OVERLAY))
        .count()
}

#[test]
fn tournaments_changed_pushes_the_tournament_card_once_per_wake() {
    let (_dir, conn) = db("overlay-worker-tournaments");
    let mut worker = worker(settings());
    let mut windows = FakeWindows::default();
    wake(&mut worker, &conn, &mut windows, Vec::new(), NOW);
    windows.calls.clear();

    store_standing(&conn, 700, 12);
    wake(
        &mut worker,
        &conn,
        &mut windows,
        vec![OverlayMsg::TournamentsChanged],
        NOW,
    );
    assert_eq!(league_pushes(&windows), 1);
    assert_eq!(
        worker.league_state().map(|state| state.status),
        Some(overlay::LeagueStatus::WaitingForRound)
    );

    // 150 closes in one tick: one push of the new count.
    windows.calls.clear();
    store_standing(&conn, 700, 1);
    let closes = (0..150).map(|_| OverlayMsg::TournamentsChanged).collect();
    wake(&mut worker, &conn, &mut windows, closes, NOW);
    assert_eq!(league_pushes(&windows), 1, "merged into one push");
    assert_eq!(
        worker
            .league_state()
            .and_then(|state| state.tournament)
            .and_then(|block| block.matches_left),
        Some(1)
    );

    // Nothing changed: nothing pushed.
    windows.calls.clear();
    wake(
        &mut worker,
        &conn,
        &mut windows,
        vec![OverlayMsg::TournamentsChanged],
        NOW,
    );
    assert_eq!(league_pushes(&windows), 0, "an unchanged card");
}

#[test]
fn a_tournament_match_update_and_its_live_events_refresh_the_card() {
    let (_dir, conn) = db("overlay-worker-tournament-match");
    let mut worker = worker(settings());
    let mut windows = FakeWindows::default();
    store_standing(&conn, 700, 12);
    wake(&mut worker, &conn, &mut windows, Vec::new(), NOW);
    assert_eq!(
        worker.league_state().map(|state| state.status),
        Some(overlay::LeagueStatus::WaitingForRound)
    );

    // Round 5 pairs and its match starts: the match update moves the card.
    conn.execute(
        "UPDATE tournament_standings SET status = 'playing', round = 5, matches_left = NULL",
        [],
    )
    .expect("paired");
    conn.execute(
        "INSERT INTO matches (token, account_id, source, format, state, outcome, started_at,
             last_activity_at, revision, derived_version, tournament_event_id, tournament_round)
         VALUES ('q-t700-r5', 1, 'log', 'CMODERN', 'in_progress', 'unknown', ?1, ?1, 1, 1,
             700, 5)",
        [NOW - MINUTE],
    )
    .expect("the round's match");
    windows.calls.clear();
    let mut round_match = live("q-t700-r5", NOW - MINUTE);
    round_match.format = Some("CMODERN".to_string());
    wake(
        &mut worker,
        &conn,
        &mut windows,
        vec![OverlayMsg::MatchUpdated {
            live: Box::new(round_match),
            league: true,
        }],
        NOW,
    );
    assert_eq!(league_pushes(&windows), 1);
    assert_eq!(
        worker.league_state().map(|state| state.status),
        Some(overlay::LeagueStatus::InGame)
    );

    // Its live events (a sideboarding state change) are the card's too.
    windows.calls.clear();
    worker.handle(&conn, announced(&["q-t700-r5"]));
    assert!(worker.league_dirty, "a live tournament match's events");
    // Another match's events are not.
    worker.league_dirty = false;
    worker.handle(&conn, announced(&["q-other"]));
    assert!(!worker.league_dirty);
}

// ---- The live fixture match ----

#[test]
fn a_tournament_rounds_live_match_names_the_opponents_archetype() {
    let (_dir, conn) = db("overlay-worker-tournament-opponent");
    let mut worker = worker(Settings {
        game_overlay: false,
        ..settings()
    });
    let mut windows = FakeWindows::default();
    store_standing(&conn, 700, 12);
    conn.execute(
        "INSERT INTO opponents (id, username, first_seen_at) VALUES (1, 'Qrival', 0)",
        [],
    )
    .expect("opponent");
    conn.execute(
        "INSERT INTO archetypes (id, uuid, name, format, source, created_at, updated_at)
         VALUES (101, 'q-tempo', 'Dimir Tempo', 'modern', 'downloaded', 1, 1)",
        [],
    )
    .expect("archetype");
    wake(&mut worker, &conn, &mut windows, Vec::new(), NOW);

    conn.execute(
        "UPDATE tournament_standings SET status = 'playing', round = 5, matches_left = NULL",
        [],
    )
    .expect("paired");
    conn.execute(
        "INSERT INTO matches (id, token, account_id, opponent_id, source, format, state, outcome,
             started_at, last_activity_at, revision, derived_version, tournament_event_id,
             tournament_round)
         VALUES (1, 'q-t700-r5', 1, 1, 'log', 'CMODERN', 'in_progress', 'unknown', ?1, ?1, 1, 1,
             700, 5)",
        [NOW - MINUTE],
    )
    .expect("the round's match");
    conn.execute(
        "INSERT INTO match_archetypes (match_id, is_opponent, archetype_id, confidence, manual)
         VALUES (1, 1, 101, 1.0, 1)",
        [],
    )
    .expect("pick");
    let mut round_match = live("q-t700-r5", NOW - MINUTE);
    round_match.format = Some("CMODERN".to_string());
    wake(
        &mut worker,
        &conn,
        &mut windows,
        vec![OverlayMsg::MatchUpdated {
            live: Box::new(round_match),
            league: true,
        }],
        NOW,
    );

    let card = worker.league_state().expect("a card");
    assert_eq!(card.status, overlay::LeagueStatus::InGame);
    assert_eq!(
        card.live_match
            .as_ref()
            .and_then(|live| live.opponent_archetype.as_deref()),
        Some("Dimir Tempo")
    );
}

const HOBBIT_GAME_TWO: &str = "Game Play Status Update for Game ID: 959762636,";
const HOBBIT_LIVE_MATCH_ID: i64 = 289_328_158;

/// `draft_hobbit` with its draft matches turned into a constructed format
/// of the same length (offsets unchanged), so the game overlay shows them.
fn constructed_hobbit() -> Fixture {
    let mut fixture = load_fixture("draft_hobbit");
    let from = b"PlayFormatCd=DHOBHOBHOB";
    let to = b"PlayFormatCd=CPREMODERN";
    let mut at = 0;
    while let Some(found) = fixture.bytes[at..]
        .windows(from.len())
        .position(|window| window == from)
    {
        let start = at + found;
        fixture.bytes[start..start + to.len()].copy_from_slice(to);
        at = start + to.len();
    }
    fixture
}

fn hobbit_config(dir: &TestDir, fixture: &Fixture) -> TrackerConfig {
    TrackerConfig {
        db_path: dir.join("data/mymtgo.sqlite"),
        timezone: fixture.meta.tz(),
        os_timezone: None,
        log_overrides: vec![dir.join("logs")],
        arena_log_overrides: vec![dir.join("arena/Player.log")],
        mtgo_data_folder: Some(dir.join("mtgo-data")),
        replay_day_anchor: Some(fixture.meta.day_anchor()),
        watching: true,
        arena_enabled: true,
        endstep_enabled: false,
    }
}

/// Tick until every byte is read; the events the ticks pushed.
fn tick_until_read(engine: &mut Engine, mut now_ms: i64) -> Vec<TrackerEvent> {
    let mut events = Vec::new();
    for _ in 0..20_000 {
        let tick = engine.tick(now_ms).expect("tick");
        events.extend(tick.events);
        now_ms += 1;
        if !tick.more {
            break;
        }
    }
    events
}

/// The offset of the start of the line holding the `n`th (from 1)
/// occurrence of `needle`.
fn nth_line_start(bytes: &[u8], needle: &str, n: usize) -> usize {
    let mut from = 0;
    let mut found = 0;
    for _ in 0..n {
        let at = bytes[from..]
            .windows(needle.len())
            .position(|window| window == needle.as_bytes())
            .expect("the needle occurs often enough");
        found = from + at;
        from = found + needle.len();
    }
    bytes[..found]
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(0, |newline| newline + 1)
}

/// The newest event time before `cut`: "now" for a live tracker there.
fn time_at_cut(fixture: &Fixture, cut: usize, label: &str) -> i64 {
    let dir = TestDir::new(label);
    dir.write("logs/mtgo.log", &fixture.bytes[..cut]);
    let config = hobbit_config(&dir, fixture);
    let mut engine = Engine::open(
        config.clone(),
        Env::current(),
        Arc::default(),
        Vec::new(),
        fixture.meta.now_ms(),
    )
    .expect("engine");
    tick_until_read(&mut engine, fixture.meta.now_ms());
    engine.flush().expect("flush");
    drop(engine);
    store::open_reader(&config.db_path)
        .expect("reader")
        .query_row("SELECT MAX(logged_at) FROM log_events", [], |row| {
            row.get(0)
        })
        .expect("time")
}

#[test]
fn the_game_overlay_shows_the_live_fixture_match() {
    let fixture = constructed_hobbit();
    let cut = nth_line_start(&fixture.bytes, HOBBIT_GAME_TWO, 10);
    let now = time_at_cut(&fixture, cut, "overlay-worker-fixture-time") + 1_000;
    let dir = TestDir::new("overlay-worker-fixture");
    dir.write("logs/mtgo.log", &fixture.bytes[..cut]);
    let config = hobbit_config(&dir, &fixture);
    let mut engine = Engine::open(
        config.clone(),
        Env::current(),
        Arc::default(),
        Vec::new(),
        now,
    )
    .expect("engine");
    let events = tick_until_read(&mut engine, now);
    engine.flush().expect("flush");
    let conn = store::open_writer(&config.db_path).expect("writer");
    let token: String = conn
        .query_row(
            "SELECT token FROM matches WHERE mtgo_match_id = ?1",
            [HOBBIT_LIVE_MATCH_ID],
            |row| row.get(0),
        )
        .expect("the live match");
    // The match also links a deck version with a card it never played: the
    // current game's own list wins (0.x "prefers per-game deck_json over the
    // match deck version").
    conn.execute_batch(
        "INSERT INTO decks (id, mtgo_id, name, format, created_at, updated_at, source)
             VALUES (900, 'q-deck', 'Qdeck', 'CPREMODERN', 1, 1, 'file');
         INSERT INTO deck_versions (id, deck_id, signature, modified_at)
             VALUES (900, 900, '', 1);",
    )
    .expect("deck");
    let signature = decks::signature(&[decks::DeckCard {
        catalog_id: 999_999,
        quantity: 60,
        sideboard: false,
    }]);
    conn.execute(
        "UPDATE deck_versions SET signature = ?1 WHERE id = 900",
        [&signature],
    )
    .expect("signature");
    conn.execute(
        "UPDATE matches SET deck_version_id = 900 WHERE token = ?1",
        [&token],
    )
    .expect("link");

    let mut worker = worker(settings());
    let mut windows = FakeWindows::default();
    let messages: Vec<OverlayMsg> = events.iter().filter_map(from_tracker).collect();
    assert!(
        messages
            .iter()
            .any(|message| matches!(message, OverlayMsg::LiveEventsStored { tokens, .. } if tokens.contains(&token))),
        "the live match is announced"
    );
    wake(&mut worker, &conn, &mut windows, messages, now);
    assert_eq!(game_opens(&windows), 1);
    let state = worker.game_state().expect("a game view");
    let view = state.view;
    assert!(view.has_match);
    assert!(view.has_deck);
    assert!(view.format.is_some());
    assert!(!view.is_sideboarding, "game 2 is under way");
    let odds = view.draw_odds.expect("draw odds");
    assert!(odds.library_size > 0);
    assert!(odds.live_library_count > 0);
    assert!(
        odds.cards.iter().all(|card| card.catalog_id != 999_999),
        "the game's own list, not the version"
    );
    assert!(!view.reveals.expect("reveals").is_empty());
    drop(engine);
}

/// Overlay spec 14: log bytes detected to the game overlay's push, p50 < 20
/// ms and p95 < 50 ms. The log player writes games 2 and 3 of a constructed
/// copy of `draft_hobbit` (and the rest of the draft) ten times faster than
/// they were played, while the tracker engine polls the file as its thread
/// does (75 ms) at the replay's own clock (a live match must not look two
/// months old). Game syncs and the league sync run meanwhile, as live. Each
/// tick's events go to the worker as the bridge hands them. Run with
/// `cargo test --release -p app --lib -- --ignored a_frame_reaches`.
#[test]
#[ignore = "speed: run with --release"]
fn a_frame_reaches_the_overlay_within_the_budget() {
    let fixture = constructed_hobbit();
    let cut = nth_line_start(&fixture.bytes, HOBBIT_GAME_TWO, 1);
    let start_ms = time_at_cut(&fixture, cut, "overlay-worker-budget-time") + 1_000;
    let dir = TestDir::new("overlay-worker-budget");
    let log_path = dir.write("logs/mtgo.log", &fixture.bytes[..cut]);
    let config = hobbit_config(&dir, &fixture);
    let mut engine = Engine::open(
        config.clone(),
        Env::current(),
        Arc::default(),
        Vec::new(),
        start_ms,
    )
    .expect("engine");
    let conn = store::open_reader(&config.db_path).expect("reader");
    let mut worker = worker(settings());
    let mut windows = FakeWindows::default();
    for event in tick_until_read(&mut engine, start_ms) {
        if let Some(message) = from_tracker(&event) {
            worker.handle(&conn, message);
        }
    }
    worker.update(&conn, &mut windows, start_ms);

    let speed = 10.0;
    let rest = fixture.bytes[cut..].to_vec();
    let player_path = log_path.clone();
    let player = std::thread::spawn(move || {
        log_player::play(&rest, &player_path, speed, |_| {}).expect("play");
    });
    let started = Instant::now();
    let mut latencies: Vec<Duration> = Vec::new();
    let mut syncs_in_flight = 0;
    let mut pushes_before = windows.emitted().len();
    let mut last_round = false;
    loop {
        let finished = player.is_finished();
        let now_ms = start_ms + (started.elapsed().as_millis() as f64 * speed) as i64;
        let tick = engine.tick(now_ms).expect("tick");
        if tick.events.iter().any(|event| {
            matches!(
                event,
                TrackerEvent::LeaguesChanged | TrackerEvent::LimitedChanged
            )
        }) {
            syncs_in_flight += 1;
        }
        let mut live = false;
        for event in &tick.events {
            if let Some(message) = from_tracker(event) {
                live = live || matches!(message, OverlayMsg::LiveEventsStored { .. });
                worker.handle(&conn, message);
            }
        }
        worker.update(&conn, &mut windows, now_ms);
        let pushes = windows.emitted().len();
        if live && pushes > pushes_before {
            latencies.extend(worker.last_push_latency());
        }
        pushes_before = pushes;
        if last_round && !tick.more {
            break;
        }
        last_round = finished;
        std::thread::sleep(Duration::from_millis(POLL_ACTIVE_MS));
    }
    player.join().expect("player");
    drop(engine);
    latencies.sort();
    assert!(latencies.len() > 50, "frames pushed: {}", latencies.len());
    let at = |share: f64| latencies[((latencies.len() - 1) as f64 * share) as usize];
    let (p50, p95) = (at(0.50), at(0.95));
    println!(
        "overlay_push: {} pushes, p50 {:?}, p95 {:?}, max {:?}, ticks with a league or limited sync: {}",
        latencies.len(),
        p50,
        p95,
        latencies.last().copied().unwrap_or_default(),
        syncs_in_flight
    );
    assert!(p50 < Duration::from_millis(20), "p50 {p50:?}");
    assert!(p95 < Duration::from_millis(50), "p95 {p95:?}");
}

#[test]
fn a_failed_league_read_clears_its_due_timer_instead_of_spinning() {
    let (_dir, conn) = db("overlay-worker-league-read-fails");
    conn.execute_batch("DROP TABLE leagues").expect("drop");
    let mut worker = worker(settings());
    worker.league_changes_at_ms = Some(NOW - 1);
    let mut windows = FakeWindows::default();
    worker.update(&conn, &mut windows, NOW);
    assert_eq!(worker.next_wake(NOW), None);
}

#[test]
fn a_failed_draft_read_clears_its_due_timer_instead_of_spinning() {
    let (_dir, conn) = db("overlay-worker-draft-read-fails");
    conn.execute_batch("DROP TABLE drafts").expect("drop");
    let mut worker = worker(settings());
    worker.grace_ends_at_ms = Some(NOW - 1);
    let mut windows = FakeWindows::default();
    worker.update(&conn, &mut windows, NOW);
    assert_eq!(worker.next_wake(NOW), None);
}
