use super::*;
use std::sync::mpsc::Receiver;

use decks::DeckCard;
use decks::stats::Range;
use fixture_tools::TestDir;
use serde_json::json;
use store::params;
use tracker::TrackerEvent;

use crate::commands::tests::{ready_state, sync_leagues};
use crate::leagues::gather;
use crate::matches::{MatchKindFilter, MatchListQuery, match_filter};

/// 2026-07-01T10:00:00Z.
const T0: i64 = 1_782_900_000_000;
const MINUTE: i64 = 60_000;
const HOUR: i64 = 60 * MINUTE;
const DAY: i64 = 24 * HOUR;
const UTC: chrono_tz::Tz = chrono_tz::UTC;

/// The run of Q-league (four wins on deck 1, Active).
const OPEN_RUN: &str = "Q-league:Q-league-m0";
/// The run of Q-full (five wins on deck 1, Complete).
const FULL_RUN: &str = "Q-full:Q-full-m0";

/// A ready app (OS timezone UTC) whose only account, Qlocal, has deck 1
/// (Qburn, version 10: 4 Qbolt, 4 Qgoyf, 20 Qmountain main, 3 Qrest side)
/// and deck 2 (Qcontrol, version 20); runs Q-league (Active, four wins from
/// T0, an hour apart) and Q-full (Complete, five wins from a day before),
/// both on version 10; manual league 7 of deck 1 (two days before) and 8
/// of deck 2 (three days before); and the four cards plus Qseize (5001).
fn seeded(label: &str) -> (TestDir, AppState, Receiver<TrackerEvent>) {
    let dir = TestDir::new(label);
    let (state, events) = ready_state(&dir);
    let conn = store::open_writer(&state.db_path).expect("open");
    let card = |catalog_id, quantity, sideboard| DeckCard {
        catalog_id,
        quantity,
        sideboard,
    };
    let registered = decks::signature(&[
        card(4001, 4, false),
        card(4002, 4, false),
        card(4004, 20, false),
        card(4003, 3, true),
    ]);
    conn.execute_batch(&format!(
        "INSERT INTO accounts (id, username, first_seen_at) VALUES (1, 'Qlocal', 0);
         INSERT INTO opponents (id, username, first_seen_at) VALUES (1, 'Qopponent', 0);
         INSERT INTO decks (id, mtgo_id, account_id, name, format, created_at, updated_at, source)
         VALUES (1, 'Q-deck-1', 1, 'Qburn', 'CMODERN', 0, 0, 'file'),
                (2, 'Q-deck-2', 1, 'Qcontrol', 'CMODERN', 0, 0, 'file');
         INSERT INTO deck_versions (id, deck_id, signature, modified_at)
         VALUES (10, 1, '{registered}', {v10}), (20, 2, 'Q-sig-20', {v10});
         INSERT INTO cards (catalog_id, oracle_id, name, type, image) VALUES
             (4001, 'o-bolt', 'Qbolt', 'Instant', 'https://cards.scryfall.io/q/1.jpg'),
             (4002, 'o-goyf', 'Qgoyf', 'Creature', NULL),
             (4003, 'o-rest', 'Qrest', 'Enchantment', NULL),
             (4004, 'o-mountain', 'Qmountain', 'Basic Land', NULL),
             (5001, 'o-seize', 'Qseize', 'Sorcery', NULL);",
        v10 = T0 - 10 * DAY,
    ))
    .expect("seed");
    manual_league(&conn, 7, 1, "Qmanual seven", T0 - 2 * DAY);
    manual_league(&conn, 8, 2, "Qmanual eight", T0 - 3 * DAY);
    league(&conn, "Q-league", T0, 4);
    league(&conn, "Q-full", T0 - DAY, 5);
    sync_leagues(&conn);
    (dir, state, events)
}

/// Manual league `id` of Qlocal's deck `deck_id`, made at `at` (the deck's
/// format and the version it had then).
fn manual_league(conn: &Connection, id: i64, deck_id: i64, name: &str, at: i64) {
    conn.execute(
        "INSERT INTO leagues (id, run_key, account_id, source, kind, state, name, format,
             deck_version_id, deck_id, started_at, created_at, token)
         VALUES (?1, 'manual:' || ?1, 1, 'manual', 'constructed', 'complete', ?2,
             (SELECT format FROM decks WHERE id = ?3),
             (SELECT id FROM deck_versions WHERE deck_id = ?3 ORDER BY modified_at, id LIMIT 1),
             ?3, ?4, ?4, 'manual_Q' || ?1)",
        params![id, name, deck_id, at],
    )
    .expect("manual league");
}

/// `count` complete log wins of `token` on version 10, an hour apart.
fn league(conn: &Connection, token: &str, at: i64, count: i64) {
    for index in 0..count {
        let match_token = format!("{token}-m{index}");
        let started_at = at + index * HOUR;
        let id: i64 = conn
            .query_row(
                "INSERT INTO matches (token, account_id, opponent_id, source, format, match_type,
                     state, outcome, started_at, ended_at, last_activity_at, revision,
                     derived_version, league_token, game_structure)
                 VALUES (?1, 1, 1, 'log', 'CMODERN', 'Constructed', 'complete', 'win', ?2, ?3,
                     ?3, 1, 4, ?4, 'Modern')
                 RETURNING id",
                params![match_token, started_at, started_at + 20 * MINUTE, token],
                |row| row.get(0),
            )
            .expect("match");
        for number in 1..=2_i64 {
            conn.execute(
                "INSERT INTO games (match_id, game_number, result, end_reason, on_play,
                     started_at, ended_at)
                 VALUES (?1, ?2, 'win', 'win', 1, ?3, ?4)",
                params![
                    id,
                    number,
                    started_at + number * MINUTE,
                    started_at + number * 5 * MINUTE
                ],
            )
            .expect("game");
        }
        conn.execute(
            "UPDATE matches SET deck_version_id = 10 WHERE token = ?1",
            [&match_token],
        )
        .expect("link");
    }
}

fn game(won: bool, on_play: bool) -> ManualGameInput {
    ManualGameInput {
        won,
        on_play,
        turns: None,
    }
}

/// 0.x `manualMatchPayload`: Qkai, won, lost, won, at 15:00 for 40 minutes
/// (after Q-league's matches), in `league`.
fn form(deck_id: u32, league: Option<&str>) -> ManualMatchInput {
    ManualMatchInput {
        deck_id,
        opponent_name: " Qkai ".to_string(),
        league: league.map(str::to_string),
        started_at: "2026-07-01T15:00".to_string(),
        ended_at: "2026-07-01T15:40".to_string(),
        games: vec![game(true, true), game(false, false), game(true, true)],
        archetype_id: None,
        ranked: None,
        best_of: None,
        rank_gap: None,
    }
}

fn errors(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(field, message)| (field.to_string(), message.to_string()))
        .collect()
}

fn invalid(pairs: &[(&str, &str)]) -> ManualMatchSaveView {
    ManualMatchSaveView::Invalid {
        errors: errors(pairs),
    }
}

fn created(state: &AppState, input: &ManualMatchInput) -> String {
    match create_match(state, input, T0 + DAY).expect("create") {
        ManualMatchSaveView::Created { token, message } => {
            assert_eq!(message, "Manual match added.");
            token
        }
        refused => panic!("not created: {refused:?}"),
    }
}

fn keys(options: &[ManualLeagueOption]) -> Vec<&str> {
    options.iter().map(|option| option.key.as_str()).collect()
}

fn options(state: &AppState, deck_id: Option<i64>) -> Vec<ManualLeagueOption> {
    league_options(&lock(&state.reader), deck_id).expect("options")
}

fn text(state: &AppState, sql: &str, token: &str) -> Option<String> {
    lock(&state.reader)
        .query_row(sql, [token], |row| row.get(0))
        .expect("a value")
}

fn number(state: &AppState, sql: &str, token: &str) -> Option<i64> {
    lock(&state.reader)
        .query_row(sql, [token], |row| row.get(0))
        .expect("a value")
}

// ---- League options ----------------------------------------------------------

/// 0.x `GetManualMatchLeagueOptionsTest`: runs with room left and manual
/// leagues with room, of the deck or of none, newest first; full runs and
/// another deck's are left out.
#[test]
fn league_options_are_open_runs_and_manual_leagues_of_the_deck_newest_first() {
    let (_dir, state, _events) = seeded("manual-league-options");
    let deck_one = options(&state, Some(1));
    assert_eq!(keys(&deck_one), vec![OPEN_RUN, "manual:7"]);
    let run = &deck_one[0];
    assert_eq!(
        (
            run.deck_id,
            run.match_count,
            run.round_count,
            run.started_at_ms
        ),
        (Some(1), 4, 5, T0)
    );
    assert!(run.name.contains("League"), "the run's name: {}", run.name);
    let manual = &deck_one[1];
    assert_eq!(
        (
            manual.name.as_str(),
            manual.deck_id,
            manual.match_count,
            manual.round_count
        ),
        ("Qmanual seven", Some(1), 0, 5)
    );
    assert_eq!(keys(&options(&state, Some(2))), vec!["manual:8"]);
    assert_eq!(
        keys(&options(&state, None)),
        vec![OPEN_RUN, "manual:7", "manual:8"]
    );
    assert!(options(&state, Some(404)).is_empty());

    // A run with no version is offered to every deck.
    {
        let conn = store::open_writer(&state.db_path).expect("open");
        conn.execute(
            "UPDATE matches SET deck_version_id = NULL WHERE token LIKE 'Q-league-%'",
            [],
        )
        .expect("unlink");
        sync_leagues(&conn);
    }
    assert_eq!(keys(&options(&state, Some(2))), vec![OPEN_RUN, "manual:8"]);
    crate::commands::stop_tracker(&state);
}

/// 0.x `finds an open league buried under more than fifty full runs`, and
/// the newest 50 only.
#[test]
fn league_options_skip_full_runs_before_keeping_the_newest_fifty() {
    let (_dir, state, _events) = seeded("manual-league-options-limit");
    {
        let conn = store::open_writer(&state.db_path).expect("open");
        for index in 0..55 {
            league(
                &conn,
                &format!("Q-old-full-{index}"),
                T0 - 30 * DAY - index * DAY,
                5,
            );
        }
        for index in 0..60_i64 {
            manual_league(&conn, 100 + index, 1, "Qmore", T0 - 100 * DAY - index * DAY);
        }
        sync_leagues(&conn);
    }
    let found = options(&state, Some(1));
    assert_eq!(found.len(), 50);
    assert_eq!(&keys(&found)[..2], &[OPEN_RUN, "manual:7"]);
    crate::commands::stop_tracker(&state);
}

// ---- Adding a match ----------------------------------------------------------

/// 0.x `files under a league using its version and completes a full run`:
/// the fifth match of an Active run completes it.
#[test]
fn a_manual_match_in_an_active_run_counts_and_completes_it_at_five() {
    let (_dir, state, _events) = seeded("manual-create-run");
    let token = created(&state, &form(1, Some(OPEN_RUN)));
    assert_eq!(
        text(
            &state,
            "SELECT league_token FROM matches WHERE token = ?1",
            &token
        ),
        Some("Q-league".to_string())
    );
    assert_eq!(
        text(
            &state,
            "SELECT game_structure FROM matches WHERE token = ?1",
            &token
        ),
        Some("Modern".to_string())
    );
    assert_eq!(
        number(
            &state,
            "SELECT deck_version_id FROM matches WHERE token = ?1",
            &token
        ),
        Some(10)
    );
    let gathered = gather(&lock(&state.reader), &[1]).expect("runs");
    let run = gathered
        .runs
        .iter()
        .find(|run| run.key == OPEN_RUN)
        .expect("the run");
    assert_eq!(run.input.matches.len(), 5);
    assert_eq!(run.input.matches[4].token, token);
    assert_eq!(run.input.state, RunState::Complete);

    assert_eq!(keys(&options(&state, Some(1))), vec!["manual:7"]);
    // 0.x `rejects a league holding a full run`.
    assert_eq!(
        create_match(&state, &form(1, Some(OPEN_RUN)), T0).expect("checked"),
        invalid(&[(
            "league_id",
            "This league already has a full run of matches."
        )])
    );
    assert_eq!(
        create_match(&state, &form(1, Some(FULL_RUN)), T0).expect("checked"),
        invalid(&[(
            "league_id",
            "This league already has a full run of matches."
        )])
    );
    crate::commands::stop_tracker(&state);
}

/// The fold places a manual match by its start, so one that starts before
/// its derived run's first match would start a new run and orphan the run's
/// notes and drops: refused. A later start, or a manual league, is fine.
#[test]
fn a_manual_match_must_start_after_its_derived_runs_first_match() {
    let (_dir, state, _events) = seeded("manual-create-early");
    // Q-league's first match starts at 10:00.
    let mut early = form(1, Some(OPEN_RUN));
    early.started_at = "2026-07-01T09:59".to_string();
    early.ended_at = "2026-07-01T10:30".to_string();
    assert_eq!(
        create_match(&state, &early, T0 + DAY).expect("checked"),
        invalid(&[(
            "started_at",
            "The match must start after this league's first match."
        )])
    );
    // After the first match: accepted.
    created(&state, &form(1, Some(OPEN_RUN)));
    // A manual league takes an early start.
    let mut manual = form(1, Some("manual:7"));
    manual.started_at = "2026-07-01T09:59".to_string();
    manual.ended_at = "2026-07-01T10:30".to_string();
    created(&state, &manual);
    crate::commands::stop_tracker(&state);
}

/// 0.x `rejects a league played with another deck`, and the other form
/// refusals, each under its 0.x field; a refusal writes nothing.
#[test]
fn a_refused_form_answers_with_0x_messages_by_field() {
    let (_dir, state, _events) = seeded("manual-create-refused");
    assert_eq!(
        create_match(&state, &form(2, Some(OPEN_RUN)), T0).expect("checked"),
        invalid(&[("league_id", "This league was played with a different deck.")])
    );
    assert_eq!(
        create_match(&state, &form(1, Some("manual:8")), T0).expect("checked"),
        invalid(&[("league_id", "This league was played with a different deck.")])
    );
    for unknown in ["Q-league:Q-none", "manual:404", "manual:x"] {
        assert_eq!(
            create_match(&state, &form(1, Some(unknown)), T0).expect("checked"),
            invalid(&[("league_id", "The selected league id is invalid.")]),
            "{unknown}"
        );
    }
    for league in [None, Some(OPEN_RUN)] {
        assert_eq!(
            create_match(&state, &form(404, league), T0).expect("checked"),
            invalid(&[("deck_id", "The selected deck id is invalid.")])
        );
    }
    let blank = ManualMatchInput {
        opponent_name: "  ".to_string(),
        games: vec![game(true, true), game(true, true), game(true, true)],
        ..form(1, None)
    };
    assert_eq!(
        create_match(&state, &blank, T0).expect("checked"),
        invalid(&[("opponent_name", "The opponent name field is required.")])
    );
    // Rule errors, then league and deck errors, arrive together (0.x).
    let blank_in_a_wrong_deck_league = ManualMatchInput {
        opponent_name: "  ".to_string(),
        ..form(2, Some(OPEN_RUN))
    };
    assert_eq!(
        create_match(&state, &blank_in_a_wrong_deck_league, T0).expect("checked"),
        invalid(&[
            ("league_id", "This league was played with a different deck."),
            ("opponent_name", "The opponent name field is required."),
        ])
    );
    let blank_with_no_deck = ManualMatchInput {
        opponent_name: "  ".to_string(),
        ..form(404, None)
    };
    assert_eq!(
        create_match(&state, &blank_with_no_deck, T0).expect("checked"),
        invalid(&[
            ("deck_id", "The selected deck id is invalid."),
            ("opponent_name", "The opponent name field is required."),
        ])
    );
    let three_wins = ManualMatchInput {
        games: vec![game(true, true), game(true, true), game(true, true)],
        ..form(1, None)
    };
    // 0.x StoreManualMatchRequest `exists:archetypes,id`.
    let unknown_archetype = ManualMatchInput {
        archetype_id: Some(99_999),
        ..form(1, None)
    };
    assert_eq!(
        create_match(&state, &unknown_archetype, T0).expect("checked"),
        invalid(&[("archetype_id", "The selected archetype id is invalid.")])
    );
    assert_eq!(
        create_match(&state, &three_wins, T0).expect("checked"),
        invalid(&[(
            "games",
            "A best-of-three match cannot have more than two wins or two losses."
        )])
    );
    let turns = ManualMatchInput {
        games: vec![ManualGameInput {
            turns: Some(100),
            ..game(true, true)
        }],
        ..form(1, None)
    };
    assert_eq!(
        create_match(&state, &turns, T0).expect("checked"),
        invalid(&[(
            "games.0.turns",
            "The games.0.turns field must not be greater than 99."
        )])
    );
    let manual_rows: i64 = lock(&state.reader)
        .query_row(
            "SELECT COUNT(*) FROM matches WHERE source = 'manual'",
            [],
            |row| row.get(0),
        )
        .expect("count");
    assert_eq!(manual_rows, 0);
    crate::commands::stop_tracker(&state);
}

#[test]
fn a_manual_match_in_a_manual_league_is_linked_to_it() {
    let (_dir, state, _events) = seeded("manual-create-manual-league");
    let token = created(&state, &form(1, Some("manual:7")));
    assert_eq!(
        number(
            &state,
            "SELECT league_id FROM matches WHERE token = ?1 AND league_linked = 1",
            &token
        ),
        Some(7)
    );
    assert_eq!(
        text(
            &state,
            "SELECT league_token FROM matches WHERE token = ?1",
            &token
        ),
        None
    );
    let gathered = gather(&lock(&state.reader), &[1]).expect("runs");
    let manual = gathered
        .runs
        .iter()
        .find(|run| run.key == "manual:7")
        .expect("the manual league");
    assert_eq!(manual.rows.len(), 1);
    assert_eq!(manual.rows[0].token, token);
    let seven = options(&state, Some(1))
        .into_iter()
        .find(|option| option.key == "manual:7")
        .expect("still has room");
    assert_eq!(seven.match_count, 1);
    crate::commands::stop_tracker(&state);
}

/// An active 0.x constructed run (0.x id 77) with no matches and no deck,
/// started half an hour after T0, imported while v1 has two accounts: it has
/// no account.
fn accountless_run(conn: &mut Connection) {
    conn.execute(
        "INSERT INTO accounts (id, username, first_seen_at) VALUES (2, 'Qsecond', 0)",
        [],
    )
    .expect("a second account");
    let league = store::import_leagues::ImportedLeague {
        legacy_id: 77,
        manual: false,
        kind: "constructed".to_string(),
        set_code: None,
        course_id: None,
        token: "Q-0x-run".to_string(),
        event_id: None,
        name: "Qmodern League".to_string(),
        format: "CMODERN".to_string(),
        state: "active".to_string(),
        started_at_ms: T0 + 30 * MINUTE,
        joined_at_ms: None,
        completed_at_ms: None,
        dropped_at_ms: None,
        created_at_ms: None,
        deck: None,
        notes: None,
        matches: Vec::new(),
    };
    let counts = store::import_leagues::import_leagues(conn, std::slice::from_ref(&league), 1)
        .expect("league");
    assert_eq!(counts.without_account, 1);
    sync_leagues(conn);
}

/// Issue #162: 0.x offered every constructed run with room
/// (`GetManualMatchLeagueOptions` has no account filter), so an imported
/// run with no account is offered too; a match placed in it gives the run
/// the match's account, and a rerun of the 0.x import keeps it.
#[test]
fn an_imported_run_with_no_account_is_offered_and_takes_the_match_s_account() {
    let (_dir, state, _events) = seeded("manual-accountless-run");
    {
        let mut conn = store::open_writer(&state.db_path).expect("open");
        accountless_run(&mut conn);
    }
    assert_eq!(
        keys(&options(&state, Some(1))),
        vec!["0x:77", OPEN_RUN, "manual:7"]
    );
    let token = created(&state, &form(1, Some("0x:77")));
    assert_eq!(
        number(
            &state,
            "SELECT account_id FROM leagues WHERE run_key = ?1",
            "0x:77"
        ),
        Some(1)
    );
    let gathered = gather(&lock(&state.reader), &[1]).expect("runs");
    let run = gathered
        .runs
        .iter()
        .find(|run| run.key == "0x:77")
        .expect("the run is the account's");
    assert_eq!(run.input.matches.len(), 1);
    assert_eq!(run.input.matches[0].token, token);

    // A rerun of the 0.x import finds no account of its own: v1's stays.
    {
        let mut conn = store::open_writer(&state.db_path).expect("open");
        let league = store::import_leagues::ImportedLeague {
            legacy_id: 77,
            manual: false,
            kind: "constructed".to_string(),
            set_code: None,
            course_id: None,
            token: "Q-0x-run".to_string(),
            event_id: None,
            name: "Qmodern League".to_string(),
            format: "CMODERN".to_string(),
            state: "active".to_string(),
            started_at_ms: T0 + 30 * MINUTE,
            joined_at_ms: None,
            completed_at_ms: None,
            dropped_at_ms: None,
            created_at_ms: None,
            deck: None,
            notes: None,
            matches: Vec::new(),
        };
        let counts = store::import_leagues::import_leagues(&mut conn, &[league], 2).expect("rerun");
        assert_eq!((counts.unchanged, counts.without_account), (1, 0));
    }
    assert_eq!(
        number(
            &state,
            "SELECT account_id FROM leagues WHERE run_key = ?1",
            "0x:77"
        ),
        Some(1)
    );
    crate::commands::stop_tracker(&state);
}

// ---- Editing a game ------------------------------------------------------------

fn hand(mulligans: u32, kept: &[u32], bottomed: &[u32], put_back: usize) -> ManualHandInput {
    ManualHandInput {
        mulligan_count: mulligans,
        starting_hand: kept.to_vec(),
        bottomed: bottomed.to_vec(),
        mulliganed_hands: vec![Vec::new(); put_back],
    }
}

fn saved(message: &str) -> Result<ManualEditView, CommandError> {
    Ok(ManualEditView::Saved {
        message: message.to_string(),
    })
}

fn edit_invalid(pairs: &[(&str, &str)]) -> Result<ManualEditView, CommandError> {
    Ok(ManualEditView::Invalid {
        errors: errors(pairs),
    })
}

fn change(catalog_id: u32, quantity: u32, kind: SideboardKind) -> ManualSideboardChangeInput {
    ManualSideboardChangeInput {
        catalog_id,
        quantity,
        kind,
    }
}

/// 0.x `UpdateManualGame{Hand,Sideboard,Reveals}Test` through the commands:
/// each edit's flash message, its field errors, and the match page showing
/// the records.
#[test]
fn hand_sideboard_and_reveal_edits_answer_like_0x_and_show_on_the_page() {
    let (_dir, state, _events) = seeded("manual-edits");
    let token = created(&state, &form(1, None));
    let one_mulligan = hand(1, &[4001, 4001, 4002, 4004, 4004, 4004], &[4002], 1);
    assert_eq!(
        set_hand(&state, &token, 1, &one_mulligan),
        saved("Opening hand saved.")
    );
    assert_eq!(
        set_hand(&state, &token, 1, &hand(0, &[4001, 4001, 4001], &[], 0)),
        edit_invalid(&[(
            "kept_hand",
            "The final hand is seven cards: the ones you kept plus the ones you bottomed."
        )])
    );
    assert_eq!(
        set_sideboard(&state, &token, 1, &[change(4003, 1, SideboardKind::In)]),
        edit_invalid(&[("changes", "Game 1 is played with the registered deck.")])
    );

    let with_goyfs = hand(0, &[4002, 4002, 4001, 4004, 4004, 4004, 4004], &[], 0);
    assert_eq!(
        set_hand(&state, &token, 2, &with_goyfs),
        saved("Opening hand saved.")
    );
    let goyfs_out = [
        change(4002, 4, SideboardKind::Out),
        change(4003, 3, SideboardKind::In),
    ];
    assert_eq!(
        set_sideboard(&state, &token, 2, &goyfs_out),
        saved("Sideboard saved. Opening hand cleared because it used a card you sided out.")
    );
    assert_eq!(
        set_sideboard(
            &state,
            &token,
            3,
            &[
                change(4001, 1, SideboardKind::Out),
                change(4003, 1, SideboardKind::In)
            ]
        ),
        saved("Sideboard saved.")
    );

    let seize = ManualRevealInput {
        catalog_id: 5001,
        quantity: 2,
    };
    assert_eq!(
        set_reveals(&state, &token, 1, &[seize]),
        saved("Revealed cards saved.")
    );
    let unknown = ManualRevealInput {
        catalog_id: 9999,
        quantity: 1,
    };
    assert_eq!(
        set_reveals(&state, &token, 1, &[unknown]),
        edit_invalid(&[(
            "cards.0.mtgo_id",
            "The selected cards.0.mtgo_id is invalid."
        )])
    );
    assert_eq!(
        set_reveals(&state, "q-none", 1, &[seize]),
        Err(CommandError::Refused(
            "This match no longer exists.".to_string()
        ))
    );
    assert_eq!(
        set_reveals(&state, &token, 9, &[seize]),
        Err(CommandError::Refused(
            "This game no longer exists.".to_string()
        ))
    );
    assert_eq!(
        set_reveals(&state, "Q-league-m0", 1, &[seize]),
        Err(CommandError::Refused(
            "Only a match entered by hand can be edited here.".to_string()
        ))
    );

    let detail = crate::matches::detail(&lock(&state.reader), &token, &[])
        .expect("detail")
        .expect("listed");
    assert!(detail.manual);
    let first = &detail.games[0];
    assert_eq!(first.opening_hand.len(), 7);
    // Hand order: the bottomed Qgoyf (4002) follows its card's kept copy.
    let bottomed: Vec<(u32, bool)> = first
        .opening_hand
        .iter()
        .map(|card| (card.catalog_id, card.bottomed))
        .collect();
    assert_eq!(bottomed.iter().filter(|(_, bottomed)| *bottomed).count(), 1);
    assert!(bottomed.contains(&(4002, true)));
    assert_eq!(first.local_mulligans, 1);
    assert_eq!(first.opponent_cards_seen.len(), 1);
    assert_eq!(first.opponent_cards_seen[0].name, "Qseize");
    let second = &detail.games[1];
    assert!(second.opening_hand.is_empty(), "the edit cleared it");
    assert_eq!(second.local_mulligans, 0);
    assert_eq!(second.sideboard_changes.len(), 2);
    let editing = detail.manual_editing.expect("options");
    assert_eq!(editing.games[2].mains.len(), 4);

    crate::commands::stop_tracker(&state);
    assert_eq!(
        set_reveals(&state, &token, 1, &[seize]),
        Err(CommandError::Tracker("tracking is not running".to_string()))
    );
}

/// The match list's "manual" type finds the matches entered by hand only.
#[test]
fn the_manual_type_filter_lists_only_manual_matches() {
    let (_dir, state, _events) = seeded("manual-list-filter");
    let first = created(&state, &form(1, None));
    let second = created(&state, &form(1, Some("manual:7")));
    let query = MatchListQuery {
        kind: Some(MatchKindFilter::Manual),
        ..MatchListQuery::newest_first(1)
    };
    let filter = match_filter(&query, Range::all_time(), UTC).expect("filter");
    let (rows, total) =
        crate::live::read_page(&lock(&state.reader), store::Platform::Mtgo, &filter, 1)
            .expect("page");
    let mut tokens: Vec<String> = rows.iter().map(|row| row.token.clone()).collect();
    tokens.sort();
    let mut expected = vec![first, second];
    expected.sort();
    assert_eq!((tokens, total), (expected, 2));
    crate::commands::stop_tracker(&state);
}

// ---- Card search -------------------------------------------------------------

#[test]
fn a_card_search_needs_two_to_a_hundred_characters() {
    let (_dir, state, _events) = seeded("manual-card-search");
    let conn = lock(&state.reader);
    let refused = |message: &str| Err(CommandError::Refused(message.to_string()));
    assert_eq!(search(&conn, "   "), refused("The q field is required."));
    assert_eq!(
        search(&conn, " q "),
        refused("The q field must be at least 2 characters.")
    );
    assert_eq!(
        search(&conn, &"q".repeat(101)),
        refused("The q field must not be greater than 100 characters.")
    );
    assert_eq!(
        search(&conn, " qB "),
        Ok(vec![CardSearchView {
            catalog_id: 4001,
            oracle_id: Some("o-bolt".to_string()),
            name: "Qbolt".to_string(),
            image: Some("https://cards.scryfall.io/q/1.jpg".to_string()),
            local_image: None,
            type_line: Some("Instant".to_string()),
        }])
    );
    drop(conn);
    crate::commands::stop_tracker(&state);
}

// ---- Editing a match ---------------------------------------------------------

/// Whether the tracker pushed both `DecksChanged` and `LeaguesChanged` (the
/// overlay worker refreshes on them too) within 30 seconds.
fn saw_decks_and_leagues_changed(events: &Receiver<TrackerEvent>) -> bool {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let (mut decks, mut leagues) = (false, false);
    while std::time::Instant::now() < deadline && !(decks && leagues) {
        match events.recv_timeout(std::time::Duration::from_millis(100)) {
            Ok(TrackerEvent::DecksChanged) => decks = true,
            Ok(TrackerEvent::LeaguesChanged) => leagues = true,
            _ => {}
        }
    }
    decks && leagues
}

fn correction(opponent: Option<&str>, ended_at: Option<&str>) -> MatchCorrectionInput {
    MatchCorrectionInput {
        opponent_name: opponent.map(str::to_string),
        started_at: Some("2026-07-01T10:00".to_string()),
        ended_at: ended_at.map(str::to_string),
        games: vec![game(false, true), game(false, false)],
    }
}

/// "Edit match" on a log match: the sheet opens with the log's values, the
/// fix is saved in place and read by the match page, and the tracker
/// pushes the deck and league refreshes the windows and overlays follow.
#[test]
fn a_log_match_is_corrected_in_place_and_the_windows_are_told() {
    let (_dir, state, events) = seeded("manual-correct");
    let match_id = number(
        &state,
        "SELECT id FROM matches WHERE token = ?1",
        "Q-league-m0",
    )
    .expect("the match") as u32;

    let form = edit_form(&lock(&state.reader), match_id).expect("the form");
    assert_eq!(form.opponent_name.as_deref(), Some("Qopponent"));
    assert_eq!(form.started_at_ms, T0);
    assert_eq!(form.ended_at_ms, Some(T0 + 20 * MINUTE));
    assert_eq!(form.games.len(), 2);
    assert_eq!(form.games[0].result, "win");
    assert_eq!(form.games[0].on_play, Some(true));
    assert!(!form.corrected);
    assert_eq!(form.platform, crate::platform::PlatformView::Mtgo);

    // Add's messages: a missing start, an end that is no date, a 3-0.
    let mut broken = correction(Some(" "), Some("soon"));
    broken.started_at = Some(String::new());
    broken.games.push(game(false, true));
    assert_eq!(
        correct(&state, match_id, &broken, T0 + DAY),
        Ok(ManualEditView::Invalid {
            errors: errors(&[
                ("started_at", "The started at field is required."),
                ("ended_at", "The ended at field must be a valid date."),
                (
                    "games",
                    "A best-of-three match cannot have more than two wins or two losses."
                ),
            ]),
        })
    );

    assert_eq!(
        correct(
            &state,
            match_id,
            &correction(Some(" Qrival "), Some("2026-07-01T10:30")),
            T0 + DAY
        ),
        Ok(ManualEditView::Saved {
            message: "Match updated.".to_string()
        })
    );
    assert!(saw_decks_and_leagues_changed(&events));
    let detail = crate::matches::detail(&lock(&state.reader), "Q-league-m0", &[])
        .expect("detail")
        .expect("listed");
    assert!(detail.corrected);
    assert_eq!(detail.opponent.as_deref(), Some("Qrival"));
    assert_eq!((detail.wins, detail.losses), (0, 2));
    assert_eq!(detail.ended_at_ms, Some(T0 + 30 * MINUTE));
    let form = edit_form(&lock(&state.reader), match_id).expect("the form");
    assert!(form.corrected);
    assert_eq!(form.games[1].result, "loss");

    assert_eq!(
        correct(&state, 99_999, &correction(None, None), T0 + DAY),
        Err(CommandError::Refused(
            "This match no longer exists.".to_string()
        ))
    );
    assert_eq!(
        edit_form(&lock(&state.reader), 99_999),
        Err(CommandError::Refused(
            "This match no longer exists.".to_string()
        ))
    );
    crate::commands::stop_tracker(&state);
}

/// A start or end the sheet sends as `null` (the player left it alone) keeps
/// the stored time to the millisecond: the sheet shows minutes only.
#[test]
fn a_time_left_alone_keeps_the_stored_milliseconds() {
    let (_dir, state, _events) = seeded("manual-correct-kept-times");
    let match_id = number(
        &state,
        "SELECT id FROM matches WHERE token = ?1",
        "Q-league-m0",
    )
    .expect("the match") as u32;
    store::open_writer(&state.db_path)
        .expect("open")
        .execute(
            "UPDATE matches SET started_at = ?1, ended_at = ?2 WHERE id = ?3",
            params![T0 + 42_123, T0 + 20 * MINUTE + 7_456, match_id],
        )
        .expect("times with seconds");
    let mut kept = correction(Some("Qopponent"), None);
    kept.started_at = None;
    assert_eq!(
        correct(&state, match_id, &kept, T0 + DAY),
        Ok(ManualEditView::Saved {
            message: "Match updated.".to_string()
        })
    );
    let form = edit_form(&lock(&state.reader), match_id).expect("the form");
    assert_eq!(form.started_at_ms, T0 + 42_123);
    assert_eq!(form.ended_at_ms, Some(T0 + 20 * MINUTE + 7_456));
    assert_eq!(form.games[0].result, "loss");
    crate::commands::stop_tracker(&state);
}

// ---- Editing an Arena match ----------------------------------------------------

/// A ready app with an Arena account and a tracked Arena match, `q-arena`,
/// against opponent 50: the opaque Arena id `Qopaque-id`, shown as Qrival.
/// Returns the match row id, and the tracker's events (the tracker stops
/// once nothing receives them).
fn arena_match_seeded(label: &str) -> (TestDir, AppState, u32, Receiver<TrackerEvent>) {
    let dir = TestDir::new(label);
    let (state, events) = ready_state(&dir);
    let conn = store::open_writer(&state.db_path).expect("open");
    let account = crate::rank::tests::arena_account(&conn);
    conn.execute_batch(&format!(
        "INSERT INTO opponents (id, username, first_seen_at, platform, display_name)
         VALUES (50, 'Qopaque-id', 0, 'arena', 'Qrival');
         INSERT INTO matches (token, account_id, opponent_id, source, format, match_type,
             state, outcome, started_at, ended_at, last_activity_at, revision,
             derived_version)
         VALUES ('q-arena', {account}, 50, 'log', 'CSTANDARD', 'ranked', 'complete', 'win',
             {T0}, {end}, {end}, 1, 1);",
        end = T0 + 20 * MINUTE,
    ))
    .expect("an Arena match");
    let match_id = number(&state, "SELECT id FROM matches WHERE token = ?1", "q-arena")
        .expect("the match") as u32;
    (dir, state, match_id, events)
}

/// The Arena match's opponent: `(username, display_name, platform)`, or
/// `None` without one.
fn arena_opponent(state: &AppState) -> Option<(String, Option<String>, String)> {
    lock(&state.reader)
        .query_row(
            "SELECT o.username, o.display_name, o.platform FROM matches m
             LEFT JOIN opponents o ON o.id = m.opponent_id WHERE m.token = 'q-arena'",
            [],
            |row| {
                let username: Option<String> = row.get(0)?;
                let display_name: Option<String> = row.get(1)?;
                let platform: Option<String> = row.get(2)?;
                Ok(username.map(|username| (username, display_name, platform.unwrap_or_default())))
            },
        )
        .expect("the match")
}

fn saved_correction(state: &AppState, match_id: u32, opponent: Option<&str>) {
    assert_eq!(
        correct(state, match_id, &correction(opponent, None), T0 + DAY),
        Ok(ManualEditView::Saved {
            message: "Match updated.".to_string()
        })
    );
}

#[test]
fn editing_an_arena_match_shows_the_opponents_display_name() {
    let (_dir, state, match_id, _events) = arena_match_seeded("manual-edit-arena-name");
    let form = edit_form(&lock(&state.reader), match_id).expect("the form");
    assert_eq!(form.opponent_name.as_deref(), Some("Qrival"));
    assert_eq!(form.platform, crate::platform::PlatformView::Arena);
    // An Arena opponent with no shown name shows its username.
    store::open_writer(&state.db_path)
        .expect("open")
        .execute_batch("UPDATE opponents SET display_name = NULL WHERE id = 50")
        .expect("no shown name");
    let form = edit_form(&lock(&state.reader), match_id).expect("the form");
    assert_eq!(form.opponent_name.as_deref(), Some("Qopaque-id"));
    crate::commands::stop_tracker(&state);
}

#[test]
fn saving_an_arena_edit_with_the_same_name_keeps_the_opponent() {
    let (_dir, state, match_id, _events) = arena_match_seeded("manual-edit-arena-same");
    saved_correction(&state, match_id, Some(" Qrival "));
    assert_eq!(
        number(
            &state,
            "SELECT opponent_id FROM matches WHERE token = ?1",
            "q-arena"
        ),
        Some(50)
    );
    assert_eq!(
        number(
            &state,
            "SELECT COUNT(*) FROM opponents WHERE username <> ?1",
            ""
        ),
        Some(1),
        "no new opponent"
    );
    crate::commands::stop_tracker(&state);
}

#[test]
fn renaming_an_arena_opponent_makes_a_named_opponent() {
    let (_dir, state, match_id, _events) = arena_match_seeded("manual-edit-arena-rename");
    saved_correction(&state, match_id, Some(" Qother "));
    assert_eq!(
        arena_opponent(&state),
        Some((
            "arena:name:Qother".to_string(),
            Some("Qother".to_string()),
            "arena".to_string()
        ))
    );
    // The tracked opponent is left as it was.
    assert_eq!(
        text(
            &state,
            "SELECT display_name FROM opponents WHERE username = ?1",
            "Qopaque-id"
        ),
        Some("Qrival".to_string())
    );
    let form = edit_form(&lock(&state.reader), match_id).expect("the form");
    assert_eq!(form.opponent_name.as_deref(), Some("Qother"));
    crate::commands::stop_tracker(&state);
}

#[test]
fn an_arena_edit_may_clear_the_opponent() {
    let (_dir, state, match_id, _events) = arena_match_seeded("manual-edit-arena-clear");
    saved_correction(&state, match_id, Some("  "));
    assert_eq!(arena_opponent(&state), None);
    let form = edit_form(&lock(&state.reader), match_id).expect("the form");
    assert_eq!(form.opponent_name, None);
    crate::commands::stop_tracker(&state);
}

// ---- Arena rank gaps -----------------------------------------------------------

/// A ready app with only the rank tests' Arena account (`rank::tests::seed`:
/// readings at offsets 100, 300, 500 and 700, the gap ending at 500 played
/// elsewhere), plus a reading at 750 (Platinum 4 step 1, same counters): a
/// second played elsewhere gap, after `q-b`'s. Returns the deck `Qdeck`,
/// and the tracker's events (the tracker stops once nothing receives them).
fn arena_seeded(label: &str) -> (TestDir, AppState, i64, Receiver<TrackerEvent>) {
    let dir = TestDir::new(label);
    let (state, events) = ready_state(&dir);
    let conn = store::open_writer(&state.db_path).expect("open");
    let (account, own) = crate::rank::tests::seed(&conn);
    conn.execute(
        "INSERT INTO arena_rank_observations (account_id, ladder, season, observed_at, class,
             level, step, matches_won, matches_lost, arena_log_file_id, byte_offset)
         VALUES (?1, 'constructed', 95, 750000, 'Platinum', 4, 1, 3, 0, 1, 750)",
        [account],
    )
    .expect("reading");
    (dir, state, own, events)
}

fn gap_options(state: &AppState, deck_id: i64) -> Vec<RankGapOption> {
    rank_gap_options(&lock(&state.reader), deck_id, UTC).expect("gaps")
}

#[test]
fn manual_rank_gaps_lists_played_elsewhere_gaps_newest_first() {
    let (_dir, state, own, _events) = arena_seeded("manual-rank-gaps");
    let options = gap_options(&state, own);
    assert_eq!(
        options,
        vec![
            RankGapOption {
                key: RankGapKey {
                    file_id: 1,
                    byte_offset: 750
                },
                label: "Platinum 4 to Platinum 4, 1 Jan 00:11 to 00:12".to_string(),
                change_steps: 1,
                claims: 0,
            },
            RankGapOption {
                key: RankGapKey {
                    file_id: 1,
                    byte_offset: 500
                },
                label: "Gold 2 to Gold 1, 1 Jan 00:05 to 00:08".to_string(),
                change_steps: 5,
                claims: 0,
            },
        ]
    );
    // An MTGO deck has none.
    let conn = store::open_writer(&state.db_path).expect("open");
    conn.execute_batch(
        "INSERT INTO accounts (id, username, first_seen_at) VALUES (77, 'Qlocal', 0);
         INSERT INTO decks (id, mtgo_id, account_id, name, format, created_at, updated_at, source)
         VALUES (77, 'Q-mtgo', 77, 'Qmtgo', 'CMODERN', 0, 0, 'file');",
    )
    .expect("mtgo deck");
    assert!(gap_options(&state, 77).is_empty());
}

#[test]
fn a_claimed_manual_match_makes_its_gap_attributed() {
    let (_dir, state, own, _events) = arena_seeded("manual-rank-claim");
    let before = crate::rank::deck_rank_view(&lock(&state.reader), own, None)
        .expect("rank")
        .expect("a line");
    assert_eq!(
        before.line.gaps[1].kind,
        crate::rank::GapKindView::PlayedElsewhere
    );
    let input = ManualMatchInput {
        deck_id: own as u32,
        opponent_name: String::new(),
        rank_gap: Some(RankGapKey {
            file_id: 1,
            byte_offset: 500,
        }),
        ..form(own as u32, None)
    };
    // An Arena match has no league, before or after the form's own rules.
    for games in [vec![game(true, true); 3], vec![game(true, true)]] {
        let in_a_league = ManualMatchInput {
            league: Some("manual:7".to_string()),
            games,
            ..input.clone()
        };
        let answer = create_match(&state, &in_a_league, T0).expect("checked");
        let ManualMatchSaveView::Invalid { errors } = answer else {
            panic!("not refused: {answer:?}");
        };
        assert_eq!(
            errors.get("league_id").map(String::as_str),
            Some("Arena matches have no leagues.")
        );
    }
    let token = created(&state, &input);
    let rank = crate::rank::deck_rank_view(&lock(&state.reader), own, None)
        .expect("rank")
        .expect("a line");
    assert_eq!(rank.line.gaps[1].kind, crate::rank::GapKindView::Attributed);
    assert!(rank.line.gaps[1].of_deck);
    assert_eq!(rank.climb.attributed, 3);
    let manual = rank
        .line
        .matches
        .iter()
        .find(|m| m.token == token)
        .expect("listed");
    assert!(manual.manual && manual.of_deck);
    assert_eq!(manual.gap, Some(1));
    // The gap is still offered, now with its claim.
    let options = gap_options(&state, own);
    let claimed = options
        .iter()
        .find(|option| option.key.byte_offset == 500)
        .expect("still offered");
    assert_eq!(claimed.claims, 1);
}

// ---- What the UI sends and receives ------------------------------------------

#[test]
fn the_ui_sends_and_receives_camel_case_and_tagged_answers() {
    let input: ManualMatchInput = serde_json::from_value(json!({
        "deckId": 1,
        "opponentName": "Qkai",
        "league": "manual:7",
        "startedAt": "2026-07-01T15:00",
        "endedAt": "2026-07-01T15:40",
        "games": [{"won": true, "onPlay": false, "turns": 7}]
    }))
    .expect("input");
    assert_eq!(input.league.as_deref(), Some("manual:7"));
    assert_eq!(input.games[0].turns, Some(7));
    let hand: ManualHandInput = serde_json::from_value(json!({
        "mulliganCount": 0, "startingHand": [1], "bottomed": [], "mulliganedHands": [[]]
    }))
    .expect("hand");
    assert_eq!(hand.mulliganed_hands, vec![Vec::<u32>::new()]);
    let fix: MatchCorrectionInput = serde_json::from_value(json!({
        "opponentName": null,
        "startedAt": null,
        "endedAt": null,
        "games": [{"won": false, "onPlay": true, "turns": null}]
    }))
    .expect("correction");
    assert_eq!(
        (fix.opponent_name, fix.started_at, fix.ended_at),
        (None, None, None)
    );
    let side: ManualSideboardChangeInput =
        serde_json::from_value(json!({"catalogId": 4003, "quantity": 2, "kind": "in"}))
            .expect("change");
    assert_eq!(side.kind, SideboardKind::In);
    assert_eq!(
        serde_json::to_value(ManualMatchSaveView::Created {
            token: "q".to_string(),
            message: "Manual match added.".to_string(),
        })
        .expect("json"),
        json!({"kind": "created", "token": "q", "message": "Manual match added."})
    );
    assert_eq!(
        serde_json::to_value(ManualEditView::Invalid {
            errors: errors(&[("cards", "Each card can only be listed once.")]),
        })
        .expect("json"),
        json!({"kind": "invalid", "errors": {"cards": "Each card can only be listed once."}})
    );
}
