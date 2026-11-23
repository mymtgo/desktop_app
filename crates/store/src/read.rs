//! Read queries. Each takes any connection: the tracker's read-only one, or a
//! transaction on the writer thread.

use chrono::{NaiveDate, NaiveTime};
use mtgo_protocol::ClockState;
use rusqlite::{OptionalExtension, params};

use crate::platform::account_of_mtgo;
use crate::write::{decode_raw, unhex};
use crate::{Connection, Platform, StoreError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveLogFile {
    pub id: i64,
    pub path: String,
    pub head_hash: u64,
    pub head_len: u64,
    pub created_at_fs: Option<i64>,
    pub anchor: Option<(u64, u64)>,
    pub timezone: String,
    pub inherited_username: Option<String>,
    pub local_username: Option<String>,
    pub local_login_id: Option<u64>,
    pub cursor_offset: u64,
    pub clock: ClockState,
    pub last_observed_size: u64,
}

/// One `log_files` row as SQLite returns it, before parsing hashes and dates.
struct FileRow {
    id: i64,
    path: String,
    head_hash: String,
    head_len: i64,
    created_at_fs: Option<i64>,
    anchor_offset: Option<i64>,
    anchor_hash: Option<String>,
    timezone: String,
    inherited_username: Option<String>,
    local_username: Option<String>,
    local_login_id: Option<i64>,
    cursor_offset: i64,
    clock_date: String,
    clock_last_secs: Option<i64>,
    last_observed_size: i64,
}

impl FileRow {
    fn into_active(self) -> Result<ActiveLogFile, StoreError> {
        let id = self.id;
        let corrupt =
            move |what: &str| StoreError::Corrupt(format!("log_files row {id}: bad {what}"));
        let head_hash = unhex(&self.head_hash).ok_or_else(|| corrupt("head_hash"))?;
        let anchor = match (self.anchor_offset, self.anchor_hash) {
            (Some(offset), Some(hash)) => Some((
                offset as u64,
                unhex(&hash).ok_or_else(|| corrupt("anchor_hash"))?,
            )),
            _ => None,
        };
        let date = NaiveDate::parse_from_str(&self.clock_date, "%Y-%m-%d")
            .map_err(|_| corrupt("clock_date"))?;
        let last = match self.clock_last_secs {
            Some(secs) => Some(
                NaiveTime::from_num_seconds_from_midnight_opt(secs as u32, 0)
                    .ok_or_else(|| corrupt("clock_last_secs"))?,
            ),
            None => None,
        };
        Ok(ActiveLogFile {
            id,
            path: self.path,
            head_hash,
            head_len: self.head_len as u64,
            created_at_fs: self.created_at_fs,
            anchor,
            timezone: self.timezone,
            inherited_username: self.inherited_username,
            local_username: self.local_username,
            local_login_id: self.local_login_id.map(|v| v as u64),
            cursor_offset: self.cursor_offset as u64,
            clock: ClockState { date, last },
            last_observed_size: self.last_observed_size as u64,
        })
    }
}

/// Every unsealed `log_files` row, oldest first.
pub fn active_log_files(conn: &Connection) -> Result<Vec<ActiveLogFile>, StoreError> {
    let mut stmt = conn.prepare(
        "SELECT id, path, head_hash, head_len, created_at_fs, anchor_offset, anchor_hash, timezone,
             inherited_username, local_username, local_login_id, cursor_offset, clock_date,
             clock_last_secs, last_observed_size
         FROM log_files WHERE sealed_at IS NULL ORDER BY id",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(FileRow {
            id: row.get(0)?,
            path: row.get(1)?,
            head_hash: row.get(2)?,
            head_len: row.get(3)?,
            created_at_fs: row.get(4)?,
            anchor_offset: row.get(5)?,
            anchor_hash: row.get(6)?,
            timezone: row.get(7)?,
            inherited_username: row.get(8)?,
            local_username: row.get(9)?,
            local_login_id: row.get(10)?,
            cursor_offset: row.get(11)?,
            clock_date: row.get(12)?,
            clock_last_secs: row.get(13)?,
            last_observed_size: row.get(14)?,
        })
    })?;
    let mut files = Vec::new();
    for row in rows {
        files.push(row?.into_active()?);
    }
    Ok(files)
}

/// The local username last known for a path (MTGO's rotation opens a new file
/// without logging in again, so a new row inherits it).
pub fn latest_username_for_path(
    conn: &Connection,
    path: &str,
) -> Result<Option<String>, StoreError> {
    let name: Option<Option<String>> = conn
        .query_row(
            "SELECT local_username FROM log_files WHERE path = ?1 ORDER BY id DESC LIMIT 1",
            [path],
            |row| row.get(0),
        )
        .optional()?;
    Ok(name.flatten())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredEvent {
    pub log_file_id: i64,
    pub byte_offset: u64,
    pub logged_at_ms: i64,
    pub raw: String,
}

/// The match's Match IDs and Game IDs come from its events, or from its derived
/// rows once its events are archived.
const LIVE_EVENTS_FOR_MATCH: &str = "
WITH ids AS (
    SELECT match_id FROM log_events WHERE match_token = ?1 AND match_id IS NOT NULL
    UNION SELECT mtgo_match_id FROM matches WHERE token = ?1 AND mtgo_match_id IS NOT NULL
),
game_ids AS (
    SELECT game_id FROM log_events WHERE match_id IN (SELECT match_id FROM ids) AND game_id IS NOT NULL
    UNION SELECT g.mtgo_game_id FROM games g JOIN matches m ON m.id = g.match_id
        WHERE m.token = ?1 AND g.mtgo_game_id IS NOT NULL
)
SELECT log_file_id, byte_offset, logged_at, raw, compressed FROM log_events
WHERE quarantined = 0
  AND (match_token = ?1
       OR match_id IN (SELECT match_id FROM ids)
       OR game_id IN (SELECT game_id FROM game_ids))";

/// A match's events still in `log_events` (not archived), in key order.
pub fn live_events_for_match(
    conn: &Connection,
    token: &str,
) -> Result<Vec<StoredEvent>, StoreError> {
    let mut stmt = conn.prepare_cached(LIVE_EVENTS_FOR_MATCH)?;
    let rows = stmt.query_map([token], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, i64>(2)?,
            row.get::<_, Vec<u8>>(3)?,
            row.get::<_, bool>(4)?,
        ))
    })?;
    let mut events = Vec::new();
    for row in rows {
        let (file, offset, at, raw, compressed) = row?;
        // One unreadable row must not stop the tracker from opening.
        let raw = match decode_raw(raw, compressed) {
            Ok(raw) => raw,
            Err(error) => {
                tracing::warn!(file, offset, %error, "a stored event did not decode; skipped");
                continue;
            }
        };
        events.push(StoredEvent {
            log_file_id: file,
            byte_offset: offset as u64,
            logged_at_ms: at,
            raw,
        });
    }
    sort_and_dedup(&mut events);
    Ok(events)
}

/// Every stored event of a match, archived or not, in key order.
pub fn events_for_match(conn: &Connection, token: &str) -> Result<Vec<StoredEvent>, StoreError> {
    let mut events = crate::compact::archived_events(conn, token)?;
    events.extend(live_events_for_match(conn, token)?);
    sort_and_dedup(&mut events);
    Ok(events)
}

pub fn sort_and_dedup(events: &mut Vec<StoredEvent>) {
    events.sort_by_key(|e| (e.log_file_id, e.byte_offset));
    events.dedup_by_key(|e| (e.log_file_id, e.byte_offset));
}

pub fn token_for_match_id(conn: &Connection, match_id: u64) -> Result<Option<String>, StoreError> {
    let from_events: Option<String> = conn
        .query_row(
            "SELECT match_token FROM log_events WHERE match_id = ?1 AND match_token IS NOT NULL LIMIT 1",
            [match_id as i64],
            |row| row.get(0),
        )
        .optional()?;
    if from_events.is_some() {
        return Ok(from_events);
    }
    Ok(conn
        .query_row(
            "SELECT token FROM matches WHERE mtgo_match_id = ?1 LIMIT 1",
            [match_id as i64],
            |row| row.get(0),
        )
        .optional()?)
}

pub fn match_id_for_game_id(conn: &Connection, game_id: u64) -> Result<Option<u64>, StoreError> {
    let from_events: Option<i64> = conn
        .query_row(
            "SELECT match_id FROM log_events WHERE game_id = ?1 AND match_id IS NOT NULL LIMIT 1",
            [game_id as i64],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(id) = from_events {
        return Ok(Some(id as u64));
    }
    let from_rows: Option<Option<i64>> = conn
        .query_row(
            "SELECT m.mtgo_match_id FROM games g JOIN matches m ON m.id = g.match_id
             WHERE g.mtgo_game_id = ?1 LIMIT 1",
            [game_id as i64],
            |row| row.get(0),
        )
        .optional()?;
    Ok(from_rows.flatten().map(|id| id as u64))
}

pub const FILE_LOGINS: &str = "SELECT byte_offset, raw, compressed FROM log_events
         WHERE log_file_id = ?1 AND kind = 'login' AND quarantined = 0 ORDER BY byte_offset";

/// The Login events of one file: (offset, raw text), in order.
pub fn file_logins(conn: &Connection, log_file_id: i64) -> Result<Vec<(u64, String)>, StoreError> {
    let mut stmt = conn.prepare_cached(FILE_LOGINS)?;
    let rows = stmt.query_map([log_file_id], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, Vec<u8>>(1)?,
            row.get::<_, bool>(2)?,
        ))
    })?;
    let mut logins = Vec::new();
    for row in rows {
        let (offset, raw, compressed) = row?;
        logins.push((offset as u64, decode_raw(raw, compressed)?));
    }
    Ok(logins)
}

pub fn file_inherited_username(
    conn: &Connection,
    log_file_id: i64,
) -> Result<Option<String>, StoreError> {
    let name: Option<Option<String>> = conn
        .query_row(
            "SELECT inherited_username FROM log_files WHERE id = ?1",
            [log_file_id],
            |row| row.get(0),
        )
        .optional()?;
    Ok(name.flatten())
}

/// The first column of every row of a query without parameters.
fn strings(conn: &Connection, sql: &str) -> Result<Vec<String>, StoreError> {
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
    let mut values = Vec::new();
    for row in rows {
        values.push(row?);
    }
    Ok(values)
}

/// Matches and games stored, hidden ones included: the report bundle's
/// summary (release spec 8.1).
pub fn match_and_game_counts(conn: &Connection) -> Result<(u64, u64), StoreError> {
    let count = |sql: &str| -> Result<u64, StoreError> {
        let n: i64 = conn.query_row(sql, [], |row| row.get(0))?;
        Ok(n.max(0) as u64)
    };
    Ok((
        count("SELECT COUNT(*) FROM matches")?,
        count("SELECT COUNT(*) FROM games")?,
    ))
}

/// Every stored player name: local accounts and opponents (registries, spec
/// 4.1). The app keeps them out of error reports.
pub fn player_names(conn: &Connection) -> Result<Vec<String>, StoreError> {
    strings(
        conn,
        "SELECT username FROM accounts UNION SELECT username FROM opponents ORDER BY 1",
    )
}

/// The MTGO accounts' usernames, oldest first: the names that identify the
/// local player in MTGO logs (an Arena account never does).
pub fn known_accounts(conn: &Connection) -> Result<Vec<String>, StoreError> {
    strings(
        conn,
        "SELECT username FROM accounts WHERE platform = 'mtgo' ORDER BY id",
    )
}

/// Every local account's username and MTGO login id, oldest first (the
/// MyMTGO API's account attestation, 07 spec section 4).
pub fn account_logins(conn: &Connection) -> Result<Vec<(String, Option<i64>)>, StoreError> {
    let mut stmt = conn.prepare("SELECT username, login_id FROM accounts ORDER BY id")?;
    let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
    let mut list = Vec::new();
    for row in rows {
        list.push(row?);
    }
    Ok(list)
}

/// Matches that can still change on their own: rehydrated at startup (spec 3).
/// Unfinished matches, and any match active since `active_since_ms`: a finished
/// match can still change with time (a lost connection settles a later result).
pub fn open_matches(
    conn: &Connection,
    active_since_ms: i64,
) -> Result<Vec<(String, u64)>, StoreError> {
    let mut stmt = conn.prepare(
        "SELECT token, revision FROM matches
         WHERE source = 'log' AND (state IN ('in_progress', 'ended') OR last_activity_at >= ?1)
         ORDER BY id",
    )?;
    let rows = stmt.query_map([active_since_ms], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
    })?;
    let mut open = Vec::new();
    for row in rows {
        let (token, revision) = row?;
        open.push((token, revision as u64));
    }
    Ok(open)
}

/// Every match token with a derived row. A manual match (Plan L) is never
/// derived, so it is left out: the projector has nothing to reload for it.
pub fn stored_match_tokens(conn: &Connection) -> Result<Vec<String>, StoreError> {
    strings(
        conn,
        "SELECT token FROM matches WHERE source <> 'manual' ORDER BY id",
    )
}

/// Every 0.x or history copy's token (`source` imported or history). They are
/// stored matches, but the tracker did not show them as live ones.
pub fn copy_match_tokens(conn: &Connection) -> Result<Vec<String>, StoreError> {
    strings(
        conn,
        "SELECT token FROM matches WHERE source IN ('imported', 'history') ORDER BY id",
    )
}

/// Every history copy's token (`source` history): the only copies a 0.x
/// import replaces (`import::import_matches`), so the only ones it can
/// remove.
pub fn history_match_tokens(conn: &Connection) -> Result<Vec<String>, StoreError> {
    strings(
        conn,
        "SELECT token FROM matches WHERE source = 'history' ORDER BY id",
    )
}

/// Every detached match token (a log match that stopped being visible while
/// it carried input): stored, but not shown.
pub fn detached_match_tokens(conn: &Connection) -> Result<Vec<String>, StoreError> {
    strings(
        conn,
        "SELECT token FROM matches WHERE detached_at IS NOT NULL ORDER BY id",
    )
}

/// Every match token with a stored join (live) or archived events. After a
/// restart each one is marked stored, so its next event reloads its history: a
/// match joined without a game state yet has events but no row. A token without
/// a join is lobby noise (another player's match) and never derives a visible
/// result, so it needs no mark; the partial join index keeps this read small.
/// Quarantined rows are stored without subjects, so their token is NULL.
pub fn joined_match_tokens(conn: &Connection) -> Result<Vec<String>, StoreError> {
    strings(conn, JOINED_MATCH_TOKENS)
}

/// The WHERE terms repeat the `log_events_joins` index's, so the index covers it.
pub const JOINED_MATCH_TOKENS: &str = "
SELECT DISTINCT match_token FROM log_events
WHERE match_token IS NOT NULL
  AND kind IN ('game_management_json', 'match_state_changed')
  AND (compressed = 1 OR instr(raw, 'MatchJoinedEventUnderwayState') > 0)
UNION SELECT match_token FROM match_event_archives";

/// Joined matches with no `matches` row: waiting for the local username,
/// failed before their first row, or discarded (issue #51). Read when a new
/// local account appears and once when the tracker opens knowing one, so
/// discarded ones cost little.
pub fn joined_tokens_without_match(conn: &Connection) -> Result<Vec<String>, StoreError> {
    strings(conn, JOINED_TOKENS_WITHOUT_MATCH)
}

/// The WHERE terms repeat the `log_events_joins` index's, so the index covers it.
pub const JOINED_TOKENS_WITHOUT_MATCH: &str = "
SELECT DISTINCT e.match_token FROM log_events e
WHERE e.match_token IS NOT NULL
  AND e.kind IN ('game_management_json', 'match_state_changed')
  AND (e.compressed = 1 OR instr(e.raw, 'MatchJoinedEventUnderwayState') > 0)
  AND NOT EXISTS (SELECT 1 FROM matches m WHERE m.token = e.match_token)
ORDER BY e.match_token";

/// MTGO log matches derived by an older projection (spec 4.1 rule 3). Arena
/// and Endstep matches are log rows too, but MTGO's projection never derived
/// them: rebuilding one would find no MTGO events and remove it.
pub fn stale_matches(conn: &Connection, version: u32) -> Result<Vec<String>, StoreError> {
    let mut stmt = conn.prepare(&format!(
        "SELECT token FROM matches
         WHERE source = 'log' AND derived_version < ?1
           AND {mtgo}
         ORDER BY id",
        mtgo = account_of_mtgo("account_id"),
    ))?;
    let rows = stmt.query_map([version], |row| row.get::<_, String>(0))?;
    let mut tokens = Vec::new();
    for row in rows {
        tokens.push(row?);
    }
    Ok(tokens)
}

/// Matches whose last projection failed (issue #47), sorted by token.
pub fn failed_match_tokens(conn: &Connection) -> Result<Vec<String>, StoreError> {
    strings(conn, "SELECT token FROM match_failures ORDER BY token")
}

/// Failures a newer projection should retry. Like `stale_matches`, but it also
/// finds a match that failed before it ever had a row.
pub fn stale_failures(conn: &Connection, version: u32) -> Result<Vec<String>, StoreError> {
    let mut stmt =
        conn.prepare("SELECT token FROM match_failures WHERE derived_version < ?1 ORDER BY token")?;
    let rows = stmt.query_map([version], |row| row.get::<_, String>(0))?;
    let mut tokens = Vec::new();
    for row in rows {
        tokens.push(row?);
    }
    Ok(tokens)
}

pub fn match_revision(conn: &Connection, token: &str) -> Result<Option<u64>, StoreError> {
    let revision: Option<i64> = conn
        .query_row(
            "SELECT revision FROM matches WHERE token = ?1",
            [token],
            |row| row.get(0),
        )
        .optional()?;
    Ok(revision.map(|r| r as u64))
}

/// The tokens of the stored matches whose format is `ARENA_BRAWL`: what the
/// last release wrote for Brawl ladder matches, which the tracker repairs. A
/// corrected match keeps the player's fix (the upsert skips it), so it is
/// left out rather than queued on every start.
pub fn arena_brawl_tokens(conn: &Connection) -> Result<Vec<String>, StoreError> {
    let mut stmt = conn.prepare(
        "SELECT token FROM matches WHERE format = 'ARENA_BRAWL' AND corrected_at IS NULL",
    )?;
    let rows = stmt.query_map([], |row| row.get(0))?;
    let mut tokens = Vec::new();
    for row in rows {
        tokens.push(row?);
    }
    Ok(tokens)
}

/// Whether the stored match has a non-blank league token or a tournament id
/// (false when there is no such match).
pub fn is_league_or_tournament_match(conn: &Connection, token: &str) -> Result<bool, StoreError> {
    let found: Option<bool> = conn
        .query_row(
            "SELECT TRIM(COALESCE(league_token, '')) <> '' OR tournament_event_id IS NOT NULL
             FROM matches WHERE token = ?1",
            [token],
            |row| row.get(0),
        )
        .optional()?;
    Ok(found.unwrap_or(false))
}

/// The id of the account with this username (an untagged Arena name is the
/// account holding its user id), `None` when there is none yet.
pub fn account_id(conn: &Connection, username: &str) -> Result<Option<i64>, StoreError> {
    Ok(conn
        .query_row(
            "SELECT id FROM accounts WHERE username = ?1",
            [crate::arena::platform_account_username(conn, username)?],
            |row| row.get(0),
        )
        .optional()?)
}

/// The username of the Arena account that played the stored limited match
/// `token`, hidden or not: `None` for any other match. Its Arena runs are
/// synced again when the match is hidden or shown again.
pub fn arena_limited_match_username(
    conn: &Connection,
    token: &str,
) -> Result<Option<String>, StoreError> {
    Ok(conn
        .prepare_cached(
            "SELECT a.username FROM matches m
             JOIN accounts a ON a.id = m.account_id AND a.platform = 'arena'
             WHERE m.token = ?1 AND m.match_type = 'limited'",
        )?
        .query_row([token], |row| row.get(0))
        .optional()?)
}

/// The stored match's account: `None` when there is no such match,
/// `Some(None)` when the match has no account (0.x data without one).
pub fn match_account(conn: &Connection, token: &str) -> Result<Option<Option<i64>>, StoreError> {
    Ok(conn
        .query_row(
            "SELECT account_id FROM matches WHERE token = ?1",
            [token],
            |row| row.get(0),
        )
        .optional()?)
}

/// The account of the league run with this key: `None` when there is no
/// such run, `Some(None)` when the run has no account.
pub fn league_run_account(
    conn: &Connection,
    run_key: &str,
) -> Result<Option<Option<i64>>, StoreError> {
    Ok(conn
        .query_row(
            "SELECT account_id FROM leagues WHERE run_key = ?1",
            [run_key],
            |row| row.get(0),
        )
        .optional()?)
}

/// The run key the league row `id` has now: `None` when there is no such
/// row. A rekey keeps the row and changes its key, so a caller holding
/// the id (the delete toast's Undo, sync identity spec 2.5) reads the key
/// here.
pub fn league_run_key(conn: &Connection, id: i64) -> Result<Option<String>, StoreError> {
    Ok(conn
        .query_row("SELECT run_key FROM leagues WHERE id = ?1", [id], |row| {
            row.get(0)
        })
        .optional()?)
}

/// The earliest `abandons_at` of the attached drafts: when an open draft
/// turns abandoned without a new event.
pub fn next_abandon_at(conn: &Connection) -> Result<Option<i64>, StoreError> {
    Ok(conn.query_row(
        "SELECT MIN(abandons_at) FROM drafts WHERE detached_at IS NULL",
        [],
        |row| row.get(0),
    )?)
}

/// The accounts (`None` for drafts without one) of the attached drafts whose
/// `abandons_at` is before `now_ms`: the open drafts that are abandoned now.
pub fn abandoned_draft_accounts(
    conn: &Connection,
    now_ms: i64,
) -> Result<Vec<Option<i64>>, StoreError> {
    let mut stmt = conn.prepare_cached(
        "SELECT DISTINCT account_id FROM drafts
         WHERE detached_at IS NULL AND abandons_at < ?1 ORDER BY account_id",
    )?;
    let rows = stmt.query_map([now_ms], |row| row.get(0))?;
    let mut accounts = Vec::new();
    for row in rows {
        accounts.push(row?);
    }
    Ok(accounts)
}

pub fn derivation_version(conn: &Connection) -> Result<Option<u32>, StoreError> {
    Ok(conn
        .query_row(
            "SELECT version FROM derivations WHERE name = 'match_projection'",
            [],
            |row| row.get(0),
        )
        .optional()?)
}

/// A match still has live rows when any row carries its token, its Match ID, or
/// one of its games' Game IDs (late id-only rows such as `game_state_update` or
/// `deck_used` can arrive after a compaction). Each test walks an index.
pub const COMPACTABLE_MATCHES: &str = "
SELECT m.token FROM matches m
WHERE m.source = 'log' AND m.state IN ('complete', 'abandoned')
  AND m.last_activity_at <= ?1
  AND (EXISTS (SELECT 1 FROM log_events e WHERE e.match_token = m.token AND e.quarantined = 0)
       OR EXISTS (SELECT 1 FROM log_events e
                  WHERE e.match_id = m.mtgo_match_id AND e.quarantined = 0)
       OR EXISTS (SELECT 1 FROM games g JOIN log_events e ON e.game_id = g.mtgo_game_id
                  WHERE g.match_id = m.id AND e.quarantined = 0))
ORDER BY m.id";

/// Finished matches quiet since `quiet_before_ms` whose events are not all archived.
pub fn compactable_matches(
    conn: &Connection,
    quiet_before_ms: i64,
) -> Result<Vec<String>, StoreError> {
    let mut stmt = conn.prepare(COMPACTABLE_MATCHES)?;
    let rows = stmt.query_map(params![quiet_before_ms], |row| row.get::<_, String>(0))?;
    let mut tokens = Vec::new();
    for row in rows {
        tokens.push(row?);
    }
    Ok(tokens)
}

/// One row of the matches list page (spec 6). Games score comes from `games`.
#[derive(Debug, Clone, PartialEq)]
pub struct MatchRow {
    /// The `matches` row id (the match page and "Edit match" take it).
    pub id: i64,
    pub token: String,
    /// The opponent's display name: an Arena opponent's screen name, else
    /// the username.
    pub opponent: Option<String>,
    pub format: Option<String>,
    pub match_type: Option<String>,
    /// The platform of the match's account.
    pub platform: Platform,
    /// Derived from the log; always `None` for a match imported from 0.x.
    pub league_token: Option<String>,
    pub game_structure: Option<String>,
    pub state: String,
    pub outcome: String,
    pub wins: u32,
    pub losses: u32,
    pub needs_attention: bool,
    pub attention_reason: Option<String>,
    pub started_at_ms: i64,
    pub ended_at_ms: Option<i64>,
    pub revision: u64,
    /// The player's note on the match.
    pub notes: Option<String>,
    /// Entered by hand (`source = 'manual'`).
    pub manual: bool,
    /// Every game, by game number.
    pub games: Vec<RowGame>,
    /// The opponent seat's archetype (archetypes spec section 12).
    pub opponent_archetype: Option<crate::match_list::SeatArchetype>,
    /// The constructed rank a ranked Arena match was played at
    /// (`matches.rank_file_id` and `rank_byte_offset`); `None` otherwise.
    pub rank: Option<MatchRank>,
    /// An Endstep ranked match's rating before and after it; `None` unless
    /// both are stored.
    pub rating: Option<MatchRating>,
}

/// An Endstep ranked result as a match row shows it. The change shown is
/// `after - before`; the stored `delta` is Endstep's score change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MatchRating {
    pub before: i64,
    pub after: i64,
}

/// One constructed rank reading as a match row shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct MatchRank {
    pub class: String,
    pub level: u32,
    pub step: u32,
    pub mythic_percentile: Option<f64>,
    pub mythic_placement: Option<u32>,
}

/// One game of a `MatchRow`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowGame {
    /// `win`, `loss`, `draw` or `unknown`.
    pub result: String,
    pub on_play: Option<bool>,
}

/// The `MatchRow` columns of `listed_matches m` (hidden matches left out)
/// and its opponent `o` (shown by display name), its account `a`, with the
/// opponent seat's archetype `oa`. Other match lists (a deck's matches) add their
/// joins and filters after it, so every list reads its rows the same way
/// (`match_row`). The games column is `result:on_play` per game (`1`, `0`
/// or empty), comma separated, by game number.
pub const MATCH_ROW_SELECT: &str = "
SELECT m.token, COALESCE(o.display_name, o.username), m.format, m.match_type, m.state, m.outcome,
       (SELECT COUNT(*) FROM games g WHERE g.match_id = m.id AND g.result = 'win'),
       (SELECT COUNT(*) FROM games g WHERE g.match_id = m.id AND g.result = 'loss'),
       m.needs_attention, m.attention_reason, m.started_at, m.ended_at, m.revision,
       m.league_token, m.game_structure,
       m.notes,
       (SELECT group_concat(g.result || ':' || COALESCE(g.on_play, ''), ',' ORDER BY g.game_number)
        FROM games g WHERE g.match_id = m.id),
       m.source = 'manual',
       oa.id, oa.name, oa.color_identity,
       m.id, a.platform,
       rk.class, rk.level, rk.step, rk.mythic_percentile, rk.mythic_placement,
       er.rating_before, er.rating_after,
       CASE WHEN oa.source = 'commander' THEN (SELECT json_group_array(json_object(
         'catalog_id', mc.catalog_id, 'name', c.name, 'color_identity', c.color_identity,
         'image', c.image, 'local_image', c.local_image) ORDER BY mc.position)
       FROM match_commanders mc LEFT JOIN cards c ON c.catalog_id = mc.catalog_id
       WHERE mc.match_id = m.id AND mc.seat = 'opponent') END
FROM listed_matches m LEFT JOIN opponents o ON o.id = m.opponent_id
LEFT JOIN accounts a ON a.id = m.account_id
LEFT JOIN match_archetypes ma ON ma.match_id = m.id AND ma.is_opponent = 1
LEFT JOIN archetypes oa ON oa.id = ma.archetype_id
LEFT JOIN arena_rank_observations rk ON rk.arena_log_file_id = m.rank_file_id
    AND rk.byte_offset = m.rank_byte_offset AND rk.ladder = 'constructed'
LEFT JOIN endstep_ranked_results er ON er.match_id = m.id";

/// Newest first, one page: `MATCH_ROW_SELECT` with `?1` the limit and `?2`
/// the offset.
pub fn list_matches_sql() -> String {
    format!("{MATCH_ROW_SELECT}\nORDER BY m.started_at DESC, m.id DESC\nLIMIT ?1 OFFSET ?2")
}

/// One row of `MATCH_ROW_SELECT`.
pub fn match_row(row: &rusqlite::Row) -> rusqlite::Result<MatchRow> {
    let revision: i64 = row.get(12)?;
    Ok(MatchRow {
        id: row.get(21)?,
        token: row.get(0)?,
        opponent: row.get(1)?,
        format: row.get(2)?,
        match_type: row.get(3)?,
        // The schema allows only the two platforms.
        platform: row.get(22)?,
        league_token: row.get(13)?,
        game_structure: row.get(14)?,
        state: row.get(4)?,
        outcome: row.get(5)?,
        wins: row.get(6)?,
        losses: row.get(7)?,
        needs_attention: row.get(8)?,
        attention_reason: row.get(9)?,
        started_at_ms: row.get(10)?,
        ended_at_ms: row.get(11)?,
        revision: revision as u64,
        notes: row.get(15)?,
        manual: row.get(17)?,
        games: row_games(row.get::<_, Option<String>>(16)?.as_deref().unwrap_or("")),
        opponent_archetype: match row.get::<_, Option<i64>>(18)? {
            Some(id) => Some(crate::match_list::SeatArchetype {
                id,
                name: row.get(19)?,
                color_identity: row.get(20)?,
                commander_cards: crate::match_list::commander_cards(
                    row.get::<_, Option<String>>(30)?.as_deref(),
                ),
            }),
            None => None,
        },
        rank: match row.get::<_, Option<String>>(23)? {
            Some(class) => Some(MatchRank {
                class,
                level: row.get(24)?,
                step: row.get(25)?,
                mythic_percentile: row.get(26)?,
                mythic_placement: row.get(27)?,
            }),
            None => None,
        },
        rating: match (
            row.get::<_, Option<i64>>(28)?,
            row.get::<_, Option<i64>>(29)?,
        ) {
            (Some(before), Some(after)) => Some(MatchRating { before, after }),
            _ => None,
        },
    })
}

/// The games column of `MATCH_ROW_SELECT` read back.
fn row_games(column: &str) -> Vec<RowGame> {
    let mut games = Vec::new();
    for part in column.split(',') {
        let Some((result, on_play)) = part.split_once(':') else {
            continue;
        };
        games.push(RowGame {
            result: result.to_string(),
            on_play: match on_play {
                "1" => Some(true),
                "0" => Some(false),
                _ => None,
            },
        });
    }
    games
}

/// Newest first, one page.
pub fn list_matches(
    conn: &Connection,
    limit: u32,
    offset: u32,
) -> Result<Vec<MatchRow>, StoreError> {
    let mut stmt = conn.prepare_cached(&list_matches_sql())?;
    let rows = stmt.query_map(params![limit, offset], match_row)?;
    let mut list = Vec::new();
    for row in rows {
        list.push(row?);
    }
    Ok(list)
}

/// How many matches the list has in total, for pagination.
pub fn count_matches(conn: &Connection) -> Result<u32, StoreError> {
    Ok(conn.query_row("SELECT COUNT(*) FROM listed_matches", [], |row| row.get(0))?)
}

/// Every stored setting as `(key, JSON value)`, sorted by key (spec 4.5).
pub fn settings(conn: &Connection) -> Result<Vec<(String, String)>, StoreError> {
    let mut stmt = conn.prepare_cached("SELECT key, value FROM settings ORDER BY key")?;
    let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
    let mut list = Vec::new();
    for row in rows {
        list.push(row?);
    }
    Ok(list)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::*;
    use crate::write::{
        COMPRESS_OVER, MatchWrite, NewEvent, Seal, WriteBatch, commit, insert_log_file,
    };
    use mtgo_match::{MatchState, MatchView, PROJECTION_VERSION};
    use mtgo_protocol::{EventKind, Subjects};

    fn write(conn: &mut Connection, batch: WriteBatch) {
        commit(conn, &batch).expect("commit");
    }

    /// Only a history copy can be replaced by a 0.x import (#314 row 2), so
    /// the tracker reads those alone around an import.
    #[test]
    fn history_tokens_leave_out_every_other_source() {
        let (_dir, conn) = db("read-history-tokens");
        for (token, source) in [
            ("q-history", "history"),
            ("q-imported", "imported"),
            ("q-manual", "manual"),
        ] {
            insert_manual(&conn, token);
            conn.execute(
                "UPDATE matches SET source = ?2 WHERE token = ?1",
                [token, source],
            )
            .expect("source");
        }
        assert_eq!(history_match_tokens(&conn).expect("read"), ["q-history"]);
        assert_eq!(
            copy_match_tokens(&conn).expect("read"),
            ["q-history", "q-imported"]
        );
    }

    /// A corrected Brawl ladder match keeps the player's fix, so it is not
    /// repaired (the upsert would skip it on every start).
    #[test]
    fn arena_brawl_tokens_leave_out_corrected_matches() {
        let (_dir, conn) = db("read-brawl-tokens");
        for token in ["q-brawl", "q-corrected", "q-historic"] {
            insert_manual(&conn, token);
        }
        conn.execute_batch(
            "UPDATE matches SET format = 'ARENA_BRAWL' WHERE token IN ('q-brawl', 'q-corrected');
             UPDATE matches SET format = 'ARENA_HISTORIC_BRAWL' WHERE token = 'q-historic';
             UPDATE matches SET corrected_at = 5 WHERE token = 'q-corrected';",
        )
        .expect("formats");
        assert_eq!(arena_brawl_tokens(&conn).expect("read"), ["q-brawl"]);
    }

    #[test]
    fn match_and_game_counts_count_every_stored_row() {
        let (_dir, conn) = db("read-counts");
        assert_eq!(match_and_game_counts(&conn).expect("empty"), (0, 0));
        insert_manual(&conn, "manual-a");
        insert_manual(&conn, "manual-b");
        hide(&conn, "manual-b");
        // Two games each, hidden matches included (the report summary).
        assert_eq!(match_and_game_counts(&conn).expect("counted"), (2, 4));
    }

    fn subjects(token: Option<&str>, match_id: Option<u64>, game_id: Option<u64>) -> Subjects {
        Subjects {
            match_token: token.map(str::to_string),
            match_id,
            game_id,
            ..Subjects::default()
        }
    }

    /// A match's events (token, Match ID 99, Game ID 7) mixed with another match's.
    fn match_events(file: i64) -> Vec<NewEvent> {
        let mut quarantined = token_event(file, 50, TOKEN);
        quarantined.quarantined = true;
        vec![
            event_with(
                file,
                0,
                EventKind::GameManagementJson,
                subjects(Some(TOKEN), Some(99), Some(7)),
                "gm",
            ),
            event_with(
                file,
                10,
                EventKind::GameStateUpdate,
                subjects(None, Some(99), Some(7)),
                "state",
            ),
            event_with(
                file,
                20,
                EventKind::DeckUsed,
                subjects(None, None, Some(7)),
                "deck",
            ),
            token_event(file, 30, OTHER),
            event_with(
                file,
                40,
                EventKind::GameStateUpdate,
                subjects(None, Some(55), Some(3)),
                "other state",
            ),
            quarantined,
        ]
    }

    fn offsets(events: &[StoredEvent]) -> Vec<u64> {
        events.iter().map(|e| e.byte_offset).collect()
    }

    #[test]
    fn a_manual_match_is_never_rehydrated_rebuilt_reloaded_or_compacted() {
        let (_dir, conn) = db("read-manual-never-derived");
        insert_manual(&conn, TOKEN);
        assert_eq!(open_matches(&conn, 0).expect("open"), Vec::new());
        assert_eq!(
            stale_matches(&conn, PROJECTION_VERSION).expect("stale"),
            Vec::<String>::new()
        );
        assert_eq!(
            stored_match_tokens(&conn).expect("stored"),
            Vec::<String>::new()
        );
        assert_eq!(
            compactable_matches(&conn, i64::MAX).expect("compactable"),
            Vec::<String>::new()
        );
    }

    #[test]
    fn a_stored_match_is_a_league_match_with_a_league_token_or_tournament_id() {
        let (_dir, mut conn) = db("read-league-match");
        let league = MatchView {
            league_token: Some("Q-league".to_string()),
            ..view("Q-league-match", "Qopponent", 1_000)
        };
        let blank = MatchView {
            league_token: Some("  ".to_string()),
            ..view("Q-blank-match", "Qopponent", 2_000)
        };
        let tournament = MatchView {
            tournament_event_id: Some(500),
            ..view("Q-tournament-match", "Qopponent", 3_000)
        };
        let plain = view("Q-plain-match", "Qopponent", 4_000);
        let mut matches = Vec::new();
        for played in [league, blank, tournament, plain] {
            matches.push(MatchWrite::Upsert {
                view: Box::new(played),
                revision: 1,
            });
        }
        write(
            &mut conn,
            WriteBatch {
                matches,
                ..WriteBatch::default()
            },
        );
        for (token, wanted) in [
            ("Q-league-match", true),
            ("Q-blank-match", false),
            ("Q-tournament-match", true),
            ("Q-plain-match", false),
            ("Q-missing", false),
        ] {
            assert_eq!(
                is_league_or_tournament_match(&conn, token).expect("read"),
                wanted,
                "{token}"
            );
        }
    }

    #[test]
    fn settings_are_read_back_sorted_by_key() {
        let (_dir, mut conn) = db("read-settings");
        write(
            &mut conn,
            WriteBatch {
                settings: vec![
                    ("log_folder".to_string(), "\"C:/logs\"".to_string()),
                    ("close_to_tray".to_string(), "true".to_string()),
                ],
                ..WriteBatch::default()
            },
        );
        assert_eq!(
            settings(&conn).expect("settings"),
            vec![
                ("close_to_tray".to_string(), "true".to_string()),
                ("log_folder".to_string(), "\"C:/logs\"".to_string()),
            ]
        );
    }

    #[test]
    fn list_rows_carry_revision_and_attention() {
        let (_dir, mut conn) = db("read-list-fields");
        write(
            &mut conn,
            WriteBatch {
                matches: vec![
                    MatchWrite::Upsert {
                        view: Box::new(view(TOKEN, "Qopponent", 1_000)),
                        revision: 7,
                    },
                    MatchWrite::Upsert {
                        view: Box::new(view(OTHER, "Qother", 2_000)),
                        revision: 2,
                    },
                ],
                ..WriteBatch::default()
            },
        );
        write(
            &mut conn,
            WriteBatch {
                matches: vec![MatchWrite::Attention {
                    token: TOKEN.to_string(),
                    reason: "segments and game ids disagree".to_string(),
                }],
                ..WriteBatch::default()
            },
        );
        let rows = list_matches(&conn, 10, 0).expect("list");
        assert_eq!(count_matches(&conn).expect("count"), 2);
        assert_eq!((rows[0].token.as_str(), rows[0].revision), (OTHER, 2));
        assert!(!rows[0].needs_attention);
        assert_eq!(rows[0].attention_reason, None);
        assert_eq!((rows[1].token.as_str(), rows[1].revision), (TOKEN, 7));
        assert!(rows[1].needs_attention);
        assert_eq!(
            rows[1].attention_reason.as_deref(),
            Some("segments and game ids disagree")
        );
    }

    #[test]
    fn list_rows_carry_the_note_and_each_game_in_game_order() {
        let (_dir, mut conn) = db("read-list-games");
        let mut played = view(TOKEN, "Qopponent", 1_000);
        // Stored out of order: the row lists games by number.
        played.games.swap(0, 2);
        played.games[0].on_play = None;
        played.games[1].result = mtgo_match::GameResult::Unknown;
        write(
            &mut conn,
            WriteBatch {
                matches: vec![
                    MatchWrite::Upsert {
                        view: Box::new(played),
                        revision: 1,
                    },
                    MatchWrite::Upsert {
                        view: Box::new(view(OTHER, "Qother", 2_000)),
                        revision: 1,
                    },
                ],
                ..WriteBatch::default()
            },
        );
        conn.execute(
            "UPDATE matches SET notes = 'Q note' WHERE token = ?1",
            [TOKEN],
        )
        .expect("note");
        let rows = list_matches(&conn, 10, 0).expect("list");
        assert_eq!(rows[0].token, OTHER);
        assert_eq!(rows[0].notes, None);
        assert_eq!(rows[1].notes.as_deref(), Some("Q note"));
        let games: Vec<(&str, Option<bool>)> = rows[1]
            .games
            .iter()
            .map(|game| (game.result.as_str(), game.on_play))
            .collect();
        assert_eq!(
            games,
            vec![("win", Some(true)), ("unknown", Some(false)), ("win", None)]
        );
    }

    #[test]
    fn list_rows_carry_the_league_token_and_game_structure() {
        let (_dir, mut conn) = db("read-list-league");
        let mut league = view(TOKEN, "Qopponent", 1_000);
        league.league_token = Some("d2050286-53fd-4072-804f-190d6a3c030a".to_string());
        league.game_structure = Some("Modern".to_string());
        write(
            &mut conn,
            WriteBatch {
                matches: vec![
                    MatchWrite::Upsert {
                        view: Box::new(league),
                        revision: 1,
                    },
                    MatchWrite::Upsert {
                        view: Box::new(view(OTHER, "Qother", 2_000)),
                        revision: 1,
                    },
                ],
                ..WriteBatch::default()
            },
        );
        let rows = list_matches(&conn, 10, 0).expect("list");
        assert_eq!(rows[0].token, OTHER);
        assert_eq!(rows[0].league_token, None);
        assert_eq!(rows[0].game_structure, None);
        assert_eq!(rows[1].token, TOKEN);
        assert_eq!(
            rows[1].league_token.as_deref(),
            Some("d2050286-53fd-4072-804f-190d6a3c030a")
        );
        assert_eq!(rows[1].game_structure.as_deref(), Some("Modern"));
    }

    #[test]
    fn compaction_never_archives_league_events() {
        let (_dir, mut conn) = db("read-compact-league");
        let file = insert_log_file(&conn, &new_log_file("C:/mtgo.log")).expect("file");
        let mut events = match_events(file);
        events.push(event_with(
            file,
            5,
            EventKind::LeagueJoined,
            Subjects {
                league_token: Some("d2050286-53fd-4072-804f-190d6a3c030a".to_string()),
                league_event_id: Some(10397),
                ..Subjects::default()
            },
            "league joined",
        ));
        events.push(event_with(
            file,
            15,
            EventKind::LeagueDropped,
            Subjects::default(),
            "league dropped",
        ));
        write(
            &mut conn,
            WriteBatch {
                events,
                matches: vec![MatchWrite::Upsert {
                    view: Box::new(view(TOKEN, "Qopponent", 1_000)),
                    revision: 1,
                }],
                ..WriteBatch::default()
            },
        );
        write(
            &mut conn,
            WriteBatch {
                compact: vec![TOKEN.to_string()],
                now_ms: 9,
                ..WriteBatch::default()
            },
        );
        assert_eq!(
            number(
                &conn,
                "SELECT COUNT(*) FROM log_events WHERE kind IN ('league_joined', 'league_dropped')"
            ),
            2,
            "league events have no match subjects, so they stay in log_events"
        );
        assert_eq!(
            offsets(&events_for_match(&conn, TOKEN).expect("history")),
            vec![0, 10, 20],
            "and they never join a match's history"
        );
    }

    #[test]
    fn a_corrupt_stored_event_is_skipped_not_fatal() {
        let (_dir, mut conn) = db("read-corrupt-event");
        let file = insert_log_file(&conn, &new_log_file("C:/mtgo.log")).expect("file");
        write(
            &mut conn,
            WriteBatch {
                events: match_events(file),
                ..WriteBatch::default()
            },
        );
        // Marked compressed but not zstd: the row cannot be decoded.
        conn.execute(
            "UPDATE log_events SET raw = X'0102', compressed = 1 WHERE byte_offset = 10",
            [],
        )
        .expect("corrupt");
        assert_eq!(
            offsets(&events_for_match(&conn, TOKEN).expect("a bad row does not fail the read")),
            vec![0, 20]
        );
    }

    #[test]
    fn a_match_s_events_follow_its_token_match_id_and_game_ids() {
        let (_dir, mut conn) = db("read-history");
        let file = insert_log_file(&conn, &new_log_file("C:/mtgo.log")).expect("file");
        write(
            &mut conn,
            WriteBatch {
                events: match_events(file),
                ..WriteBatch::default()
            },
        );
        assert_eq!(
            offsets(&events_for_match(&conn, TOKEN).expect("history")),
            vec![0, 10, 20]
        );
        assert_eq!(
            token_for_match_id(&conn, 99).expect("lookup"),
            Some(TOKEN.to_string())
        );
        assert_eq!(match_id_for_game_id(&conn, 7).expect("lookup"), Some(99));
        assert_eq!(token_for_match_id(&conn, 55).expect("lookup"), None);
    }

    #[test]
    fn compaction_archives_a_match_and_reads_back_identically() {
        let (_dir, mut conn) = db("read-compact");
        let file = insert_log_file(&conn, &new_log_file("C:/mtgo.log")).expect("file");
        write(
            &mut conn,
            WriteBatch {
                events: match_events(file),
                matches: vec![MatchWrite::Upsert {
                    view: Box::new(view(TOKEN, "Qopponent", 1_000)),
                    revision: 1,
                }],
                ..WriteBatch::default()
            },
        );
        let before = events_for_match(&conn, TOKEN).expect("history");

        write(
            &mut conn,
            WriteBatch {
                compact: vec![TOKEN.to_string()],
                now_ms: 9,
                ..WriteBatch::default()
            },
        );
        assert_eq!(
            number(&conn, "SELECT COUNT(*) FROM match_event_archives"),
            1
        );
        // Only the quarantined row stays behind: quarantined events are never archived.
        assert_eq!(
            number(
                &conn,
                "SELECT COUNT(*) FROM log_events WHERE quarantined = 0 AND (match_token = 'f5e33a1f-c2e7-4678-b30d-309b63f17a40' OR match_id = 99 OR game_id = 7)"
            ),
            0
        );
        assert_eq!(events_for_match(&conn, TOKEN).expect("history"), before);
        assert_eq!(
            crate::compact::compacted_at(&conn, TOKEN).expect("stamp"),
            Some(9)
        );
        assert_eq!(
            crate::compact::compacted_at(&conn, OTHER).expect("stamp"),
            None
        );
        // Lookups still work through the derived rows.
        assert_eq!(
            token_for_match_id(&conn, 99).expect("lookup"),
            Some(TOKEN.to_string())
        );
        assert_eq!(match_id_for_game_id(&conn, 7).expect("lookup"), Some(99));

        // A late event is read with the archive and joins it on the next compaction.
        write(
            &mut conn,
            WriteBatch {
                events: vec![token_event(file, 60, TOKEN)],
                ..WriteBatch::default()
            },
        );
        assert_eq!(
            events_for_match(&conn, TOKEN).expect("history").len(),
            before.len() + 1
        );
        write(
            &mut conn,
            WriteBatch {
                compact: vec![TOKEN.to_string()],
                now_ms: 10,
                ..WriteBatch::default()
            },
        );
        assert_eq!(
            number(&conn, "SELECT event_count FROM match_event_archives"),
            4
        );
        assert_eq!(
            events_for_match(&conn, TOKEN).expect("history").len(),
            before.len() + 1
        );
    }

    #[test]
    fn logins_and_inherited_usernames_are_read_per_file() {
        let (_dir, mut conn) = db("read-logins");
        let file = insert_log_file(&conn, &new_log_file("C:/mtgo.log")).expect("file");
        let login = event_with(
            file,
            5,
            EventKind::Login,
            Subjects::default(),
            "12:00:00 [INF] (Login|MtGO Login Success) Username: Qnext (7)",
        );
        write(
            &mut conn,
            WriteBatch {
                events: vec![login],
                ..WriteBatch::default()
            },
        );
        assert_eq!(
            file_inherited_username(&conn, file).expect("inherited"),
            Some("Qlocal".to_string())
        );
        let logins = file_logins(&conn, file).expect("logins");
        assert_eq!(logins.len(), 1);
        assert_eq!(logins[0].0, 5);
    }

    #[test]
    fn active_files_round_trip_their_progress_until_sealed() {
        let (_dir, mut conn) = db("read-files");
        let file = insert_log_file(&conn, &new_log_file("C:/mtgo.log")).expect("file");
        write(
            &mut conn,
            WriteBatch {
                files: vec![progress(file, 20)],
                ..WriteBatch::default()
            },
        );
        let active = active_log_files(&conn).expect("files");
        assert_eq!(active.len(), 1);
        let p = progress(file, 20);
        assert_eq!(active[0].cursor_offset, 20);
        assert_eq!(active[0].clock, p.clock);
        assert_eq!(active[0].anchor, p.anchor);
        assert_eq!(active[0].head_hash, p.head_hash);
        assert_eq!(active[0].local_username, p.local_username);
        assert_eq!(active[0].local_login_id, p.local_login_id);
        assert_eq!(active[0].inherited_username.as_deref(), Some("Qlocal"));

        write(
            &mut conn,
            WriteBatch {
                seals: vec![Seal {
                    log_file_id: file,
                    reason: "truncated".to_string(),
                    at_ms: 5,
                }],
                ..WriteBatch::default()
            },
        );
        assert!(active_log_files(&conn).expect("files").is_empty());
        assert_eq!(
            latest_username_for_path(&conn, "C:/mtgo.log").expect("name"),
            Some("Qlocal".to_string())
        );
    }

    #[test]
    fn open_stored_and_stale_matches() {
        let (_dir, mut conn) = db("read-open");
        let mut live = view(OTHER, "Qopponent", 2_000);
        live.state = MatchState::InProgress;
        write(
            &mut conn,
            WriteBatch {
                matches: vec![
                    MatchWrite::Upsert {
                        view: Box::new(view(TOKEN, "Qopponent", 1_000)),
                        revision: 4,
                    },
                    MatchWrite::Upsert {
                        view: Box::new(live),
                        revision: 6,
                    },
                ],
                ..WriteBatch::default()
            },
        );
        // `view` ends (last activity) 4 seconds after it starts.
        assert_eq!(
            open_matches(&conn, 5_001).expect("open"),
            vec![(OTHER.to_string(), 6)],
            "a quiet complete match is not open"
        );
        assert_eq!(
            open_matches(&conn, 5_000).expect("open"),
            vec![(TOKEN.to_string(), 4), (OTHER.to_string(), 6)],
            "a recently active complete match can still change with time"
        );
        let mut stored = stored_match_tokens(&conn).expect("stored");
        stored.sort();
        assert_eq!(stored, vec![OTHER.to_string(), TOKEN.to_string()]);
        assert_eq!(
            stale_matches(&conn, PROJECTION_VERSION + 1)
                .expect("stale")
                .len(),
            2
        );
        assert!(
            stale_matches(&conn, PROJECTION_VERSION)
                .expect("stale")
                .is_empty()
        );
        assert_eq!(match_revision(&conn, TOKEN).expect("revision"), Some(4));
        assert_eq!(
            known_accounts(&conn).expect("accounts"),
            vec!["Qlocal".to_string()]
        );
        assert_eq!(derivation_version(&conn).expect("version"), None);
    }

    /// An Arena or Endstep match is a log row too, but MTGO's projection
    /// never derived it: a projection bump must not rebuild (and so remove) it.
    #[test]
    fn stale_matches_are_mtgo_matches_only() {
        let (_dir, mut conn) = db("read-stale-mtgo-only");
        write(
            &mut conn,
            WriteBatch {
                matches: vec![
                    MatchWrite::Upsert {
                        view: Box::new(view(TOKEN, "Qopponent", 1_000)),
                        revision: 1,
                    },
                    MatchWrite::Upsert {
                        view: Box::new(view(OTHER, "Qopponent", 2_000)),
                        revision: 1,
                    },
                ],
                ..WriteBatch::default()
            },
        );
        conn.execute(
            "INSERT INTO accounts (username, first_seen_at, platform) VALUES ('Qarena#12345', 0, 'arena')",
            [],
        )
        .expect("arena account");
        conn.execute(
            "UPDATE matches SET account_id = (SELECT id FROM accounts WHERE platform = 'arena')
             WHERE token = ?1",
            [OTHER],
        )
        .expect("an Arena match");
        assert_eq!(
            stale_matches(&conn, PROJECTION_VERSION + 1).expect("stale"),
            vec![TOKEN.to_string()]
        );
    }

    #[test]
    fn known_accounts_are_mtgo_accounts_only() {
        let (_dir, conn) = db("read-known-mtgo");
        crate::test_support::account(&conn, "Qlocal");
        conn.execute(
            "INSERT INTO accounts (username, first_seen_at, platform) VALUES ('Qarena#12345', 0, 'arena')",
            [],
        )
        .expect("arena account");
        assert_eq!(
            known_accounts(&conn).expect("accounts"),
            vec!["Qlocal".to_string()],
            "an Arena account never identifies an MTGO log"
        );
    }

    const JOIN_HEADER: &str = "12:00:00 [INF] (Game Management|Processing Registered Handler for GsMessageMessage in MatchJoinedEventUnderwayState) Message: {}";

    #[test]
    fn joined_match_tokens_name_joined_and_archived_matches_once() {
        const STATE_JOINED: &str = "aaaaaaaa-0000-0000-0000-000000000003";
        const BIG_JOIN: &str = "aaaaaaaa-0000-0000-0000-000000000004";
        let (_dir, mut conn) = db("read-joined-tokens");
        let file = insert_log_file(&conn, &new_log_file("C:/mtgo.log")).expect("file");
        // A quarantined row is stored without subjects, as the tracker stores it.
        let mut quarantined = event_with(
            file,
            40,
            EventKind::MatchStateChanged,
            Subjects::default(),
            "poison",
        );
        quarantined.kind = None;
        quarantined.quarantined = true;
        let state_join = event_with(
            file,
            50,
            EventKind::MatchStateChanged,
            subjects(Some(STATE_JOINED), None, None),
            "12:00:00 [INF] (Game Management|Match State Changed for aaaaaaaa-0000-0000-0000-000000000003 from MatchJoinedWaitingForGameToStartState to MatchJoinedEventUnderwayState)",
        );
        // Stored compressed, so its text cannot be searched: it still counts.
        let big_join_text = format!("{JOIN_HEADER}{}", " ".repeat(COMPRESS_OVER));
        let big_join = event_with(
            file,
            70,
            EventKind::GameManagementJson,
            subjects(Some(BIG_JOIN), Some(98), Some(6)),
            &big_join_text,
        );
        write(
            &mut conn,
            WriteBatch {
                events: vec![
                    event_with(
                        file,
                        0,
                        EventKind::GameManagementJson,
                        subjects(Some(TOKEN), Some(99), Some(7)),
                        JOIN_HEADER,
                    ),
                    token_event(file, 10, TOKEN),
                    // Lobby noise: another player's match, never joined.
                    token_event(file, 20, OTHER),
                    quarantined,
                    state_join,
                    big_join,
                ],
                matches: vec![MatchWrite::Upsert {
                    view: Box::new(view(TOKEN, "Qopponent", 1_000)),
                    revision: 1,
                }],
                ..WriteBatch::default()
            },
        );
        write(
            &mut conn,
            WriteBatch {
                compact: vec![TOKEN.to_string()],
                now_ms: 9,
                ..WriteBatch::default()
            },
        );
        assert_eq!(
            number(
                &conn,
                "SELECT COUNT(*) FROM log_events WHERE match_token = 'f5e33a1f-c2e7-4678-b30d-309b63f17a40'"
            ),
            0,
            "the match's events are archived"
        );
        // A late event: the token is now both live and archived.
        write(
            &mut conn,
            WriteBatch {
                events: vec![token_event(file, 60, TOKEN)],
                ..WriteBatch::default()
            },
        );
        let mut tokens = joined_match_tokens(&conn).expect("tokens");
        tokens.sort();
        assert_eq!(
            tokens,
            vec![
                STATE_JOINED.to_string(),
                BIG_JOIN.to_string(),
                TOKEN.to_string()
            ]
        );
    }

    #[test]
    fn the_matches_list_is_newest_first_with_game_scores() {
        let (_dir, mut conn) = db("read-list");
        let tokens = [
            ("aaaaaaaa-0000-0000-0000-000000000001", 1_000),
            ("aaaaaaaa-0000-0000-0000-000000000002", 3_000),
            ("aaaaaaaa-0000-0000-0000-000000000003", 2_000),
        ];
        let matches = tokens
            .iter()
            .map(|(token, started)| MatchWrite::Upsert {
                view: Box::new(view(token, "Qopponent", *started)),
                revision: 1,
            })
            .collect();
        write(
            &mut conn,
            WriteBatch {
                matches,
                ..WriteBatch::default()
            },
        );
        let rows = list_matches(&conn, 10, 0).expect("list");
        let started: Vec<i64> = rows.iter().map(|r| r.started_at_ms).collect();
        assert_eq!(started, vec![3_000, 2_000, 1_000]);
        assert_eq!((rows[0].wins, rows[0].losses), (2, 1));
        assert_eq!(rows[0].opponent.as_deref(), Some("Qopponent"));
        assert_eq!(rows[0].outcome, "win");
        assert_eq!(
            list_matches(&conn, 1, 1).expect("page")[0].started_at_ms,
            2_000
        );
    }

    #[test]
    fn arena_rows_show_the_display_name() {
        let (_dir, mut conn) = db("read-arena-name");
        let matches = [(TOKEN, "Qopponent", 1_000), (OTHER, "arena:u1", 2_000)]
            .iter()
            .map(|(token, opponent, started)| MatchWrite::Upsert {
                view: Box::new(view(token, opponent, *started)),
                revision: 1,
            })
            .collect();
        write(
            &mut conn,
            WriteBatch {
                matches,
                ..WriteBatch::default()
            },
        );
        move_match_to_arena(&conn, OTHER);
        name_arena_opponent(&conn, "arena:u1", "Opponent");
        let rows = list_matches(&conn, 10, 0).expect("list");
        let shown: Vec<(&str, Option<&str>, Platform)> = rows
            .iter()
            .map(|row| (row.token.as_str(), row.opponent.as_deref(), row.platform))
            .collect();
        assert_eq!(
            shown,
            vec![
                (OTHER, Some("Opponent"), Platform::Arena),
                (TOKEN, Some("Qopponent"), Platform::Mtgo),
            ]
        );
    }

    #[test]
    fn list_rows_carry_the_rank_the_match_was_played_at() {
        let (_dir, mut conn) = db("read-list-rank");
        let matches = [(TOKEN, 1_000), (OTHER, 2_000)]
            .iter()
            .map(|(token, started)| MatchWrite::Upsert {
                view: Box::new(view(token, "Qopponent", *started)),
                revision: 1,
            })
            .collect();
        write(
            &mut conn,
            WriteBatch {
                matches,
                ..WriteBatch::default()
            },
        );
        move_match_to_arena(&conn, OTHER);
        conn.execute_batch(
            "INSERT INTO arena_log_files (id, path, identity_hash, head_hash, head_len,
                 first_seen_at) VALUES (1, '/arena/Player.log', 1, 2, 3, 4);
             INSERT INTO arena_rank_observations (account_id, ladder, season, observed_at,
                 class, level, step, matches_won, matches_lost, mythic_percentile,
                 mythic_placement, arena_log_file_id, byte_offset)
             SELECT id, 'constructed', 42, 10, 'Mythic', 1, 0, 4, 5, 85.4, 120, 1, 100
             FROM accounts WHERE platform = 'arena';
             UPDATE matches SET rank_file_id = 1, rank_byte_offset = 100
             WHERE account_id = (SELECT id FROM accounts WHERE platform = 'arena');",
        )
        .expect("rank");
        let rows = list_matches(&conn, 10, 0).expect("list");
        let ranks: Vec<(&str, Option<MatchRank>)> = rows
            .iter()
            .map(|row| (row.token.as_str(), row.rank.clone()))
            .collect();
        assert_eq!(
            ranks,
            vec![
                (
                    OTHER,
                    Some(MatchRank {
                        class: "Mythic".to_string(),
                        level: 1,
                        step: 0,
                        mythic_percentile: Some(85.4),
                        mythic_placement: Some(120),
                    })
                ),
                (TOKEN, None),
            ]
        );
    }

    #[test]
    fn a_hidden_match_is_left_out_of_the_list_and_its_count() {
        let (_dir, mut conn) = db("read-list-hidden");
        let matches = [(TOKEN, 1_000), (OTHER, 2_000)]
            .iter()
            .map(|(token, started)| MatchWrite::Upsert {
                view: Box::new(view(token, "Qopponent", *started)),
                revision: 1,
            })
            .collect();
        write(
            &mut conn,
            WriteBatch {
                matches,
                ..WriteBatch::default()
            },
        );
        hide(&conn, TOKEN);
        let rows = list_matches(&conn, 10, 0).expect("list");
        let tokens: Vec<&str> = rows.iter().map(|row| row.token.as_str()).collect();
        assert_eq!(tokens, vec![OTHER]);
        assert_eq!(count_matches(&conn).expect("count"), 1);
        // The row itself stays: the tracker still derives and stores it.
        assert_eq!(match_revision(&conn, TOKEN).expect("revision"), Some(1));
    }

    #[test]
    fn the_matches_list_walks_the_started_at_index() {
        let (_dir, conn) = db("read-plan");
        let mut stmt = conn
            .prepare(&format!("EXPLAIN QUERY PLAN {}", list_matches_sql()))
            .expect("explain");
        let details: Vec<String> = stmt
            .query_map([10, 0], |row| row.get::<_, String>(3))
            .expect("rows")
            .map(|row| row.expect("row"))
            .collect();
        let plan = details.join("\n");
        assert!(plan.contains("matches_started_at"), "plan:\n{plan}");
        // Each row's games are put in order in a small B-tree (at most a
        // few games); the list itself must not be sorted.
        assert!(!plan.contains("TEMP B-TREE FOR ORDER BY"), "plan:\n{plan}");
    }

    #[test]
    fn the_login_lookup_walks_the_partial_login_index() {
        let (_dir, conn) = db("read-login-plan");
        let mut stmt = conn
            .prepare(&format!("EXPLAIN QUERY PLAN {FILE_LOGINS}"))
            .expect("explain");
        let details: Vec<String> = stmt
            .query_map([1], |row| row.get::<_, String>(3))
            .expect("rows")
            .map(|row| row.expect("row"))
            .collect();
        let plan = details.join("\n");
        assert!(plan.contains("log_events_logins"), "plan:\n{plan}");
        assert!(!plan.contains("TEMP B-TREE"), "plan:\n{plan}");
    }

    #[test]
    fn joined_match_tokens_walk_the_partial_join_index() {
        let (_dir, conn) = db("read-joined-tokens-plan");
        let mut stmt = conn
            .prepare(&format!("EXPLAIN QUERY PLAN {JOINED_MATCH_TOKENS}"))
            .expect("explain");
        let details: Vec<String> = stmt
            .query_map([], |row| row.get::<_, String>(3))
            .expect("rows")
            .map(|row| row.expect("row"))
            .collect();
        let plan = details.join("\n");
        // Only join rows are visited (lobby noise is never read). Their text is
        // rechecked, which stays cheap: compaction archives a finished match's
        // rows, so the index holds the joins of recent and open matches only.
        assert!(
            plan.contains("SEARCH log_events USING INDEX log_events_joins"),
            "plan:\n{plan}"
        );
        assert!(!plan.contains("SCAN log_events"), "plan:\n{plan}");
    }

    #[test]
    fn finished_quiet_matches_with_live_events_are_compactable() {
        let (_dir, mut conn) = db("read-compactable");
        let file = insert_log_file(&conn, &new_log_file("C:/mtgo.log")).expect("file");
        let mut live = view(OTHER, "Qopponent", 2_000);
        live.state = MatchState::InProgress;
        write(
            &mut conn,
            WriteBatch {
                events: vec![token_event(file, 0, TOKEN), token_event(file, 10, OTHER)],
                matches: vec![
                    MatchWrite::Upsert {
                        view: Box::new(view(TOKEN, "Qopponent", 1_000)),
                        revision: 1,
                    },
                    MatchWrite::Upsert {
                        view: Box::new(live),
                        revision: 1,
                    },
                ],
                ..WriteBatch::default()
            },
        );
        // `view` ends 4 seconds after it starts.
        assert_eq!(
            compactable_matches(&conn, 5_000).expect("compactable"),
            vec![TOKEN.to_string()]
        );
        assert!(
            compactable_matches(&conn, 4_999)
                .expect("compactable")
                .is_empty()
        );
    }

    #[test]
    fn late_id_only_events_make_a_compacted_match_compactable_again() {
        let (_dir, mut conn) = db("read-compactable-late-ids");
        let file = insert_log_file(&conn, &new_log_file("C:/mtgo.log")).expect("file");
        write(
            &mut conn,
            WriteBatch {
                events: vec![token_event(file, 0, TOKEN)],
                matches: vec![MatchWrite::Upsert {
                    view: Box::new(view(TOKEN, "Qopponent", 1_000)),
                    revision: 1,
                }],
                ..WriteBatch::default()
            },
        );
        let compact = |conn: &mut Connection, now_ms: i64| {
            write(
                conn,
                WriteBatch {
                    compact: vec![TOKEN.to_string()],
                    now_ms,
                    ..WriteBatch::default()
                },
            );
        };
        compact(&mut conn, 10);
        assert!(
            compactable_matches(&conn, 10_000)
                .expect("compactable")
                .is_empty()
        );

        // A late `deck_used` carries only Game ID 9, one of the match's games.
        write(
            &mut conn,
            WriteBatch {
                events: vec![event_with(
                    file,
                    100,
                    EventKind::DeckUsed,
                    subjects(None, None, Some(9)),
                    "deck",
                )],
                ..WriteBatch::default()
            },
        );
        assert_eq!(
            compactable_matches(&conn, 10_000).expect("compactable"),
            vec![TOKEN.to_string()]
        );
        compact(&mut conn, 11);
        assert!(
            compactable_matches(&conn, 10_000)
                .expect("compactable")
                .is_empty()
        );

        // A late row carrying only the match's Match ID.
        write(
            &mut conn,
            WriteBatch {
                events: vec![event_with(
                    file,
                    110,
                    EventKind::GameStateUpdate,
                    subjects(None, Some(99), None),
                    "state",
                )],
                ..WriteBatch::default()
            },
        );
        assert_eq!(
            compactable_matches(&conn, 10_000).expect("compactable"),
            vec![TOKEN.to_string()]
        );
        compact(&mut conn, 12);
        assert!(
            compactable_matches(&conn, 10_000)
                .expect("compactable")
                .is_empty()
        );
        assert_eq!(
            number(&conn, "SELECT event_count FROM match_event_archives"),
            3
        );

        // Rows of another match's ids are not this match's.
        write(
            &mut conn,
            WriteBatch {
                events: vec![event_with(
                    file,
                    120,
                    EventKind::GameStateUpdate,
                    subjects(None, Some(55), Some(3)),
                    "other state",
                )],
                ..WriteBatch::default()
            },
        );
        assert!(
            compactable_matches(&conn, 10_000)
                .expect("compactable")
                .is_empty()
        );
    }

    #[test]
    fn the_compactable_matches_query_never_scans_log_events() {
        let (_dir, conn) = db("read-compactable-plan");
        let mut stmt = conn
            .prepare(&format!("EXPLAIN QUERY PLAN {COMPACTABLE_MATCHES}"))
            .expect("explain");
        let details: Vec<String> = stmt
            .query_map([0], |row| row.get::<_, String>(3))
            .expect("rows")
            .map(|row| row.expect("row"))
            .collect();
        let plan = details.join("\n");
        assert!(!plan.contains("SCAN log_events"), "plan:\n{plan}");
        assert!(!plan.contains("SCAN e"), "plan:\n{plan}");
        for index in [
            "log_events_match_token",
            "log_events_match_id",
            "log_events_game_id",
        ] {
            assert!(plan.contains(index), "{index} unused, plan:\n{plan}");
        }
    }

    #[test]
    fn re_inserting_an_archived_key_leaves_the_history_unchanged() {
        let (_dir, mut conn) = db("read-reinsert-archived");
        let file = insert_log_file(&conn, &new_log_file("C:/mtgo.log")).expect("file");
        write(
            &mut conn,
            WriteBatch {
                events: match_events(file),
                matches: vec![MatchWrite::Upsert {
                    view: Box::new(view(TOKEN, "Qopponent", 1_000)),
                    revision: 1,
                }],
                ..WriteBatch::default()
            },
        );
        write(
            &mut conn,
            WriteBatch {
                compact: vec![TOKEN.to_string()],
                now_ms: 9,
                ..WriteBatch::default()
            },
        );
        let archived = events_for_match(&conn, TOKEN).expect("history");
        assert_eq!(offsets(&archived), vec![0, 10, 20]);

        // The tracker replays the first event (a restart behind the cursor).
        write(
            &mut conn,
            WriteBatch {
                events: match_events(file).into_iter().take(1).collect(),
                ..WriteBatch::default()
            },
        );
        assert_eq!(events_for_match(&conn, TOKEN).expect("history"), archived);
    }

    #[test]
    fn a_quarantined_row_does_not_make_a_match_compactable() {
        let (_dir, mut conn) = db("read-compactable-quarantined");
        let file = insert_log_file(&conn, &new_log_file("C:/mtgo.log")).expect("file");
        // Quarantined rows carrying the match's token, Match ID and Game ID.
        let mut by_token = token_event(file, 0, TOKEN);
        by_token.quarantined = true;
        let mut by_match_id = event_with(
            file,
            10,
            EventKind::GameStateUpdate,
            subjects(None, Some(99), None),
            "state",
        );
        by_match_id.quarantined = true;
        let mut by_game_id = event_with(
            file,
            20,
            EventKind::DeckUsed,
            subjects(None, None, Some(7)),
            "deck",
        );
        by_game_id.quarantined = true;
        write(
            &mut conn,
            WriteBatch {
                events: vec![by_token, by_match_id, by_game_id],
                matches: vec![MatchWrite::Upsert {
                    view: Box::new(view(TOKEN, "Qopponent", 1_000)),
                    revision: 1,
                }],
                ..WriteBatch::default()
            },
        );
        assert!(
            compactable_matches(&conn, 10_000)
                .expect("compactable")
                .is_empty()
        );

        // The same match with one healthy row is compactable.
        write(
            &mut conn,
            WriteBatch {
                events: vec![token_event(file, 30, TOKEN)],
                ..WriteBatch::default()
            },
        );
        assert_eq!(
            compactable_matches(&conn, 10_000).expect("compactable"),
            vec![TOKEN.to_string()]
        );
    }

    #[test]
    fn player_names_are_accounts_and_opponents() {
        let (_dir, mut conn) = db("read-player-names");
        write(
            &mut conn,
            WriteBatch {
                matches: vec![
                    MatchWrite::Upsert {
                        view: Box::new(view(TOKEN, "Qopponent", 1_000)),
                        revision: 1,
                    },
                    MatchWrite::Upsert {
                        view: Box::new(view(OTHER, "Qother", 2_000)),
                        revision: 1,
                    },
                ],
                ..WriteBatch::default()
            },
        );
        assert_eq!(
            player_names(&conn).expect("names"),
            vec![
                "Qlocal".to_string(),
                "Qopponent".to_string(),
                "Qother".to_string()
            ]
        );
    }

    #[test]
    fn account_logins_list_every_account_with_its_login_id() {
        let (_dir, conn) = db("read-account-logins");
        conn.execute_batch(
            "INSERT INTO accounts (username, login_id, first_seen_at) VALUES ('Qkeyed', 6031972, 1);
             INSERT INTO accounts (username, login_id, first_seen_at) VALUES ('Qunkeyed', NULL, 2);",
        )
        .expect("accounts");
        assert_eq!(
            account_logins(&conn).expect("accounts"),
            vec![
                ("Qkeyed".to_string(), Some(6_031_972)),
                ("Qunkeyed".to_string(), None)
            ]
        );
    }

    #[test]
    fn joined_matches_without_a_row_are_listed() {
        const WAITING: &str = "aaaaaaaa-0000-0000-0000-000000000005";
        let (_dir, mut conn) = db("read-joined-without-row");
        let file = insert_log_file(&conn, &new_log_file("C:/mtgo.log")).expect("file");
        let join = |offset: u64, token: &str, match_id: u64| {
            event_with(
                file,
                offset,
                EventKind::GameManagementJson,
                subjects(Some(token), Some(match_id), Some(match_id + 100)),
                JOIN_HEADER,
            )
        };
        write(
            &mut conn,
            WriteBatch {
                events: vec![
                    join(0, TOKEN, 99),
                    join(10, WAITING, 98),
                    // Lobby noise: another player's match, never joined.
                    token_event(file, 20, OTHER),
                ],
                matches: vec![MatchWrite::Upsert {
                    view: Box::new(view(TOKEN, "Qopponent", 1_000)),
                    revision: 1,
                }],
                ..WriteBatch::default()
            },
        );
        assert_eq!(
            joined_tokens_without_match(&conn).expect("tokens"),
            vec![WAITING.to_string()]
        );
    }

    #[test]
    fn joined_tokens_without_match_walk_the_partial_join_index() {
        let (_dir, conn) = db("read-joined-without-row-plan");
        let mut stmt = conn
            .prepare(&format!("EXPLAIN QUERY PLAN {JOINED_TOKENS_WITHOUT_MATCH}"))
            .expect("explain");
        let details: Vec<String> = stmt
            .query_map([], |row| row.get::<_, String>(3))
            .expect("rows")
            .map(|row| row.expect("row"))
            .collect();
        let plan = details.join("\n");
        assert!(
            plan.contains("USING INDEX log_events_joins"),
            "plan:\n{plan}"
        );
        assert!(!plan.contains("SCAN e"), "plan:\n{plan}");
    }

    #[test]
    fn failed_matches_are_listed_and_failures_from_an_older_projection_found() {
        let (_dir, mut conn) = db("read-failures");
        let failed = |token: &str| MatchWrite::Attention {
            token: token.to_string(),
            reason: "projection panicked: Q boom".to_string(),
        };
        write(
            &mut conn,
            WriteBatch {
                matches: vec![failed(TOKEN)],
                ..WriteBatch::default()
            },
        );
        conn.execute("UPDATE match_failures SET derived_version = 0", [])
            .expect("an older projection failed it");
        write(
            &mut conn,
            WriteBatch {
                matches: vec![failed(OTHER)],
                ..WriteBatch::default()
            },
        );
        assert_eq!(
            failed_match_tokens(&conn).expect("failed"),
            vec![OTHER.to_string(), TOKEN.to_string()]
        );
        assert_eq!(
            stale_failures(&conn, PROJECTION_VERSION).expect("stale"),
            vec![TOKEN.to_string()]
        );
    }
}
