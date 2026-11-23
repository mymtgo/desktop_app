//! Detection's speed budgets (archetypes spec section 17, Plan T Review
//! Focus 5): over 3,600 complete matches and 550 held archetypes (500
//! downloaded with a definition and a 60-card list 1, 50 manual with two
//! variants each), the detection inputs read stays under 10 ms, the pure
//! decision under 20 ms (p95), and a slice of 10 detection writes under
//! 100 ms, so detection never holds the writer. The change check's apply
//! of 500 definitions runs in slices under 100 ms each.
//!
//! Ignored by default because debug builds are much slower. Run it in
//! release:
//! `cargo test --release -p store --test archetype_detection_speed -- --ignored --nocapture`.

use std::time::{Duration, Instant};

use fixture_tools::TestDir;
use store::archetype_edit::{ArchetypeEdit, MatchDetection, SeatResult, edit_archetypes};
use store::archetypes::{detection_input, held_candidates};

const MATCHES: i64 = 3_600;
const DOWNLOADED: i64 = 500;
const MANUAL: i64 = 50;
const CARDS: i64 = 2_000;

fn build(conn: &mut store::Connection) {
    let tx = conn.transaction().expect("tx");
    tx.execute_batch(
        "INSERT INTO accounts (id, username, first_seen_at) VALUES (1, 'Qlocal', 0);
         INSERT INTO opponents (id, username, first_seen_at) VALUES (1, 'Qopponent', 0);",
    )
    .expect("registries");
    for card in 1..=CARDS {
        let kind = if card % 10 == 0 { "Land" } else { "Creature" };
        tx.execute(
            "INSERT INTO cards (catalog_id, oracle_id, name, type, color_identity)
             VALUES (?1, 'q-oracle-' || ?1, 'Qcard ' || ?1, ?2, 'R')",
            (card, kind),
        )
        .expect("card");
    }
    for archetype in 1..=(DOWNLOADED + MANUAL) {
        let downloaded = archetype <= DOWNLOADED;
        let definition = if downloaded {
            let wanted = archetype * 3 % CARDS + 1;
            Some(format!(
                r#"{{"uuid":"q-{archetype}","name":"Q{archetype}","format":"modern","rank":[1,0,{archetype},0,0,0,0],
                   "conditions":[{{"type":"InMainboard","cards":["Qcard {wanted}"]}},
                                 {{"type":"OneOrMoreInMainboard","cards":["Qcard {}","Qcard {}"]}}],
                   "colour_fork":"{archetype}:Base"}}"#,
                wanted + 1,
                wanted + 2
            ))
        } else {
            None
        };
        tx.execute(
            "INSERT INTO archetypes (id, uuid, name, format, source, definition, created_at, updated_at)
             VALUES (?1, 'q-' || ?1, 'Q' || ?1, 'modern', ?2, ?3, 1, 1)",
            (
                archetype + 100,
                if downloaded { "downloaded" } else { "manual" },
                definition,
            ),
        )
        .expect("archetype");
        let lists = if downloaded { 1 } else { 2 };
        for list in 1..=lists {
            for slot in 0..15 {
                let card = (archetype * 7 + list * 13 + slot) % CARDS + 1;
                tx.execute(
                    "INSERT OR IGNORE INTO archetype_cards
                         (archetype_id, list, catalog_id, quantity, sideboard)
                     VALUES (?1, ?2, ?3, 4, 0)",
                    (archetype + 100, list, card),
                )
                .expect("list card");
            }
        }
    }
    for id in 1..=MATCHES {
        tx.execute(
            "INSERT INTO matches (id, token, account_id, opponent_id, source, format, match_type,
                 state, outcome, started_at, last_activity_at, revision, derived_version,
                 games_synced_at)
             VALUES (?1, 'Q-match-' || ?1, 1, 1, 'log', 'CMODERN', 'League', 'complete', 'win',
                 ?1, ?1, 1, 0, 1)",
            [id],
        )
        .expect("match");
        for number in 1..=3 {
            let game = id * 10 + number;
            tx.execute(
                "INSERT INTO games (id, match_id, game_number, result, end_reason)
                 VALUES (?1, ?2, ?3, 'win', 'win')",
                (game, id, number),
            )
            .expect("game");
            for slot in 0..15 {
                let card = (id * 11 + slot) % CARDS + 1;
                tx.execute(
                    "INSERT INTO game_cards (game_id, side, catalog_id, main_qty, side_qty)
                     VALUES (?1, 'local', ?2, 4, 0)",
                    (game, card),
                )
                .expect("local card");
            }
            for slot in 0..8 {
                let card = (id * 5 + slot * 3) % CARDS + 1;
                tx.execute(
                    "INSERT INTO game_cards (game_id, side, catalog_id, seen)
                     VALUES (?1, 'opponent', ?2, 2)",
                    (game, card),
                )
                .expect("opponent card");
            }
        }
    }
    tx.commit().expect("commit");
}

fn p95(mut samples: Vec<Duration>) -> Duration {
    samples.sort();
    samples[samples.len() * 95 / 100]
}

#[test]
#[ignore = "timing check, run in release"]
fn a_detection_slice_stays_under_100_ms() {
    let dir = TestDir::new("archetype-detection-speed");
    let path = dir.join("mymtgo.sqlite");
    let mut writer = store::open_writer(&path).expect("writer");
    build(&mut writer);
    let reader = store::open_reader(&path).expect("reader");

    let candidates = held_candidates(&reader, "modern").expect("candidates");
    assert_eq!(candidates.len() as i64, DOWNLOADED + MANUAL);

    let mut reads = Vec::new();
    let mut candidate_reads = Vec::new();
    let mut decisions = Vec::new();
    let mut results = Vec::new();
    for id in (1..=MATCHES).step_by(36) {
        let started = Instant::now();
        let candidates = held_candidates(&reader, "modern").expect("candidates");
        candidate_reads.push(started.elapsed());
        let started = Instant::now();
        let input = detection_input(&reader, id).expect("read").expect("ready");
        reads.push(started.elapsed());

        let started = Instant::now();
        let player = archetypes::decide(
            archetypes::local_answers(&input.player_cards, &candidates, false),
            false,
        );
        let opponent = archetypes::decide(
            archetypes::local_answers(&input.opponent_cards, &candidates, true),
            false,
        );
        decisions.push(started.elapsed());
        let seat = |decision: archetypes::Decision| match decision {
            archetypes::Decision::Accept {
                archetype_id,
                confidence,
            }
            | archetypes::Decision::AcceptOffline {
                archetype_id,
                confidence,
            } => Some(SeatResult {
                archetype_id,
                confidence,
            }),
            _ => None,
        };
        results.push(MatchDetection {
            match_id: id,
            deck_archetype_id: None,
            deck_archetype: None,
            player: seat(player),
            opponent: seat(opponent).or(Some(SeatResult {
                archetype_id: 1,
                confidence: 0.0,
            })),
        });
    }

    let mut writes = Vec::new();
    for slice in results.chunks(10) {
        let started = Instant::now();
        edit_archetypes(
            &mut writer,
            &ArchetypeEdit::SetDetections {
                results: slice.to_vec(),
                now_ms: 1,
            },
        )
        .expect("write");
        writes.push(started.elapsed());
    }
    let read = p95(reads);
    let candidates_read = p95(candidate_reads);
    let decision = p95(decisions);
    let write = writes.iter().max().copied().unwrap_or_default();
    println!(
        "detection over {MATCHES} matches and {} held archetypes: inputs read p95 {read:?}, held candidates read p95 {candidates_read:?}, decision p95 {decision:?}, slice of 10 writes max {write:?}",
        DOWNLOADED + MANUAL
    );
    assert!(write < Duration::from_millis(100), "{write:?}");
    assert!(decision < Duration::from_millis(20), "{decision:?}");
    assert!(read < Duration::from_millis(10), "{read:?}");
}

/// Spec section 17: the change check's apply of 500 changed definitions
/// (60-card key lists) runs in slices of 50 plus a last slice with the
/// re-detection of the format, each under 100 ms.
#[test]
#[ignore = "timing check, run in release"]
fn a_change_check_apply_slice_stays_under_100_ms() {
    let dir = TestDir::new("archetype-change-check-speed");
    let path = dir.join("mymtgo.sqlite");
    let mut writer = store::open_writer(&path).expect("writer");
    build(&mut writer);
    writer
        .execute("UPDATE matches SET archetypes_detected_at = 1", [])
        .expect("detected");
    let changed: Vec<archetypes::Definition> = (1..=DOWNLOADED)
        .map(|archetype| archetypes::Definition {
            uuid: format!("q-{archetype}"),
            name: format!("Q{archetype} changed"),
            format: Some("modern".to_string()),
            version: Some("q-v2".to_string()),
            cards: (0..15)
                .map(|offset| archetypes::DefinitionCard {
                    mtgo_id: (archetype * 7 + offset) % CARDS + 1,
                    arena_id: None,
                    oracle_id: None,
                    name: None,
                    quantity: 4,
                    sideboard: false,
                })
                .collect(),
            ..archetypes::Definition::default()
        })
        .collect();
    let held = store::archetypes::held_definitions(&writer).expect("held");
    let check = archetypes::DefinitionsCheck {
        changed,
        ..archetypes::DefinitionsCheck::default()
    };
    let slices = archetypes::plan_slices(&held, &check, 50);
    assert_eq!(slices.len(), 11);
    // Every replace slice re-detects its format; the last is moves only.
    let mut times = Vec::new();
    for plan in slices {
        let started = Instant::now();
        edit_archetypes(
            &mut writer,
            &ArchetypeEdit::ApplyDefinitions {
                plan,
                checked_at: Some(2),
                now_ms: 2,
            },
        )
        .expect("apply");
        times.push(started.elapsed());
    }
    let replace = times[..times.len() - 1]
        .iter()
        .max()
        .copied()
        .unwrap_or_default();
    let last = times[times.len() - 1];
    println!(
        "change check apply: slice of 50 definitions with their re-detection max {replace:?}, moves slice {last:?}"
    );
    assert!(replace < Duration::from_millis(100), "{replace:?}");
    assert!(last < Duration::from_millis(100), "{last:?}");
}
