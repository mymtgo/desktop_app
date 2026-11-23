//! Archetype, sideboard guide and matchup note import tests (archetypes spec
//! sections 15 and 21.5): a 0.x database built from `fixtures/0x/schema.sql`
//! and fake rows, read with `read_database` and `read_archetypes_at`, and
//! written in the app's order: matches, decks, then archetypes. Fake names
//! start with Q.

mod common;

use std::path::{Path, PathBuf};

use common::{count, legacy_db, texts, v1_db};
use decks::DeckCard;
use fixture_tools::TestDir;
use importer::archetype_read::{read_archetypes, read_archetypes_at};
use importer::read_database;
use rusqlite::{Connection, params};
use store::archetype_edit::{
    ArchetypeEdit, ArchetypeEditOutcome, MatchDetection, SeatResult, edit_archetypes,
};
use store::import::import_matches;
use store::import_archetypes::{ArchetypeImportCounts, import_archetypes};
use store::import_decks::import_decks;

const SCHEMA: &str = include_str!("../../../fixtures/0x/schema.sql");
const NOW: i64 = 1_800_000_000_000;
const LATER: i64 = 1_800_000_500_000;
const HOMEBREW: &str = "00000000-0000-0000-0000-000000000001";

fn ms(text: &str) -> i64 {
    chrono::NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M:%S")
        .expect("time")
        .and_utc()
        .timestamp_millis()
}

fn id_of(conn: &Connection, uuid: &str) -> i64 {
    conn.query_row("SELECT id FROM archetypes WHERE uuid = ?1", [uuid], |row| {
        row.get(0)
    })
    .expect(uuid)
}

fn match_id(conn: &Connection, token: &str) -> i64 {
    conn.query_row("SELECT id FROM matches WHERE token = ?1", [token], |row| {
        row.get(0)
    })
    .expect(token)
}

fn deck_id(conn: &Connection, mtgo_id: &str) -> i64 {
    conn.query_row(
        "SELECT id FROM decks WHERE mtgo_id = ?1",
        [mtgo_id],
        |row| row.get(0),
    )
    .expect(mtgo_id)
}

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

/// Players, one account, three cards, two decks (the second archived).
fn add_basics(conn: &Connection) {
    conn.execute_batch(
        "INSERT INTO players (id, username) VALUES (1, 'Qlocal'), (2, 'Qopponent'),
             (3, 'Qrival');
         INSERT INTO accounts (id, username) VALUES (1, 'Qlocal');
         INSERT INTO cards (id, mtgo_id, oracle_id, name, type) VALUES
             (1, '12345', 'q-oracle-1', 'Q Bolt', 'Instant'),
             (2, '67890', 'q-oracle-2', 'Q Guide', 'Creature'),
             (3, '55555', 'q-oracle-3', 'Q Path', 'Instant'),
             (4, 'not-a-number', NULL, 'Q Token', NULL);
         INSERT INTO decks (id, mtgo_id, name, format, created_at, updated_at, account_id,
             deleted_at)
         VALUES (1, '1001', 'Q Burn', 'CMODERN', '2026-01-01 10:00:00',
                 '2026-01-01 10:00:00', 1, NULL),
                (2, '1002', 'Q Old', 'CMODERN', '2026-01-01 10:00:00',
                 '2026-01-01 10:00:00', 1, '2026-01-04 10:00:00');",
    )
    .expect("basics");
    conn.execute(
        "INSERT INTO deck_versions (id, deck_id, signature, modified_at)
         VALUES (10, 1, ?1, '2026-01-01 10:00:00'), (11, 2, ?1, '2026-01-01 10:00:00')",
        [signature(&[(12345, 4, false), (67890, 2, true)])],
    )
    .expect("versions");
}

/// A complete 0.x match `id` played with deck version 10: one game where
/// `local` sat at the local seat and `opponent` across.
fn add_match(conn: &Connection, id: i64, state: &str, local: i64, opponent: i64) {
    conn.execute(
        "INSERT INTO matches (id, token, mtgo_id, deck_version_id, format, match_type, games_won,
             games_lost, started_at, ended_at, state)
         VALUES (?1, ?2, ?3, 10, 'CMODERN', 'Constructed', 2, 0, ?4, ?4, ?5)",
        params![
            id,
            format!("q-m{id}"),
            (290_000_000 + id).to_string(),
            format!("2026-01-0{id} 12:00:00"),
            state
        ],
    )
    .expect("match");
    conn.execute(
        "INSERT INTO games (id, match_id, mtgo_id, won, started_at) VALUES (?1, ?2, ?3, 1, ?4)",
        params![
            id * 10,
            id,
            (id * 10).to_string(),
            format!("2026-01-0{id} 12:00:00")
        ],
    )
    .expect("game");
    conn.execute(
        "INSERT INTO game_player (game_id, player_id, instance_id, is_local)
         VALUES (?1, ?2, 1, 1), (?1, ?3, 2, 0)",
        params![id * 10, local, opponent],
    )
    .expect("seats");
}

/// The archetype rows of the main fixture:
/// - 1, 2: 0.x's fallbacks (Homebrew, Rogue);
/// - 10 Q Burn Arch (downloaded, the decks' and player seats' archetype,
///   with an API sample deck that is not imported);
/// - 11 Q Tron (downloaded, opponents, a guide and two notes);
/// - 12 Q Unused (downloaded, only a dropped second opponent row: skipped);
/// - 13 Q Guide Only (downloaded, only the archived deck's empty guide);
/// - 14 Q Parent merged into 15 Q Grand (downloaded, only the merge chain
///   of manual 21);
/// - 16 a stub without a format (only a deck link of deck 2);
/// - 20 Q Manual A (manual, two variants and a legacy list that is
///   ignored, made from match 3, incomplete);
/// - 21 Q Manual B (manual, no variant: its legacy list is list 1; merged
///   into 14);
/// - 22 Q Manual C (manual, not referenced: still imported).
fn add_archetypes(conn: &Connection) {
    conn.execute_batch(
        "INSERT INTO archetypes (id, uuid, name, format, color_identity, created_at, updated_at,
             decklist_downloaded_at, manual, is_fallback, source_match_id, incomplete,
             merged_into_id)
         VALUES
             (1, '00000000-0000-0000-0000-000000000001', 'Homebrew', NULL, NULL,
              '2026-01-01 00:00:00', '2026-01-01 00:00:00', NULL, 0, 1, NULL, 0, NULL),
             (2, '00000000-0000-0000-0000-000000000002', 'Rogue', NULL, NULL,
              '2026-01-01 00:00:00', '2026-01-01 00:00:00', NULL, 0, 1, NULL, 0, NULL),
             (10, 'q-uuid-burn', 'Q Burn Arch', 'modern', 'R', '2026-01-01 00:00:00',
              '2026-01-01 00:00:00', '2026-01-01 00:00:00', 0, 0, NULL, 0, NULL),
             (11, 'q-uuid-tron', 'Q Tron', 'modern', 'G', '2026-01-01 00:00:00',
              '2026-01-01 00:00:00', NULL, 0, 0, NULL, 0, NULL),
             (12, 'q-uuid-unused', 'Q Unused', 'modern', 'U', '2026-01-01 00:00:00',
              '2026-01-01 00:00:00', NULL, 0, 0, NULL, 0, NULL),
             (13, 'q-uuid-guide-only', 'Q Guide Only', 'modern', 'W', '2026-01-01 00:00:00',
              '2026-01-01 00:00:00', NULL, 0, 0, NULL, 0, NULL),
             (14, 'q-uuid-parent', 'Q Parent', 'modern', 'B', '2026-01-01 00:00:00',
              '2026-01-01 00:00:00', NULL, 0, 0, NULL, 0, 15),
             (15, 'q-uuid-grand', 'Q Grand', 'modern', 'B', '2026-01-01 00:00:00',
              '2026-01-01 00:00:00', NULL, 0, 0, NULL, 0, NULL),
             (16, 'q-uuid-stub', 'q-uuid-stub', NULL, NULL, '2026-01-01 00:00:00',
              '2026-01-01 00:00:00', NULL, 0, 0, NULL, 0, NULL),
             (20, 'qdev0001-manual-a', 'Q Manual A', 'modern', 'R,G', '2026-01-02 00:00:00',
              '2026-01-03 00:00:00', NULL, 1, 0, 3, 1, NULL),
             (21, 'qdev0001-manual-b', 'Q Manual B', 'modern', 'B', '2026-01-02 00:00:00',
              '2026-01-02 00:00:00', NULL, 1, 0, NULL, 0, 14),
             (22, 'qdev0001-manual-c', 'Q Manual C', 'pauper', 'U', '2026-01-02 00:00:00',
              '2026-01-02 00:00:00', NULL, 1, 0, NULL, 0, NULL);
         INSERT INTO archetype_decks (id, uuid, archetype_id, seen_count) VALUES
             (100, 'q-variant-1', 20, 0), (101, 'q-variant-2', 20, 0),
             (200, 'q-sample', 10, 40);
         INSERT INTO archetype_deck_cards (id, archetype_deck_id, card_id, quantity, sideboard)
         VALUES (1, 100, 1, 4, 0), (2, 100, 2, 2, 1), (3, 101, 3, 3, 0), (4, 101, 4, 1, 0),
                (5, 200, 1, 4, 0);
         INSERT INTO archetype_cards (id, archetype_id, card_id, quantity, sideboard) VALUES
             (1, 20, 3, 1, 0), (2, 21, 2, 4, 0), (3, 21, 1, 1, 1), (4, 22, 3, 2, 0);
         UPDATE decks SET archetype_id = 10 WHERE id = 1;
         UPDATE decks SET archetype_id = 16 WHERE id = 2;",
    )
    .expect("archetypes");
}

/// Matches 1 to 5 (complete) and 6 (still in progress, not imported), with
/// their archetype rows:
/// - 1: the opponent row comes first by id (the seat is `is_local`'s), a
///   second opponent row is dropped (the first per seat wins);
/// - 2: Homebrew for the opponent;
/// - 3: the local seat is player 3 (a second account); the opponent was set
///   by hand to Q Manual A;
/// - 4: only the player seat;
/// - 5: no rows;
/// - 6: a row of a match v1 does not import.
fn add_matches(conn: &Connection) {
    add_match(conn, 1, "complete", 1, 2);
    add_match(conn, 2, "complete", 1, 2);
    add_match(conn, 3, "complete", 3, 2);
    add_match(conn, 4, "complete", 1, 2);
    add_match(conn, 5, "complete", 1, 2);
    add_match(conn, 6, "started", 1, 2);
    conn.execute_batch(
        "INSERT INTO match_archetypes (id, archetype_id, mtgo_match_id, player_id, confidence,
             archetype_deck_id, manual)
         VALUES (1, 11, 1, 2, '0.8', NULL, 0),
                (2, 10, 1, 1, 0.95, 200, 0),
                (3, 12, 1, 2, 0.5, NULL, 0),
                (4, 1, 2, 2, 0, NULL, 0),
                (5, 10, 2, 1, 1, NULL, 0),
                (6, 20, 3, 2, 1, 100, 1),
                (7, 10, 3, 3, 1, NULL, 0),
                (8, 10, 4, 1, 1, NULL, 0),
                (9, 11, 6, 2, 0.9, NULL, 0);",
    )
    .expect("match archetypes");
}

/// Two guides (the archived deck's one empty, against an archetype only
/// it references), cards in and out, and three notes on deck 1: two against
/// Q Tron in the same second, one against Q Burn Arch (a pair without a
/// guide).
fn add_guides(conn: &Connection) {
    conn.execute_batch(
        "INSERT INTO sideboard_guides (id, deck_id, archetype_id, created_at, updated_at)
         VALUES (1, 1, 11, '2026-01-05 09:00:00', '2026-01-05 09:30:00'),
                (2, 2, 13, '2026-01-05 09:00:00', '2026-01-05 09:00:00');
         INSERT INTO sideboard_guide_cards (id, sideboard_guide_id, oracle_id, direction,
             quantity)
         VALUES (1, 1, 'q-oracle-2', 'in', 2), (2, 1, 'q-oracle-1', 'out', 2),
                (3, 1, 'q-oracle-3', 'sideways', 1), (4, 1, 'q-oracle-9', 'in', 0);
         INSERT INTO deck_archetype_notes (id, deck_id, archetype_id, body, created_at,
             updated_at)
         VALUES (1, 1, 11, 'Q keep the bolts', '2026-01-05 10:00:00', '2026-01-05 10:00:00'),
                (2, 1, 11, 'Q side in the guide', '2026-01-05 10:00:00',
                 '2026-01-05 10:00:00'),
                (3, 1, 10, 'Q mirror: be the control', '2026-01-06 10:00:00',
                 '2026-01-06 10:00:00'),
                (4, 1, 11, '', '2026-01-06 11:00:00', '2026-01-06 11:00:00');",
    )
    .expect("guides and notes");
}

fn full_legacy(dir: &TestDir) -> PathBuf {
    let (path, conn) = legacy_db(dir, SCHEMA);
    add_basics(&conn);
    add_archetypes(&conn);
    add_matches(&conn);
    add_guides(&conn);
    path
}

/// Read the 0.x database and import it into `v1` the way the app does:
/// matches, decks, then archetypes.
fn import_all(path: &Path, v1: &mut Connection, now_ms: i64) -> ArchetypeImportCounts {
    let read = read_database(path).expect("read");
    import_matches(v1, &read.matches, now_ms).expect("matches");
    let decks = read.decks.expect("decks");
    import_decks(v1, &decks.data, now_ms).expect("decks");
    let mut archetypes = read_archetypes_at(path).expect("read archetypes");
    archetypes.import_started_at_ms = Some(now_ms);
    import_archetypes(v1, &archetypes, now_ms).expect("archetypes")
}

/// `(uuid, name, format, colours, source)` of every archetype.
fn archetype_rows(conn: &Connection) -> Vec<String> {
    texts(
        conn,
        "SELECT concat_ws('|', uuid, name, coalesce(format, '-'), coalesce(color_identity, '-'),
             source)
         FROM archetypes ORDER BY uuid",
    )
}

/// `(list, catalog id, oracle id, quantity, sideboard)` of an archetype.
fn list_rows(conn: &Connection, uuid: &str) -> Vec<String> {
    texts(
        conn,
        &format!(
            "SELECT concat_ws('|', c.list, c.catalog_id, coalesce(c.oracle_id, '-'), c.quantity,
                 c.sideboard)
             FROM archetype_cards c JOIN archetypes a ON a.id = c.archetype_id
             WHERE a.uuid = '{uuid}' ORDER BY c.list, c.sideboard, c.catalog_id"
        ),
    )
}

/// `(token, seat, uuid, confidence, manual)` of every match row.
fn match_rows(conn: &Connection) -> Vec<String> {
    texts(
        conn,
        "SELECT concat_ws('|', m.token, CASE x.is_opponent WHEN 1 THEN 'opponent' ELSE 'player' END,
             a.uuid, x.confidence, x.manual)
         FROM match_archetypes x JOIN matches m ON m.id = x.match_id
         JOIN archetypes a ON a.id = x.archetype_id
         ORDER BY m.token, x.is_opponent",
    )
}

/// `(deck, archetype uuid, created, updated, cards)` of every guide.
fn guide_rows(conn: &Connection) -> Vec<String> {
    texts(
        conn,
        "SELECT concat_ws('|', d.mtgo_id, a.uuid, g.created_at, g.updated_at,
             coalesce((SELECT group_concat(c.direction || ':' || c.oracle_id || ':' || c.quantity, ',')
                       FROM (SELECT * FROM sideboard_guide_cards WHERE guide_id = g.id
                             ORDER BY direction, oracle_id) c), '-'))
         FROM sideboard_guides g JOIN decks d ON d.id = g.deck_id
         JOIN archetypes a ON a.id = g.archetype_id
         ORDER BY d.mtgo_id, a.uuid",
    )
}

/// `(deck, archetype uuid, created, body)` of every note.
fn note_rows(conn: &Connection) -> Vec<String> {
    texts(
        conn,
        "SELECT concat_ws('|', d.mtgo_id, a.uuid, n.created_at, n.body)
         FROM deck_archetype_notes n JOIN decks d ON d.id = n.deck_id
         JOIN archetypes a ON a.id = n.archetype_id
         ORDER BY d.mtgo_id, a.uuid, n.created_at, n.body",
    )
}

#[test]
fn archetypes_match_rows_deck_links_guides_and_notes_import() {
    let dir = TestDir::new("import-archetypes-all");
    let path = full_legacy(&dir);
    let mut v1 = v1_db(&dir);
    let counts = import_all(&path, &mut v1, NOW);

    // Referenced, chained and manual rows; not the unused catalog row; the
    // fallbacks are v1's seeds; the format-less stub takes its deck's format.
    assert_eq!(
        archetype_rows(&v1),
        vec![
            "00000000-0000-0000-0000-000000000001|Homebrew|-|-|fallback",
            "00000000-0000-0000-0000-000000000002|Rogue|-|-|fallback",
            "q-uuid-burn|Q Burn Arch|modern|R|downloaded",
            "q-uuid-grand|Q Grand|modern|B|downloaded",
            "q-uuid-guide-only|Q Guide Only|modern|W|downloaded",
            "q-uuid-parent|Q Parent|modern|B|downloaded",
            "q-uuid-stub|q-uuid-stub|modern|-|downloaded",
            "q-uuid-tron|Q Tron|modern|G|downloaded",
            "qdev0001-manual-a|Q Manual A|modern|R,G|manual",
            "qdev0001-manual-b|Q Manual B|modern|B|manual",
            "qdev0001-manual-c|Q Manual C|pauper|U|manual",
        ]
    );
    // Downloaded rows wait for the change check: no definition, no list.
    assert_eq!(
        count(
            &v1,
            "SELECT count(*) FROM archetypes WHERE source = 'downloaded' AND definition IS NULL"
        ),
        6
    );
    assert!(list_rows(&v1, "q-uuid-burn").is_empty(), "no sample decks");
    // Manual A: its variants in id order (the card without a catalog id is
    // left out); its legacy list is ignored.
    assert_eq!(
        list_rows(&v1, "qdev0001-manual-a"),
        vec![
            "1|12345|q-oracle-1|4|0",
            "1|67890|q-oracle-2|2|1",
            "2|55555|q-oracle-3|3|0"
        ]
    );
    // Manual B: no variant, so its legacy list is list 1.
    assert_eq!(
        list_rows(&v1, "qdev0001-manual-b"),
        vec!["1|67890|q-oracle-2|4|0", "1|12345|q-oracle-1|1|1"]
    );
    // Merge chains, the source match, incomplete, times.
    let merged = texts(
        &v1,
        "SELECT concat_ws('|', a.uuid, p.uuid) FROM archetypes a
         JOIN archetypes p ON p.id = a.merged_into_id ORDER BY a.uuid",
    );
    assert_eq!(
        merged,
        vec![
            "q-uuid-parent|q-uuid-grand",
            "qdev0001-manual-b|q-uuid-parent"
        ]
    );
    assert_eq!(
        count(
            &v1,
            "SELECT source_match_id FROM archetypes WHERE uuid = 'qdev0001-manual-a'"
        ),
        match_id(&v1, "q-m3")
    );
    assert_eq!(
        count(
            &v1,
            "SELECT incomplete FROM archetypes WHERE uuid = 'qdev0001-manual-a'"
        ),
        1
    );
    assert_eq!(
        count(
            &v1,
            "SELECT created_at FROM archetypes WHERE uuid = 'qdev0001-manual-a'"
        ),
        ms("2026-01-02 00:00:00")
    );
    assert_eq!(
        count(
            &v1,
            "SELECT updated_at FROM archetypes WHERE uuid = 'qdev0001-manual-a'"
        ),
        ms("2026-01-03 00:00:00"),
        "0.x's time, so a later v1 edit is seen as newer"
    );

    // Match rows: the seat from is_local, the first row per seat, Homebrew
    // mapped to the seed, the hand-set row kept hand-set.
    assert_eq!(
        match_rows(&v1),
        vec![
            "q-m1|player|q-uuid-burn|0.95|0".to_string(),
            "q-m1|opponent|q-uuid-tron|0.8|0".to_string(),
            "q-m2|player|q-uuid-burn|1.0|0".to_string(),
            format!("q-m2|opponent|{HOMEBREW}|0.0|0"),
            "q-m3|player|q-uuid-burn|1.0|0".to_string(),
            "q-m3|opponent|qdev0001-manual-a|1.0|1".to_string(),
            "q-m4|player|q-uuid-burn|1.0|0".to_string(),
        ]
    );
    // Decks: both links (the archived deck's too).
    assert_eq!(
        texts(
            &v1,
            "SELECT concat_ws('|', d.mtgo_id, a.uuid) FROM decks d
             JOIN archetypes a ON a.id = d.archetype_id ORDER BY d.mtgo_id"
        ),
        vec!["1001|q-uuid-burn", "1002|q-uuid-stub"]
    );

    // Guides: cards kept (rows breaking a CHECK skipped), the archived
    // deck's empty guide, and an empty guide for the noted pair without one.
    assert_eq!(
        guide_rows(&v1),
        vec![
            format!("1001|q-uuid-burn|{NOW}|{NOW}|-"),
            format!(
                "1001|q-uuid-tron|{}|{}|in:q-oracle-2:2,out:q-oracle-1:2",
                ms("2026-01-05 09:00:00"),
                ms("2026-01-05 09:30:00")
            ),
            format!(
                "1002|q-uuid-guide-only|{}|{}|-",
                ms("2026-01-05 09:00:00"),
                ms("2026-01-05 09:00:00")
            ),
        ]
    );
    // Notes: all three (two in the same second), not the empty one.
    assert_eq!(
        note_rows(&v1),
        vec![
            format!(
                "1001|q-uuid-burn|{}|Q mirror: be the control",
                ms("2026-01-06 10:00:00")
            ),
            format!(
                "1001|q-uuid-tron|{}|Q keep the bolts",
                ms("2026-01-05 10:00:00")
            ),
            format!(
                "1001|q-uuid-tron|{}|Q side in the guide",
                ms("2026-01-05 10:00:00")
            ),
        ]
    );

    assert_eq!(
        counts,
        ArchetypeImportCounts {
            archetypes_added: 9,
            archetypes_updated: 0,
            // Homebrew, mapped to the seed (Rogue is not referenced).
            archetypes_unchanged: 1,
            match_rows_added: 7,
            match_rows_kept: 0,
            match_rows_skipped: 1,
            matches_detected: 3,
            decks_linked: 2,
            guides_added: 3,
            guides_kept: 0,
            guide_cards_added: 2,
            notes_added: 3,
            notes_unchanged: 0,
            notes_skipped: 1,
        }
    );
}

#[test]
fn imported_rows_mark_their_matches_detected() {
    let dir = TestDir::new("import-archetypes-detected");
    let path = full_legacy(&dir);
    let mut v1 = v1_db(&dir);
    import_all(&path, &mut v1, NOW);
    // Matches that brought an opponent row are detected at the import time;
    // a match with only its player seat, or none, waits for detection (an
    // empty opponent seat with a marker reads as the player's "none").
    assert_eq!(
        texts(
            &v1,
            "SELECT concat_ws('|', token, coalesce(archetypes_detected_at, '-')) FROM matches
             ORDER BY token"
        ),
        vec![
            format!("q-m1|{NOW}"),
            format!("q-m2|{NOW}"),
            format!("q-m3|{NOW}"),
            "q-m4|-".to_string(),
            "q-m5|-".to_string(),
        ]
    );
}

#[test]
fn the_import_rerun_keeps_v1_hand_edits() {
    let dir = TestDir::new("import-archetypes-rerun");
    let path = full_legacy(&dir);
    let mut v1 = v1_db(&dir);
    import_all(&path, &mut v1, NOW);

    // In v1: match 1's opponent set by hand to Q Manual C, match 2's
    // opponent set to "none", match 3's player seat set by hand, and Q
    // Manual A renamed.
    let manual_a = id_of(&v1, "qdev0001-manual-a");
    let manual_c = id_of(&v1, "qdev0001-manual-c");
    let set = |v1: &mut Connection, edit: ArchetypeEdit| {
        let outcome = edit_archetypes(v1, &edit).expect("edit");
        assert!(
            matches!(outcome, ArchetypeEditOutcome::Done { .. }),
            "{outcome:?}"
        );
    };
    let (m1, m2) = (match_id(&v1, "q-m1"), match_id(&v1, "q-m2"));
    set(
        &mut v1,
        ArchetypeEdit::SetOpponent {
            match_ids: vec![m1],
            archetype_id: Some(manual_c),
            now_ms: LATER,
        },
    );
    set(
        &mut v1,
        ArchetypeEdit::SetOpponent {
            match_ids: vec![m2],
            archetype_id: None,
            now_ms: LATER,
        },
    );
    v1.execute(
        "UPDATE match_archetypes SET archetype_id = ?1, manual = 1
         WHERE match_id = ?2 AND is_opponent = 0",
        params![manual_c, match_id(&v1, "q-m3")],
    )
    .expect("player seat by hand");
    set(
        &mut v1,
        ArchetypeEdit::UpdateManual {
            id: manual_a,
            name: "Q Renamed".to_string(),
            format: "modern".to_string(),
            color_identity: Some("R".to_string()),
            variant: None,
            now_ms: LATER,
        },
    );
    let before_matches = match_rows(&v1);
    let before_archetypes = archetype_rows(&v1);
    let before_lists = list_rows(&v1, "qdev0001-manual-a");

    let counts = import_all(&path, &mut v1, LATER + 1);

    assert_eq!(match_rows(&v1), before_matches);
    assert_eq!(archetype_rows(&v1), before_archetypes);
    assert!(before_archetypes.contains(&"qdev0001-manual-a|Q Renamed|modern|R|manual".to_string()));
    assert_eq!(list_rows(&v1, "qdev0001-manual-a"), before_lists);
    assert!(
        !match_rows(&v1)
            .iter()
            .any(|row| row.starts_with("q-m2|opponent")),
        "the player's none stays"
    );
    assert_eq!(counts.archetypes_added, 0);
    assert_eq!(counts.match_rows_added, 0);
    assert_eq!(counts.matches_detected, 0);
    assert_eq!(counts.decks_linked, 0);
}

#[test]
fn a_rerun_takes_0x_edits_of_a_manual_archetype_v1_left_alone() {
    let dir = TestDir::new("import-archetypes-rerun-0x-edit");
    let (path, conn) = legacy_db(&dir, SCHEMA);
    add_basics(&conn);
    add_archetypes(&conn);
    let mut v1 = v1_db(&dir);
    import_all(&path, &mut v1, NOW);
    conn.execute(
        "UPDATE archetypes SET name = 'Q Manual B2', updated_at = '2026-02-01 00:00:00'
         WHERE id = 21",
        [],
    )
    .expect("0.x rename");
    let counts = import_all(&path, &mut v1, LATER);
    assert_eq!(counts.archetypes_updated, 1);
    assert!(
        archetype_rows(&v1).contains(&"qdev0001-manual-b|Q Manual B2|modern|B|manual".to_string())
    );
}

#[test]
fn the_import_rerun_keeps_v1_guides_and_dedupes_notes() {
    let dir = TestDir::new("import-archetypes-guides-rerun");
    let path = full_legacy(&dir);
    let mut v1 = v1_db(&dir);
    import_all(&path, &mut v1, NOW);

    // In v1: the Q Tron plan edited, a note added.
    let deck = deck_id(&v1, "1001");
    let tron = id_of(&v1, "q-uuid-tron");
    let guide: i64 = v1
        .query_row(
            "SELECT id FROM sideboard_guides WHERE deck_id = ?1 AND archetype_id = ?2",
            [deck, tron],
            |row| row.get(0),
        )
        .expect("guide");
    let outcome = edit_archetypes(
        &mut v1,
        &ArchetypeEdit::SaveGuideCards {
            deck_id: deck,
            guide_id: guide,
            cards: vec![archetypes::guides::GuideCardInput {
                oracle_id: "q-oracle-2".to_string(),
                direction: "in".to_string(),
                quantity: 1,
            }],
            now_ms: LATER,
        },
    )
    .expect("save plan");
    assert!(
        matches!(outcome, ArchetypeEditOutcome::Done { .. }),
        "{outcome:?}"
    );
    let outcome = edit_archetypes(
        &mut v1,
        &ArchetypeEdit::AddNote {
            deck_id: deck,
            archetype_id: tron,
            body: "Q a v1 note".to_string(),
            now_ms: LATER,
        },
    )
    .expect("add note");
    assert!(
        matches!(outcome, ArchetypeEditOutcome::Note { .. }),
        "{outcome:?}"
    );
    let guides = guide_rows(&v1);
    let notes = note_rows(&v1);
    assert!(guides.contains(&format!(
        "1001|q-uuid-tron|{}|{LATER}|in:q-oracle-2:1",
        ms("2026-01-05 09:00:00")
    )));
    assert_eq!(notes.len(), 4);

    let counts = import_all(&path, &mut v1, LATER + 1);

    assert_eq!(guide_rows(&v1), guides, "v1's plan stays");
    assert_eq!(note_rows(&v1), notes, "no note doubled");
    assert_eq!(counts.guides_added, 0);
    // The two 0.x guides; the noted pair's empty guide is v1's now.
    assert_eq!(counts.guides_kept, 2);
    assert_eq!(counts.guide_cards_added, 0);
    assert_eq!(counts.notes_added, 0);
    assert_eq!(counts.notes_unchanged, 3);
}

#[test]
fn an_old_0x_without_archetype_tables_has_nothing_to_import() {
    let conn = Connection::open_in_memory().expect("memory");
    conn.execute_batch(
        "CREATE TABLE matches (id INTEGER PRIMARY KEY, token TEXT, mtgo_id TEXT);
         CREATE TABLE players (id INTEGER PRIMARY KEY, username TEXT);",
    )
    .expect("old schema");
    let read = read_archetypes(&conn).expect("read");
    assert!(read.archetypes.is_empty());
    assert!(read.match_rows.is_empty());
    assert!(read.deck_links.is_empty());
    assert!(read.guides.is_empty());
    assert!(read.notes.is_empty());
}

// 0.x tests/Feature/Migrations/BackfillManualArchetypeVariantsTest.php, as
// the legacy list rule of the import.

/// A manual archetype whose only cards are in the legacy pivot (4 Q Bolt),
/// and the match 1 opponent row pointing at it.
fn manual_with_legacy_cards(dir: &TestDir) -> (PathBuf, Connection) {
    let (path, conn) = legacy_db(dir, SCHEMA);
    add_basics(&conn);
    add_match(&conn, 1, "complete", 1, 2);
    conn.execute_batch(
        "INSERT INTO archetypes (id, uuid, name, format, manual, is_fallback, created_at,
             updated_at)
         VALUES (30, 'qdev0001-legacy', 'Q Legacy', 'modern', 1, 0, '2026-01-01 00:00:00',
             '2026-01-01 00:00:00');
         INSERT INTO archetype_cards (id, archetype_id, card_id, quantity, sideboard)
         VALUES (1, 30, 1, 4, 0);
         INSERT INTO match_archetypes (id, archetype_id, mtgo_match_id, player_id, confidence)
         VALUES (1, 30, 1, 2, 1);",
    )
    .expect("legacy manual archetype");
    (path, conn)
}

#[test]
fn rebuilds_a_variant_from_the_legacy_pivot() {
    let dir = TestDir::new("import-archetypes-legacy-pivot");
    let (path, _conn) = manual_with_legacy_cards(&dir);
    let mut v1 = v1_db(&dir);
    import_all(&path, &mut v1, NOW);
    assert_eq!(
        list_rows(&v1, "qdev0001-legacy"),
        vec!["1|12345|q-oracle-1|4|0"]
    );
}

#[test]
fn a_match_row_of_a_legacy_list_archetype_points_at_it() {
    // 0.x backfilled `archetype_deck_id`; v1 keeps no sample decks
    // (ruling 9), so the row points at the archetype and its list 1.
    let dir = TestDir::new("import-archetypes-legacy-match");
    let (path, _conn) = manual_with_legacy_cards(&dir);
    let mut v1 = v1_db(&dir);
    import_all(&path, &mut v1, NOW);
    assert_eq!(match_rows(&v1), vec!["q-m1|opponent|qdev0001-legacy|1.0|0"]);
}

#[test]
fn the_legacy_list_import_is_safe_to_run_twice() {
    let dir = TestDir::new("import-archetypes-legacy-twice");
    let (path, _conn) = manual_with_legacy_cards(&dir);
    let mut v1 = v1_db(&dir);
    import_all(&path, &mut v1, NOW);
    import_all(&path, &mut v1, NOW);
    assert_eq!(
        count(
            &v1,
            "SELECT count(DISTINCT list) FROM archetype_cards c JOIN archetypes a
             ON a.id = c.archetype_id WHERE a.uuid = 'qdev0001-legacy'"
        ),
        1
    );
    assert_eq!(list_rows(&v1, "qdev0001-legacy").len(), 1);
}

#[test]
fn leaves_archetypes_that_already_have_variants_alone() {
    let dir = TestDir::new("import-archetypes-legacy-variants");
    let (path, conn) = manual_with_legacy_cards(&dir);
    conn.execute_batch(
        "INSERT INTO archetype_decks (id, uuid, archetype_id) VALUES (100, 'q-v', 30);
         INSERT INTO archetype_deck_cards (id, archetype_deck_id, card_id, quantity, sideboard)
         VALUES (1, 100, 3, 2, 0);",
    )
    .expect("a variant");
    let mut v1 = v1_db(&dir);
    import_all(&path, &mut v1, NOW);
    assert_eq!(
        list_rows(&v1, "qdev0001-legacy"),
        vec!["1|55555|q-oracle-3|2|0"]
    );
}

// 0.x tests/Feature/Migrations/SideboardGuidesBackfillTest.php, as the
// empty guide rule of the import.

/// Decks 1001 and 1002, archetypes Q Blink and Q Tron, and notes on
/// (1001, Blink) twice, (1001, Tron) and (1002, Blink).
fn noted_pairs(dir: &TestDir) -> PathBuf {
    let (path, conn) = legacy_db(dir, SCHEMA);
    add_basics(&conn);
    conn.execute_batch(
        "UPDATE decks SET deleted_at = NULL;
         INSERT INTO archetypes (id, uuid, name, format, created_at, updated_at) VALUES
             (40, 'q-uuid-blink', 'Q Blink', 'modern', '2026-01-01 00:00:00',
              '2026-01-01 00:00:00'),
             (41, 'q-uuid-tron', 'Q Tron', 'modern', '2026-01-01 00:00:00',
              '2026-01-01 00:00:00');
         INSERT INTO deck_archetype_notes (id, deck_id, archetype_id, body, created_at) VALUES
             (1, 1, 40, 'Q one', '2026-01-05 10:00:00'),
             (2, 1, 40, 'Q two', '2026-01-05 10:01:00'),
             (3, 1, 41, 'Q three', '2026-01-05 10:02:00'),
             (4, 2, 40, 'Q four', '2026-01-05 10:03:00');",
    )
    .expect("noted pairs");
    path
}

#[test]
fn creates_one_guide_per_noted_deck_and_archetype_pair() {
    let dir = TestDir::new("import-archetypes-noted-pairs");
    let path = noted_pairs(&dir);
    let mut v1 = v1_db(&dir);
    import_all(&path, &mut v1, NOW);
    assert_eq!(
        guide_rows(&v1),
        vec![
            format!("1001|q-uuid-blink|{NOW}|{NOW}|-"),
            format!("1001|q-uuid-tron|{NOW}|{NOW}|-"),
            format!("1002|q-uuid-blink|{NOW}|{NOW}|-"),
        ]
    );
}

#[test]
fn the_empty_guides_are_safe_to_run_twice() {
    let dir = TestDir::new("import-archetypes-noted-twice");
    let path = noted_pairs(&dir);
    let mut v1 = v1_db(&dir);
    import_all(&path, &mut v1, NOW);
    let counts = import_all(&path, &mut v1, LATER);
    assert_eq!(count(&v1, "SELECT count(*) FROM sideboard_guides"), 3);
    assert_eq!(count(&v1, "SELECT count(*) FROM deck_archetype_notes"), 4);
    assert_eq!(counts.guides_added, 0);
}

/// T14 review C1: detection that ran during the import (between the match
/// step and the archetype step) never makes the import drop 0.x rows: a
/// marker set at or after the import's start is not "v1 decided", and an
/// auto row detection wrote then gives way to 0.x's row.
#[test]
fn detection_during_the_import_never_drops_0x_rows() {
    let dir = TestDir::new("import-archetypes-race");
    let path = full_legacy(&dir);
    let mut v1 = v1_db(&dir);
    let read = read_database(&path).expect("read");
    import_matches(&mut v1, &read.matches, NOW).expect("matches");
    import_decks(&mut v1, &read.decks.expect("decks").data, NOW).expect("decks");
    // Match 3's opponent showed cards (the details step wrote them), so a
    // marker without an opponent row would read as a provable "none".
    let m3 = match_id(&v1, "q-m3");
    v1.execute(
        "INSERT INTO game_cards (game_id, side, catalog_id, seen)
         SELECT id, 'opponent', 12345, 2 FROM games WHERE match_id = ?1",
        [m3],
    )
    .expect("opponent cards");
    // The worker detects while the import runs: match 1's opponent as
    // Homebrew (an auto row), matches 2 and 3 with nothing.
    let homebrew = id_of(&v1, HOMEBREW);
    let empty = |match_id: i64| MatchDetection {
        match_id,
        deck_archetype_id: None,
        deck_archetype: None,
        player: None,
        opponent: None,
    };
    let (m1, m2) = (match_id(&v1, "q-m1"), match_id(&v1, "q-m2"));
    let outcome = edit_archetypes(
        &mut v1,
        &ArchetypeEdit::SetDetections {
            results: vec![
                MatchDetection {
                    opponent: Some(SeatResult {
                        archetype_id: homebrew,
                        confidence: 0.0,
                    }),
                    ..empty(m1)
                },
                empty(m2),
                empty(m3),
            ],
            now_ms: NOW + 1,
        },
    )
    .expect("detections");
    assert!(
        matches!(outcome, ArchetypeEditOutcome::Done { .. }),
        "{outcome:?}"
    );

    let mut archetypes = read_archetypes_at(&path).expect("read archetypes");
    archetypes.import_started_at_ms = Some(NOW);
    let counts = import_archetypes(&mut v1, &archetypes, NOW + 2).expect("archetypes");

    let rows = match_rows(&v1);
    assert!(
        rows.contains(&"q-m1|opponent|q-uuid-tron|0.8|0".to_string()),
        "{rows:?}"
    );
    assert!(
        rows.contains(&format!("q-m2|opponent|{HOMEBREW}|0.0|0")),
        "{rows:?}"
    );
    assert!(
        rows.contains(&"q-m3|opponent|qdev0001-manual-a|1.0|1".to_string()),
        "the 0.x hand assignment lands: {rows:?}"
    );
    assert_eq!(counts.match_rows_kept, 0);
    assert_eq!(counts.match_rows_added, 7);
}

/// The other side of the rule: a marker from before the import (the
/// player's "none" on an earlier run) still stands, and a hand-set v1 row
/// beats a different hand-set 0.x row (review M8).
#[test]
fn a_v1_choice_made_before_the_import_beats_0x_hand_set_rows() {
    let dir = TestDir::new("import-archetypes-v1-choice");
    let path = full_legacy(&dir);
    let mut v1 = v1_db(&dir);
    import_all(&path, &mut v1, NOW);
    let m3 = match_id(&v1, "q-m3");
    v1.execute(
        "INSERT INTO game_cards (game_id, side, catalog_id, seen)
         SELECT id, 'opponent', 12345, 2 FROM games WHERE match_id = ?1",
        [m3],
    )
    .expect("opponent cards");
    let outcome = edit_archetypes(
        &mut v1,
        &ArchetypeEdit::SetOpponent {
            match_ids: vec![m3],
            archetype_id: None,
            now_ms: LATER,
        },
    )
    .expect("none");
    assert!(matches!(outcome, ArchetypeEditOutcome::Done { .. }));
    let manual_c = id_of(&v1, "qdev0001-manual-c");
    let m1 = match_id(&v1, "q-m1");
    let outcome = edit_archetypes(
        &mut v1,
        &ArchetypeEdit::SetOpponent {
            match_ids: vec![m1],
            archetype_id: Some(manual_c),
            now_ms: LATER,
        },
    )
    .expect("by hand");
    assert!(matches!(outcome, ArchetypeEditOutcome::Done { .. }));

    let counts = import_all(&path, &mut v1, LATER + 1);

    let rows = match_rows(&v1);
    assert!(
        !rows.iter().any(|row| row.starts_with("q-m3|opponent")),
        "the provable none stays: {rows:?}"
    );
    assert!(
        rows.contains(&"q-m1|opponent|qdev0001-manual-c|1.0|1".to_string()),
        "{rows:?}"
    );
    assert_eq!(counts.match_rows_added, 0);
}

/// #300 F3: only the recorded "none" (`opponent_archetype_none`) keeps an
/// empty opponent seat from 0.x's hand-set row. A seat emptied another way
/// (the hand-set archetype was deleted, which takes its rows) while the
/// opponent showed cards and the match stays marked is no choice.
#[test]
fn an_emptied_seat_without_the_none_flag_takes_0x_hand_set_row() {
    let dir = TestDir::new("import-archetypes-emptied-seat");
    let path = full_legacy(&dir);
    let mut v1 = v1_db(&dir);
    import_all(&path, &mut v1, NOW);
    let m3 = match_id(&v1, "q-m3");
    v1.execute(
        "INSERT INTO game_cards (game_id, side, catalog_id, seen)
         SELECT id, 'opponent', 12345, 2 FROM games WHERE match_id = ?1",
        [m3],
    )
    .expect("opponent cards");
    v1.execute(
        "DELETE FROM match_archetypes WHERE match_id = ?1 AND is_opponent = 1",
        [m3],
    )
    .expect("the seat emptied");
    assert_eq!(
        count(
            &v1,
            &format!(
                "SELECT count(*) FROM matches WHERE id = {m3} AND archetypes_detected_at IS NOT NULL"
            )
        ),
        1
    );

    import_all(&path, &mut v1, LATER);

    let rows = match_rows(&v1);
    assert!(
        rows.contains(&"q-m3|opponent|qdev0001-manual-a|1.0|1".to_string()),
        "{rows:?}"
    );
}

/// T14 review I1: the server moved Q Tron into Q Burn Arch; a rerun brings
/// the old uuid back with 0.x's notes, and the next move must not double
/// them on the target.
#[test]
fn a_rerun_after_a_server_move_does_not_double_notes() {
    let dir = TestDir::new("import-archetypes-rerun-moved");
    let path = full_legacy(&dir);
    let mut v1 = v1_db(&dir);
    import_all(&path, &mut v1, NOW);
    let moved = || ArchetypeEdit::ApplyDefinitions {
        plan: archetypes::ChangePlan {
            moves: vec![("q-uuid-tron".to_string(), "q-uuid-burn".to_string())],
            ..archetypes::ChangePlan::default()
        },
        checked_at: None,
        now_ms: LATER,
    };
    edit_archetypes(&mut v1, &moved()).expect("move");
    let notes = note_rows(&v1);
    assert_eq!(notes.len(), 3);

    import_all(&path, &mut v1, LATER + 1);
    edit_archetypes(&mut v1, &moved()).expect("move again");

    assert_eq!(note_rows(&v1), notes, "no note doubled");
}

/// Final review I2 (Review Focus 1): the player's "none" is input
/// (`matches.opponent_archetype_none`), so a rerun never refills it, even
/// after a re-detection cleared the marker and whether 0.x's row was
/// detected (match 1) or hand-set (match 3).
#[test]
fn a_rerun_keeps_the_players_none_after_a_redetection() {
    let dir = TestDir::new("import-archetypes-none-flag");
    let path = full_legacy(&dir);
    let mut v1 = v1_db(&dir);
    import_all(&path, &mut v1, NOW);
    let (m1, m3) = (match_id(&v1, "q-m1"), match_id(&v1, "q-m3"));
    for edit in [
        ArchetypeEdit::SetOpponent {
            match_ids: vec![m1, m3],
            archetype_id: None,
            now_ms: LATER,
        },
        // A re-detection (the deck banner, a definitions change) clears
        // the marker the import used to read as the choice.
        ArchetypeEdit::ClearDetected {
            match_ids: vec![m1, m3],
        },
    ] {
        let outcome = edit_archetypes(&mut v1, &edit).expect("edit");
        assert!(matches!(outcome, ArchetypeEditOutcome::Done { .. }));
    }

    import_all(&path, &mut v1, LATER + 1);

    let rows = match_rows(&v1);
    assert!(
        !rows
            .iter()
            .any(|row| row.starts_with("q-m1|opponent") || row.starts_with("q-m3|opponent")),
        "the player's none stays: {rows:?}"
    );
}
