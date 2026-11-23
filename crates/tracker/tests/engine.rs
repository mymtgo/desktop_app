//! The engine, tick by tick, over real fixtures in a temporary log folder.

mod common;

use std::collections::BTreeMap;

use common::*;
use fixture_tools::synth::Synth;
use fixture_tools::{Fixture, FixtureMeta, TestDir, fixture_names, load_fixture, prepared_events};
use mtgo_match::{
    ABANDON_AFTER_MS, MatchState, Outcome, PROJECTION_VERSION, Projector, Resolution,
};
use mtgo_protocol::EventKind;
use store::names::outcome_str;
use store::{open_reader, read};
use tracker::engine::TWO_CLIENTS_WINDOW_MS;
use tracker::{TrackerConfig, TrackerEvent};

#[test]
fn every_fixture_is_stored_projected_and_rebuilt_identically() {
    for name in fixture_names() {
        let fixture = load_fixture(&name);
        let dir = TestDir::new(&format!("engine-{name}"));
        dir.write("logs/mtgo.log", &fixture.bytes);
        let config = config(&dir, &fixture);
        let mut engine = open(&config, fixture.meta.now_ms());
        let (events, now) = run_until_settled(&mut engine, fixture.meta.now_ms());
        drop(engine);

        let want = expected(&fixture, now);
        assert_eq!(last_views(&events), want, "pushed views, fixture {name}");

        let reader = open_reader(&config.db_path).expect("reader");
        let mut stored: Vec<(String, String)> = read::list_matches(&reader, 100, 0)
            .expect("list")
            .into_iter()
            .map(|row| (row.token, row.outcome))
            .collect();
        stored.sort();
        let mut wanted: Vec<(String, String)> = want
            .iter()
            .map(|view| (view.token.clone(), outcome_str(view.outcome).to_string()))
            .collect();
        wanted.sort();
        assert_eq!(stored, wanted, "stored rows, fixture {name}");

        let tokens: Vec<String> = want.iter().map(|view| view.token.clone()).collect();
        assert_eq!(
            rebuilt_from_storage(&config, &tokens, now),
            want,
            "rebuild from storage, fixture {name}"
        );

        let status = last_status(&events).expect("a status was pushed");
        assert_eq!(status.files.len(), 1);
        assert_eq!(status.files[0].cursor, fixture.bytes.len() as u64);
        assert!(!status.two_clients);
    }
}

const JOINED_STATE: &str = "MatchJoinedEventUnderwayState";

/// The token and Match ID of every match the local player joined, in log order.
fn joined_matches(fixture: &Fixture) -> Vec<(String, u64)> {
    let mut joined: Vec<(String, u64)> = Vec::new();
    for event in prepared_events(fixture) {
        if event.kind != EventKind::GameManagementJson
            || !event.header.context.contains(JOINED_STATE)
        {
            continue;
        }
        if let (Some(token), Some(match_id)) = (event.subjects.match_token, event.subjects.match_id)
            && !joined.iter().any(|(known, _)| *known == token)
        {
            joined.push((token, match_id));
        }
    }
    joined
}

/// `now` for a first run that stops at `cut`: just after the last event before
/// it, as if the app stopped right then (a match cut in progress stays open).
fn now_after_last_event_before(fixture: &Fixture, cut: usize) -> i64 {
    let last = prepared_events(fixture)
        .iter()
        .filter(|event| (event.offset as usize) < cut)
        .map(|event| event.at_ms)
        .max()
        .expect("events before the cut");
    last + 1_000
}

fn stored_state(config: &TrackerConfig, token: &str) -> Option<String> {
    let reader = open_reader(&config.db_path).expect("reader");
    reader
        .query_row(
            "SELECT state FROM matches WHERE token = ?1",
            [token],
            |row| row.get(0),
        )
        .ok()
}

/// Write the fixture up to `cut`, track it, and stop the engine.
fn first_half(
    fixture: &Fixture,
    cut: usize,
    label: &str,
) -> (TestDir, TrackerConfig, Vec<TrackerEvent>) {
    let dir = TestDir::new(label);
    dir.write("logs/mtgo.log", &fixture.bytes[..cut]);
    let config = config(&dir, fixture);
    let now = now_after_last_event_before(fixture, cut);
    let mut first = open(&config, now);
    let (events, _) = run_until_settled(&mut first, now);
    drop(first);
    (dir, config, events)
}

/// Append the rest of the fixture and track it with a new engine (a restart).
/// Returns every event of both runs and the final `now`.
fn second_half(
    fixture: &Fixture,
    cut: usize,
    dir: &TestDir,
    config: &TrackerConfig,
    mut events: Vec<TrackerEvent>,
) -> (Vec<TrackerEvent>, i64) {
    dir.append("logs/mtgo.log", &fixture.bytes[cut..]);
    let mut second = open(config, fixture.meta.now_ms());
    let (after, now) = run_until_settled(&mut second, fixture.meta.now_ms());
    drop(second);
    events.extend(after);
    (events, now)
}

/// Each match's pushed revisions rise. The same push again (the game sync
/// wrote the match's card rows, so an open page refetches) repeats one.
fn assert_revisions_rise(events: &[TrackerEvent]) {
    let mut last: BTreeMap<String, (u64, &mtgo_match::MatchView)> = BTreeMap::new();
    for event in events {
        if let TrackerEvent::MatchUpdated { view, revision, .. } = event {
            let previous = last.insert(view.token.clone(), (*revision, view));
            let Some((previous, previous_view)) = previous else {
                continue;
            };
            if *revision == previous && view == previous_view {
                continue;
            }
            assert!(
                *revision > previous,
                "revision went from {previous} to {revision}"
            );
        }
    }
}

/// A cut halfway through the first joined match's own events, on an event
/// boundary: the match is in progress there.
fn mid_match_cut(fixture: &Fixture) -> (String, usize) {
    let (token, match_id) = joined_matches(fixture)
        .into_iter()
        .next()
        .expect("a joined match");
    let match_offsets: Vec<u64> = prepared_events(fixture)
        .iter()
        .filter(|event| event.subjects.match_id == Some(match_id))
        .map(|event| event.offset)
        .collect();
    (token, match_offsets[match_offsets.len() / 2] as usize)
}

#[test]
fn a_restart_mid_match_resumes_from_storage() {
    let fixture = load_fixture("league_join_drop");
    let (token, cut) = mid_match_cut(&fixture);
    let (dir, config, events) = first_half(&fixture, cut, "engine-restart");
    assert_eq!(
        stored_state(&config, &token).as_deref(),
        Some("in_progress"),
        "the restart must rehydrate an open match"
    );

    let (events, now) = second_half(&fixture, cut, &dir, &config, events);
    assert_eq!(last_views(&events), expected(&fixture, now));
    assert_revisions_rise(&events);
}

#[test]
fn an_open_match_is_rehydrated_and_finalized_after_a_restart() {
    let fixture = load_fixture("league_join_drop");
    let (token, cut) = mid_match_cut(&fixture);
    let (_dir, config, mut events) = first_half(&fixture, cut, "engine-rehydrate");
    assert_eq!(
        stored_state(&config, &token).as_deref(),
        Some("in_progress")
    );

    // Nothing new arrives: only rehydration can bring the match back to finalize it.
    let mut second = open(&config, fixture.meta.now_ms());
    let (after, now) = run_until_settled(&mut second, fixture.meta.now_ms());
    drop(second);
    events.extend(after);

    let cut_fixture = Fixture {
        name: fixture.name.clone(),
        bytes: fixture.bytes[..cut].to_vec(),
        meta: fixture.meta.clone(),
    };
    let want = expected(&cut_fixture, now);
    assert_eq!(last_views(&events), want);
    assert_ne!(
        stored_state(&config, &token).as_deref(),
        Some("in_progress")
    );
    assert_revisions_rise(&events);
}

#[test]
fn a_restart_between_join_and_first_game_state_resumes_from_storage() {
    let fixture = load_fixture("draft_hobbit");
    let prepared = prepared_events(&fixture);
    let joined = joined_matches(&fixture);
    assert!(!joined.is_empty());
    for (token, match_id) in joined {
        let first_game_state = prepared
            .iter()
            .find(|event| {
                event.kind == EventKind::GameStateUpdate
                    && event.subjects.match_id == Some(match_id)
            })
            .expect("a game state")
            .offset;
        // Cut just before the last game_management event ahead of the first game
        // state. It names both the token and the Match ID, so it learns the mapping
        // without asking storage: only the restart's stored marks bring back the
        // match's earlier facts (some carry gameplay entries).
        let cut = prepared
            .iter()
            .filter(|event| {
                event.kind == EventKind::GameManagementJson
                    && event.subjects.match_id == Some(match_id)
                    && event.offset < first_game_state
            })
            .map(|event| event.offset)
            .max()
            .expect("game_management events before the first game state")
            as usize;
        assert!(
            prepared.iter().any(|event| {
                event.subjects.match_token.as_deref() == Some(token.as_str())
                    && event.header.context.contains(JOINED_STATE)
                    && (event.offset as usize) < cut
            }),
            "match {token} is joined before the cut"
        );

        let (dir, config, events) = first_half(&fixture, cut, "engine-restart-joined");
        assert_eq!(
            stored_state(&config, &token),
            None,
            "match {token}: joined without a game state, so no derived row yet"
        );

        let (events, now) = second_half(&fixture, cut, &dir, &config, events);
        assert_eq!(
            last_views(&events),
            expected(&fixture, now),
            "restart before match {token}'s first game state"
        );
        assert_revisions_rise(&events);
    }
}

#[test]
fn a_truncated_log_is_sealed_and_restarted() {
    let fixture = load_fixture("league_join_drop");
    let dir = TestDir::new("engine-truncate");
    dir.write("logs/mtgo.log", &fixture.bytes);
    let config = config(&dir, &fixture);
    let mut engine = open(&config, fixture.meta.now_ms());
    let (_, now) = run_until_settled(&mut engine, fixture.meta.now_ms());

    let fresh = b"12:00:00 [INF] (UI|a new day)\n12:00:01 [INF] (UI|more)\n";
    dir.write("logs/mtgo.log", fresh);
    run_until_settled(&mut engine, now + 1_000);
    drop(engine);

    let reader = open_reader(&config.db_path).expect("reader");
    let mut stmt = reader
        .prepare("SELECT seal_reason, inherited_username, cursor_offset FROM log_files ORDER BY id")
        .expect("query");
    let rows: Vec<(Option<String>, Option<String>, i64)> = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .expect("rows")
        .map(|row| row.expect("row"))
        .collect();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].0.as_deref(), Some("truncated"));
    assert_eq!(rows[0].2, fixture.bytes.len() as i64);
    assert_eq!(rows[1].0, None);
    assert_eq!(
        rows[1].1.as_deref(),
        Some(fixture.meta.local_username.as_str())
    );
    assert_eq!(rows[1].2, fresh.len() as i64);
}

#[test]
fn a_projection_version_bump_rebuilds_stored_matches() {
    let fixture = load_fixture("league_join_drop");
    let dir = TestDir::new("engine-rebuild");
    dir.write("logs/mtgo.log", &fixture.bytes);
    let config = config(&dir, &fixture);
    let mut engine = open(&config, fixture.meta.now_ms());
    let (_, now) = run_until_settled(&mut engine, fixture.meta.now_ms());
    drop(engine);
    let registries = |conn: &store::Connection| -> (i64, i64) {
        let accounts = conn
            .query_row("SELECT COUNT(*) FROM accounts", [], |row| row.get(0))
            .expect("accounts");
        let opponents = conn
            .query_row("SELECT COUNT(*) FROM opponents", [], |row| row.get(0))
            .expect("opponents");
        (accounts, opponents)
    };
    let before = {
        let conn = store::open_writer(&config.db_path).expect("writer");
        conn.execute_batch(
            "UPDATE matches SET derived_version = 0, outcome = 'unknown';
             UPDATE derivations SET version = 0;",
        )
        .expect("age the stored rows");
        registries(&conn)
    };

    let mut engine = open(&config, now + 1_000);
    let (events, now) = run_until_settled(&mut engine, now + 1_000);
    drop(engine);

    let reader = open_reader(&config.db_path).expect("reader");
    let stale: i64 = reader
        .query_row(
            "SELECT COUNT(*) FROM matches WHERE derived_version < ?1",
            [PROJECTION_VERSION],
            |row| row.get(0),
        )
        .expect("count");
    assert_eq!(stale, 0);
    assert_eq!(registries(&reader), before, "registries survive a rebuild");
    assert_eq!(
        read::derivation_version(&reader).expect("version"),
        Some(PROJECTION_VERSION)
    );
    let mut stored: Vec<(String, String)> = read::list_matches(&reader, 100, 0)
        .expect("list")
        .into_iter()
        .map(|row| (row.token, row.outcome))
        .collect();
    stored.sort();
    let mut wanted: Vec<(String, String)> = expected(&fixture, now)
        .iter()
        .map(|view| (view.token.clone(), outcome_str(view.outcome).to_string()))
        .collect();
    wanted.sort();
    assert_eq!(stored, wanted);
    assert!(
        events
            .iter()
            .any(|event| matches!(event, TrackerEvent::MatchUpdated { .. }))
    );
}

/// The fixture's league panel views, match and drop reach the league sync of
/// its player and are stored as one dropped run.
#[test]
fn a_league_fixture_folds_into_its_dropped_run() {
    const LEAGUE: &str = "d2050286-53fd-4072-804f-190d6a3c030a";
    const MATCH: &str = "f5e33a1f-c2e7-4678-b30d-309b63f17a40";
    // 2026-08-22, UTC: the first panel view and the drop.
    const JOINED: i64 = 1_787_401_463_000;
    const DROPPED: i64 = 1_787_401_695_000;
    let fixture = load_fixture("league_join_drop");
    let dir = TestDir::new("engine-league-inputs");
    dir.write("logs/mtgo.log", &fixture.bytes);
    let config = config(&dir, &fixture);
    let mut engine = open(&config, fixture.meta.now_ms());
    run_until_settled(&mut engine, fixture.meta.now_ms());
    drop(engine);
    let mut writer = store::open_writer(&config.db_path).expect("writer");
    store::league_sync::sync_all(&mut writer, fixture.meta.tz(), fixture.meta.now_ms())
        .expect("league sync");
    drop(writer);

    let reader = open_reader(&config.db_path).expect("reader");
    let account: i64 = reader
        .query_row(
            "SELECT id FROM accounts WHERE username = ?1",
            [&fixture.meta.local_username],
            |row| row.get(0),
        )
        .expect("the local account");
    let runs = store::leagues::stored_runs(
        &reader,
        &store::leagues::RunFilter {
            owners: vec![store::league_sync::LeagueOwner::Account(account)],
            kinds: vec![leagues::LeagueKind::Constructed],
            ..store::leagues::RunFilter::default()
        },
    )
    .expect("stored runs");
    assert_eq!(runs.len(), 1);
    let run = &runs[0];
    assert_eq!(run.key, format!("{LEAGUE}:{MATCH}"));
    assert_eq!(run.event_id, Some(10_397));
    assert_eq!(run.joined_at, Some(JOINED));
    assert_eq!(run.state, leagues::RunState::Dropped);
    assert_eq!(run.dropped_at, Some(DROPPED));
    let tokens: Vec<&str> = run.matches.iter().map(|row| row.token.as_str()).collect();
    assert_eq!(tokens, vec![MATCH]);

    // Three panel views and one drop in the log, one run row, and the drop is the
    // log's: the player never dropped it by hand.
    let events_of = |kind: &str| -> i64 {
        reader
            .query_row(
                "SELECT COUNT(*) FROM log_events WHERE kind = ?1",
                [kind],
                |row| row.get(0),
            )
            .expect("count")
    };
    assert_eq!(
        events_of("league_joined"),
        3,
        "the panel was viewed three times: still one run"
    );
    assert_eq!(events_of("league_dropped"), 1, "a single drop");
    let (run_rows, manual_drops): (i64, i64) = reader
        .query_row(
            "SELECT COUNT(*), COUNT(user_dropped_at) FROM leagues",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("run rows");
    assert_eq!(run_rows, 1, "one stored run");
    assert_eq!(manual_drops, 0, "no manual drop");
}

/// Matches stored by projection version 3 have no league columns: the version
/// bump re-derives them, so they gain their league token and game structure.
#[test]
fn a_projection_version_bump_fills_the_league_columns_of_stored_matches() {
    let fixture = load_fixture("league_join_drop");
    let dir = TestDir::new("engine-rebuild-league");
    dir.write("logs/mtgo.log", &fixture.bytes);
    let config = config(&dir, &fixture);
    let mut engine = open(&config, fixture.meta.now_ms());
    let (_, now) = run_until_settled(&mut engine, fixture.meta.now_ms());
    drop(engine);
    {
        let conn = store::open_writer(&config.db_path).expect("writer");
        conn.execute_batch(
            "UPDATE matches SET derived_version = 3, league_token = NULL, game_structure = NULL;
             UPDATE derivations SET version = 3;",
        )
        .expect("rows as projection version 3 stored them");
    }
    // Without the bump (version still 3) nothing is re-derived and the
    // columns stay NULL.

    let mut engine = open(&config, now + 1_000);
    run_until_settled(&mut engine, now + 1_000);
    drop(engine);

    let reader = open_reader(&config.db_path).expect("reader");
    let rows: Vec<(Option<String>, Option<String>)> = reader
        .prepare("SELECT league_token, game_structure FROM matches ORDER BY id")
        .expect("prepare")
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .expect("query")
        .map(|row| row.expect("row"))
        .collect();
    assert!(!rows.is_empty(), "the fixture has a league match");
    for row in rows {
        assert_eq!(
            row,
            (
                Some("d2050286-53fd-4072-804f-190d6a3c030a".to_string()),
                Some("Modern".to_string())
            )
        );
    }
}

#[test]
fn finished_matches_are_compacted_and_still_rebuild() {
    let fixture = load_fixture("draft_hobbit");
    let dir = TestDir::new("engine-compact");
    dir.write("logs/mtgo.log", &fixture.bytes);
    let config = config(&dir, &fixture);
    let mut engine = open(&config, fixture.meta.now_ms());
    let (_, now) = run_until_settled(&mut engine, fixture.meta.now_ms());
    // A minute later the next compaction also archives events committed after
    // the first one (the provisional last event).
    let now = now + tracker::engine::COMPACT_EVERY_MS + 1;
    engine.tick(now).expect("tick");
    engine.flush().expect("flush");
    drop(engine);

    let want = expected(&fixture, now);
    let reader = open_reader(&config.db_path).expect("reader");
    let archives: i64 = reader
        .query_row("SELECT COUNT(*) FROM match_event_archives", [], |row| {
            row.get(0)
        })
        .expect("count");
    assert_eq!(
        archives,
        want.len() as i64,
        "every fixture match is finished"
    );
    let left: i64 = reader
        .query_row(
            "SELECT COUNT(*) FROM log_events e JOIN matches m ON e.match_token = m.token",
            [],
            |row| row.get(0),
        )
        .expect("count");
    assert_eq!(left, 0);
    let tokens: Vec<String> = want.iter().map(|view| view.token.clone()).collect();
    assert_eq!(rebuilt_from_storage(&config, &tokens, now), want);
}

#[test]
fn two_logs_growing_live_raise_the_two_clients_warning() {
    let dir = TestDir::new("engine-two-clients");
    dir.write("logs/a/mtgo.log", b"12:00:00 [INF] (UI|a)\n");
    dir.write("logs/b/mtgo.log", b"12:00:00 [INF] (UI|b)\n");
    let config = TrackerConfig {
        db_path: dir.join("data/mymtgo.sqlite"),
        timezone: chrono_tz::UTC,
        os_timezone: None,
        log_overrides: vec![dir.join("logs")],
        arena_log_overrides: vec![dir.join("arena/Player.log")],
        mtgo_data_folder: Some(dir.join("mtgo-data")),
        replay_day_anchor: None,
        watching: true,
        arena_enabled: true,
        endstep_enabled: false,
    };
    let start = 1_767_000_000_000;
    let mut engine = open(&config, start);
    engine.tick(start).expect("catch-up tick");

    dir.append("logs/a/mtgo.log", b"12:00:01 [INF] (UI|a2)\n");
    dir.append("logs/b/mtgo.log", b"12:00:01 [INF] (UI|b2)\n");
    let tick = engine.tick(start + 100).expect("live tick");
    assert!(tick.active);
    assert!(last_status(&tick.events).expect("status").two_clients);

    let later = engine
        .tick(start + 100 + TWO_CLIENTS_WINDOW_MS + 1)
        .expect("quiet tick");
    assert!(!last_status(&later.events).expect("status").two_clients);
}

#[test]
fn orphan_lines_are_counted_in_the_file_status() {
    let dir = TestDir::new("engine-orphans");
    dir.write(
        "logs/mtgo.log",
        b"the tail of an event\nanother orphan\n12:00:00 [INF] (UI|a)\n12:00:01 [INF] (UI|b)\n",
    );
    let config = TrackerConfig {
        db_path: dir.join("data/mymtgo.sqlite"),
        timezone: chrono_tz::UTC,
        os_timezone: None,
        log_overrides: vec![dir.join("logs")],
        arena_log_overrides: vec![dir.join("arena/Player.log")],
        mtgo_data_folder: Some(dir.join("mtgo-data")),
        replay_day_anchor: None,
        watching: true,
        arena_enabled: true,
        endstep_enabled: false,
    };
    let mut engine = open(&config, 1_767_000_000_000);
    let (events, _) = run_until_settled(&mut engine, 1_767_000_000_000);
    let status = last_status(&events).expect("status");
    assert_eq!(status.files.len(), 1);
    assert_eq!(status.files[0].dropped_orphan_lines, 2);
}

#[test]
fn a_bad_stored_timezone_falls_back_and_is_reported() {
    let dir = TestDir::new("engine-bad-timezone");
    dir.write("logs/mtgo.log", b"12:00:00 [INF] (UI|a)\n");
    let config = TrackerConfig {
        db_path: dir.join("data/mymtgo.sqlite"),
        timezone: chrono_tz::UTC,
        os_timezone: None,
        log_overrides: vec![dir.join("logs")],
        arena_log_overrides: vec![dir.join("arena/Player.log")],
        mtgo_data_folder: Some(dir.join("mtgo-data")),
        replay_day_anchor: None,
        watching: true,
        arena_enabled: true,
        endstep_enabled: false,
    };
    let start = 1_767_000_000_000;
    let mut engine = open(&config, start);
    let first = engine.tick(start).expect("tick");
    assert!(!last_status(&first.events).expect("status").files[0].bad_timezone);
    engine.flush().expect("flush");
    drop(engine);

    store::open_writer(&config.db_path)
        .expect("writer")
        .execute("UPDATE log_files SET timezone = 'Not/AZone'", [])
        .expect("break the timezone");
    dir.append("logs/mtgo.log", b"12:00:01 [INF] (UI|b)\n");

    let mut engine = open(&config, start + 1_000);
    let tick = engine.tick(start + 1_000).expect("tick");
    let status = last_status(&tick.events).expect("status");
    assert_eq!(status.files.len(), 1);
    assert!(status.files[0].bad_timezone);
}

const ME: &str = "Local_Name";
const OPP: &str = "Opp Name";
const TOKEN: &str = "f5e33a1f-c2e7-4678-b30d-309b63f17a40";

/// A log whose match loses its connection in game 2 and then goes quiet: it is
/// in progress until `ABANDON_AFTER_MS` later, then a loss by disconnect. (With
/// a completion signal after the lost connection the signal would settle it at
/// once, spec 5.5.) Events are one second apart, after a Login line.
fn disconnect_fixture() -> Fixture {
    let mut s = Synth::new(Some(ME));
    let events = [
        s.joined(TOKEN, 99, 7, &[]),
        s.game_state(99, 7, &[ME, OPP]),
        s.entry(TOKEN, 99, 7, "@PLocal_Name wins the game."),
        s.game_state(99, 8, &[ME, OPP]),
        s.entry(
            TOKEN,
            99,
            8,
            "@PLocal_Name has lost connection to the game.",
        ),
    ];
    let mut text = format!("12:00:00 [INF] (Login|MtGO Login Success) Username: {ME} (7)\n");
    for (index, event) in events.iter().enumerate() {
        // Synthetic lines all start with "12:00:00"; give each its own second.
        let body = event.text.trim_end();
        text.push_str(&format!("12:00:{:02}{}\n", index + 1, &body[8..]));
    }
    Fixture {
        name: "synthetic_disconnect".to_string(),
        bytes: text.into_bytes(),
        meta: FixtureMeta {
            date: "2026-01-05".to_string(),
            timezone: "UTC".to_string(),
            local_username: ME.to_string(),
            now: "2026-01-05T12:00:05Z".to_string(),
        },
    }
}

#[test]
fn a_restart_inside_the_disconnect_window_still_settles_the_match() {
    let fixture = disconnect_fixture();
    let last = prepared_events(&fixture)
        .iter()
        .map(|event| event.at_ms)
        .max()
        .expect("events");
    let dir = TestDir::new("engine-restart-disconnect");
    dir.write("logs/mtgo.log", &fixture.bytes);
    let config = config(&dir, &fixture);

    let first_now = last + 10 * 60 * 1000;
    let mut first = open(&config, first_now);
    let (mut events, now) = run_until_settled(&mut first, first_now);
    drop(first);
    let before = expected(&fixture, now);
    assert_eq!(last_views(&events), before);
    assert_eq!(
        before[0].state,
        MatchState::InProgress,
        "in progress until the window ends"
    );

    let second_now = last + ABANDON_AFTER_MS + 60 * 1000;
    let mut second = open(&config, second_now);
    let (after, now) = run_until_settled(&mut second, second_now);
    drop(second);
    events.extend(after);

    let want = expected(&fixture, now);
    assert_eq!(want[0].outcome, Outcome::Loss);
    assert_eq!(want[0].resolution, Some(Resolution::Disconnect));
    assert_eq!(last_views(&events), want, "pushed views");
    let reader = open_reader(&config.db_path).expect("reader");
    let stored: Vec<String> = read::list_matches(&reader, 10, 0)
        .expect("list")
        .into_iter()
        .map(|row| row.outcome)
        .collect();
    assert_eq!(stored, vec!["loss".to_string()], "stored outcome");
    assert_eq!(
        rebuilt_from_storage(&config, &[TOKEN.to_string()], now),
        want,
        "rebuild from storage"
    );
    assert_revisions_rise(&events);
}

/// A log reached through a second root that appears later (a symlinked Steam
/// home, say) is the same file: it must not get a second row and be read again.
#[cfg(unix)]
#[test]
fn a_log_reached_through_a_new_alias_is_not_tracked_twice() {
    let fixture = load_fixture("league_join_drop");
    let dir = TestDir::new("engine-alias");
    dir.write("logs/mtgo.log", &fixture.bytes);
    // The alias root sorts first, so once it exists discovery keeps its path.
    let config = TrackerConfig {
        log_overrides: vec![dir.join("alias"), dir.join("logs")],
        arena_log_overrides: vec![dir.join("arena/Player.log")],
        ..config(&dir, &fixture)
    };
    let mut engine = open(&config, fixture.meta.now_ms());
    let (_, now) = run_until_settled(&mut engine, fixture.meta.now_ms());

    std::os::unix::fs::symlink(dir.join("logs"), dir.join("alias")).expect("symlink");
    run_until_settled(&mut engine, now + tracker::engine::RESCAN_EVERY_MS + 1);
    drop(engine);

    let reader = open_reader(&config.db_path).expect("reader");
    let rows: i64 = reader
        .query_row("SELECT COUNT(*) FROM log_files", [], |row| row.get(0))
        .expect("count");
    assert_eq!(rows, 1, "one row for the one physical file");
    assert_eq!(
        stored_event_count(&config),
        prepared_events(&fixture).len() as i64,
        "every whitelisted event stored once"
    );
}

/// A restart after a match's first events but before its join: the token was not
/// marked stored (no join yet), so the match starts from the events after the
/// restart. Derive does read pre-join state changes, but the guarantee is that no
/// observed pre-join shape reaches a completion or end rule, so the first view
/// pushed after the restart (before any retire and reload) equals the straight
/// fold's view at the same event, and the final result is identical too.
#[test]
fn a_restart_just_before_a_join_resumes_identically() {
    let mut checked = 0;
    for name in fixture_names() {
        let fixture = load_fixture(&name);
        let prepared = prepared_events(&fixture);
        for (token, _) in joined_matches(&fixture) {
            let is_token = |event: &mtgo_protocol::PreparedEvent| {
                event.subjects.match_token.as_deref() == Some(token.as_str())
            };
            let cut = prepared
                .iter()
                .find(|event| {
                    is_token(event)
                        && matches!(
                            event.kind,
                            EventKind::GameManagementJson | EventKind::MatchStateChanged
                        )
                        && event.header.context.contains(JOINED_STATE)
                })
                .expect("a join")
                .offset as usize;
            if !prepared
                .iter()
                .any(|event| is_token(event) && (event.offset as usize) < cut)
            {
                continue;
            }
            let (dir, config, events) = first_half(&fixture, cut, "engine-restart-pre-join");
            let before_restart = events.len();
            let (events, now) = second_half(&fixture, cut, &dir, &config, events);

            // The first view pushed for the match after the restart, compared with
            // the straight fold (every event, pre-join ones included) stopped at the
            // event that caused that push. A push with no causing event would mean the
            // stored pre-join events were reloaded and changed the result.
            let first_push = events[before_restart..]
                .iter()
                .find_map(|event| match event {
                    TrackerEvent::MatchUpdated { view, cause, .. } if view.token == token => {
                        Some((view.clone(), *cause))
                    }
                    _ => None,
                })
                .expect("a view is pushed after the restart");
            let (pushed, cause) = first_push;
            let (_, cause_offset) = cause.unwrap_or_else(|| {
                panic!(
                    "fixture {name}: the first push for match {token} after the restart came \
                     from a reload of its pre-join events, so they changed the result"
                )
            });
            let mut straight = Projector::new(vec![]);
            for event in &prepared {
                if event.kind == EventKind::Login
                    && let Some(name) = &event.local_username
                {
                    straight.add_account(name);
                }
                straight.apply(event);
                if event.offset == cause_offset {
                    break;
                }
            }
            assert_eq!(
                Some(&pushed),
                straight.view(&token),
                "fixture {name}: first push after a restart just before match {token}'s join"
            );

            assert_eq!(
                last_views(&events),
                expected(&fixture, now),
                "fixture {name}: restart just before match {token}'s join"
            );
            assert_revisions_rise(&events);
            checked += 1;
        }
    }
    assert!(checked > 0, "some joined match has events before its join");
}

/// `(seal_reason, cursor_offset)` of every `log_files` row, oldest first.
fn file_rows(config: &TrackerConfig) -> Vec<(Option<String>, i64)> {
    let reader = open_reader(&config.db_path).expect("reader");
    let mut stmt = reader
        .prepare("SELECT seal_reason, cursor_offset FROM log_files ORDER BY id")
        .expect("query");
    stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .expect("rows")
        .map(|row| row.expect("row"))
        .collect()
}

/// A replacement log that is larger than `len` and starts differently from any fixture.
fn larger_replacement(len: usize) -> Vec<u8> {
    let line = "12:00:00 [INF] (UI|a replacement log written by a new client)\n";
    line.repeat(len / line.len() + 2).into_bytes()
}

fn is_replacement_seal(reason: &Option<String>) -> bool {
    matches!(reason.as_deref(), Some("head_changed" | "created_forward"))
}

#[test]
fn a_log_replaced_by_a_larger_file_is_sealed_and_restarted() {
    let fixture = load_fixture("league_join_drop");
    assert!(fixture.bytes.len() > 4096, "the head must be frozen");
    let dir = TestDir::new("engine-replace-larger");
    dir.write("logs/mtgo.log", &fixture.bytes);
    let config = config(&dir, &fixture);
    let mut engine = open(&config, fixture.meta.now_ms());
    let (_, now) = run_until_settled(&mut engine, fixture.meta.now_ms());

    // Rewritten in place: the creation time stays, so only the head says it is new.
    let fresh = larger_replacement(fixture.bytes.len());
    dir.write("logs/mtgo.log", &fresh);
    run_until_settled(&mut engine, now + 1_000);
    drop(engine);

    let rows = file_rows(&config);
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].0.as_deref(), Some("head_changed"));
    assert_eq!(rows[0].1, fixture.bytes.len() as i64, "sealed where it was");
    assert_eq!(rows[1], (None, fresh.len() as i64));
}

#[test]
fn a_log_deleted_and_recreated_larger_is_sealed_and_restarted() {
    let fixture = load_fixture("league_join_drop");
    let dir = TestDir::new("engine-recreate-larger");
    dir.write("logs/mtgo.log", &fixture.bytes);
    let config = config(&dir, &fixture);
    let mut engine = open(&config, fixture.meta.now_ms());
    let (_, now) = run_until_settled(&mut engine, fixture.meta.now_ms());

    let fresh = larger_replacement(fixture.bytes.len());
    std::fs::remove_file(dir.join("logs/mtgo.log")).expect("remove");
    dir.write("logs/mtgo.log", &fresh);
    run_until_settled(&mut engine, now + 1_000);
    drop(engine);

    let rows = file_rows(&config);
    assert_eq!(rows.len(), 2);
    // Whichever rule fires first (a new creation time or the changed head), the
    // replaced file is sealed and the new one is read from the start.
    assert!(is_replacement_seal(&rows[0].0), "{:?}", rows[0]);
    assert_eq!(rows[0].1, fixture.bytes.len() as i64);
    assert_eq!(rows[1], (None, fresh.len() as i64));
}

#[test]
fn a_rotation_while_the_app_was_closed_is_sealed_on_the_first_tick() {
    let fixture = load_fixture("league_join_drop");
    let dir = TestDir::new("engine-rotated-while-closed");
    dir.write("logs/mtgo.log", &fixture.bytes);
    let config = config(&dir, &fixture);
    let mut first = open(&config, fixture.meta.now_ms());
    let (_, now) = run_until_settled(&mut first, fixture.meta.now_ms());
    drop(first);

    let fresh = larger_replacement(fixture.bytes.len());
    dir.write("logs/mtgo.log", &fresh);

    let mut second = open(&config, now + 1_000);
    second.tick(now + 1_001).expect("first tick after open");
    second.flush().expect("flush");
    let rows = file_rows(&config);
    assert_eq!(rows.len(), 2, "the old row is sealed and a new one started");
    assert!(is_replacement_seal(&rows[0].0), "{:?}", rows[0]);
    assert_eq!(
        rows[0].1,
        fixture.bytes.len() as i64,
        "the old row was not resumed into the new file's bytes"
    );
    assert_eq!(rows[1].0, None);

    run_until_settled(&mut second, now + 2_000);
    drop(second);
    assert_eq!(file_rows(&config)[1], (None, fresh.len() as i64));
}

#[test]
fn a_tick_that_reads_events_reports_its_timing() {
    let fixture = load_fixture("league_join_drop");
    let dir = TestDir::new("engine-timing");
    dir.write("logs/mtgo.log", &fixture.bytes);
    let config = config(&dir, &fixture);
    let start = fixture.meta.now_ms();
    let mut engine = open(&config, start);

    let tick = engine.tick(start).expect("tick");
    assert!(tick.worked);
    let t = tick.timing;
    assert!(t.total_us > 0);
    let phases = t.rescan_us
        + t.poll_us
        + t.lookups_us
        + t.finalize_us
        + t.rebuild_us
        + t.decks_us
        + t.write_us;
    assert!(
        phases <= t.total_us,
        "phases {phases} > total {}",
        t.total_us
    );

    // Caught up and settled: a tick with nothing new did no work.
    let (_, now) = run_until_settled(&mut engine, start + 1);
    let idle = engine.tick(now + 10).expect("tick");
    assert!(!idle.worked);
}

/// The watcher switched off mid-match (release spec 4.1): nothing is read
/// from the log and no time rule settles the open match while paused; once
/// on again, the lines written meanwhile are read from the stored cursor,
/// each once, and the result is the straight fold's.
#[test]
fn paused_tracking_reads_nothing_and_resumes_from_the_cursor() {
    let fixture = load_fixture("league_join_drop");
    let (token, cut) = mid_match_cut(&fixture);
    let dir = TestDir::new("engine-paused");
    dir.write("logs/mtgo.log", &fixture.bytes[..cut]);
    let config = config(&dir, &fixture);
    let start = now_after_last_event_before(&fixture, cut);
    let mut engine = open(&config, start);
    let (mut events, now) = run_until_settled(&mut engine, start);
    let stored_before = stored_event_count(&config);
    assert_eq!(committed_cursor(&config), Some(cut as i64));

    engine.set_watching(false);
    assert!(!engine.watching());
    dir.append("logs/mtgo.log", &fixture.bytes[cut..]);
    // Past several rescans and the abandon window.
    let mut paused_at = now;
    for step in 0..5 {
        paused_at += tracker::engine::RESCAN_EVERY_MS + 1;
        if step == 4 {
            paused_at += 2 * ABANDON_AFTER_MS;
        }
        let tick = engine.tick(paused_at).expect("paused tick");
        assert!(!tick.more, "a paused tracker has nothing left to read");
        events.extend(tick.events);
    }
    engine.flush().expect("flush");
    assert_eq!(
        stored_event_count(&config),
        stored_before,
        "nothing read while paused"
    );
    assert_eq!(committed_cursor(&config), Some(cut as i64), "cursor kept");
    assert_eq!(
        stored_state(&config, &token).as_deref(),
        Some("in_progress"),
        "no time rule settled the open match while paused"
    );
    let status = last_status(&events).expect("a status was pushed");
    assert!(status.paused, "the status says the watcher is off");
    assert_eq!(status.files.len(), 1, "the log is still found");

    engine.set_watching(true);
    let resume = paused_at.max(fixture.meta.now_ms()) + 1;
    let (after, end) = run_until_settled(&mut engine, resume);
    drop(engine);
    events.extend(after);
    assert!(!last_status(&events).expect("status").paused);
    assert_eq!(
        committed_cursor(&config),
        Some(fixture.bytes.len() as i64),
        "the rest was read"
    );
    assert_eq!(
        stored_event_count(&config),
        prepared_events(&fixture).len() as i64,
        "every whitelisted event stored exactly once"
    );
    assert_eq!(last_views(&events), expected(&fixture, end));
    assert_revisions_rise(&events);
}

/// A tracker started with the watcher off (a restart on a new folder, or
/// the app opened while it was off) reads nothing until switched on.
#[test]
fn a_tracker_started_paused_reads_nothing_until_switched_on() {
    let fixture = load_fixture("league_join_drop");
    let dir = TestDir::new("engine-start-paused");
    dir.write("logs/mtgo.log", &fixture.bytes);
    let config = TrackerConfig {
        watching: false,
        arena_enabled: true,
        endstep_enabled: false,
        ..config(&dir, &fixture)
    };
    let mut engine = open(&config, fixture.meta.now_ms());
    assert!(!engine.watching());
    let (events, now) = run_until_settled(&mut engine, fixture.meta.now_ms());
    assert_eq!(stored_event_count(&config), 0, "nothing read");
    let status = last_status(&events).expect("a status was pushed");
    assert!(status.paused);
    assert_eq!(status.files.len(), 1, "found, so the watcher can start");

    engine.set_watching(true);
    let (after, _) = run_until_settled(&mut engine, now + 1);
    drop(engine);
    assert!(!last_status(&after).expect("status").paused);
    assert_eq!(
        stored_event_count(&config),
        prepared_events(&fixture).len() as i64
    );
}
