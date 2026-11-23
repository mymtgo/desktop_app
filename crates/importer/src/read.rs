//! Reading the 0.x database into `ImportedMatch` values (importer spec sections
//! 3 and 4). The database is opened read-only and read in one transaction, so
//! 0.x can keep running and writing while v1 reads a consistent snapshot.
//!
//! 0.x data is user data written by older code, so every value is read loosely:
//! a number stored as text still counts, and anything odd becomes `None` or a
//! skipped match, never an error.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;
use std::path::Path;
use std::time::{Duration, Instant};

use rusqlite::config::DbConfig;
use rusqlite::{Connection, OpenFlags};
use store::import::{ImportedAccount, ImportedGame, ImportedMatch, ImportedReplayShare};
use store::manual::ManualGameDetail;

use crate::account_read::{AccountFallback, read_account_fallback, read_accounts};
use crate::deck_read::{DeckRead, ImportedVersions, read_decks};
use crate::league_read::{LeagueRead, read_leagues};
use crate::limited_read::{LimitedRead, LimitedVersions, read_limited};
use crate::manual_read::{RegisteredMains, game_detail, registered_mains};
use crate::parity_rules::{FINISHED_STATES, OUTCOMES, known_code};
use crate::sql::{
    as_bool, as_integer, as_text, column_list, has_column, parse_id, parse_utc_ms, select_list,
    table_columns, value,
};
use store::import_leagues::LeagueKey;

/// Shown when a required column is missing (spec section 3).
const TOO_OLD: &str =
    "This 0.x database is too old to import. Update MyMTGO 0.x, start it once, then try again.";

/// Shown when the file has no `matches` or `players` table at all (a v1
/// database, an empty file or some other SQLite file).
const NOT_LEGACY: &str = "This file is not a MyMTGO 0.x database.";

/// Tables every 0.x database has; without them the file is not a 0.x database.
const LEGACY_TABLES: [&str; 2] = ["matches", "players"];

/// Columns v1 cannot import without (spec section 3).
const REQUIRED: [(&str, &[&str]); 4] = [
    ("matches", &MATCH_COLUMNS),
    ("games", &GAME_COLUMNS),
    (
        "game_player",
        &["game_id", "player_id", "is_local", "on_play", "dice_roll"],
    ),
    ("players", &["id", "username"]),
];

/// `matches` columns every read selects, in the order `read_matches` reads
/// them (before the optional ones).
const MATCH_COLUMNS: [&str; 10] = [
    "id",
    "token",
    "mtgo_id",
    "format",
    "match_type",
    "games_won",
    "games_lost",
    "started_at",
    "ended_at",
    "state",
];

/// `games` columns, in the order `read_games` reads them.
const GAME_COLUMNS: [&str; 7] = [
    "id",
    "match_id",
    "mtgo_id",
    "won",
    "started_at",
    "ended_at",
    "turn_count",
];

/// `game_player` columns, in the order `read_seats` reads them.
const SEAT_COLUMNS: [&str; 4] = ["game_id", "is_local", "on_play", "dice_roll"];

/// The `game_player` column that points at `players.id`.
const SEAT_PLAYER_KEY: &str = "player_id";

/// `players` columns, in the order `read_seats` reads them (after the seat's).
const PLAYER_COLUMNS: [&str; 1] = ["username"];

/// Every `(table, columns)` this reader selects or joins on. The SQL below is
/// built from the same lists, so the coverage ledger cannot drift from it.
pub const READ_COLUMNS: [(&str, &[&str]); 8] = [
    ("matches", &MATCH_COLUMNS),
    ("matches", &OPTIONAL_MATCH_COLUMNS),
    ("games", &[LEGACY_GAME_LINK]),
    ("games", &GAME_COLUMNS),
    ("game_player", &SEAT_COLUMNS),
    ("game_player", &[SEAT_PLAYER_KEY]),
    ("game_player", &OPTIONAL_SEAT_COLUMNS),
    ("players", &["id", "username"]),
];

/// `matches` columns read when present, `NULL` otherwise. Without `manual`
/// (an older 0.x) no match was entered by hand; without `imported` (a 0.x
/// from before its match history import, which added the column) every
/// match was tracked from the log (issue #313).
const OPTIONAL_MATCH_COLUMNS: [&str; 10] = [
    "outcome",
    "notes",
    "tournament_event_id",
    "tournament_round",
    "manual",
    "deck_version_id",
    "replay_share_uuid",
    "replay_share_url",
    "replay_shared_at",
    "imported",
];

/// The `games` column of a pre-release per-game replay link (replays spec
/// section 10): counted when present, never imported.
const LEGACY_GAME_LINK: &str = "replay_share_uuid";

/// `game_player` columns read when present, `NULL` (nothing recorded)
/// otherwise: the mulligans (an older 0.x has no such column) and what the
/// player recorded on a manual match's games.
const OPTIONAL_SEAT_COLUMNS: [&str; 3] = ["mulligan_count", "opening_hand_json", "deck_json"];

/// The name 0.x gave the local player of a manual match when MTGO gave none.
const MANUAL_PLACEHOLDER: &str = store::import::LEGACY_LOCAL_PLACEHOLDER;

/// How many 0.x matches were left out, by reason (spec section 4).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Skipped {
    /// `started`, `in_progress` (or any state v1 does not know).
    pub in_progress: u32,
    /// No game has a local seat, the match's deck has no account, and 0.x
    /// does not have exactly one account (Plan O fallback order).
    pub no_local_player: u32,
    /// A later 0.x row with a token already read in this run, or with an MTGO
    /// match ID already read in this run (0.x can hold a log capture and a
    /// history import of the same match under different tokens).
    pub duplicate_token: u32,
    /// The local player is not one of 0.x's accounts (issue #337): 0.x
    /// recorded the match while another MTGO user played it. Counted only
    /// when 0.x has an account it shows (one not deleted).
    pub not_account: u32,
    /// `started_at` does not parse as a time.
    pub bad_time: u32,
    /// The token is empty or blank (0.x declares it NOT NULL, but an empty
    /// string fits).
    pub blank_token: u32,
    /// Games (not matches) left out of an imported match because an earlier
    /// game of the same match has the same Game ID (v1 stores one game per
    /// Game ID and match).
    pub duplicate_games: u32,
    /// Matches (not left out) whose 0.x shared replay link misses a part
    /// (a uuid without a URL, or no readable time): the link is not
    /// imported (replays spec section 10).
    pub replay_shares_incomplete: u32,
    /// Matches left out (any skip above) that hold a complete shared replay
    /// link: the link stays on mymtgo.com with no v1 match to switch it off
    /// from.
    pub replay_shares_unplaced: u32,
    /// Pre-release per-game replay links (`games.replay_share_uuid`), never
    /// imported: v1 shares per match. Switch them off on mymtgo.com.
    pub legacy_game_links: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ReadResult {
    pub matches: Vec<ImportedMatch>,
    pub skipped: Skipped,
    /// Cards, decks, versions and match links; `None` when the database has
    /// no deck tables (an old 0.x), so only matches are imported.
    pub decks: Option<DeckRead>,
    /// Leagues (constructed, draft and sealed) and their membership; `None`
    /// when the database has no `leagues` table (an old 0.x).
    pub leagues: Option<LeagueRead>,
    /// Drafts, picks, notes and limited registered decks (Plan N); `None`
    /// when the database has no Limited tables (an old 0.x).
    pub limited: Option<LimitedRead>,
    /// The 0.x `matches.id` of each read match, by token: where
    /// `DetailReader` finds its games.
    pub match_ids: HashMap<String, i64>,
    /// Every 0.x account not deleted, in id order (Plan O); empty without
    /// an `accounts` table.
    pub accounts: Vec<ImportedAccount>,
    /// The matches left out because their local player is not one of
    /// 0.x's accounts, for the data tied only to them (issue #337).
    pub not_account: NotAccountMatches,
}

/// The 0.x matches left out because their local player is not one of 0.x's
/// accounts (issue #337), so the data tied only to them is left out too:
/// their leagues, drafts, registered decks and tournament names.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NotAccountMatches {
    /// Their 0.x tokens.
    pub tokens: HashSet<String>,
    /// The MTGO event ids of tournaments whose every 0.x match is one of
    /// them.
    pub tournament_event_ids: HashSet<i64>,
}

impl NotAccountMatches {
    /// Whether 0.x's tournament `event_id` belongs only to left out
    /// players, so its name is not imported.
    pub fn holds_tournament(&self, event_id: i64) -> bool {
        self.tournament_event_ids.contains(&event_id)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportError {
    /// The `matches` or `players` table is missing: not a 0.x database.
    NotLegacy,
    /// A required column is missing; holds `table.column`.
    TooOld(String),
    /// The file could not be opened.
    Open(String),
    /// Reading failed (or settings.json is not a JSON object).
    Read(String),
}

impl fmt::Display for ImportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ImportError::NotLegacy => write!(f, "{NOT_LEGACY}"),
            ImportError::TooOld(_) => write!(f, "{TOO_OLD}"),
            ImportError::Open(what) => write!(f, "the 0.x database could not be opened: {what}"),
            ImportError::Read(what) => write!(f, "the 0.x data could not be read: {what}"),
        }
    }
}

impl std::error::Error for ImportError {}

pub(crate) fn read_error(error: rusqlite::Error) -> ImportError {
    ImportError::Read(error.to_string())
}

/// Read every importable match of the 0.x database at `path`.
pub fn read_database(path: &Path) -> Result<ReadResult, ImportError> {
    let mut conn = open(path)?;
    let tx = conn.transaction().map_err(read_error)?;
    check_required(&tx)?;
    let matches = read_matches(&tx)?;
    let legacy_game_links = count_legacy_game_links(&tx)?;
    let games = read_games(&tx)?;
    let seats = read_seats(&tx)?;
    let accounts = read_accounts(&tx)?;
    let fallback = read_account_fallback(&tx, &accounts)?;
    let (decks, versions, limited_versions) = match read_decks(&tx)? {
        Some((decks, versions, limited_versions)) => (Some(decks), versions, limited_versions),
        None => (None, HashMap::new(), LimitedVersions::new()),
    };
    let leagues = read_leagues(&tx, &versions)?;
    let limited = read_limited(&tx, &limited_versions)?;
    tx.commit().map_err(read_error)?;
    let registered = registered_mains(&versions);
    let known = account_names(&accounts);
    let mut result = map_all(
        matches,
        &games,
        &seats,
        &registered,
        &versions,
        &fallback,
        known.as_ref(),
    );
    result.skipped.legacy_game_links = legacy_game_links;
    let mut leagues = leagues;
    let mut limited = limited;
    // The leagues, drafts and registered decks of left out players go too
    // (issue #337), before the Limited import's league keys are taken.
    let dropped = match &mut leagues {
        Some(read) => drop_not_account_leagues(read, &result.not_account.tokens),
        None => HashSet::new(),
    };
    if let Some(limited) = &mut limited {
        drop_not_account_limited(limited, &dropped, &result.not_account.tokens);
        // The Limited import finds a 0.x league's run by its natural key: a
        // second 0.x database reuses the league ids (issue #143).
        let keys = match &leagues {
            Some(read) => read.leagues.iter().map(LeagueKey::of).collect(),
            None => Vec::new(),
        };
        limited.data.league_keys = Some(keys);
    }
    result.decks = decks;
    result.leagues = leagues;
    result.limited = limited;
    result.accounts = accounts;
    Ok(result)
}

/// The names a 0.x match's local player must have to be imported (issue
/// #337): 0.x's accounts not deleted. `None` when 0.x shows no account (no
/// `accounts` table, an empty one, or only deleted rows): 0.x then scopes
/// nothing by account (`Account::currentId()` is null), so every match is
/// imported as before.
fn account_names(accounts: &[ImportedAccount]) -> Option<HashSet<String>> {
    if accounts.is_empty() {
        return None;
    }
    let mut names = HashSet::new();
    for account in accounts {
        names.insert(account.username.clone());
    }
    Some(names)
}

/// Leave out each league that has 0.x matches, all of them left out for a
/// player that is not one of 0.x's accounts; returns their 0.x ids. A
/// league with no match, or with one match of a real account, stays.
fn drop_not_account_leagues(read: &mut LeagueRead, tokens: &HashSet<String>) -> HashSet<i64> {
    let mut dropped = HashSet::new();
    if tokens.is_empty() {
        return dropped;
    }
    let mut kept = Vec::new();
    for league in std::mem::take(&mut read.leagues) {
        let foreign = !league.matches.is_empty()
            && league
                .matches
                .iter()
                .all(|member| tokens.contains(&member.token));
        if foreign {
            dropped.insert(league.legacy_id);
            read.skipped_not_account += 1;
        } else {
            kept.push(league);
        }
    }
    read.leagues = kept;
    dropped
}

/// Leave out the drafts of the dropped leagues (counted) and the registered
/// decks of left out matches or dropped leagues (their matches are counted).
fn drop_not_account_limited(
    read: &mut LimitedRead,
    dropped: &HashSet<i64>,
    tokens: &HashSet<String>,
) {
    let before = read.data.drafts.len();
    read.data
        .drafts
        .retain(|draft| !draft.league_id.is_some_and(|id| dropped.contains(&id)));
    let left_out = before - read.data.drafts.len();
    read.drafts_not_account += u32::try_from(left_out).unwrap_or(u32::MAX);
    read.data
        .decks
        .retain(|deck| !dropped.contains(&deck.league_id) && !tokens.contains(&deck.match_token));
}

/// How long one read of the 0.x database may run before SQLite interrupts
/// it. Real reads take seconds; a crafted view or trigger could loop forever.
pub(crate) const READ_TIME_LIMIT: Duration = Duration::from_secs(120);

/// How many SQLite virtual machine steps run between time limit checks.
const PROGRESS_STEPS: i32 = 10_000;

/// Open the 0.x database read-only and hardened (see [`harden`]), with
/// [`READ_TIME_LIMIT`] counted from now.
pub(crate) fn open(path: &Path) -> Result<Connection, ImportError> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|error| ImportError::Open(error.to_string()))?;
    conn.busy_timeout(Duration::from_secs(5))
        .map_err(|error| ImportError::Open(error.to_string()))?;
    harden(&conn)?;
    Ok(conn)
}

/// Treat the 0.x database as untrusted input: its schema's views and
/// triggers may not call functions with side effects (`trusted_schema`
/// off), the defensive flag refuses writes that could corrupt the file, and
/// every read stops after [`READ_TIME_LIMIT`].
pub(crate) fn harden(conn: &Connection) -> Result<(), ImportError> {
    let open_error = |error: rusqlite::Error| ImportError::Open(error.to_string());
    conn.pragma_update(None, "trusted_schema", false)
        .map_err(open_error)?;
    conn.set_db_config(DbConfig::SQLITE_DBCONFIG_DEFENSIVE, true)
        .map_err(open_error)?;
    set_time_limit(conn, READ_TIME_LIMIT).map_err(open_error)
}

/// Interrupt any statement still running `limit` from now. Long-lived
/// readers call it again before each read, so the limit is per read.
pub(crate) fn set_time_limit(conn: &Connection, limit: Duration) -> rusqlite::Result<()> {
    let started = Instant::now();
    conn.progress_handler(PROGRESS_STEPS, Some(move || started.elapsed() > limit))
}

fn check_required(conn: &Connection) -> Result<(), ImportError> {
    for table in LEGACY_TABLES {
        if table_columns(conn, table)?.is_empty() {
            return Err(ImportError::NotLegacy);
        }
    }
    for (table, required) in REQUIRED {
        let present = table_columns(conn, table)?;
        for column in required {
            if !has_column(&present, column) {
                return Err(ImportError::TooOld(format!("{table}.{column}")));
            }
        }
    }
    Ok(())
}

/// One `matches` row, loosely typed.
struct MatchRow {
    id: i64,
    token: String,
    mtgo_id: Option<String>,
    format: Option<String>,
    match_type: Option<String>,
    games_won: i64,
    games_lost: i64,
    started_at: Option<String>,
    ended_at: Option<String>,
    state: Option<String>,
    outcome: Option<String>,
    notes: Option<String>,
    tournament_event_id: Option<i64>,
    tournament_round: Option<i64>,
    /// Entered by hand in 0.x (`matches.manual`).
    manual: bool,
    deck_version_id: Option<i64>,
    replay_share_uuid: Option<String>,
    replay_share_url: Option<String>,
    replay_shared_at: Option<String>,
    /// 0.x imported it from MTGO's match history (`matches.imported`).
    imported: Option<bool>,
}

/// One `games` row.
struct GameRow {
    mtgo_id: Option<String>,
    won: Option<i64>,
    started_at: Option<String>,
    ended_at: Option<String>,
    turn_count: Option<i64>,
}

/// One `game_player` row with its player's name.
struct Seat {
    is_local: bool,
    on_play: Option<bool>,
    dice_roll: Option<i64>,
    mulligan_count: Option<i64>,
    username: String,
    opening_hand_json: Option<String>,
    deck_json: Option<String>,
}

/// Every match, ordered by id (the first of duplicate tokens or match IDs wins).
fn read_matches(conn: &Connection) -> Result<Vec<MatchRow>, ImportError> {
    let present = table_columns(conn, "matches")?;
    let sql = format!(
        "SELECT {}, {} FROM matches ORDER BY id",
        column_list("", &MATCH_COLUMNS),
        select_list(&present, &OPTIONAL_MATCH_COLUMNS)
    );
    let mut stmt = conn.prepare(&sql).map_err(read_error)?;
    let rows = stmt
        .query_map([], |row| {
            Ok(MatchRow {
                id: row.get(0)?,
                token: as_text(value(row, 1)?).unwrap_or_default(),
                mtgo_id: as_text(value(row, 2)?),
                format: as_text(value(row, 3)?),
                match_type: as_text(value(row, 4)?),
                games_won: as_integer(value(row, 5)?).unwrap_or(0),
                games_lost: as_integer(value(row, 6)?).unwrap_or(0),
                started_at: as_text(value(row, 7)?),
                ended_at: as_text(value(row, 8)?),
                state: as_text(value(row, 9)?),
                outcome: as_text(value(row, 10)?),
                notes: as_text(value(row, 11)?),
                tournament_event_id: as_integer(value(row, 12)?),
                tournament_round: as_integer(value(row, 13)?),
                manual: as_integer(value(row, 14)?).is_some_and(|flag| flag != 0),
                deck_version_id: as_integer(value(row, 15)?),
                replay_share_uuid: as_text(value(row, 16)?),
                replay_share_url: as_text(value(row, 17)?),
                replay_shared_at: as_text(value(row, 18)?),
                imported: as_bool(value(row, 19)?),
            })
        })
        .map_err(read_error)?;
    let mut matches = Vec::new();
    for row in rows {
        matches.push(row.map_err(read_error)?);
    }
    Ok(matches)
}

/// How many games hold a pre-release per-game replay link; none when the
/// column does not exist (every released 0.x).
fn count_legacy_game_links(conn: &Connection) -> Result<u32, ImportError> {
    let present = table_columns(conn, "games")?;
    if !has_column(&present, LEGACY_GAME_LINK) {
        return Ok(0);
    }
    let count: i64 = conn
        .query_row(
            &format!(
                "SELECT COUNT(*) FROM games WHERE \"{LEGACY_GAME_LINK}\" IS NOT NULL
                     AND TRIM(\"{LEGACY_GAME_LINK}\") <> ''"
            ),
            [],
            |row| row.get(0),
        )
        .map_err(read_error)?;
    Ok(u32::try_from(count).unwrap_or(u32::MAX))
}

/// Games grouped by match id, each group in game order (`started_at`, then id).
/// Also returns each game's id alongside, to find its seats.
fn read_games(conn: &Connection) -> Result<HashMap<i64, Vec<(i64, GameRow)>>, ImportError> {
    let sql = format!(
        "SELECT {} FROM games ORDER BY match_id, started_at, id",
        column_list("", &GAME_COLUMNS)
    );
    let mut stmt = conn.prepare(&sql).map_err(read_error)?;
    let rows = stmt
        .query_map([], |row| {
            let game_id: i64 = row.get(0)?;
            let match_id = as_integer(value(row, 1)?);
            let game = GameRow {
                mtgo_id: as_text(value(row, 2)?),
                won: as_integer(value(row, 3)?),
                started_at: as_text(value(row, 4)?),
                ended_at: as_text(value(row, 5)?),
                turn_count: as_integer(value(row, 6)?),
            };
            Ok((game_id, match_id, game))
        })
        .map_err(read_error)?;
    let mut games: HashMap<i64, Vec<(i64, GameRow)>> = HashMap::new();
    for row in rows {
        let (game_id, match_id, game) = row.map_err(read_error)?;
        if let Some(match_id) = match_id {
            games.entry(match_id).or_default().push((game_id, game));
        }
    }
    Ok(games)
}

/// Seats grouped by game id, in `game_player` row order. Rows whose player is
/// missing or has no name are left out.
fn read_seats(conn: &Connection) -> Result<HashMap<i64, Vec<Seat>>, ImportError> {
    let present = table_columns(conn, "game_player")?;
    let sql = format!(
        "SELECT {}, {}, {}
         FROM game_player gp JOIN players p ON p.id = gp.{SEAT_PLAYER_KEY}
         ORDER BY gp.game_id, gp.rowid",
        column_list("gp.", &SEAT_COLUMNS),
        column_list("p.", &PLAYER_COLUMNS),
        select_list(&present, &OPTIONAL_SEAT_COLUMNS)
    );
    let mut stmt = conn.prepare(&sql).map_err(read_error)?;
    let rows = stmt
        .query_map([], |row| {
            let game_id = as_integer(value(row, 0)?);
            let seat = Seat {
                is_local: as_bool(value(row, 1)?).unwrap_or(false),
                on_play: as_bool(value(row, 2)?),
                dice_roll: as_integer(value(row, 3)?),
                username: as_text(value(row, 4)?).unwrap_or_default(),
                mulligan_count: as_integer(value(row, 5)?),
                opening_hand_json: as_text(value(row, 6)?),
                deck_json: as_text(value(row, 7)?),
            };
            Ok((game_id, seat))
        })
        .map_err(read_error)?;
    let mut seats: HashMap<i64, Vec<Seat>> = HashMap::new();
    for row in rows {
        let (game_id, seat) = row.map_err(read_error)?;
        if let Some(game_id) = game_id
            && !seat.username.trim().is_empty()
        {
            seats.entry(game_id).or_default().push(seat);
        }
    }
    Ok(seats)
}

/// Why a match is left out.
enum Skip {
    InProgress,
    NoLocalPlayer,
    NotAccount,
    BadTime,
}

fn map_all(
    rows: Vec<MatchRow>,
    games: &HashMap<i64, Vec<(i64, GameRow)>>,
    seats: &HashMap<i64, Vec<Seat>>,
    registered: &RegisteredMains,
    versions: &ImportedVersions,
    fallback: &AccountFallback,
    known: Option<&HashSet<String>>,
) -> ReadResult {
    let mut matches: Vec<ImportedMatch> = Vec::new();
    let mut not_account = NotAccountMatches::default();
    // Tournaments of left out players, and of every other match.
    let mut not_account_events: HashSet<i64> = HashSet::new();
    let mut other_events: HashSet<i64> = HashSet::new();
    let mut skipped = Skipped::default();
    let mut tokens: HashSet<String> = HashSet::new();
    let mut match_ids: HashSet<i64> = HashSet::new();
    let mut legacy_ids: HashMap<String, i64> = HashMap::new();
    let no_games: Vec<(i64, GameRow)> = Vec::new();
    for row in rows {
        let legacy_id = row.id;
        let match_games = games.get(&row.id).unwrap_or(&no_games);
        let linked = u32::from(matches!(replay_share(&row), ShareRead::Link(_)));
        let token = row.token.clone();
        let event_id = row.tournament_event_id;
        let mapped = map_match(
            row,
            match_games,
            seats,
            registered,
            versions,
            fallback,
            known,
        );
        let is_not_account = matches!(mapped, Err(Skip::NotAccount));
        if let Some(event_id) = event_id {
            if is_not_account {
                not_account_events.insert(event_id);
            } else {
                other_events.insert(event_id);
            }
        }
        match mapped {
            Err(skip) => {
                skipped.replay_shares_unplaced += linked;
                match skip {
                    Skip::InProgress => skipped.in_progress += 1,
                    Skip::NoLocalPlayer => skipped.no_local_player += 1,
                    Skip::NotAccount => {
                        skipped.not_account += 1;
                        not_account.tokens.insert(token);
                    }
                    Skip::BadTime => skipped.bad_time += 1,
                }
            }
            Ok(Mapped {
                imported,
                share_incomplete,
                duplicate_games,
            }) => {
                if imported.token.trim().is_empty() {
                    skipped.blank_token += 1;
                    skipped.replay_shares_unplaced += linked;
                    continue;
                }
                // Rows come in id order, so the first row wins. The
                // `duplicate_token` counter covers duplicate tokens and
                // duplicate MTGO match IDs.
                let seen_token = tokens.contains(&imported.token);
                let seen_match_id = match imported.mtgo_match_id {
                    Some(match_id) => match_ids.contains(&match_id),
                    None => false,
                };
                if seen_token || seen_match_id {
                    skipped.duplicate_token += 1;
                    skipped.replay_shares_unplaced += linked;
                    continue;
                }
                tokens.insert(imported.token.clone());
                if let Some(match_id) = imported.mtgo_match_id {
                    match_ids.insert(match_id);
                }
                skipped.duplicate_games += duplicate_games;
                if share_incomplete {
                    skipped.replay_shares_incomplete += 1;
                }
                legacy_ids.insert(imported.token.clone(), legacy_id);
                matches.push(imported);
            }
        }
    }
    for event_id in not_account_events {
        if !other_events.contains(&event_id) {
            not_account.tournament_event_ids.insert(event_id);
        }
    }
    ReadResult {
        matches,
        skipped,
        decks: None,
        leagues: None,
        limited: None,
        match_ids: legacy_ids,
        accounts: Vec::new(),
        not_account,
    }
}

/// One kept game of a match, from `numbered_unique_games`.
pub(crate) struct NumberedGame<T> {
    /// Its place among all the match's games in game order, from 1.
    pub number: u32,
    /// Its Game ID, when it has one.
    pub mtgo_game_id: Option<i64>,
    pub game: T,
}

/// A match's games (each with its Game ID, in game order) as v1 numbers and
/// keeps them: a game's number is its place among all the games, and a
/// game whose Game ID an earlier game has is left out (v1 stores one game
/// per Game ID and match; games without one are all kept), so the numbers
/// keep the gap (games 1 and 3). Returns the kept games and how many were
/// left out. The match read, the detail read and the card stats comparison
/// all number games here, so their games line up.
pub(crate) fn numbered_unique_games<T>(
    games: Vec<(Option<i64>, T)>,
) -> (Vec<NumberedGame<T>>, u32) {
    let mut seen: HashSet<i64> = HashSet::new();
    let mut kept = Vec::new();
    let mut dropped: u32 = 0;
    for (index, (mtgo_game_id, game)) in games.into_iter().enumerate() {
        if let Some(id) = mtgo_game_id
            && !seen.insert(id)
        {
            dropped = dropped.saturating_add(1);
            continue;
        }
        kept.push(NumberedGame {
            number: u32::try_from(index + 1).unwrap_or(u32::MAX),
            mtgo_game_id,
            game,
        });
    }
    (kept, dropped)
}

/// What a 0.x match row holds of a shared replay link.
enum ShareRead {
    None,
    Link(ImportedReplayShare),
    /// A part is missing or its time does not read (0.x never wrote one
    /// so): skipped and counted.
    Incomplete,
}

/// A match's 0.x shared replay link.
fn replay_share(row: &MatchRow) -> ShareRead {
    let present = |text: &Option<String>| -> Option<String> {
        text.as_deref()
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_string)
    };
    let uuid = present(&row.replay_share_uuid);
    let url = present(&row.replay_share_url);
    let at = present(&row.replay_shared_at);
    if uuid.is_none() && url.is_none() && at.is_none() {
        return ShareRead::None;
    }
    match (uuid, url, at.as_deref().and_then(parse_utc_ms)) {
        (Some(uuid), Some(url), Some(shared_at_ms)) => ShareRead::Link(ImportedReplayShare {
            uuid,
            url,
            shared_at_ms,
        }),
        _ => ShareRead::Incomplete,
    }
}

/// A match as v1 imports it.
struct Mapped {
    imported: ImportedMatch,
    /// Its shared replay link was left out for a missing part.
    share_incomplete: bool,
    /// Its games left out for a repeated Game ID.
    duplicate_games: u32,
}

/// The match as v1 imports it.
fn map_match(
    row: MatchRow,
    games: &[(i64, GameRow)],
    seats: &HashMap<i64, Vec<Seat>>,
    registered: &RegisteredMains,
    versions: &ImportedVersions,
    fallback: &AccountFallback,
    known: Option<&HashSet<String>>,
) -> Result<Mapped, Skip> {
    let (share, share_incomplete) = match replay_share(&row) {
        ShareRead::None => (None, false),
        ShareRead::Link(link) => (Some(link), false),
        ShareRead::Incomplete => (None, true),
    };
    let state = row.state.unwrap_or_default();
    if !FINISHED_STATES.contains(&state.as_str()) {
        return Err(Skip::InProgress);
    }
    let started_at_ms = row
        .started_at
        .as_deref()
        .and_then(parse_utc_ms)
        .ok_or(Skip::BadTime)?;
    // The local player, first hit wins (Plan O): the local seat of any
    // game, else the account of the match's deck, else 0.x's only account.
    let deck_account = || {
        row.deck_version_id
            .and_then(|id| fallback.by_version.get(&id).cloned())
    };
    let account = || deck_account().or_else(|| fallback.only.clone());
    let seat_name = local_name(games, seats);
    // 0.x named the local seat of a manual match "You" when MTGO gave no
    // username (`CreateManualMatch`): the account is the real player (#162),
    // the deck's, else the only account 0.x has not deleted.
    let seat_name = match seat_name {
        Some(name) if row.manual && name == MANUAL_PLACEHOLDER => deck_account()
            .or_else(|| fallback.only_active.clone())
            .or(Some(name)),
        other => other,
    };
    let mut local_username = seat_name.or_else(account).ok_or(Skip::NoLocalPlayer)?;
    // Only 0.x's own accounts are imported (issue #337). A seat that is not
    // one of them gives way to the account of the match's deck (0.x lists
    // a match under its deck's account, `forAccount`); with neither, the
    // match is left out. A manual match still on the "You" placeholder was
    // entered on this install, so it is the user's own (issue #162 keeps it
    // on the placeholder account).
    let placeholder = row.manual && local_username == MANUAL_PLACEHOLDER;
    if let Some(names) = known
        && !placeholder
        && !names.contains(&local_username)
    {
        local_username = deck_account().ok_or(Skip::NotAccount)?;
    }
    let opponent = opponent_name(games, seats, &local_username);
    // A match without game rows keeps 0.x's score: the deck stats fold it in.
    let (games_won_imported, games_lost_imported) = if games.is_empty() {
        (Some(row.games_won), Some(row.games_lost))
    } else {
        (None, None)
    };

    let no_seats: Vec<Seat> = Vec::new();

    let mut with_ids = Vec::new();
    for (game_id, game) in games {
        with_ids.push((game.mtgo_id.as_deref().and_then(parse_id), (game_id, game)));
    }
    let (numbered, duplicate_games) = numbered_unique_games(with_ids);
    let mut imported_games = Vec::new();
    for kept in numbered {
        let (game_id, game) = kept.game;
        let game_seats = seats.get(game_id).unwrap_or(&no_seats);
        imported_games.push(map_game(
            kept.number,
            kept.mtgo_game_id,
            game,
            game_seats,
            &local_username,
        ));
    }

    let manual = if row.manual {
        let registered = row.deck_version_id.and_then(|id| registered.get(&id));
        Some(manual_details(games, seats, registered, &local_username))
    } else {
        None
    };
    // A manual match has no MTGO match id (0.x gave it `manual_*`).
    let mtgo_match_id = if row.manual {
        None
    } else {
        row.mtgo_id.as_deref().and_then(parse_id)
    };
    // The deck import links the match when 0.x's version is imported.
    let deck_linked = row
        .deck_version_id
        .is_some_and(|id| versions.contains_key(&id));
    let outcome = match row.outcome {
        Some(outcome) if OUTCOMES.contains(&outcome.as_str()) => outcome,
        _ => outcome_from_score(row.games_won, row.games_lost),
    };
    let imported = ImportedMatch {
        token: row.token,
        mtgo_match_id,
        local_username,
        opponent,
        format: known_code(row.format),
        match_type: known_code(row.match_type),
        tournament_event_id: row.tournament_event_id,
        tournament_round: row.tournament_round,
        state,
        outcome,
        started_at_ms,
        ended_at_ms: row.ended_at.as_deref().and_then(parse_utc_ms),
        note: row.notes,
        games: imported_games,
        manual,
        games_won_imported,
        games_lost_imported,
        deck_linked,
        replay_share: share,
        legacy_history: row.imported.unwrap_or(false),
    };
    Ok(Mapped {
        imported,
        share_incomplete,
        duplicate_games,
    })
}

/// What the player recorded on each game of a 0.x manual match, numbered as
/// `numbered_unique_games` numbers them (a game's place among all the
/// match's games). Games with nothing recorded are left out.
fn manual_details(
    games: &[(i64, GameRow)],
    seats: &HashMap<i64, Vec<Seat>>,
    registered: Option<&BTreeMap<u32, u32>>,
    local_username: &str,
) -> Vec<ManualGameDetail> {
    let no_seats: Vec<Seat> = Vec::new();
    let mut details = Vec::new();
    for (index, (game_id, _)) in games.iter().enumerate() {
        let game_seats = seats.get(game_id).unwrap_or(&no_seats);
        let local = local_seat_of(game_seats, local_username);
        let opponent = opponent_seat_of(game_seats, local_username);
        let detail = game_detail(
            u32::try_from(index + 1).unwrap_or(u32::MAX),
            local.and_then(|seat| seat.opening_hand_json.as_deref()),
            local.and_then(|seat| seat.deck_json.as_deref()),
            opponent.and_then(|seat| seat.deck_json.as_deref()),
            registered,
        );
        if let Some(detail) = detail {
            details.push(detail);
        }
    }
    details
}

fn map_game(
    game_number: u32,
    mtgo_game_id: Option<i64>,
    game: &GameRow,
    seats: &[Seat],
    local_username: &str,
) -> ImportedGame {
    let local = local_seat_of(seats, local_username);
    let opponent = opponent_seat_of(seats, local_username);
    let result = match game.won {
        Some(0) => "loss",
        Some(_) => "win",
        None => "unknown",
    };
    ImportedGame {
        mtgo_game_id,
        game_number,
        result: result.to_string(),
        on_play: local.and_then(|seat| seat.on_play),
        local_mulligans: local.and_then(|seat| seat.mulligan_count),
        opp_mulligans: opponent.and_then(|seat| seat.mulligan_count),
        local_die_roll: local.and_then(|seat| seat.dice_roll),
        opp_die_roll: opponent.and_then(|seat| seat.dice_roll),
        turn_count: game.turn_count,
        started_at_ms: game.started_at.as_deref().and_then(parse_utc_ms),
        ended_at_ms: game.ended_at.as_deref().and_then(parse_utc_ms),
    }
}

/// The name on the local seat of the first game that has one.
fn local_name(games: &[(i64, GameRow)], seats: &HashMap<i64, Vec<Seat>>) -> Option<String> {
    for (game_id, _) in games {
        if let Some(seat) = seats.get(game_id).and_then(|found| local_seat(found)) {
            return Some(seat.username.clone());
        }
    }
    None
}

/// The name on the first non-local seat, in game order, that is not the
/// local player's own (a fallback account can be on a seat 0.x did not mark
/// local).
fn opponent_name(
    games: &[(i64, GameRow)],
    seats: &HashMap<i64, Vec<Seat>>,
    local_username: &str,
) -> Option<String> {
    for (game_id, _) in games {
        let Some(game_seats) = seats.get(game_id) else {
            continue;
        };
        for seat in game_seats {
            if !seat.is_local && seat.username != local_username {
                return Some(seat.username.clone());
            }
        }
    }
    None
}

fn local_seat(seats: &[Seat]) -> Option<&Seat> {
    seats.iter().find(|seat| seat.is_local)
}

/// The local seat: the one marked local, else the one with the local
/// player's name (a fallback account can sit on a seat 0.x did not mark,
/// issue #190).
fn local_seat_of<'a>(seats: &'a [Seat], local_username: &str) -> Option<&'a Seat> {
    local_seat(seats).or_else(|| seats.iter().find(|seat| seat.username == local_username))
}

/// The first seat not marked local and not the local player's own.
fn opponent_seat_of<'a>(seats: &'a [Seat], local_username: &str) -> Option<&'a Seat> {
    seats
        .iter()
        .find(|seat| !seat.is_local && seat.username != local_username)
}

/// `win` if more games were won, `loss` if fewer, `unknown` otherwise.
fn outcome_from_score(won: i64, lost: i64) -> String {
    let outcome = if won > lost {
        "win"
    } else if won < lost {
        "loss"
    } else {
        "unknown"
    };
    outcome.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hardened_connection_distrusts_the_schema_and_is_defensive() {
        let conn = Connection::open_in_memory().expect("memory");
        harden(&conn).expect("harden");
        let trusted: i64 = conn
            .query_row("PRAGMA trusted_schema", [], |row| row.get(0))
            .expect("pragma");
        assert_eq!(trusted, 0);
        assert!(
            conn.db_config(DbConfig::SQLITE_DBCONFIG_DEFENSIVE)
                .expect("db config")
        );
    }

    #[test]
    fn a_repeated_game_id_is_left_out_and_the_numbers_keep_the_gap() {
        let games = vec![(Some(7), "a"), (None, "b"), (Some(7), "c"), (None, "d")];
        let (kept, dropped) = numbered_unique_games(games);
        let numbers: Vec<(u32, &str)> = kept.iter().map(|game| (game.number, game.game)).collect();
        assert_eq!(numbers, vec![(1, "a"), (2, "b"), (4, "d")]);
        assert_eq!(dropped, 1);
    }

    #[test]
    fn a_query_past_the_time_limit_is_interrupted() {
        let conn = Connection::open_in_memory().expect("memory");
        set_time_limit(&conn, Duration::from_millis(50)).expect("limit");
        let endless = "WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM n) \
                       SELECT count(*) FROM n";
        let result: Result<i64, _> = conn.query_row(endless, [], |row| row.get(0));
        assert!(result.is_err(), "the endless query was interrupted");

        set_time_limit(&conn, Duration::from_secs(60)).expect("limit");
        let one: i64 = conn
            .query_row("SELECT 1", [], |row| row.get(0))
            .expect("a fresh limit lets queries run");
        assert_eq!(one, 1);
    }
}
