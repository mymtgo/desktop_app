//! Limited league runs from real draft logs, end to end (0.x
//! `DraftLeaguePipelineTest`): the engine stores the log, the league sync
//! stores the draft, its league and its matches, and
//! `store::leagues::stored_limited_runs` reads them.

mod common;

use common::*;
use fixture_tools::{Fixture, TestDir, load_fixture};
use leagues::{LeagueKind, RunState};
use limited::DraftState;
use store::league_sync::{LeagueOwner, sync_all};
use store::leagues::{
    LeagueEdit, LeagueEditOutcome, LimitedRuns, RunFilter, UserState, edit_league,
    stored_limited_runs,
};
use store::{open_reader, open_writer};
use tracker::TrackerConfig;

const LEAGUE_TOKEN: &str = "48a2e914-f2ee-4fce-a4ad-47e396488889";
const DRAFT_TOKEN: &str = "791bacca-caea-4d88-b6c7-3bc067d412c2";
/// The constructed match played before the draft.
const CONSTRUCTED_MATCH: &str = "c0d8ff76-dc13-480b-bdf0-a228a6cd20f8";

/// The fixture stored by the engine, in `dir`.
fn ingest(name: &str, label: &str) -> (TestDir, Fixture, TrackerConfig) {
    let fixture = load_fixture(name);
    let dir = TestDir::new(label);
    dir.write("logs/mtgo.log", &fixture.bytes);
    let config = config(&dir, &fixture);
    let mut engine = open(&config, fixture.meta.now_ms());
    run_until_settled(&mut engine, fixture.meta.now_ms());
    drop(engine);
    (dir, fixture, config)
}

fn account(config: &TrackerConfig, fixture: &Fixture) -> i64 {
    let reader = open_reader(&config.db_path).expect("reader");
    reader
        .query_row(
            "SELECT id FROM accounts WHERE username = ?1",
            [&fixture.meta.local_username],
            |row| row.get(0),
        )
        .expect("the local account")
}

/// The account's limited runs as the pages read them, after a league sync
/// at the fixture's time.
fn limited(config: &TrackerConfig, fixture: &Fixture) -> LimitedRuns {
    let mut writer = open_writer(&config.db_path).expect("writer");
    sync_all(&mut writer, fixture.meta.tz(), fixture.meta.now_ms()).expect("league sync");
    drop(writer);
    let reader = open_reader(&config.db_path).expect("reader");
    read_limited(&reader, account(config, fixture))
}

fn read_limited(reader: &store::Connection, account: i64) -> LimitedRuns {
    stored_limited_runs(
        reader,
        &RunFilter {
            owners: vec![LeagueOwner::Account(account)],
            ..RunFilter::default()
        },
    )
    .expect("limited runs")
}

#[test]
fn the_hobbit_draft_is_one_draft_run_with_its_three_matches() {
    let (_dir, fixture, config) = ingest("draft_hobbit", "limited-runs-hobbit");
    let found = limited(&config, &fixture);

    assert_eq!(found.runs.len(), 1);
    assert!(found.unlinked_drafts.is_empty());
    let limited = &found.runs[0];
    let run = &limited.run;
    assert_eq!(run.key, format!("draft:{DRAFT_TOKEN}"));
    assert_eq!(run.kind, LeagueKind::Draft);
    assert_eq!(run.set_code.as_deref(), Some("HOB"));
    assert_eq!(run.event_id, Some(11039));
    assert_eq!(run.course_id, Some(35_746_768));
    assert_eq!(run.token, LEAGUE_TOKEN);
    assert_eq!(run.state, RunState::Complete);
    assert_eq!(run.draft_token.as_deref(), Some(DRAFT_TOKEN));

    let draft = limited.draft.as_ref().expect("the draft");
    assert_eq!(draft.state, DraftState::Finished);
    assert_eq!(draft.picks.len(), 42);

    assert_eq!(run.match_tokens.len(), 3);
    assert_eq!(limited.matches.len(), 3);
    assert!(
        limited
            .matches
            .iter()
            .all(|row| row.format.as_deref() == Some("DHOBHOBHOB"))
    );
    // One registered deck per match, even where match 3 reuses match 2's.
    assert_eq!(limited.registered.len(), 3);
    assert!(
        !run.match_tokens
            .iter()
            .any(|token| token == CONSTRUCTED_MATCH)
    );

    // The constructed reads never see the draft run's matches.
    let reader = open_reader(&config.db_path).expect("reader");
    let constructed = store::leagues::stored_runs(
        &reader,
        &RunFilter {
            owners: vec![LeagueOwner::Account(account(&config, &fixture))],
            kinds: vec![LeagueKind::Constructed],
            ..RunFilter::default()
        },
    )
    .expect("constructed runs");
    for other in &constructed {
        assert_eq!(other.kind, LeagueKind::Constructed);
        assert!(
            other
                .matches
                .iter()
                .all(|row| !run.match_tokens.contains(&row.token))
        );
    }
}

#[test]
fn the_incomplete_draft_is_an_active_draft_run_without_matches() {
    let (_dir, fixture, config) = ingest("draft_incomplete", "limited-runs-incomplete");
    let found = limited(&config, &fixture);
    assert_eq!(found.runs.len(), 1);
    let limited = &found.runs[0];
    assert_eq!(limited.run.kind, LeagueKind::Draft);
    assert_eq!(limited.run.event_id, Some(10814));
    assert_eq!(limited.run.set_code.as_deref(), Some("MSH"));
    assert_eq!(limited.run.state, RunState::Active);
    assert!(limited.matches.is_empty());
    assert!(limited.registered.is_empty());
    let draft = limited.draft.as_ref().expect("the draft");
    assert_eq!(draft.state, DraftState::Finished);
    assert_eq!(draft.picks.len(), 42);
}

#[test]
fn a_second_read_and_a_rebuild_give_the_same_runs() {
    let (_dir, fixture, config) = ingest("draft_hobbit", "limited-runs-rebuild");
    let first = limited(&config, &fixture);
    assert_eq!(limited(&config, &fixture), first);

    // The same log stored again from scratch.
    let (_again_dir, _, again) = ingest("draft_hobbit", "limited-runs-rebuild-again");
    assert_eq!(limited(&again, &fixture), first);
}

#[test]
fn notes_and_drops_reach_the_draft_run_by_its_key() {
    let (_dir, fixture, config) = ingest("draft_incomplete", "limited-runs-edits");
    let key = limited(&config, &fixture).runs[0].run.key.clone();
    assert!(key.starts_with("draft:"));
    let mut writer = open_writer(&config.db_path).expect("writer");
    let notes = LeagueEdit::SetNotes {
        run_key: key.clone(),
        notes: "Q note".to_string(),
        now_ms: 5,
    };
    assert_eq!(
        edit_league(&mut writer, &notes, chrono_tz::UTC).expect("notes"),
        LeagueEditOutcome::Done
    );
    let dropped = LeagueEdit::SetStatus {
        run_key: key.clone(),
        status: Some(UserState::Dropped),
        now_ms: 7,
    };
    assert_eq!(
        edit_league(&mut writer, &dropped, chrono_tz::UTC).expect("drop"),
        LeagueEditOutcome::Done
    );
    drop(writer);
    let run = &limited(&config, &fixture).runs[0].run;
    assert_eq!(run.state, RunState::Dropped);
    assert_eq!(run.dropped_at, Some(7));
}

/// The hobbit log replayed as a second entry of the same league the next
/// day: new draft token, course, draft id, pack ids, game ids, match tokens
/// and match ids (0.x `DraftLeaguePipelineTest`).
fn second_entry(text: &str) -> String {
    let mut text = text
        .replace(DRAFT_TOKEN, "aaaaaaaa-0000-4000-8000-000000000002")
        .replace("\"CourseID\":35746768", "\"CourseID\":35746999")
        .replace("\"DraftID\":6781001", "\"DraftID\":6781999")
        .replace("\"DraftGroupID\":6781001", "\"DraftGroupID\":6781999")
        .replace("2026-08-22T", "2026-08-23T");
    for game_id in [
        959762288_u64,
        959762636,
        959762762,
        959763634,
        959764088,
        959764748,
        959764884,
    ] {
        text = text.replace(&game_id.to_string(), &(game_id + 1_000_000).to_string());
    }
    for (from, to) in [
        (
            "08efb7ad-3670-4fab-b015-97cea823cde1",
            "bbbbbbbb-0000-4000-8000-000000000001",
        ),
        (
            "614c280d-a378-4c64-bcd1-73f1739fd192",
            "bbbbbbbb-0000-4000-8000-000000000002",
        ),
        (
            "6250e242-fcec-4d44-b642-9268b6581719",
            "bbbbbbbb-0000-4000-8000-000000000003",
        ),
        ("289328482", "289329482"),
        ("289328793", "289329793"),
        ("289328829", "289329829"),
    ] {
        text = text.replace(from, to);
    }
    // Pack ids 143682090..143682199 move to a range of their own.
    let mut moved = String::with_capacity(text.len());
    let mut rest = text.as_str();
    while let Some(at) = rest.find("\"PackID\":1436820") {
        moved.push_str(&rest[..at]);
        moved.push_str("\"PackID\":1436828");
        rest = &rest[at + "\"PackID\":1436820".len()..];
    }
    moved.push_str(rest);
    moved.replace("\"PackID\":1436821", "\"PackID\":1436829")
}

#[test]
fn two_entries_of_the_same_league_are_two_runs() {
    let (dir, fixture, config) = ingest("draft_hobbit", "limited-runs-two-entries");
    let text = String::from_utf8(fixture.bytes.clone()).expect("utf-8 fixture");
    let mut grown = fixture.bytes.clone();
    grown.extend_from_slice(second_entry(&text).as_bytes());
    dir.write("logs/mtgo.log", &grown);
    let mut engine = open(&config, fixture.meta.now_ms());
    run_until_settled(&mut engine, fixture.meta.now_ms());
    drop(engine);

    let found = limited(&config, &fixture);
    let mut runs: Vec<_> = found
        .runs
        .iter()
        .filter(|limited| limited.run.event_id == Some(11039))
        .collect();
    runs.sort_by_key(|limited| limited.run.started_at);
    assert_eq!(runs.len(), 2);
    for limited in &runs {
        assert_eq!(limited.run.state, RunState::Complete);
        assert_eq!(limited.matches.len(), 3);
        assert_eq!(
            limited.draft.as_ref().map(|draft| draft.picks.len()),
            Some(42)
        );
    }
    assert_eq!(runs[1].run.course_id, Some(35_746_999));
    assert!(found.unlinked_drafts.is_empty());
}

/// The Limited read budget (foundation spec section 8: 100 ms for a page
/// read): `stored_limited_runs` over the hobbit log, read again as the pages
/// do. Run in release: `cargo test -p tracker --release --test limited_runs
/// -- --ignored`. Ignored by default because debug builds are much slower.
#[test]
#[ignore = "timing check, run in release"]
fn limited_runs_are_read_within_the_page_budget() {
    let (_dir, fixture, config) = ingest("draft_hobbit", "limited-runs-budget");
    let reader = open_reader(&config.db_path).expect("reader");
    let account = account(&config, &fixture);
    let read = || read_limited(&reader, account);

    let started = std::time::Instant::now();
    let first = read();
    let first_ms = started.elapsed().as_secs_f64() * 1000.0;

    let mut best = f64::MAX;
    for _ in 0..5 {
        let started = std::time::Instant::now();
        let again = read();
        best = best.min(started.elapsed().as_secs_f64() * 1000.0);
        assert_eq!(again, first);
    }

    println!(
        "stored_limited_runs (hobbit): first read {first_ms:.2} ms, then best of 5 = {best:.2} ms"
    );
    assert!(first_ms < 100.0, "the first read took {first_ms:.2} ms");
    assert!(best < 100.0, "a later read took {best:.2} ms");
}
