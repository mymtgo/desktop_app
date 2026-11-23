//! The current account read's speed budget (release spec 19: under 5 ms on
//! the 10,000-match database; it runs once per page read, 29 call sites).
//!
//! Ignored by default because debug builds are much slower. Run it in
//! release:
//! `cargo test --release -p store --test accounts_speed -- --ignored --nocapture`.
//!
//! The data set is synthetic (names are fakes starting with Q): 4 accounts,
//! one untracked; 400 log files with a Login line each and 50 other events
//! per file; 10,000 log matches spread over the accounts; a stored switch.

use std::time::{Duration, Instant};

use fixture_tools::TestDir;
use mtgo_protocol::{EventKind, Subjects};
use store::accounts::{AccountEdit, current_account_id, edit_account};
use store::write::{NewAccount, NewEvent, NewLogFile, WriteBatch, commit, insert_log_file};
use store::{Connection, open_writer, params};

const ACCOUNTS: [&str; 4] = ["Qplayer", "Qsecond", "Qthird", "Qfourth"];
const FILES: usize = 400;
const EVENTS_PER_FILE: u64 = 50;
const MATCHES: usize = 10_000;
const START_MS: i64 = 1_780_000_000_000;
const HOUR_MS: i64 = 3_600_000;

fn build(conn: &mut Connection) {
    for file in 0..FILES {
        let username = ACCOUNTS[file % ACCOUNTS.len()];
        let at_ms = START_MS + file as i64 * HOUR_MS;
        let id = insert_log_file(
            conn,
            &NewLogFile {
                path: format!("/logs/{file}/mtgo.log"),
                identity_hash: file as u64,
                head_hash: file as u64,
                head_len: 10,
                created_at_fs: Some(at_ms),
                day_anchor: chrono::NaiveDate::from_ymd_opt(2026, 6, 1).expect("date"),
                timezone: "UTC".to_string(),
                inherited_username: Some(username.to_string()),
                seen_at_ms: at_ms,
            },
        )
        .expect("file");
        let mut events = vec![NewEvent {
            log_file_id: id,
            byte_offset: 0,
            kind: Some(EventKind::Login),
            subjects: Subjects::default(),
            logged_at_ms: at_ms,
            raw: format!("12:00:00 [INF] (Login|MtGO Login Success) Username: {username} (7)"),
            quarantined: false,
        }];
        for offset in 1..=EVENTS_PER_FILE {
            events.push(NewEvent {
                log_file_id: id,
                byte_offset: offset * 100,
                kind: Some(EventKind::MatchStateChanged),
                subjects: Subjects {
                    match_token: Some(format!("Q-{file}-{offset}")),
                    ..Subjects::default()
                },
                logged_at_ms: at_ms + offset as i64,
                raw: "state change".to_string(),
                quarantined: false,
            });
        }
        commit(
            conn,
            &WriteBatch {
                events,
                accounts: vec![NewAccount {
                    username: username.to_string(),
                    login_id: None,
                    seen_at_ms: at_ms,
                    logged_at_ms: Some(at_ms),
                }],
                now_ms: at_ms,
                ..WriteBatch::default()
            },
        )
        .expect("file events");
    }
    let tx = conn.transaction().expect("tx");
    for index in 0..MATCHES {
        let account = (index % ACCOUNTS.len()) as i64 + 1;
        let at_ms = START_MS + index as i64 * 60_000;
        tx.execute(
            "INSERT INTO matches (token, account_id, source, state, outcome, started_at,
                 last_activity_at, revision, derived_version)
             VALUES (?1, ?2, 'log', 'complete', 'win', ?3, ?3, 1, 1)",
            params![format!("Q-match-{index}"), account, at_ms],
        )
        .expect("match");
    }
    tx.commit().expect("commit");
    for edit in [
        AccountEdit::SetTracked {
            username: "Qfourth".to_string(),
            tracked: false,
        },
        AccountEdit::Switch {
            username: "Qsecond".to_string(),
            at_ms: START_MS + 10 * HOUR_MS,
        },
    ] {
        edit_account(conn, &edit).expect("edit");
    }
}

#[test]
#[ignore = "timing check, run in release"]
fn current_account_under_5_ms() {
    let dir = TestDir::new("accounts-speed");
    let mut conn = open_writer(&dir.join("mymtgo.sqlite")).expect("open");
    build(&mut conn);
    let matches: i64 = conn
        .query_row("SELECT COUNT(*) FROM matches", [], |row| row.get(0))
        .expect("count");
    assert_eq!(matches, MATCHES as i64);
    let reader = store::open_reader(&dir.join("mymtgo.sqlite")).expect("reader");
    // The last file's Login (Qfourth, untracked) does not count; the
    // newest tracked one (file 398, Qthird) is newer than the switch.
    let third: i64 = reader
        .query_row(
            "SELECT id FROM accounts WHERE username = 'Qthird'",
            [],
            |row| row.get(0),
        )
        .expect("Qthird");
    assert_eq!(
        current_account_id(&reader, store::Platform::Mtgo).expect("read"),
        Some(third)
    );

    let mut samples: Vec<Duration> = Vec::new();
    for _ in 0..500 {
        let started = Instant::now();
        let account = current_account_id(&reader, store::Platform::Mtgo).expect("read");
        samples.push(started.elapsed());
        assert_eq!(account, Some(third));
    }
    samples.sort();
    let p50 = samples[samples.len() / 2].as_secs_f64() * 1000.0;
    let p95 = samples[samples.len() * 95 / 100].as_secs_f64() * 1000.0;
    println!();
    println!("| Measure | Budget | p50 (ms) | p95 (ms) |");
    println!("|---|---|---|---|");
    println!("| current_account read | < 5 ms | {p50:.3} | {p95:.3} |");
    assert!(p95 < 5.0, "p95 {p95:.3} ms misses the 5 ms budget");
}
