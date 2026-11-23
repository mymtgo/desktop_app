//! The 0.x import of shared replay links (replays spec section 10, Plan V
//! task 3): `matches.replay_share_*` onto the v1 match with the same token
//! when v1 has none, a link missing a part skipped and counted, and the
//! pre-release per-game links (`games.replay_share_*`) counted only. 0.x
//! databases are built from `fixtures/0x/schema.sql` plus fake rows; fake
//! names start with Q.

mod common;

use std::path::PathBuf;

use common::v1_db;
use fixture_tools::TestDir;
use importer::read_database;
use rusqlite::{Connection, params};
use store::import::{ImportedReplayShare, import_matches};

const SCHEMA: &str = include_str!("../../../fixtures/0x/schema.sql");

const UUID: &str = "9b2f6f4e-0000-4000-8000-000000000001";
const URL: &str = "https://mymtgo.test/players/Qlocal/replays/9b2f6f4e-0000-4000-8000-000000000001";
/// 2026-01-02 03:10:00 UTC.
const SHARED: &str = "2026-01-02 03:10:00";
const SHARED_MS: i64 = 1_767_323_400_000;

fn legacy_db(dir: &TestDir, schema: &str) -> (PathBuf, Connection) {
    let path = dir.join("nativephp.sqlite");
    let conn = Connection::open(&path).expect("open 0.x test database");
    conn.execute_batch("PRAGMA foreign_keys = OFF;")
        .expect("no foreign keys");
    conn.execute_batch(schema).expect("0.x schema");
    conn.execute_batch(
        "INSERT INTO players (id, username) VALUES (1, 'Qlocal'), (2, 'Qopponent');",
    )
    .expect("players");
    (path, conn)
}

/// A complete 0.x match `id` with one game (`id * 10`) and both seats.
fn add_match(conn: &Connection, id: i64, token: &str) {
    conn.execute(
        "INSERT INTO matches (id, token, mtgo_id, format, match_type, games_won, games_lost,
             started_at, ended_at, state)
         VALUES (?1, ?2, ?3, 'CMODERN', 'Constructed', 1, 0, '2026-01-02 03:04:05',
             '2026-01-02 03:34:05', 'complete')",
        params![id, token, (4_000 + id).to_string()],
    )
    .expect("match");
    let game = id * 10;
    conn.execute(
        "INSERT INTO games (id, match_id, mtgo_id, won, started_at)
         VALUES (?1, ?2, ?3, 1, '2026-01-02 03:04:05')",
        params![game, id, (7_000 + id).to_string()],
    )
    .expect("game");
    for (player, local) in [(1, true), (2, false)] {
        conn.execute(
            "INSERT INTO game_player (game_id, player_id, instance_id, is_local)
             VALUES (?1, ?2, 1, ?3)",
            params![game, player, local],
        )
        .expect("seat");
    }
}

fn share(conn: &Connection, id: i64, uuid: Option<&str>, url: Option<&str>, at: Option<&str>) {
    conn.execute(
        "UPDATE matches SET replay_share_uuid = ?2, replay_share_url = ?3, replay_shared_at = ?4
         WHERE id = ?1",
        params![id, uuid, url, at],
    )
    .expect("share");
}

/// The v1 match's three share columns as one text, `-` for NULL.
fn v1_share(v1: &Connection, token: &str) -> String {
    v1.query_row(
        "SELECT concat_ws('|', COALESCE(replay_share_uuid, '-'), COALESCE(replay_share_url, '-'),
             COALESCE(replay_shared_at, '-'))
         FROM matches WHERE token = ?1",
        [token],
        |row| row.get(0),
    )
    .expect("the v1 match")
}

#[test]
fn the_share_columns_are_imported() {
    let dir = TestDir::new("share-import");
    let (path, conn) = legacy_db(&dir, SCHEMA);
    add_match(&conn, 1, "q-shared");
    add_match(&conn, 2, "q-not-shared");
    share(&conn, 1, Some(UUID), Some(URL), Some(SHARED));

    let read = read_database(&path).expect("read");
    let shared = read
        .matches
        .iter()
        .find(|imported| imported.token == "q-shared")
        .expect("read");
    assert_eq!(
        shared.replay_share,
        Some(ImportedReplayShare {
            uuid: UUID.to_string(),
            url: URL.to_string(),
            shared_at_ms: SHARED_MS,
        })
    );
    let mut v1 = v1_db(&dir);
    let counts = import_matches(&mut v1, &read.matches, 10).expect("import");
    assert_eq!(counts.replay_shares_added, 1);
    assert_eq!(
        v1_share(&v1, "q-shared"),
        format!("{UUID}|{URL}|{SHARED_MS}")
    );
    assert_eq!(v1_share(&v1, "q-not-shared"), "-|-|-");

    // A rerun writes nothing new and keeps the link.
    let again = import_matches(&mut v1, &read.matches, 20).expect("rerun");
    assert_eq!(again.replay_shares_added, 0);
    assert_eq!(again.unchanged, 2);
    assert_eq!(
        v1_share(&v1, "q-shared"),
        format!("{UUID}|{URL}|{SHARED_MS}")
    );
}

#[test]
fn a_v1_share_is_kept_on_a_rerun() {
    let dir = TestDir::new("share-import-v1-kept");
    let (path, conn) = legacy_db(&dir, SCHEMA);
    add_match(&conn, 1, "q-shared");
    let read = read_database(&path).expect("read");
    let mut v1 = v1_db(&dir);
    import_matches(&mut v1, &read.matches, 10).expect("first import");
    // Shared in v1 since, then shared again in 0.x: v1's link is input and
    // is never overwritten.
    v1.execute(
        "UPDATE matches SET replay_share_uuid = 'Q-v1', replay_share_url = 'https://mymtgo.test/r/v1',
             replay_shared_at = 5 WHERE token = 'q-shared'",
        [],
    )
    .expect("v1 share");
    share(&conn, 1, Some(UUID), Some(URL), Some(SHARED));
    let read = read_database(&path).expect("read again");
    let counts = import_matches(&mut v1, &read.matches, 20).expect("rerun");
    assert_eq!(counts.replay_shares_added, 0);
    assert_eq!(
        counts.replay_shares_unplaced, 1,
        "0.x's link has no v1 match to hold it (#307 F1)"
    );
    assert_eq!(v1_share(&v1, "q-shared"), "Q-v1|https://mymtgo.test/r/v1|5");
}

// #307 F1: a link whose match is left out stays on mymtgo.com: counted, so
// the summary says to switch it off there. A rerun holding the link counts
// nothing.
#[test]
fn a_link_of_a_match_left_out_is_counted_as_unplaced() {
    let dir = TestDir::new("share-import-unplaced");
    let (path, conn) = legacy_db(&dir, SCHEMA);
    add_match(&conn, 1, "q-shared");
    add_match(&conn, 2, "q-in-progress");
    share(&conn, 1, Some(UUID), Some(URL), Some(SHARED));
    share(&conn, 2, Some(UUID), Some(URL), Some(SHARED));
    conn.execute("UPDATE matches SET state = 'started' WHERE id = 2", [])
        .expect("in progress");

    let read = read_database(&path).expect("read");
    assert_eq!(read.skipped.in_progress, 1);
    assert_eq!(read.skipped.replay_shares_unplaced, 1);
    let mut v1 = v1_db(&dir);
    let counts = import_matches(&mut v1, &read.matches, 10).expect("import");
    assert_eq!(
        (counts.replay_shares_added, counts.replay_shares_unplaced),
        (1, 0)
    );
    let again = import_matches(&mut v1, &read.matches, 20).expect("rerun");
    assert_eq!(
        (again.replay_shares_added, again.replay_shares_unplaced),
        (0, 0)
    );
}

#[test]
fn a_link_missing_a_part_is_skipped_and_counted() {
    let dir = TestDir::new("share-import-incomplete");
    let (path, conn) = legacy_db(&dir, SCHEMA);
    add_match(&conn, 1, "q-no-url");
    add_match(&conn, 2, "q-no-time");
    add_match(&conn, 3, "q-bad-time");
    add_match(&conn, 4, "q-url-only");
    share(&conn, 1, Some(UUID), None, Some(SHARED));
    share(&conn, 2, Some(UUID), Some(URL), None);
    share(&conn, 3, Some(UUID), Some(URL), Some("yesterday"));
    share(&conn, 4, None, Some(URL), Some(SHARED));

    let read = read_database(&path).expect("read");
    assert_eq!(read.skipped.replay_shares_incomplete, 4);
    assert!(
        read.matches
            .iter()
            .all(|imported| imported.replay_share.is_none())
    );
    assert_eq!(read.matches.len(), 4, "the matches themselves are imported");
    let mut v1 = v1_db(&dir);
    import_matches(&mut v1, &read.matches, 10).expect("import");
    for token in ["q-no-url", "q-no-time", "q-bad-time", "q-url-only"] {
        assert_eq!(v1_share(&v1, token), "-|-|-", "{token}");
    }
}

#[test]
fn legacy_per_game_links_are_counted_and_never_imported() {
    let dir = TestDir::new("share-import-legacy-games");
    let (path, conn) = legacy_db(&dir, SCHEMA);
    add_match(&conn, 1, "q-legacy");
    conn.execute(
        "UPDATE games SET replay_share_uuid = 'Q-game-link', replay_share_url = 'https://mymtgo.test/r/game',
             replay_shared_at = ?1 WHERE id = 10",
        [SHARED],
    )
    .expect("a per-game link");
    let read = read_database(&path).expect("read");
    assert_eq!(read.skipped.legacy_game_links, 1);
    assert_eq!(read.matches[0].replay_share, None);
    let mut v1 = v1_db(&dir);
    import_matches(&mut v1, &read.matches, 10).expect("import");
    assert_eq!(v1_share(&v1, "q-legacy"), "-|-|-");
}

#[test]
fn a_0x_without_share_columns_reads_no_links() {
    let dir = TestDir::new("share-import-old");
    let old_schema = SCHEMA.replace(
        r#", "replay_share_uuid" varchar, "replay_share_url" varchar, "replay_shared_at" datetime"#,
        "",
    );
    assert!(!old_schema.contains("replay_share"));
    let (path, conn) = legacy_db(&dir, &old_schema);
    add_match(&conn, 1, "q-old");
    let read = read_database(&path).expect("read");
    assert_eq!(read.matches.len(), 1);
    assert_eq!(read.matches[0].replay_share, None);
    assert_eq!(read.skipped.replay_shares_incomplete, 0);
    assert_eq!(read.skipped.legacy_game_links, 0);
}
