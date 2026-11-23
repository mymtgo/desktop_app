//! The SQLite store (spec section 4): schema, migrations, the writer thread, read
//! queries, and per-match compaction. One writer connection lives on the writer
//! thread; everything else reads through read-only connections (WAL mode).

pub mod accounts;
pub mod api_reads;
pub mod archetype_edit;
pub mod archetypes;
pub mod arena;
pub mod arena_courses;
pub mod arena_game_sync;
pub mod arena_games;
pub mod arena_rank;
pub mod arena_replay;
pub mod arena_run_sync;
pub mod card_stats;
pub mod cards;
pub mod compact;
pub mod console;
pub mod dashboard;
pub mod deck_index;
pub mod deck_pages;
pub mod decks;
pub mod deleted;
pub mod endstep_captures;
pub mod endstep_cards;
pub mod endstep_replay;
pub mod endstep_sync;
pub mod game_stats;
pub mod game_sync;
pub mod guides;
pub mod history_check;
pub mod history_import;
pub mod import;
pub mod import_api;
pub mod import_archetypes;
pub mod import_decks;
pub mod import_leagues;
pub mod import_limited;
pub mod import_overlay;
pub mod imported_detail;
pub mod league_sync;
pub mod leagues;
pub mod limited;
pub mod manual;
pub mod manual_rules;
pub mod match_commanders;
pub mod match_detail;
pub mod match_edits;
pub mod match_list;
pub mod matchups;
pub mod metagame;
pub mod names;
pub mod opponents;
pub mod outbox;
pub mod overlay;
pub mod platform;
pub mod read;
pub mod recorded_replays;
pub mod replay;
mod replay_shared;
pub mod sync;
mod sync_player;
pub mod sync_sources;
pub mod synced_sources;
pub mod tournament_sync;
pub mod write;
pub mod writer;

#[cfg(test)]
mod test_support;

use std::fmt;
use std::path::Path;
use std::time::Duration;

use rusqlite::OpenFlags;
use rusqlite_migration::{M, Migrations};

pub use platform::{BestOf, Platform};
pub use rusqlite::{Connection, Params, Transaction, TransactionBehavior, params};

#[derive(Debug)]
pub enum StoreError {
    Sqlite(rusqlite::Error),
    Migration(rusqlite_migration::Error),
    Io(std::io::Error),
    /// Stored data that does not parse (bad hash text, bad date, bad archive).
    Corrupt(String),
    /// The database cannot run here as designed (the file system does not
    /// support WAL mode).
    Unsupported(String),
    /// A batch failed earlier on the writer thread; nothing after it was written.
    Failed(String),
    /// The writer thread has stopped.
    WriterGone,
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StoreError::Sqlite(error) => write!(f, "sqlite: {error}"),
            StoreError::Migration(error) => write!(f, "migration: {}", migration_text(error)),
            StoreError::Io(error) => write!(f, "io: {error}"),
            StoreError::Corrupt(what) => write!(f, "corrupt data: {what}"),
            StoreError::Unsupported(what) => write!(f, "unsupported: {what}"),
            StoreError::Failed(what) => write!(f, "an earlier write failed: {what}"),
            StoreError::WriterGone => write!(f, "the writer thread has stopped"),
        }
    }
}

impl std::error::Error for StoreError {}

/// A failed migration query by its first line (each migration file opens
/// with a comment naming it) and SQLite's extended code, which tells apart
/// failures that share one message (a read-only database: a moved file, a
/// `-shm` file it cannot write, a read-only folder). The whole query, often
/// thousands of characters, is left out.
fn migration_text(error: &rusqlite_migration::Error) -> String {
    let rusqlite_migration::Error::RusqliteError { query, err } = error else {
        return error.to_string();
    };
    let first_line = query
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("");
    let first_line: String = first_line.chars().take(120).collect();
    match err.sqlite_error() {
        Some(code) => format!(
            "{err} (SQLite code {}) in the query starting `{first_line}`",
            code.extended_code
        ),
        None => format!("{err} in the query starting `{first_line}`"),
    }
}

impl From<rusqlite::Error> for StoreError {
    fn from(error: rusqlite::Error) -> Self {
        StoreError::Sqlite(error)
    }
}

impl From<rusqlite_migration::Error> for StoreError {
    fn from(error: rusqlite_migration::Error) -> Self {
        StoreError::Migration(error)
    }
}

impl From<std::io::Error> for StoreError {
    fn from(error: std::io::Error) -> Self {
        StoreError::Io(error)
    }
}

impl From<serde_json::Error> for StoreError {
    fn from(error: serde_json::Error) -> Self {
        StoreError::Corrupt(error.to_string())
    }
}

/// The baseline schema (target schema spec section 3): one file per domain,
/// concatenated in this order into the first migration. Frozen at
/// 1.0.0-beta.2, the first release testers installed (issue #311): never edit
/// these files; change the schema with a new migration in `LATER_MIGRATIONS`.
const BASELINE: &str = concat!(
    include_str!("schema/log.sql"),
    include_str!("schema/registries.sql"),
    include_str!("schema/cards_decks.sql"),
    include_str!("schema/matches.sql"),
    include_str!("schema/leagues.sql"),
    include_str!("schema/limited.sql"),
    include_str!("schema/archetypes.sql"),
    include_str!("schema/app.sql"),
    include_str!("schema/sync.sql"),
);

/// The forward-only migrations after the baseline, oldest first (foundation
/// spec 4.1 rule 8). To change the schema, add a file under `migrations/`
/// (`NNNN_what_it_does.sql`, numbered from 0002) and its `include_str!` at
/// the end of this list; never edit or reorder one that has shipped. Test it
/// on a database built to the version before it
/// (`migrations().to_version(&mut conn, n)`) with rows in it, and refresh the
/// schema snapshot.
const LATER_MIGRATIONS: &[&str] = &[
    // League self-management: player columns on matches and leagues, the
    // listed_leagues view, existing drops kept as the Dropped status.
    include_str!("migrations/0002_league_self_management.sql"),
    // The league a deleted match was in: matches.deleted_from_league_id,
    // recorded by triggers when hidden_at is set or a hidden match gets a
    // league, and cleared on restore.
    include_str!("migrations/0003_deleted_from_league.sql"),
    // A stable league sync id: leagues.sync_client_id, backfilled with the
    // 0.x formula and filled by triggers on insert (never recomputed, so a
    // rekey keeps it); and sync_state.download_cursor, the server's cursor.
    include_str!("migrations/0004_sync_identity_and_cursor.sql"),
    // The player's fix of a match: matches.corrected_at, which keeps the
    // projection and the import from writing over it.
    include_str!("migrations/0005_match_corrections.sql"),
    // MTGO's verdict on a log match from its game history:
    // matches.history_outcome, which the projection's upsert writes as the
    // outcome while it is set.
    include_str!("migrations/0006_match_history_outcome.sql"),
    // League run identity: triggers that drop this device's queued or sent
    // match report when the league id it named changes (a league's
    // sync_client_id, a match's league_id), so the next enqueue reports the
    // match again with the new id.
    include_str!("migrations/0007_report_league_client_id.sql"),
    // Card ids the API does not know: cards.api_unknown_at, so population
    // passes over them for a while instead of asking every run.
    include_str!("migrations/0008_card_api_unknown.sql"),
    // The offline archetype download: archetypes.offline_held, the server
    // archetypes an offline player holds as Custom rows, turned back into
    // downloaded rows when they go online.
    include_str!("migrations/0009_offline_held_archetypes.sql"),
    // Arena tracking: accounts and opponents get a platform (and the Arena
    // user id, the opponent's screen name); the Arena log files, their
    // decoded events and rank observations get tables.
    include_str!("migrations/0010_arena.sql"),
    // Arena rank: where a manual Arena match sits in the rank history.
    include_str!("migrations/0011_arena_rank_gap_claims.sql"),
    // Arena game data: the raw game messages of a match, their archive
    // blob once the match is over, and the game sync's stamp per match.
    include_str!("migrations/0012_arena_game_messages.sql"),
    // The rank a ranked Arena match was played at: matches.rank_file_id and
    // rank_byte_offset name its constructed reading, by the rule in the
    // match_rank_readings view; existing ranked Arena matches are filled.
    include_str!("migrations/0013_match_rank.sql"),
    // An Arena account known only by its user id (`arena:<user id>`, trap
    // 29) shows its untagged screen name: accounts.display_name.
    include_str!("migrations/0014_arena_account_display_name.sql"),
    // Arena limited runs: arena_course_observations (each courses response's
    // limited courses per account) and arena_match_events (each Arena
    // match's event name).
    include_str!("migrations/0015_arena_courses.sql"),
    // Recorded MTGO replays: recorded_replays (each imported .mtgr file) and
    // recorded_replay_games; matches link to them at read time.
    include_str!("migrations/0016_recorded_replays.sql"),
    // A deck's own cover image: decks.cover_file, a file in the app data
    // folder. Local only, never synced.
    include_str!("migrations/0017_deck_cover_file.sql"),
    // Arena formats key like the server's: archetypes.format `arena_historic`
    // becomes `historic`.
    include_str!("migrations/0018_arena_format_keys.sql"),
    // A commander match's commanders per seat: match_commanders.
    include_str!("migrations/0019_match_commanders.sql"),
    // Commander archetypes: archetypes.source gains 'commander' (the table is
    // rebuilt, ids and rows kept).
    include_str!("migrations/0020_archetype_commander_source.sql"),
    // Endstep captures: endstep_captures (each imported capture file, kept
    // whole as zstd).
    include_str!("migrations/0021_endstep_captures.sql"),
    // The Endstep platform: accounts and opponents are rebuilt so their
    // platform CHECK takes 'endstep' (ids, columns and indexes kept; the
    // match_rank_readings view is dropped and created again around it).
    include_str!("migrations/0022_endstep_platform.sql"),
    // Endstep card identity: endstep_cards (each card name and printing
    // seen; catalog id = ENDSTEP_ID_OFFSET + row id).
    include_str!("migrations/0023_endstep_cards.sql"),
    // Endstep sync bookkeeping: endstep_captures.synced_version, each unit's
    // refs (endstep_unit_refs) and each ranked match's rating change
    // (endstep_ranked_results).
    include_str!("migrations/0024_endstep_sync.sql"),
    // League entries: leagues.rounds (each run's stated length) and the
    // leagues index covering join requests.
    include_str!("migrations/0025_league_rounds.sql"),
    // Cross-device sync of Arena and Endstep: synced_sources (a pulled
    // match's raw source) and synced_arena_runs (a pulled Arena run); Arena
    // course runs take `arena_<course id>` as their sync client id and the
    // 0004 triggers no longer fill Arena runs.
    include_str!("migrations/0026_cross_device_sync.sql"),
    // Parked pulls (#830): synced_rows.parked_hash and parked_reason, a
    // pulled row this device cannot apply yet, so it is not fetched again
    // until its server copy changes or an Arena account exists.
    include_str!("migrations/0027_parked_pulls.sql"),
    // The event overlay's tournaments: tournament_standings (each joined
    // tournament's standing), tournament_rounds (its decoded round info) and
    // tournament_player_states (the InPlayoffs facts). Per device, never
    // synced, a 48-hour window: bare tables, no backfill.
    include_str!("migrations/0028_tournament_standings.sql"),
];

/// Embedded migrations: the baseline, then `LATER_MIGRATIONS`. The runner
/// checks foreign keys before it commits each one.
fn migrations() -> Migrations<'static> {
    let mut list = vec![M::up(BASELINE).foreign_key_check()];
    list.extend(
        LATER_MIGRATIONS
            .iter()
            .map(|sql| M::up(sql).foreign_key_check()),
    );
    Migrations::new(list)
}

/// The `user_version` a database has once this build has migrated it.
fn migration_count() -> i64 {
    1 + LATER_MIGRATIONS.len() as i64
}

/// Open (creating if needed) the database for writing and bring the schema up to
/// date. Only the writer thread holds one of these.
///
/// `PRAGMA user_version` says how many migrations a database has had; the
/// runner applies the rest. One that a newer version has migrated further
/// than this build knows is refused, untouched (the app never deletes or
/// rewrites a file it cannot read).
pub fn open_writer(path: &Path) -> Result<Connection, StoreError> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
        if !dir.as_os_str().is_empty() {
            make_private(dir, PRIVATE_FOLDER_MODE);
        }
    }
    let mut conn = Connection::open(path)?;
    // Before WAL mode is on: SQLite gives the `-wal` and `-shm` files it
    // creates the database file's mode.
    make_private(path, PRIVATE_FILE_MODE);
    conn.busy_timeout(Duration::from_secs(5))?;
    // Read before anything writes: switching to WAL changes the file header.
    let user_version: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if user_version > migration_count() {
        return Err(StoreError::Unsupported(format!(
            "{} was made by a newer version of MyMTGO; update MyMTGO to open it",
            path.display()
        )));
    }
    let mode: String = conn.query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))?;
    if !mode.eq_ignore_ascii_case("wal") {
        return Err(StoreError::Unsupported(format!(
            "the file system does not support SQLite WAL mode (journal mode is {mode})"
        )));
    }
    // Migrations run with foreign keys off, as SQLite's procedure for
    // rebuilding a table asks (with them on, dropping a table that others
    // reference cascades). The pragma is a no-op inside a transaction and the
    // runner wraps the pending migrations in one, so it is set around the
    // runner. The runner checks the foreign keys before it commits.
    conn.execute_batch("PRAGMA foreign_keys = OFF;")?;
    migrations().to_latest(&mut conn)?;
    conn.execute_batch("PRAGMA foreign_keys = ON; PRAGMA synchronous = NORMAL;")?;
    cards::create_stub_count(&conn)?;
    // Files left by an earlier build, before their modes were set.
    for suffix in ["-wal", "-shm"] {
        let mut name = path.as_os_str().to_os_string();
        name.push(suffix);
        make_private(Path::new(&name), PRIVATE_FILE_MODE);
    }
    Ok(conn)
}

/// The data folder's mode on unix: only the user may list or enter it.
pub const PRIVATE_FOLDER_MODE: u32 = 0o700;
/// The database files' mode on unix: only the user may read or write them.
pub const PRIVATE_FILE_MODE: u32 = 0o600;

/// Set `path` to `mode` on unix, so other users on the machine cannot read
/// the match history (nothing on other systems). A missing file is skipped;
/// a file system that refuses the change is logged, never fatal.
pub fn make_private(path: &Path, mode: u32) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        match std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                tracing::warn!(%error, path = %path.display(), "could not make a file private");
            }
        }
    }
    #[cfg(not(unix))]
    let _ = (path, mode);
}

/// A read-only connection. Open the writer first: it creates the file and schema.
pub fn open_reader(path: &Path) -> Result<Connection, StoreError> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    conn.busy_timeout(Duration::from_secs(5))?;
    conn.execute_batch("PRAGMA foreign_keys = ON;")?;
    Ok(conn)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::number as count;
    use fixture_tools::TestDir;
    use mtgo_protocol::EventKind;
    use rusqlite::params;

    #[test]
    fn a_failed_migration_names_its_first_line_and_sqlite_code_not_the_whole_query() {
        let error = StoreError::Migration(rusqlite_migration::Error::RusqliteError {
            query: "\n-- Cross-device sync of Arena and Endstep (#776).\nCREATE TABLE t (id INTEGER);\n"
                .to_string(),
            err: rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(1032),
                Some("attempt to write a readonly database".to_string()),
            ),
        });
        assert_eq!(
            error.to_string(),
            "migration: attempt to write a readonly database (SQLite code 1032) \
             in the query starting `-- Cross-device sync of Arena and Endstep (#776).`"
        );
    }

    /// SQL text with comments dropped and whitespace collapsed, so how a
    /// statement was laid out (or which migration wrote it) does not count.
    fn normalise_sql(sql: &str) -> String {
        let mut words = Vec::new();
        for line in sql.lines() {
            let code = match line.find("--") {
                Some(start) => &line[..start],
                None => line,
            };
            for word in code.split_whitespace() {
                words.push(word.to_string());
            }
        }
        words.join(" ").replace("( ", "(").replace(" )", ")")
    }

    /// Every `CHECK (...)` expression in a normalised `CREATE TABLE`.
    fn check_constraints(sql: &str) -> Vec<String> {
        let upper = sql.to_ascii_uppercase();
        let bytes = sql.as_bytes();
        let mut found = Vec::new();
        let mut from = 0;
        while let Some(at) = upper[from..].find("CHECK") {
            let start = from + at;
            from = start + "CHECK".len();
            let before_ok = start == 0 || !bytes[start - 1].is_ascii_alphanumeric();
            if !before_ok {
                continue;
            }
            let mut open = from;
            while open < bytes.len() && bytes[open] == b' ' {
                open += 1;
            }
            if open >= bytes.len() || bytes[open] != b'(' {
                continue;
            }
            let mut depth = 0;
            let mut close = open;
            while close < bytes.len() {
                match bytes[close] {
                    b'(' => depth += 1,
                    b')' => {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    _ => {}
                }
                close += 1;
            }
            found.push(sql[open..=close].to_string());
            from = close;
        }
        found
    }

    fn strings(conn: &Connection, sql: &str, param: &str) -> Vec<Vec<String>> {
        let mut stmt = conn.prepare(sql).expect(sql);
        let columns = stmt.column_count();
        let rows = stmt
            .query_map([param], |row| {
                let mut values = Vec::new();
                for index in 0..columns {
                    let value: rusqlite::types::Value = row.get(index)?;
                    values.push(match value {
                        rusqlite::types::Value::Null => "NULL".to_string(),
                        rusqlite::types::Value::Integer(number) => number.to_string(),
                        rusqlite::types::Value::Text(text) => text,
                        other => format!("{other:?}"),
                    });
                }
                Ok(values)
            })
            .expect(sql);
        rows.map(|row| row.expect("row")).collect()
    }

    /// Every table, view and trigger as SQLite reports it: columns, STRICT,
    /// CHECKs, foreign keys, indexes (with their columns and partial WHERE)
    /// and view SQL, normalised and sorted, so two schemas built by different
    /// statements compare equal when SQLite sees the same thing.
    fn schema_description(conn: &Connection) -> String {
        let mut out = Vec::new();
        let objects = strings(
            conn,
            "SELECT type, name, sql FROM sqlite_master
             WHERE type IN ('table', 'view', 'trigger') AND name NOT LIKE 'sqlite_%'
               AND ?1 = ?1
             ORDER BY name",
            "",
        );
        for object in objects {
            let (kind, name, sql) = (&object[0], &object[1], normalise_sql(&object[2]));
            if kind == "trigger" {
                out.push(format!("trigger {name}: {sql}"));
                continue;
            }
            if kind == "view" {
                out.push(format!("view {name}: {sql}"));
            } else {
                let list = strings(
                    conn,
                    "SELECT strict, wr FROM pragma_table_list WHERE schema = 'main' AND name = ?1",
                    name,
                );
                // AUTOINCREMENT shows only in the table's SQL (and the hidden
                // `sqlite_sequence` table), so it is read from there.
                let autoincrement = if sql.to_uppercase().contains("AUTOINCREMENT") {
                    " autoincrement=1"
                } else {
                    ""
                };
                out.push(format!(
                    "table {name} strict={} without_rowid={}{autoincrement}",
                    list[0][0], list[0][1]
                ));
            }
            for column in strings(
                conn,
                "SELECT cid, name, type, \"notnull\", dflt_value, pk
                 FROM pragma_table_info(?1) ORDER BY cid",
                name,
            ) {
                out.push(format!(
                    "  column {} {} {} notnull={} default={} pk={}",
                    column[0], column[1], column[2], column[3], column[4], column[5]
                ));
            }
            if kind == "view" {
                continue;
            }
            let mut checks = check_constraints(&sql);
            checks.sort();
            for check in checks {
                out.push(format!("  check {check}"));
            }
            let mut keys = Vec::new();
            for key in strings(
                conn,
                "SELECT id, group_concat(\"from\", ', '), \"table\", group_concat(\"to\", ', '),
                     on_update, on_delete, \"match\"
                 FROM (SELECT * FROM pragma_foreign_key_list(?1) ORDER BY id, seq)
                 GROUP BY id",
                name,
            ) {
                keys.push(format!(
                    "  foreign key ({}) -> {} ({}) on update {} on delete {} match {}",
                    key[1], key[2], key[3], key[4], key[5], key[6]
                ));
            }
            keys.sort();
            out.extend(keys);
            let mut indexes = Vec::new();
            for index in strings(
                conn,
                "SELECT name, \"unique\", origin, partial FROM pragma_index_list(?1)",
                name,
            ) {
                let columns: Vec<String> = strings(
                    conn,
                    "SELECT coalesce(name, '<expr>') FROM pragma_index_info(?1) ORDER BY seqno",
                    &index[0],
                )
                .into_iter()
                .map(|row| row[0].clone())
                .collect();
                let index_sql = strings(
                    conn,
                    "SELECT coalesce(sql, '') FROM sqlite_master WHERE type = 'index' AND name = ?1",
                    &index[0],
                );
                let index_sql = normalise_sql(&index_sql[0][0]);
                let mut line = format!(
                    "  index {} unique={} origin={} partial={} on ({})",
                    index[0],
                    index[1],
                    index[2],
                    index[3],
                    columns.join(", ")
                );
                if !index_sql.is_empty() {
                    line.push_str(&format!(": {index_sql}"));
                }
                indexes.push(line);
            }
            indexes.sort();
            out.extend(indexes);
        }
        out.join("\n")
    }

    #[test]
    fn the_schema_snapshot() {
        let dir = TestDir::new("store-baseline-schema");
        let conn = open_writer(&dir.join("mymtgo.sqlite")).expect("open");
        insta::assert_snapshot!(schema_description(&conn));
    }

    /// A database file with only a header: a `user_version` and one table.
    fn old_database(path: &Path, user_version: i64) -> Vec<u8> {
        {
            let conn = Connection::open(path).expect("open");
            conn.execute_batch(&format!(
                "CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL) STRICT;
                 PRAGMA user_version = {user_version};"
            ))
            .expect("old database");
        }
        std::fs::read(path).expect("read the file")
    }

    /// A copy of the beta.2 fixture: the baseline built by SQLite's own
    /// shell from the v1.0.0-beta.2 schema files, in WAL mode, with one
    /// settings row.
    fn beta_2_database(dir: &TestDir) -> std::path::PathBuf {
        let path = dir.join("mymtgo.sqlite");
        std::fs::copy(
            concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/beta2.sqlite"),
            &path,
        )
        .expect("copy the fixture");
        path
    }

    #[test]
    fn a_beta_2_database_opens_and_reaches_this_builds_schema() {
        let dir = TestDir::new("store-beta-2");
        let conn = open_writer(&beta_2_database(&dir)).expect("a beta.2 database opens");
        assert_eq!(
            count(&conn, "PRAGMA user_version"),
            migration_count(),
            "every later migration ran"
        );
        let value: String = conn
            .query_row(
                "SELECT value FROM settings WHERE key = 'overlay_artwork'",
                [],
                |row| row.get(0),
            )
            .expect("the row survives");
        assert_eq!(value, "\"deck\"");

        let fresh_dir = TestDir::new("store-beta-2-fresh");
        let fresh = open_writer(&fresh_dir.join("mymtgo.sqlite")).expect("open");
        assert_eq!(
            schema_description(&conn),
            schema_description(&fresh),
            "a migrated beta.2 database has the schema a fresh one has; the baseline \
             (crates/store/src/schema/) is frozen at 1.0.0-beta.2: undo any edit to it and \
             add a migration to LATER_MIGRATIONS instead"
        );
    }

    /// A database built to the baseline only (migration 1) with rows in it:
    /// a log run the player dropped, an active log run, a manual league, a
    /// match in the first run and a hidden manual match linked to the
    /// manual league.
    fn baseline_with_rows(path: &Path) {
        let mut conn = Connection::open(path).expect("open");
        migrations().to_version(&mut conn, 1).expect("the baseline");
        conn.execute_batch(
            "INSERT INTO accounts (id, username, first_seen_at) VALUES (1, 'Qlocal', 0);
             INSERT INTO leagues (id, run_key, account_id, source, kind, state, token, name,
                 started_at, created_at, dropped_at, user_dropped_at)
             VALUES (1, 'q-l1:q-m1', 1, 'log', 'constructed', 'dropped', 'q-l1', 'Q one',
                     10, 11, 50, 50),
                    (2, 'q-l2:q-m2', 1, 'log', 'constructed', 'active', 'q-l2', 'Q two',
                     20, 21, NULL, NULL),
                    (4, 'q-l4:q-m4', 1, 'log', 'constructed', 'complete', 'q-l4', 'Q four',
                     40, 41, NULL, 60);
             INSERT INTO leagues (id, run_key, account_id, source, kind, state, token, name,
                 started_at, created_at)
             VALUES (3, 'manual:3', 1, 'manual', 'constructed', 'complete',
                 'manual_Q00000000000000000000003', 'Q hand', 30, 31);
             INSERT INTO matches (token, account_id, source, format, match_type, state,
                 outcome, started_at, last_activity_at, revision, derived_version, league_id,
                 league_linked, hidden_at)
             VALUES ('q-m1', 1, 'log', 'CMODERN', 'Modern', 'complete', 'win', 10, 12, 1, 4,
                     1, 0, NULL),
                    ('q-m3', 1, 'manual', 'CMODERN', 'Modern', 'complete', 'loss', 30, 32, 1,
                     0, 3, 1, 99);",
        )
        .expect("rows");
    }

    #[test]
    fn the_league_self_management_migration_keeps_rows_and_carries_drops_over() {
        let dir = TestDir::new("store-migration-0002");
        let path = dir.join("mymtgo.sqlite");
        baseline_with_rows(&path);
        let old_leagues = "SELECT id, run_key, state, dropped_at, user_dropped_at, notes
             FROM leagues WHERE ?1 = ?1 ORDER BY id";
        let old_matches = "SELECT token, league_id, league_linked, hidden_at
             FROM matches WHERE ?1 = ?1 ORDER BY id";
        let (leagues_before, matches_before) = {
            let conn = Connection::open(&path).expect("open");
            (
                strings(&conn, old_leagues, ""),
                strings(&conn, old_matches, ""),
            )
        };

        let conn = open_writer(&path).expect("the migration runs");
        assert_eq!(count(&conn, "PRAGMA user_version"), migration_count());
        assert_eq!(strings(&conn, old_leagues, ""), leagues_before);
        assert_eq!(strings(&conn, old_matches, ""), matches_before);
        assert_eq!(
            strings(
                &conn,
                "SELECT id, removed_at, user_state, edited_at FROM leagues
                 WHERE ?1 = ?1 ORDER BY id",
                ""
            ),
            vec![
                vec!["1", "NULL", "dropped", "NULL"],
                vec!["2", "NULL", "NULL", "NULL"],
                vec!["3", "NULL", "NULL", "NULL"],
                vec!["4", "NULL", "NULL", "NULL"],
            ],
            "only a run the fold dropped gets the Dropped status, not a Complete run holding an ignored drop"
        );
        assert_eq!(
            strings(
                &conn,
                "SELECT token, no_league, edited_at FROM matches WHERE ?1 = ?1 ORDER BY id",
                ""
            ),
            vec![vec!["q-m1", "0", "NULL"], vec!["q-m3", "0", "NULL"]]
        );

        assert_eq!(count(&conn, "SELECT COUNT(*) FROM listed_leagues"), 4);
        conn.execute("UPDATE leagues SET removed_at = 7 WHERE id = 2", [])
            .expect("remove");
        assert_eq!(
            count(&conn, "SELECT COUNT(*) FROM listed_leagues"),
            3,
            "the view leaves removed leagues out"
        );
        assert!(
            conn.execute("UPDATE matches SET no_league = 2 WHERE token = 'q-m1'", [])
                .is_err(),
            "no_league is 0 or 1"
        );
        assert!(
            conn.execute("UPDATE leagues SET user_state = 'gone' WHERE id = 1", [])
                .is_err(),
            "user_state is a run state or NULL"
        );
        conn.execute("UPDATE leagues SET user_state = 'partial' WHERE id = 1", [])
            .expect("a run state is allowed");
    }

    /// A database built to migration 2 with rows in it: two leagues of
    /// Qlocal deleted at 50 (ids 1 and 2), one of another account deleted at
    /// 50 (id 4), a listed league (id 3), and matches hidden in a league
    /// (`q-in`), hidden in none at those leagues' deletion (`q-with`), hidden
    /// in none at another time (`q-alone`), hidden at 50 in a listed league
    /// (`q-in-at`) and listed in a league (`q-listed`).
    fn version_2_with_rows(path: &Path) {
        let mut conn = Connection::open(path).expect("open");
        migrations().to_version(&mut conn, 2).expect("migration 2");
        conn.execute_batch(
            "INSERT INTO accounts (id, username, first_seen_at)
             VALUES (1, 'Qlocal', 0), (2, 'Qother', 0);
             INSERT INTO leagues (id, run_key, account_id, source, kind, state, token, name,
                 started_at, created_at, removed_at)
             VALUES (1, 'q-l1:q-in', 1, 'log', 'constructed', 'complete', 'q-l1', 'Q one',
                     10, 11, 50),
                    (2, 'q-l2:q-x', 1, 'log', 'constructed', 'complete', 'q-l2', 'Q two',
                     20, 21, 50),
                    (3, 'q-l3:q-listed', 1, 'log', 'constructed', 'active', 'q-l3', 'Q three',
                     30, 31, NULL),
                    (4, 'q-l4:q-y', 2, 'log', 'constructed', 'complete', 'q-l4', 'Q four',
                     40, 41, 50);
             INSERT INTO matches (token, account_id, source, format, match_type, state,
                 outcome, started_at, last_activity_at, revision, derived_version, league_id,
                 hidden_at)
             VALUES ('q-in', 1, 'log', 'CMODERN', 'Modern', 'complete', 'win', 10, 12, 1, 4,
                     1, 45),
                    ('q-with', 1, 'log', 'CMODERN', 'Modern', 'complete', 'win', 20, 22, 1, 4,
                     NULL, 50),
                    ('q-alone', 1, 'log', 'CMODERN', 'Modern', 'complete', 'loss', 25, 26, 1, 4,
                     NULL, 60),
                    ('q-in-at', 1, 'log', 'CMODERN', 'Modern', 'complete', 'loss', 30, 32, 1,
                     4, 3, 50),
                    ('q-listed', 1, 'log', 'CMODERN', 'Modern', 'complete', 'win', 35, 36, 1,
                     4, 3, NULL);",
        )
        .expect("rows");
    }

    fn recorded_leagues(conn: &Connection) -> Vec<Vec<String>> {
        strings(
            conn,
            "SELECT token, ifnull(deleted_from_league_id, '-') FROM matches
             WHERE ?1 = ?1 ORDER BY id",
            "",
        )
    }

    #[test]
    fn the_deleted_from_league_migration_fills_hidden_rows_and_records_later_hides() {
        let dir = TestDir::new("store-migration-0003");
        let path = dir.join("mymtgo.sqlite");
        version_2_with_rows(&path);
        let old_matches = "SELECT token, league_id, hidden_at FROM matches
             WHERE ?1 = ?1 ORDER BY id";
        let matches_before = {
            let conn = Connection::open(&path).expect("open");
            strings(&conn, old_matches, "")
        };

        let conn = open_writer(&path).expect("the migration runs");
        assert_eq!(count(&conn, "PRAGMA user_version"), migration_count());
        assert_eq!(strings(&conn, old_matches, ""), matches_before);
        assert_eq!(
            recorded_leagues(&conn),
            vec![
                vec!["q-in", "1"],
                vec!["q-with", "-"],
                vec!["q-alone", "-"],
                vec!["q-in-at", "3"],
                vec!["q-listed", "-"],
            ],
            "its league_id; a match in no league is in none, whatever its time"
        );

        // From now on the triggers record it.
        conn.execute_batch(
            "UPDATE matches SET hidden_at = 70 WHERE token = 'q-listed';
             UPDATE matches SET hidden_at = NULL WHERE token = 'q-in';
             UPDATE matches SET hidden_at = 80 WHERE token = 'q-with';
             INSERT INTO matches (token, account_id, source, format, match_type, state,
                 outcome, started_at, last_activity_at, revision, derived_version, league_id,
                 hidden_at)
             VALUES ('q-new', 1, 'log', 'CMODERN', 'Modern', 'complete', 'win', 90, 91, 1, 4,
                     3, 95);",
        )
        .expect("writes");
        assert_eq!(
            recorded_leagues(&conn),
            vec![
                vec!["q-in", "-"],
                vec!["q-with", "-"],
                vec!["q-alone", "-"],
                vec!["q-in-at", "3"],
                vec!["q-listed", "3"],
                vec!["q-new", "3"],
            ],
            "a hide records, a restore clears, a second hide keeps, a hidden insert records"
        );
        // A league change while hidden: a new league is recorded (a pulled
        // link); a cleared league (the league sync) changes nothing.
        conn.execute_batch(
            "UPDATE matches SET league_id = 1 WHERE token = 'q-in-at';
             UPDATE matches SET league_id = NULL WHERE token = 'q-listed';
             UPDATE matches SET league_id = 1 WHERE token = 'q-in';",
        )
        .expect("league changes");
        assert_eq!(
            recorded_leagues(&conn),
            vec![
                vec!["q-in", "-"],
                vec!["q-with", "-"],
                vec!["q-alone", "-"],
                vec!["q-in-at", "1"],
                vec!["q-listed", "3"],
                vec!["q-new", "3"],
            ],
            "only a hidden match's new league is recorded"
        );
        conn.execute("DELETE FROM leagues WHERE id = 1", [])
            .expect("a league row goes");
        assert_eq!(
            recorded_leagues(&conn)[3],
            vec!["q-in-at", "-"],
            "a deleted league row clears it"
        );
    }

    /// Starts that the SQL formula must agree with `league_client_id` on: a
    /// start with milliseconds just before a second boundary, one before
    /// 2001 (nine digit unix seconds) and one before 1970 with milliseconds
    /// (Rust floors it to the earlier second; SQLite's `/` truncates).
    const MS_START: i64 = 1_760_000_000_999;
    const PRE_2001_START: i64 = 978_307_199_999;
    const PRE_1970_START: i64 = -1;

    /// A database built to migration 3 with rows in it: a log league with a
    /// start carrying milliseconds (id 1), a log league started before 2001
    /// (id 2), a manual league (id 3), an imported league with a token (id
    /// 4), an imported league with no token (id 5), a log league started
    /// before 1970 (id 6), and the league `sync_state` row.
    fn version_3_with_rows(path: &Path) {
        let mut conn = Connection::open(path).expect("open");
        migrations().to_version(&mut conn, 3).expect("migration 3");
        conn.execute_batch(&format!(
            "INSERT INTO accounts (id, username, first_seen_at) VALUES (1, 'Qlocal', 0);
             INSERT INTO sync_state (type, last_synced_at, canonical_version)
             VALUES ('league', 5, 1);
             INSERT INTO leagues (id, run_key, account_id, source, kind, state, token,
                 name, started_at, created_at)
             VALUES (1, 'q-l1:q-m1', 1, 'log', 'constructed', 'complete', 'q-l1', 'Q one',
                     {MS_START}, 0),
                    (2, 'q-l2:q-m2', 1, 'log', 'constructed', 'complete', 'q-l2', 'Q two',
                     {PRE_2001_START}, 0),
                    (3, 'manual:3', 1, 'manual', 'constructed', 'complete',
                     'manual_Q00000000000000000000003', 'Q hand', 1700000000500, 0),
                    (6, 'q-l6:q-m6', 1, 'log', 'constructed', 'complete', 'q-l6', 'Q six',
                     {PRE_1970_START}, 0);
             INSERT INTO leagues (id, run_key, account_id, source, kind, state, token,
                 name, started_at, created_at, imported_from, imported_seed)
             VALUES (4, '0x:40', 1, 'imported', 'constructed', 'complete', 'q-l4', 'Q four',
                     1700000123456, 0, 40, '{{}}'),
                    (5, '0x:50', 1, 'imported', 'constructed', 'complete', NULL, 'Q five',
                     1700000000000, 0, 50, '{{}}');"
        ))
        .expect("rows");
    }

    /// Each league's token, start and stored sync client id, by id.
    fn league_sync_ids(conn: &Connection) -> Vec<(i64, Option<String>, i64, Option<String>)> {
        let mut stmt = conn
            .prepare("SELECT id, token, started_at, sync_client_id FROM leagues ORDER BY id")
            .expect("prepare");
        stmt.query_map([], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })
        .expect("query")
        .map(|row| row.expect("row"))
        .collect()
    }

    #[test]
    fn migration_0004_backfills_league_sync_ids() {
        let dir = TestDir::new("store-migration-0004");
        let path = dir.join("mymtgo.sqlite");
        version_3_with_rows(&path);

        let conn = open_writer(&path).expect("the migration runs");
        assert_eq!(count(&conn, "PRAGMA user_version"), migration_count());
        let leagues = league_sync_ids(&conn);
        assert_eq!(leagues.len(), 6);
        for (id, token, started_at, sync_client_id) in leagues {
            let expected = token
                .as_deref()
                .map(|token| sync_bundles::league_client_id(token, started_at));
            assert_eq!(sync_client_id, expected, "league {id}");
        }
        assert_eq!(
            strings(
                &conn,
                "SELECT sync_client_id FROM leagues WHERE id = ?1",
                "1"
            ),
            vec![vec!["q-l1_20251009T085320Z"]],
            "the milliseconds are dropped, not rounded"
        );
        assert_eq!(
            strings(
                &conn,
                "SELECT sync_client_id FROM leagues WHERE id = ?1",
                "6"
            ),
            vec![vec!["q-l6_19691231T235959Z"]],
            "a start before 1970 floors to the earlier second, as the Rust formula does"
        );
    }

    #[test]
    fn a_new_league_row_gets_its_sync_id_on_insert() {
        let dir = TestDir::new("store-sync-id-insert");
        let conn = open_writer(&dir.join("mymtgo.sqlite")).expect("open");
        conn.execute_batch(&format!(
            "INSERT INTO leagues (id, run_key, source, kind, state, token, name,
                 started_at, created_at)
             VALUES (1, 'q-l1:q-m1', 'log', 'constructed', 'active', 'q-l1', 'Q one',
                     {MS_START}, 0);
             INSERT INTO leagues (id, run_key, source, kind, state, token, name,
                 started_at, created_at, imported_from, imported_seed)
             VALUES (2, '0x:20', 'imported', 'constructed', 'complete', NULL, 'Q two',
                     {PRE_2001_START}, 0, 20, '{{}}');"
        ))
        .expect("inserts");
        let ids: Vec<Option<String>> = league_sync_ids(&conn)
            .into_iter()
            .map(|row| row.3)
            .collect();
        assert_eq!(
            ids,
            vec![Some(sync_bundles::league_client_id("q-l1", MS_START)), None],
            "a row with a token gets the formula; a row with none stays NULL"
        );

        conn.execute("UPDATE leagues SET token = 'q-l2' WHERE id = 2", [])
            .expect("a token set later");
        conn.execute(
            "UPDATE leagues SET token = 'q-moved', started_at = 1 WHERE id = 1",
            [],
        )
        .expect("a later token change");
        let ids: Vec<Option<String>> = league_sync_ids(&conn)
            .into_iter()
            .map(|row| row.3)
            .collect();
        assert_eq!(
            ids,
            vec![
                Some(sync_bundles::league_client_id("q-l1", MS_START)),
                Some(sync_bundles::league_client_id("q-l2", PRE_2001_START)),
            ],
            "a token set later fills it once; a filled id is never recomputed"
        );
    }

    #[test]
    fn a_league_inserted_with_a_sync_id_keeps_it() {
        let dir = TestDir::new("store-sync-id-given");
        let conn = open_writer(&dir.join("mymtgo.sqlite")).expect("open");
        conn.execute(
            "INSERT INTO leagues (id, run_key, source, kind, state, token, name,
                 started_at, created_at, imported_from, imported_seed, sync_client_id)
             VALUES (1, '0x:10', 'imported', 'constructed', 'complete', 'q-l1', 'Q one',
                     ?1, 0, 10, '{}', 'q-l1_20200101T000000Z')",
            [MS_START],
        )
        .expect("insert");
        assert_ne!(
            sync_bundles::league_client_id("q-l1", MS_START),
            "q-l1_20200101T000000Z"
        );
        assert_eq!(
            strings(
                &conn,
                "SELECT sync_client_id FROM leagues WHERE id = ?1",
                "1"
            ),
            vec![vec!["q-l1_20200101T000000Z"]],
            "a pulled id that disagrees with the formula is kept as given"
        );
    }

    #[test]
    fn the_sync_cursor_column_starts_empty() {
        let dir = TestDir::new("store-migration-0004-cursor");
        let path = dir.join("mymtgo.sqlite");
        version_3_with_rows(&path);

        let conn = open_writer(&path).expect("the migration runs");
        assert_eq!(
            strings(
                &conn,
                "SELECT last_synced_at, download_cursor FROM sync_state
                 WHERE type = ?1",
                "league"
            ),
            vec![vec!["5", "NULL"]]
        );
    }

    #[test]
    fn migration_0005_leaves_every_match_uncorrected() {
        let dir = TestDir::new("store-migration-0005");
        let path = dir.join("mymtgo.sqlite");
        {
            let mut conn = Connection::open(&path).expect("open");
            migrations().to_version(&mut conn, 4).expect("migration 4");
            conn.execute_batch(
                "INSERT INTO accounts (id, username, first_seen_at) VALUES (1, 'Qlocal', 0);
                 INSERT INTO matches (token, account_id, source, format, match_type, state,
                     outcome, started_at, last_activity_at, revision, derived_version)
                 VALUES ('q-m1', 1, 'log', 'CMODERN', 'Modern', 'complete', 'win', 10, 12,
                         1, 4),
                        ('q-m2', 1, 'imported', 'CMODERN', 'Modern', 'complete', 'loss', 20,
                         22, 1, 0),
                        ('q-m3', 1, 'manual', 'CMODERN', 'Modern', 'complete', 'win', 30, 32,
                         1, 0);",
            )
            .expect("rows");
        }

        let conn = open_writer(&path).expect("the migration runs");
        assert_eq!(count(&conn, "PRAGMA user_version"), migration_count());
        assert_eq!(
            strings(
                &conn,
                "SELECT token, outcome, corrected_at FROM matches WHERE ?1 = ?1 ORDER BY id",
                "x"
            ),
            vec![
                vec!["q-m1", "win", "NULL"],
                vec!["q-m2", "loss", "NULL"],
                vec!["q-m3", "win", "NULL"],
            ],
            "every row keeps its values and reads as not corrected"
        );
    }

    #[test]
    fn migration_0006_leaves_every_match_without_a_history_verdict() {
        let dir = TestDir::new("store-migration-0006");
        let path = dir.join("mymtgo.sqlite");
        {
            let mut conn = Connection::open(&path).expect("open");
            migrations().to_version(&mut conn, 5).expect("migration 5");
            conn.execute_batch(
                "INSERT INTO accounts (id, username, first_seen_at) VALUES (1, 'Qlocal', 0);
                 INSERT INTO matches (token, account_id, source, format, match_type, state,
                     outcome, started_at, last_activity_at, revision, derived_version,
                     corrected_at)
                 VALUES ('q-m1', 1, 'log', 'CMODERN', 'Modern', 'complete', 'draw', 10, 12,
                         1, 4, NULL),
                        ('q-m2', 1, 'log', 'CMODERN', 'Modern', 'complete', 'loss', 20,
                         22, 1, 4, 25);",
            )
            .expect("rows");
        }

        let conn = open_writer(&path).expect("the migration runs");
        assert_eq!(count(&conn, "PRAGMA user_version"), migration_count());
        assert_eq!(
            strings(
                &conn,
                "SELECT token, outcome, corrected_at, history_outcome FROM matches
                 WHERE ?1 = ?1 ORDER BY id",
                "x"
            ),
            vec![
                vec!["q-m1", "draw", "NULL", "NULL"],
                vec!["q-m2", "loss", "25", "NULL"],
            ],
            "every row keeps its values and has no verdict yet"
        );
        let refused = conn.execute(
            "UPDATE matches SET history_outcome = 'draw' WHERE token = 'q-m1'",
            [],
        );
        assert!(refused.is_err(), "only a win or a loss is a verdict");
    }

    #[test]
    fn migration_0010_adds_arena_columns_and_keeps_rows() {
        let dir = TestDir::new("store-migration-0010");
        let path = dir.join("mymtgo.sqlite");
        baseline_with_rows(&path);
        {
            let mut conn = Connection::open(&path).expect("open");
            migrations().to_version(&mut conn, 9).expect("migration 9");
            conn.execute_batch(
                "INSERT INTO accounts (id, username, first_seen_at) VALUES (2, 'Qsecond', 5);
                 INSERT INTO opponents (id, username, first_seen_at) VALUES (1, 'Qrival', 6);",
            )
            .expect("rows");
        }
        let accounts = "SELECT id, username, first_seen_at FROM accounts WHERE ?1 = ?1 ORDER BY id";
        let opponents =
            "SELECT id, username, first_seen_at FROM opponents WHERE ?1 = ?1 ORDER BY id";
        let matches = "SELECT token, account_id FROM matches WHERE ?1 = ?1 ORDER BY id";
        let (accounts_before, opponents_before, matches_before) = {
            let conn = Connection::open(&path).expect("open");
            (
                strings(&conn, accounts, ""),
                strings(&conn, opponents, ""),
                strings(&conn, matches, ""),
            )
        };

        let conn = open_writer(&path).expect("the migration runs");
        assert_eq!(count(&conn, "PRAGMA user_version"), migration_count());
        assert_eq!(strings(&conn, accounts, ""), accounts_before);
        assert_eq!(strings(&conn, opponents, ""), opponents_before);
        assert_eq!(strings(&conn, matches, ""), matches_before);
        assert_eq!(
            strings(
                &conn,
                "SELECT username, platform, arena_user_id FROM accounts WHERE ?1 = ?1 ORDER BY id",
                ""
            ),
            vec![
                vec!["Qlocal", "mtgo", "NULL"],
                vec!["Qsecond", "mtgo", "NULL"]
            ],
            "every existing account is an MTGO one"
        );
        assert_eq!(
            strings(
                &conn,
                "SELECT username, platform, display_name FROM opponents WHERE ?1 = ?1 ORDER BY id",
                ""
            ),
            vec![vec!["Qrival", "mtgo", "NULL"]],
            "every existing opponent is an MTGO one"
        );
        for table in [
            "arena_log_files",
            "arena_log_events",
            "arena_rank_observations",
        ] {
            assert_eq!(
                count(&conn, &format!("SELECT COUNT(*) FROM {table}")),
                0,
                "{table} exists and is empty"
            );
        }
        let refused = conn.execute("UPDATE accounts SET platform = 'steam' WHERE id = 1", []);
        assert!(refused.is_err(), "the platform is mtgo or arena");
    }

    #[test]
    fn migration_0011_adds_gap_claims_and_keeps_rows() {
        let dir = TestDir::new("store-migration-0011");
        let path = dir.join("mymtgo.sqlite");
        baseline_with_rows(&path);
        {
            let mut conn = Connection::open(&path).expect("open");
            migrations()
                .to_version(&mut conn, 10)
                .expect("migration 10");
            conn.execute_batch(
                "INSERT INTO accounts (id, username, first_seen_at, platform, arena_user_id)
                     VALUES (2, 'Qarena#12345', 5, 'arena', 'Qarena-id');
                 INSERT INTO arena_log_files (id, path, identity_hash, head_hash, head_len,
                     first_seen_at) VALUES (1, '/arena/Player.log', 1, 2, 3, 4);
                 INSERT INTO arena_rank_observations (account_id, ladder, season, observed_at,
                     class, level, step, matches_won, matches_lost, arena_log_file_id,
                     byte_offset)
                 VALUES (2, 'constructed', 42, 10, 'Gold', 2, 3, 4, 5, 1, 100),
                        (2, 'constructed', 42, 20, 'Gold', 2, 4, 5, 5, 1, 900);
                 INSERT INTO matches (token, account_id, source, format, match_type, state,
                     outcome, started_at, last_activity_at, revision, derived_version)
                 VALUES ('q-arena', 2, 'log', 'Standard', 'ranked', 'complete', 'win', 300,
                     400, 1, 0);",
            )
            .expect("rows");
        }
        let observations = "SELECT account_id, ladder, byte_offset FROM arena_rank_observations
             WHERE ?1 = ?1 ORDER BY byte_offset";
        let matches = "SELECT token, account_id FROM matches WHERE ?1 = ?1 ORDER BY id";
        let (observations_before, matches_before) = {
            let conn = Connection::open(&path).expect("open");
            (
                strings(&conn, observations, ""),
                strings(&conn, matches, ""),
            )
        };

        let conn = open_writer(&path).expect("the migration runs");
        assert_eq!(count(&conn, "PRAGMA user_version"), migration_count());
        assert_eq!(strings(&conn, observations, ""), observations_before);
        assert_eq!(strings(&conn, matches, ""), matches_before);
        assert_eq!(
            count(&conn, "SELECT COUNT(*) FROM arena_rank_observations"),
            2
        );
        assert_eq!(
            count(&conn, "SELECT COUNT(*) FROM arena_rank_gap_claims"),
            0,
            "the claims table exists and is empty"
        );
    }

    #[test]
    fn migration_0014_adds_account_display_names_and_keeps_rows() {
        let dir = TestDir::new("store-migration-0014");
        let path = dir.join("mymtgo.sqlite");
        baseline_with_rows(&path);
        {
            let mut conn = Connection::open(&path).expect("open");
            migrations()
                .to_version(&mut conn, 13)
                .expect("migration 13");
            conn.execute(
                "INSERT INTO accounts (id, username, first_seen_at, platform, arena_user_id)
                 VALUES (2, 'Qarena#12345', 5, 'arena', 'Qarena-id')",
                [],
            )
            .expect("rows");
        }
        let accounts = "SELECT id, username, platform FROM accounts WHERE ?1 = ?1 ORDER BY id";
        let before = strings(&Connection::open(&path).expect("open"), accounts, "");

        let conn = open_writer(&path).expect("the migration runs");
        assert_eq!(count(&conn, "PRAGMA user_version"), migration_count());
        assert_eq!(strings(&conn, accounts, ""), before);
        assert_eq!(
            count(
                &conn,
                "SELECT COUNT(*) FROM accounts WHERE display_name IS NOT NULL"
            ),
            0,
            "no existing account gets a display name"
        );
    }

    #[test]
    fn migration_0015_adds_arena_course_tables_and_keeps_rows() {
        let dir = TestDir::new("store-migration-0015");
        let path = dir.join("mymtgo.sqlite");
        baseline_with_rows(&path);
        {
            let mut conn = Connection::open(&path).expect("open");
            migrations()
                .to_version(&mut conn, 14)
                .expect("migration 14");
            conn.execute_batch(
                "INSERT INTO accounts (id, username, first_seen_at, platform, arena_user_id)
                     VALUES (2, 'Qarena#12345', 5, 'arena', 'Qarena-id');
                 INSERT INTO arena_log_files (id, path, identity_hash, head_hash, head_len,
                     first_seen_at) VALUES (1, '/arena/Player.log', 1, 2, 3, 4);
                 INSERT INTO matches (token, account_id, source, format, match_type, state,
                     outcome, started_at, last_activity_at, revision, derived_version)
                 VALUES ('q-arena', 2, 'log', 'S6EOE', 'limited', 'complete', 'win', 300,
                     400, 1, 0);",
            )
            .expect("rows");
        }
        let matches = "SELECT token, account_id FROM matches WHERE ?1 = ?1 ORDER BY id";
        let before = strings(&Connection::open(&path).expect("open"), matches, "");

        let conn = open_writer(&path).expect("the migration runs");
        assert_eq!(count(&conn, "PRAGMA user_version"), migration_count());
        assert_eq!(strings(&conn, matches, ""), before);
        for table in ["arena_course_observations", "arena_match_events"] {
            assert_eq!(
                count(&conn, &format!("SELECT COUNT(*) FROM {table}")),
                0,
                "{table} exists and is empty"
            );
        }
        conn.execute(
            "INSERT INTO arena_course_observations (arena_log_file_id, byte_offset, observed_at,
                 account_id, course_id, event_name, module, wins, losses, deck_id, deck_name,
                 cards)
             VALUES (1, 10, 20, 2, '', '', '', 0, 0, '', '', '{}')",
            [],
        )
        .expect("an empty snapshot's marker row");
        let unknown_account = conn.execute(
            "INSERT INTO arena_course_observations (arena_log_file_id, byte_offset, observed_at,
                 account_id, course_id, event_name, module, wins, losses, deck_id, deck_name,
                 cards)
             VALUES (1, 30, 40, 99, '', '', '', 0, 0, '', '', '{}')",
            [],
        );
        assert!(unknown_account.is_err(), "the account must exist");
    }

    #[test]
    fn migration_0021_adds_endstep_captures_and_keeps_rows() {
        let dir = TestDir::new("store-migration-0021");
        let path = dir.join("mymtgo.sqlite");
        baseline_with_rows(&path);
        {
            let mut conn = Connection::open(&path).expect("open");
            migrations()
                .to_version(&mut conn, 20)
                .expect("migration 20");
            conn.execute_batch(
                "INSERT INTO matches (token, account_id, source, format, match_type, state,
                     outcome, started_at, last_activity_at, revision, derived_version)
                 VALUES ('q-before', 1, 'log', 'Modern', 'constructed', 'complete', 'win', 300,
                     400, 1, 0);",
            )
            .expect("rows");
        }
        let matches = "SELECT token, account_id FROM matches WHERE ?1 = ?1 ORDER BY id";
        let before = strings(&Connection::open(&path).expect("open"), matches, "");

        let conn = open_writer(&path).expect("the migration runs");
        assert_eq!(count(&conn, "PRAGMA user_version"), migration_count());
        assert_eq!(strings(&conn, matches, ""), before);
        assert_eq!(
            count(&conn, "SELECT COUNT(*) FROM endstep_captures"),
            0,
            "endstep_captures exists and is empty"
        );
        conn.execute(
            "INSERT INTO endstep_captures (unit_id, kind, started_at, counter, format, extension,
                 browser, developer, classification, frame_count, file_hash, body, imported_at)
             VALUES ('0f8fad5b-d9cb-469f-a165-70867728950e', 'match', 1, 1, 1, '0.1.0',
                 'chrome', 0, 'human', 0, 'h', x'00', 1)",
            [],
        )
        .expect("a capture");
    }

    #[test]
    fn migration_0022_rebuilds_accounts_and_opponents_and_keeps_rows() {
        let dir = TestDir::new("store-migration-0022");
        let path = dir.join("mymtgo.sqlite");
        baseline_with_rows(&path);
        {
            let mut conn = Connection::open(&path).expect("open");
            migrations()
                .to_version(&mut conn, 21)
                .expect("migration 21");
            // An MTGO account (1, from the baseline rows) and an Arena one
            // known by its user id; an opponent on each platform; a ranked
            // Arena match with a constructed reading before its first event.
            conn.execute_batch(
                "UPDATE accounts SET login_id = 77, last_login_at = 9, tracked_chosen = 1
                     WHERE id = 1;
                 INSERT INTO accounts (id, username, first_seen_at, tracked, platform,
                     arena_user_id, display_name)
                     VALUES (2, 'arena:Qarena-id', 5, 0, 'arena', 'Qarena-id', 'Qarena');
                 INSERT INTO opponents (id, username, first_seen_at)
                     VALUES (1, 'Qopponent', 3);
                 INSERT INTO opponents (id, username, first_seen_at, platform, display_name)
                     VALUES (2, 'arena:Qrival-id', 6, 'arena', 'Qrival');
                 INSERT INTO arena_log_files (id, path, identity_hash, head_hash, head_len,
                     first_seen_at) VALUES (1, '/arena/Player.log', 1, 2, 3, 4);
                 INSERT INTO arena_rank_observations (account_id, ladder, season, observed_at,
                     class, level, step, matches_won, matches_lost, arena_log_file_id,
                     byte_offset)
                 VALUES (2, 'constructed', 42, 10, 'Gold', 2, 3, 4, 5, 1, 100);
                 INSERT INTO arena_log_events (arena_log_file_id, byte_offset, seq, kind,
                     match_token, logged_at, raw)
                 VALUES (1, 500, 0, 'room_state', 'q-ranked', 1, x'');
                 INSERT INTO matches (token, account_id, opponent_id, source, format,
                     match_type, state, outcome, started_at, last_activity_at, revision,
                     derived_version, rank_file_id, rank_byte_offset)
                 VALUES ('q-ranked', 2, 2, 'log', 'Standard', 'ranked', 'complete', 'win',
                     300, 400, 1, 0, 1, 100);
                 UPDATE matches SET opponent_id = 1 WHERE token = 'q-m1';",
            )
            .expect("rows");
        }
        let accounts = "SELECT id, username, login_id, first_seen_at, tracked, tracked_chosen,
                 last_login_at, platform, arena_user_id, display_name
             FROM accounts WHERE ?1 = ?1 ORDER BY id";
        let opponents = "SELECT id, username, first_seen_at, platform, display_name
             FROM opponents WHERE ?1 = ?1 ORDER BY id";
        let matches = "SELECT token, account_id, opponent_id FROM matches WHERE ?1 = ?1
             ORDER BY id";
        let readings = "SELECT * FROM match_rank_readings WHERE ?1 = ?1 ORDER BY match_id";
        let (accounts_before, opponents_before, matches_before, readings_before) = {
            let conn = Connection::open(&path).expect("open");
            (
                strings(&conn, accounts, ""),
                strings(&conn, opponents, ""),
                strings(&conn, matches, ""),
                strings(&conn, readings, ""),
            )
        };
        assert_eq!(readings_before.len(), 1, "the ranked match has a reading");

        let conn = open_writer(&path).expect("the migration runs");
        assert_eq!(count(&conn, "PRAGMA user_version"), migration_count());
        assert_eq!(strings(&conn, accounts, ""), accounts_before);
        assert_eq!(strings(&conn, opponents, ""), opponents_before);
        assert_eq!(strings(&conn, matches, ""), matches_before);
        assert_eq!(strings(&conn, readings, ""), readings_before);
        assert_eq!(
            count(&conn, "SELECT COUNT(*) FROM pragma_foreign_key_check"),
            0,
            "every foreign key still points at a row"
        );
        for table in ["accounts", "opponents"] {
            let sql: String = conn
                .query_row(
                    "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = ?1",
                    [table],
                    |row| row.get(0),
                )
                .expect(table);
            assert!(sql.ends_with("STRICT"), "{table} is still STRICT: {sql}");
        }

        conn.execute(
            "INSERT INTO accounts (username, first_seen_at, platform, display_name)
             VALUES ('endstep:Qend-id', 7, 'endstep', 'Qend')",
            [],
        )
        .expect("an Endstep account");
        conn.execute(
            "INSERT INTO opponents (username, first_seen_at, platform, display_name)
             VALUES ('endstep:Qfoe-id', 7, 'endstep', 'Qfoe')",
            [],
        )
        .expect("an Endstep opponent");
        let same_arena_user = conn.execute(
            "INSERT INTO accounts (username, first_seen_at, platform, arena_user_id)
             VALUES ('arena:Qother', 8, 'arena', 'Qarena-id')",
            [],
        );
        assert!(
            same_arena_user.is_err(),
            "an Arena user id names one account"
        );
        let same_login = conn.execute(
            "INSERT INTO accounts (username, first_seen_at, login_id) VALUES ('Qtwin', 8, 77)",
            [],
        );
        assert!(same_login.is_err(), "a login id names one account");
        let same_username = conn.execute(
            "INSERT INTO opponents (username, first_seen_at) VALUES ('Qopponent', 8)",
            [],
        );
        assert!(same_username.is_err(), "an opponent username is unique");
        for table in ["accounts", "opponents"] {
            let steam = conn.execute(
                &format!(
                    "INSERT INTO {table} (username, first_seen_at, platform)
                     VALUES ('Qsteam', 9, 'steam')"
                ),
                [],
            );
            assert!(steam.is_err(), "{table}.platform is a known platform");
        }
        let unknown_account = conn.execute(
            "INSERT INTO matches (token, account_id, source, state, outcome, started_at,
                 last_activity_at, revision, derived_version)
             VALUES ('q-orphan', 99, 'log', 'complete', 'win', 1, 1, 1, 1)",
            [],
        );
        assert!(
            unknown_account.is_err(),
            "matches still reference the rebuilt accounts"
        );
    }

    #[test]
    fn migration_0023_adds_endstep_cards_and_keeps_rows() {
        let dir = TestDir::new("store-migration-0023");
        let path = dir.join("mymtgo.sqlite");
        baseline_with_rows(&path);
        {
            let mut conn = Connection::open(&path).expect("open");
            // As `open_writer` runs them: 0022 rebuilds referenced tables.
            conn.execute_batch("PRAGMA foreign_keys = OFF;")
                .expect("foreign keys off");
            migrations()
                .to_version(&mut conn, 22)
                .expect("migration 22");
            conn.execute_batch(
                "INSERT INTO cards (catalog_id, name) VALUES (5, 'Qcard'), (100000007, 'Qarena');",
            )
            .expect("rows");
        }
        let cards = "SELECT catalog_id, name FROM cards WHERE ?1 = ?1 ORDER BY catalog_id";
        let before = strings(&Connection::open(&path).expect("open"), cards, "");

        let conn = open_writer(&path).expect("the migration runs");
        assert_eq!(count(&conn, "PRAGMA user_version"), migration_count());
        assert_eq!(strings(&conn, cards, ""), before);
        assert_eq!(count(&conn, "SELECT COUNT(*) FROM endstep_cards"), 0);
        conn.execute(
            "INSERT INTO endstep_cards (name, first_seen_at) VALUES ('Qbolt', 1)",
            [],
        )
        .expect("a card with no printing");
        conn.execute(
            "INSERT INTO endstep_cards (name, set_code, collector_number, first_seen_at)
             VALUES ('Qbolt', 'm10', '146', 1)",
            [],
        )
        .expect("a printing is its own row");
        let again = conn.execute(
            "INSERT INTO endstep_cards (name, first_seen_at) VALUES ('Qbolt', 2)",
            [],
        );
        assert!(again.is_err(), "a name and printing is one row");
        let no_name = conn.execute(
            "INSERT INTO endstep_cards (name, first_seen_at) VALUES (NULL, 2)",
            [],
        );
        assert!(no_name.is_err(), "a card has a name");
        let sql: String = conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'endstep_cards'",
                [],
                |row| row.get(0),
            )
            .expect("endstep_cards");
        assert!(sql.ends_with("STRICT"), "{sql}");
    }

    #[test]
    fn migration_0025_adds_league_rounds_and_indexes_join_requests() {
        let dir = TestDir::new("store-migration-0025");
        let path = dir.join("mymtgo.sqlite");
        baseline_with_rows(&path);
        {
            let mut conn = Connection::open(&path).expect("open");
            conn.execute_batch("PRAGMA foreign_keys = OFF;")
                .expect("foreign keys off");
            migrations()
                .to_version(&mut conn, 24)
                .expect("migration 24");
            conn.execute_batch(
                "INSERT INTO log_files (id, path, identity_hash, head_hash, head_len, day_anchor,
                     timezone, clock_date, first_seen_at, last_seen_at)
                 VALUES (7, 'p', '0', '0', 0, '2026-08-22', 'UTC', '2026-08-22', 0, 0);
                 INSERT INTO log_events (log_file_id, byte_offset, kind, logged_at, raw)
                 VALUES (7, 10, 'league_join_request', 0, x'00'),
                        (7, 20, 'league_joined', 0, x'00');",
            )
            .expect("rows");
        }
        let events = "SELECT kind || ':' || byte_offset FROM log_events WHERE ?1 = ?1 ORDER BY log_file_id, byte_offset";
        let before = strings(&Connection::open(&path).expect("open"), events, "");

        let conn = open_writer(&path).expect("the migration runs");
        assert_eq!(count(&conn, "PRAGMA user_version"), migration_count());
        assert_eq!(strings(&conn, events, ""), before);
        let plan: String = conn
            .query_row(
                "EXPLAIN QUERY PLAN SELECT byte_offset FROM log_events
                 WHERE kind IN ('league_joined', 'league_dropped', 'league_join_request')",
                [],
                |row| row.get(3),
            )
            .expect("plan");
        assert!(plan.contains("log_events_leagues"), "{plan}");
        assert_eq!(
            count(&conn, "SELECT COUNT(*) FROM leagues WHERE rounds IS NULL"),
            count(&conn, "SELECT COUNT(*) FROM leagues"),
            "existing runs take their kind's length until they refold"
        );
    }

    #[test]
    fn migration_0026_adds_sync_tables_and_rewrites_arena_run_ids() {
        let dir = TestDir::new("store-migration-0026");
        let path = dir.join("mymtgo.sqlite");
        baseline_with_rows(&path);
        {
            let mut conn = Connection::open(&path).expect("open");
            conn.execute_batch("PRAGMA foreign_keys = OFF;")
                .expect("foreign keys off");
            migrations()
                .to_version(&mut conn, 25)
                .expect("migration 25");
            // An Arena account with a course run, an orphan run, a limited
            // deck used in a limited match and a constructed deck; an MTGO
            // deck used in an MTGO limited match.
            conn.execute_batch(
                "INSERT INTO accounts (id, username, first_seen_at, platform)
                 VALUES (2, 'Qarena#12345', 0, 'arena');
                 INSERT INTO leagues (id, run_key, account_id, source, kind, state, token, name,
                     started_at, created_at)
                 VALUES (10, 'arena:C1', 2, 'log', 'draft', 'active', 'arena:C1', 'Q draft',
                         1000, 1001),
                        (11, 'arena:Ev:tok', 2, 'log', 'draft', 'active', 'arena:Ev:tok',
                         'Q orphan', 2000, 2001);
                 INSERT INTO decks (id, mtgo_id, account_id, name, format, created_at,
                     updated_at, source)
                 VALUES (20, 'q-arena-limited', 2, 'Q limited', 'DFRAFRAFRA', 1, 1, 'file'),
                        (21, 'q-arena-constructed', 2, 'Q constructed', 'CPIONEER', 1, 1, 'file'),
                        (22, 'q-mtgo-deck', 1, 'Q mtgo', 'CMODERN', 1, 1, 'file');
                 INSERT INTO deck_versions (id, deck_id, signature, modified_at)
                 VALUES (30, 20, 'q-sig-l', 1), (31, 21, 'q-sig-c', 1), (32, 22, 'q-sig-m', 1);
                 INSERT INTO matches (token, account_id, source, format, match_type, state,
                     outcome, started_at, last_activity_at, revision, derived_version,
                     deck_version_id, league_id)
                 VALUES ('q-arena-l', 2, 'log', 'DFRAFRAFRA', 'limited', 'complete', 'win',
                         1100, 1200, 1, 0, 30, 10),
                        ('q-arena-c', 2, 'log', 'CPIONEER', 'ranked', 'complete', 'win',
                         1300, 1400, 1, 0, 31, NULL),
                        ('q-mtgo-l', 1, 'log', 'DFRAFRAFRA', 'limited', 'complete', 'win',
                         1500, 1600, 1, 0, 32, NULL);",
            )
            .expect("rows");
        }
        let ids = "SELECT run_key, sync_client_id FROM leagues WHERE ?1 = ?1 ORDER BY id";
        let before = strings(&Connection::open(&path).expect("open"), ids, "");
        let decks = "SELECT m.token, d.id, d.mtgo_id FROM matches m
                     JOIN deck_versions v ON v.id = m.deck_version_id
                     JOIN decks d ON d.id = v.deck_id
                     WHERE ?1 = ?1 ORDER BY m.token";
        let decks_before = strings(&Connection::open(&path).expect("open"), decks, "");
        assert_eq!(decks_before.len(), 3, "three linked matches");
        let orphan_id = before
            .iter()
            .find(|row| row[0] == "arena:Ev:tok")
            .map(|row| row[1].clone())
            .expect("the orphan run");
        assert_ne!(orphan_id, "NULL", "the old trigger filled the orphan");

        let conn = open_writer(&path).expect("the migration runs");
        assert_eq!(count(&conn, "PRAGMA user_version"), migration_count());

        // Course runs take `arena_<course id>`; every other run keeps its id.
        let mut expected = before.clone();
        for row in &mut expected {
            if row[0] == "arena:C1" {
                row[1] = "arena_C1".to_string();
            }
        }
        assert_eq!(strings(&conn, ids, ""), expected);

        // Every deck keeps its key, its matches still linked: an Arena
        // limited match has no deck row of its own (the run sync makes one).
        assert_eq!(strings(&conn, decks, ""), decks_before);

        // `decks.pulled_platform` is new and NULL on every existing deck;
        // only an Arena or Endstep platform goes in it.
        assert_eq!(
            count(
                &conn,
                "SELECT COUNT(*) FROM decks WHERE pulled_platform IS NULL"
            ),
            count(&conn, "SELECT COUNT(*) FROM decks")
        );
        assert!(count(&conn, "SELECT COUNT(*) FROM decks") >= 3);
        conn.execute(
            "UPDATE decks SET pulled_platform = 'endstep' WHERE id = 21",
            [],
        )
        .expect("endstep");
        assert!(
            conn.execute(
                "UPDATE decks SET pulled_platform = 'mtgo' WHERE id = 22",
                []
            )
            .is_err(),
            "only arena and endstep"
        );

        // An Arena run's record columns are new and empty on every existing
        // league until the run sync writes them.
        assert!(count(&conn, "SELECT COUNT(*) FROM leagues") >= 2);
        assert_eq!(
            count(
                &conn,
                "SELECT COUNT(*) FROM leagues WHERE course_wins IS NOT NULL
                     OR course_losses IS NOT NULL OR course_observed_at IS NOT NULL"
            ),
            0
        );

        // The triggers no longer fill an Arena run; MTGO runs are filled as before.
        conn.execute_batch(
            "INSERT INTO leagues (id, run_key, account_id, source, kind, state, token, name,
                 started_at, created_at)
             VALUES (12, 'arena:C2', 2, 'log', 'draft', 'active', 'arena:C2', 'Q new', 3000,
                     3001),
                    (13, 'q-l5:q-m5', 1, 'log', 'constructed', 'active', 'q-l5', 'Q five',
                     0, 1);
             INSERT INTO leagues (id, run_key, account_id, source, kind, state, name,
                 started_at, created_at, imported_from, imported_seed)
             VALUES (14, '0x:77', 2, 'imported', 'draft', 'active', 'Q pulled', 0, 1, 77, '{}');
             UPDATE leagues SET token = 'arena:C3' WHERE id = 14;",
        )
        .expect("new runs");
        assert_eq!(
            strings(
                &conn,
                "SELECT id, sync_client_id FROM leagues WHERE id >= 12 AND ?1 = ?1 ORDER BY id",
                ""
            ),
            vec![
                vec!["12".to_string(), "NULL".to_string()],
                vec!["13".to_string(), "q-l5_19700101T000000Z".to_string()],
                vec!["14".to_string(), "arena:C3_19700101T000000Z".to_string()],
            ],
            "only a run_key starting arena: is left to the run sync"
        );

        // synced_sources and synced_arena_runs exist; a match's source goes with it.
        conn.execute_batch(
            "INSERT INTO synced_sources (match_id, platform, payload)
             SELECT id, 'arena', x'00' FROM matches WHERE token = 'q-arena-c';
             INSERT INTO synced_arena_runs (account_id, course_id, event_name, kind, wins,
                 losses, state, observed_at, player_fields)
             VALUES (2, 'C1', 'PremierDraft_FRA_20260929', 'draft', 1, 0, 'active', 5, NULL);",
        )
        .expect("sync rows");
        let wrong_platform = conn.execute(
            "INSERT INTO synced_sources (match_id, platform, payload)
             SELECT id, 'mtgo', x'00' FROM matches WHERE token = 'q-arena-l'",
            [],
        );
        assert!(wrong_platform.is_err(), "only arena and endstep");
        conn.execute("DELETE FROM matches WHERE token = 'q-arena-c'", [])
            .expect("delete the match");
        assert_eq!(count(&conn, "SELECT COUNT(*) FROM synced_sources"), 0);
        for table in ["synced_sources", "synced_arena_runs"] {
            let sql: String = conn
                .query_row(
                    "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = ?1",
                    [table],
                    |row| row.get(0),
                )
                .expect("table");
            assert!(sql.ends_with("STRICT"), "{sql}");
        }
    }

    #[test]
    fn migration_0028_adds_the_tournament_tables_and_keeps_rows() {
        let dir = TestDir::new("store-migration-0028");
        let path = dir.join("mymtgo.sqlite");
        baseline_with_rows(&path);
        {
            let mut conn = Connection::open(&path).expect("open");
            conn.execute_batch("PRAGMA foreign_keys = OFF;")
                .expect("foreign keys off");
            migrations()
                .to_version(&mut conn, 27)
                .expect("migration 27");
            conn.execute_batch(
                "INSERT INTO accounts (id, username, first_seen_at) VALUES (2, 'Qother', 0);
                 INSERT INTO tournaments (id, mtgo_event_id, account_id, token, name, format,
                     started_at)
                 VALUES (1, 12859509, 1, 'q-t1', 'Q Challenge', 'CLEGACY', 70);
                 INSERT INTO matches (token, account_id, source, format, match_type, state,
                     outcome, started_at, last_activity_at, revision, derived_version,
                     tournament_event_id, tournament_token, tournament_round)
                 VALUES ('q-tm1', 1, 'log', 'CLEGACY', 'Legacy', 'complete', 'win', 80, 90, 1,
                     4, 12859509, 'q-t1', 1);
                 INSERT INTO log_files (id, path, identity_hash, head_hash, head_len,
                     day_anchor, timezone, clock_date, first_seen_at, last_seen_at)
                 VALUES (1, 'q.log', 'i', 'h', 10, '2026-11-22', 'UTC', '2026-11-22', 0, 0);
                 INSERT INTO log_events (log_file_id, byte_offset, kind, tournament_token,
                     logged_at, raw)
                 VALUES (1, 10, 'tournament_state_changed', 'q-t1', 75, CAST(
                     'Tournament State Changed for q-t1 from PremierJoinedAwaitingMaxPlayersState
                      to TournamentJoinedRoundInProgressState' AS BLOB));",
            )
            .expect("rows");
        }
        let tournaments = "SELECT id, mtgo_event_id, account_id, token, name FROM tournaments
                           WHERE ?1 = ?1 ORDER BY id";
        let matches = "SELECT token, tournament_event_id, tournament_round FROM matches
                       WHERE ?1 = ?1 ORDER BY id";
        let events = "SELECT log_file_id, byte_offset, kind, tournament_token FROM log_events
                      WHERE ?1 = ?1 ORDER BY log_file_id, byte_offset";
        let before = {
            let conn = Connection::open(&path).expect("open");
            (
                strings(&conn, tournaments, ""),
                strings(&conn, matches, ""),
                strings(&conn, events, ""),
            )
        };

        let conn = open_writer(&path).expect("the migration runs");
        assert_eq!(count(&conn, "PRAGMA user_version"), migration_count());
        assert_eq!(strings(&conn, tournaments, ""), before.0);
        assert_eq!(strings(&conn, matches, ""), before.1);
        assert_eq!(strings(&conn, events, ""), before.2);
        for table in [
            "tournament_standings",
            "tournament_rounds",
            "tournament_player_states",
        ] {
            assert_eq!(
                count(&conn, &format!("SELECT COUNT(*) FROM {table}")),
                0,
                "{table} starts empty: no backfill"
            );
            let sql: String = conn
                .query_row(
                    "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = ?1",
                    [table],
                    |row| row.get(0),
                )
                .expect("table");
            assert!(sql.ends_with("STRICT"), "{sql}");
        }

        conn.execute(
            "INSERT INTO tournament_standings (account_id, mtgo_event_id, token, status, stage,
                 last_transition_at, last_event_at)
             VALUES (2, 12859509, 'q-t1', 'playing', 'swiss', 75, 75)",
            [],
        )
        .expect("a standing");
        let finished_without_finish =
            conn.execute("UPDATE tournament_standings SET status = 'finished'", []);
        assert!(finished_without_finish.is_err(), "finished needs a finish");
        let finish_while_running =
            conn.execute("UPDATE tournament_standings SET finish = 'dropped'", []);
        assert!(finish_while_running.is_err(), "a finish means finished");
        let unplaced_round = conn.execute(
            "INSERT INTO tournament_rounds (account_id, mtgo_event_id, round)
             VALUES (2, 12859509, 1)",
            [],
        );
        assert!(
            unplaced_round.is_err(),
            "a round row is a decoded round info line, with its log position"
        );
        conn.execute(
            "INSERT INTO tournament_rounds (account_id, mtgo_event_id, round, paired_file_id,
                 paired_offset, paired_at)
             VALUES (2, 12859509, 1, 1, 10, 76)",
            [],
        )
        .expect("a round");
        conn.execute("DELETE FROM accounts WHERE id = 2", [])
            .expect("an account with only overlay rows can go");
        assert_eq!(
            count(&conn, "SELECT COUNT(*) FROM tournament_standings"),
            0,
            "its standings go with it"
        );
        assert_eq!(count(&conn, "SELECT COUNT(*) FROM tournament_rounds"), 0);

        let state = "INSERT OR IGNORE INTO tournament_player_states (log_file_id, byte_offset,
                         tournament_token, new_state, in_playoffs, logged_at)
                     VALUES (1, 20, 'q-t1', 16, 0, 77)";
        assert_eq!(conn.execute(state, []).expect("a state"), 1);
        assert_eq!(
            conn.execute(state, []).expect("the same line again"),
            0,
            "keyed by log position"
        );
        let unknown_file = conn.execute(
            "INSERT INTO tournament_player_states (log_file_id, byte_offset, tournament_token,
                 new_state, in_playoffs, logged_at)
             VALUES (9, 20, 'q-t1', 16, 0, 77)",
            [],
        );
        assert!(unknown_file.is_err(), "a state names a stored log file");

        // The sync fills the tables from the stored log: the joined state
        // change kept from before the migration, and the event's snapshot.
        conn.execute_batch(
            "UPDATE log_files SET inherited_username = (SELECT username FROM accounts
                 WHERE id = 1) WHERE id = 1;
             INSERT INTO log_events (log_file_id, byte_offset, kind, tournament_token,
                 logged_at, raw)
             VALUES (1, 30, 'tournament_sync', 'q-t1', 78, CAST(
                 '16:48:31 [INF] (Game Management|Processing Registered Handler for
                  EventSyncData_t in TournamentUninitializedState) Message:
                  {\"EventToken\":\"q-t1\",\"EventID\":12859509,
                  \"Description\":\"Legacy Challenge\",\"GameStructureCd\":\"CLEGACY\"}'
                 AS BLOB));",
        )
        .expect("the event's snapshot");
        assert!(tournament_sync::sync(&conn, 100).expect("the sync runs"));
        let filled: (i64, String, Option<String>) = conn
            .query_row(
                "SELECT account_id, token, format FROM tournament_standings
                 WHERE mtgo_event_id = 12859509",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("a standing");
        assert_eq!(filled, (1, "q-t1".to_string(), Some("CLEGACY".to_string())));
    }

    #[test]
    fn migration_0027_adds_parked_pulls_and_keeps_rows() {
        let dir = TestDir::new("store-migration-0027");
        let path = dir.join("mymtgo.sqlite");
        baseline_with_rows(&path);
        {
            let mut conn = Connection::open(&path).expect("open");
            conn.execute_batch("PRAGMA foreign_keys = OFF;")
                .expect("foreign keys off");
            migrations()
                .to_version(&mut conn, 26)
                .expect("migration 26");
            conn.execute_batch(
                "INSERT INTO synced_rows (type, client_id, hash, synced_at, carry)
                 VALUES ('match', 'q-m1', 'h1', 5, '{}'), ('league', 'arena_q1', 'h2', 6, '{}');",
            )
            .expect("rows");
        }
        let rows = "SELECT type || ' ' || client_id || ' ' || hash || ' ' || synced_at
                    FROM synced_rows WHERE ?1 = ?1 ORDER BY type, client_id";
        let before = strings(&Connection::open(&path).expect("open"), rows, "");

        let conn = open_writer(&path).expect("the migration runs");
        assert_eq!(count(&conn, "PRAGMA user_version"), migration_count());
        assert_eq!(strings(&conn, rows, ""), before);
        assert_eq!(
            count(
                &conn,
                "SELECT COUNT(*) FROM synced_rows
                 WHERE parked_hash IS NULL AND parked_reason IS NULL"
            ),
            2,
            "no existing row is parked"
        );
        conn.execute(
            "UPDATE synced_rows SET parked_hash = 'h3', parked_reason = 'no_sources'
             WHERE client_id = 'q-m1'",
            [],
        )
        .expect("a parked row");
        let unknown = conn.execute(
            "UPDATE synced_rows SET parked_reason = 'later' WHERE client_id = 'q-m1'",
            [],
        );
        assert!(unknown.is_err(), "only the known reasons");
    }

    #[test]
    fn migration_0024_adds_endstep_sync_and_keeps_rows() {
        let dir = TestDir::new("store-migration-0024");
        let path = dir.join("mymtgo.sqlite");
        baseline_with_rows(&path);
        {
            let mut conn = Connection::open(&path).expect("open");
            // As `open_writer` runs them: 0022 rebuilds referenced tables.
            conn.execute_batch("PRAGMA foreign_keys = OFF;")
                .expect("foreign keys off");
            migrations()
                .to_version(&mut conn, 23)
                .expect("migration 23");
            conn.execute_batch(
                "INSERT INTO endstep_captures (unit_id, kind, started_at, counter, format,
                     extension, browser, developer, classification, frame_count, file_hash, body,
                     imported_at)
                 VALUES ('0f8fad5b-d9cb-469f-a165-70867728950e', 'match', 1, 1, 1, '0.1.0',
                     'chrome', 0, 'human', 3, 'h', x'00', 1);",
            )
            .expect("rows");
        }
        let captures = "SELECT unit_id, frame_count, file_hash FROM endstep_captures
                        WHERE ?1 = ?1 ORDER BY unit_id";
        let matches = "SELECT token, account_id FROM matches WHERE ?1 = ?1 ORDER BY id";
        let before_captures = strings(&Connection::open(&path).expect("open"), captures, "");
        let before_matches = strings(&Connection::open(&path).expect("open"), matches, "");

        let conn = open_writer(&path).expect("the migration runs");
        assert_eq!(count(&conn, "PRAGMA user_version"), migration_count());
        assert_eq!(strings(&conn, captures, ""), before_captures);
        assert_eq!(strings(&conn, matches, ""), before_matches);
        assert_eq!(
            count(&conn, "SELECT synced_version FROM endstep_captures"),
            0,
            "a capture stored before the migration is unsynced"
        );
        conn.execute(
            "INSERT INTO endstep_unit_refs (unit_id, ref)
             VALUES ('0f8fad5b-d9cb-469f-a165-70867728950e', 'game:1')",
            [],
        )
        .expect("a ref of a stored unit");
        let unknown_unit = conn.execute(
            "INSERT INTO endstep_unit_refs (unit_id, ref) VALUES ('nope', 'game:1')",
            [],
        );
        assert!(unknown_unit.is_err(), "a ref names a stored unit");
        let match_id: i64 = conn
            .query_row("SELECT id FROM matches ORDER BY id LIMIT 1", [], |row| {
                row.get(0)
            })
            .expect("a baseline match");
        conn.execute(
            "INSERT INTO endstep_ranked_results (match_id, status, delta) VALUES (?1, 'ok', 12)",
            [match_id],
        )
        .expect("a ranked result of a stored match");
        for table in ["endstep_unit_refs", "endstep_ranked_results"] {
            let sql: String = conn
                .query_row(
                    "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = ?1",
                    [table],
                    |row| row.get(0),
                )
                .expect("table");
            assert!(sql.ends_with("STRICT"), "{sql}");
        }
    }

    #[test]
    fn migration_0016_adds_recorded_replays_and_keeps_rows() {
        let dir = TestDir::new("store-migration-0016");
        let path = dir.join("mymtgo.sqlite");
        baseline_with_rows(&path);
        {
            let mut conn = Connection::open(&path).expect("open");
            migrations()
                .to_version(&mut conn, 15)
                .expect("migration 15");
            conn.execute_batch(
                "INSERT INTO matches (token, account_id, source, format, match_type, state,
                     outcome, started_at, last_activity_at, revision, derived_version)
                 VALUES ('q-before', 1, 'log', 'Modern', 'constructed', 'complete', 'win', 300,
                     400, 1, 0);",
            )
            .expect("rows");
        }
        let matches = "SELECT token, account_id FROM matches WHERE ?1 = ?1 ORDER BY id";
        let before = strings(&Connection::open(&path).expect("open"), matches, "");

        let conn = open_writer(&path).expect("the migration runs");
        assert_eq!(count(&conn, "PRAGMA user_version"), migration_count());
        assert_eq!(strings(&conn, matches, ""), before);
        for table in ["recorded_replays", "recorded_replay_games"] {
            assert_eq!(
                count(&conn, &format!("SELECT COUNT(*) FROM {table}")),
                0,
                "{table} exists and is empty"
            );
        }
        conn.execute(
            "INSERT INTO recorded_replays (token, text, file_hash, imported_at)
             VALUES ('t', 'x', 'h', 1)",
            [],
        )
        .expect("a recording");
        conn.execute(
            "INSERT INTO recorded_replay_games (token, position, game_number)
             VALUES ('t', 0, 1)",
            [],
        )
        .expect("its game");
        conn.execute("DELETE FROM recorded_replays WHERE token = 't'", [])
            .expect("delete");
        assert_eq!(
            count(&conn, "SELECT COUNT(*) FROM recorded_replay_games"),
            0,
            "games go with their recording"
        );
    }

    #[test]
    fn migration_0020_allows_commander_archetypes() {
        let dir = TestDir::new("store-migration-0020");
        let path = dir.join("mymtgo.sqlite");
        baseline_with_rows(&path);
        let archetype_rows = "SELECT id, uuid, name, format, color_identity, source, definition,
                definition_version, retired_at, merged_into_id, source_match_id, incomplete,
                created_at, updated_at, offline_held
             FROM archetypes WHERE ?1 = ?1 ORDER BY id";
        let before = {
            let mut conn = Connection::open(&path).expect("open");
            migrations()
                .to_version(&mut conn, 19)
                .expect("migration 19");
            conn.execute_batch(
                "INSERT INTO archetypes (id, uuid, name, format, color_identity, source,
                     definition, definition_version, created_at, updated_at)
                 VALUES (10, 'q-a10', 'Q Burn', 'modern', 'R', 'downloaded',
                     '{\"k\":1}', 'v1', 1, 2);
                 INSERT INTO archetypes (id, uuid, name, format, source, created_at, updated_at,
                     offline_held, incomplete)
                 VALUES (11, 'q-a11', 'Q Hand', 'modern', 'manual', 3, 4, 1, 1);
                 INSERT INTO archetypes (id, uuid, name, format, source, merged_into_id,
                     retired_at, source_match_id, created_at, updated_at)
                 VALUES (12, 'q-a12', 'Q Merged', 'modern', 'manual', 11, 9,
                     (SELECT id FROM matches WHERE token = 'q-m1'), 5, 6);
                 INSERT INTO archetype_cards (archetype_id, list, catalog_id, quantity, sideboard)
                 VALUES (10, 1, 5, 4, 0);
                 INSERT INTO match_archetypes (match_id, is_opponent, archetype_id, confidence)
                 VALUES ((SELECT id FROM matches WHERE token = 'q-m1'), 1, 10, 0.5);
                 INSERT INTO decks (id, mtgo_id, name, format, created_at, updated_at, source,
                     archetype_id)
                 VALUES (1, 'q-d1', 'Q deck', 'Modern', 1, 1, 'file', 10);",
            )
            .expect("rows");
            strings(&conn, archetype_rows, "")
        };

        let conn = open_writer(&path).expect("the migration runs");
        assert_eq!(count(&conn, "PRAGMA user_version"), migration_count());
        assert_eq!(strings(&conn, archetype_rows, ""), before);
        assert_eq!(
            count(&conn, "SELECT COUNT(*) FROM pragma_foreign_key_check"),
            0
        );
        assert_eq!(
            count(&conn, "SELECT archetype_id FROM decks WHERE id = 1"),
            10
        );
        assert_eq!(count(&conn, "SELECT COUNT(*) FROM match_archetypes"), 1);
        assert_eq!(count(&conn, "SELECT COUNT(*) FROM archetype_cards"), 1);
        assert_eq!(
            count(
                &conn,
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE type = 'index' AND name = 'archetypes_format'"
            ),
            1
        );
        conn.execute(
            "INSERT INTO archetypes (uuid, name, format, source, created_at, updated_at)
             VALUES ('q-c1', 'Q Commander', 'historic_brawl', 'commander', 1, 1)",
            [],
        )
        .expect("a commander row");
        assert!(
            conn.execute(
                "INSERT INTO archetypes (uuid, name, format, source, created_at, updated_at)
                 VALUES ('q-o1', 'Q Other', 'modern', 'other', 1, 1)",
                [],
            )
            .is_err(),
            "other sources stay refused"
        );
        // The self reference still works: deleting the parent clears the child.
        conn.execute("DELETE FROM archetypes WHERE id = 11", [])
            .expect("delete");
        assert_eq!(
            count(
                &conn,
                "SELECT COUNT(*) FROM archetypes WHERE merged_into_id IS NOT NULL"
            ),
            0
        );
    }

    #[test]
    fn migration_0019_adds_match_commanders() {
        let dir = TestDir::new("store-migration-0019");
        let path = dir.join("mymtgo.sqlite");
        baseline_with_rows(&path);
        {
            let mut conn = Connection::open(&path).expect("open");
            migrations()
                .to_version(&mut conn, 18)
                .expect("migration 18");
        }

        let conn = open_writer(&path).expect("the migration runs");
        assert_eq!(count(&conn, "PRAGMA user_version"), migration_count());
        let match_id = count(&conn, "SELECT MIN(id) FROM matches");
        conn.execute(
            "INSERT INTO match_commanders (match_id, seat, position, catalog_id)
             VALUES (?1, 'player', 0, 5), (?1, 'opponent', 0, 6)",
            [match_id],
        )
        .expect("commanders");
        assert!(
            conn.execute(
                "INSERT INTO match_commanders (match_id, seat, position, catalog_id)
                 VALUES (?1, 'player', 2, 7)",
                [match_id],
            )
            .is_err(),
            "only two positions"
        );
        conn.execute("DELETE FROM matches WHERE id = ?1", [match_id])
            .expect("delete");
        assert_eq!(
            count(&conn, "SELECT COUNT(*) FROM match_commanders"),
            0,
            "commanders go with their match"
        );
    }

    #[test]
    fn migration_0018_rekeys_arena_archetypes() {
        let dir = TestDir::new("store-migration-0018");
        let path = dir.join("mymtgo.sqlite");
        baseline_with_rows(&path);
        {
            let mut conn = Connection::open(&path).expect("open");
            migrations()
                .to_version(&mut conn, 17)
                .expect("migration 17");
            conn.execute_batch(
                "INSERT INTO archetypes (uuid, name, format, source, created_at, updated_at) VALUES
                     ('q-a1', 'Arena Burn', 'arena_historic', 'manual', 1, 1),
                     ('q-a2', 'Arena Elves', 'arena_timeless', 'manual', 1, 1),
                     ('q-a3', 'Modern Burn', 'modern', 'manual', 1, 1),
                     ('q-a4', 'Arena Burn', 'historic', 'manual', 1, 1),
                     ('q-a5', 'Odd Burn', 'arenaxfoo', 'manual', 1, 1);",
            )
            .expect("rows");
        }

        let conn = open_writer(&path).expect("the migration runs");
        assert_eq!(count(&conn, "PRAGMA user_version"), migration_count());
        let formats: Vec<String> = conn
            .prepare("SELECT format FROM archetypes WHERE uuid LIKE 'q-a%' ORDER BY uuid")
            .expect("prepare")
            .query_map([], |row| row.get(0))
            .expect("query")
            .map(|row| row.expect("row"))
            .collect();
        // A format that only looks like `arena_` (no underscore) stays.
        assert_eq!(
            formats,
            ["historic", "timeless", "modern", "historic", "arenaxfoo"]
        );
        assert_eq!(
            count(
                &conn,
                "SELECT COUNT(*) FROM archetypes WHERE name = 'Arena Burn'"
            ),
            2,
            "a same-named row is kept beside the existing one"
        );
    }

    #[test]
    fn migration_0017_adds_the_deck_cover_file_and_keeps_rows() {
        let dir = TestDir::new("store-migration-0017");
        let path = dir.join("mymtgo.sqlite");
        baseline_with_rows(&path);
        {
            let mut conn = Connection::open(&path).expect("open");
            migrations()
                .to_version(&mut conn, 16)
                .expect("migration 16");
            conn.execute_batch(
                "INSERT INTO decks (id, mtgo_id, name, format, color_identity_manual,
                     cover_catalog_id, created_at, updated_at, source)
                 VALUES (1, 'q-deck', 'Q deck', 'CMODERN', 0, 77, 1, 2, 'file');",
            )
            .expect("rows");
        }

        let conn = open_writer(&path).expect("the migration runs");
        assert_eq!(count(&conn, "PRAGMA user_version"), migration_count());
        let (name, cover, file): (String, Option<i64>, Option<String>) = conn
            .query_row(
                "SELECT name, cover_catalog_id, cover_file FROM decks WHERE id = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("the deck");
        assert_eq!((name.as_str(), cover, file), ("Q deck", Some(77), None));
        conn.execute("UPDATE decks SET cover_file = 'q.png' WHERE id = 1", [])
            .expect("the column takes a name");
    }

    #[test]
    fn migration_0012_adds_arena_game_tables_and_keeps_rows() {
        let dir = TestDir::new("store-migration-0012");
        let path = dir.join("mymtgo.sqlite");
        baseline_with_rows(&path);
        {
            let mut conn = Connection::open(&path).expect("open");
            migrations()
                .to_version(&mut conn, 11)
                .expect("migration 11");
            conn.execute_batch(
                "INSERT INTO accounts (id, username, first_seen_at, platform, arena_user_id)
                     VALUES (2, 'Qarena#12345', 5, 'arena', 'Qarena-id');
                 INSERT INTO arena_log_files (id, path, identity_hash, head_hash, head_len,
                     first_seen_at) VALUES (1, '/arena/Player.log', 1, 2, 3, 4);
                 INSERT INTO matches (token, account_id, source, format, match_type, state,
                     outcome, started_at, last_activity_at, revision, derived_version)
                 VALUES ('q-arena', 2, 'log', 'Standard', 'ranked', 'complete', 'win', 300,
                     400, 1, 0);",
            )
            .expect("rows");
        }
        let matches = "SELECT token, account_id FROM matches WHERE ?1 = ?1 ORDER BY id";
        let before = {
            let conn = Connection::open(&path).expect("open");
            strings(&conn, matches, "")
        };

        let conn = open_writer(&path).expect("the migration runs");
        assert_eq!(count(&conn, "PRAGMA user_version"), migration_count());
        assert_eq!(strings(&conn, matches, ""), before);
        assert_eq!(count(&conn, "SELECT COUNT(*) FROM arena_log_files"), 1);
        for table in [
            "arena_game_messages",
            "arena_game_archives",
            "arena_game_syncs",
        ] {
            assert_eq!(
                count(&conn, &format!("SELECT COUNT(*) FROM {table}")),
                0,
                "{table} exists and is empty"
            );
        }
        let refused = conn.execute(
            "INSERT INTO arena_game_syncs (match_id, status, derived_version, synced_at)
             VALUES (1, 'bogus', 1, 1)",
            [],
        );
        assert!(
            refused.is_err(),
            "the sync status is derived or no_messages"
        );
    }

    #[test]
    fn migration_0013_fills_ranked_arena_matches_with_the_reading_before_them() {
        let dir = TestDir::new("store-migration-0013");
        let path = dir.join("mymtgo.sqlite");
        baseline_with_rows(&path);
        {
            let mut conn = Connection::open(&path).expect("open");
            migrations()
                .to_version(&mut conn, 12)
                .expect("migration 12");
            // Readings of account 2 at offsets 100 (season 42), 400 (a stale
            // season 41 answer), 900 (limited) and 2000; matches whose first
            // event is at 500 (ranked, casual), 50 (ranked, nothing before)
            // and a manual ranked one started between the 100 and 2000
            // readings' times.
            conn.execute_batch(
                "INSERT INTO accounts (id, username, first_seen_at, platform, arena_user_id)
                     VALUES (2, 'Qarena#12345', 5, 'arena', 'Qarena-id');
                 INSERT INTO arena_log_files (id, path, identity_hash, head_hash, head_len,
                     first_seen_at) VALUES (1, '/arena/Player.log', 1, 2, 3, 4);
                 INSERT INTO arena_rank_observations (account_id, ladder, season, observed_at,
                     class, level, step, matches_won, matches_lost, arena_log_file_id,
                     byte_offset)
                 VALUES (2, 'constructed', 42, 10, 'Gold', 2, 3, 4, 5, 1, 100),
                        (2, 'constructed', 41, 15, 'Silver', 1, 0, 0, 0, 1, 400),
                        (2, 'limited', 42, 18, 'Bronze', 4, 0, 0, 0, 1, 450),
                        (2, 'constructed', 42, 50, 'Gold', 1, 0, 6, 5, 1, 2000);
                 INSERT INTO arena_log_events (arena_log_file_id, byte_offset, seq, kind,
                     match_token, logged_at, raw)
                 VALUES (1, 700, 0, 'game_info', 'q-ranked', 1, x''),
                        (1, 500, 0, 'room_state', 'q-ranked', 1, x''),
                        (1, 500, 1, 'room_state', 'q-casual', 1, x''),
                        (1, 50, 0, 'room_state', 'q-early', 1, x'');
                 INSERT INTO matches (token, account_id, source, format, match_type, state,
                     outcome, started_at, last_activity_at, revision, derived_version)
                 VALUES ('q-ranked', 2, 'log', 'Standard', 'ranked', 'complete', 'win', 300,
                     400, 1, 0),
                        ('q-casual', 2, 'log', 'Standard', 'unranked', 'complete', 'win', 300,
                     400, 1, 0),
                        ('q-early', 2, 'log', 'Standard', 'ranked', 'complete', 'win', 300,
                     400, 1, 0),
                        ('q-hand', 2, 'manual', 'Standard', 'ranked', 'complete', 'win', 20,
                     30, 1, 0);",
            )
            .expect("rows");
        }
        let matches = "SELECT token, account_id, match_type FROM matches WHERE ?1 = ?1
             ORDER BY id";
        let before = {
            let conn = Connection::open(&path).expect("open");
            strings(&conn, matches, "")
        };

        let conn = open_writer(&path).expect("the migration runs");
        assert_eq!(count(&conn, "PRAGMA user_version"), migration_count());
        assert_eq!(strings(&conn, matches, ""), before);
        assert_eq!(
            strings(
                &conn,
                "SELECT token, rank_file_id, rank_byte_offset FROM matches WHERE ?1 = ?1
                 ORDER BY token",
                ""
            ),
            vec![
                vec!["q-casual", "NULL", "NULL"],
                vec!["q-early", "NULL", "NULL"],
                vec!["q-hand", "1", "100"],
                vec!["q-m1", "NULL", "NULL"],
                vec!["q-m3", "NULL", "NULL"],
                vec!["q-ranked", "1", "100"],
            ],
            "a ranked Arena match has the season's last constructed reading before it"
        );
    }

    #[test]
    fn migration_0009_leaves_every_archetype_unheld_offline() {
        let dir = TestDir::new("store-migration-0009");
        let path = dir.join("mymtgo.sqlite");
        {
            let mut conn = Connection::open(&path).expect("open");
            migrations().to_version(&mut conn, 8).expect("migration 8");
            conn.execute_batch(
                "INSERT INTO archetypes (uuid, name, format, source, created_at, updated_at)
                 VALUES ('q-a', 'Qa', 'modern', 'downloaded', 1, 1),
                        ('q-b', 'Qb', 'modern', 'manual', 1, 1);",
            )
            .expect("rows");
        }

        let conn = open_writer(&path).expect("the migration runs");
        assert_eq!(count(&conn, "PRAGMA user_version"), migration_count());
        assert_eq!(
            strings(
                &conn,
                "SELECT uuid, source, offline_held FROM archetypes
                 WHERE format = ?1 ORDER BY uuid",
                "modern"
            ),
            vec![vec!["q-a", "downloaded", "0"], vec!["q-b", "manual", "0"]],
            "every archetype keeps its values and is not offline-held"
        );
        let refused = conn.execute("UPDATE archetypes SET offline_held = 2", []);
        assert!(refused.is_err(), "the marker is 0 or 1");
    }

    #[test]
    fn migration_0008_leaves_every_card_unmarked() {
        let dir = TestDir::new("store-migration-0008");
        let path = dir.join("mymtgo.sqlite");
        {
            let mut conn = Connection::open(&path).expect("open");
            migrations().to_version(&mut conn, 7).expect("migration 7");
            conn.execute_batch(
                "INSERT INTO cards (catalog_id, scryfall_id, name) VALUES
                     (1, 'q-scry', 'Qresolved'), (2, NULL, NULL);",
            )
            .expect("rows");
        }

        let conn = open_writer(&path).expect("the migration runs");
        assert_eq!(count(&conn, "PRAGMA user_version"), migration_count());
        assert_eq!(
            strings(
                &conn,
                "SELECT catalog_id, scryfall_id, name, api_unknown_at FROM cards
                 WHERE ?1 = ?1 ORDER BY catalog_id",
                "x"
            ),
            vec![
                vec!["1", "q-scry", "Qresolved", "NULL"],
                vec!["2", "NULL", "NULL", "NULL"],
            ],
            "every card keeps its values and is not marked unknown"
        );
    }

    #[test]
    fn migration_0007_requeues_own_reports_when_a_league_id_changes() {
        let dir = TestDir::new("store-migration-0007");
        let path = dir.join("mymtgo.sqlite");
        {
            let mut conn = Connection::open(&path).expect("open");
            migrations().to_version(&mut conn, 6).expect("migration 6");
            conn.execute_batch(
                "INSERT INTO accounts (id, username, first_seen_at) VALUES (1, 'Qlocal', 0);
                 INSERT INTO leagues (id, run_key, account_id, source, kind, state, token,
                     name, started_at, created_at)
                 VALUES (1, 'log:q-l1', 1, 'log', 'constructed', 'complete', 'q-l1',
                         'Qleague', 0, 0),
                        (2, 'log:q-l2', 1, 'log', 'constructed', 'complete', 'q-l2',
                         'Qleague', 0, 0);
                 INSERT INTO matches (token, account_id, source, format, match_type, state,
                     outcome, started_at, last_activity_at, revision, derived_version,
                     league_id)
                 VALUES ('q-sent', 1, 'log', 'CMODERN', 'Modern', 'complete', 'win', 10, 12,
                         1, 4, 1),
                        ('q-pulled', 1, 'log', 'CMODERN', 'Modern', 'complete', 'win', 20,
                         22, 1, 4, 1),
                        ('q-queued', 1, 'log', 'CMODERN', 'Modern', 'complete', 'win', 30,
                         32, 1, 4, 1),
                        ('q-other', 1, 'log', 'CMODERN', 'Modern', 'complete', 'win', 40,
                         42, 1, 4, 2),
                        ('q-loose', 1, 'log', 'CMODERN', 'Modern', 'complete', 'win', 50,
                         52, 1, 4, NULL),
                        ('q-deleted', 1, 'log', 'CMODERN', 'Modern', 'complete', 'win', 60,
                         62, 1, 4, 2);
                 INSERT INTO outbox (kind, subject, payload, status, attempts, sent_at,
                     created_at)
                 VALUES ('match_report', 'q-sent', X'', 'sent', 1, 13, 13),
                        ('match_report', 'q-pulled', X'', 'sent', 0, 23, 23),
                        ('match_report', 'q-queued', X'01', 'pending', 0, NULL, 33),
                        ('match_report', 'q-other', X'', 'sent', 1, 43, 43),
                        ('match_report', 'q-loose', X'', 'sent', 1, 53, 53),
                        ('match_report', 'q-deleted', X'', 'sent', 1, 63, 63),
                        ('card_stats', 'q-sent:1', X'', 'sent', 1, 14, 14);",
            )
            .expect("rows");
        }

        let conn = open_writer(&path).expect("the migration runs");
        assert_eq!(count(&conn, "PRAGMA user_version"), migration_count());
        let reports = |conn: &Connection| -> Vec<String> {
            strings(
                conn,
                "SELECT subject FROM outbox WHERE kind = ?1 ORDER BY subject",
                "match_report",
            )
            .into_iter()
            .map(|row| row[0].clone())
            .collect()
        };
        assert_eq!(
            reports(&conn),
            vec![
                "q-deleted",
                "q-loose",
                "q-other",
                "q-pulled",
                "q-queued",
                "q-sent"
            ],
            "the migration itself queues nothing again"
        );

        // A write that keeps the id changes nothing.
        conn.execute_batch(
            "UPDATE leagues SET sync_client_id = sync_client_id;
             UPDATE matches SET league_id = league_id;",
        )
        .expect("same values");
        assert_eq!(reports(&conn).len(), 6);

        // League 1's id swaps (`take_synced_id`): its matches' own reports
        // go, so the next enqueue builds them again with the new id. The
        // report another device sent (`record_sent`, never attempted here)
        // stays, and so does league 2's.
        conn.execute(
            "UPDATE leagues SET sync_client_id = 'q-l1_20270101T000000Z' WHERE id = 1",
            [],
        )
        .expect("swapped");
        assert_eq!(
            reports(&conn),
            vec!["q-deleted", "q-loose", "q-other", "q-pulled"]
        );

        // A match given a league after its report went (or moved to another
        // run) is reported again.
        conn.execute(
            "UPDATE matches SET league_id = 1 WHERE token IN ('q-loose', 'q-other')",
            [],
        )
        .expect("moved");
        assert_eq!(reports(&conn), vec!["q-deleted", "q-pulled"]);

        // A deleted match leaving its league is not reported again.
        conn.execute(
            "UPDATE matches SET hidden_at = 70, league_id = NULL WHERE token = 'q-deleted'",
            [],
        )
        .expect("deleted");
        assert_eq!(reports(&conn), vec!["q-deleted", "q-pulled"]);
        assert_eq!(
            count(
                &conn,
                "SELECT COUNT(*) FROM outbox WHERE kind = 'card_stats'"
            ),
            1,
            "other kinds are left alone"
        );
    }

    // Beta.2 and beta.3 stamped `PRAGMA application_id` with a hash of the
    // schema files, which the Windows installers computed over CRLF line
    // endings (1748696188). Beta.4 refused those databases. The stamp is
    // no longer read.
    #[test]
    fn a_beta_database_opens_whatever_its_application_id() {
        for stamp in [221_012_885, 1_748_696_188, 0] {
            let dir = TestDir::new(&format!("store-beta-stamp-{stamp}"));
            let path = beta_2_database(&dir);
            Connection::open(&path)
                .expect("open")
                .execute_batch(&format!("PRAGMA application_id = {stamp};"))
                .expect("stamp");
            let conn = open_writer(&path).expect("a beta database opens");
            assert_eq!(count(&conn, "PRAGMA user_version"), migration_count());
        }
    }

    #[test]
    fn a_database_from_a_newer_version_is_refused_untouched() {
        let dir = TestDir::new("store-newer");
        let path = dir.join("newer.sqlite");
        let before = old_database(&path, migration_count() + 1);
        let error = open_writer(&path).expect_err("refused");
        assert!(matches!(error, StoreError::Unsupported(_)), "got {error:?}");
        let message = error.to_string();
        assert!(message.contains("newer version of MyMTGO"), "{message}");
        assert_eq!(
            std::fs::read(&path).expect("read the file"),
            before,
            "the file is untouched"
        );
    }

    const LEAGUE_TABLES: [&str; 2] = ["leagues", "tournaments"];

    #[test]
    fn every_table_is_strict() {
        let dir = TestDir::new("store-strict-tables");
        let conn = open_writer(&dir.join("mymtgo.sqlite")).expect("open");
        let mut stmt = conn
            .prepare(
                "SELECT name, strict FROM pragma_table_list
                 WHERE schema = 'main' AND type = 'table' AND name NOT LIKE 'sqlite_%'",
            )
            .expect("prepare");
        let tables: Vec<(String, i64)> = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .expect("query")
            .map(|row| row.expect("row"))
            .collect();
        assert!(tables.len() > 20, "{tables:?}");
        for (table, strict) in tables {
            assert_eq!(strict, 1, "{table} is STRICT");
        }
    }

    fn insert_account_and_match(conn: &Connection) {
        conn.execute_batch(
            "INSERT INTO accounts (id, username, first_seen_at) VALUES (1, 'Qlocal', 0);
             INSERT INTO matches (token, account_id, source, format, match_type, state,
                 outcome, started_at, last_activity_at, revision, derived_version)
             VALUES ('tok', 1, 'log', 'CMODERN', 'Modern', 'complete', 'win', 5, 6, 1, 4);",
        )
        .expect("a match");
    }

    #[test]
    fn listed_matches_leaves_hidden_matches_out() {
        let dir = TestDir::new("store-listed-matches");
        let conn = open_writer(&dir.join("mymtgo.sqlite")).expect("open");
        insert_account_and_match(&conn);
        assert_eq!(count(&conn, "SELECT COUNT(*) FROM listed_matches"), 1);
        conn.execute("UPDATE matches SET hidden_at = 9 WHERE token = 'tok'", [])
            .expect("hide");
        assert_eq!(
            count(&conn, "SELECT COUNT(*) FROM listed_matches"),
            0,
            "the view leaves hidden matches out"
        );
        assert_eq!(count(&conn, "SELECT COUNT(*) FROM matches"), 1);
    }

    #[test]
    fn a_match_note_has_1_to_5000_characters_and_a_game_number_is_used_once() {
        let dir = TestDir::new("store-match-checks");
        let conn = open_writer(&dir.join("mymtgo.sqlite")).expect("open");
        insert_account_and_match(&conn);
        let note =
            |text: &str| conn.execute("UPDATE matches SET notes = ?1 WHERE token = 'tok'", [text]);
        assert!(note("").is_err(), "an empty note is NULL");
        assert!(note(&"é".repeat(5001)).is_err(), "at most 5000 characters");
        note(&"é".repeat(5000)).expect("5000 characters");
        conn.execute(
            "INSERT INTO games (match_id, game_number, result, end_reason)
             VALUES (1, 1, 'win', 'win')",
            [],
        )
        .expect("game 1");
        let again = conn.execute(
            "INSERT INTO games (match_id, game_number, result, end_reason)
             VALUES (1, 1, 'loss', 'win')",
            [],
        );
        assert!(again.is_err(), "one row per game number");
    }

    #[test]
    fn a_manual_match_has_no_mtgo_match_id_and_no_other_source_is_allowed() {
        let dir = TestDir::new("store-match-sources");
        let conn = open_writer(&dir.join("mymtgo.sqlite")).expect("open");
        insert_account_and_match(&conn);
        conn.execute(
            "INSERT INTO matches (token, account_id, source, state, outcome, started_at,
                 last_activity_at, revision, derived_version)
             VALUES ('q-manual', 1, 'manual', 'complete', 'win', 1, 1, 1, 0)",
            [],
        )
        .expect("a manual row is allowed");
        let manual_with_match_id = conn.execute(
            "INSERT INTO matches (token, mtgo_match_id, account_id, source, state, outcome,
                 started_at, last_activity_at, revision, derived_version)
             VALUES ('q-manual-2', 5, 1, 'manual', 'complete', 'win', 1, 1, 1, 0)",
            [],
        );
        assert!(
            manual_with_match_id.is_err(),
            "a manual row has no MTGO match id"
        );
        let other_source = conn.execute(
            "INSERT INTO matches (token, account_id, source, state, outcome, started_at,
                 last_activity_at, revision, derived_version)
             VALUES ('q-other', 1, 'other', 'complete', 'win', 1, 1, 1, 0)",
            [],
        );
        assert!(other_source.is_err(), "no other source");
        conn.execute(
            "INSERT INTO matches (token, mtgo_match_id, account_id, source, state, outcome,
                 started_at, last_activity_at, revision, derived_version)
             VALUES ('q-history', 6, 1, 'history', 'complete', 'win', 1, 1, 1, 0)",
            [],
        )
        .expect("a history import is allowed");
    }

    #[test]
    fn a_league_run_kind_is_constructed_draft_or_sealed() {
        let dir = TestDir::new("store-league-run-kind");
        let conn = open_writer(&dir.join("mymtgo.sqlite")).expect("open");
        conn.execute_batch(
            "INSERT INTO accounts (id, username, first_seen_at) VALUES (1, 'Qlocal', 0);
             INSERT INTO leagues (id, run_key, account_id, source, kind, state, token, name,
                 started_at, created_at)
             VALUES (29, 'q-league:q-match', 1, 'log', 'sealed', 'complete', 'q-league',
                 'Q League', 5, 6);",
        )
        .expect("a run");
        for kind in ["constructed", "draft", "sealed"] {
            conn.execute("UPDATE leagues SET kind = ?1 WHERE id = 29", [kind])
                .unwrap_or_else(|error| panic!("{kind}: {error}"));
        }
        let bad_kind = conn.execute("UPDATE leagues SET kind = 'cube' WHERE id = 29", []);
        assert!(bad_kind.is_err(), "only constructed, draft and sealed");
    }

    #[test]
    fn league_rows_keep_their_source_rules() {
        let dir = TestDir::new("store-league-sources");
        let conn = open_writer(&dir.join("mymtgo.sqlite")).expect("open");
        conn.execute_batch(
            "INSERT INTO accounts (id, username, first_seen_at) VALUES (1, 'Qlocal', 0);",
        )
        .expect("account");
        let insert = |sql: &str| conn.execute(sql, []);
        // A manual row is complete and constructed, with a `manual_` token.
        assert!(
            insert(
                "INSERT INTO leagues (run_key, account_id, source, kind, state, name, started_at,
                     created_at, token)
                 VALUES ('manual:1', 1, 'manual', 'constructed', 'complete', 'Q', 1, 1,
                     'manual_Q')"
            )
            .is_ok()
        );
        assert!(
            insert(
                "INSERT INTO leagues (run_key, account_id, source, kind, state, name, started_at,
                     created_at)
                 VALUES ('manual:3', 1, 'manual', 'constructed', 'complete', 'Q', 1, 1)"
            )
            .is_err(),
            "a manual run has a manual_ token"
        );
        assert!(
            insert(
                "INSERT INTO leagues (run_key, account_id, source, kind, state, name, started_at,
                     created_at, token)
                 VALUES ('manual:2', 1, 'manual', 'constructed', 'active', 'Q', 1, 1, 'manual_Q')"
            )
            .is_err(),
            "a manual run is complete"
        );
        // An imported row carries its seed and its 0.x id in its key.
        assert!(
            insert(
                "INSERT INTO leagues (run_key, account_id, source, kind, state, token, name,
                     started_at, created_at, imported_from, imported_seed)
                 VALUES ('0x:7', 1, 'imported', 'draft', 'active', 'q', 'Q', 1, 1, 7, '{}')"
            )
            .is_ok()
        );
        assert!(
            insert(
                "INSERT INTO leagues (run_key, account_id, source, kind, state, token, name,
                     started_at, created_at, imported_from)
                 VALUES ('0x:8', 1, 'imported', 'draft', 'active', 'q', 'Q', 1, 1, 8)"
            )
            .is_err(),
            "an imported row has a seed"
        );
        // A log row has a token and no 0.x id.
        assert!(
            insert(
                "INSERT INTO leagues (run_key, account_id, source, kind, state, name, started_at,
                     created_at)
                 VALUES ('q:m', 1, 'log', 'constructed', 'active', 'Q', 1, 1)"
            )
            .is_err(),
            "a log row has a token"
        );
    }

    #[test]
    fn a_linked_match_has_a_league() {
        let dir = TestDir::new("store-league-linked");
        let conn = open_writer(&dir.join("mymtgo.sqlite")).expect("open");
        insert_account_and_match(&conn);
        let linked = conn.execute("UPDATE matches SET league_linked = 1", []);
        assert!(linked.is_err(), "a link names its league");
    }

    #[test]
    fn draft_events_walk_their_partial_index() {
        let dir = TestDir::new("store-draft-index");
        let conn = open_writer(&dir.join("mymtgo.sqlite")).expect("open");
        let mut stmt = conn
            .prepare(
                "EXPLAIN QUERY PLAN SELECT byte_offset FROM log_events
                 WHERE kind IN ('draft_created', 'draft_state_changed', 'draft_selection',
                     'draft_league_standing', 'draft_joined', 'draft_pod_state',
                     'draft_pack_opened', 'draft_pending_pick', 'draft_ended',
                     'draft_pick_committed', 'league_pool_granted')
                 ORDER BY log_file_id, byte_offset",
            )
            .expect("explain");
        let plan: Vec<String> = stmt
            .query_map([], |row| row.get::<_, String>(3))
            .expect("rows")
            .map(|row| row.expect("row"))
            .collect();
        assert!(plan.join("\n").contains("log_events_drafts"), "{plan:?}");
    }

    #[test]
    fn limited_foreign_keys_state_their_delete_rules() {
        let dir = TestDir::new("store-limited-fk-rules");
        let conn = open_writer(&dir.join("mymtgo.sqlite")).expect("open");
        let mut found = Vec::new();
        for table in ["drafts", "draft_picks", "limited_deck_snapshots"] {
            let mut stmt = conn
                .prepare("SELECT \"table\", \"from\", on_delete FROM pragma_foreign_key_list(?1)")
                .expect("prepare");
            let rows = stmt
                .query_map([table], |row| {
                    Ok(format!(
                        "{table}.{} -> {} {}",
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(2)?
                    ))
                })
                .expect("query");
            for row in rows {
                found.push(row.expect("row"));
            }
        }
        found.sort();
        assert_eq!(
            found,
            vec![
                "draft_picks.draft_id -> drafts CASCADE",
                "drafts.account_id -> accounts RESTRICT",
                "drafts.league_id -> leagues SET NULL",
                "limited_deck_snapshots.match_id -> matches CASCADE",
            ]
        );
    }

    #[test]
    fn league_foreign_keys_state_their_delete_rules() {
        let dir = TestDir::new("store-league-fk-rules");
        let conn = open_writer(&dir.join("mymtgo.sqlite")).expect("open");
        let mut found = Vec::new();
        for table in LEAGUE_TABLES {
            let mut stmt = conn
                .prepare("SELECT \"table\", \"from\", on_delete FROM pragma_foreign_key_list(?1)")
                .expect("prepare");
            let rows = stmt
                .query_map([table], |row| {
                    Ok(format!(
                        "{table}.{} -> {} {}",
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(2)?
                    ))
                })
                .expect("query");
            for row in rows {
                found.push(row.expect("row"));
            }
        }
        found.sort();
        assert_eq!(
            found,
            vec![
                "leagues.account_id -> accounts RESTRICT",
                "leagues.deck_id -> decks RESTRICT",
                "leagues.deck_version_id -> deck_versions SET NULL",
                "tournaments.account_id -> accounts RESTRICT",
            ]
        );
    }

    #[test]
    fn league_lookups_walk_their_partial_indexes() {
        let dir = TestDir::new("store-league-indexes");
        let conn = open_writer(&dir.join("mymtgo.sqlite")).expect("open");
        for (sql, index) in [
            (
                "SELECT token FROM matches WHERE account_id = 1 AND league_token IS NOT NULL",
                "matches_account_league",
            ),
            (
                "SELECT token FROM matches WHERE account_id = 1 AND tournament_event_id = 7",
                "matches_account_tournament",
            ),
            (
                "SELECT token FROM matches WHERE league_id = 3",
                "matches_league_id",
            ),
            (
                "SELECT token FROM matches WHERE tournament_id = 3",
                "matches_tournament_id",
            ),
        ] {
            let mut stmt = conn
                .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
                .expect("explain");
            let plan: Vec<String> = stmt
                .query_map([], |row| row.get::<_, String>(3))
                .expect("rows")
                .map(|row| row.expect("row"))
                .collect();
            let plan = plan.join("\n");
            assert!(plan.contains(index), "{sql}: {plan}");
        }
    }

    fn insert_file(conn: &Connection) -> i64 {
        conn.execute(
            "INSERT INTO log_files (path, identity_hash, head_hash, head_len, day_anchor, timezone, clock_date, first_seen_at, last_seen_at)
             VALUES ('p', '0', '0', 0, '2026-08-22', 'UTC', '2026-08-22', 0, 0)",
            [],
        )
        .expect("insert file");
        conn.last_insert_rowid()
    }

    fn insert_event(
        conn: &Connection,
        file_id: i64,
        offset: i64,
        kind: Option<&str>,
        quarantined: bool,
    ) -> rusqlite::Result<usize> {
        conn.execute(
            "INSERT INTO log_events (log_file_id, byte_offset, kind, logged_at, raw, quarantined)
             VALUES (?1, ?2, ?3, 0, x'00', ?4)",
            params![file_id, offset, kind, quarantined],
        )
    }

    #[cfg(unix)]
    fn mode_of(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path)
            .expect("metadata")
            .permissions()
            .mode()
            & 0o777
    }

    #[cfg(unix)]
    #[test]
    fn a_fresh_store_is_private_to_the_user() {
        let dir = TestDir::new("store-private");
        let db = dir.join("data/mymtgo.sqlite");
        let conn = open_writer(&db).expect("open");
        assert_eq!(mode_of(&dir.join("data")), 0o700);
        for name in ["mymtgo.sqlite", "mymtgo.sqlite-wal", "mymtgo.sqlite-shm"] {
            assert_eq!(mode_of(&dir.join("data").join(name)), 0o600, "{name}");
        }
        drop(conn);
    }

    #[cfg(unix)]
    #[test]
    fn opening_an_open_store_again_makes_it_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TestDir::new("store-private-again");
        let db = dir.join("data/mymtgo.sqlite");
        drop(open_writer(&db).expect("open"));
        let opened = std::fs::Permissions::from_mode(0o755);
        std::fs::set_permissions(dir.join("data"), opened.clone()).expect("chmod");
        std::fs::set_permissions(&db, std::fs::Permissions::from_mode(0o644)).expect("chmod");
        let conn = open_writer(&db).expect("open again");
        assert_eq!(mode_of(&dir.join("data")), 0o700);
        for name in ["mymtgo.sqlite", "mymtgo.sqlite-wal", "mymtgo.sqlite-shm"] {
            assert_eq!(mode_of(&dir.join("data").join(name)), 0o600, "{name}");
        }
        drop(conn);
    }

    #[test]
    fn a_fresh_database_gets_the_schema_wal_and_foreign_keys() {
        let dir = TestDir::new("store-open");
        let conn = open_writer(&dir.join("db/mymtgo.sqlite")).expect("open");
        let mode: String = conn
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .expect("mode");
        assert_eq!(mode, "wal");
        let foreign_keys: i64 = conn
            .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
            .expect("fk");
        assert_eq!(foreign_keys, 1);
        for table in [
            "log_files",
            "log_events",
            "match_event_archives",
            "accounts",
            "opponents",
            "matches",
            "games",
            "settings",
            "derivations",
        ] {
            let found: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
                    [table],
                    |row| row.get(0),
                )
                .expect("count");
            assert_eq!(found, 1, "table {table}");
        }
    }

    #[test]
    fn reopening_keeps_data() {
        let dir = TestDir::new("store-reopen");
        let path = dir.join("mymtgo.sqlite");
        insert_file(&open_writer(&path).expect("open"));
        let conn = open_writer(&path).expect("reopen");
        let files: i64 = conn
            .query_row("SELECT COUNT(*) FROM log_files", [], |row| row.get(0))
            .expect("count");
        assert_eq!(files, 1);
    }

    #[test]
    fn every_event_kind_is_allowed_and_nothing_else() {
        let dir = TestDir::new("store-kinds");
        let conn = open_writer(&dir.join("mymtgo.sqlite")).expect("open");
        let file = insert_file(&conn);
        for (index, kind) in EventKind::ALL.iter().enumerate() {
            insert_event(&conn, file, index as i64, Some(kind.as_str()), false)
                .unwrap_or_else(|error| panic!("{}: {error}", kind.as_str()));
        }
        assert!(insert_event(&conn, file, 1000, Some("bogus"), false).is_err());
        assert!(insert_event(&conn, file, 1001, None, false).is_err());
        assert!(insert_event(&conn, file, 1002, None, true).is_ok());
    }

    #[test]
    fn foreign_keys_and_strict_types_are_enforced() {
        let dir = TestDir::new("store-strict");
        let conn = open_writer(&dir.join("mymtgo.sqlite")).expect("open");
        assert!(insert_event(&conn, 999, 0, Some("login"), false).is_err());
        let wrong_type = conn.execute(
            "INSERT INTO accounts (username, first_seen_at) VALUES ('Qx', 'not a number')",
            [],
        );
        assert!(wrong_type.is_err());
    }

    #[test]
    fn every_foreign_key_has_an_explicit_delete_rule() {
        let dir = TestDir::new("store-fk-rules");
        let conn = open_writer(&dir.join("mymtgo.sqlite")).expect("open");
        let mut found = Vec::new();
        for table in ["log_events", "matches", "games"] {
            let mut stmt = conn
                .prepare("SELECT \"table\", \"from\", on_delete FROM pragma_foreign_key_list(?1)")
                .expect("prepare");
            let rows = stmt
                .query_map([table], |row| {
                    Ok(format!(
                        "{table}.{} -> {} {}",
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(2)?
                    ))
                })
                .expect("query");
            for row in rows {
                found.push(row.expect("row"));
            }
        }
        found.sort();
        assert_eq!(
            found,
            vec![
                "games.match_id -> matches CASCADE",
                "log_events.log_file_id -> log_files RESTRICT",
                "matches.account_id -> accounts RESTRICT",
                "matches.deck_version_id -> deck_versions SET NULL",
                "matches.deleted_from_league_id -> leagues SET NULL",
                "matches.league_id -> leagues SET NULL",
                "matches.opponent_id -> opponents RESTRICT",
                "matches.tournament_id -> tournaments SET NULL",
            ]
        );
    }

    #[test]
    fn facts_and_registries_cannot_be_deleted_while_referenced() {
        let dir = TestDir::new("store-fk-restrict");
        let conn = open_writer(&dir.join("mymtgo.sqlite")).expect("open");
        let file = insert_file(&conn);
        insert_event(&conn, file, 0, Some("login"), false).expect("event");
        assert!(conn.execute("DELETE FROM log_files", []).is_err());
    }

    #[test]
    fn a_database_without_wal_support_is_reported_as_unsupported() {
        // An in-memory database answers `PRAGMA journal_mode = WAL` with "memory",
        // as a file system without shared memory support would refuse WAL.
        let error = open_writer(Path::new(":memory:")).expect_err("no wal");
        assert!(matches!(error, StoreError::Unsupported(_)), "got {error:?}");
        assert!(error.to_string().contains("WAL"), "{error}");
    }

    #[test]
    fn a_reader_reads_and_its_write_error_is_read_only() {
        let dir = TestDir::new("store-reader-readonly");
        let path = dir.join("mymtgo.sqlite");
        let writer = open_writer(&path).expect("open");
        writer
            .execute("INSERT INTO settings (key, value) VALUES ('seen', '1')", [])
            .expect("insert");
        let reader = open_reader(&path).expect("reader");
        let value: String = reader
            .query_row("SELECT value FROM settings WHERE key = 'seen'", [], |row| {
                row.get(0)
            })
            .expect("a SELECT works");
        assert_eq!(value, "1");
        let error = reader
            .execute("INSERT INTO settings (key, value) VALUES ('a', '1')", [])
            .expect_err("a write is refused");
        assert_eq!(
            error.sqlite_error_code(),
            Some(rusqlite::ErrorCode::ReadOnly),
            "{error}"
        );
    }

    #[test]
    fn a_reader_cannot_write() {
        let dir = TestDir::new("store-reader");
        let path = dir.join("mymtgo.sqlite");
        let _writer = open_writer(&path).expect("open");
        let reader = open_reader(&path).expect("reader");
        assert!(
            reader
                .execute("INSERT INTO settings (key, value) VALUES ('a', '1')", [])
                .is_err()
        );
    }
}
