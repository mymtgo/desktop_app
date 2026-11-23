//! The worst-case 0.x import (Plan T task 15, owner question): the biggest
//! user has 2,603 matches, so a doubled worst case is built here: 5,200
//! complete constructed matches of 3 games, each game with 100 state frames
//! (60% of them repeating the one before, as 0.x's do), card stats rows and
//! a decoded game log, 5,000 distinct opponent archetypes (one per match,
//! about), 260 decks with 3 versions, 1,040 leagues, manual archetypes with
//! variants, sideboard guides and matchup notes. Every name is a fake
//! starting with Q; nothing comes from a real database.
//!
//! The import runs end to end as the app runs it (`run_import` on a running
//! tracker: match batches, details, decks, leagues, Limited, the API state,
//! the archetypes, the settings save and the card stub scan with its 30 s
//! timeout). Each step is timed from the import's own progress reports and
//! the process's resident memory is sampled (`ps`, no new dependency) for
//! each step's peak.
//!
//! Ignored by default (debug builds are much slower). Run it in release:
//! `cargo test --release -p app --lib import_scale -- --ignored --nocapture`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use decks::DeckCard;
use fixture_tools::TestDir;
use store::params;

use super::{ImportProgress, run_import};
use crate::bridge::lock;
use crate::commands::tests::ready_state;

const SCHEMA: &str = include_str!("../../../fixtures/0x/schema.sql");

const MATCHES: i64 = 5_200;
const GAMES_PER_MATCH: i64 = 3;
const FRAMES_PER_GAME: i64 = 100;
/// The most cards a frame shows (a frame grows to it over the game).
const FRAME_CARDS: i64 = 50;
const ARCHETYPES: i64 = 5_000;
const MANUAL_ARCHETYPES: i64 = 50;
const DECKS: i64 = 260;
const VERSIONS_PER_DECK: i64 = 3;
const MATCHES_PER_LEAGUE: i64 = 5;
const OPPONENTS: i64 = 2_000;
/// Catalog ids the decks and games use; 0.x has card rows for the first
/// `KNOWN_CARDS` of them, so the rest need stubs.
const FIRST_CATALOG_ID: i64 = 10_001;
const CARD_POOL: i64 = 4_000;
const KNOWN_CARDS: i64 = 3_000;
const LOCAL_STATS_PER_GAME: i64 = 20;
const OPPONENT_STATS_PER_GAME: i64 = 10;
const LOG_LINES_PER_GAME: i64 = 30;
const GUIDES_PER_DECK: i64 = 5;
const NOTES_PER_GUIDE: i64 = 2;

/// 2026-01-01 00:00:00 UTC, the first match's start.
const START_MS: i64 = 1_767_225_600_000;

fn datetime(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .unwrap_or_default()
        .format("%Y-%m-%d %H:%M:%S")
        .to_string()
}

fn datetime_ms(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .unwrap_or_default()
        .format("%Y-%m-%d %H:%M:%S%.3f")
        .to_string()
}

fn catalog(index: i64) -> i64 {
    FIRST_CATALOG_ID + index.rem_euclid(CARD_POOL)
}

/// Deck `deck` (0-based): 15 maindeck cards of 4 and 5 sideboard cards of 3,
/// shifted by one card per version.
fn deck_list(deck: i64, version: i64) -> Vec<(i64, i64, bool)> {
    let base = deck * 13 + version;
    let mut cards = Vec::new();
    for index in 0..15 {
        cards.push((catalog(base + index), 4, false));
    }
    for index in 15..20 {
        cards.push((catalog(base + index), 3, true));
    }
    cards
}

fn signature(list: &[(i64, i64, bool)]) -> String {
    let cards: Vec<DeckCard> = list
        .iter()
        .map(|(catalog_id, quantity, sideboard)| DeckCard {
            catalog_id: *catalog_id as u32,
            quantity: *quantity as u32,
            sideboard: *sideboard,
        })
        .collect();
    decks::signature(&cards)
}

/// One state frame (0.x `game_timelines.content`): both players and the
/// first `visible` cards, the local ones from the deck, the opponent's from
/// the pool.
fn frame(local: &[(i64, i64, bool)], opponent_base: i64, visible: i64, step: i64) -> String {
    let mut cards = Vec::new();
    for index in 0..visible {
        let (id, catalog_id, owner, zone) = if index % 3 == 2 {
            (
                500 + index,
                catalog(opponent_base + index),
                2,
                "Battlefield",
            )
        } else {
            let (catalog_id, _, _) = local[(index as usize) % 15];
            let zone = if index < 7 { "Hand" } else { "Battlefield" };
            (100 + index, catalog_id, 1, zone)
        };
        cards.push(format!(
            "{{\"Id\":{id},\"CatalogID\":{catalog_id},\"Zone\":\"{zone}\",\"ActualZone\":\"{zone}\",\"Owner\":{owner},\"Controller\":{owner}}}"
        ));
    }
    format!(
        "{{\"Players\":[{{\"Id\":1,\"Name\":\"Qlocal\",\"LibraryCount\":{},\"HandCount\":7,\"Life\":{}}},{{\"Id\":2,\"Name\":\"Qopponent\",\"LibraryCount\":53,\"HandCount\":6,\"Life\":20}}],\"Cards\":[{}]}}",
        53 - step.min(40),
        20 - step.min(15),
        cards.join(",")
    )
}

/// What the synthetic 0.x database holds.
#[derive(Default)]
struct Built {
    games: i64,
    frames: i64,
    card_stats: i64,
    frame_bytes: i64,
}

/// The worst-case 0.x install under `dir`; returns its database path.
fn build_0x(dir: &TestDir, built: &mut Built) -> PathBuf {
    dir.write(
        "scale/storage/app/private/settings.json",
        br#"{"deck_card_size":"large"}"#,
    );
    dir.write("scale/database/.keep", b"");
    let path = dir.join("scale/database/nativephp.sqlite");
    let mut conn = store::Connection::open(&path).expect("open 0.x test database");
    conn.execute_batch(
        "PRAGMA foreign_keys = OFF; PRAGMA journal_mode = OFF; PRAGMA synchronous = OFF;",
    )
    .expect("pragmas");
    conn.execute_batch(SCHEMA).expect("0.x schema");
    let tx = conn.transaction().expect("tx");
    basics(&tx);
    decks_and_leagues(&tx);
    archetypes(&tx);
    matches(&tx, built);
    guides(&tx);
    tx.commit().expect("commit");
    path
}

fn basics(tx: &store::Transaction) {
    tx.execute_batch(
        "INSERT INTO accounts (id, username, created_at) VALUES (1, 'Qlocal', '2025-12-01 00:00:00');
         INSERT INTO players (id, username, is_player) VALUES (1, 'Qlocal', 1);",
    )
    .expect("account");
    let mut player = tx
        .prepare("INSERT INTO players (id, username) VALUES (?1, ?2)")
        .expect("prepare");
    for index in 0..OPPONENTS {
        player
            .execute(params![index + 2, format!("Qopp{index:04}")])
            .expect("opponent");
    }
    let mut card = tx
        .prepare(
            "INSERT INTO cards (id, mtgo_id, oracle_id, name, type, color_identity)
             VALUES (?1, ?2, ?3, ?4, 'Instant', 'R')",
        )
        .expect("prepare");
    for index in 0..KNOWN_CARDS {
        let catalog_id = FIRST_CATALOG_ID + index;
        card.execute(params![
            index + 1,
            catalog_id.to_string(),
            format!("q-oracle-{catalog_id}"),
            format!("Q Card {catalog_id}")
        ])
        .expect("card");
    }
}

fn decks_and_leagues(tx: &store::Transaction) {
    let mut deck = tx
        .prepare(
            "INSERT INTO decks (id, mtgo_id, name, format, created_at, updated_at, account_id,
                 archetype_id, color_identity)
             VALUES (?1, ?2, ?3, 'CMODERN', ?4, ?4, 1, ?5, 'R')",
        )
        .expect("prepare");
    let mut version = tx
        .prepare(
            "INSERT INTO deck_versions (id, deck_id, signature, modified_at)
             VALUES (?1, ?2, ?3, ?4)",
        )
        .expect("prepare");
    for index in 0..DECKS {
        let at = datetime(START_MS - 86_400_000 + index * 1_000);
        // Every deck tagged with one of the downloaded archetypes.
        deck.execute(params![
            index + 1,
            (50_000 + index).to_string(),
            format!("Qdeck {index}"),
            at,
            (index % ARCHETYPES) + 3
        ])
        .expect("deck");
        for number in 0..VERSIONS_PER_DECK {
            version
                .execute(params![
                    index * VERSIONS_PER_DECK + number + 1,
                    index + 1,
                    signature(&deck_list(index, number)),
                    datetime(START_MS - 86_400_000 + index * 1_000 + number)
                ])
                .expect("version");
        }
    }
    let mut league = tx
        .prepare(
            "INSERT INTO leagues (id, token, name, format, started_at, state, deck_version_id,
                 kind)
             VALUES (?1, ?2, 'Q League', 'CMODERN', ?3, 'complete', ?4, 'constructed')",
        )
        .expect("prepare");
    for index in 0..MATCHES / MATCHES_PER_LEAGUE {
        let first_match = index * MATCHES_PER_LEAGUE;
        league
            .execute(params![
                index + 1,
                format!("q-league-{index:05}"),
                datetime(START_MS + first_match * 3_600_000 - 60_000),
                version_of(first_match)
            ])
            .expect("league");
    }
}

/// The deck a match is played with (0-based) and its version id.
fn deck_of(match_index: i64) -> i64 {
    (match_index / MATCHES_PER_LEAGUE) % DECKS
}

fn version_of(match_index: i64) -> i64 {
    deck_of(match_index) * VERSIONS_PER_DECK + (match_index % VERSIONS_PER_DECK) + 1
}

/// Ids 1 and 2 are 0.x's fallbacks; 3 to 5,002 the downloaded archetypes;
/// the manual ones follow, each with two variants.
fn archetypes(tx: &store::Transaction) {
    tx.execute_batch(
        "INSERT INTO archetypes (id, uuid, name, format, is_fallback, created_at, updated_at)
         VALUES (1, '00000000-0000-0000-0000-000000000001', 'Homebrew', NULL, 1,
                 '2025-01-01 00:00:00', '2025-01-01 00:00:00'),
                (2, '00000000-0000-0000-0000-000000000002', 'Rogue', NULL, 1,
                 '2025-01-01 00:00:00', '2025-01-01 00:00:00');",
    )
    .expect("fallbacks");
    let mut archetype = tx
        .prepare(
            "INSERT INTO archetypes (id, uuid, name, format, color_identity, manual, created_at,
                 updated_at)
             VALUES (?1, ?2, ?3, 'modern', 'R,G', ?4, '2025-06-01 00:00:00',
                 '2025-06-01 00:00:00')",
        )
        .expect("prepare");
    for index in 0..ARCHETYPES {
        archetype
            .execute(params![
                index + 3,
                format!("q{index:07}-0000-4000-8000-000000000000"),
                format!("Q Archetype {index}"),
                false
            ])
            .expect("archetype");
    }
    let mut variant = tx
        .prepare("INSERT INTO archetype_decks (id, uuid, archetype_id) VALUES (?1, ?2, ?3)")
        .expect("prepare");
    let mut variant_card = tx
        .prepare(
            "INSERT INTO archetype_deck_cards (archetype_deck_id, card_id, quantity, sideboard)
             VALUES (?1, ?2, ?3, ?4)",
        )
        .expect("prepare");
    for index in 0..MANUAL_ARCHETYPES {
        let id = ARCHETYPES + 3 + index;
        archetype
            .execute(params![
                id,
                format!("qdev0001-manual-{index:04}"),
                format!("Q Manual {index}"),
                true
            ])
            .expect("manual archetype");
        for number in 0..2 {
            let variant_id = index * 2 + number + 1;
            variant
                .execute(params![variant_id, format!("q-variant-{variant_id}"), id])
                .expect("variant");
            for card in 0..20 {
                // 0.x card row ids 1..=KNOWN_CARDS.
                let card_id = (index * 7 + number + card) % KNOWN_CARDS + 1;
                variant_card
                    .execute(params![variant_id, card_id, 4, card >= 15])
                    .expect("variant card");
            }
        }
    }
}

fn matches(tx: &store::Transaction, built: &mut Built) {
    let mut match_row = tx
        .prepare(
            "INSERT INTO matches (id, token, mtgo_id, deck_version_id, format, match_type,
                 result, games_won, games_lost, started_at, ended_at, league_id, state, outcome,
                 submitted_at)
             VALUES (?1, ?2, ?3, ?4, 'CMODERN', 'Constructed', '2-1', 2, 1, ?5, ?6, ?7,
                 'complete', 'win', ?6)",
        )
        .expect("prepare");
    let mut game_row = tx
        .prepare(
            "INSERT INTO games (id, match_id, mtgo_id, won, started_at, ended_at, turn_count)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 9)",
        )
        .expect("prepare");
    let mut seat = tx
        .prepare(
            "INSERT INTO game_player (game_id, player_id, instance_id, is_local, on_play,
                 starting_hand_size, deck_json, dice_roll, mulligan_count)
             VALUES (?1, ?2, ?3, ?4, ?5, 7, ?6, ?7, 0)",
        )
        .expect("prepare");
    let mut timeline = tx
        .prepare("INSERT INTO game_timelines (game_id, timestamp, content) VALUES (?1, ?2, ?3)")
        .expect("prepare");
    let mut stat = tx
        .prepare(
            "INSERT INTO card_game_stats (oracle_id, game_id, deck_version_id, quantity, kept,
                 seen, won, is_postboard, opponent, cast, played, sided_in, sided_out)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 1, 1, 0, 0)",
        )
        .expect("prepare");
    let mut log = tx
        .prepare(
            "INSERT INTO game_logs (match_token, file_path, decoded_entries) VALUES (?1, ?2, ?3)",
        )
        .expect("prepare");
    let mut seat_archetype = tx
        .prepare(
            "INSERT INTO match_archetypes (archetype_id, mtgo_match_id, player_id, confidence,
                 manual)
             VALUES (?1, ?2, ?3, ?4, ?5)",
        )
        .expect("prepare");

    for index in 0..MATCHES {
        let match_id = index + 1;
        let token = format!("q-scale-{index:05}");
        let started = START_MS + index * 3_600_000;
        let version_id = version_of(index);
        let deck = deck_of(index);
        let list = deck_list(deck, index % VERSIONS_PER_DECK);
        let opponent = (index % OPPONENTS) + 2;
        match_row
            .execute(params![
                match_id,
                token,
                (300_000_000 + index).to_string(),
                version_id,
                datetime(started),
                datetime(started + 3_000_000),
                index / MATCHES_PER_LEAGUE + 1
            ])
            .expect("match");
        let mut entries = Vec::new();
        for number in 0..GAMES_PER_MATCH {
            let game_id = index * GAMES_PER_MATCH + number + 1;
            let game_start = started + number * 1_000_000;
            built.games += 1;
            game_row
                .execute(params![
                    game_id,
                    match_id,
                    (900_000_000 + game_id).to_string(),
                    number != 1,
                    datetime(game_start),
                    datetime(game_start + 900_000)
                ])
                .expect("game");
            let local_deck: Vec<String> = list
                .iter()
                .map(|(catalog_id, quantity, sideboard)| {
                    format!(
                        "{{\"mtgo_id\":{catalog_id},\"quantity\":{quantity},\"sideboard\":{sideboard}}}"
                    )
                })
                .collect();
            let opponent_base = index * 11 + number * 3;
            let opponent_deck: Vec<String> = (0..10)
                .map(|card| {
                    format!(
                        "{{\"mtgo_id\":{},\"quantity\":2,\"sideboard\":false}}",
                        catalog(opponent_base + card)
                    )
                })
                .collect();
            seat.execute(params![
                game_id,
                1,
                1,
                true,
                number != 1,
                format!("[{}]", local_deck.join(",")),
                4
            ])
            .expect("local seat");
            seat.execute(params![
                game_id,
                opponent,
                2,
                false,
                number == 1,
                format!("[{}]", opponent_deck.join(",")),
                2
            ])
            .expect("opponent seat");
            for step in 0..FRAMES_PER_GAME {
                // 60% of 0.x's frames repeat the one before.
                let distinct = step * 2 / 5;
                let visible = (distinct * FRAME_CARDS * 5 / (FRAMES_PER_GAME * 2)).min(FRAME_CARDS);
                let content = frame(&list, opponent_base, visible, distinct);
                built.frames += 1;
                built.frame_bytes += content.len() as i64;
                timeline
                    .execute(params![
                        game_id,
                        datetime_ms(game_start + step * 9_000),
                        content
                    ])
                    .expect("frame");
            }
            for card in 0..LOCAL_STATS_PER_GAME {
                let (catalog_id, quantity, _) = list[card as usize];
                stat.execute(params![
                    format!("q-oracle-{catalog_id}"),
                    game_id,
                    version_id,
                    quantity,
                    i64::from(card < 7),
                    2,
                    number != 1,
                    number > 0,
                    false
                ])
                .expect("local stat");
                built.card_stats += 1;
            }
            for card in 0..OPPONENT_STATS_PER_GAME {
                stat.execute(params![
                    format!("q-oracle-{}", catalog(opponent_base + card)),
                    game_id,
                    version_id,
                    0,
                    0,
                    2,
                    number != 1,
                    number > 0,
                    true
                ])
                .expect("opponent stat");
                built.card_stats += 1;
            }
            for line in 0..LOG_LINES_PER_GAME {
                entries.push(format!(
                    "{{\"timestamp\":\"{}\",\"message\":\"Qlocal casts Q Card {}.\"}}",
                    chrono::DateTime::from_timestamp_millis(game_start + line * 20_000)
                        .unwrap_or_default()
                        .to_rfc3339(),
                    list[(line % 15) as usize].0
                ));
            }
        }
        log.execute(params![
            token,
            format!("Match_GameLog_{index}.dat"),
            format!("[{}]", entries.join(","))
        ])
        .expect("game log");
        // The player seat: the deck's archetype; the opponent's: one of the
        // 5,000 (a new one for almost every match), every 40th set by hand to
        // a manual archetype.
        seat_archetype
            .execute(params![(deck % ARCHETYPES) + 3, match_id, 1, 0.95, false])
            .expect("player archetype");
        let (opponent_archetype, manual) = if index % 40 == 0 {
            (ARCHETYPES + 3 + (index / 40) % MANUAL_ARCHETYPES, true)
        } else {
            ((index % ARCHETYPES) + 3, false)
        };
        seat_archetype
            .execute(params![
                opponent_archetype,
                match_id,
                opponent,
                0.82,
                manual
            ])
            .expect("opponent archetype");
    }
}

fn guides(tx: &store::Transaction) {
    let mut guide = tx
        .prepare(
            "INSERT INTO sideboard_guides (id, deck_id, archetype_id, created_at, updated_at)
             VALUES (?1, ?2, ?3, '2026-02-01 10:00:00', '2026-02-01 10:30:00')",
        )
        .expect("prepare");
    let mut card = tx
        .prepare(
            "INSERT INTO sideboard_guide_cards (sideboard_guide_id, oracle_id, direction,
                 quantity)
             VALUES (?1, ?2, ?3, ?4)",
        )
        .expect("prepare");
    let mut note = tx
        .prepare(
            "INSERT INTO deck_archetype_notes (deck_id, archetype_id, body, created_at)
             VALUES (?1, ?2, ?3, ?4)",
        )
        .expect("prepare");
    for deck in 0..DECKS {
        for number in 0..GUIDES_PER_DECK {
            let id = deck * GUIDES_PER_DECK + number + 1;
            let archetype = (deck * 17 + number) % ARCHETYPES + 3;
            guide
                .execute(params![id, deck + 1, archetype])
                .expect("guide");
            for (slot, direction) in [(15, "in"), (16, "in"), (0, "out"), (1, "out")] {
                let (catalog_id, _, _) = deck_list(deck, 0)[slot];
                card.execute(params![id, format!("q-oracle-{catalog_id}"), direction, 2])
                    .expect("guide card");
            }
            for line in 0..NOTES_PER_GUIDE {
                note.execute(params![
                    deck + 1,
                    archetype,
                    format!("Qnote {line}: board in the sweepers"),
                    format!("2026-02-0{} 10:00:00", line + 1)
                ])
                .expect("note");
            }
        }
    }
}

/// The resident memory of this process in KiB (`ps`), `None` when it
/// cannot be read.
fn resident_kib() -> Option<u64> {
    let output = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .ok()?;
    String::from_utf8_lossy(&output.stdout).trim().parse().ok()
}

/// Samples the resident memory every 100 ms into the current step's peak.
struct Sampler {
    step: Arc<Mutex<String>>,
    peaks: Arc<Mutex<BTreeMap<String, u64>>>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Sampler {
    fn start() -> Sampler {
        let step = Arc::new(Mutex::new("start".to_string()));
        let peaks: Arc<Mutex<BTreeMap<String, u64>>> = Arc::new(Mutex::new(BTreeMap::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let step = Arc::clone(&step);
            let peaks = Arc::clone(&peaks);
            let stop = Arc::clone(&stop);
            std::thread::spawn(move || {
                while !stop.load(Ordering::SeqCst) {
                    if let Some(kib) = resident_kib() {
                        let name = lock(&step).clone();
                        let mut peaks = lock(&peaks);
                        let peak = peaks.entry(name).or_insert(0);
                        *peak = (*peak).max(kib);
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
            })
        };
        Sampler {
            step,
            peaks,
            stop,
            thread: Some(thread),
        }
    }

    fn enter(&self, name: &str) {
        *lock(&self.step) = name.to_string();
    }

    fn finish(mut self) -> BTreeMap<String, u64> {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        lock(&self.peaks).clone()
    }
}

fn step_of(progress: &ImportProgress) -> &'static str {
    match progress {
        ImportProgress::Reading => "1 read the 0.x database",
        ImportProgress::Writing { .. } => "2 match batches",
        ImportProgress::Details { .. } => "3 details (frames, card stats, logs)",
        ImportProgress::Decks => "4 decks, versions, cards",
        ImportProgress::Leagues => "5 leagues",
        ImportProgress::Limited => "6 Limited and the API state",
        ImportProgress::Archetypes => "7 archetypes, settings, card stub scan",
        ImportProgress::Finished { .. } | ImportProgress::Failed { .. } => "8 done",
    }
}

fn count(state: &crate::commands::AppState, sql: &str) -> i64 {
    lock(&state.reader)
        .query_row(sql, [], |row| row.get(0))
        .expect(sql)
}

fn file_mib(path: &Path) -> f64 {
    std::fs::metadata(path).map_or(0.0, |meta| meta.len() as f64 / 1_048_576.0)
}

#[test]
#[ignore = "release-only scale measure: cargo test --release -p app --lib import_scale -- --ignored --nocapture"]
fn a_worst_case_0x_import_runs_end_to_end() {
    let dir = TestDir::new("legacy-import-scale");
    let mut built = Built::default();
    let building = Instant::now();
    let path = build_0x(&dir, &mut built);
    println!(
        "0.x database built in {:.1?}: {MATCHES} matches, {} games, {} frames ({:.0} MiB of \
         frame JSON), {} card stats rows, {ARCHETYPES} + {MANUAL_ARCHETYPES} archetypes, \
         {DECKS} decks, file {:.0} MiB",
        building.elapsed(),
        built.games,
        built.frames,
        built.frame_bytes as f64 / 1_048_576.0,
        built.card_stats,
        file_mib(&path)
    );

    let (state, _events) = ready_state(&dir);
    state.import_running.store(true, Ordering::SeqCst);
    let sampler = Sampler::start();
    let mut steps: Vec<(&'static str, Instant)> = Vec::new();
    let mut last: Option<ImportProgress> = None;
    let started = Instant::now();
    run_import(
        &state,
        &path.to_string_lossy(),
        &crate::legacy::fake_launch::FakeLaunchAtLogin::default(),
        &mut |progress| {
            let step = step_of(&progress);
            if steps.last().is_none_or(|(name, _)| *name != step) {
                steps.push((step, Instant::now()));
                sampler.enter(step);
            }
            last = Some(progress);
        },
    );
    let total = started.elapsed();

    // The stub scan alone (the import ran it inside step 7): a second full
    // scan reads the same lists and timelines.
    sampler.enter("9 card stub scan again");
    let scanner = lock(&state.tracker)
        .as_ref()
        .expect("tracker")
        .card_stub_scanner();
    let scanning = Instant::now();
    let rescan = scanner.scan();
    let rescan_time = scanning.elapsed();
    // Step 7's 0.x read alone: the rest of the step is the store import, the
    // settings save and the stub scan.
    sampler.enter("10 archetype read again");
    let reading = Instant::now();
    let archetype_data =
        importer::archetype_read::read_archetypes_at(&path).expect("archetypes read");
    let read_time = reading.elapsed();
    let peaks = sampler.finish();

    println!("step | seconds | peak resident MiB");
    for (index, (name, at)) in steps.iter().enumerate() {
        let until = steps
            .get(index + 1)
            .map_or(started + total, |(_, next)| *next);
        let peak = peaks.get(*name).copied().unwrap_or(0) as f64 / 1024.0;
        println!(
            "{name} | {:.2} | {peak:.0}",
            until.duration_since(*at).as_secs_f64()
        );
    }
    println!(
        "9 card stub scan again | {:.2} | {:.0} ({:?})",
        rescan_time.as_secs_f64(),
        peaks.get("9 card stub scan again").copied().unwrap_or(0) as f64 / 1024.0,
        rescan
    );
    println!(
        "10 archetype read again | {:.2} | {:.0} ({} archetypes, {} match rows)",
        read_time.as_secs_f64(),
        peaks.get("10 archetype read again").copied().unwrap_or(0) as f64 / 1024.0,
        archetype_data.archetypes.len(),
        archetype_data.match_rows.len()
    );
    println!(
        "total {:.2} s for {MATCHES} matches ({:.2} s per 1,000; budget 5 s per 1,000); v1 \
         database {:.0} MiB",
        total.as_secs_f64(),
        total.as_secs_f64() * 1_000.0 / MATCHES as f64,
        file_mib(&state.db_path)
    );

    let Some(ImportProgress::Finished { summary }) = last else {
        panic!("the import did not finish: {last:?}");
    };
    assert_eq!(summary.added, MATCHES as u32);
    assert_eq!(count(&state, "SELECT COUNT(*) FROM matches"), MATCHES);
    assert_eq!(count(&state, "SELECT COUNT(*) FROM games"), built.games);
    assert_eq!(
        count(&state, "SELECT COUNT(*) FROM game_timelines"),
        built.games
    );
    // Every archetype the 0.x rows reference (ruling 4: not the rest of the
    // catalog), and every manual one.
    let referenced: i64 = store::Connection::open(&path)
        .expect("0.x")
        .query_row(
            "SELECT COUNT(*) FROM (SELECT archetype_id FROM match_archetypes
                 UNION SELECT archetype_id FROM decks WHERE archetype_id IS NOT NULL
                 UNION SELECT archetype_id FROM sideboard_guides
                 UNION SELECT archetype_id FROM deck_archetype_notes
                 UNION SELECT id FROM archetypes WHERE manual = 1)",
            [],
            |row| row.get(0),
        )
        .expect("referenced");
    assert!(
        count(
            &state,
            "SELECT COUNT(*) FROM archetypes WHERE source <> 'fallback'"
        ) >= referenced,
        "{referenced} referenced"
    );
    assert_eq!(
        count(&state, "SELECT COUNT(*) FROM match_archetypes"),
        MATCHES * 2
    );
    assert!(count(&state, "SELECT COUNT(*) FROM sideboard_guides") >= DECKS * GUIDES_PER_DECK);
    // #300 F20a: every matchup note (one per deck, archetype, time and
    // body), and each manual archetype's two variants as lists 1 and 2 of
    // 20 cards.
    assert_eq!(
        count(&state, "SELECT COUNT(*) FROM deck_archetype_notes"),
        DECKS * GUIDES_PER_DECK * NOTES_PER_GUIDE
    );
    let manual_lists = "FROM archetype_cards
         WHERE archetype_id IN (SELECT id FROM archetypes WHERE source = 'manual')";
    assert_eq!(
        count(
            &state,
            &format!("SELECT COUNT(DISTINCT archetype_id || ':' || list) {manual_lists}")
        ),
        MANUAL_ARCHETYPES * 2
    );
    assert_eq!(
        count(&state, &format!("SELECT MAX(list) {manual_lists}")),
        2
    );
    assert_eq!(
        count(&state, &format!("SELECT COUNT(*) {manual_lists}")),
        MANUAL_ARCHETYPES * 2 * 20
    );
    assert!(
        count(&state, "SELECT COUNT(*) FROM cards") >= CARD_POOL,
        "the stub scan gave every catalog id a row"
    );
    lock(&state.tracker).take().expect("tracker").stop();
}
