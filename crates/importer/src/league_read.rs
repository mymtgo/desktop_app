//! Reading 0.x leagues and their match membership (leagues spec section 9,
//! limited spec section 7: draft and sealed leagues keep their kind, set code
//! and course id).
//! The `leagues` table is optional: an old 0.x database without it has no
//! leagues to import. Columns added to it over time (`kind`, `manual`,
//! `notes`, and so on) read as their 0.x defaults when missing. Values are read
//! loosely, as in `read`.

use std::collections::HashMap;

use rusqlite::Connection;
use store::import_leagues::{ImportedLeague, LEAGUE_KINDS, LeagueMember};
use sync_bundles::USER_STATES;

use crate::deck_read::ImportedVersions;
use crate::read::{ImportError, read_error};
use crate::sql::{
    as_integer, as_text, column_list, has_all, has_column, parse_id, parse_utc_ms, select_list,
    table_columns, value,
};

/// Columns without which a `leagues` row means nothing.
const REQUIRED: [&str; 5] = ["id", "token", "name", "format", "started_at"];

/// `matches` columns of league membership, in the order `read_members` reads
/// them.
const MEMBER_COLUMNS: [&str; 3] = ["league_id", "token", "mtgo_id"];

/// Every `(table, columns)` this reader selects. The SQL is built from the
/// same lists, so the coverage ledger cannot drift from it.
pub const READ_COLUMNS: [(&str, &[&str]); 3] = [
    ("leagues", &REQUIRED),
    ("leagues", &OPTIONAL),
    ("matches", &MEMBER_COLUMNS),
];

/// Columns read when present, `NULL` (their 0.x default) otherwise.
const OPTIONAL: [&str; 13] = [
    "created_at",
    "deleted_at",
    "state",
    "deck_version_id",
    "event_id",
    "joined_at",
    "dropped_at",
    "notes",
    "completed_at",
    "manual",
    "kind",
    "set_code",
    "mtgo_course_id",
];

/// The 0.x leagues v1 imports and what was left out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeagueRead {
    /// Not deleted, in 0.x id order: constructed, draft and sealed.
    pub leagues: Vec<ImportedLeague>,
    /// Deleted in 0.x (`deleted_at`).
    pub skipped_deleted: u32,
    /// A start time that does not parse, or a state or kind v1 does not know.
    pub skipped_unreadable: u32,
    /// Every match of the league was left out because its local player is
    /// not one of 0.x's accounts (issue #337).
    pub skipped_not_account: u32,
}

/// The leagues, or `None` when the 0.x database has no `leagues` table (or
/// one without the columns every 0.x had). `versions` maps a 0.x deck version
/// id to the imported deck version it became.
pub(crate) fn read_leagues(
    conn: &Connection,
    versions: &ImportedVersions,
) -> Result<Option<LeagueRead>, ImportError> {
    let present = table_columns(conn, "leagues")?;
    if !has_all(&present, &REQUIRED) {
        return Ok(None);
    }
    let mut members = read_members(conn)?;
    let sql = format!(
        "SELECT {}, {} FROM leagues ORDER BY id",
        column_list("", &REQUIRED),
        select_list(&present, &OPTIONAL)
    );
    let mut stmt = conn.prepare(&sql).map_err(read_error)?;
    let rows = stmt
        .query_map([], |row| {
            Ok(LeagueRow {
                id: row.get(0)?,
                token: as_text(value(row, 1)?).unwrap_or_default(),
                name: as_text(value(row, 2)?).unwrap_or_default(),
                format: as_text(value(row, 3)?).unwrap_or_default(),
                started_at: as_text(value(row, 4)?),
                created_at: as_text(value(row, 5)?),
                deleted_at: as_text(value(row, 6)?),
                state: as_text(value(row, 7)?),
                deck_version_id: as_integer(value(row, 8)?),
                event_id: as_integer(value(row, 9)?),
                joined_at: as_text(value(row, 10)?),
                dropped_at: as_text(value(row, 11)?),
                notes: as_text(value(row, 12)?),
                completed_at: as_text(value(row, 13)?),
                manual: as_integer(value(row, 14)?).is_some_and(|flag| flag != 0),
                kind: as_text(value(row, 15)?),
                set_code: as_text(value(row, 16)?),
                course_id: as_integer(value(row, 17)?),
            })
        })
        .map_err(read_error)?;

    let mut read = LeagueRead {
        leagues: Vec::new(),
        skipped_deleted: 0,
        skipped_unreadable: 0,
        skipped_not_account: 0,
    };
    for row in rows {
        let row = row.map_err(read_error)?;
        if row
            .deleted_at
            .as_deref()
            .is_some_and(|deleted| !deleted.trim().is_empty())
        {
            read.skipped_deleted += 1;
            continue;
        }
        // 0.x's default state is active.
        let state = row
            .state
            .as_deref()
            .unwrap_or("active")
            .trim()
            .to_lowercase();
        // 0.x's default kind is constructed.
        let kind = row
            .kind
            .as_deref()
            .unwrap_or("constructed")
            .trim()
            .to_lowercase();
        let started_at_ms = row.started_at.as_deref().and_then(parse_utc_ms);
        let known = USER_STATES.contains(&state.as_str()) && LEAGUE_KINDS.contains(&kind.as_str());
        let Some(started_at_ms) = started_at_ms.filter(|_| known) else {
            read.skipped_unreadable += 1;
            continue;
        };
        let limited = kind != "constructed";
        read.leagues.push(ImportedLeague {
            legacy_id: row.id,
            // 0.x made only constructed leagues by hand.
            manual: row.manual && !limited,
            set_code: row
                .set_code
                .map(|code| code.trim().to_string())
                .filter(|code| limited && !code.is_empty()),
            course_id: row.course_id.filter(|_| limited),
            kind,
            token: row.token,
            event_id: row.event_id,
            name: row.name,
            format: row.format,
            state,
            started_at_ms,
            joined_at_ms: row.joined_at.as_deref().and_then(parse_utc_ms),
            completed_at_ms: row.completed_at.as_deref().and_then(parse_utc_ms),
            dropped_at_ms: row.dropped_at.as_deref().and_then(parse_utc_ms),
            created_at_ms: row.created_at.as_deref().and_then(parse_utc_ms),
            deck: row
                .deck_version_id
                .and_then(|id| versions.get(&id).cloned()),
            notes: row.notes,
            matches: members.remove(&row.id).unwrap_or_default(),
        });
    }
    Ok(Some(read))
}

/// One `leagues` row, loosely typed.
struct LeagueRow {
    id: i64,
    token: String,
    name: String,
    format: String,
    started_at: Option<String>,
    created_at: Option<String>,
    deleted_at: Option<String>,
    state: Option<String>,
    deck_version_id: Option<i64>,
    event_id: Option<i64>,
    joined_at: Option<String>,
    dropped_at: Option<String>,
    notes: Option<String>,
    completed_at: Option<String>,
    manual: bool,
    kind: Option<String>,
    set_code: Option<String>,
    course_id: Option<i64>,
}

/// Each league's matches (`matches.league_id`), in start order. Empty when
/// the matches table has no `league_id` (an old 0.x).
fn read_members(conn: &Connection) -> Result<HashMap<i64, Vec<LeagueMember>>, ImportError> {
    let mut members: HashMap<i64, Vec<LeagueMember>> = HashMap::new();
    if !has_column(&table_columns(conn, "matches")?, "league_id") {
        return Ok(members);
    }
    let sql = format!(
        "SELECT {} FROM matches WHERE league_id IS NOT NULL ORDER BY started_at, id",
        column_list("", &MEMBER_COLUMNS)
    );
    let mut stmt = conn.prepare(&sql).map_err(read_error)?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                as_integer(value(row, 0)?),
                as_text(value(row, 1)?).unwrap_or_default(),
                as_text(value(row, 2)?),
            ))
        })
        .map_err(read_error)?;
    for row in rows {
        let (league_id, token, mtgo_id) = row.map_err(read_error)?;
        let Some(league_id) = league_id else {
            continue;
        };
        if token.trim().is_empty() {
            continue;
        }
        members.entry(league_id).or_default().push(LeagueMember {
            token,
            mtgo_match_id: mtgo_id.as_deref().and_then(parse_id),
        });
    }
    Ok(members)
}
