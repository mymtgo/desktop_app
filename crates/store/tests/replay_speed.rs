//! The replay speed budgets (replays spec section 12), measured and
//! reported, not CI gates:
//! - `replay_view` for one game of 2,000 frames (archive read, decode,
//!   decorate) at p95 under 300 ms;
//! - the snapshot build, redaction, validation and gzip for a 3-game match
//!   of 2,000 frames each under 1.5 s.
//!
//! Ignored by default because debug builds are much slower. Run them in
//! release:
//! `cargo test --release -p store --test replay_speed -- --ignored --nocapture`.
//!
//! The data is synthetic: one complete log match of Qplayer against Qfoe,
//! three games of 2,000 frames each, 60 cards a frame, all archived by
//! compaction. For the view about 60% of frames repeat the one before (as
//! MTGO's do); the view measure reads the whole match (`replay_inputs`,
//! three games) and decorates one game, which is more than one game's view
//! needs. For the snapshot every frame differs, so all 6,000 frames are
//! shared; the measure is the whole share up to the network: the read,
//! the card lookup, `build_snapshot`, `prepare_shared_snapshot` and the
//! gzipped request.

use std::time::{Duration, Instant};

use chrono::NaiveDate;
use fixture_tools::TestDir;
use mtgo_protocol::{EventKind, Subjects};
use replay::{CardInfo, ImageMode, build_snapshot, decorate_frames, prepare_shared_snapshot};
use store::replay::{card_infos, replay_inputs};
use store::write::{NewEvent, NewLogFile, WriteBatch, commit, insert_log_file};
use store::{open_reader, open_writer, params};

const GAMES: u64 = 3;
const FRAMES: u64 = 2_000;
const CARDS: u32 = 60;
const RUNS: usize = 20;
const BUDGET: Duration = Duration::from_millis(300);
const SNAPSHOT_RUNS: usize = 5;
const SNAPSHOT_BUDGET: Duration = Duration::from_millis(1_500);

/// Frame `step` of a game: the board changes every few steps, so most
/// frames repeat the one before; with `distinct` every frame differs.
fn frame_json(step: u64, distinct: bool) -> String {
    let change = if distinct { step } else { step * 2 / 5 };
    let mut cards = Vec::new();
    for card in 0..CARDS {
        let zone = if u64::from(card) < change % u64::from(CARDS) {
            "Battlefield"
        } else {
            "Library"
        };
        cards.push(format!(
            r#"{{"Id":{},"CatalogID":{},"Zone":"{zone}","ActualZone":"{zone}","Owner":{},"Controller":{}}}"#,
            300 + card,
            1_000 + card % 40,
            card % 2,
            card % 2
        ));
    }
    format!(
        r#"{{"Players":[{{"Id":0,"Name":"Qplayer","LibraryCount":53,"HandCount":7,"Life":{}}},{{"Id":1,"Name":"Qfoe","LibraryCount":53,"HandCount":7,"Life":20}}],"Cards":[{}]}}"#,
        20 - (change % 20),
        cards.join(",")
    )
}

fn build(conn: &mut store::Connection, distinct: bool) -> i64 {
    conn.execute_batch(
        "INSERT INTO accounts (id, username, first_seen_at) VALUES (1, 'Qplayer', 0);
         INSERT INTO opponents (id, username, first_seen_at) VALUES (1, 'Qfoe', 0);
         INSERT INTO matches (id, token, mtgo_match_id, account_id, opponent_id, source, format,
             state, outcome, started_at, ended_at, last_activity_at, revision, derived_version)
         VALUES (1, 'q-speed', 99, 1, 1, 'log', 'CMODERN', 'complete', 'win', 0, 30000000,
             30000000, 1, 1);",
    )
    .expect("match");
    for game in 1..=GAMES {
        conn.execute(
            "INSERT INTO games (match_id, mtgo_game_id, game_number, result, end_reason,
                 started_at, ended_at)
             VALUES (1, ?1, ?1, 'win', 'win', ?2, ?3)",
            params![
                game as i64,
                (game as i64 - 1) * 10_000_000,
                game as i64 * 10_000_000
            ],
        )
        .expect("game");
    }
    let mut card = conn
        .prepare(
            "INSERT INTO cards (catalog_id, name, type, image) VALUES (?1, ?2, 'Creature', ?3)",
        )
        .expect("card");
    for id in 1_000..1_040 {
        card.execute(params![
            id,
            format!("Qcard {id}"),
            format!("https://cards.example/{id}.jpg")
        ])
        .expect("card");
    }
    drop(card);

    let file = insert_log_file(
        conn,
        &NewLogFile {
            path: "/q/mtgo.log".to_string(),
            identity_hash: 1,
            head_hash: 2,
            head_len: 10,
            created_at_fs: None,
            day_anchor: NaiveDate::from_ymd_opt(2026, 10, 9).expect("date"),
            timezone: "UTC".to_string(),
            inherited_username: Some("Qplayer".to_string()),
            seen_at_ms: 0,
        },
    )
    .expect("file");
    let mut offset = 0;
    for game in 1..=GAMES {
        let mut events = Vec::new();
        for step in 0..FRAMES {
            offset += 10_000;
            let second = step / 2;
            let time = format!(
                "{:02}:{:02}:{:02}",
                game * 2 + second / 3600,
                (second / 60) % 60,
                second % 60
            );
            events.push(NewEvent {
                log_file_id: file,
                byte_offset: offset,
                kind: Some(EventKind::GameStateUpdate),
                subjects: Subjects {
                    match_id: Some(99),
                    game_id: Some(game),
                    ..Subjects::default()
                },
                logged_at_ms: offset as i64,
                raw: format!(
                    "{time} [INF] (Twitch Info|Game Play Status Update for Game ID: {game}, Match ID: 99, Event ID: 99) {}",
                    frame_json(step, distinct)
                ),
                quarantined: false,
            });
        }
        commit(
            conn,
            &WriteBatch {
                events,
                now_ms: 0,
                ..WriteBatch::default()
            },
        )
        .expect("events");
    }
    let moved = store::compact::compact_match(conn, "q-speed", i64::MAX / 2).expect("compact");
    assert_eq!(moved as u64, GAMES * FRAMES, "every frame archived");
    1
}

#[test]
#[ignore = "speed budget: run in release with --ignored --nocapture"]
fn replay_inputs_under_300_ms() {
    let dir = TestDir::new("replay-speed");
    let path = dir.join("mymtgo.sqlite");
    let mut conn = open_writer(&path).expect("open");
    let match_id = build(&mut conn, false);
    drop(conn);
    let reader = open_reader(&path).expect("reader");

    let mut timings = Vec::new();
    let mut kept = 0;
    for _ in 0..RUNS {
        let started = Instant::now();
        let inputs = replay_inputs(&reader, match_id, chrono_tz::UTC, &[])
            .expect("read")
            .expect("a match");
        let game = inputs.games[0].clone();
        let ids: Vec<u32> = (1_000..1_040).collect();
        let cards = card_infos(&reader, &ids).expect("cards");
        let decorated =
            decorate_frames(game.frames, &inputs.local_username, &cards, ImageMode::App);
        timings.push(started.elapsed());
        kept = inputs
            .games
            .iter()
            .map(|game| game.frames.len())
            .sum::<usize>();
        assert!(!decorated.is_empty());
    }
    timings.sort();
    let p95 = timings[(RUNS * 95).div_ceil(100) - 1];
    println!(
        "replay speed: {GAMES} games x {FRAMES} frames archived, {kept} kept after repeats; \
         min {:?}, median {:?}, p95 {p95:?} (budget {BUDGET:?})",
        timings[0],
        timings[RUNS / 2]
    );
    assert!(p95 < BUDGET, "p95 {p95:?} over {BUDGET:?}");
}

#[test]
#[ignore = "speed budget: run in release with --ignored --nocapture"]
fn snapshot_build_and_prepare_under_1_5_s() {
    let dir = TestDir::new("replay-snapshot-speed");
    let path = dir.join("mymtgo.sqlite");
    let mut conn = open_writer(&path).expect("open");
    let match_id = build(&mut conn, true);
    drop(conn);
    let reader = open_reader(&path).expect("reader");

    let mut timings = Vec::new();
    // Read, build, prepare and gzip, summed over the runs.
    let mut phases = [Duration::ZERO; 4];
    let mut shared = 0;
    let mut gzip = 0;
    for _ in 0..SNAPSHOT_RUNS {
        let started = Instant::now();
        let inputs = replay_inputs(&reader, match_id, chrono_tz::UTC, &[])
            .expect("read")
            .expect("a match");
        let read = Instant::now();
        let cards = |ids: &[u32]| -> std::collections::BTreeMap<u32, CardInfo> {
            card_infos(&reader, ids).expect("cards")
        };
        let snapshot = build_snapshot(inputs.snapshot_input(), &cards);
        shared = snapshot.games.iter().map(|game| game.frames.len()).sum();
        let built = Instant::now();
        let prepared = prepare_shared_snapshot(snapshot, &inputs.opponent_names).expect("prepared");
        let ready = Instant::now();
        let request = mymtgo_api::share_replay_request(&inputs.token, 1, &prepared);
        let done = Instant::now();
        timings.push(done - started);
        for (phase, took) in
            phases
                .iter_mut()
                .zip([read - started, built - read, ready - built, done - ready])
        {
            *phase += took;
        }
        gzip = request.body.bytes().len();
        assert!(!String::from_utf8_lossy(prepared.json()).contains("Qfoe"));
    }
    timings.sort();
    let mean = |phase: Duration| phase / SNAPSHOT_RUNS as u32;
    println!(
        "snapshot speed: {GAMES} games, {shared} frames shared, {gzip} bytes gzipped; \
         min {:?}, median {:?}, max {:?} (budget {SNAPSHOT_BUDGET:?}); \
         mean read {:?}, build {:?}, prepare {:?}, gzip {:?}",
        timings[0],
        timings[SNAPSHOT_RUNS / 2],
        timings[SNAPSHOT_RUNS - 1],
        mean(phases[0]),
        mean(phases[1]),
        mean(phases[2]),
        mean(phases[3])
    );
    assert_eq!(shared as u64, GAMES * FRAMES, "every frame shared");
    // The median, so one run slowed by other work on the machine does not
    // decide; every run is printed above.
    let median = timings[SNAPSHOT_RUNS / 2];
    assert!(
        median < SNAPSHOT_BUDGET,
        "{median:?} over {SNAPSHOT_BUDGET:?}"
    );
}
