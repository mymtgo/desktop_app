//! Deck import tests (decks spec section 9): 0.x databases are built from
//! `fixtures/0x/schema.sql` (the 0.x schema, no data) plus fake rows, read with
//! `read_database`, and written with `import_matches` then `import_decks`, the
//! order the app uses. Fake names start with Q.

mod common;

use std::path::Path;

use common::{count, legacy_db, text, v1_db};
use decks::DeckCard;
use fixture_tools::TestDir;
use importer::{DeckRead, read_database};
use rusqlite::types::Value;
use rusqlite::{Connection, params};
use store::import::import_matches;
use store::import_decks::{DeckImportCounts, import_decks};

const SCHEMA: &str = include_str!("../../../fixtures/0x/schema.sql");

const STARTED: &str = "2026-01-02 03:04:05";
const STARTED_MS: i64 = 1_767_323_045_000;
const ENDED: &str = "2026-01-02 03:34:05";
const ENDED_MS: i64 = 1_767_324_845_000;

/// A legacy oracle-anchored signature: its first segment is an oracle id, not
/// a catalog id, so v1 cannot read it.
const LEGACY_SIGNATURE: &str = "YTNmMWMyZDQtMDAwMC00MDAwLTgwMDAtMDAwMDAwMDAwMDAxOjQ6ZmFsc2V8YjNmMWMyZDQtMDAwMC00MDAwLTgwMDAtMDAwMDAwMDAwMDAyOjI6dHJ1ZQ==";

/// A signature for `(catalog id, quantity, sideboard)` rows, as 0.x and v1
/// write it.
fn signature(rows: &[(u32, u32, bool)]) -> String {
    let cards: Vec<DeckCard> = rows
        .iter()
        .map(|(catalog_id, quantity, sideboard)| DeckCard {
            catalog_id: *catalog_id,
            quantity: *quantity,
            sideboard: *sideboard,
        })
        .collect();
    decks::signature(&cards)
}

fn burn_list() -> String {
    signature(&[(12345, 4, false), (67890, 2, true)])
}

fn burn_list_v2() -> String {
    signature(&[(12345, 3, false), (67890, 3, true)])
}

/// A 0.x database at `<dir>/nativephp.sqlite` built from `schema`.
/// The 0.x schema without the `cards`, `decks` and `deck_versions` tables and
/// their indexes (an old 0.x database).
fn schema_without_deck_tables() -> String {
    let mut kept = String::new();
    for line in SCHEMA.lines() {
        let deck_table = ["\"cards\"", "\"decks\"", "\"deck_versions\""]
            .iter()
            .any(|table| line.contains(&format!("TABLE IF NOT EXISTS {table}")))
            || ["\"cards\"", "\"decks\"", "\"deck_versions\""]
                .iter()
                .any(|table| line.contains(&format!(" on {table} ")));
        if !deck_table {
            kept.push_str(line);
            kept.push('\n');
        }
    }
    assert_ne!(
        kept.trim(),
        SCHEMA.trim(),
        "the deck tables are in the schema"
    );
    kept
}

/// Every row of `table`, every column, in rowid order.
fn rows(conn: &Connection, table: &str) -> Vec<Vec<Value>> {
    let sql = format!("SELECT * FROM {table} ORDER BY rowid");
    let mut stmt = conn.prepare(&sql).expect("prepare");
    let columns = stmt.column_count();
    let found = stmt
        .query_map([], |row| {
            let mut values = Vec::new();
            for index in 0..columns {
                values.push(row.get::<_, Value>(index)?);
            }
            Ok(values)
        })
        .expect("query");
    found.map(|row| row.expect("row")).collect()
}

/// Two players, one 0.x account, and two cards (plus a duplicate printing row
/// and one with a non-numeric id).
fn add_basics(conn: &Connection) {
    conn.execute_batch(
        "INSERT INTO players (id, username) VALUES (1, 'Qlocal'), (2, 'Qopponent');
         INSERT INTO accounts (id, username) VALUES (1, 'Qlocal');
         INSERT INTO cards (id, mtgo_id, scryfall_id, oracle_id, name, type, sub_type, rarity,
             color_identity, image, colors, cmc, set_name, set_code, art_crop, local_image,
             mana_cost)
         VALUES
             (1, '12345', 'q-scry-1', 'q-oracle-1', 'Q Bolt', 'Instant', NULL, 'common', 'R',
              'https://q.example/bolt.jpg', 'R', 1, 'Q Set', 'QST', 'https://q.example/bolt-art.jpg',
              '/local/q.jpg', '{R}'),
             (2, '67890', NULL, 'q-oracle-2', 'Q Guide', 'Creature', 'Goblin Scout', 'uncommon',
              'R', NULL, 'R', 1.5, NULL, NULL, NULL, NULL, '{R}'),
             (3, '67890', NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL,
              NULL, NULL, NULL),
             (4, 'not-a-number', NULL, NULL, 'Q Token', NULL, NULL, NULL, NULL, NULL, NULL, NULL,
              NULL, NULL, NULL, NULL, NULL);",
    )
    .expect("basics");
}

/// Give the cards the oracle ids `LEGACY_SIGNATURE` uses, and add a second
/// printing (catalog id 99999) of the first card: the lowest catalog id wins.
fn use_legacy_oracle_ids(conn: &Connection) {
    conn.execute_batch(
        "UPDATE cards SET oracle_id = 'a3f1c2d4-0000-4000-8000-000000000001' WHERE mtgo_id = '12345';
         UPDATE cards SET oracle_id = 'b3f1c2d4-0000-4000-8000-000000000002' WHERE mtgo_id = '67890';
         INSERT INTO cards (id, mtgo_id, oracle_id, name)
         VALUES (5, '99999', 'a3f1c2d4-0000-4000-8000-000000000001', 'Q Bolt (reprint)');",
    )
    .expect("oracle ids");
}

/// The deck row's user-editable columns, `|` separated.
fn deck_edits(conn: &Connection, mtgo_id: &str) -> Option<String> {
    text(
        conn,
        &format!(
            "SELECT concat_ws('|', name, original_name, format, color_identity,
                 color_identity_manual, cover_catalog_id, archived_at)
             FROM decks WHERE mtgo_id = '{mtgo_id}'"
        ),
    )
}

/// A complete 0.x match `id` with one won game between Qlocal and Qopponent,
/// played with 0.x deck version `deck_version_id`.
fn add_match(
    conn: &Connection,
    id: i64,
    token: &str,
    mtgo_id: &str,
    state: &str,
    deck_version_id: Option<i64>,
) {
    conn.execute(
        "INSERT INTO matches (id, token, mtgo_id, deck_version_id, format, match_type, games_won,
             games_lost, started_at, ended_at, state)
         VALUES (?1, ?2, ?3, ?4, 'CMODERN', 'Constructed', 2, 1, ?5, ?6, ?7)",
        params![id, token, mtgo_id, deck_version_id, STARTED, ENDED, state],
    )
    .expect("match");
    conn.execute(
        "INSERT INTO games (id, match_id, mtgo_id, won, started_at) VALUES (?1, ?2, ?3, 1, ?4)",
        params![id * 10, id, (id * 10).to_string(), STARTED],
    )
    .expect("game");
    conn.execute(
        "INSERT INTO game_player (game_id, player_id, instance_id, is_local)
         VALUES (?1, 1, 1, 1), (?1, 2, 1, 0)",
        [id * 10],
    )
    .expect("seats");
}

fn add_deck(conn: &Connection, id: i64, mtgo_id: &str, name: &str, format: &str) {
    conn.execute(
        "INSERT INTO decks (id, mtgo_id, name, format, created_at, updated_at, account_id,
             color_identity, cover_id, original_name)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1, 'R', 1, NULL)",
        params![id, mtgo_id, name, format, STARTED, ENDED],
    )
    .expect("deck");
}

fn add_version(conn: &Connection, id: i64, deck_id: i64, signature: &str, modified: &str) {
    conn.execute(
        "INSERT INTO deck_versions (id, deck_id, signature, modified_at) VALUES (?1, ?2, ?3, ?4)",
        params![id, deck_id, signature, modified],
    )
    .expect("version");
}

/// Read the 0.x database and import it into `v1` the way the app does:
/// matches first, then decks. Returns the deck counts.
fn import_all(path: &Path, v1: &mut Connection, now_ms: i64) -> DeckImportCounts {
    let read = read_database(path).expect("read");
    import_matches(v1, &read.matches, now_ms).expect("matches");
    let DeckRead { data, .. } = read.decks.expect("the 0.x database has decks");
    import_decks(v1, &data, now_ms).expect("decks")
}

#[test]
fn maps_cards_decks_and_versions() {
    let dir = TestDir::new("import-decks-mapping");
    let (path, conn) = legacy_db(&dir, SCHEMA);
    add_basics(&conn);
    add_deck(&conn, 1, "1001", "Q Burn", "CMODERN");
    conn.execute(
        "UPDATE decks SET original_name = 'Q Burn (file)' WHERE id = 1",
        [],
    )
    .expect("renamed in 0.x");
    add_version(&conn, 10, 1, &burn_list(), "2026-01-01 10:00:00");
    add_version(&conn, 11, 1, &burn_list_v2(), "2026-01-03 10:00:00");

    let read = read_database(&path).expect("read");
    let deck_read = read.decks.clone().expect("deck tables found");
    assert_eq!(
        deck_read.data.cards.len(),
        2,
        "one row per numeric catalog id"
    );
    assert_eq!(deck_read.data.decks.len(), 1);

    let mut v1 = v1_db(&dir);
    let counts = import_decks(&mut v1, &deck_read.data, 5_000).expect("import");
    assert_eq!(
        counts,
        DeckImportCounts {
            cards_added: 2,
            decks_added: 1,
            versions_added: 2,
            ..DeckImportCounts::default()
        }
    );

    assert_eq!(
        text(
            &v1,
            "SELECT concat_ws('|', catalog_id, oracle_id, name, type, rarity, color_identity,
                 colors, cmc, mana_cost, set_name, set_code, image, art_crop)
             FROM cards WHERE catalog_id = 12345"
        ),
        Some(
            "12345|q-oracle-1|Q Bolt|Instant|common|R|R|1.0|{R}|Q Set|QST|https://q.example/bolt.jpg|https://q.example/bolt-art.jpg"
                .to_string()
        )
    );
    assert_eq!(
        text(
            &v1,
            "SELECT concat_ws('|', name, sub_type, cmc) FROM cards WHERE catalog_id = 67890"
        ),
        Some("Q Guide|Goblin Scout|1.5".to_string()),
        "a duplicate row without data does not hide the row with data"
    );

    assert_eq!(
        text(
            &v1,
            "SELECT concat_ws('|', d.mtgo_id, a.username, d.name, d.original_name, d.format,
                 d.color_identity, d.color_identity_manual, d.cover_catalog_id, d.source,
                 d.created_at, d.updated_at)
             FROM decks d JOIN accounts a ON a.id = d.account_id"
        ),
        Some(format!(
            "1001|Qlocal|Q Burn|Q Burn (file)|CMODERN|R|0|12345|imported|{STARTED_MS}|{ENDED_MS}"
        ))
    );
    assert_eq!(
        count(
            &v1,
            "SELECT COUNT(*) FROM decks WHERE archived_at IS NULL AND file_synced_at IS NULL"
        ),
        1
    );
    let versions: Vec<(String, i64)> = {
        let mut stmt = v1
            .prepare("SELECT signature, modified_at FROM deck_versions ORDER BY modified_at")
            .expect("prepare");
        stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .expect("query")
            .map(|row| row.expect("row"))
            .collect()
    };
    assert_eq!(
        versions,
        vec![
            (burn_list(), 1_767_261_600_000),
            (burn_list_v2(), 1_767_434_400_000),
        ]
    );
}

#[test]
fn limited_decks_are_skipped() {
    let dir = TestDir::new("import-decks-limited");
    let (path, conn) = legacy_db(&dir, SCHEMA);
    add_basics(&conn);
    add_deck(&conn, 1, "1001", "Q Burn", "CMODERN");
    add_deck(&conn, 2, "1002", "Q Draft", "Limited");
    add_deck(&conn, 3, "limited:q-event", "Q Sealed", "CMODERN");
    add_version(&conn, 10, 1, &burn_list(), "2026-01-01 10:00:00");
    add_version(&conn, 20, 2, &burn_list(), "2026-01-01 10:00:00");
    add_version(&conn, 30, 3, &burn_list(), "2026-01-01 10:00:00");

    let deck_read = read_database(&path).expect("read").decks.expect("decks");
    assert_eq!(deck_read.skipped_limited, 2);
    let mut v1 = v1_db(&dir);
    import_decks(&mut v1, &deck_read.data, 5_000).expect("import");
    assert_eq!(
        text(&v1, "SELECT group_concat(mtgo_id) FROM decks"),
        Some("1001".to_string())
    );
    assert_eq!(count(&v1, "SELECT COUNT(*) FROM deck_versions"), 1);
}

#[test]
fn decks_with_match_style_format_codes_are_not_limited() {
    let dir = TestDir::new("import-decks-match-codes");
    let (path, conn) = legacy_db(&dir, SCHEMA);
    add_basics(&conn);
    add_deck(&conn, 1, "1001", "Q Duel", "DMU");
    add_deck(&conn, 2, "1002", "Q Pool", "S2");
    add_version(&conn, 10, 1, &burn_list(), "2026-01-01 10:00:00");
    add_version(&conn, 20, 2, &burn_list(), "2026-01-01 10:00:00");

    let deck_read = read_database(&path).expect("read").decks.expect("decks");
    assert_eq!(deck_read.skipped_limited, 0);
    let mut v1 = v1_db(&dir);
    import_decks(&mut v1, &deck_read.data, 5_000).expect("import");
    assert_eq!(count(&v1, "SELECT COUNT(*) FROM decks"), 2);
}

#[test]
fn an_archived_deck_is_archived_and_a_restore_in_0x_does_not_unarchive_it() {
    let dir = TestDir::new("import-decks-archived");
    let (path, conn) = legacy_db(&dir, SCHEMA);
    add_basics(&conn);
    add_deck(&conn, 1, "1001", "Q Burn", "CMODERN");
    conn.execute(
        "UPDATE decks SET deleted_at = '2026-02-03 04:05:06' WHERE id = 1",
        [],
    )
    .expect("archived in 0.x");
    let mut v1 = v1_db(&dir);

    import_all(&path, &mut v1, 5_000);
    assert_eq!(
        count(&v1, "SELECT archived_at FROM decks WHERE mtgo_id = '1001'"),
        1_770_091_506_000
    );

    conn.execute("UPDATE decks SET deleted_at = NULL WHERE id = 1", [])
        .expect("restored in 0.x");
    let counts = import_all(&path, &mut v1, 6_000);
    assert_eq!(
        counts,
        DeckImportCounts {
            cards_unchanged: 2,
            decks_unchanged: 1,
            ..DeckImportCounts::default()
        }
    );
    assert_eq!(
        count(&v1, "SELECT archived_at FROM decks WHERE mtgo_id = '1001'"),
        1_770_091_506_000,
        "v1's archive is never undone by the import"
    );
}

#[test]
fn a_legacy_version_with_an_unknown_oracle_id_is_skipped() {
    let dir = TestDir::new("import-decks-legacy");
    let (path, conn) = legacy_db(&dir, SCHEMA);
    add_basics(&conn);
    add_deck(&conn, 1, "1001", "Q Burn", "CMODERN");
    add_version(&conn, 10, 1, LEGACY_SIGNATURE, "2025-06-01 10:00:00");
    add_version(&conn, 11, 1, &burn_list(), "2026-01-01 10:00:00");
    add_version(&conn, 12, 1, "not base64 at all!", "2026-01-02 10:00:00");

    let deck_read = read_database(&path).expect("read").decks.expect("decks");
    assert_eq!(deck_read.skipped_versions, 2);
    let mut v1 = v1_db(&dir);
    import_decks(&mut v1, &deck_read.data, 5_000).expect("import");
    assert_eq!(
        text(&v1, "SELECT group_concat(signature) FROM deck_versions"),
        Some(burn_list())
    );
}

#[test]
fn a_legacy_version_is_converted_through_the_cards_and_its_matches_link() {
    let dir = TestDir::new("import-decks-legacy-converted");
    let (path, conn) = legacy_db(&dir, SCHEMA);
    add_basics(&conn);
    use_legacy_oracle_ids(&conn);
    add_deck(&conn, 1, "1001", "Q Burn", "CMODERN");
    // A modern version and a legacy one with the same list: one v1 version.
    add_version(&conn, 10, 1, &burn_list(), "2026-01-01 10:00:00");
    add_version(&conn, 11, 1, LEGACY_SIGNATURE, "2025-06-01 10:00:00");
    // Only a legacy version.
    add_deck(&conn, 2, "1002", "Q Burn Too", "CPAUPER");
    add_version(&conn, 20, 2, LEGACY_SIGNATURE, "2025-06-01 10:00:00");
    add_match(&conn, 1, "tok-modern", "101", "complete", Some(10));
    add_match(&conn, 2, "tok-legacy", "102", "complete", Some(11));
    add_match(&conn, 3, "tok-legacy-only", "103", "complete", Some(20));

    let deck_read = read_database(&path).expect("read").decks.clone();
    assert_eq!(deck_read.expect("decks").skipped_versions, 0);
    let mut v1 = v1_db(&dir);
    let counts = import_all(&path, &mut v1, 5_000);
    assert_eq!(
        (
            counts.versions_added,
            counts.versions_unchanged,
            counts.links_added
        ),
        (2, 1, 3)
    );

    let versions: Vec<(String, String, i64)> = {
        let mut stmt = v1
            .prepare(
                "SELECT d.mtgo_id, v.signature, v.modified_at FROM deck_versions v
                 JOIN decks d ON d.id = v.deck_id ORDER BY d.mtgo_id",
            )
            .expect("prepare");
        stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .expect("query")
            .map(|row| row.expect("row"))
            .collect()
    };
    assert_eq!(
        versions,
        vec![
            ("1001".to_string(), burn_list(), 1_767_261_600_000),
            ("1002".to_string(), burn_list(), 1_748_772_000_000),
        ],
        "the lowest catalog id per oracle id; a known list reuses its version"
    );
    assert_eq!(
        text(
            &v1,
            "SELECT group_concat(m.token || '=' || d.mtgo_id, ',') FROM matches m
             JOIN deck_versions v ON v.id = m.deck_version_id
             JOIN decks d ON d.id = v.deck_id"
        ),
        Some("tok-modern=1001,tok-legacy=1001,tok-legacy-only=1002".to_string())
    );
}

#[test]
fn a_file_deck_keeps_its_file_data_and_gains_0x_edits_and_new_versions() {
    let dir = TestDir::new("import-decks-file-wins");
    let (path, conn) = legacy_db(&dir, SCHEMA);
    add_basics(&conn);
    add_deck(&conn, 1, "1001", "Q Old Name", "CMODERN");
    conn.execute(
        "UPDATE decks SET deleted_at = '2026-02-03 04:05:06', original_name = 'Q Orig' WHERE id = 1",
        [],
    )
    .expect("0.x deck");
    add_version(&conn, 10, 1, &burn_list(), "2025-12-01 10:00:00");
    add_version(&conn, 11, 1, &burn_list_v2(), "2026-01-03 10:00:00");

    let mut v1 = v1_db(&dir);
    v1.execute(
        "INSERT INTO decks (id, mtgo_id, name, format, created_at, updated_at, file_synced_at,
             source)
         VALUES (7, '1001', 'Q File Name', 'CPIONEER', 1, 2, 3, 'file')",
        [],
    )
    .expect("file deck");
    v1.execute("UPDATE decks SET color_identity = 'G' WHERE id = 7", [])
        .expect("identity from the file");
    v1.execute(
        "INSERT INTO deck_versions (id, deck_id, signature, modified_at) VALUES (70, 7, ?1, 99)",
        [burn_list()],
    )
    .expect("file version");

    let counts = import_all(&path, &mut v1, 5_000);
    assert_eq!(
        counts,
        DeckImportCounts {
            cards_added: 2,
            decks_updated: 1,
            versions_added: 1,
            versions_unchanged: 1,
            ..DeckImportCounts::default()
        }
    );
    assert_eq!(
        deck_edits(&v1, "1001"),
        Some("Q Old Name|Q Orig|CPIONEER|R|0|12345|1770091506000".to_string()),
        "the 0.x rename, identity, cover and archive fill the blanks; the format stays the file's"
    );
    assert_eq!(
        text(
            &v1,
            "SELECT concat_ws('|', source, created_at, updated_at, file_synced_at) FROM decks"
        ),
        Some("file|1|2|3".to_string())
    );
    let deck_after = rows(&v1, "decks");
    let again = import_all(&path, &mut v1, 6_000);
    assert_eq!(
        (
            again.decks_unchanged,
            again.decks_updated,
            again.versions_added
        ),
        (1, 0, 0)
    );
    assert_eq!(rows(&v1, "decks"), deck_after, "a rerun changes nothing");
    assert_eq!(
        text(
            &v1,
            "SELECT group_concat(id || ':' || modified_at) FROM deck_versions WHERE signature = (SELECT signature FROM deck_versions WHERE id = 70)"
        ),
        Some("70:99".to_string()),
        "the file's version keeps its id and time"
    );
    assert_eq!(
        count(
            &v1,
            &format!(
                "SELECT COUNT(*) FROM deck_versions WHERE deck_id = 7 AND signature = '{}'",
                burn_list_v2()
            )
        ),
        1,
        "the new 0.x version is added to the file deck"
    );
}

#[test]
fn links_go_to_imported_and_live_matches_and_never_replace_a_link() {
    let dir = TestDir::new("import-decks-links");
    let (path, conn) = legacy_db(&dir, SCHEMA);
    add_basics(&conn);
    add_deck(&conn, 1, "1001", "Q Burn", "CMODERN");
    add_version(&conn, 10, 1, &burn_list(), "2026-01-01 10:00:00");
    add_version(&conn, 11, 1, LEGACY_SIGNATURE, "2025-06-01 10:00:00");
    add_deck(&conn, 2, "1002", "Q Draft", "Limited");
    add_version(&conn, 20, 2, &burn_list(), "2026-01-01 10:00:00");
    add_match(&conn, 1, "tok-imported", "101", "complete", Some(10));
    add_match(&conn, 2, "tok-live", "102", "complete", Some(10));
    // A 0.x history import of a match v1 tracked live under another token.
    add_match(&conn, 3, "tok-history", "555", "complete", Some(10));
    add_match(&conn, 4, "tok-linked", "104", "complete", Some(10));
    add_match(&conn, 5, "tok-legacy", "105", "complete", Some(11));
    add_match(&conn, 6, "tok-limited", "106", "complete", Some(20));
    add_match(&conn, 7, "tok-running", "107", "in_progress", Some(10));
    add_match(&conn, 8, "tok-no-deck", "108", "complete", None);

    let mut v1 = v1_db(&dir);
    v1.execute_batch(
        "INSERT INTO accounts (username, first_seen_at) VALUES ('Qlocal', 1);
         INSERT INTO matches (token, mtgo_match_id, account_id, source, state, outcome,
             started_at, last_activity_at, revision, derived_version)
         VALUES ('tok-live', 102, 1, 'log', 'complete', 'loss', 10, 20, 3, 1),
                ('tok-other-live', 555, 1, 'log', 'complete', 'loss', 10, 20, 3, 1);
         INSERT INTO decks (id, mtgo_id, name, format, created_at, updated_at, source)
         VALUES (9, '9009', 'Q Other', 'CMODERN', 1, 1, 'file');
         INSERT INTO deck_versions (id, deck_id, signature, modified_at) VALUES (90, 9, 'q', 1);",
    )
    .expect("live rows");
    // The 0.x matches are stored first, then v1 links one of them by hand.
    let read = read_database(&path).expect("read");
    import_matches(&mut v1, &read.matches, 4_000).expect("matches");
    v1.execute(
        "UPDATE matches SET deck_version_id = 90 WHERE token IN ('tok-live', 'tok-linked')",
        [],
    )
    .expect("links");

    let counts = import_all(&path, &mut v1, 5_000);
    assert_eq!((counts.links_added, counts.links_unchanged), (2, 2));

    let links: Vec<(String, String)> = {
        let mut stmt = v1
            .prepare(
                "SELECT m.token, d.mtgo_id FROM matches m
                 JOIN deck_versions v ON v.id = m.deck_version_id
                 JOIN decks d ON d.id = v.deck_id
                 ORDER BY m.token",
            )
            .expect("prepare");
        stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .expect("query")
            .map(|row| row.expect("row"))
            .collect()
    };
    assert_eq!(
        links,
        vec![
            ("tok-imported".to_string(), "1001".to_string()),
            ("tok-linked".to_string(), "9009".to_string()),
            ("tok-live".to_string(), "9009".to_string()),
            ("tok-other-live".to_string(), "1001".to_string()),
        ]
    );
}

#[test]
fn file_decks_without_an_identity_get_one_from_the_imported_cards() {
    let dir = TestDir::new("import-decks-identity");
    let (path, conn) = legacy_db(&dir, SCHEMA);
    add_basics(&conn);
    let mut v1 = v1_db(&dir);
    // Synced from their files before any card data: no identity computed.
    v1.execute_batch(
        "INSERT INTO decks (id, mtgo_id, name, format, created_at, updated_at, file_synced_at,
             source, color_identity, color_identity_manual)
         VALUES (7, '1007', 'Q Empty', 'CMODERN', 1, 2, 3, 'file', NULL, 0),
                (8, '1008', 'Q Manual', 'CMODERN', 1, 2, 3, 'file', NULL, 1),
                (9, '1009', 'Q Known', 'CMODERN', 1, 2, 3, 'file', 'G', 0),
                (10, '1010', 'Q Imported', 'CMODERN', 1, 2, NULL, 'imported', NULL, 0),
                (11, '1011', 'Q Unknown', 'CMODERN', 1, 2, 3, 'file', NULL, 0);",
    )
    .expect("decks");
    for deck_id in [7, 8, 9, 10] {
        v1.execute(
            "INSERT INTO deck_versions (deck_id, signature, modified_at) VALUES (?1, ?2, 99)",
            params![deck_id, burn_list()],
        )
        .expect("version");
    }
    // An older version with other cards: the latest one counts.
    v1.execute(
        "INSERT INTO deck_versions (deck_id, signature, modified_at) VALUES (7, ?1, 50)",
        [signature(&[(55555, 4, false)])],
    )
    .expect("older version");
    v1.execute(
        "INSERT INTO deck_versions (deck_id, signature, modified_at) VALUES (11, ?1, 99)",
        [signature(&[(55555, 4, false)])],
    )
    .expect("version without card data");

    import_all(&path, &mut v1, 5_000);

    let identity = |deck_id: i64| {
        text(
            &v1,
            &format!("SELECT color_identity FROM decks WHERE id = {deck_id}"),
        )
    };
    assert_eq!(
        identity(7),
        Some("R".to_string()),
        "computed from the cards"
    );
    assert_eq!(identity(8), None, "a manual identity is kept");
    assert_eq!(
        identity(9),
        Some("G".to_string()),
        "a stored identity is kept"
    );
    assert_eq!(identity(10), None, "an imported deck is left alone");
    assert_eq!(identity(11), None, "still no card data");
    assert_eq!(
        count(
            &v1,
            "SELECT COUNT(*) FROM decks WHERE color_identity_manual = 1"
        ),
        1
    );
}

#[test]
fn edits_made_in_v1_are_never_overwritten_by_a_rerun() {
    let dir = TestDir::new("import-decks-v1-edits");
    let (path, conn) = legacy_db(&dir, SCHEMA);
    add_basics(&conn);
    add_deck(&conn, 1, "1001", "Q Burn", "CMODERN");
    add_version(&conn, 10, 1, &burn_list(), "2026-01-01 10:00:00");
    let mut v1 = v1_db(&dir);
    import_all(&path, &mut v1, 5_000);

    // The user renames, picks a cover and colours, and archives in v1.
    v1.execute_batch(
        "UPDATE decks SET name = 'Q Mine', original_name = 'Q Burn', cover_catalog_id = 67890,
             color_identity = 'U', color_identity_manual = 1, archived_at = 777",
    )
    .expect("v1 edits");
    // 0.x changes the same things, and the format.
    conn.execute_batch(
        "UPDATE decks SET name = 'Q Zero', original_name = 'Q Burn', cover_id = 2,
             color_identity = 'B', deleted_at = '2026-02-03 04:05:06', format = 'CPAUPER'",
    )
    .expect("0.x edits");

    let counts = import_all(&path, &mut v1, 6_000);
    assert_eq!((counts.decks_updated, counts.decks_unchanged), (1, 0));
    assert_eq!(
        deck_edits(&v1, "1001"),
        Some("Q Mine|Q Burn|CPAUPER|U|1|67890|777".to_string()),
        "the v1 edits stay; the format follows 0.x"
    );
    let again = import_all(&path, &mut v1, 7_000);
    assert_eq!((again.decks_updated, again.decks_unchanged), (0, 1));
}

#[test]
fn a_second_import_changes_nothing() {
    let dir = TestDir::new("import-decks-rerun");
    let (path, conn) = legacy_db(&dir, SCHEMA);
    add_basics(&conn);
    use_legacy_oracle_ids(&conn);
    add_deck(&conn, 1, "1001", "Q Burn", "CMODERN");
    add_deck(&conn, 2, "1002", "Q Archived", "CPAUPER");
    conn.execute(
        "UPDATE decks SET deleted_at = '2026-02-03 04:05:06', account_id = NULL, cover_id = NULL
         WHERE id = 2",
        [],
    )
    .expect("archived deck");
    add_version(&conn, 10, 1, &burn_list(), "2026-01-01 10:00:00");
    add_version(&conn, 11, 1, &burn_list_v2(), "2026-01-03 10:00:00");
    add_version(&conn, 20, 2, LEGACY_SIGNATURE, "2026-01-01 10:00:00");
    add_match(&conn, 1, "tok-one", "101", "complete", Some(10));
    add_match(&conn, 2, "tok-two", "102", "complete", Some(11));
    add_match(&conn, 3, "tok-three", "103", "complete", Some(20));
    let mut v1 = v1_db(&dir);

    let first = import_all(&path, &mut v1, 5_000);
    assert_eq!(
        first,
        DeckImportCounts {
            cards_added: 3,
            decks_added: 2,
            versions_added: 3,
            links_added: 3,
            ..DeckImportCounts::default()
        }
    );
    let before: Vec<Vec<Vec<Value>>> = ["cards", "decks", "deck_versions", "matches"]
        .iter()
        .map(|table| rows(&v1, table))
        .collect();

    let second = import_all(&path, &mut v1, 9_000);
    assert_eq!(
        second,
        DeckImportCounts {
            cards_unchanged: 3,
            decks_unchanged: 2,
            versions_unchanged: 3,
            links_unchanged: 3,
            ..DeckImportCounts::default()
        }
    );
    let after: Vec<Vec<Vec<Value>>> = ["cards", "decks", "deck_versions", "matches"]
        .iter()
        .map(|table| rows(&v1, table))
        .collect();
    assert_eq!(after, before);
}

#[test]
fn a_database_without_deck_tables_imports_matches_only() {
    let dir = TestDir::new("import-decks-old-0x");
    let (path, conn) = legacy_db(&dir, &schema_without_deck_tables());
    conn.execute_batch(
        "INSERT INTO players (id, username) VALUES (1, 'Qlocal'), (2, 'Qopponent');",
    )
    .expect("players");
    add_match(&conn, 1, "tok-one", "101", "complete", Some(10));

    let read = read_database(&path).expect("read");
    assert_eq!(read.matches.len(), 1);
    assert!(read.decks.is_none(), "no deck data: matches only");
}

// Cards spec section 13: `scryfall_id` comes over with the other columns.
#[test]
fn the_0x_import_keeps_scryfall_ids() {
    let dir = TestDir::new("import-decks-scryfall");
    let (path, conn) = legacy_db(&dir, SCHEMA);
    add_basics(&conn);
    let mut v1 = v1_db(&dir);
    // v1 resolved 67890 itself (another answer than 0.x has): v1's values stay.
    v1.execute_batch(
        "INSERT INTO cards (catalog_id, scryfall_id, oracle_id, name)
         VALUES (67890, 'q-scry-v1', 'q-oracle-v1', 'Q Guide (v1)');",
    )
    .expect("v1 resolved row");

    let read = read_database(&path).expect("read");
    let data = read.decks.expect("deck tables").data;
    import_decks(&mut v1, &data, 5_000).expect("import");

    assert_eq!(
        text(
            &v1,
            "SELECT scryfall_id FROM cards WHERE catalog_id = 12345"
        ),
        Some("q-scry-1".to_string())
    );
    assert_eq!(
        text(
            &v1,
            "SELECT concat_ws('|', scryfall_id, oracle_id, name, type, rarity)
             FROM cards WHERE catalog_id = 67890"
        ),
        Some("q-scry-v1|q-oracle-v1|Q Guide (v1)|Creature|uncommon".to_string()),
        "a row v1 resolved keeps v1's values; 0.x fills only what v1 lacks"
    );

    // A rerun leaves both rows as they are.
    let before = rows(&v1, "cards");
    let again = import_decks(&mut v1, &data, 6_000).expect("again");
    assert_eq!(again.cards_unchanged, 2);
    assert_eq!(rows(&v1, "cards"), before);
}

#[test]
fn an_imported_resolved_card_is_not_asked_again() {
    let dir = TestDir::new("import-decks-resolved");
    let (path, conn) = legacy_db(&dir, SCHEMA);
    add_basics(&conn);
    let mut v1 = v1_db(&dir);
    let read = read_database(&path).expect("read");
    import_decks(&mut v1, &read.decks.expect("deck tables").data, 5_000).expect("import");

    let asked: Vec<u32> = store::cards::unresolved(&v1, None)
        .expect("unresolved")
        .iter()
        .map(|card| card.mtgo_id)
        .collect();
    assert_eq!(asked, vec![67890], "12345 came resolved from 0.x");
}

/// Issue #180 (Plan M ruling I1: the 0.x copy wins over a history import):
/// a 0.x deck link replaces the link a history import gave the same match,
/// by token or by MTGO match ID. A 0.x match with no link keeps the history
/// import's, and a rerun changes nothing.
#[test]
fn a_0x_deck_link_replaces_a_history_import_s_link() {
    let dir = TestDir::new("import-decks-history-link");
    let (path, conn) = legacy_db(&dir, SCHEMA);
    add_basics(&conn);
    add_deck(&conn, 1, "1001", "Q Burn", "CMODERN");
    add_version(&conn, 10, 1, &burn_list(), "2026-01-01 10:00:00");
    add_match(&conn, 1, "tok-same", "101", "complete", Some(10));
    add_match(&conn, 2, "tok-0x", "102", "complete", Some(10));
    add_match(&conn, 3, "tok-unlinked", "103", "complete", None);

    let mut v1 = v1_db(&dir);
    v1.execute_batch(
        "INSERT INTO accounts (username, first_seen_at) VALUES ('Qlocal', 1);
         INSERT INTO decks (id, mtgo_id, name, format, created_at, updated_at, source)
         VALUES (9, '9009', 'Q Other', 'CMODERN', 1, 1, 'file');
         INSERT INTO deck_versions (id, deck_id, signature, modified_at) VALUES (90, 9, 'q', 1);
         INSERT INTO matches (token, mtgo_match_id, account_id, source, state, outcome,
             started_at, last_activity_at, revision, derived_version, deck_version_id)
         VALUES ('tok-same', 101, 1, 'history', 'complete', 'win', 10, 20, 1, 0, 90),
                ('tok-history', 102, 1, 'history', 'complete', 'win', 10, 20, 1, 0, 90),
                ('tok-unlinked-history', 103, 1, 'history', 'complete', 'win', 10, 20, 1, 0, 90);",
    )
    .expect("history imports");

    import_all(&path, &mut v1, 5_000);
    let links = |v1: &Connection| {
        text(
            v1,
            "SELECT group_concat(row, ',') FROM (SELECT m.token || '=' || d.mtgo_id AS row
                 FROM matches m JOIN deck_versions v ON v.id = m.deck_version_id
                 JOIN decks d ON d.id = v.deck_id ORDER BY m.token)",
        )
    };
    assert_eq!(
        links(&v1),
        Some("tok-0x=1001,tok-same=1001,tok-unlinked=9009".to_string())
    );
    let before = rows(&v1, "matches");
    let again = import_all(&path, &mut v1, 6_000);
    assert_eq!((again.links_added, again.links_unchanged), (0, 2));
    assert_eq!(rows(&v1, "matches"), before, "a rerun writes nothing");
}

/// Issue #122 M13: of 0.x decks sharing an MTGO id the first wins; the
/// others (with their versions and links) are counted, not dropped silently.
#[test]
fn duplicate_0x_deck_ids_are_counted() {
    let dir = TestDir::new("import-decks-duplicates");
    let (path, conn) = legacy_db(&dir, SCHEMA);
    add_basics(&conn);
    add_deck(&conn, 1, "1001", "Q Burn", "CMODERN");
    add_deck(&conn, 2, "1001", "Q Burn copy", "CMODERN");
    add_deck(&conn, 3, "1001", "Q Burn copy two", "CMODERN");
    add_deck(&conn, 4, "1004", "Q Other", "CMODERN");
    add_version(&conn, 10, 1, &burn_list(), "2026-01-01 10:00:00");
    add_version(&conn, 20, 2, &burn_list_v2(), "2026-01-01 10:00:00");

    let deck_read = read_database(&path).expect("read").decks.expect("decks");
    assert_eq!(deck_read.skipped_duplicates, 2);
    let names: Vec<&str> = deck_read
        .data
        .decks
        .iter()
        .map(|deck| deck.name.as_str())
        .collect();
    assert_eq!(names, vec!["Q Burn", "Q Other"], "the first wins");
    // A Deck stats card on a duplicate finds the deck that won (#152).
    assert_eq!(
        deck_read.deck_mtgo_ids.get(&2).map(String::as_str),
        Some("1001")
    );
}
