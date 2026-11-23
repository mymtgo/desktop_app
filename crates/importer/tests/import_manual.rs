//! Manual match import tests (Plan L): a match the player entered by hand in
//! 0.x (`matches.manual = 1`) comes into v1 as a manual match, with each
//! game's hand, sideboarding and the opponent's revealed cards. 0.x databases
//! are built from `fixtures/0x/schema.sql` plus the 0.x `leagues` table and
//! fake rows, read with `read_database`, and written in the app's order:
//! matches, decks, then leagues. Fake names start with Q.

mod common;

use std::path::Path;

use common::{count, legacy_db, text, v1_db};
use decks::DeckCard;
use decks::stats::Range;
use fixture_tools::TestDir;
use importer::read_database;
use rusqlite::{Connection, params};
use store::import::{ImportCounts, ImportedMatch, import_matches};
use store::import_decks::import_decks;
use store::import_leagues::import_leagues;
use store::manual::{
    CardCount, ManualEdit, ManualEditOutcome, ManualGameDetail, edit_manual, manual_game_rows,
};
use store::match_list::{KindFilter, MatchListFilter, Scope, match_list};

const SCHEMA: &str = include_str!("../../../fixtures/0x/schema.sql");

/// The 0.x `leagues` table as the current 0.x creates it.
const LEAGUES_TABLE: &str = r#"CREATE TABLE IF NOT EXISTS "leagues" ("id" integer primary key autoincrement not null, "token" varchar not null, "name" varchar not null, "format" varchar not null, "started_at" datetime not null, "created_at" datetime, "updated_at" datetime, "deck_change_detected" tinyint(1) not null default ('0'), "deleted_at" datetime, "state" varchar not null default ('active'), "deck_version_id" integer, "event_id" integer, "joined_at" datetime, "dropped_at" datetime, "notes" text, "completed_at" datetime, "manual" tinyint(1) not null default '0', "kind" varchar not null default 'constructed', "set_code" varchar, "mtgo_course_id" integer, "synced_hash" varchar, "synced_at" datetime);"#;

/// The `matches.manual` column and its index, which older 0.x lack.
const MANUAL_COLUMN: &str = r#""manual" tinyint(1) not null default '0', "#;
const MANUAL_INDEX: &str = r#"CREATE INDEX "matches_manual_index" on "matches" ("manual");"#;
/// `game_player` columns older 0.x lack.
const OPENING_HAND_COLUMN: &str = r#", "opening_hand_json" text"#;
const DECK_JSON_COLUMN: &str = r#""deck_json" text, "#;

const NOW: i64 = 1_800_000_000_000;

/// A manual match (0.x `CreateManualMatch` gave it a UUID token).
const MANUAL: &str = "0e0e0e0e-1111-4111-8111-000000000001";
/// A second manual match.
const MANUAL_2: &str = "0e0e0e0e-1111-4111-8111-000000000002";
/// A match 0.x tracked from the log.
const TRACKED: &str = "0e0e0e0e-1111-4111-8111-000000000003";

/// The registered deck (0.x version 10): 4 of 4001, 4 of 4002 and 20 of
/// 4004 main, 3 of 4003 in the sideboard.
fn registered() -> String {
    let card = |catalog_id, quantity, sideboard| DeckCard {
        catalog_id,
        quantity,
        sideboard,
    };
    decks::signature(&[
        card(4001, 4, false),
        card(4002, 4, false),
        card(4004, 20, false),
        card(4003, 3, true),
    ])
}

/// The registered deck as 0.x wrote a game's `deck_json`.
const REGISTERED_JSON: &str = r#"[{"mtgo_id":4001,"quantity":4,"sideboard":false},{"mtgo_id":4002,"quantity":4,"sideboard":false},{"mtgo_id":4004,"quantity":20,"sideboard":false},{"mtgo_id":4003,"quantity":3,"sideboard":true}]"#;

/// One 4002 out, one 4003 in, as 0.x `UpdateManualGameSideboard` wrote it.
const SIDEBOARDED_JSON: &str = r#"[{"mtgo_id":4001,"quantity":4,"sideboard":false},{"mtgo_id":4002,"quantity":3,"sideboard":false},{"mtgo_id":4004,"quantity":20,"sideboard":false},{"mtgo_id":4003,"quantity":1,"sideboard":false},{"mtgo_id":4002,"quantity":1,"sideboard":true},{"mtgo_id":4003,"quantity":2,"sideboard":true}]"#;

/// Kept six after one mulligan, as 0.x `UpdateManualGameHand` wrote it.
const HAND_JSON: &str = r#"{"kept":[4001,4001,4002,4004,4004,4004],"bottomed":[4002],"mulligans":[[4001,4001,4001,4002,4002,4004,4004]]}"#;

/// A 0.x database at `<dir>/nativephp.sqlite` built from `schema`.
fn full_schema() -> String {
    format!("{SCHEMA}\n{LEAGUES_TABLE}")
}

/// The 0.x schema without these pieces (each must be in it), plus leagues.
fn schema_without(pieces: &[&str]) -> String {
    let mut schema = SCHEMA.to_string();
    for piece in pieces {
        assert!(schema.contains(piece), "{piece} is in the schema");
        schema = schema.replace(piece, "");
    }
    format!("{schema}\n{LEAGUES_TABLE}")
}

/// Every row of `table`, every column, as text, in rowid order.
fn dump(conn: &Connection, table: &str) -> Vec<Vec<String>> {
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
        Ok(values)
    })
    .expect("query")
    .map(|row| row.expect("row"))
    .collect()
}

/// Players, the account, the four cards, and deck 1 (`1001`) with version 10.
fn add_basics(conn: &Connection) {
    conn.execute_batch(
        "INSERT INTO players (id, username) VALUES (1, 'Qlocal'), (2, 'Qopponent');
         INSERT INTO accounts (id, username) VALUES (1, 'Qlocal');
         INSERT INTO cards (id, mtgo_id, name)
         VALUES (1, '4001', 'Q Bolt'), (2, '4002', 'Q Goyf'), (3, '4003', 'Q Peace'),
                (4, '4004', 'Q Mountain');
         INSERT INTO decks (id, mtgo_id, name, format, account_id)
         VALUES (1, '1001', 'Q Burn', 'CMODERN', 1);",
    )
    .expect("basics");
    conn.execute(
        "INSERT INTO deck_versions (id, deck_id, signature, modified_at)
         VALUES (10, 1, ?1, '2026-01-01 00:00:00')",
        [registered()],
    )
    .expect("version");
}

/// A complete 2-1 match `id` with deck version 10, games `id * 10 + 1..=3`
/// ten minutes apart, Qlocal against Qopponent. `manual` sets
/// `matches.manual` when the column exists.
fn add_match(conn: &Connection, id: i64, token: &str, manual: bool, league_id: Option<i64>) {
    let mtgo_id = if manual {
        format!("manual_Q{id:023}")
    } else {
        (500 + id).to_string()
    };
    conn.execute(
        "INSERT INTO matches (id, token, mtgo_id, deck_version_id, format, match_type, games_won,
             games_lost, started_at, ended_at, state, outcome, league_id, notes)
         VALUES (?1, ?2, ?3, 10, 'CMODERN', 'Constructed', 2, 1, '2026-01-02 03:00:00',
             '2026-01-02 03:30:00', 'complete', 'win', ?4, ?5)",
        params![
            id,
            token,
            mtgo_id,
            league_id,
            manual.then_some("Q entered by hand")
        ],
    )
    .expect("match");
    if manual {
        conn.execute("UPDATE matches SET manual = 1 WHERE id = ?1", [id])
            .expect("manual");
    }
    for game in 1..=3_i64 {
        let game_id = id * 10 + game;
        conn.execute(
            "INSERT INTO games (id, match_id, mtgo_id, won, started_at, ended_at, turn_count)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                game_id,
                id,
                format!("0e0e0e0e-2222-4222-8222-{game_id:012}"),
                i64::from(game != 2),
                format!("2026-01-02 03:{:02}:00", (game - 1) * 10),
                format!("2026-01-02 03:{:02}:00", game * 10),
                4 + game
            ],
        )
        .expect("game");
        conn.execute(
            "INSERT INTO game_player (game_id, player_id, instance_id, is_local, on_play,
                 mulligan_count)
             VALUES (?1, 1, 0, 1, ?2, 0), (?1, 2, 1, 0, ?3, 0)",
            params![game_id, game != 2, game == 2],
        )
        .expect("seats");
    }
}

/// Set a `game_player` column of the local (or opponent) seat of a game.
fn set_seat(conn: &Connection, game_id: i64, local: bool, column: &str, value: &str) {
    conn.execute(
        &format!("UPDATE game_player SET {column} = ?3 WHERE game_id = ?1 AND is_local = ?2"),
        params![game_id, local, value],
    )
    .expect("seat");
}

/// What the player recorded in 0.x for match `id`:
/// - game 1: kept six after one mulligan, the opponent showed a 4003;
/// - game 2: one 4002 out and one 4003 in, the opponent showed two 4002 and
///   a 4001;
/// - game 3: the registered list written out in full (no change) and
///   nothing revealed.
fn add_records(conn: &Connection, id: i64) {
    let game = |number: i64| id * 10 + number;
    set_seat(conn, game(1), true, "opening_hand_json", HAND_JSON);
    set_seat(conn, game(1), true, "mulligan_count", "1");
    set_seat(conn, game(1), true, "deck_json", "[]");
    set_seat(
        conn,
        game(1),
        false,
        "deck_json",
        r#"[{"mtgo_id":4003,"quantity":1}]"#,
    );
    set_seat(conn, game(2), true, "deck_json", SIDEBOARDED_JSON);
    set_seat(
        conn,
        game(2),
        false,
        "deck_json",
        r#"[{"mtgo_id":4002,"quantity":2},{"mtgo_id":4001,"quantity":1}]"#,
    );
    set_seat(conn, game(3), true, "deck_json", REGISTERED_JSON);
    set_seat(conn, game(3), false, "deck_json", "[]");
}

fn cards(list: &[(u32, u32)]) -> Vec<CardCount> {
    list.iter()
        .map(|&(catalog_id, quantity)| CardCount {
            catalog_id,
            quantity,
        })
        .collect()
}

/// `add_records` as v1 stores it. Game 3 has nothing to store.
fn expected_details() -> Vec<ManualGameDetail> {
    vec![
        ManualGameDetail {
            game_number: 1,
            starting_hand: Some(vec![4001, 4001, 4002, 4004, 4004, 4004]),
            bottomed: vec![4002],
            mulligans: vec![vec![4001, 4001, 4001, 4002, 4002, 4004, 4004]],
            postboard_main: None,
            reveals: cards(&[(4003, 1)]),
        },
        ManualGameDetail {
            game_number: 2,
            starting_hand: None,
            bottomed: Vec::new(),
            mulligans: Vec::new(),
            postboard_main: Some(cards(&[(4001, 4), (4002, 3), (4003, 1), (4004, 20)])),
            // Per-card rows: the reveals read in catalog order.
            reveals: cards(&[(4001, 1), (4002, 2)]),
        },
    ]
}

/// Read the 0.x database and import it into `v1` the way the app does:
/// matches, decks, then leagues. Returns the match counts.
fn import_all(path: &Path, v1: &mut Connection) -> ImportCounts {
    let read = read_database(path).expect("read");
    let counts = import_matches(v1, &read.matches, NOW).expect("matches");
    if let Some(deck_read) = &read.decks {
        import_decks(v1, &deck_read.data, NOW).expect("decks");
    }
    if let Some(league_read) = &read.leagues {
        import_leagues(v1, &league_read.leagues, NOW).expect("leagues");
    }
    counts
}

fn read_match(path: &Path, token: &str) -> ImportedMatch {
    let read = read_database(path).expect("read");
    read.matches
        .into_iter()
        .find(|found| found.token == token)
        .expect("the match is read")
}

/// The tokens the match list shows under the type filter `kind`.
fn listed(conn: &Connection, kind: Option<KindFilter>) -> Vec<String> {
    let filter = MatchListFilter {
        kind,
        ..MatchListFilter::newest_first(Range {
            from_ms: i64::MIN,
            to_ms: i64::MAX,
        })
    };
    let page = match_list(conn, Scope::All(store::Platform::Mtgo), &filter, 1, 50).expect("list");
    let mut tokens: Vec<String> = page.rows.into_iter().map(|row| row.token).collect();
    tokens.sort();
    tokens
}

/// A game's number, result, on the play, turns and mulligans.
type GameSummary = (u32, String, Option<bool>, Option<i64>, Option<i64>);

#[test]
fn a_0x_manual_match_reads_with_its_hand_sideboarding_and_reveals() {
    let dir = TestDir::new("import-manual-read");
    let (path, conn) = legacy_db(&dir, &full_schema());
    add_basics(&conn);
    add_match(&conn, 1, MANUAL, true, None);
    add_records(&conn, 1);
    add_match(&conn, 3, TRACKED, false, None);
    add_records(&conn, 3);

    let manual = read_match(&path, MANUAL);
    // As read: 0.x's order. v1 stores per-card rows (catalog order).
    let mut as_read = expected_details();
    as_read[1].reveals = cards(&[(4002, 2), (4001, 1)]);
    assert_eq!(manual.manual, Some(as_read));
    assert_eq!(manual.mtgo_match_id, None, "0.x gave it `manual_*`");
    assert_eq!(manual.opponent.as_deref(), Some("Qopponent"));
    assert_eq!(manual.note.as_deref(), Some("Q entered by hand"));
    let games: Vec<GameSummary> = manual
        .games
        .iter()
        .map(|game| {
            (
                game.game_number,
                game.result.clone(),
                game.on_play,
                game.turn_count,
                game.local_mulligans,
            )
        })
        .collect();
    assert_eq!(
        games,
        vec![
            (1, "win".to_string(), Some(true), Some(5), Some(1)),
            (2, "loss".to_string(), Some(false), Some(6), Some(0)),
            (3, "win".to_string(), Some(true), Some(7), Some(0)),
        ]
    );

    let tracked = read_match(&path, TRACKED);
    assert_eq!(tracked.manual, None, "a tracked match is never manual");
}

#[test]
fn a_0x_manual_match_imports_as_manual_and_lists_under_manual() {
    let dir = TestDir::new("import-manual-write");
    let (path, conn) = legacy_db(&dir, &full_schema());
    add_basics(&conn);
    add_match(&conn, 1, MANUAL, true, None);
    add_records(&conn, 1);
    add_match(&conn, 3, TRACKED, false, None);
    let mut v1 = v1_db(&dir);

    let counts = import_all(&path, &mut v1);
    assert_eq!(
        counts,
        ImportCounts {
            added: 2,
            ..ImportCounts::default()
        }
    );
    let one = format!("FROM matches WHERE token = '{MANUAL}'");
    assert_eq!(
        text(&v1, &format!("SELECT source {one}")).as_deref(),
        Some("manual")
    );
    assert_eq!(
        count(
            &v1,
            &format!("SELECT COUNT(*) {one} AND mtgo_match_id IS NULL")
        ),
        1
    );
    assert_eq!(
        text(
            &v1,
            &format!("SELECT source FROM matches WHERE token = '{TRACKED}'")
        )
        .as_deref(),
        Some("imported")
    );
    assert_eq!(
        manual_game_rows(&v1, MANUAL).expect("details"),
        expected_details()
    );
    assert_eq!(
        text(
            &v1,
            &format!(
                "SELECT d.mtgo_id FROM matches m
                 JOIN deck_versions v ON v.id = m.deck_version_id
                 JOIN decks d ON d.id = v.deck_id WHERE m.token = '{MANUAL}'"
            )
        )
        .as_deref(),
        Some("1001"),
        "linked to its deck version like any imported match"
    );
    assert_eq!(
        text(
            &v1,
            &format!("SELECT notes FROM matches WHERE token = '{MANUAL}'")
        )
        .as_deref(),
        Some("Q entered by hand")
    );
    assert_eq!(listed(&v1, Some(KindFilter::Manual)), vec![MANUAL]);
    assert_eq!(listed(&v1, None), vec![MANUAL, TRACKED]);
}

#[test]
fn a_rerun_is_unchanged() {
    let dir = TestDir::new("import-manual-rerun");
    let (path, conn) = legacy_db(&dir, &full_schema());
    add_basics(&conn);
    add_match(&conn, 1, MANUAL, true, None);
    add_records(&conn, 1);
    let mut v1 = v1_db(&dir);
    import_all(&path, &mut v1);
    let tables = ["matches", "games", "game_cards", "game_mulligan_cards"];
    let before: Vec<Vec<Vec<String>>> = tables.iter().map(|table| dump(&v1, table)).collect();

    let counts = import_all(&path, &mut v1);
    assert_eq!(
        counts,
        ImportCounts {
            unchanged: 1,
            ..ImportCounts::default()
        }
    );
    let after: Vec<Vec<Vec<String>>> = tables.iter().map(|table| dump(&v1, table)).collect();
    assert_eq!(after, before);
}

#[test]
fn a_v1_edit_survives_a_rerun() {
    let dir = TestDir::new("import-manual-edit-kept");
    let (path, conn) = legacy_db(&dir, &full_schema());
    add_basics(&conn);
    add_match(&conn, 1, MANUAL, true, None);
    add_records(&conn, 1);
    let mut v1 = v1_db(&dir);
    import_all(&path, &mut v1);

    let reveals = ManualEdit::SetReveals {
        token: MANUAL.to_string(),
        game: 1,
        cards: cards(&[(4004, 2)]),
    };
    assert_eq!(
        edit_manual(&mut v1, &reveals, "UTC".parse().expect("a zone")).expect("reveals"),
        ManualEditOutcome::Done
    );
    let reset = ManualEdit::SetSideboard {
        token: MANUAL.to_string(),
        game: 2,
        changes: Vec::new(),
    };
    assert_eq!(
        edit_manual(&mut v1, &reset, "UTC".parse().expect("a zone")).expect("sideboard"),
        ManualEditOutcome::Done
    );
    let edited = manual_game_rows(&v1, MANUAL).expect("details");
    assert_eq!(edited[0].reveals, cards(&[(4004, 2)]));
    assert_eq!(edited[1].postboard_main, None);

    let counts = import_all(&path, &mut v1);
    assert_eq!(counts.unchanged, 1);
    assert_eq!(
        manual_game_rows(&v1, MANUAL).expect("details"),
        edited,
        "the v1 edits are kept"
    );
}

#[test]
fn manual_matches_keep_their_0x_league_membership() {
    let dir = TestDir::new("import-manual-leagues");
    let (path, conn) = legacy_db(&dir, &full_schema());
    add_basics(&conn);
    conn.execute_batch(
        "INSERT INTO leagues (id, token, name, format, started_at, created_at, state,
             deck_version_id, manual, kind)
         VALUES (1, 'q-league-1', 'Q League 1', 'CMODERN', '2026-01-02 02:00:00',
                 '2026-01-02 02:00:00', 'active', 10, 0, 'constructed'),
                (2, 'manual_q', 'Q League 2', 'CMODERN', '2026-01-02 02:00:00',
                 '2026-01-02 02:00:00', 'active', 10, 1, 'constructed');",
    )
    .expect("leagues");
    add_match(&conn, 1, MANUAL, true, Some(1));
    add_match(&conn, 2, MANUAL_2, true, Some(2));
    let mut v1 = v1_db(&dir);

    import_all(&path, &mut v1);
    assert_eq!(
        count(
            &v1,
            &format!(
                "SELECT l.imported_from FROM matches m JOIN leagues l ON l.id = m.league_id
                 WHERE m.token = '{MANUAL}' AND m.league_linked = 1 AND l.source = 'imported'"
            )
        ),
        1,
        "in the run its constructed league became"
    );
    assert_eq!(
        count(
            &v1,
            &format!(
                "SELECT l.imported_from FROM matches m JOIN leagues l ON l.id = m.league_id
                 WHERE m.token = '{MANUAL_2}' AND m.league_linked = 1 AND l.source = 'manual'"
            )
        ),
        2,
        "in the manual league its 0.x manual league became"
    );
    assert_eq!(
        count(
            &v1,
            "SELECT COUNT(*) FROM matches WHERE source = 'manual' AND league_token IS NULL"
        ),
        2,
        "no league token: the membership places them"
    );

    import_all(&path, &mut v1);
    let linked = |source: &str| {
        count(
            &v1,
            &format!(
                "SELECT COUNT(*) FROM matches m JOIN leagues l ON l.id = m.league_id
                 WHERE m.league_linked = 1 AND l.source = '{source}'"
            ),
        )
    };
    assert_eq!(linked("imported"), 1);
    assert_eq!(linked("manual"), 1);
}

#[test]
fn an_older_0x_without_the_manual_column_imports_plain_matches() {
    let dir = TestDir::new("import-manual-old-matches");
    let (path, conn) = legacy_db(&dir, &schema_without(&[MANUAL_COLUMN, MANUAL_INDEX]));
    add_basics(&conn);
    add_match(&conn, 3, TRACKED, false, None);
    add_records(&conn, 3);

    assert_eq!(read_match(&path, TRACKED).manual, None);
    let mut v1 = v1_db(&dir);
    assert_eq!(import_all(&path, &mut v1).added, 1);
    assert_eq!(
        text(&v1, "SELECT source FROM matches").as_deref(),
        Some("imported")
    );
    assert_eq!(count(&v1, "SELECT COUNT(*) FROM game_cards"), 0);
}

#[test]
fn an_older_0x_without_the_hand_and_deck_columns_imports_manual_matches_without_records() {
    let dir = TestDir::new("import-manual-old-seats");
    let (path, conn) = legacy_db(
        &dir,
        &schema_without(&[OPENING_HAND_COLUMN, DECK_JSON_COLUMN]),
    );
    add_basics(&conn);
    add_match(&conn, 1, MANUAL, true, None);

    assert_eq!(read_match(&path, MANUAL).manual, Some(Vec::new()));
    let mut v1 = v1_db(&dir);
    assert_eq!(import_all(&path, &mut v1).added, 1);
    assert_eq!(
        text(&v1, "SELECT source FROM matches").as_deref(),
        Some("manual")
    );
    assert_eq!(count(&v1, "SELECT COUNT(*) FROM game_cards"), 0);
    assert_eq!(import_all(&path, &mut v1).unchanged, 1);
}
