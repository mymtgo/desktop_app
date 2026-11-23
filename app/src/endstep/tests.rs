//! The deck pages' numbers over the synced fixture Endstep match: Game Stats
//! and the Sideboard Guides read the game rows the Endstep sync writes.

use decks::stats::Range;
use fixture_tools::TestDir;
use store::game_stats::{GameGroup, PlaySplit, game_stats};

use crate::endstep_seed::{ENDSTEP_FIXTURE_TOKEN, sync_endstep_fixture};

/// A database with the fixture match synced (the user lost 1-2).
fn synced(label: &str) -> (TestDir, std::path::PathBuf) {
    let dir = TestDir::new(label);
    let path = dir.join("mymtgo.sqlite");
    sync_endstep_fixture(&path, &dir, 1_794_400_000_000);
    (dir, path)
}

fn deck_id(conn: &store::Connection) -> i64 {
    conn.query_row(
        "SELECT v.deck_id FROM matches m JOIN deck_versions v ON v.id = m.deck_version_id
         WHERE m.token = ?1",
        [ENDSTEP_FIXTURE_TOKEN],
        |row| row.get(0),
    )
    .expect("the match's deck")
}

#[test]
fn game_stats_count_an_endstep_deck() {
    let (_dir, path) = synced("endstep-game-stats");
    let conn = store::open_reader(&path).expect("reader");
    let rows = game_stats(&conn, deck_id(&conn), Range::all_time(), None).expect("stats");
    let overall = |group: GameGroup| {
        *rows
            .iter()
            .find(|row| row.group == group && row.split == PlaySplit::Overall)
            .expect("row")
    };
    let all = overall(GameGroup::AllGames);
    assert_eq!((all.wins, all.losses), (1, 2), "three games, one win");
    // One local mulligan, in game 3: 1 over 3 games.
    assert_eq!(all.mulligans, Some(0.33));
    assert_eq!(overall(GameGroup::Game1).wins, 1);
    assert_eq!(overall(GameGroup::Game3).mulligans, Some(1.0));
    assert_eq!(overall(GameGroup::Game2).mulligans, Some(0.0));
}

#[test]
fn sided_stats_count_endstep_sideboarding() {
    let (_dir, path) = synced("endstep-sided-stats");
    let writer = store::open_writer(&path).expect("writer");
    // Card data is not downloaded in a test: give every named card its own
    // oracle id, and the opponent an archetype.
    writer
        .execute_batch(
            "UPDATE cards SET oracle_id = 'oracle:' || name WHERE name IS NOT NULL AND name <> '';
             INSERT INTO archetypes (uuid, name, format, source, created_at, updated_at)
                 VALUES ('endstep-test-arch', 'Qopponent', 'CMODERN', 'manual', 0, 0);",
        )
        .expect("seed oracle ids and archetype");
    let attached = writer
        .execute(
            "INSERT INTO match_archetypes (match_id, is_opponent, archetype_id, confidence)
                 SELECT m.id, 1, a.id, 1.0 FROM matches m, archetypes a
                 WHERE a.uuid = 'endstep-test-arch' AND m.token = ?1",
            [ENDSTEP_FIXTURE_TOKEN],
        )
        .expect("the fixture match's opponent archetype");
    assert_eq!(attached, 1, "only the fixture match gets the archetype");
    let conn = store::open_reader(&path).expect("reader");
    let archetype: i64 = conn
        .query_row(
            "SELECT id FROM archetypes WHERE uuid = 'endstep-test-arch'",
            [],
            |row| row.get(0),
        )
        .expect("archetype");
    let (stats, postboard) =
        store::card_stats::sided_stats(&conn, deck_id(&conn), archetype).expect("sided");
    assert_eq!(
        (postboard.games, postboard.wins),
        (2, 0),
        "games 2 and 3 lost"
    );
    let boulder = stats.get("oracle:Giant's Boulder").expect("boulder");
    assert_eq!(boulder.sided_out_games, 2);
    assert_eq!(boulder.sided_in_games, 0);
    let chalice = stats.get("oracle:Chalice of the Void").expect("chalice");
    assert_eq!(
        (
            chalice.sided_in_games,
            chalice.sided_in_won,
            chalice.sided_in_lost
        ),
        (2, 0, 2)
    );
    assert_eq!(chalice.sided_out_games, 0);
    let flute = stats.get("oracle:Disruptor Flute").expect("flute");
    assert_eq!(flute.sided_in_games, 2);
}
