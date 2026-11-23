//! The deck Card Stats tab's speed budget (cards spec section 12): data
//! under 100 ms for a deck with 1,000 complete matches (about 2,500 games
//! and 100,000 `game_cards` rows). Measured and reported, not a CI gate.
//!
//! Ignored by default because debug builds are much slower. Run it in
//! release:
//! `cargo test --release -p store --test card_stats_speed -- --ignored --nocapture`.
//!
//! The data is synthetic (row inserts only): one deck with two versions,
//! 1,000 complete log matches alternating two and three games, each game 34
//! local rows and 6 opponent rows over 80 cards (a few without card data),
//! and a cast by an alternative method in every game.

use std::time::{Duration, Instant};

use cards::stats::Board;
use decks::{DeckCard, signature};
use fixture_tools::TestDir;
use game_detail::Side;
use store::card_stats::{CardStatsFilter, card_stats, deck_prior, opponent_archetypes};
use store::{open_reader, open_writer, params};

const MATCHES: i64 = 1_000;
const LOCAL_ROWS: u32 = 34;
const OPPONENT_ROWS: u32 = 6;
const BUDGET: Duration = Duration::from_millis(100);

fn build(conn: &mut store::Connection) -> i64 {
    let tx = conn.transaction().expect("tx");
    tx.execute_batch(
        "INSERT INTO accounts (id, username, first_seen_at) VALUES (1, 'Qlocal', 0);
         INSERT INTO decks (id, mtgo_id, account_id, name, format, created_at, updated_at, source)
             VALUES (1, 'q-deck', 1, 'Qdeck', 'CMODERN', 0, 0, 'file');",
    )
    .expect("deck");
    let list: Vec<DeckCard> = (0..LOCAL_ROWS)
        .map(|i| DeckCard {
            catalog_id: 1_000 + i,
            quantity: 2,
            sideboard: i >= 30,
        })
        .collect();
    for (id, modified_at) in [(1, 0), (2, 1)] {
        tx.execute(
            "INSERT INTO deck_versions (id, deck_id, signature, modified_at) VALUES (?1, 1, ?2, ?3)",
            params![id, format!("{}{id}", signature(&list)), modified_at],
        )
        .expect("version");
    }
    // Local cards 1000..1033, opponent cards 2000..2045; every tenth card
    // has no card data, and two printings share each oracle id.
    let mut card = tx
        .prepare("INSERT INTO cards (catalog_id, oracle_id, name, type) VALUES (?1, ?2, ?3, ?4)")
        .expect("card");
    for id in (1_000..1_000 + LOCAL_ROWS).chain(2_000..2_046) {
        if id % 10 == 3 {
            continue;
        }
        let oracle = format!("q-oracle-{}", id / 2);
        let kind = if id % 3 == 0 { "Creature" } else { "Instant" };
        card.execute(params![id, oracle, format!("Qcard {id}"), kind])
            .expect("card");
    }
    drop(card);

    let mut matches = tx
        .prepare(
            "INSERT INTO matches (id, token, account_id, source, state, outcome, started_at,
                 last_activity_at, revision, derived_version, deck_version_id)
             VALUES (?1, 'q-' || ?1, 1, 'log', 'complete', 'win', ?2, ?2, 1, 1, ?3)",
        )
        .expect("matches");
    let mut games = tx
        .prepare(
            "INSERT INTO games (id, match_id, game_number, result, end_reason, on_play)
             VALUES (?1, ?2, ?3, ?4, 'win', ?5)",
        )
        .expect("games");
    let mut rows = tx
        .prepare(
            "INSERT INTO game_cards (game_id, side, catalog_id, main_qty, opening_hand,
                 starting_hand, seen, cast, played, sided_in, pregame_revealed, inferred)
             VALUES (?1, ?2, ?3, ?4, ?5, ?5, ?6, ?7, ?8, ?9, ?10, 0)",
        )
        .expect("rows");
    let mut casts = tx
        .prepare(
            "INSERT INTO game_card_casts (game_id, side, catalog_id, method, count)
             VALUES (?1, 'local', ?2, 'kicked', 1)",
        )
        .expect("casts");
    let mut game_id = 0;
    for m in 1..=MATCHES {
        matches
            .execute(params![m, m * 60_000, 1 + m % 2])
            .expect("match");
        let count = if m % 2 == 0 { 3 } else { 2 };
        for number in 1..=count {
            game_id += 1;
            let result = if (m + number) % 3 == 0 { "loss" } else { "win" };
            games
                .execute(params![game_id, m, number, result, (m + number) % 2])
                .expect("game");
            for i in 0..LOCAL_ROWS {
                let seed = (game_id + i64::from(i)) % 4;
                rows.execute(params![
                    game_id,
                    "local",
                    1_000 + i,
                    if i >= 30 { 0 } else { 2 },
                    seed % 2,
                    seed % 3,
                    seed / 2,
                    seed % 2,
                    i64::from(number > 1 && i >= 30 && seed == 0),
                    i64::from(seed == 3),
                ])
                .expect("local row");
            }
            for i in 0..OPPONENT_ROWS {
                let id = 2_000 + ((game_id as u32 + i * 7) % 46);
                rows.execute(params![game_id, "opponent", id, 0, 0, 1, 1, 0, 0, 0])
                    .expect("opponent row");
            }
            casts
                .execute(params![game_id, 1_000 + (game_id % 30) as u32])
                .expect("cast");
        }
    }
    drop((matches, games, rows, casts));
    tx.commit().expect("commit");
    1
}

/// One uncached read of the tab's data under `filter`.
fn tab_data(conn: &store::Connection, filter: &CardStatsFilter) -> (usize, Duration) {
    let started = Instant::now();
    let stats = card_stats(conn, 1, filter).expect("stats");
    deck_prior(conn, 1, filter).expect("prior");
    opponent_archetypes(conn, 1, filter.version_id, None).expect("archetypes");
    (stats.len(), started.elapsed())
}

#[test]
#[ignore = "timing check, run in release"]
fn card_stats_for_a_thousand_matches_meets_the_budget() {
    let dir = TestDir::new("card-stats-speed");
    let path = dir.join("mymtgo.sqlite");
    let mut writer = open_writer(&path).expect("writer");
    build(&mut writer);
    let rows: i64 = writer
        .query_row("SELECT COUNT(*) FROM game_cards", [], |row| row.get(0))
        .expect("rows");
    let game_count: i64 = writer
        .query_row("SELECT COUNT(*) FROM games", [], |row| row.get(0))
        .expect("games");
    drop(writer);

    let filters = [
        ("mine, every game", CardStatsFilter::all()),
        (
            "theirs, every game",
            CardStatsFilter {
                side: Side::Opponent,
                ..CardStatsFilter::all()
            },
        ),
        (
            "mine, version 1, postboard, on the play",
            CardStatsFilter {
                version_id: Some(1),
                board: Some(Board::Postboard),
                on_play: Some(true),
                ..CardStatsFilter::all()
            },
        ),
    ];
    println!("card stats speed: {MATCHES} matches, {game_count} games, {rows} game_cards rows");
    let mut worst = Duration::ZERO;
    for (label, filter) in filters {
        // A fresh connection: nothing prepared or cached by an earlier read.
        let reader = open_reader(&path).expect("reader");
        let (cards, first) = tab_data(&reader, &filter);
        let (_, again) = tab_data(&reader, &filter);
        println!("  {label}: {cards} cards, first {first:?}, again {again:?}");
        worst = worst.max(first);
    }
    assert!(rows >= 95_000, "about 100,000 rows: {rows}");
    assert!(
        worst < BUDGET,
        "slowest first read {worst:?} over {BUDGET:?}"
    );
}
