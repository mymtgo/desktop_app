//! The game view's speed budget (overlay spec section 14: "Game view
//! compute, no network: p95 < 10 ms, release test on a 3-game fixture match
//! with 2,000 frames"). Ignored by default because debug builds are much
//! slower. Run it in release:
//! `cargo test --release -p app --lib -- --ignored game_view_compute_under_10_ms --nocapture`.
//!
//! The data set (fake names start with Q): the live Modern match against
//! Qrival in game 3, its three games holding 2,000 state frames that grow
//! to 60 local and 15 opponent cards, and sideboarding transitions; a
//! 75-card deck; 300 earlier matches against Qrival with that deck (three
//! games of card rows each, sided cards included, and an opponent
//! archetype) and 3,300 against someone else; 500 held downloaded
//! archetypes with definitions and lists (the archetypes spec section 17
//! seed size); a guide with planned cards and notes for the archetype the
//! reveals name.

use std::time::{Duration, Instant};

use store::{Connection, params};

use super::tests::{GAME_ONE, LIVE, NOW, card, db, frame, set_version, settings};
use super::*;
use crate::overlay::game_net::testing::FakePort;

const FRAMES: usize = 2_000;
const LOCAL_CARDS: u32 = 60;
const OPPONENT_CARDS: u32 = 15;
const LOCAL_CATALOG: u32 = 2_000;
const OPPONENT_CATALOG: u32 = 3_000;
const HISTORY: i64 = 300;
const OTHERS: i64 = 3_300;
const ARCHETYPES: i64 = 500;
/// The archetype the opponent's reveals name (its definition needs the
/// first opponent card).
const NAMED: i64 = 1_000;

fn deck_cards() -> Vec<(u32, u32, bool)> {
    let mut cards = Vec::new();
    for index in 0..15 {
        cards.push((LOCAL_CATALOG + index, 4, false));
    }
    for index in 15..30 {
        cards.push((LOCAL_CATALOG + index, 1, true));
    }
    cards
}

fn seed_cards(conn: &Connection) {
    for index in 0..LOCAL_CARDS {
        let id = LOCAL_CATALOG + index;
        let kind = if index % 3 == 0 { "Land" } else { "Creature" };
        card(conn, id, &format!("o-{id}"), &format!("Qlocal {id}"), kind);
    }
    for index in 0..OPPONENT_CARDS {
        let id = OPPONENT_CATALOG + index;
        card(
            conn,
            id,
            &format!("o-{id}"),
            &format!("Qrival {id}"),
            "Creature",
        );
    }
}

/// The live match in game 3 and its 2,000 frames, in log order.
fn live_games(conn: &Connection) {
    super::tests::a_match(
        conn,
        1,
        LIVE,
        Some(1),
        "CModern",
        "in_progress",
        "unknown",
        NOW - 60_000,
        Some(1),
    );
    for (number, result) in [(1, "win"), (2, "loss"), (3, "unknown")] {
        conn.execute(
            "INSERT INTO games (match_id, game_number, mtgo_game_id, result, end_reason)
             VALUES (1, ?1, ?2, ?3, 'unknown')",
            params![number, GAME_ONE + number - 1, result],
        )
        .expect("game");
    }
    let per_game = FRAMES / 3 + 1;
    let mut offset = 0;
    let tx = conn.unchecked_transaction().expect("tx");
    for game in 0..3_i64 {
        let frames = if game == 2 {
            FRAMES - 2 * per_game
        } else {
            per_game
        };
        for index in 0..frames {
            let shown = 7 + (index * (LOCAL_CARDS as usize - 7)) / frames;
            let local: Vec<(u32, &str)> = (0..shown as u32)
                .map(|n| {
                    let zone = if n < 7 {
                        "Hand"
                    } else if n % 4 == 0 {
                        "Graveyard"
                    } else {
                        "Battlefield"
                    };
                    (LOCAL_CATALOG + n % 30, zone)
                })
                .collect();
            let revealed = (index * OPPONENT_CARDS as usize / frames) as u32 + 1;
            let opponent: Vec<u32> = (0..revealed).map(|n| OPPONENT_CATALOG + n).collect();
            let library = 53 - (index * 20 / frames) as i64;
            frame(&tx, offset, GAME_ONE + game, library, &local, &opponent);
            offset += 1_000;
        }
        // The game's end moves the match to sideboarding (games 1 and 2).
        if game < 2 {
            tx.execute(
                "INSERT INTO log_events (log_file_id, byte_offset, kind, match_token, logged_at, raw)
                 VALUES (1, ?1, 'match_state_changed', ?2, ?3, CAST(?4 AS BLOB))",
                params![
                    offset,
                    LIVE,
                    NOW,
                    "12:00:00 [INF] (Match|State) Match State Changed for q-live from MatchJoinedEventUnderwayState to MatchJoinedSideboardingState"
                ],
            )
            .expect("sideboarding");
            offset += 1_000;
        }
    }
    tx.commit().expect("commit");
}

/// 300 complete matches against Qrival with deck 1 (three games of card
/// rows, the opponent seat named) and 3,300 against Qother.
fn history(conn: &Connection) {
    let tx = conn.unchecked_transaction().expect("tx");
    for n in 0..HISTORY + OTHERS {
        let id = 10 + n;
        let against_rival = n < HISTORY;
        let outcome = if n % 3 == 0 { "loss" } else { "win" };
        tx.execute(
            "INSERT INTO matches (id, token, account_id, opponent_id, source, format, state,
                 outcome, started_at, last_activity_at, revision, derived_version, deck_version_id)
             VALUES (?1, 'q-old-' || ?1, 1, ?2, 'log', 'CModern', 'complete', ?3, ?4, ?4, 1, 1, ?5)",
            params![
                id,
                if against_rival { 1 } else { 2 },
                outcome,
                NOW - (n + 1) * 3_600_000,
                if against_rival { Some(10) } else { None }
            ],
        )
        .expect("match");
        if !against_rival {
            continue;
        }
        tx.execute(
            "INSERT INTO match_archetypes (match_id, is_opponent, archetype_id, confidence, manual)
             VALUES (?1, 1, ?2, 0.9, 0)",
            params![id, NAMED + n % 5],
        )
        .expect("seat");
        for number in 1..=3_i64 {
            let game_id: i64 = tx
                .query_row(
                    "INSERT INTO games (match_id, game_number, mtgo_game_id, result, end_reason)
                     VALUES (?1, ?2, ?3, ?4, 'win') RETURNING id",
                    params![
                        id,
                        number,
                        100_000 + id * 10 + number,
                        if (n + number) % 2 == 0 { "win" } else { "loss" }
                    ],
                    |row| row.get(0),
                )
                .expect("game");
            for (catalog_id, quantity, sideboard) in deck_cards() {
                let (main, side) = if sideboard {
                    (0, quantity)
                } else {
                    (quantity, 0)
                };
                let sided_in = i64::from(number > 1 && sideboard && catalog_id % 3 == 0);
                let sided_out = i64::from(number > 1 && !sideboard && catalog_id % 5 == 0);
                tx.execute(
                    "INSERT INTO game_cards (game_id, side, catalog_id, main_qty, side_qty,
                         sided_in, sided_out, seen)
                     VALUES (?1, 'local', ?2, ?3, ?4, ?5, ?6, 1)",
                    params![game_id, catalog_id, main, side, sided_in, sided_out],
                )
                .expect("card row");
            }
        }
    }
    tx.commit().expect("commit");
}

/// 500 held downloaded Modern archetypes (the archetypes spec section 17
/// seed size) with a definition and a 20-row list 1; `NAMED` needs the
/// first opponent card in the mainboard.
fn archetypes(conn: &Connection) {
    let tx = conn.unchecked_transaction().expect("tx");
    for n in 0..ARCHETYPES {
        let id = NAMED + n;
        let key_card = OPPONENT_CATALOG + (n as u32 % OPPONENT_CARDS);
        let definition = serde_json::json!({
            "uuid": format!("q-arch-{id}"),
            "name": format!("Qarch {id}"),
            "format": "modern",
            "conditions": [{
                "type": "InMainboard",
                "cards": [format!("Qrival {}", if n == 0 { OPPONENT_CATALOG } else { 90_000 + n as u32 })]
            }],
            "cards": [{
                "mtgo_id": key_card,
                "oracle_id": format!("o-{key_card}"),
                "name": format!("Qrival {key_card}"),
                "quantity": 4,
                "sideboard": false
            }]
        })
        .to_string();
        tx.execute(
            "INSERT INTO archetypes (id, uuid, name, format, color_identity, source, definition,
                 created_at, updated_at)
             VALUES (?1, 'q-arch-' || ?1, 'Qarch ' || ?1, 'modern', 'U,B', 'downloaded', ?2, 1, 1)",
            params![id, definition],
        )
        .expect("archetype");
        for index in 0..20_u32 {
            let catalog_id = OPPONENT_CATALOG + (index + n as u32) % OPPONENT_CARDS + index * 100;
            tx.execute(
                "INSERT OR IGNORE INTO archetype_cards (archetype_id, list, catalog_id, oracle_id,
                     quantity, sideboard)
                 VALUES (?1, 1, ?2, 'o-' || ?2, ?3, ?4)",
                params![id, catalog_id, 1 + index % 4, index >= 15],
            )
            .expect("list card");
        }
    }
    tx.execute(
        "INSERT INTO sideboard_guides (id, deck_id, archetype_id, created_at, updated_at)
         VALUES (1, 1, ?1, 1, 1)",
        [NAMED],
    )
    .expect("guide");
    for (catalog_id, quantity, sideboard) in deck_cards().into_iter().step_by(3) {
        tx.execute(
            "INSERT INTO sideboard_guide_cards (guide_id, oracle_id, direction, quantity)
             VALUES (1, 'o-' || ?1, ?2, ?3)",
            params![
                catalog_id,
                if sideboard { "in" } else { "out" },
                quantity.min(2)
            ],
        )
        .expect("plan card");
    }
    for n in 0..5 {
        super::tests::note(&tx, 1, NAMED, &format!("Qnote {n}"), n);
    }
    tx.commit().expect("commit");
}

fn ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

/// p50 and p95 of the samples, in milliseconds.
fn percentiles(samples: &[Duration]) -> (f64, f64) {
    let mut sorted: Vec<f64> = samples.iter().map(|sample| ms(*sample)).collect();
    sorted.sort_by(f64::total_cmp);
    let at = |share: usize| sorted[(sorted.len() * share / 100).min(sorted.len() - 1)];
    (at(50), at(95))
}

#[test]
#[ignore = "timing check, run in release"]
fn game_view_compute_under_10_ms() {
    let (dir, conn) = db("game-overlay-speed");
    seed_cards(&conn);
    set_version(&conn, 1, &deck_cards());
    live_games(&conn);
    archetypes(&conn);
    history(&conn);
    let frames: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM log_events WHERE kind = 'game_state_update'",
            [],
            |row| row.get(0),
        )
        .expect("frames");
    assert_eq!(frames, FRAMES as i64);
    let reader = store::open_reader(&dir.join("mymtgo.sqlite")).expect("reader");
    let settings = settings();
    let inputs = GameInputs {
        settings: &settings,
        collapsed: false,
        now_ms: NOW,
        previous: None,
    };

    // The view is complete: the reveals name the archetype, its guide and
    // potential cards show, and the record against Qrival counts.
    let port = FakePort::online();
    let mut net = port.net();
    let (view, memo) = game_view(&reader, Some(LIVE), &inputs, &mut net).expect("view");
    let memo = memo.expect("a live match");
    let opponent = view.opponent.as_ref().expect("opponent");
    assert_eq!(opponent.archetype_id, Some(NAMED), "{opponent:?}");
    assert_eq!(opponent.previous_matches, HISTORY as u32);
    assert!(view.draw_odds.is_some());
    assert!(
        view.reveals
            .as_ref()
            .is_some_and(|reveals| reveals.len() == 15)
    );
    assert!(view.potential.is_some());
    assert!(view.sideboard.is_some());
    assert_eq!(view.notes.current.len(), 5);
    assert!(!view.is_sideboarding, "game 3 is under way");

    // A new reveal set: every match part is computed again, the live read
    // included (each sample past the live read's 30 minutes); the worker's
    // lookups keep the format's candidates (their stamp is unchanged).
    let mut full = Vec::new();
    for n in 0..50 {
        let later = GameInputs {
            now_ms: NOW + (n + 1) * (crate::overlay::game_net::LIVE_TTL_MS + 1),
            ..inputs
        };
        let started = Instant::now();
        let (view, _) = game_view(&reader, Some(LIVE), &later, &mut net).expect("view");
        full.push(started.elapsed());
        assert!(view.opponent.is_some());
    }
    // The first compute after start or after `ArchetypesChanged`: the
    // candidates are read too (once per format and change, reported only).
    let mut cold = Vec::new();
    for _ in 0..20 {
        let port = FakePort::online();
        let mut fresh = port.net();
        let started = Instant::now();
        game_view(&reader, Some(LIVE), &inputs, &mut fresh).expect("view");
        cold.push(started.elapsed());
    }
    // A frame-only wake: the match parts come from the memo.
    let frame_inputs = GameInputs {
        previous: Some(&memo),
        ..inputs
    };
    let mut frame_only = Vec::new();
    for _ in 0..200 {
        let started = Instant::now();
        let (view, _) = game_view(&reader, Some(LIVE), &frame_inputs, &mut net).expect("view");
        frame_only.push(started.elapsed());
        assert!(view.draw_odds.is_some());
    }

    let (full_p50, full_p95) = percentiles(&full);
    let (cold_p50, cold_p95) = percentiles(&cold);
    let (frame_p50, frame_p95) = percentiles(&frame_only);
    println!();
    println!("| Measure | Budget | p50 (ms) | p95 (ms) | Within |");
    println!("|---|---|---|---|---|");
    for (name, p50, p95) in [
        (
            "Game view, full compute (new reveal set)",
            full_p50,
            full_p95,
        ),
        ("Game view, frame-only recompute", frame_p50, frame_p95),
    ] {
        println!(
            "| {name} | p95 < 10 ms | {p50:.2} | {p95:.2} | {} |",
            if p95 < 10.0 { "yes" } else { "no" }
        );
    }
    println!(
        "| Game view, first compute (candidates read, once per format and change) | reported | {cold_p50:.2} | {cold_p95:.2} | - |"
    );
    assert!(full_p95 < 10.0, "full compute p95 {full_p95:.2} ms");
    assert!(frame_p95 < 10.0, "frame-only p95 {frame_p95:.2} ms");
}
