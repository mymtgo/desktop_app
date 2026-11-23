//! Limited import tests (limited spec section 7, import audit section 5.5):
//! a 0.x database built from `fixtures/0x/schema.sql` with a draft league
//! (draft, picks with notes, snapshots) and a sealed league (no snapshots,
//! only its limited deck version), read with `read_database` and written in
//! the app's order: matches, decks, leagues, then Limited. Fake names start
//! with Q.

mod common;

use std::path::Path;

use common::{count, legacy_db, texts, v1_db};
use decks::DeckCard;
use fixture_tools::TestDir;
use importer::{LimitedRead, read_database};
use limited::{DraftState, RegisteredCard, Reservation};
use rusqlite::{Connection, params};
use store::import::import_matches;
use store::import_decks::import_decks;
use store::import_leagues::import_leagues;
use store::import_limited::{LimitedImportCounts, import_limited};

const SCHEMA: &str = include_str!("../../../fixtures/0x/schema.sql");
const NOW: i64 = 1_800_000_000_000;
const DRAFT: &str = "q-draft-token";

/// The tokens of the matches linked to the imported run of 0.x league `id`.
fn members(id: i64) -> String {
    format!(
        "SELECT m.token FROM matches m JOIN leagues l ON l.id = m.league_id
         WHERE l.imported_from = {id} AND m.league_linked = 1 ORDER BY m.token"
    )
}

/// Every row of `table` as debug text, in rowid order.
fn dump(conn: &Connection, table: &str) -> Vec<String> {
    let mut stmt = conn
        .prepare(&format!("SELECT * FROM {table} ORDER BY rowid"))
        .expect("prepare");
    let columns = stmt.column_count();
    stmt.query_map([], |row| {
        let mut values = Vec::new();
        for index in 0..columns {
            let value: rusqlite::types::Value = row.get(index)?;
            values.push(format!("{value:?}"));
        }
        Ok(values.join("|"))
    })
    .expect("query")
    .map(|row| row.expect("row"))
    .collect()
}

fn signature(cards: &[(u32, u32, bool)]) -> String {
    let cards: Vec<DeckCard> = cards
        .iter()
        .map(|(catalog_id, quantity, sideboard)| DeckCard {
            catalog_id: *catalog_id,
            quantity: *quantity,
            sideboard: *sideboard,
        })
        .collect();
    decks::signature(&cards)
}

/// A complete 0.x match `id` in league `league_id` with deck version
/// `version`, one won game between Qlocal and Qopponent.
fn add_match(conn: &Connection, id: i64, token: &str, format: &str, league_id: i64, version: i64) {
    conn.execute(
        "INSERT INTO matches (id, token, mtgo_id, deck_version_id, format, match_type, games_won,
             games_lost, started_at, ended_at, state, league_id)
         VALUES (?1, ?2, ?3, ?4, ?5, 'Limited', 2, 0, ?6, ?6, 'complete', ?7)",
        params![
            id,
            token,
            (289_000_000 + id).to_string(),
            version,
            format,
            format!("2026-08-22 12:0{id}:00"),
            league_id
        ],
    )
    .expect("match");
    conn.execute(
        "INSERT INTO games (id, match_id, mtgo_id, won, started_at) VALUES (?1, ?2, ?3, 1, ?4)",
        params![id * 10, id, (id * 10).to_string(), "2026-08-22 12:00:00"],
    )
    .expect("game");
    conn.execute(
        "INSERT INTO game_player (game_id, player_id, instance_id, is_local)
         VALUES (?1, 1, 1, 1), (?1, 2, 1, 0)",
        [id * 10],
    )
    .expect("seats");
}

/// Draft league 29 (three matches, a draft, two snapshots: one with its
/// token, one only with its match id; the third match has none) and sealed
/// league 34 (one match, no snapshot, only its limited deck version).
fn add_limited(conn: &Connection) {
    conn.execute_batch(
        "INSERT INTO players (id, username) VALUES (1, 'Qlocal'), (2, 'Qopponent');
         INSERT INTO accounts (id, username) VALUES (1, 'Qlocal');
         INSERT INTO leagues (id, token, name, format, started_at, state, event_id, joined_at,
             completed_at, kind, set_code, mtgo_course_id)
         VALUES (29, 'q-draft-league', 'Q Draft League', 'Limited', '2026-08-22 11:00:00',
                 'complete', 11_195, '2026-08-22 11:00:00', '2026-08-22 13:00:00', 'draft',
                 'HOB', 35),
                (34, 'q-sealed-league', 'Q Sealed League', 'S6FRA', '2026-10-01 14:00:00',
                 'active', 11_196, NULL, NULL, 'sealed', ' FRA ', NULL);
         INSERT INTO decks (id, mtgo_id, name, format, account_id)
         VALUES (26, 'limited:q-draft-token', 'Q Draft Deck', 'Limited', 1),
                (49, 'limited:league-34', 'Q Sealed Deck', 'Limited', 1);",
    )
    .expect("leagues and decks");
    conn.execute(
        "INSERT INTO deck_versions (id, deck_id, signature, modified_at)
         VALUES (52, 26, ?1, '2026-08-22 13:00:00'), (93, 49, ?2, '2026-10-01 14:48:35')",
        params![
            signature(&[(153_892, 9, false), (153_940, 1, false)]),
            signature(&[(155_526, 7, false), (155_530, 1, true)])
        ],
    )
    .expect("limited versions");
    add_match(conn, 1, "q-tok-1", "DHOBHOBHOB", 29, 52);
    add_match(conn, 2, "q-tok-2", "DHOBHOBHOB", 29, 52);
    add_match(conn, 3, "q-tok-3", "DHOBHOBHOB", 29, 52);
    add_match(conn, 4, "q-tok-4", "S6FRA", 34, 93);
    conn.execute_batch(
        r#"INSERT INTO limited_deck_snapshots (id, league_id, match_id, game_id, source, cards,
               signature, captured_at, match_token)
           VALUES (7, 29, 1, NULL, 'registered',
                   '[{"catalog_id":153896,"quantity":1,"sideboard":true},{"catalog_id":"153940","quantity":"2","sideboard":false}]',
                   'unused', '2026-08-22T12:01:00Z', 'q-tok-1'),
                  (8, 29, 2, NULL, 'registered',
                   '[{"catalog_id":153896,"quantity":3,"sideboard":false}]',
                   'unused', '2026-08-22 12:02:00', NULL),
                  (9, 29, NULL, NULL, 'opponent', '[]', '', '2026-08-22 12:02:00', 'q-tok-9');
           INSERT INTO drafts (id, league_id, tournament_id, draft_token, mtgo_draft_id,
               pod_token, seat_count, seat_index, booster_catalog_id, state, pack_size,
               picks_expected, started_at, ended_at)
           VALUES (3, 29, NULL, 'q-draft-token', 35746768, 'q-pod', 8, 7, 154732, 'finished',
                   14, 42, '2026-08-22T11:00:12Z', '2026-08-22T13:00:00Z'),
                  (4, NULL, NULL, '  ', NULL, NULL, 8, NULL, NULL, 'finished', 14, 42, NULL,
                   NULL),
                  (5, NULL, NULL, 'q-odd-state', NULL, NULL, 8, NULL, NULL, 'exploded', 14, 42,
                   NULL, NULL);
           INSERT INTO draft_picks (id, draft_id, ordinal, pack_number, pick_number, pack_id,
               direction, cards_available, picked_catalog_id, picked_card_id, reservations,
               shown_at, deadline_at, picked_at, note)
           VALUES (1, 3, 1, 1, 1, 9001, 0, '[153998,"153940",154118]', 153940, 5000000001,
                   '[{"catalog_id":153998,"at":"2026-08-22T11:12:05+00:00"},{"catalog_id":1}]',
                   '2026-08-22T11:12:00Z', '2026-08-22T11:13:09Z', '2026-08-22T11:12:26Z',
                   '  Q bomb first  '),
                  (2, 3, 2, 1, 2, 9002, 0, '[154244]', 154244, NULL, '[]',
                   '2026-08-22T11:12:27Z', NULL, NULL, NULL),
                  (3, 3, 0, 1, 0, NULL, NULL, 'not json', NULL, NULL, 'not json', NULL, NULL,
                   NULL, 'Q lost');"#,
    )
    .expect("draft, picks and snapshots");
}

/// Read the 0.x database and import it into `v1` the way the app does:
/// matches, decks, leagues, then Limited.
fn import_all(path: &Path, v1: &mut Connection, now_ms: i64) -> (LimitedRead, LimitedImportCounts) {
    let read = read_database(path).expect("read");
    import_matches(v1, &read.matches, now_ms).expect("matches");
    if let Some(deck_read) = &read.decks {
        import_decks(v1, &deck_read.data, now_ms).expect("decks");
    }
    let leagues = read.leagues.expect("leagues");
    import_leagues(v1, &leagues.leagues, now_ms).expect("leagues");
    let limited = read.limited.expect("the 0.x database has Limited tables");
    let counts = import_limited(v1, &limited.data, now_ms).expect("limited");
    (limited, counts)
}

fn card(catalog_id: u32, quantity: u32, sideboard: bool) -> RegisteredCard {
    RegisteredCard {
        catalog_id,
        quantity,
        sideboard,
    }
}

#[test]
fn drafts_picks_notes_limited_leagues_and_registered_decks_import() {
    let dir = TestDir::new("import-limited-all");
    let (path, conn) = legacy_db(&dir, SCHEMA);
    add_limited(&conn);
    let mut v1 = v1_db(&dir);
    let (read, counts) = import_all(&path, &mut v1, NOW);
    assert_eq!(
        (read.drafts_skipped, read.decks_skipped),
        (2, 1),
        "a blank token and an unknown state; an opponent snapshot"
    );
    assert_eq!(
        counts,
        LimitedImportCounts {
            drafts_added: 1,
            notes_added: 1,
            decks_added: 4,
            ..LimitedImportCounts::default()
        }
    );

    // The limited leagues are runs with their kind, set code and course id.
    assert_eq!(
        texts(
            &v1,
            "SELECT imported_from || ':' || kind || ':' || COALESCE(set_code, '-') || ':'
                 || COALESCE(mtgo_course_id, '-')
             FROM leagues WHERE source = 'imported' ORDER BY imported_from"
        ),
        vec!["29:draft:HOB:35", "34:sealed:FRA:-"]
    );
    assert_eq!(
        count(&v1, "SELECT COUNT(*) FROM leagues WHERE source = 'manual'"),
        0
    );
    assert_eq!(
        texts(&v1, &members(29)),
        vec!["q-tok-1", "q-tok-2", "q-tok-3"]
    );
    // No deck rows for limited (ruling 3).
    assert_eq!(count(&v1, "SELECT COUNT(*) FROM decks"), 0);

    // The draft, its picks and notes round-trip.
    let local = count(&v1, "SELECT id FROM accounts WHERE username = 'Qlocal'");
    assert_eq!(
        texts(
            &v1,
            "SELECT draft_token || ':' || source || ':' || imported_from || ':'
                 || imported_league_from || ':' || account_id FROM drafts"
        ),
        vec![format!("{DRAFT}:imported:3:29:{local}")]
    );
    let draft = store::limited::stored_draft(&v1, DRAFT)
        .expect("read")
        .expect("the draft");
    assert_eq!(draft.token, DRAFT);
    assert_eq!(draft.state, DraftState::Finished);
    assert_eq!(draft.league_event_id, Some(11_195));
    assert_eq!(draft.course_id, Some(35));
    assert_eq!(draft.draft_id, Some(35_746_768));
    assert_eq!(draft.seat_index, Some(7));
    assert_eq!(draft.booster_catalog_id, Some(154_732));
    assert_eq!(draft.started_at_ms, Some(1_787_396_412_000));
    let ordinals: Vec<u32> = draft.picks.iter().map(|pick| pick.ordinal).collect();
    assert_eq!(ordinals, vec![1, 2], "the ordinal 0 row is left out");
    let first = &draft.picks[0];
    assert_eq!(first.cards_available, vec![153_998, 153_940, 154_118]);
    assert_eq!(first.picked_catalog_id, Some(153_940));
    assert_eq!(first.picked_card_id, Some(5_000_000_001));
    assert_eq!(first.pack_id, Some(9_001));
    assert_eq!(
        first.reservations,
        vec![Reservation {
            catalog_id: 153_998,
            at_ms: 1_787_397_125_000
        }],
        "an entry without a time is left out"
    );
    assert_eq!(first.shown_at_ms, Some(1_787_397_120_000));
    assert_eq!(first.picked_at_ms, Some(1_787_397_146_000));
    assert_eq!(draft.picks[1].deadline_at_ms, None);
    assert_eq!(
        store::limited::pick_notes(&v1, DRAFT).expect("notes"),
        std::collections::BTreeMap::from([(1, "Q bomb first".to_string())])
    );

    // Registered decks: the snapshots, then the version for the matches
    // without one.
    let run: Vec<limited::RegisteredDeck> = ["q-tok-1", "q-tok-2", "q-tok-3"]
        .iter()
        .map(|token| {
            store::limited::stored_registered_deck(&v1, token)
                .expect("read")
                .expect("a deck")
        })
        .collect();
    let by_token: Vec<(Option<&str>, &Vec<RegisteredCard>)> = run
        .iter()
        .map(|deck| (deck.match_token.as_deref(), &deck.cards))
        .collect();
    assert_eq!(
        by_token,
        vec![
            (
                Some("q-tok-1"),
                &vec![card(153_896, 1, true), card(153_940, 2, false)]
            ),
            (Some("q-tok-2"), &vec![card(153_896, 3, false)]),
            (
                Some("q-tok-3"),
                &vec![card(153_892, 9, false), card(153_940, 1, false)]
            ),
        ]
    );
    assert_eq!(run[0].captured_at_ms, 1_787_400_060_000);
    let sealed = store::limited::stored_registered_deck(&v1, "q-tok-4")
        .expect("read")
        .expect("the sealed match's version");
    assert_eq!(
        sealed.cards,
        vec![card(155_526, 7, false), card(155_530, 1, true)]
    );
    assert_eq!(
        sealed.captured_at_ms, 1_787_400_240_000,
        "the match's start"
    );
}

#[test]
fn a_rerun_of_the_import_adds_nothing() {
    let dir = TestDir::new("import-limited-rerun");
    let (path, conn) = legacy_db(&dir, SCHEMA);
    add_limited(&conn);
    let mut v1 = v1_db(&dir);
    import_all(&path, &mut v1, NOW);
    let tables = ["leagues", "drafts", "draft_picks", "limited_deck_snapshots"];
    let before: Vec<Vec<String>> = tables.iter().map(|table| dump(&v1, table)).collect();
    let members_before = texts(&v1, &members(29));
    let (_, counts) = import_all(&path, &mut v1, NOW + 60_000);
    assert_eq!(
        counts,
        LimitedImportCounts {
            drafts_unchanged: 1,
            decks_unchanged: 4,
            ..LimitedImportCounts::default()
        }
    );
    let after: Vec<Vec<String>> = tables.iter().map(|table| dump(&v1, table)).collect();
    assert_eq!(after, before, "an unchanged rerun writes nothing");
    assert_eq!(texts(&v1, &members(29)), members_before);
}

#[test]
fn an_old_0x_without_limited_tables_has_nothing_to_import() {
    let dir = TestDir::new("import-limited-old");
    let path = dir.join("nativephp.sqlite");
    let conn = Connection::open(&path).expect("open");
    let schema: String = SCHEMA
        .lines()
        .filter(|line| {
            !line.contains("\"drafts\"")
                && !line.contains("\"draft_picks\"")
                && !line.contains("\"limited_deck_snapshots\"")
        })
        .collect::<Vec<_>>()
        .join("\n");
    conn.execute_batch(&schema).expect("old schema");
    let read = read_database(&path).expect("read");
    assert_eq!(read.limited, None);
}

/// A second, different 0.x database (issues #194 and #143): its draft
/// league 29 is another run (token `q-b-league`), its draft 3 another draft
/// (`q-b-draft`), and its draft 4 the first database's `q-draft-token` seen
/// again, abandoned there.
fn add_second_limited(conn: &Connection) {
    conn.execute_batch(
        "INSERT INTO players (id, username) VALUES (1, 'Qlocal'), (2, 'Qopponent');
         INSERT INTO accounts (id, username) VALUES (1, 'Qlocal');
         INSERT INTO leagues (id, token, name, format, started_at, state, event_id, joined_at,
             completed_at, kind, set_code, mtgo_course_id)
         VALUES (29, 'q-b-league', 'Q B League', 'Limited', '2026-09-05 09:00:00',
                 'active', 12_001, '2026-09-05 09:00:00', NULL, 'draft', 'HOB', 36);
         INSERT INTO matches (id, token, mtgo_id, format, match_type, games_won, games_lost,
             started_at, ended_at, state, league_id)
         VALUES (1, 'q-b-1', '290000001', 'DHOBHOBHOB', 'Limited', 2, 0,
                 '2026-09-05 10:00:00', '2026-09-05 10:30:00', 'complete', 29);
         INSERT INTO games (id, match_id, mtgo_id, won, started_at)
         VALUES (10, 1, '9010', 1, '2026-09-05 10:00:00');
         INSERT INTO game_player (game_id, player_id, instance_id, is_local)
         VALUES (10, 1, 1, 1), (10, 2, 1, 0);",
    )
    .expect("second league and match");
    conn.execute_batch(
        r#"INSERT INTO limited_deck_snapshots (id, league_id, match_id, game_id, source, cards,
               signature, captured_at, match_token)
           VALUES (7, 29, 1, NULL, 'registered',
                   '[{"catalog_id":153900,"quantity":2,"sideboard":false}]',
                   'unused', '2026-09-05T10:00:00Z', 'q-b-1');
           INSERT INTO drafts (id, league_id, tournament_id, draft_token, mtgo_draft_id,
               pod_token, seat_count, seat_index, booster_catalog_id, state, pack_size,
               picks_expected, started_at, ended_at)
           VALUES (3, 29, NULL, 'q-b-draft', 35800000, 'q-b-pod', 8, 2, 154732, 'finished',
                   14, 42, '2026-09-05T09:00:12Z', '2026-09-05T09:40:00Z'),
                  (4, NULL, NULL, 'q-draft-token', 35746768, 'q-pod', 8, 7, 154732,
                   'abandoned', 14, 42, '2026-08-22T11:00:12Z', NULL);
           INSERT INTO draft_picks (id, draft_id, ordinal, pack_number, pick_number, pack_id,
               direction, cards_available, picked_catalog_id, picked_card_id, reservations,
               shown_at, deadline_at, picked_at, note)
           VALUES (1, 3, 1, 1, 1, 9101, 0, '[153900]', 153900, NULL, '[]',
                   '2026-09-05T09:12:00Z', NULL, '2026-09-05T09:12:20Z', 'Q b note'),
                  (2, 4, 1, 1, 1, 9001, 0, '[153998]', 153998, NULL, '[]',
                   '2026-08-22T11:12:00Z', NULL, NULL, 'Q second note');"#,
    )
    .expect("second drafts");
}

/// `(draft token, imported_from, imported_league_from, state)` of every draft.
fn draft_rows(v1: &Connection) -> Vec<String> {
    texts(
        v1,
        "SELECT draft_token || ':' || imported_from || ':'
             || COALESCE(imported_league_from, '-') || ':' || state
         FROM drafts ORDER BY id",
    )
}

#[test]
fn a_second_0x_database_never_overwrites_the_first_ones_runs_and_drafts() {
    let dir = TestDir::new("import-limited-second-db");
    let (first_path, first) = legacy_db(&dir, SCHEMA);
    add_limited(&first);
    let second_dir = TestDir::new("import-limited-second-db-b");
    let (second_path, second) = legacy_db(&second_dir, SCHEMA);
    add_second_limited(&second);
    let mut v1 = v1_db(&dir);
    import_all(&first_path, &mut v1, NOW);
    let first_run = texts(
        &v1,
        "SELECT run_key || ':' || token || ':' || imported_seed FROM leagues
         WHERE imported_from = 29",
    );
    let first_draft = texts(
        &v1,
        "SELECT p.ordinal || ':' || p.cards_available || ':' || COALESCE(p.note, '-')
         FROM draft_picks p JOIN drafts d ON d.id = p.draft_id
         WHERE d.draft_token = 'q-draft-token' ORDER BY p.ordinal",
    );

    let (_, counts) = import_all(&second_path, &mut v1, NOW + 1_000);

    assert_eq!(
        texts(
            &v1,
            "SELECT run_key || ':' || token || ':' || imported_seed FROM leagues
             WHERE imported_from = 29",
        ),
        first_run,
        "the first database's run 29 is untouched"
    );
    assert_eq!(
        texts(&v1, &members(29)),
        vec!["q-tok-1", "q-tok-2", "q-tok-3"]
    );
    let second_run: i64 = count(
        &v1,
        "SELECT imported_from FROM leagues WHERE token = 'q-b-league'",
    );
    assert!(
        ![29, 34].contains(&second_run),
        "the second database's league 29 gets an id of its own: {second_run}"
    );
    assert_eq!(texts(&v1, &members(second_run)), vec!["q-b-1"]);
    assert_eq!(
        draft_rows(&v1)[0],
        "q-draft-token:3:29:finished",
        "the first database's draft keeps its row"
    );
    let second_draft = texts(
        &v1,
        "SELECT imported_from || ':' || imported_league_from FROM drafts
         WHERE draft_token = 'q-b-draft'",
    );
    assert_eq!(second_draft.len(), 1);
    assert_ne!(second_draft[0], "3:29");
    assert!(second_draft[0].ends_with(&format!(":{second_run}")));
    assert_eq!(
        texts(
            &v1,
            "SELECT p.ordinal || ':' || p.cards_available || ':' || COALESCE(p.note, '-')
             FROM draft_picks p JOIN drafts d ON d.id = p.draft_id
             WHERE d.draft_token = 'q-draft-token' ORDER BY p.ordinal",
        ),
        first_draft,
        "the shared draft keeps the first database's picks and notes"
    );
    assert_eq!(
        store::limited::stored_registered_deck(&v1, "q-b-1")
            .expect("read")
            .expect("the second database's registered deck")
            .cards,
        vec![card(153_900, 2, false)]
    );
    assert_eq!(counts.drafts_added, 1);

    // Each database imported again changes nothing.
    let tables = [
        "leagues",
        "matches",
        "drafts",
        "draft_picks",
        "limited_deck_snapshots",
    ];
    let before: Vec<Vec<String>> = tables.iter().map(|table| dump(&v1, table)).collect();
    import_all(&second_path, &mut v1, NOW + 2_000);
    import_all(&first_path, &mut v1, NOW + 3_000);
    let after: Vec<Vec<String>> = tables.iter().map(|table| dump(&v1, table)).collect();
    assert_eq!(after, before, "reruns of both databases write nothing");
}

// Review C1: 0.x heals a draft league's placeholder token at its first
// match (`HealPlaceholderLeague`). A rerun after the heal finds the same
// run: one row, its members and its draft kept.
#[test]
fn a_rerun_after_0x_heals_a_placeholder_token_keeps_one_run() {
    let dir = TestDir::new("import-limited-token-heal");
    let (path, conn) = legacy_db(&dir, SCHEMA);
    add_limited(&conn);
    conn.execute(
        "UPDATE leagues SET token = 'draft-q-placeholder' WHERE id = 29",
        [],
    )
    .expect("placeholder");
    let mut v1 = v1_db(&dir);
    import_all(&path, &mut v1, NOW);
    let row = count(&v1, "SELECT id FROM leagues WHERE imported_from = 29");

    conn.execute(
        "UPDATE leagues SET token = 'q-draft-league' WHERE id = 29",
        [],
    )
    .expect("healed");
    import_all(&path, &mut v1, NOW + 1_000);

    assert_eq!(
        count(
            &v1,
            "SELECT COUNT(*) FROM leagues WHERE source = 'imported'"
        ),
        2
    );
    assert_eq!(
        texts(
            &v1,
            "SELECT id || ':' || json_extract(imported_seed, '$.token') FROM leagues
             WHERE imported_from = 29"
        ),
        vec![format!("{row}:q-draft-league")]
    );
    assert_eq!(
        texts(&v1, &members(29)),
        vec!["q-tok-1", "q-tok-2", "q-tok-3"]
    );
    assert_eq!(draft_rows(&v1), vec!["q-draft-token:3:29:finished"]);
}

/// Issue #337: a draft league whose matches are all of a player that is not
/// one of 0.x's accounts is left out with its draft and registered decks;
/// the account's own Limited data stays, and its limited match with no seat
/// takes the account of its synthesised deck (0.x's `limited:` deck).
#[test]
fn a_league_of_another_player_is_left_out_with_its_draft_and_decks() {
    let dir = TestDir::new("importer-limited-not-account");
    let (path, conn) = legacy_db(&dir, SCHEMA);
    add_limited(&conn);
    conn.execute_batch(
        "INSERT INTO players (id, username) VALUES (3, 'Qstranger');
         INSERT INTO leagues (id, token, name, format, started_at, state, event_id, kind,
             set_code, mtgo_course_id)
         VALUES (40, 'q-stranger-league', 'Q Stranger League', 'Limited',
                 '2026-08-23 11:00:00', 'complete', 11_197, 'draft', 'HOB', 36);
         INSERT INTO decks (id, mtgo_id, name, format, account_id)
         VALUES (60, 'limited:q-stranger-draft', 'Q Stranger Deck', 'Limited', NULL);
         INSERT INTO drafts (id, league_id, tournament_id, draft_token, seat_count, state,
             pack_size, picks_expected)
         VALUES (6, 40, NULL, 'q-stranger-draft', 8, 'finished', 14, 42);",
    )
    .expect("another player's league");
    conn.execute(
        "INSERT INTO deck_versions (id, deck_id, signature, modified_at)
         VALUES (61, 60, ?1, '2026-08-23 13:00:00')",
        [signature(&[(153_892, 9, false)])],
    )
    .expect("version");
    add_match(&conn, 5, "q-tok-5", "DHOBHOBHOB", 40, 61);
    conn.execute_batch(
        r#"UPDATE game_player SET player_id = 3 WHERE game_id = 50 AND is_local = 1;
           INSERT INTO limited_deck_snapshots (id, league_id, match_id, game_id, source, cards,
               signature, captured_at, match_token)
           VALUES (10, 40, 5, NULL, 'registered',
                   '[{"catalog_id":153896,"quantity":3,"sideboard":false}]',
                   'unused', '2026-08-23 12:05:00', 'q-tok-5');"#,
    )
    .expect("its seat and registered deck");
    // A match of Qlocal's draft league without game rows: no seat at all.
    conn.execute(
        "INSERT INTO matches (id, token, mtgo_id, deck_version_id, format, match_type,
             games_won, games_lost, started_at, ended_at, state, league_id)
         VALUES (6, 'q-tok-6', '289000006', 52, 'DHOBHOBHOB', 'Limited', 2, 1,
                 '2026-08-22 12:06:00', '2026-08-22 12:06:00', 'complete', 29)",
        [],
    )
    .expect("a seatless limited match");

    let read = read_database(&path).expect("read");
    assert_eq!(read.skipped.not_account, 1);
    let seatless = read
        .matches
        .iter()
        .find(|found| found.token == "q-tok-6")
        .expect("the seatless match is kept");
    assert_eq!(seatless.local_username, "Qlocal");
    let leagues = read.leagues.as_ref().expect("leagues");
    assert_eq!(leagues.skipped_not_account, 1);
    let league_ids: Vec<i64> = leagues
        .leagues
        .iter()
        .map(|league| league.legacy_id)
        .collect();
    assert_eq!(league_ids, vec![29, 34]);
    let limited = read.limited.as_ref().expect("limited");
    assert_eq!(limited.drafts_not_account, 1);
    assert!(
        limited
            .data
            .decks
            .iter()
            .all(|deck| deck.match_token != "q-tok-5" && deck.league_id != 40),
        "{:?}",
        limited.data.decks
    );

    let mut v1 = v1_db(&dir);
    let (_, counts) = import_all(&path, &mut v1, NOW);
    assert_eq!(counts.drafts_added, 1, "{counts:?}");
    assert_eq!(
        texts(&v1, "SELECT draft_token FROM drafts ORDER BY id"),
        vec![DRAFT]
    );
    assert_eq!(
        texts(
            &v1,
            "SELECT token FROM leagues WHERE source = 'imported' ORDER BY token"
        ),
        vec!["q-draft-league", "q-sealed-league"]
    );
    assert_eq!(
        texts(&v1, "SELECT username FROM accounts ORDER BY username"),
        vec!["Qlocal"]
    );
    assert_eq!(
        texts(
            &v1,
            "SELECT m.token FROM matches m JOIN accounts a ON a.id = m.account_id
             WHERE a.username = 'Qlocal' ORDER BY m.token"
        ),
        vec!["q-tok-1", "q-tok-2", "q-tok-3", "q-tok-4", "q-tok-6"]
    );
    assert_eq!(
        count(&v1, "SELECT COUNT(*) FROM matches WHERE token = 'q-tok-5'"),
        0
    );
}
