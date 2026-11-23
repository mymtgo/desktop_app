//! League import tests (leagues spec section 9): 0.x databases are built from
//! `fixtures/0x/schema.sql` plus the 0.x `leagues` table (the fixture has it, so the
//! extra copy is a no-op) and fake rows, read with `read_database`, and
//! written in the app's order: matches, decks, then leagues. Fake names start with Q.

mod common;

use std::path::Path;

use common::{count, legacy_db, text, texts, v1_db};
use decks::DeckCard;
use fixture_tools::TestDir;
use importer::{LeagueRead, read_database};
use rusqlite::{Connection, params};
use store::import::import_matches;
use store::import_decks::import_decks;
use store::import_leagues::{LeagueImportCounts, import_leagues};
use store::league_sync::LeagueOwner;
use store::leagues::{LeagueKind, RunFilter, StoredRun, stored_runs};

const SCHEMA: &str = include_str!("../../../fixtures/0x/schema.sql");

/// The 0.x `leagues` table as the current 0.x creates it.
const LEAGUES_TABLE: &str = r#"CREATE TABLE IF NOT EXISTS "leagues" ("id" integer primary key autoincrement not null, "token" varchar not null, "name" varchar not null, "format" varchar not null, "started_at" datetime not null, "created_at" datetime, "updated_at" datetime, "deck_change_detected" tinyint(1) not null default ('0'), "deleted_at" datetime, "state" varchar not null default ('active'), "deck_version_id" integer, "event_id" integer, "joined_at" datetime, "dropped_at" datetime, "notes" text, "completed_at" datetime, "manual" tinyint(1) not null default '0', "kind" varchar not null default 'constructed', "set_code" varchar, "mtgo_course_id" integer, "synced_hash" varchar, "synced_at" datetime);"#;

/// The `leagues` table of an older 0.x: only the first columns.
const OLD_LEAGUES_TABLE: &str = r#"CREATE TABLE IF NOT EXISTS "leagues" ("id" integer primary key autoincrement not null, "token" varchar not null, "name" varchar not null, "format" varchar not null, "started_at" datetime not null, "created_at" datetime, "updated_at" datetime);"#;

const STARTED: &str = "2026-01-02 03:04:05";
const STARTED_MS: i64 = 1_767_323_045_000;
const ENDED: &str = "2026-01-02 03:34:05";
const ENDED_MS: i64 = 1_767_324_845_000;
const NOW: i64 = 1_800_000_000_000;

fn burn_list() -> String {
    decks::signature(&[DeckCard {
        catalog_id: 12345,
        quantity: 4,
        sideboard: false,
    }])
}

/// A 0.x database at `<dir>/nativephp.sqlite` built from `schema`.
fn full_schema() -> String {
    format!("{SCHEMA}\n{LEAGUES_TABLE}")
}

/// The 0.x schema without the `leagues` table and its indexes.
fn schema_without_leagues() -> String {
    SCHEMA
        .lines()
        .filter(|line| {
            !line.starts_with("CREATE TABLE IF NOT EXISTS \"leagues\"")
                && !line.contains(" on \"leagues\" ")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The rows of a one-column query, in order.
/// Two players, the deck owner's account, one card, and deck 1 (`1001`, Q
/// Burn) with version 10.
fn add_basics(conn: &Connection) {
    conn.execute_batch(
        "INSERT INTO players (id, username) VALUES (1, 'Qlocal'), (2, 'Qopponent');
         INSERT INTO accounts (id, username) VALUES (1, 'Qlocal'), (2, 'Qdeckowner');
         INSERT INTO cards (id, mtgo_id, name) VALUES (1, '12345', 'Q Bolt');
         INSERT INTO decks (id, mtgo_id, name, format, account_id)
         VALUES (1, '1001', 'Q Burn', 'CMODERN', 1);",
    )
    .expect("basics");
    conn.execute(
        "INSERT INTO deck_versions (id, deck_id, signature, modified_at)
         VALUES (10, 1, ?1, '2026-01-01 00:00:00')",
        [burn_list()],
    )
    .expect("version");
}

/// A complete 0.x match `id` with one won game between Qlocal and Qopponent,
/// in 0.x league `league_id`.
fn add_match(conn: &Connection, id: i64, token: &str, state: &str, league_id: Option<i64>) {
    conn.execute(
        "INSERT INTO matches (id, token, mtgo_id, deck_version_id, format, match_type, games_won,
             games_lost, started_at, ended_at, state, league_id)
         VALUES (?1, ?2, ?3, 10, 'CMODERN', 'Constructed', 2, 1, ?4, ?5, ?6, ?7)",
        params![
            id,
            token,
            (100 + id).to_string(),
            STARTED,
            ENDED,
            state,
            league_id
        ],
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

/// A 0.x league row: constructed, not manual, not deleted, deck version 10.
struct League {
    id: i64,
    token: &'static str,
    state: &'static str,
    kind: &'static str,
    manual: bool,
    deck_version_id: Option<i64>,
    notes: Option<&'static str>,
    deleted_at: Option<&'static str>,
}

fn league(id: i64, token: &'static str) -> League {
    League {
        id,
        token,
        state: "complete",
        kind: "constructed",
        manual: false,
        deck_version_id: Some(10),
        notes: None,
        deleted_at: None,
    }
}

fn add_league(conn: &Connection, league: League) {
    conn.execute(
        "INSERT INTO leagues (id, token, name, format, started_at, created_at, updated_at,
             deleted_at, state, deck_version_id, event_id, joined_at, dropped_at, notes,
             completed_at, manual, kind)
         VALUES (?1, ?2, ?3, 'CMODERN', ?4, ?5, ?5, ?6, ?7, ?8, ?9, ?4, NULL, ?10, ?11, ?12, ?13)",
        params![
            league.id,
            league.token,
            format!("Q League {}", league.id),
            STARTED,
            ENDED,
            league.deleted_at,
            league.state,
            league.deck_version_id,
            7_000 + league.id,
            league.notes,
            if league.state == "complete" {
                Some(ENDED)
            } else {
                None
            },
            league.manual,
            league.kind
        ],
    )
    .expect("league");
}

/// Read the 0.x database and import it into `v1` the way the app does:
/// matches, decks, then leagues. Returns the league read and counts.
fn import_all(path: &Path, v1: &mut Connection, now_ms: i64) -> (LeagueRead, LeagueImportCounts) {
    let read = read_database(path).expect("read");
    import_matches(v1, &read.matches, now_ms).expect("matches");
    if let Some(deck_read) = &read.decks {
        import_decks(v1, &deck_read.data, now_ms).expect("decks");
    }
    let league_read = read.leagues.expect("the 0.x database has leagues");
    let counts = import_leagues(v1, &league_read.leagues, now_ms).expect("leagues");
    (league_read, counts)
}

/// The stored runs of `owner`, of `kinds`, from `source`.
fn runs_of(
    conn: &Connection,
    owner: LeagueOwner,
    kinds: &[LeagueKind],
    source: &str,
) -> Vec<StoredRun> {
    let filter = RunFilter {
        owners: vec![owner],
        kinds: kinds.to_vec(),
        deck_id: None,
        run_key: None,
    };
    stored_runs(conn, &filter)
        .expect("stored runs")
        .into_iter()
        .filter(|run| run.source == source)
        .collect()
}

/// The matches linked to the imported run of 0.x league `id`, by token.
fn run_members(conn: &Connection, id: i64) -> Vec<String> {
    texts(
        conn,
        &format!(
            "SELECT m.token FROM matches m JOIN leagues l ON l.id = m.league_id
             WHERE l.source = 'imported' AND l.imported_from = {id} AND m.league_linked = 1
             ORDER BY m.token"
        ),
    )
}

/// The matches linked to any imported run, by token.
const IMPORTED_MEMBERS: &str = "SELECT m.token FROM matches m JOIN leagues l ON l.id = m.league_id
     WHERE l.source = 'imported' AND m.league_linked = 1 ORDER BY m.token";

/// The matches linked to any manual league, by token.
const MANUAL_MEMBERS: &str = "SELECT m.token FROM matches m JOIN leagues l ON l.id = m.league_id
     WHERE l.source = 'manual' AND m.league_linked = 1 ORDER BY m.token";

fn account_id(conn: &Connection, username: &str) -> i64 {
    count(
        conn,
        &format!("SELECT id FROM accounts WHERE username = '{username}'"),
    )
}

#[test]
fn leagues_become_runs_and_manual_leagues_with_their_matches_and_notes() {
    let dir = TestDir::new("import-leagues-map");
    let (path, conn) = legacy_db(&dir, &full_schema());
    add_basics(&conn);
    add_league(
        &conn,
        League {
            notes: Some("Q run notes"),
            ..league(1, "q-league-1")
        },
    );
    add_league(
        &conn,
        League {
            manual: true,
            notes: Some("Q manual notes"),
            ..league(2, "manual_q")
        },
    );
    add_league(
        &conn,
        League {
            kind: "draft",
            ..league(3, "q-draft")
        },
    );
    add_league(
        &conn,
        League {
            kind: "sealed",
            ..league(4, "q-sealed")
        },
    );
    add_league(
        &conn,
        League {
            deleted_at: Some(ENDED),
            ..league(5, "q-deleted")
        },
    );
    add_match(&conn, 1, "tok-a", "complete", Some(1));
    add_match(&conn, 2, "tok-b", "complete", Some(1));
    add_match(&conn, 3, "tok-c", "complete", Some(2));
    add_match(&conn, 4, "tok-draft", "complete", Some(3));
    add_match(&conn, 5, "tok-free", "complete", None);

    let mut v1 = v1_db(&dir);
    let (read, counts) = import_all(&path, &mut v1, NOW);
    assert_eq!(
        (
            read.leagues.len(),
            read.skipped_deleted,
            read.skipped_unreadable
        ),
        (4, 1, 0)
    );
    assert_eq!(
        counts,
        LeagueImportCounts {
            added: 4,
            ..LeagueImportCounts::default()
        }
    );

    let local = account_id(&v1, "Qlocal");
    let version: i64 = count(
        &v1,
        "SELECT v.id FROM deck_versions v JOIN decks d ON d.id = v.deck_id
         WHERE d.mtgo_id = '1001'",
    );
    let run: (
        i64,
        i64,
        String,
        i64,
        String,
        String,
        String,
        i64,
        i64,
        i64,
        i64,
    ) = v1
        .query_row(
            "SELECT imported_from, account_id, token, event_id, name, format, state, started_at,
                 joined_at, completed_at, deck_version_id
             FROM leagues WHERE source = 'imported' AND kind = 'constructed'",
            [],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                    row.get(8)?,
                    row.get(9)?,
                    row.get(10)?,
                ))
            },
        )
        .expect("one imported run");
    assert_eq!(
        run,
        (
            1,
            local,
            "q-league-1".to_string(),
            7_001,
            "Q League 1".to_string(),
            "CMODERN".to_string(),
            "complete".to_string(),
            STARTED_MS,
            STARTED_MS,
            ENDED_MS,
            version
        )
    );
    assert_eq!(
        count(
            &v1,
            "SELECT COUNT(*) FROM leagues WHERE source = 'imported' AND dropped_at IS NULL
                 AND json_extract(imported_seed, '$.dropped_at') IS NULL"
        ),
        3
    );
    // Draft and sealed leagues are limited seeds (Plan N), never constructed
    // runs or manual leagues.
    assert_eq!(
        texts(
            &v1,
            "SELECT imported_from || ':' || kind FROM leagues
             WHERE source = 'imported' AND kind <> 'constructed' ORDER BY imported_from"
        ),
        vec!["3:draft", "4:sealed"]
    );
    assert_eq!(run_members(&v1, 3), vec!["tok-draft"]);
    assert_eq!(run_members(&v1, 1), vec!["tok-a", "tok-b"]);

    let manual: (i64, i64, i64, String, i64, i64, i64) = v1
        .query_row(
            "SELECT ml.id, ml.account_id, ml.deck_id, ml.name, ml.started_at, ml.created_at,
                 ml.imported_from
             FROM leagues ml WHERE ml.source = 'manual'",
            [],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                ))
            },
        )
        .expect("one manual league");
    let deck_id = count(&v1, "SELECT id FROM decks WHERE mtgo_id = '1001'");
    assert_eq!(
        manual,
        (
            manual.0,
            local,
            deck_id,
            "Q League 2".to_string(),
            STARTED_MS,
            ENDED_MS,
            2
        )
    );
    assert_eq!(
        texts(
            &v1,
            &format!(
                "SELECT token FROM matches WHERE league_id = {} AND league_linked = 1",
                manual.0
            )
        ),
        vec!["tok-c"]
    );
    assert_eq!(
        texts(
            &v1,
            "SELECT run_key || '=' || notes FROM leagues WHERE notes IS NOT NULL ORDER BY run_key"
        ),
        vec![
            "0x:1=Q run notes".to_string(),
            format!("manual:{}=Q manual notes", manual.0)
        ]
    );
    assert_eq!(
        count(
            &v1,
            "SELECT COUNT(*) FROM leagues WHERE notes IS NOT NULL AND notes_edited_at IS NOT NULL"
        ),
        0,
        "an imported 0.x note is not a v1 edit"
    );

    // The league reads list both under the account, and the limited runs
    // apart.
    let owner = LeagueOwner::Account(local);
    let runs = runs_of(&v1, owner, &[LeagueKind::Constructed], "imported");
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].matches.len(), 2);
    let limited = runs_of(
        &v1,
        owner,
        &[LeagueKind::Draft, LeagueKind::Sealed],
        "imported",
    );
    let kinds: Vec<&str> = limited.iter().map(|run| run.kind.as_str()).collect();
    assert_eq!(kinds, vec!["sealed", "draft"], "newest first, then by id");
    let manuals = runs_of(&v1, owner, &[LeagueKind::Constructed], "manual");
    assert_eq!(manuals.len(), 1);
    assert_eq!(manuals[0].matches.len(), 1);
}

#[test]
fn live_matches_join_their_imported_run_by_their_live_token_and_a_rerun_is_unchanged() {
    let dir = TestDir::new("import-leagues-live");
    let (path, conn) = legacy_db(&dir, &full_schema());
    add_basics(&conn);
    add_league(&conn, league(1, "q-league-1"));
    add_league(
        &conn,
        League {
            manual: true,
            ..league(2, "manual_q")
        },
    );
    add_match(&conn, 1, "tok-a", "complete", Some(1));
    // Tracked live with its league token (0.x and v1 side by side): the
    // imported run claims it, so the run shows once with 0.x's record.
    add_match(&conn, 2, "tok-live-league", "complete", Some(1));
    // A 0.x history import of a match v1 tracked live under another token.
    add_match(&conn, 3, "tok-history", "complete", Some(2));
    // Still in progress in 0.x: v1 has no row for it.
    add_match(&conn, 4, "tok-running", "in_progress", Some(1));
    // Tracked live without a league token: the imported run takes it.
    add_match(&conn, 5, "tok-live-plain", "complete", Some(1));

    let mut v1 = v1_db(&dir);
    v1.execute_batch(
        "INSERT INTO accounts (username, first_seen_at) VALUES ('Qlocal', 1);
         INSERT INTO matches (token, mtgo_match_id, account_id, source, state, outcome,
             started_at, last_activity_at, revision, derived_version, league_token)
         VALUES ('tok-live-league', 102, 1, 'log', 'complete', 'win', 10, 20, 3, 4, 'q-league-1'),
                ('tok-other-live', 103, 1, 'log', 'complete', 'win', 10, 20, 3, 4, NULL),
                ('tok-live-plain', 105, 1, 'log', 'complete', 'win', 10, 20, 3, 4, NULL);",
    )
    .expect("live rows");

    let (_, counts) = import_all(&path, &mut v1, NOW);
    assert_eq!(counts.added, 2);
    assert_eq!(
        texts(&v1, IMPORTED_MEMBERS),
        vec!["tok-a", "tok-live-league", "tok-live-plain"]
    );
    assert_eq!(texts(&v1, MANUAL_MEMBERS), vec!["tok-other-live"]);
    // Nothing else holds the claimed match: no derived run of its own.
    // After the league sync the claimed match is a member of the imported
    // run and of no other run.
    sync(&v1);
    let rows = texts(
        &v1,
        "SELECT l.source FROM matches m JOIN leagues l ON l.id = m.league_id
         WHERE m.token = 'tok-live-league'",
    );
    let rows: Vec<&str> = rows.iter().map(String::as_str).collect();
    assert_eq!(rows, vec!["imported"], "once, as a member");
    assert_eq!(
        count(
            &v1,
            "SELECT COUNT(*) FROM leagues WHERE source <> 'imported' AND run_key LIKE '%:tok-live-league'"
        ),
        0,
        "no derived run of its own"
    );

    let (_, rerun) = import_all(&path, &mut v1, NOW + 1);
    assert_eq!((rerun.added, rerun.updated, rerun.unchanged), (0, 0, 2));
    assert_eq!(
        texts(&v1, IMPORTED_MEMBERS),
        vec!["tok-a", "tok-live-league", "tok-live-plain"]
    );
}

#[test]
fn the_account_comes_from_the_matches_then_the_deck_then_the_only_account() {
    let dir = TestDir::new("import-leagues-account");
    let (path, conn) = legacy_db(&dir, &full_schema());
    add_basics(&conn);
    // The deck belongs to Qdeckowner; the matches are Qlocal's.
    conn.execute_batch("UPDATE decks SET account_id = 2 WHERE id = 1;")
        .expect("deck owner");
    add_league(&conn, league(1, "q-with-matches"));
    add_league(
        &conn,
        League {
            manual: true,
            ..league(2, "manual_deck_only")
        },
    );
    add_league(
        &conn,
        League {
            manual: true,
            deck_version_id: None,
            ..league(3, "manual_nothing")
        },
    );
    add_match(&conn, 1, "tok-a", "complete", Some(1));

    let mut v1 = v1_db(&dir);
    let (_, counts) = import_all(&path, &mut v1, NOW);
    // Two accounts (Qlocal from the match, Qdeckowner from the deck): the
    // league with neither matches nor a deck has no account.
    assert_eq!(
        counts,
        LeagueImportCounts {
            added: 3,
            without_account: 1,
            ..LeagueImportCounts::default()
        }
    );
    let local = account_id(&v1, "Qlocal");
    let owner = account_id(&v1, "Qdeckowner");
    assert_eq!(
        count(
            &v1,
            "SELECT account_id FROM leagues WHERE imported_from = 1"
        ),
        local
    );
    assert_eq!(
        count(
            &v1,
            "SELECT account_id FROM leagues WHERE source = 'manual' AND imported_from = 2"
        ),
        owner
    );
    assert_eq!(
        count(
            &v1,
            "SELECT COUNT(*) FROM leagues
             WHERE source = 'manual' AND imported_from = 3 AND account_id IS NULL
                 AND deck_id IS NULL"
        ),
        1
    );
}

#[test]
fn a_league_with_no_matches_and_no_deck_takes_the_only_account() {
    let dir = TestDir::new("import-leagues-only-account");
    let (path, conn) = legacy_db(&dir, &full_schema());
    add_basics(&conn);
    add_league(
        &conn,
        League {
            manual: true,
            deck_version_id: None,
            ..league(3, "manual_nothing")
        },
    );
    add_match(&conn, 1, "tok-a", "complete", None);
    // Qdeckowner owns no imported deck here, so v1 has one account.
    conn.execute_batch("UPDATE decks SET account_id = 1 WHERE id = 1;")
        .expect("deck owner");

    let mut v1 = v1_db(&dir);
    let (_, counts) = import_all(&path, &mut v1, NOW);
    assert_eq!((counts.added, counts.without_account), (1, 0));
    assert_eq!(
        count(
            &v1,
            "SELECT account_id FROM leagues WHERE source = 'manual' AND imported_from = 3"
        ),
        account_id(&v1, "Qlocal")
    );
}

#[test]
fn a_rerun_is_unchanged_keeps_v1_notes_drops_and_links_and_follows_0x_changes() {
    let dir = TestDir::new("import-leagues-rerun");
    let (path, conn) = legacy_db(&dir, &full_schema());
    add_basics(&conn);
    add_league(
        &conn,
        League {
            state: "active",
            notes: Some("Q 0x run notes"),
            ..league(1, "q-league-1")
        },
    );
    add_league(
        &conn,
        League {
            manual: true,
            notes: Some("Q 0x manual notes"),
            ..league(2, "manual_q")
        },
    );
    add_match(&conn, 1, "tok-a", "complete", Some(1));
    add_match(&conn, 2, "tok-c", "complete", Some(2));
    add_match(&conn, 3, "tok-d", "complete", Some(2));

    let mut v1 = v1_db(&dir);
    let (_, first) = import_all(&path, &mut v1, NOW);
    assert_eq!(first.added, 2);
    let manual_id = count(&v1, "SELECT id FROM leagues WHERE source = 'manual'");

    // Made in v1: a note on the run, a manual drop, and an unlinked match.
    v1.execute_batch(&format!(
        "UPDATE leagues SET notes = 'Q v1 note' WHERE run_key = '0x:1';
         UPDATE leagues SET user_dropped_at = 99, user_state = 'dropped' WHERE run_key = '0x:1';
         UPDATE matches SET league_id = NULL, league_linked = 0 WHERE token = 'tok-d';
         UPDATE leagues SET notes = 'Q v1 manual note' WHERE run_key = 'manual:{manual_id}';"
    ))
    .expect("v1 edits");
    let run_row = |v1: &Connection| {
        text(
            v1,
            "SELECT id || ':' || account_id || ':' || imported_seed FROM leagues
             WHERE imported_from = 1",
        )
    };
    let first_row = run_row(&v1);

    let (_, again) = import_all(&path, &mut v1, NOW + 1);
    assert_eq!(
        again,
        LeagueImportCounts {
            unchanged: 2,
            ..LeagueImportCounts::default()
        }
    );
    assert_eq!(run_row(&v1), first_row, "an unchanged run is not rewritten");

    // 0.x moves on: the run completes and gains a match, the manual league is
    // renamed and gains a match.
    conn.execute_batch(&format!(
        "UPDATE leagues SET state = 'complete', completed_at = '{ENDED}' WHERE id = 1;
         UPDATE leagues SET name = 'Q Renamed' WHERE id = 2;"
    ))
    .expect("0.x changes");
    add_match(&conn, 4, "tok-e", "complete", Some(1));
    add_match(&conn, 5, "tok-f", "complete", Some(2));

    let (_, changed) = import_all(&path, &mut v1, NOW + 2);
    assert_eq!(
        changed,
        LeagueImportCounts {
            updated: 2,
            ..LeagueImportCounts::default()
        }
    );
    assert_eq!(
        text(
            &v1,
            "SELECT json_extract(imported_seed, '$.state') || ':'
                 || json_extract(imported_seed, '$.completed_at')
             FROM leagues WHERE imported_from = 1"
        ),
        Some(format!("complete:{ENDED_MS}"))
    );
    assert_eq!(texts(&v1, IMPORTED_MEMBERS), vec!["tok-a", "tok-e"]);
    assert_eq!(
        text(&v1, "SELECT name FROM leagues WHERE source = 'manual'"),
        Some("Q Renamed".to_string())
    );
    assert_eq!(
        texts(&v1, MANUAL_MEMBERS),
        vec!["tok-c"],
        "a manual league's links are the user's after the first import"
    );
    assert_eq!(
        texts(
            &v1,
            "SELECT notes FROM leagues WHERE notes IS NOT NULL ORDER BY run_key"
        ),
        vec!["Q v1 note", "Q v1 manual note"]
    );
    assert_eq!(
        count(
            &v1,
            "SELECT user_dropped_at FROM leagues WHERE run_key = '0x:1'"
        ),
        99
    );
    assert_eq!(
        count(&v1, "SELECT COUNT(*) FROM leagues WHERE source = 'manual'"),
        1
    );
}

#[test]
fn an_old_leagues_table_reads_its_missing_columns_as_defaults() {
    let dir = TestDir::new("import-leagues-old-columns");
    let (path, conn) = legacy_db(&dir, &format!("{SCHEMA}\n{OLD_LEAGUES_TABLE}"));
    add_basics(&conn);
    conn.execute(
        "INSERT INTO leagues (id, token, name, format, started_at)
         VALUES (1, 'q-old', 'Q Old League', 'CMODERN', ?1),
                (2, 'q-bad-time', 'Q Bad', 'CMODERN', 'not a time')",
        [STARTED],
    )
    .expect("old leagues");
    add_match(&conn, 1, "tok-a", "complete", Some(1));

    let mut v1 = v1_db(&dir);
    let (read, counts) = import_all(&path, &mut v1, NOW);
    assert_eq!(
        (read.leagues.len(), read.skipped_unreadable),
        (1, 1),
        "constructed, not manual, not deleted; the bad time is skipped"
    );
    assert_eq!(counts.added, 1);
    assert_eq!(
        text(
            &v1,
            "SELECT state || ':' || ifnull(event_id, '-') || ':' || ifnull(deck_version_id, '-')
                 || ':' || ifnull(completed_at, '-')
             FROM leagues WHERE imported_from = 1"
        ),
        Some("active:-:-:-".to_string())
    );
    assert_eq!(texts(&v1, IMPORTED_MEMBERS), vec!["tok-a"]);
    assert_eq!(
        count(&v1, "SELECT COUNT(*) FROM leagues WHERE notes IS NOT NULL"),
        0
    );
}

#[test]
fn an_unknown_state_is_unreadable_and_matches_without_league_ids_give_no_members() {
    let dir = TestDir::new("import-leagues-no-league-id");
    let schema = full_schema()
        .replacen("\"league_id\" integer, ", "", 1)
        .replacen(
            ", foreign key(\"league_id\") references leagues(\"id\") on delete no action on update no action",
            "",
            1,
        )
        .replace(
            "CREATE INDEX \"matches_league_id_index\" on \"matches\" (\"league_id\");",
            "",
        );
    assert_ne!(schema, full_schema(), "league_id was in the matches table");
    let (path, conn) = legacy_db(&dir, &schema);
    add_basics(&conn);
    add_league(&conn, league(1, "q-league-1"));
    add_league(
        &conn,
        League {
            state: "abandoned",
            ..league(2, "q-odd-state")
        },
    );
    conn.execute(
        "INSERT INTO matches (id, token, mtgo_id, format, match_type, started_at, state)
         VALUES (1, 'tok-a', '101', 'CMODERN', 'Constructed', ?1, 'complete')",
        [STARTED],
    )
    .expect("match");

    let mut v1 = v1_db(&dir);
    let (read, counts) = import_all(&path, &mut v1, NOW);
    assert_eq!((read.leagues.len(), read.skipped_unreadable), (1, 1));
    assert_eq!(counts.added, 1);
    assert_eq!(
        count(&v1, "SELECT COUNT(*) FROM matches WHERE league_linked = 1"),
        0
    );
}

#[test]
fn a_database_without_a_leagues_table_has_nothing_to_import() {
    let dir = TestDir::new("import-leagues-no-table");
    let (path, conn) = legacy_db(&dir, &schema_without_leagues());
    add_basics(&conn);
    add_match(&conn, 1, "tok-a", "complete", None);

    let read = read_database(&path).expect("read");
    assert_eq!(read.matches.len(), 1);
    assert!(
        read.leagues.is_none(),
        "no leagues table: nothing to import"
    );
    let mut v1 = v1_db(&dir);
    assert_eq!(
        import_leagues(&mut v1, &[], NOW).expect("nothing"),
        LeagueImportCounts::default()
    );
}

/// The league sync the tracker runs after an import, for every owner.
fn sync(v1: &Connection) {
    let tx = v1.unchecked_transaction().expect("transaction");
    store::league_sync::sync_every_owner(&tx, "UTC".parse().expect("a zone"), NOW)
        .expect("league sync");
    tx.commit().expect("commit");
}

#[test]
fn an_import_rerun_updates_seeds_and_links_never_notes_or_drops() {
    let dir = TestDir::new("import-leagues-rerun-seeds");
    let (path, conn) = legacy_db(&dir, &full_schema());
    add_basics(&conn);
    add_league(
        &conn,
        League {
            state: "active",
            notes: Some("Q 0x run notes"),
            ..league(1, "q-league-1")
        },
    );
    add_league(
        &conn,
        League {
            manual: true,
            notes: Some("Q 0x manual notes"),
            ..league(2, "manual_q")
        },
    );
    add_match(&conn, 1, "tok-a", "complete", Some(1));
    add_match(&conn, 2, "tok-b", "complete", Some(1));
    add_match(&conn, 3, "tok-c", "complete", Some(2));

    let mut v1 = v1_db(&dir);
    import_all(&path, &mut v1, NOW);
    let row = count(&v1, "SELECT id FROM leagues WHERE run_key = '0x:1'");
    // Made in v1: a note and a drop on the run; then the sync folds it.
    v1.execute_batch(
        "UPDATE leagues SET notes = 'Q v1 note', user_dropped_at = 99, user_state = 'dropped' WHERE run_key = '0x:1';",
    )
    .expect("v1 edits");
    sync(&v1);
    assert_eq!(
        text(&v1, "SELECT state FROM leagues WHERE run_key = '0x:1'"),
        Some("dropped".to_string()),
        "the sync folded the drop"
    );

    // 0.x moves on: the run completes, gains a match and loses one.
    conn.execute_batch(&format!(
        "UPDATE leagues SET state = 'complete', completed_at = '{ENDED}' WHERE id = 1;
         UPDATE matches SET league_id = NULL WHERE id = 2;"
    ))
    .expect("0.x changes");
    add_match(&conn, 4, "tok-e", "complete", Some(1));

    let (_, rerun) = import_all(&path, &mut v1, NOW + 1);
    assert_eq!((rerun.added, rerun.updated, rerun.unchanged), (0, 1, 1));
    assert_eq!(
        count(&v1, "SELECT id FROM leagues WHERE run_key = '0x:1'"),
        row,
        "the same row"
    );
    assert_eq!(
        text(
            &v1,
            "SELECT json_extract(imported_seed, '$.state') || ':'
                 || json_extract(imported_seed, '$.completed_at')
             FROM leagues WHERE run_key = '0x:1'"
        ),
        Some(format!("complete:{ENDED_MS}")),
        "the seed follows 0.x"
    );
    assert_eq!(run_members(&v1, 1), vec!["tok-a", "tok-e"]);
    assert_eq!(
        count(
            &v1,
            "SELECT league_linked FROM matches WHERE token = 'tok-b'"
        ),
        0,
        "a match 0.x took out is unlinked"
    );
    sync(&v1);
    let input = texts(
        &v1,
        "SELECT run_key || '=' || ifnull(notes, '-') || '=' || ifnull(user_dropped_at, '-')
         FROM leagues ORDER BY run_key",
    );
    let manual_id = count(&v1, "SELECT id FROM leagues WHERE source = 'manual'");
    assert_eq!(
        input,
        vec![
            "0x:1=Q v1 note=99".to_string(),
            format!("manual:{manual_id}=Q 0x manual notes=-"),
        ],
        "the import and the sync never write notes or drops"
    );
}

#[test]
fn a_second_0x_database_keeps_the_first_ones_leagues() {
    let dir = TestDir::new("import-leagues-second-db");
    let (first_path, first) = legacy_db(&dir, &full_schema());
    add_basics(&first);
    add_league(&first, league(1, "q-league-1"));
    add_league(
        &first,
        League {
            manual: true,
            ..league(2, "manual_q")
        },
    );
    add_match(&first, 1, "tok-a", "complete", Some(1));
    add_match(&first, 2, "tok-b", "complete", Some(2));
    // Another 0.x database: its leagues 1 and 2 are other leagues, and its
    // league 3 is the first one's league 1 seen again.
    let second_dir = TestDir::new("import-leagues-second-db-b");
    let (second_path, second) = legacy_db(&second_dir, &full_schema());
    add_basics(&second);
    add_league(&second, league(1, "q-league-b"));
    add_league(
        &second,
        League {
            manual: true,
            ..league(2, "manual_qb")
        },
    );
    add_league(
        &second,
        League {
            state: "active",
            ..league(3, "q-league-1")
        },
    );
    add_match(&second, 11, "tok-b1", "complete", Some(1));
    add_match(&second, 12, "tok-b2", "complete", Some(2));
    add_match(&second, 13, "tok-b3", "complete", Some(3));

    let mut v1 = v1_db(&dir);
    import_all(&first_path, &mut v1, NOW);
    let rows = "SELECT run_key || ':' || source || ':' || token || ':'
                    || COALESCE(imported_seed, '-') FROM leagues ORDER BY id";
    let first_rows = texts(&v1, rows);

    let (_, counts) = import_all(&second_path, &mut v1, NOW + 1);

    assert_eq!(
        counts,
        LeagueImportCounts {
            added: 2,
            unchanged: 1,
            ..LeagueImportCounts::default()
        }
    );
    let after = texts(&v1, rows);
    assert_eq!(
        after[..2],
        first_rows[..],
        "the first database's leagues stay"
    );
    assert_eq!(after.len(), 4);
    assert_eq!(
        texts(
            &v1,
            "SELECT token || '=' || imported_from FROM leagues ORDER BY id"
        ),
        vec!["q-league-1=1", "manual_q=2", "q-league-b=3", "manual_qb=4"]
    );
    assert_eq!(run_members(&v1, 1), vec!["tok-a"]);
    assert_eq!(run_members(&v1, 3), vec!["tok-b1"]);
    assert_eq!(
        texts(
            &v1,
            "SELECT m.token FROM matches m JOIN leagues l ON l.id = m.league_id
             WHERE l.token = 'manual_qb' AND m.league_linked = 1"
        ),
        vec!["tok-b2"]
    );

    // Each database imported again changes nothing.
    let before = texts(&v1, rows);
    let links = texts(
        &v1,
        "SELECT token || '=' || COALESCE(league_id, '-') FROM matches",
    );
    let (_, second_again) = import_all(&second_path, &mut v1, NOW + 2);
    let (_, first_again) = import_all(&first_path, &mut v1, NOW + 3);
    assert_eq!((second_again.unchanged, first_again.unchanged), (3, 2));
    assert_eq!(texts(&v1, rows), before);
    assert_eq!(
        texts(
            &v1,
            "SELECT token || '=' || COALESCE(league_id, '-') FROM matches"
        ),
        links
    );
}

// Review I2: the second database's league 1, stored under id 3, is that
// database's own on a rerun: a match 0.x adds to it is linked.
#[test]
fn a_second_databases_collided_league_follows_its_0x_on_a_rerun() {
    let dir = TestDir::new("import-leagues-collided-rerun");
    let (first_path, first) = legacy_db(&dir, &full_schema());
    add_basics(&first);
    add_league(&first, league(1, "q-league-1"));
    add_match(&first, 1, "tok-a", "complete", Some(1));
    let second_dir = TestDir::new("import-leagues-collided-rerun-b");
    let (second_path, second) = legacy_db(&second_dir, &full_schema());
    add_basics(&second);
    add_league(
        &second,
        League {
            state: "active",
            ..league(1, "q-league-b")
        },
    );
    add_match(&second, 11, "tok-b1", "complete", Some(1));
    let mut v1 = v1_db(&dir);
    import_all(&first_path, &mut v1, NOW);
    import_all(&second_path, &mut v1, NOW + 1);
    let moved = count(
        &v1,
        "SELECT imported_from FROM leagues WHERE token = 'q-league-b'",
    );
    assert_ne!(moved, 1);

    second
        .execute("UPDATE leagues SET state = 'complete' WHERE id = 1", [])
        .expect("0.x completes it");
    add_match(&second, 12, "tok-b2", "complete", Some(1));
    let (_, rerun) = import_all(&second_path, &mut v1, NOW + 2);

    assert_eq!(rerun.updated, 1);
    assert_eq!(run_members(&v1, moved), vec!["tok-b1", "tok-b2"]);
    assert_eq!(
        text(
            &v1,
            &format!(
                "SELECT json_extract(imported_seed, '$.state') FROM leagues
                 WHERE imported_from = {moved}"
            )
        ),
        Some("complete".to_string())
    );
    assert_eq!(
        run_members(&v1, 1),
        vec!["tok-a"],
        "the first database's run"
    );
}

/// Issue #337: a league whose every match is of a player that is not one of
/// 0.x's accounts is left out (counted); a league with one of the account's
/// matches keeps only that one; a league without matches stays.
#[test]
fn a_league_of_another_player_is_left_out() {
    let dir = TestDir::new("importer-leagues-not-account");
    let (path, conn) = legacy_db(&dir, &full_schema());
    add_basics(&conn);
    conn.execute(
        "INSERT INTO players (id, username) VALUES (3, 'Qstranger')",
        [],
    )
    .expect("another player");
    add_league(&conn, league(1, "q-stranger-only"));
    add_league(&conn, league(2, "q-mixed"));
    add_league(&conn, league(3, "q-no-matches"));
    add_match(&conn, 1, "q-stranger-1", "complete", Some(1));
    add_match(&conn, 2, "q-stranger-2", "complete", Some(1));
    add_match(&conn, 3, "q-stranger-3", "complete", Some(2));
    add_match(&conn, 4, "q-own-4", "complete", Some(2));
    // Qstranger's seats, on no deck of an account (a real account's deck
    // would keep the match under that account).
    conn.execute_batch(
        "UPDATE game_player SET player_id = 3 WHERE is_local = 1 AND game_id IN (10, 20, 30);
         UPDATE matches SET deck_version_id = NULL WHERE id IN (1, 2, 3);",
    )
    .expect("Qstranger's seats");

    let mut v1 = v1_db(&dir);
    let (read, counts) = import_all(&path, &mut v1, NOW);
    assert_eq!(read.skipped_not_account, 1);
    assert_eq!(counts.added, 2, "{counts:?}");
    assert_eq!(
        texts(
            &v1,
            "SELECT token FROM leagues WHERE source = 'imported' ORDER BY token"
        ),
        vec!["q-mixed", "q-no-matches"]
    );
    assert_eq!(texts(&v1, IMPORTED_MEMBERS), vec!["q-own-4"]);
    assert_eq!(
        texts(&v1, "SELECT username FROM accounts ORDER BY username"),
        vec!["Qlocal"],
        "no account for Qstranger"
    );

    // A rerun leaves the same.
    let (again_read, again) = import_all(&path, &mut v1, NOW + 1);
    assert_eq!(again_read.skipped_not_account, 1);
    assert_eq!((again.added, again.updated), (0, 0), "{again:?}");
}

// League self-management spec 4.6: a rerun finds every row (removed ones
// too), never writes the player's league columns, and never links or
// unlinks a match the player decided (`edited_at` or `no_league`).
#[test]
fn a_rerun_never_moves_a_match_the_player_decided_or_undeletes_a_league() {
    let dir = TestDir::new("import-leagues-player-edits");
    let (path, conn) = legacy_db(&dir, &full_schema());
    add_basics(&conn);
    add_league(
        &conn,
        League {
            state: "active",
            ..league(1, "q-league-1")
        },
    );
    add_league(
        &conn,
        League {
            manual: true,
            ..league(2, "manual_q")
        },
    );
    add_match(&conn, 1, "tok-a", "complete", Some(1));
    add_match(&conn, 2, "tok-b", "complete", Some(1));
    add_match(&conn, 3, "tok-c", "complete", Some(1));
    add_match(&conn, 4, "tok-d", "complete", Some(2));
    add_match(&conn, 5, "tok-x", "complete", None);

    let mut v1 = v1_db(&dir);
    let (_, first) = import_all(&path, &mut v1, NOW);
    assert_eq!(first.added, 2);
    assert_eq!(run_members(&v1, 1), vec!["tok-a", "tok-b", "tok-c"]);
    let run = count(&v1, "SELECT id FROM leagues WHERE run_key = '0x:1'");

    // Made in v1: tok-a out of every league, tok-b deleted, tok-x moved into
    // the run, the run deleted with a status, the manual league deleted.
    v1.execute_batch(&format!(
        "UPDATE matches SET no_league = 1, league_linked = 0, league_id = NULL, edited_at = 50
             WHERE token = 'tok-a';
         UPDATE matches SET hidden_at = 60, edited_at = 60 WHERE token = 'tok-b';
         UPDATE matches SET league_id = {run}, league_linked = 1, edited_at = 70
             WHERE token = 'tok-x';
         UPDATE leagues SET removed_at = 80, user_state = 'complete', edited_at = 80
             WHERE id = {run};
         UPDATE leagues SET removed_at = 81, edited_at = 81 WHERE source = 'manual';"
    ))
    .expect("player edits");
    let player_columns = |v1: &Connection| {
        texts(
            v1,
            "SELECT token || ':' || no_league || ':' || league_linked || ':'
                 || ifnull(league_id, '-') || ':' || ifnull(hidden_at, '-') || ':'
                 || ifnull(edited_at, '-')
             FROM matches ORDER BY token",
        )
    };
    let league_columns = |v1: &Connection| {
        texts(
            v1,
            "SELECT source || ':' || ifnull(removed_at, '-') || ':' || ifnull(user_state, '-')
                 || ':' || ifnull(edited_at, '-')
             FROM leagues ORDER BY source",
        )
    };
    let matches_before = player_columns(&v1);
    let leagues_before = league_columns(&v1);

    let (_, again) = import_all(&path, &mut v1, NOW + 1);
    assert_eq!(
        again,
        LeagueImportCounts {
            unchanged: 2,
            ..LeagueImportCounts::default()
        },
        "the player's matches are left out of the comparison"
    );
    assert_eq!(player_columns(&v1), matches_before);
    assert_eq!(league_columns(&v1), leagues_before);

    // 0.x moves on: the run completes and gains a match, the manual league
    // is renamed.
    conn.execute_batch(&format!(
        "UPDATE leagues SET state = 'complete', completed_at = '{ENDED}' WHERE id = 1;
         UPDATE leagues SET name = 'Q Renamed' WHERE id = 2;"
    ))
    .expect("0.x changes");
    add_match(&conn, 6, "tok-f", "complete", Some(1));

    let (_, changed) = import_all(&path, &mut v1, NOW + 2);
    assert_eq!(
        changed,
        LeagueImportCounts {
            updated: 2,
            ..LeagueImportCounts::default()
        }
    );
    assert_eq!(
        run_members(&v1, 1),
        vec!["tok-b", "tok-c", "tok-f", "tok-x"],
        "the player's matches stay where the player put them"
    );
    assert_eq!(
        text(
            &v1,
            "SELECT no_league || ':' || league_linked || ':' || edited_at FROM matches
             WHERE token = 'tok-a'"
        ),
        Some("1:0:50".to_string()),
        "a match out of every league is never relinked"
    );
    assert_eq!(
        league_columns(&v1),
        leagues_before,
        "the import never writes removed_at, user_state or edited_at"
    );
    assert_eq!(
        count(&v1, "SELECT COUNT(*) FROM leagues WHERE source = 'manual'"),
        1,
        "a removed manual league is found again, not added"
    );
    assert_eq!(
        text(&v1, "SELECT name FROM leagues WHERE source = 'manual'"),
        Some("Q Renamed".to_string())
    );
}

// Review focus 1: a tester's all-imported database. A run deleted in v1
// with its matches kept stays deleted after a restart and a 0.x rerun, and
// its kept matches still read as non-league matches.
#[test]
fn a_reimport_keeps_a_removed_run_removed() {
    use store::leagues::{DeleteMatches, LeagueEdit, LeagueEditOutcome, edit_league};
    let dir = TestDir::new("import-leagues-removed-run");
    let (path, conn) = legacy_db(&dir, &full_schema());
    add_basics(&conn);
    add_league(&conn, league(1, "q-league-1"));
    add_match(&conn, 1, "tok-a", "complete", Some(1));
    add_match(&conn, 2, "tok-b", "complete", Some(1));

    let mut v1 = v1_db(&dir);
    import_all(&path, &mut v1, NOW);
    sync(&v1);
    assert_eq!(run_members(&v1, 1), vec!["tok-a", "tok-b"]);
    let delete = LeagueEdit::Delete {
        run_key: "0x:1".to_string(),
        matches: DeleteMatches::KeepAsNonLeague,
        now_ms: NOW + 10,
    };
    assert_eq!(
        edit_league(&mut v1, &delete, chrono_tz::UTC).expect("delete"),
        LeagueEditOutcome::Done
    );

    // A restart: the database opens again, then the import and the league
    // sync run again.
    drop(v1);
    let mut v1 = v1_db(&dir);
    import_all(&path, &mut v1, NOW + 20);
    sync(&v1);

    assert_eq!(
        count(&v1, "SELECT removed_at FROM leagues WHERE run_key = '0x:1'"),
        NOW + 10
    );
    assert_eq!(
        count(&v1, "SELECT COUNT(*) FROM leagues"),
        1,
        "found again, not added"
    );
    assert_eq!(
        run_members(&v1, 1),
        vec!["tok-a", "tok-b"],
        "the kept matches keep their league id"
    );
    let local = account_id(&v1, "Qlocal");
    assert!(
        runs_of(
            &v1,
            LeagueOwner::Account(local),
            &[LeagueKind::Constructed],
            "imported"
        )
        .is_empty(),
        "no league read lists it"
    );
    for token in ["tok-a", "tok-b"] {
        let header = store::match_detail::match_header(&v1, token)
            .expect("read")
            .expect("listed");
        assert!(!header.in_league, "{token} reads as a non-league match");
    }
}
