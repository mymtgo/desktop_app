//! Untracked accounts and the current account, through the engine (release
//! spec 5): an untracked account's new log matches are recorded as skipped,
//! their events kept, and turning tracking back on gives back exactly what
//! tracking all along would have (live equals rebuild).

mod common;

use common::*;
use fixture_tools::{Fixture, TestDir, load_fixture, prepared_events};
use mtgo_protocol::EventKind;
use store::accounts::{AccountEdit, AccountEditOutcome};
use store::import::ImportedAccount;
use store::{open_reader, read};
use tracker::{Engine, TrackerConfig, TrackerEvent};

/// A fixture with four matches of one account (a draft's league rounds).
const MATCHES: &str = "draft_hobbit";

fn local(fixture: &Fixture) -> String {
    fixture.meta.local_username.clone()
}

fn set_tracked(engine: &mut Engine, username: &str, tracked: bool) -> AccountEditOutcome {
    engine
        .edit_account(AccountEdit::SetTracked {
            username: username.to_string(),
            tracked,
        })
        .expect("edit")
}

/// The byte offset just after the fixture's first Login line: the account
/// is known there and no match has started.
fn after_first_login(fixture: &Fixture) -> usize {
    let login = prepared_events(fixture)
        .into_iter()
        .find(|event| event.kind == EventKind::Login)
        .expect("a Login line");
    let start = login.offset as usize;
    let end = fixture.bytes[start..]
        .iter()
        .position(|byte| *byte == b'\n')
        .expect("a line end");
    start + end + 1
}

/// The start of the line where the `index`th visible match first appears.
fn match_start(fixture: &Fixture, index: usize) -> (String, usize) {
    let views = expected(fixture, fixture.meta.now_ms());
    let token = views[index].token.clone();
    let at = prepared_events(fixture)
        .into_iter()
        .filter(|event| event.subjects.match_token.as_deref() == Some(token.as_str()))
        .map(|event| event.offset as usize)
        .min()
        .expect("an event of the match");
    (token, at)
}

fn stored_tokens(config: &TrackerConfig) -> Vec<String> {
    let reader = open_reader(&config.db_path).expect("reader");
    let mut tokens: Vec<String> = read::list_matches(&reader, 1_000, 0)
        .expect("list")
        .into_iter()
        .map(|row| row.token)
        .collect();
    tokens.sort();
    tokens
}

fn skipped_tokens(config: &TrackerConfig) -> Vec<String> {
    let reader = open_reader(&config.db_path).expect("reader");
    let mut stmt = reader
        .prepare("SELECT match_token FROM untracked_matches ORDER BY match_token")
        .expect("prepare");
    stmt.query_map([], |row| row.get::<_, String>(0))
        .expect("query")
        .map(|row| row.expect("row"))
        .collect()
}

fn pushed_tokens(events: &[TrackerEvent]) -> Vec<String> {
    let mut tokens: Vec<String> = events
        .iter()
        .filter_map(|event| match event {
            TrackerEvent::MatchUpdated { view, .. } | TrackerEvent::MatchPreview { view, .. } => {
                Some(view.token.clone())
            }
            _ => None,
        })
        .collect();
    tokens.sort();
    tokens.dedup();
    tokens
}

fn every_visible_token(fixture: &Fixture) -> Vec<String> {
    let mut tokens: Vec<String> = expected(fixture, fixture.meta.now_ms())
        .into_iter()
        .map(|view| view.token)
        .collect();
    tokens.sort();
    tokens
}

/// The fixture with its account untracked right after its Login line: one
/// engine, still running. Returns the events of the second part.
fn untracked_run(
    label: &str,
) -> (
    Fixture,
    TestDir,
    TrackerConfig,
    Engine,
    Vec<TrackerEvent>,
    i64,
) {
    let fixture = load_fixture(MATCHES);
    let cut = after_first_login(&fixture);
    let dir = TestDir::new(label);
    dir.write("logs/mtgo.log", &fixture.bytes[..cut]);
    let config = config(&dir, &fixture);
    let mut engine = open(&config, fixture.meta.now_ms());
    run_until_settled(&mut engine, fixture.meta.now_ms());
    assert_eq!(
        set_tracked(&mut engine, &local(&fixture), false),
        AccountEditOutcome::Done {
            retrack: Vec::new()
        }
    );
    dir.append("logs/mtgo.log", &fixture.bytes[cut..]);
    let (events, now) = run_until_settled(&mut engine, fixture.meta.now_ms() + 10);
    (fixture, dir, config, engine, events, now)
}

// 0.x ProcessMatchEventsTest "does not create match when sole account is untracked".
#[test]
fn an_untracked_account_skips_new_matches_and_keeps_its_events() {
    let (fixture, _dir, config, engine, events, _) = untracked_run("accounts-skip");
    drop(engine);
    let visible = every_visible_token(&fixture);
    assert!(visible.len() >= 2, "the fixture has matches");
    assert!(stored_tokens(&config).is_empty(), "no match row built");
    assert_eq!(skipped_tokens(&config), visible, "each one recorded");
    assert!(
        pushed_tokens(&events).is_empty(),
        "nothing shown, not even a preview"
    );
    assert_eq!(
        stored_event_count(&config),
        prepared_events(&fixture).len() as i64,
        "every event kept"
    );
}

/// Issue #310: an account the 0.x import brings over untracked skips its
/// new log matches from then on, in the same run.
#[test]
fn an_account_imported_untracked_skips_new_matches() {
    let fixture = load_fixture(MATCHES);
    let dir = TestDir::new("accounts-imported-untracked");
    let config = config(&dir, &fixture);
    let mut engine = open(&config, fixture.meta.now_ms());
    let counts = engine
        .import_accounts(
            vec![ImportedAccount {
                username: local(&fixture),
                login_id: None,
                created_at_ms: Some(1),
                tracked: false,
                active_since_ms: None,
            }],
            1,
        )
        .expect("accounts");
    assert_eq!(counts.untracked, 1);
    dir.write("logs/mtgo.log", &fixture.bytes);
    run_until_settled(&mut engine, fixture.meta.now_ms());
    drop(engine);
    assert!(stored_tokens(&config).is_empty(), "no match row built");
    assert_eq!(skipped_tokens(&config), every_visible_token(&fixture));
}

/// Turning tracking back on in the same run: the skipped matches come back,
/// pushed and stored as tracking all along would have made them.
#[test]
fn retracking_brings_skipped_matches_back() {
    let (fixture, _dir, config, mut engine, _, now) = untracked_run("accounts-retrack-live");
    let visible = every_visible_token(&fixture);
    let outcome = set_tracked(&mut engine, &local(&fixture), true);
    assert_eq!(
        outcome,
        AccountEditOutcome::Done {
            retrack: visible.clone()
        }
    );
    let (events, end) = run_until_settled(&mut engine, now + 1);
    drop(engine);
    assert!(skipped_tokens(&config).is_empty(), "the rows are cleared");
    assert_eq!(stored_tokens(&config), visible, "every match is built");
    assert_eq!(last_views(&events), expected(&fixture, end), "and pushed");
    // `visible` is sorted by token, so the rebuild comes back in that order.
    let mut all_along = expected(&fixture, end);
    all_along.sort_by(|a, b| a.token.cmp(&b.token));
    assert_eq!(
        rebuilt_from_storage(&config, &visible, end),
        all_along,
        "as tracking all along would have"
    );
}

/// The same after a restart: the skipped matches are only in storage, so
/// they go through the engine's rebuild queue.
#[test]
fn retracking_after_a_restart_rebuilds_the_skipped_matches() {
    let (fixture, _dir, config, engine, _, now) = untracked_run("accounts-retrack-restart");
    drop(engine);
    let visible = every_visible_token(&fixture);
    let mut engine = open(&config, now + 1);
    run_until_settled(&mut engine, now + 1);
    assert_eq!(
        skipped_tokens(&config),
        visible,
        "still skipped after a restart"
    );
    assert!(stored_tokens(&config).is_empty());
    set_tracked(&mut engine, &local(&fixture), true);
    let (events, end) = run_until_settled(&mut engine, now + 10_000);
    drop(engine);
    assert!(skipped_tokens(&config).is_empty());
    assert_eq!(stored_tokens(&config), visible);
    assert_eq!(last_views(&events), expected(&fixture, end));
}

/// Re-tracked, then the tracker stops before its rebuild queue drains (an
/// app quit, a folder save, an error restart): the next run finds the skip
/// rows of the tracked account and rebuilds each match exactly once.
#[test]
fn retracking_survives_a_restart_before_the_queue_drains() {
    let (fixture, _dir, config, engine, _, now) = untracked_run("accounts-retrack-stop");
    drop(engine);
    let visible = every_visible_token(&fixture);
    // A later run: the skipped matches are only in storage, so re-tracking
    // queues every one for rebuild. It stops before a single tick.
    let mut engine = open(&config, now + 1);
    run_until_settled(&mut engine, now + 1);
    set_tracked(&mut engine, &local(&fixture), true);
    drop(engine);
    assert_eq!(
        skipped_tokens(&config),
        visible,
        "the skip rows stay until each match is rebuilt"
    );
    let mut engine = open(&config, now + 10_000);
    let (events, end) = run_until_settled(&mut engine, now + 10_000);
    drop(engine);
    assert!(
        skipped_tokens(&config).is_empty(),
        "each row cleared once built"
    );
    assert_eq!(stored_tokens(&config), visible, "every match, one row each");
    assert_eq!(last_views(&events), expected(&fixture, end));
}

/// A projection rebuild (a version bump) while still untracked skips again.
#[test]
fn a_rebuild_while_untracked_skips_again() {
    let (fixture, _dir, config, engine, _, now) = untracked_run("accounts-rebuild-untracked");
    drop(engine);
    let visible = every_visible_token(&fixture);
    {
        let conn = store::open_writer(&config.db_path).expect("writer");
        conn.execute("UPDATE derivations SET version = 0", [])
            .expect("version bump");
    }
    let mut engine = open(&config, now + 1);
    let (events, _) = run_until_settled(&mut engine, now + 1);
    drop(engine);
    assert!(stored_tokens(&config).is_empty(), "nothing built");
    assert_eq!(skipped_tokens(&config), visible);
    assert!(pushed_tokens(&events).is_empty());
}

/// Tick until the game syncs are done, then run the league sync at `now_ms`,
/// so the stored derivations are complete.
fn settle(engine: &mut Engine, config: &TrackerConfig, fixture: &Fixture, mut now_ms: i64) {
    for _ in 0..1_000 {
        engine.flush().expect("flush");
        now_ms += 1;
        if !engine.tick(now_ms).expect("tick").syncing {
            break;
        }
    }
    engine.flush().expect("flush");
    let mut writer = store::open_writer(&config.db_path).expect("writer");
    store::league_sync::sync_all(&mut writer, fixture.meta.tz(), fixture.meta.now_ms())
        .expect("league sync");
}

/// Every row of `sql`, each column as its debug text.
fn rows(config: &TrackerConfig, sql: &str) -> Vec<String> {
    let reader = open_reader(&config.db_path).expect("reader");
    let mut stmt = reader.prepare(sql).expect("prepare");
    let columns = stmt.column_count();
    stmt.query_map([], |row| {
        let mut values = Vec::new();
        for index in 0..columns {
            values.push(format!("{:?}", row.get_ref(index)?));
        }
        Ok(values.join(" | "))
    })
    .expect("query")
    .map(|row| row.expect("row"))
    .collect()
}

/// The stored derivations a re-tracked account must get back, keyed by
/// tokens and natural keys (row ids and write times differ between runs).
fn derivations(config: &TrackerConfig) -> Vec<(&'static str, Vec<String>)> {
    vec![
        (
            "leagues",
            rows(
                config,
                "SELECT l.run_key, a.username, l.source, l.kind, l.state, l.token, l.event_id,
                     l.mtgo_course_id, l.name, l.format, l.set_code, l.deck_version_id,
                     l.deck_id, l.started_at, l.joined_at, l.completed_at, l.dropped_at,
                     l.detached_at
                 FROM leagues l LEFT JOIN accounts a ON a.id = l.account_id
                 ORDER BY l.run_key",
            ),
        ),
        (
            "tournaments",
            rows(
                config,
                "SELECT t.mtgo_event_id, a.username, t.token, t.name, t.format, t.started_at,
                     t.name_synthesized
                 FROM tournaments t LEFT JOIN accounts a ON a.id = t.account_id
                 ORDER BY t.mtgo_event_id",
            ),
        ),
        (
            "drafts",
            rows(
                config,
                "SELECT d.draft_token, a.username, d.source, l.run_key, d.league_event_id,
                     d.mtgo_course_id, d.mtgo_draft_id, d.pod_token, d.seat_count,
                     d.seat_index, d.booster_catalog_id, d.state, d.pack_size,
                     d.picks_expected, d.started_at, d.ended_at, d.abandons_at, d.detached_at,
                     (SELECT COUNT(*) FROM draft_picks p WHERE p.draft_id = d.id)
                 FROM drafts d
                     LEFT JOIN accounts a ON a.id = d.account_id
                     LEFT JOIN leagues l ON l.id = d.league_id
                 ORDER BY d.draft_token",
            ),
        ),
        (
            "matches",
            rows(
                config,
                "SELECT m.token, l.run_key, m.deck_version_id, m.state, m.outcome,
                     m.revision IS NOT NULL
                 FROM matches m LEFT JOIN leagues l ON l.id = m.league_id
                 ORDER BY m.token",
            ),
        ),
        (
            "game_cards",
            rows(
                config,
                "SELECT m.token, g.game_number, c.side, c.catalog_id, c.main_qty, c.side_qty,
                     c.sided_in, c.sided_out, c.opening_hand, c.bottomed, c.starting_hand,
                     c.first_draw, c.seen, c.pregame_revealed, c.pregame_played, c.cast,
                     c.played, c.activated, c.discarded, c.cast_turn, c.hand_seen,
                     c.graveyard_seen, c.exile_seen, c.battlefield_seen, c.has_zone_data,
                     c.inferred
                 FROM game_cards c
                     JOIN games g ON g.id = c.game_id
                     JOIN matches m ON m.id = g.match_id
                 ORDER BY m.token, g.game_number, c.side, c.catalog_id",
            ),
        ),
    ]
}

/// Re-tracked, the account gets back the league runs, tournaments, drafts,
/// league and deck links and card rows tracking all along gives (#316).
#[test]
fn retracking_gives_back_every_stored_derivation() {
    let fixture = load_fixture(MATCHES);
    let all_along_dir = TestDir::new("accounts-derivations-all-along");
    all_along_dir.write("logs/mtgo.log", &fixture.bytes);
    let all_along = config(&all_along_dir, &fixture);
    let mut engine = open(&all_along, fixture.meta.now_ms());
    let (_, now) = run_until_settled(&mut engine, fixture.meta.now_ms());
    settle(&mut engine, &all_along, &fixture, now);
    drop(engine);

    let (fixture, _dir, retracked, mut engine, _, now) =
        untracked_run("accounts-derivations-retracked");
    set_tracked(&mut engine, &local(&fixture), true);
    let (_, now) = run_until_settled(&mut engine, now + 1);
    settle(&mut engine, &retracked, &fixture, now);
    drop(engine);

    let expected = derivations(&all_along);
    for (table, rows) in &expected {
        if *table != "tournaments" {
            assert!(!rows.is_empty(), "the fixture fills {table}");
        }
    }
    assert_ne!(
        rows(
            &all_along,
            "SELECT COUNT(*) FROM matches WHERE league_id IS NOT NULL"
        ),
        ["Integer(0)"],
        "the matches are in their league run"
    );
    assert_eq!(derivations(&retracked), expected);
}

#[test]
fn existing_matches_survive_untracking() {
    let fixture = load_fixture(MATCHES);
    let (first, _) = match_start(&fixture, 0);
    let (second, cut) = match_start(&fixture, 1);
    let dir = TestDir::new("accounts-existing");
    dir.write("logs/mtgo.log", &fixture.bytes[..cut]);
    let config = config(&dir, &fixture);
    let mut engine = open(&config, fixture.meta.now_ms());
    run_until_settled(&mut engine, fixture.meta.now_ms());
    assert!(stored_tokens(&config).contains(&first));
    set_tracked(&mut engine, &local(&fixture), false);
    dir.append("logs/mtgo.log", &fixture.bytes[cut..]);
    run_until_settled(&mut engine, fixture.meta.now_ms() + 10);
    drop(engine);
    let stored = stored_tokens(&config);
    assert!(
        stored.contains(&first),
        "a row that existed is never removed"
    );
    assert!(!stored.contains(&second), "a new one is not built");
    assert!(skipped_tokens(&config).contains(&second));
    assert!(!skipped_tokens(&config).contains(&first));
}

/// 0.x AdvanceMatchStateTest "creates a match for a known account even when
/// a different account is active": the game's players decide, not the
/// current account.
#[test]
fn a_match_is_built_for_a_known_account_while_another_is_current() {
    let fixture = load_fixture("league_join_drop");
    let dir = TestDir::new("accounts-other-current");
    let config = config(&dir, &fixture);
    let mut engine = open(&config, fixture.meta.now_ms());
    engine
        .import_accounts(
            vec![ImportedAccount {
                username: "Qother".to_string(),
                login_id: None,
                created_at_ms: None,
                tracked: true,
                active_since_ms: None,
            }],
            1,
        )
        .expect("an account");
    assert!(matches!(
        engine
            .edit_account(AccountEdit::Switch {
                username: "Qother".to_string(),
                at_ms: i64::MAX / 2,
            })
            .expect("switch"),
        AccountEditOutcome::Done { .. }
    ));
    dir.write("logs/mtgo.log", &fixture.bytes);
    run_until_settled(&mut engine, fixture.meta.now_ms());
    drop(engine);
    let reader = open_reader(&config.db_path).expect("reader");
    let other: i64 = reader
        .query_row(
            "SELECT id FROM accounts WHERE username = 'Qother'",
            [],
            |row| row.get(0),
        )
        .expect("Qother");
    assert_eq!(
        store::accounts::current_account_id(&reader, store::Platform::Mtgo).expect("current"),
        Some(other),
        "the switch is newer than the log's Login"
    );
    assert_eq!(stored_tokens(&config), every_visible_token(&fixture));
}

fn account_changed(events: &[TrackerEvent]) -> usize {
    events
        .iter()
        .filter(|event| matches!(event, TrackerEvent::AccountChanged))
        .count()
}

/// Every page refetches on one `AccountChanged`: a Login line read, a switch
/// or a tracking change. At most one per tick.
#[test]
fn a_login_a_switch_or_a_tracking_change_reports_account_changed() {
    let fixture = load_fixture("league_join_drop");
    let dir = TestDir::new("accounts-changed");
    dir.write("logs/mtgo.log", &fixture.bytes);
    let config = config(&dir, &fixture);
    let mut engine = open(&config, fixture.meta.now_ms());
    // Held while the backlog is caught up, then told once.
    let mut now = fixture.meta.now_ms();
    loop {
        let tick = engine.tick(now).expect("tick");
        now += 1;
        if !tick.more {
            assert_eq!(account_changed(&tick.events), 1, "caught up: told once");
            break;
        }
        assert_eq!(account_changed(&tick.events), 0, "not while catching up");
    }
    let (quiet, now) = run_until_settled(&mut engine, now + 1);
    assert_eq!(account_changed(&quiet), 0, "nothing new, nothing told");

    engine
        .edit_account(AccountEdit::Switch {
            username: local(&fixture),
            at_ms: 5,
        })
        .expect("switch");
    let tick = engine.tick(now + 1).expect("tick");
    assert_eq!(account_changed(&tick.events), 1, "a switch");

    set_tracked(&mut engine, &local(&fixture), false);
    let tick = engine.tick(now + 2).expect("tick");
    assert_eq!(account_changed(&tick.events), 1, "a tracking change");

    let refused = engine
        .edit_account(AccountEdit::Switch {
            username: "Qnobody".to_string(),
            at_ms: 6,
        })
        .expect("refused");
    assert_eq!(refused, AccountEditOutcome::NotFound);
    let tick = engine.tick(now + 3).expect("tick");
    assert_eq!(
        account_changed(&tick.events),
        0,
        "a refusal changes nothing"
    );
}
