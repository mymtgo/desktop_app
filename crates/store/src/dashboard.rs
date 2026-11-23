//! Home dashboard numbers (dashboard redesign spec,
//! `docs/superpowers/specs/2026-10-13-dashboard-redesign-design.md`): the
//! store reads behind the home dashboard's cards.
//!
//! Counting rules: only `complete` matches count. A match belongs to an
//! account by `matches.account_id` (spec section 5.3), so a limited match
//! and a match with no linked deck count; 0.x reached the account through
//! the deck. With no current account (`None`), the record, Recent matches
//! and the window reads take every match; the streak, deltas, play/draw
//! rates, last session and Top matchups are empty, as in 0.x.
//!
//! Every card read takes a `FormatFilter` (spec section 5.2).
//!
//! Every read is a fixed number of queries, whatever the history holds.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use decks::display_format;
use decks::stats::{
    DayRow, DeckLabel, GameRecord, PlayedMatch, Range, Record, TREND_HISTORY, daily_results,
    days_ago, winrate_percent,
};
use mtgo_match::Outcome;
use rusqlite::{Connection, OptionalExtension, params};

use crate::StoreError;
use crate::deck_pages::{DeckSummary, header_sql, summary_row, version_tallies};
use crate::names::stored_outcome;
use crate::platform::{Platform, account_of_platform};
use crate::read::{MATCH_ROW_SELECT, MatchRow, match_row};

/// Recent matches shows this many (dashboard redesign spec 4.5).
pub const RECENT_MATCHES_LIMIT: u32 = 5;

/// Recent sessions shows this many local days with a match.
pub const RECENT_SESSIONS: usize = 7;
/// Last session looks at this many latest matches (0.x `GetLastSession`).
pub const LAST_SESSION_LIMIT: u32 = 50;
/// A gap longer than this between two matches ends a session.
pub const SESSION_GAP_MS: i64 = 2 * 60 * 60 * 1000;
/// A match without an end is taken to have lasted this long.
const ASSUMED_MATCH_MS: i64 = 60 * 60 * 1000;
/// The last session's name for a match with no opponent archetype (0.x).
pub const UNKNOWN_ARCHETYPE: &str = "Unknown";
// ---- Shared SQL ----------------------------------------------------------------

/// Complete matches `m` started from `?2` to `?3`.
const COMPLETE_IN_WINDOW: &str = "m.state = 'complete' AND m.started_at BETWEEN ?2 AND ?3";

/// The account's matches on a query over `matches m`: every match of the
/// platform's accounts (`?{platform}`) when `?1` is NULL, else the matches
/// the account played (`matches.account_id`).
fn of_account(platform: u32) -> String {
    format!(
        "(?1 IS NULL OR m.account_id = ?1)
  AND {of_platform}",
        of_platform = account_of_platform("m.account_id", &format!("?{platform}")),
    )
}

/// The account `?1`'s complete matches, for reads without a window. There
/// is no index on `matches.account_id`: these scan `listed_matches`.
const ACCOUNT_COMPLETE: &str = "m.account_id = ?1 AND m.state = 'complete'";

/// A limited format code on `m.format` (`decks::is_limited_match_format`).
const LIMITED_FORMAT: &str = "(m.format GLOB 'D[A-Z]*' OR m.format GLOB 'S[0-9]*')";

/// Which matches a dashboard card counts (spec section 5.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormatFilter {
    /// Every match.
    All,
    /// Matches that are not limited, those without a format too.
    Constructed,
    /// Draft and sealed matches.
    Limited,
    /// One constructed format: every played code with its label (`CModern`
    /// and `CMODERN` are one format).
    Codes(Vec<String>),
}

impl FormatFilter {
    /// The mode parameter of `format_sql`.
    fn mode(&self) -> i64 {
        match self {
            FormatFilter::All => 0,
            FormatFilter::Constructed => 1,
            FormatFilter::Limited => 2,
            FormatFilter::Codes(_) => 3,
        }
    }

    /// The codes parameter of `format_sql`: a JSON array, empty unless
    /// `Codes`.
    fn codes_json(&self) -> Result<String, StoreError> {
        let codes: &[String] = match self {
            FormatFilter::Codes(codes) => codes,
            _ => &[],
        };
        Ok(serde_json::to_string(codes)?)
    }
}

/// The format condition on `matches m`: its mode bound to `?{mode}`, its
/// codes (a JSON array) to `?{codes}`.
fn format_sql(mode: u32, codes: u32) -> String {
    format!(
        "(?{mode} = 0
  OR (?{mode} = 1 AND (m.format IS NULL OR NOT {LIMITED_FORMAT}))
  OR (?{mode} = 2 AND {LIMITED_FORMAT})
  OR (?{mode} = 3 AND m.format IN (SELECT value FROM json_each(?{codes}))))"
    )
}

/// A constructed format of the format switch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormatOption {
    /// The lowercase label (`modern`): the switch's value and `?format=`.
    pub key: String,
    /// `Modern`.
    pub label: String,
    /// Every played code with this label, sorted (`CMODERN`, `CModern`).
    pub codes: Vec<String>,
}

/// The constructed format codes of the account's complete matches.
fn played_formats_sql() -> String {
    format!(
        "SELECT DISTINCT m.format FROM listed_matches m
WHERE m.state = 'complete' AND {of_account} AND m.format IS NOT NULL AND m.format <> ''
  AND NOT {LIMITED_FORMAT}",
        of_account = of_account(2)
    )
}

/// The constructed formats the account played (every account's without
/// one), one per display label, by label.
pub fn format_options(
    conn: &Connection,
    account: Option<i64>,
    platform: Platform,
) -> Result<Vec<FormatOption>, StoreError> {
    let mut stmt = conn.prepare_cached(&played_formats_sql())?;
    let rows = stmt.query_map(params![account, platform], |row| row.get::<_, String>(0))?;
    let mut options: Vec<FormatOption> = Vec::new();
    for row in rows {
        let code = row?;
        let label = display_format(&code);
        let key = label.to_lowercase();
        match options.iter_mut().find(|option| option.key == key) {
            Some(option) => option.codes.push(code),
            None => options.push(FormatOption {
                key,
                label,
                codes: vec![code],
            }),
        }
    }
    for option in &mut options {
        option.codes.sort();
    }
    options.sort_by(|a, b| a.label.cmp(&b.label));
    Ok(options)
}

/// The format `key` asks for and its filter: `all` (kept for callers that
/// ask for it; the page no longer offers it), `constructed`, `limited` or a
/// key of `options`. Anything else (an empty key too) is `None`: the caller
/// reads the last played format instead.
pub fn resolve_format(key: &str, options: &[FormatOption]) -> Option<(String, FormatFilter)> {
    match key {
        "all" => Some(("all".to_string(), FormatFilter::All)),
        "constructed" => Some(("constructed".to_string(), FormatFilter::Constructed)),
        "limited" => Some(("limited".to_string(), FormatFilter::Limited)),
        _ => options
            .iter()
            .find(|option| option.key == key)
            .map(|option| {
                (
                    option.key.clone(),
                    FormatFilter::Codes(option.codes.clone()),
                )
            }),
    }
}

/// The account's latest complete listed match: its format and whether it is
/// limited.
fn last_played_sql() -> String {
    format!(
        "SELECT m.format, COALESCE({LIMITED_FORMAT}, 0) FROM listed_matches m
WHERE m.state = 'complete' AND {of_account}
ORDER BY m.started_at DESC, m.id DESC LIMIT 1",
        of_account = of_account(2)
    )
}

/// The format key of the account's latest complete match (the format the
/// page opens on): `limited` for a limited match, the lowercase format
/// label (`modern`) for a constructed one, `constructed` for a constructed
/// match without a format or when there is no match at all.
pub fn last_played_format(
    conn: &Connection,
    account: Option<i64>,
    platform: Platform,
) -> Result<String, StoreError> {
    let mut stmt = conn.prepare_cached(&last_played_sql())?;
    let mut rows = stmt.query(params![account, platform])?;
    let Some(row) = rows.next()? else {
        return Ok("constructed".to_string());
    };
    let format: Option<String> = row.get(0)?;
    let limited: bool = row.get(1)?;
    if limited {
        return Ok("limited".to_string());
    }
    match format {
        Some(code) if !code.is_empty() => Ok(display_format(&code).to_lowercase()),
        _ => Ok("constructed".to_string()),
    }
}

// ---- Overview ------------------------------------------------------------------

/// The current and best streaks (0.x `GetStreak`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Streak {
    /// `3W` or `2L`: the run of equal results ending with the newest match
    /// in the window; `None` when that match is neither a win nor a loss, or
    /// the window has no matches.
    pub current: Option<String>,
    /// The longest runs of wins and of losses over every match.
    pub best_win: u32,
    pub best_loss: u32,
}

/// Game win rates on the play and on the draw (0.x `GetPlayDrawSplit`).
/// Games without a known result count as lost.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PlayDrawSplit {
    pub otp_win_rate: u32,
    pub otd_win_rate: u32,
}

/// The Overview widget (0.x `KpiStripWidget`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Overview {
    /// Complete matches in the window that have games.
    pub record: Record,
    /// Their games won and lost.
    pub games: GameRecord,
    pub streak: Streak,
    /// Match and game win rate minus the previous window's (0 for all time
    /// or without an account).
    pub match_win_rate_delta: i32,
    pub game_win_rate_delta: i32,
    pub play_draw: PlayDrawSplit,
}

/// The Overview over complete matches in `range`; `previous` is the window
/// before it (`decks::stats::previous_range`), `None` for all time.
pub fn overview(
    conn: &Connection,
    account: Option<i64>,
    platform: Platform,
    range: Range,
    previous: Option<Range>,
    format: &FormatFilter,
) -> Result<Overview, StoreError> {
    let current = window_games(conn, account, platform, range, format)?;
    let games = GameRecord::new(current.games_won, current.games_lost);
    let mut overview = Overview {
        record: Record::from_total(current.wins, current.losses, current.matches),
        games,
        streak: Streak::default(),
        match_win_rate_delta: 0,
        game_win_rate_delta: 0,
        play_draw: PlayDrawSplit::default(),
    };
    // Without an account 0.x has no streak, deltas or play/draw rates.
    let Some(account) = account else {
        return Ok(overview);
    };

    overview.streak = streak(conn, account, range, format)?;
    overview.play_draw = PlayDrawSplit {
        otp_win_rate: winrate_percent(
            current.otp_won,
            current.otp_total.saturating_sub(current.otp_won),
            0,
        ),
        otd_win_rate: winrate_percent(
            current.otd_won,
            current.otd_total.saturating_sub(current.otd_won),
            0,
        ),
    };
    // 0.x `GetWinrateDelta`: the match rate over every complete match (with
    // games or not), the game rate over their games. No delta for all time.
    if let Some(previous) = previous {
        let now_rate = window_record(conn, Some(account), platform, range, format)?.win_rate;
        let before_rate = window_record(conn, Some(account), platform, previous, format)?.win_rate;
        let before = window_games(conn, Some(account), platform, previous, format)?;
        let before_games = GameRecord::new(before.games_won, before.games_lost);
        overview.match_win_rate_delta = now_rate as i32 - before_rate as i32;
        overview.game_win_rate_delta = games.win_rate as i32 - before_games.win_rate as i32;
    }
    Ok(overview)
}

/// Complete matches with games in a window and their games.
struct WindowGames {
    matches: u32,
    wins: u32,
    losses: u32,
    games_won: u32,
    games_lost: u32,
    otp_won: u32,
    otp_total: u32,
    otd_won: u32,
    otd_total: u32,
}

/// Per window (`?2` to `?3`) and account (`?1`): complete matches that have
/// games (0.x inner-joins games), their wins and losses, then their games
/// won and lost, then on the play and on the draw.
fn window_games_sql() -> String {
    format!(
        "SELECT COUNT(DISTINCT m.id),
       COUNT(DISTINCT CASE WHEN m.outcome = 'win' THEN m.id END),
       COUNT(DISTINCT CASE WHEN m.outcome = 'loss' THEN m.id END),
       COALESCE(SUM(g.result = 'win'), 0), COALESCE(SUM(g.result = 'loss'), 0),
       COALESCE(SUM(g.on_play = 1 AND g.result = 'win'), 0), COALESCE(SUM(g.on_play = 1), 0),
       COALESCE(SUM(g.on_play = 0 AND g.result = 'win'), 0), COALESCE(SUM(g.on_play = 0), 0)
FROM listed_matches m
CROSS JOIN games g ON g.match_id = m.id
WHERE {COMPLETE_IN_WINDOW} AND {of_account} AND {by_format}",
        of_account = of_account(6),
        by_format = format_sql(4, 5)
    )
}

fn window_games(
    conn: &Connection,
    account: Option<i64>,
    platform: Platform,
    range: Range,
    format: &FormatFilter,
) -> Result<WindowGames, StoreError> {
    Ok(conn.prepare_cached(&window_games_sql())?.query_row(
        params![
            account,
            range.from_ms,
            range.to_ms,
            format.mode(),
            format.codes_json()?,
            platform
        ],
        |row| {
            Ok(WindowGames {
                matches: row.get(0)?,
                wins: row.get(1)?,
                losses: row.get(2)?,
                games_won: row.get(3)?,
                games_lost: row.get(4)?,
                otp_won: row.get(5)?,
                otp_total: row.get(6)?,
                otd_won: row.get(7)?,
                otd_total: row.get(8)?,
            })
        },
    )?)
}

/// Every complete match of the window, account and format, with games or
/// not, and its wins and losses (0.x `MatchRecord::fromQuery`).
fn window_record_sql() -> String {
    format!(
        "SELECT COUNT(*), COALESCE(SUM(m.outcome = 'win'), 0), COALESCE(SUM(m.outcome = 'loss'), 0)
FROM listed_matches m
WHERE {COMPLETE_IN_WINDOW} AND {of_account} AND {by_format}",
        of_account = of_account(6),
        by_format = format_sql(4, 5)
    )
}

fn window_record(
    conn: &Connection,
    account: Option<i64>,
    platform: Platform,
    range: Range,
    format: &FormatFilter,
) -> Result<Record, StoreError> {
    Ok(conn.prepare_cached(&window_record_sql())?.query_row(
        params![
            account,
            range.from_ms,
            range.to_ms,
            format.mode(),
            format.codes_json()?,
            platform
        ],
        |row| Ok(Record::from_total(row.get(1)?, row.get(2)?, row.get(0)?)),
    )?)
}

/// The outcome and start of every complete match of the account in the
/// format, oldest first.
fn account_outcomes_sql() -> String {
    format!(
        "SELECT m.outcome, m.started_at FROM listed_matches m
WHERE {ACCOUNT_COMPLETE} AND {by_format}
ORDER BY m.started_at, m.id",
        by_format = format_sql(2, 3)
    )
}

/// 0.x `GetStreak`: the current streak in the window, the best over every
/// match of the format.
fn streak(
    conn: &Connection,
    account: i64,
    range: Range,
    format: &FormatFilter,
) -> Result<Streak, StoreError> {
    let mut stmt = conn.prepare_cached(&account_outcomes_sql())?;
    let rows = stmt.query_map(
        params![account, format.mode(), format.codes_json()?],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
    )?;
    let mut every = Vec::new();
    for row in rows {
        every.push(row?);
    }
    let oldest_first: Vec<&str> = every.iter().map(|(outcome, _)| outcome.as_str()).collect();
    let mut newest_in_window: Vec<&str> = every
        .iter()
        .filter(|(_, started_at)| *started_at >= range.from_ms && *started_at <= range.to_ms)
        .map(|(outcome, _)| outcome.as_str())
        .collect();
    newest_in_window.reverse();
    let (best_win, best_loss) = best_streaks(&oldest_first);
    Ok(Streak {
        current: current_streak(&newest_in_window),
        best_win,
        best_loss,
    })
}

/// `3W` or `2L` for the run of equal outcomes at the front of
/// `newest_first`; `None` when it is empty or starts with neither a win nor
/// a loss.
fn current_streak(newest_first: &[&str]) -> Option<String> {
    let first = *newest_first.first()?;
    let label = match first {
        "win" => "W",
        "loss" => "L",
        _ => return None,
    };
    let count = newest_first
        .iter()
        .take_while(|outcome| **outcome == first)
        .count();
    Some(format!("{count}{label}"))
}

/// The longest runs of wins and of losses; anything else breaks both.
fn best_streaks(oldest_first: &[&str]) -> (u32, u32) {
    let (mut best_win, mut best_loss) = (0, 0);
    let (mut wins, mut losses) = (0, 0);
    for outcome in oldest_first {
        match *outcome {
            "win" => {
                wins += 1;
                losses = 0;
                best_win = best_win.max(wins);
            }
            "loss" => {
                losses += 1;
                wins = 0;
                best_loss = best_loss.max(losses);
            }
            _ => {
                wins = 0;
                losses = 0;
            }
        }
    }
    (best_win, best_loss)
}

/// One of the account's latest complete matches.
struct LatestMatch {
    token: String,
    outcome: String,
    started_at: i64,
    ended_at: Option<i64>,
    games_won: u32,
    games_lost: u32,
    opponent_archetype: Option<String>,
}

/// The account's latest `?2` complete matches of the format, newest first,
/// with their games won and lost. The limit is applied to the match ids
/// first, so the game counts and the archetype are read for those rows only.
fn latest_matches_sql() -> String {
    format!(
        "SELECT m.token, m.outcome, m.started_at, m.ended_at,
       (SELECT COUNT(*) FROM games g WHERE g.match_id = m.id AND g.result = 'win'),
       (SELECT COUNT(*) FROM games g WHERE g.match_id = m.id AND g.result = 'loss'),
       (SELECT a.name FROM match_archetypes x JOIN archetypes a ON a.id = x.archetype_id
        WHERE x.match_id = m.id AND x.is_opponent = 1)
FROM matches m
WHERE m.id IN (
    SELECT m.id FROM listed_matches m
    WHERE {ACCOUNT_COMPLETE} AND {by_format}
    ORDER BY m.started_at DESC, m.id DESC
    LIMIT ?2)
ORDER BY m.started_at DESC, m.id DESC",
        by_format = format_sql(3, 4)
    )
}

fn latest_matches(
    conn: &Connection,
    account: i64,
    limit: u32,
    format: &FormatFilter,
) -> Result<Vec<LatestMatch>, StoreError> {
    let mut stmt = conn.prepare_cached(&latest_matches_sql())?;
    let rows = stmt.query_map(
        params![account, limit, format.mode(), format.codes_json()?],
        |row| {
            Ok(LatestMatch {
                token: row.get(0)?,
                outcome: row.get(1)?,
                started_at: row.get(2)?,
                ended_at: row.get(3)?,
                games_won: row.get(4)?,
                games_lost: row.get(5)?,
                opponent_archetype: row.get(6)?,
            })
        },
    )?;
    let mut latest = Vec::new();
    for row in rows {
        latest.push(row?);
    }
    Ok(latest)
}

// ---- Last session ---------------------------------------------------------------

/// One match of the last session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionMatch {
    pub token: String,
    /// `win`, `loss`, `draw` or `unknown`.
    pub outcome: String,
    /// The opponent archetype's name, `Unknown` without one.
    pub opponent_archetype: String,
    pub games_won: u32,
    pub games_lost: u32,
}

/// The Last session widget (0.x `GetLastSession`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LastSession {
    pub started_at_ms: i64,
    /// The last match's end, or its start plus an hour.
    pub ended_at_ms: i64,
    /// The first match's local day, `Oct 8, 2026`.
    pub started_label: String,
    /// Oldest first.
    pub matches: Vec<SessionMatch>,
    /// `2-1`: wins and losses.
    pub record: String,
    /// `~1h 50m` or `~45m`.
    pub duration: String,
}

/// The account's last session in the format (ignores the timeframe): the
/// latest complete match and those before it with at most 2 hours between one's end and the
/// next one's start, among the latest 50. Days in `tz`.
pub fn last_session(
    conn: &Connection,
    account: Option<i64>,
    format: &FormatFilter,
    tz: Tz,
) -> Result<Option<LastSession>, StoreError> {
    let Some(account) = account else {
        return Ok(None);
    };
    let latest = latest_matches(conn, account, LAST_SESSION_LIMIT, format)?;
    let spans: Vec<(i64, Option<i64>)> = latest
        .iter()
        .map(|played| (played.started_at, played.ended_at))
        .collect();
    let length = session_length(&spans);
    if length == 0 {
        return Ok(None);
    }

    let mut matches = Vec::new();
    let (mut wins, mut losses) = (0, 0);
    for played in latest[..length].iter().rev() {
        match played.outcome.as_str() {
            "win" => wins += 1,
            "loss" => losses += 1,
            _ => {}
        }
        matches.push(SessionMatch {
            token: played.token.clone(),
            outcome: played.outcome.clone(),
            opponent_archetype: played
                .opponent_archetype
                .clone()
                .unwrap_or_else(|| UNKNOWN_ARCHETYPE.to_string()),
            games_won: played.games_won,
            games_lost: played.games_lost,
        });
    }

    let first = &latest[length - 1];
    let last = &latest[0];
    let started_at_ms = first.started_at;
    let ended_at_ms = last.ended_at.unwrap_or(last.started_at + ASSUMED_MATCH_MS);
    Ok(Some(LastSession {
        started_at_ms,
        ended_at_ms,
        started_label: day_label(started_at_ms, tz),
        matches,
        record: format!("{wins}-{losses}"),
        duration: duration_label(ended_at_ms - started_at_ms),
    }))
}

/// How many matches from the front of `newest_first` (start, end) make the
/// session: it ends at the first gap of more than 2 hours from a match's end
/// (its start plus an hour when it has none) to the next one's start.
fn session_length(newest_first: &[(i64, Option<i64>)]) -> usize {
    if newest_first.is_empty() {
        return 0;
    }
    let mut length = 1;
    for index in 1..newest_first.len() {
        let (started_at, ended_at) = newest_first[index];
        let next_start = newest_first[index - 1].0;
        let end = ended_at.unwrap_or(started_at + ASSUMED_MATCH_MS);
        if next_start - end > SESSION_GAP_MS {
            break;
        }
        length += 1;
    }
    length
}

/// `Oct 8, 2026` (PHP `M j, Y`) in `tz`.
fn day_label(ms: i64, tz: Tz) -> String {
    let instant = DateTime::<Utc>::from_timestamp_millis(ms).unwrap_or_default();
    instant.with_timezone(&tz).format("%b %-d, %Y").to_string()
}

/// `~1h 50m`, or `~45m` under an hour, in whole minutes.
fn duration_label(ms: i64) -> String {
    let minutes = ms / 60_000;
    let hours = minutes / 60;
    if hours > 0 {
        format!("~{hours}h {}m", minutes % 60)
    } else {
        format!("~{minutes}m")
    }
}

// ---- Recent matches -------------------------------------------------------------

/// One row of the Recent matches table: the match list's row and its deck.
#[derive(Debug, Clone, PartialEq)]
pub struct RecentMatch {
    pub row: MatchRow,
    pub deck_id: Option<i64>,
    pub deck_name: Option<String>,
}

/// The match list's rows of the window, account and format, newest first,
/// `?4` of them.
fn recent_matches_sql() -> String {
    format!(
        "{MATCH_ROW_SELECT}
WHERE {COMPLETE_IN_WINDOW} AND {of_account} AND {by_format}
ORDER BY m.started_at DESC, m.id DESC
LIMIT ?4",
        of_account = of_account(7),
        by_format = format_sql(5, 6)
    )
}

/// Whether any complete match counts for the account (every match without
/// one): the existence check behind the page's `has_matches`.
fn has_matches_sql() -> String {
    format!(
        "SELECT EXISTS (SELECT 1 FROM listed_matches m
WHERE m.state = 'complete' AND {of_account})",
        of_account = of_account(2)
    )
}

/// 0.x `hasMatches`: any complete match of the account (every match without
/// one), however old.
pub fn has_matches(
    conn: &Connection,
    account: Option<i64>,
    platform: Platform,
) -> Result<bool, StoreError> {
    Ok(conn
        .prepare_cached(&has_matches_sql())?
        .query_row(params![account, platform], |row| row.get(0))?)
}

/// The deck of each match token of the JSON array `?1`.
const DECKS_OF_TOKENS: &str = "
SELECT m.token, d.id, d.name FROM matches m
JOIN deck_versions v ON v.id = m.deck_version_id
JOIN decks d ON d.id = v.deck_id
WHERE m.token IN (SELECT value FROM json_each(?1))";

/// The Recent matches widget (0.x `RecentMatchesWidget`): the latest 5
/// complete matches in `range`, newest first.
pub fn recent_matches(
    conn: &Connection,
    account: Option<i64>,
    platform: Platform,
    range: Range,
    format: &FormatFilter,
) -> Result<Vec<RecentMatch>, StoreError> {
    let mut rows = Vec::new();
    {
        let mut stmt = conn.prepare_cached(&recent_matches_sql())?;
        let found = stmt.query_map(
            params![
                account,
                range.from_ms,
                range.to_ms,
                RECENT_MATCHES_LIMIT,
                format.mode(),
                format.codes_json()?,
                platform
            ],
            match_row,
        )?;
        for row in found {
            rows.push(row?);
        }
    }
    if rows.is_empty() {
        return Ok(Vec::new());
    }

    let tokens: Vec<&str> = rows.iter().map(|row| row.token.as_str()).collect();
    let mut decks: HashMap<String, (i64, String)> = HashMap::new();
    {
        let mut stmt = conn.prepare_cached(DECKS_OF_TOKENS)?;
        let found = stmt.query_map([serde_json::to_string(&tokens)?], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        for row in found {
            let (token, id, name) = row?;
            decks.insert(token, (id, name));
        }
    }

    let mut recent = Vec::new();
    for row in rows {
        let deck = decks.remove(&row.token);
        recent.push(RecentMatch {
            row,
            deck_id: deck.as_ref().map(|(id, _)| *id),
            deck_name: deck.map(|(_, name)| name),
        });
    }
    Ok(recent)
}

// ---- Window matches -------------------------------------------------------------

/// One complete match of the window: the Results chart's days and the
/// Formats and Sets cards read these (spec 4.1, 4.10).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowMatch {
    pub token: String,
    /// The format code (`CMODERN`, `DHOBHOBHOB`).
    pub format: Option<String>,
    pub outcome: Outcome,
    pub started_at: i64,
    /// The player's own deck group: its archetype when the deck has one,
    /// else the deck. `None` for a match without a deck.
    pub deck: Option<DeckLabel>,
}

fn window_matches_sql() -> String {
    format!(
        "SELECT m.token, m.format, m.outcome, m.started_at, d.id, d.name, a.id, a.name
FROM listed_matches m
LEFT JOIN deck_versions v ON v.id = m.deck_version_id
LEFT JOIN decks d ON d.id = v.deck_id
LEFT JOIN archetypes a ON a.id = d.archetype_id
WHERE {COMPLETE_IN_WINDOW} AND {of_account} AND {by_format}
ORDER BY m.started_at, m.id",
        of_account = of_account(6),
        by_format = format_sql(4, 5)
    )
}

/// The complete matches of the window, account and format, oldest first.
pub fn window_matches(
    conn: &Connection,
    account: Option<i64>,
    platform: Platform,
    range: Range,
    format: &FormatFilter,
) -> Result<Vec<WindowMatch>, StoreError> {
    let mut stmt = conn.prepare_cached(&window_matches_sql())?;
    let rows = stmt.query_map(
        params![
            account,
            range.from_ms,
            range.to_ms,
            format.mode(),
            format.codes_json()?,
            platform
        ],
        |row| {
            Ok(WindowMatch {
                token: row.get(0)?,
                format: row.get(1)?,
                outcome: stored_outcome(&row.get::<_, String>(2)?),
                started_at: row.get(3)?,
                deck: own_deck_label(
                    row.get::<_, Option<i64>>(4)?
                        .zip(row.get::<_, Option<String>>(5)?),
                    row.get::<_, Option<i64>>(6)?
                        .zip(row.get::<_, Option<String>>(7)?),
                ),
            })
        },
    )?;
    let mut found = Vec::new();
    for row in rows {
        found.push(row?);
    }
    Ok(found)
}

/// The group a deck falls under: its archetype `(id, name)` when it has one,
/// else the deck `(id, name)`.
fn own_deck_label(
    deck: Option<(i64, String)>,
    archetype: Option<(i64, String)>,
) -> Option<DeckLabel> {
    match (archetype, deck) {
        (Some((id, name)), _) => Some(DeckLabel::archetype(id, &name)),
        (None, Some((id, name))) => Some(DeckLabel::deck(id, &name)),
        (None, None) => None,
    }
}

// ---- Chart history and recent sessions -------------------------------------------

/// The latest `?3` complete matches of the account and format that started
/// before `?2`, newest first: the matches that feed the Results chart's
/// trend before its window starts.
fn earlier_matches_sql() -> String {
    format!(
        "SELECT m.started_at, m.outcome FROM listed_matches m
WHERE m.state = 'complete' AND m.started_at < ?2 AND {of_account} AND {by_format}
ORDER BY m.started_at DESC, m.id DESC
LIMIT ?3",
        of_account = of_account(6),
        by_format = format_sql(4, 5)
    )
}

/// Up to `TREND_HISTORY` complete matches of the account and format before
/// the window, oldest first.
pub fn earlier_matches(
    conn: &Connection,
    account: Option<i64>,
    platform: Platform,
    range: Range,
    format: &FormatFilter,
) -> Result<Vec<PlayedMatch>, StoreError> {
    let mut stmt = conn.prepare_cached(&earlier_matches_sql())?;
    let rows = stmt.query_map(
        params![
            account,
            range.from_ms,
            TREND_HISTORY,
            format.mode(),
            format.codes_json()?,
            platform
        ],
        |row| {
            Ok(PlayedMatch {
                started_at_ms: row.get(0)?,
                outcome: stored_outcome(&row.get::<_, String>(1)?),
            })
        },
    )?;
    let mut found = Vec::new();
    for row in rows {
        found.push(row?);
    }
    found.reverse();
    Ok(found)
}

/// The latest `?2` complete matches of the account and format, newest first.
fn latest_outcomes_sql() -> String {
    format!(
        "SELECT m.started_at, m.outcome FROM listed_matches m
WHERE m.state = 'complete' AND {of_account} AND {by_format}
ORDER BY m.started_at DESC, m.id DESC
LIMIT ?2",
        of_account = of_account(5),
        by_format = format_sql(3, 4)
    )
}

/// One local day of Recent sessions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecentSession {
    /// `YYYY-MM-DD` in the local timezone.
    pub day: String,
    /// Whole local days before today; 0 is today.
    pub days_ago: u32,
    pub record: Record,
}

/// The last `RECENT_SESSIONS` local days with a match, newest first, over
/// the account's matches in the format (ignores the timeframe). Days in `tz`.
pub fn recent_sessions(
    conn: &Connection,
    account: Option<i64>,
    platform: Platform,
    format: &FormatFilter,
    now_ms: i64,
    tz: Tz,
) -> Result<Vec<RecentSession>, StoreError> {
    let mut stmt = conn.prepare_cached(&latest_outcomes_sql())?;
    let mut limit: u32 = 100;
    loop {
        let rows = stmt.query_map(
            params![
                account,
                limit,
                format.mode(),
                format.codes_json()?,
                platform
            ],
            |row| {
                Ok(PlayedMatch {
                    started_at_ms: row.get(0)?,
                    outcome: stored_outcome(&row.get::<_, String>(1)?),
                })
            },
        )?;
        let mut played = Vec::new();
        for row in rows {
            played.push(row?);
        }
        let read_all = (played.len() as u32) < limit;
        let days: Vec<DayRow> = daily_results(&[], &played, tz)
            .into_iter()
            .filter(|day| day.win_rate.is_some())
            .collect();
        // The oldest day read may be cut short by the limit, so it is only
        // trusted when a newer day follows it or the whole history was read.
        if read_all || days.len() > RECENT_SESSIONS {
            let newest_first = days.into_iter().rev().take(RECENT_SESSIONS);
            return Ok(newest_first
                .map(|day| RecentSession {
                    days_ago: days_ago(&day.day, now_ms, tz),
                    record: Record::from_counts(day.wins, day.losses, day.draws),
                    day: day.day,
                })
                .collect());
        }
        limit = limit.saturating_mul(4);
    }
}

// ---- Recent decks ---------------------------------------------------------------

/// Recent decks shows this many (spec 4.7).
pub const RECENT_DECKS_LIMIT: u32 = 2;
/// A recent deck's sparkline runs over this many of its latest matches.
pub const SPARKLINE_MATCHES: u32 = 20;

/// One Recent decks entry: numbers all time, whatever the timeframe.
#[derive(Debug, Clone, PartialEq)]
pub struct RecentDeck {
    /// The deck; its `record` is every complete match, all time.
    pub deck: DeckSummary,
    /// Its games, all time (gameless history imports folded, as the deck pages).
    pub games: GameRecord,
    /// The match win rate after each of its latest matches, oldest first,
    /// 0 to 100.
    pub sparkline: Vec<f64>,
}

/// The account's (`?1`, every account's when NULL) unarchived decks with a
/// complete match of the format (`?2`, `?3`), by their latest match start,
/// `?4` of them.
fn recent_deck_ids_sql() -> String {
    format!(
        "SELECT v.deck_id, MAX(m.started_at) AS last_played FROM listed_matches m
JOIN deck_versions v ON v.id = m.deck_version_id
JOIN decks d ON d.id = v.deck_id
WHERE m.state = 'complete' AND {of_account} AND d.archived_at IS NULL AND {by_format}
GROUP BY v.deck_id
ORDER BY last_played DESC, v.deck_id DESC
LIMIT ?4",
        of_account = of_account(5),
        by_format = format_sql(2, 3)
    )
}

/// Deck `?1`'s latest `?2` complete match outcomes, newest first.
const DECK_LATEST_OUTCOMES: &str = "
SELECT m.outcome FROM deck_versions v
JOIN listed_matches m ON m.deck_version_id = v.id
WHERE v.deck_id = ?1 AND m.state = 'complete'
ORDER BY m.started_at DESC, m.id DESC
LIMIT ?2";

/// Recent decks (spec 4.7): the two decks with the latest match start in the
/// format, whatever the timeframe, each with its all-time numbers.
pub fn recent_decks(
    conn: &Connection,
    account: Option<i64>,
    platform: Platform,
    format: &FormatFilter,
) -> Result<Vec<RecentDeck>, StoreError> {
    let mut ids: Vec<i64> = Vec::new();
    {
        let mut stmt = conn.prepare_cached(&recent_deck_ids_sql())?;
        let rows = stmt.query_map(
            params![
                account,
                format.mode(),
                format.codes_json()?,
                RECENT_DECKS_LIMIT,
                platform
            ],
            |row| row.get(0),
        )?;
        for row in rows {
            ids.push(row?);
        }
    }

    let mut recent = Vec::new();
    for deck_id in ids {
        let found = conn
            .prepare_cached(&header_sql())?
            .query_row([deck_id], summary_row)
            .optional()?;
        let Some(mut deck) = found else {
            continue;
        };
        crate::deck_pages::with_commander_cover(conn, &mut deck)?;
        let mut games = GameRecord::default();
        for tally in version_tallies(conn, deck_id, Range::all_time(), None)? {
            games = games.plus(tally.games);
        }
        let mut outcomes: Vec<String> = Vec::new();
        {
            let mut stmt = conn.prepare_cached(DECK_LATEST_OUTCOMES)?;
            let rows = stmt.query_map(params![deck_id, SPARKLINE_MATCHES], |row| row.get(0))?;
            for row in rows {
                outcomes.push(row?);
            }
        }
        outcomes.reverse();
        recent.push(RecentDeck {
            deck,
            games,
            sparkline: cumulative_win_rates(&outcomes),
        });
    }
    Ok(recent)
}

/// The match win rate after each outcome, oldest first, 0 to 100; anything
/// but a win counts as played.
fn cumulative_win_rates(oldest_first: &[String]) -> Vec<f64> {
    let mut rates = Vec::new();
    let mut wins = 0u32;
    for (index, outcome) in oldest_first.iter().enumerate() {
        if outcome == "win" {
            wins += 1;
        }
        rates.push(f64::from(wins) / (index as f64 + 1.0) * 100.0);
    }
    rates
}

// ---- Top matchups ----------------------------------------------------------------

/// Top matchups shows this many opponent archetypes (0.x
/// `GetDashboardMatchupSpread`, "top 5").
pub const MATCHUP_SPREAD_LIMIT: u32 = 5;

/// One opponent archetype of the Top matchups card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchupSpreadRow {
    pub archetype_id: i64,
    pub name: String,
    /// The matches against it, draws counted as played.
    pub record: Record,
}

/// The Top matchups rows and the matches their shares are of.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchupSpread {
    pub rows: Vec<MatchupSpreadRow>,
    /// Every match Top matchups could count (complete, with a game, with
    /// an opponent archetype) in the window and format, not just the rows
    /// shown; 0 without rows.
    pub total: u32,
}

/// Per opponent archetype (the opponent seat's row): account `?1`'s
/// complete listed matches of the format started from `?2` to `?3` with a
/// game (0.x pairs the row with an opponent who played a game), most played
/// first, then by name and id, `?4` of them. The last column is every
/// archetype's matches added up (the window function runs before `LIMIT`):
/// the denominator of a row's share.
fn matchup_spread_sql() -> String {
    format!(
        "SELECT a.id, a.name,
       COUNT(DISTINCT CASE WHEN m.outcome = 'win' THEN m.id END),
       COUNT(DISTINCT CASE WHEN m.outcome = 'loss' THEN m.id END),
       COUNT(DISTINCT m.id) AS played,
       SUM(COUNT(DISTINCT m.id)) OVER () AS total
FROM listed_matches m
JOIN match_archetypes ma ON ma.match_id = m.id AND ma.is_opponent = 1
JOIN archetypes a ON a.id = ma.archetype_id
WHERE m.account_id = ?1 AND m.state = 'complete' AND m.started_at BETWEEN ?2 AND ?3
  AND EXISTS (SELECT 1 FROM games g WHERE g.match_id = m.id)
  AND {by_format}
GROUP BY a.id
ORDER BY played DESC, a.name, a.id
LIMIT ?4",
        by_format = format_sql(5, 6)
    )
}

/// The Top matchups card (0.x `MatchupSpreadWidget`,
/// `GetDashboardMatchupSpread`): the five opponent archetypes the account
/// met most in `range` and the format, with the matches the shares are of
/// (spec 4.3). Empty without an account, as 0.x.
pub fn matchup_spread(
    conn: &Connection,
    account: Option<i64>,
    range: Range,
    format: &FormatFilter,
) -> Result<MatchupSpread, StoreError> {
    let Some(account) = account else {
        return Ok(MatchupSpread {
            rows: Vec::new(),
            total: 0,
        });
    };
    let mut stmt = conn.prepare_cached(&matchup_spread_sql())?;
    let rows = stmt.query_map(
        params![
            account,
            range.from_ms,
            range.to_ms,
            MATCHUP_SPREAD_LIMIT,
            format.mode(),
            format.codes_json()?
        ],
        |row| {
            let found = MatchupSpreadRow {
                archetype_id: row.get(0)?,
                name: row.get(1)?,
                record: Record::from_total(row.get(2)?, row.get(3)?, row.get(4)?),
            };
            Ok((found, row.get::<_, u32>(5)?))
        },
    )?;
    let mut spread = MatchupSpread {
        rows: Vec::new(),
        total: 0,
    };
    for row in rows {
        let (row, total) = row?;
        spread.rows.push(row);
        spread.total = total;
    }
    Ok(spread)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{account, db, hide, number as count};
    use decks::stats::{previous_range, timeframe_range};

    const MINUTE: i64 = 60_000;
    const HOUR: i64 = 60 * MINUTE;
    const DAY: i64 = 24 * HOUR;
    /// 2026-10-07T12:00:00Z.
    const NOW: i64 = 1_791_374_400_000;
    const UTC: Tz = chrono_tz::UTC;

    fn deck(conn: &Connection, account_id: Option<i64>, name: &str, format: &str) -> i64 {
        let n = count(conn, "SELECT COUNT(*) FROM decks") + 1;
        conn.query_row(
            "INSERT INTO decks (mtgo_id, account_id, name, format, created_at, updated_at, source)
             VALUES (?1, ?2, ?3, ?4, 0, 0, 'file') RETURNING id",
            params![format!("deck-{n}"), account_id, name, format],
            |row| row.get(0),
        )
        .expect("deck")
    }

    fn add_version(conn: &Connection, deck_id: i64, modified_at: i64) -> i64 {
        let n = count(conn, "SELECT COUNT(*) FROM deck_versions") + 1;
        conn.query_row(
            "INSERT INTO deck_versions (deck_id, signature, modified_at) VALUES (?1, ?2, ?3)
             RETURNING id",
            params![deck_id, format!("SIG-{n}"), modified_at],
            |row| row.get(0),
        )
        .expect("version")
    }

    /// An account Qme with one deck and one version.
    fn setup(conn: &Connection) -> (i64, i64, i64) {
        let me = account(conn, "Qme");
        let deck_id = deck(conn, Some(me), "Qtron", "CModern");
        let version_id = add_version(conn, deck_id, 0);
        (me, deck_id, version_id)
    }

    /// A stored match, as the tests need it again.
    #[derive(Debug, Clone)]
    struct Played {
        id: i64,
        token: String,
        outcome: String,
    }

    /// What to store: a complete match with `outcome` of `version` (none for
    /// a match without a deck) by `account_id`.
    struct Spec<'a> {
        account_id: i64,
        version: Option<i64>,
        state: &'a str,
        outcome: &'a str,
        format: Option<&'a str>,
        started_at: i64,
        ended_at: Option<i64>,
    }

    fn spec(account_id: i64, version: i64, outcome: &str, started_at: i64) -> Spec<'_> {
        Spec {
            account_id,
            version: Some(version),
            state: "complete",
            outcome,
            format: Some("CModern"),
            started_at,
            ended_at: None,
        }
    }

    fn store(conn: &Connection, spec: Spec) -> Played {
        let n = count(conn, "SELECT COUNT(*) FROM matches") + 1;
        let token = format!("tok-{n}");
        let id: i64 = conn
            .query_row(
                "INSERT INTO matches (token, account_id, source, format, state, outcome,
                     started_at, ended_at, last_activity_at, revision, derived_version)
                 VALUES (?1, ?2, 'log', ?3, ?4, ?5, ?6, ?7, ?6, 1, 1) RETURNING id",
                params![
                    token,
                    spec.account_id,
                    spec.format,
                    spec.state,
                    spec.outcome,
                    spec.started_at,
                    spec.ended_at
                ],
                |row| row.get(0),
            )
            .expect("match");
        if let Some(version) = spec.version {
            conn.execute(
                "UPDATE matches SET deck_version_id = ?2 WHERE token = ?1",
                params![token, version],
            )
            .expect("link");
        }
        Played {
            id,
            token,
            outcome: spec.outcome.to_string(),
        }
    }

    fn outcome_at(conn: &Connection, me: i64, version: i64, outcome: &str, at: i64) -> Played {
        store(conn, spec(me, version, outcome, at))
    }

    fn won(conn: &Connection, me: i64, version: i64, at: i64) -> Played {
        outcome_at(conn, me, version, "win", at)
    }

    fn lost(conn: &Connection, me: i64, version: i64, at: i64) -> Played {
        outcome_at(conn, me, version, "loss", at)
    }

    fn game(conn: &Connection, match_id: i64, result: &str, on_play: Option<bool>) {
        let number = count(
            conn,
            &format!("SELECT COUNT(*) FROM games WHERE match_id = {match_id}"),
        ) + 1;
        conn.execute(
            "INSERT INTO games (match_id, game_number, result, end_reason, on_play)
             VALUES (?1, ?2, ?3, 'win', ?4)",
            params![match_id, number, result, on_play],
        )
        .expect("game");
    }

    fn week() -> (Range, Option<Range>) {
        let range = timeframe_range("week", NOW, UTC).expect("week");
        (range, previous_range("week", range.from_ms, UTC))
    }

    fn monthly() -> Range {
        timeframe_range("monthly", NOW, UTC).expect("monthly")
    }

    fn at(text: &str) -> i64 {
        DateTime::parse_from_rfc3339(text)
            .expect("time")
            .timestamp_millis()
    }

    /// A complete match of `me` in `format` with one game of its outcome
    /// (on the play), on `version` when given (a limited match has none).
    fn played_in(
        conn: &Connection,
        me: i64,
        version: Option<i64>,
        format: Option<&str>,
        outcome: &str,
        at: i64,
    ) -> Played {
        let played = store(
            conn,
            Spec {
                account_id: me,
                version,
                state: "complete",
                outcome,
                format,
                started_at: at,
                ended_at: None,
            },
        );
        let result = if outcome == "win" { "win" } else { "loss" };
        game(conn, played.id, result, Some(true));
        played
    }

    // Format filter and account scoping (dashboard redesign spec 5.2, 5.3)

    #[test]
    fn the_format_filter_counts_each_mode() {
        let (_dir, conn) = db("dashboard-format-modes");
        let (me, _, version) = setup(&conn);
        played_in(&conn, me, Some(version), Some("CModern"), "win", NOW - HOUR);
        played_in(
            &conn,
            me,
            Some(version),
            Some("CMODERN"),
            "loss",
            NOW - HOUR,
        );
        played_in(&conn, me, None, Some("CPAUPER"), "win", NOW - HOUR);
        played_in(&conn, me, None, None, "win", NOW - HOUR);
        played_in(&conn, me, None, Some("DQNWQNWQNW"), "win", NOW - HOUR);
        played_in(&conn, me, None, Some("S6QSL"), "loss", NOW - HOUR);
        let options = format_options(&conn, Some(me), Platform::Mtgo).expect("options");
        let total = |filter: &FormatFilter| {
            overview(
                &conn,
                Some(me),
                Platform::Mtgo,
                Range::all_time(),
                None,
                filter,
            )
            .expect("overview")
            .record
            .total
        };
        assert_eq!(total(&FormatFilter::All), 6);
        assert_eq!(
            total(&FormatFilter::Constructed),
            4,
            "a match with no format is constructed"
        );
        assert_eq!(total(&FormatFilter::Limited), 2);
        assert_eq!(
            total(&resolve_format("modern", &options).expect("offered").1),
            2,
            "both codes of Modern"
        );
        assert_eq!(
            total(&resolve_format("pauper", &options).expect("offered").1),
            1
        );
    }

    #[test]
    fn format_keys_merge_code_case_variants_and_leave_limited_out() {
        let (_dir, conn) = db("dashboard-format-options");
        let (me, _, version) = setup(&conn);
        played_in(&conn, me, Some(version), Some("CModern"), "win", NOW - HOUR);
        played_in(
            &conn,
            me,
            Some(version),
            Some("CMODERN"),
            "loss",
            NOW - HOUR,
        );
        played_in(&conn, me, None, Some("CPAUPER"), "win", NOW - HOUR);
        played_in(&conn, me, None, Some("DQNWQNWQNW"), "win", NOW - HOUR);
        let other = account(&conn, "Qother");
        played_in(&conn, other, None, Some("CLEGACY"), "win", NOW - HOUR);

        let options = format_options(&conn, Some(me), Platform::Mtgo).expect("options");
        assert_eq!(
            options,
            vec![
                FormatOption {
                    key: "modern".to_string(),
                    label: "Modern".to_string(),
                    codes: vec!["CMODERN".to_string(), "CModern".to_string()],
                },
                FormatOption {
                    key: "pauper".to_string(),
                    label: "Pauper".to_string(),
                    codes: vec!["CPAUPER".to_string()],
                },
            ]
        );
        assert_eq!(
            resolve_format("modern", &options),
            Some((
                "modern".to_string(),
                FormatFilter::Codes(vec!["CMODERN".to_string(), "CModern".to_string()])
            ))
        );
        assert_eq!(
            resolve_format("limited", &options),
            Some(("limited".to_string(), FormatFilter::Limited))
        );
        assert_eq!(
            resolve_format("constructed", &options),
            Some(("constructed".to_string(), FormatFilter::Constructed))
        );
        assert_eq!(
            format_options(&conn, None, Platform::Mtgo)
                .expect("options")
                .len(),
            3
        );
    }

    #[test]
    fn an_unknown_format_key_is_left_to_the_last_played_format() {
        let options = vec![FormatOption {
            key: "modern".to_string(),
            label: "Modern".to_string(),
            codes: vec!["CMODERN".to_string()],
        }];
        for key in ["legacy", "", "Modern"] {
            assert_eq!(resolve_format(key, &options), None, "{key}");
        }
        assert_eq!(
            resolve_format("all", &options),
            Some(("all".to_string(), FormatFilter::All))
        );
    }

    #[test]
    fn the_page_opens_on_the_format_last_played() {
        let (_dir, conn) = db("dashboard-last-played");
        let (me, _, version) = setup(&conn);
        assert_eq!(
            last_played_format(&conn, Some(me), Platform::Mtgo).expect("last"),
            "constructed"
        );
        played_in(
            &conn,
            me,
            Some(version),
            Some("CMODERN"),
            "win",
            NOW - 3 * HOUR,
        );
        played_in(&conn, me, None, Some("DQSET"), "win", NOW - 2 * HOUR);
        assert_eq!(
            last_played_format(&conn, Some(me), Platform::Mtgo).expect("last"),
            "limited"
        );
        played_in(
            &conn,
            me,
            Some(version),
            Some("CPAUPER"),
            "loss",
            NOW - HOUR,
        );
        assert_eq!(
            last_played_format(&conn, Some(me), Platform::Mtgo).expect("last"),
            "pauper"
        );
        // A constructed match with no format falls back to Constructed.
        played_in(&conn, me, Some(version), None, "win", NOW - 60_000);
        assert_eq!(
            last_played_format(&conn, Some(me), Platform::Mtgo).expect("last"),
            "constructed"
        );
    }

    #[test]
    fn the_last_played_format_ignores_hidden_and_incomplete_matches() {
        let (_dir, conn) = db("dashboard-last-played-hidden");
        let (me, _, version) = setup(&conn);
        played_in(
            &conn,
            me,
            Some(version),
            Some("CMODERN"),
            "win",
            NOW - 3 * HOUR,
        );
        let hidden = played_in(&conn, me, None, Some("DQSET"), "win", NOW - HOUR);
        conn.execute(
            "UPDATE matches SET hidden_at = 1 WHERE id = ?1",
            [hidden.id],
        )
        .expect("hide");
        assert_eq!(
            last_played_format(&conn, Some(me), Platform::Mtgo).expect("last"),
            "modern"
        );
    }

    #[test]
    fn every_match_of_the_account_counts_linked_or_not() {
        let (_dir, conn) = db("dashboard-account-scope");
        let (me, _, version) = setup(&conn);
        // A deck with no account (an import that could not give it one).
        let orphan = add_version(&conn, deck(&conn, None, "Qorphan", "CModern"), 0);
        played_in(&conn, me, Some(version), Some("CModern"), "win", NOW - HOUR);
        played_in(&conn, me, None, Some("CModern"), "loss", NOW - HOUR);
        played_in(&conn, me, None, Some("DQNWQNWQNW"), "win", NOW - HOUR);
        played_in(&conn, me, Some(orphan), Some("CModern"), "win", NOW - HOUR);
        let other = account(&conn, "Qother");
        played_in(&conn, other, None, Some("CModern"), "win", NOW - HOUR);

        let mine = overview(
            &conn,
            Some(me),
            Platform::Mtgo,
            Range::all_time(),
            None,
            &FormatFilter::All,
        )
        .expect("overview");
        assert_eq!(mine.record, Record::from_counts(3, 1, 0));
        let everyone = overview(
            &conn,
            None,
            Platform::Mtgo,
            Range::all_time(),
            None,
            &FormatFilter::All,
        )
        .expect("overview");
        assert_eq!(everyone.record.total, 5, "no current account: every match");
        let limited = recent_sessions(
            &conn,
            Some(me),
            Platform::Mtgo,
            &FormatFilter::Limited,
            NOW,
            UTC,
        )
        .expect("sessions");
        assert_eq!(
            limited.len(),
            1,
            "the limited match is the account's, on one day"
        );
        assert_eq!(limited[0].record, Record::from_counts(1, 0, 0));
        let recent = recent_matches(
            &conn,
            Some(me),
            Platform::Mtgo,
            monthly(),
            &FormatFilter::All,
        )
        .expect("recent");
        assert_eq!(recent.len(), 4);
        assert!(has_matches(&conn, Some(me), Platform::Mtgo).expect("has matches"));
        assert!(!has_matches(&conn, Some(other + 100), Platform::Mtgo).expect("has matches"));
    }

    #[test]
    fn the_session_and_matchups_follow_the_format() {
        let (_dir, conn) = db("dashboard-format-session");
        let (me, _, version) = setup(&conn);
        let burn = archetype(&conn, "Qburn");
        let modern = played_in(
            &conn,
            me,
            Some(version),
            Some("CModern"),
            "win",
            NOW - 2 * HOUR,
        );
        seat(&conn, modern.id, true, burn);
        played_in(&conn, me, None, Some("DQNWQNWQNW"), "loss", NOW - HOUR);

        let session = last_session(&conn, Some(me), &FormatFilter::Constructed, UTC)
            .expect("session")
            .expect("a session");
        assert_eq!(session.matches.len(), 1);
        assert_eq!(session.matches[0].token, modern.token);
        let limited = last_session(&conn, Some(me), &FormatFilter::Limited, UTC)
            .expect("session")
            .expect("a session");
        assert_eq!(limited.record, "0-1");
        let (range, _) = week();
        assert_eq!(
            matchup_spread(&conn, Some(me), range, &FormatFilter::Constructed)
                .expect("spread")
                .rows
                .len(),
            1
        );
        assert!(
            matchup_spread(&conn, Some(me), range, &FormatFilter::Limited)
                .expect("spread")
                .rows
                .is_empty()
        );
    }

    // Overview (0.x KpiStripWidgetTest, HomeMatchWinrateTest)

    #[test]
    fn the_overview_has_the_match_record_game_win_rate_and_deltas() {
        let (_dir, conn) = db("dashboard-overview");
        let (me, _, version) = setup(&conn);
        for outcome in ["win", "loss", "draw"] {
            let played = outcome_at(&conn, me, version, outcome, NOW - HOUR);
            let result = if outcome == "win" { "win" } else { "loss" };
            game(&conn, played.id, result, None);
        }

        let overview = overview(
            &conn,
            Some(me),
            Platform::Mtgo,
            Range::all_time(),
            None,
            &FormatFilter::All,
        )
        .expect("overview");
        assert_eq!(overview.record, Record::from_counts(1, 1, 1));
        assert_eq!(overview.games, GameRecord::new(1, 2));
        assert_eq!(overview.match_win_rate_delta, 0, "all time has no delta");
        assert_eq!(overview.game_win_rate_delta, 0);
    }

    #[test]
    fn the_overview_counts_draws_as_matches_played_in_the_match_win_rate() {
        let (_dir, conn) = db("dashboard-overview-draws");
        let (me, _, version) = setup(&conn);
        for outcome in ["win", "loss", "draw", "draw"] {
            let played = outcome_at(&conn, me, version, outcome, NOW - HOUR);
            let result = if outcome == "win" { "win" } else { "loss" };
            game(&conn, played.id, result, None);
        }

        let overview = overview(
            &conn,
            Some(me),
            Platform::Mtgo,
            Range::all_time(),
            None,
            &FormatFilter::All,
        )
        .expect("overview");
        assert_eq!(overview.record.wins, 1);
        assert_eq!(overview.record.losses, 1);
        assert_eq!(overview.record.draws, 2);
        // 1 / 4 matches played.
        assert_eq!(overview.record.win_rate, 25);
    }

    #[test]
    fn the_overview_record_leaves_out_matches_without_games_and_other_accounts_but_counts_unlinked_ones()
     {
        let (_dir, conn) = db("dashboard-overview-scope");
        let (me, _, version) = setup(&conn);
        let counted = won(&conn, me, version, NOW - HOUR);
        game(&conn, counted.id, "win", Some(true));
        game(&conn, counted.id, "unknown", Some(false));
        // No games: 0.x inner-joins games.
        lost(&conn, me, version, NOW - HOUR);
        // Not complete.
        let mut ongoing = spec(me, version, "loss", NOW - HOUR);
        ongoing.state = "in_progress";
        let ongoing = store(&conn, ongoing);
        game(&conn, ongoing.id, "loss", None);
        // Another account's deck, and a match without a deck.
        let other = account(&conn, "Qother");
        let other_version = add_version(&conn, deck(&conn, Some(other), "Qelse", "CModern"), 0);
        let theirs = lost(&conn, other, other_version, NOW - HOUR);
        game(&conn, theirs.id, "loss", None);
        let mut deckless = spec(me, version, "loss", NOW - HOUR);
        deckless.version = None;
        let deckless = store(&conn, deckless);
        game(&conn, deckless.id, "loss", None);

        let mine = overview(
            &conn,
            Some(me),
            Platform::Mtgo,
            Range::all_time(),
            None,
            &FormatFilter::All,
        )
        .expect("overview");
        // The match without a deck is the account's (spec 5.3).
        assert_eq!(mine.record, Record::from_counts(1, 1, 0));
        assert_eq!(mine.games, GameRecord::new(1, 1));

        // Without an account every complete match with games counts.
        let everyone = overview(
            &conn,
            None,
            Platform::Mtgo,
            Range::all_time(),
            None,
            &FormatFilter::All,
        )
        .expect("overview");
        assert_eq!(everyone.record, Record::from_counts(1, 2, 0));
        assert_eq!(everyone.streak, Streak::default());
        assert_eq!(everyone.play_draw, PlayDrawSplit::default());
    }

    // Streak (0.x GetStreakTest)

    #[test]
    fn no_account_gives_no_streak() {
        let (_dir, conn) = db("dashboard-streak-none");
        let (_range, previous) = week();
        let overview = overview(
            &conn,
            None,
            Platform::Mtgo,
            week().0,
            previous,
            &FormatFilter::All,
        )
        .expect("overview");
        assert_eq!(
            overview.streak,
            Streak {
                current: None,
                best_win: 0,
                best_loss: 0
            }
        );
    }

    fn streak_of(outcomes: &[&str], window: Range) -> Streak {
        let (_dir, conn) = db("dashboard-streak");
        let (me, _, version) = setup(&conn);
        let count = outcomes.len() as i64;
        for (index, outcome) in outcomes.iter().enumerate() {
            let hours_ago = count + 1 - index as i64;
            outcome_at(&conn, me, version, outcome, NOW - hours_ago * HOUR);
        }
        overview(
            &conn,
            Some(me),
            Platform::Mtgo,
            window,
            None,
            &FormatFilter::All,
        )
        .expect("overview")
        .streak
    }

    #[test]
    fn the_current_win_streak() {
        let streak = streak_of(&["loss", "win", "win", "win"], week().0);
        assert_eq!(streak.current.as_deref(), Some("3W"));
        assert_eq!(streak.best_win, 3);
    }

    #[test]
    fn the_current_loss_streak() {
        let streak = streak_of(&["win", "loss", "loss"], week().0);
        assert_eq!(streak.current.as_deref(), Some("2L"));
        assert_eq!(streak.best_loss, 2);
    }

    #[test]
    fn draws_break_the_streak() {
        let streak = streak_of(&["win", "win", "draw", "win"], week().0);
        assert_eq!(streak.current.as_deref(), Some("1W"));
        assert_eq!(streak.best_win, 2);
    }

    #[test]
    fn unknown_outcomes_break_the_streak() {
        let streak = streak_of(&["win", "win", "unknown"], week().0);
        assert_eq!(streak.current, None);
        assert_eq!(streak.best_win, 2);
    }

    #[test]
    fn best_streaks_cover_the_whole_history() {
        let (_dir, conn) = db("dashboard-streak-history");
        let (me, _, version) = setup(&conn);
        for days_ago in (6..=10).rev() {
            won(&conn, me, version, NOW - days_ago * DAY);
        }
        lost(&conn, me, version, NOW - 5 * DAY);
        won(&conn, me, version, NOW - 2 * HOUR);
        won(&conn, me, version, NOW - HOUR);

        let window = Range {
            from_ms: NOW - 3 * DAY,
            to_ms: NOW,
        };
        let streak = overview(
            &conn,
            Some(me),
            Platform::Mtgo,
            window,
            None,
            &FormatFilter::All,
        )
        .expect("overview")
        .streak;
        assert_eq!(streak.current.as_deref(), Some("2W"));
        assert_eq!(streak.best_win, 5);
    }

    // Win rate deltas (0.x GetWinrateDeltaTest)

    #[test]
    fn no_account_gives_no_deltas() {
        let (_dir, conn) = db("dashboard-delta-none");
        let (range, previous) = week();
        let overview = overview(
            &conn,
            None,
            Platform::Mtgo,
            range,
            previous,
            &FormatFilter::All,
        )
        .expect("overview");
        assert_eq!(overview.match_win_rate_delta, 0);
        assert_eq!(overview.game_win_rate_delta, 0);
    }

    #[test]
    fn a_better_window_gives_a_positive_delta() {
        let (_dir, conn) = db("dashboard-delta-up");
        let (me, _, version) = setup(&conn);
        // Previous week: 1W 1L (50%).
        won(&conn, me, version, NOW - 10 * DAY);
        lost(&conn, me, version, NOW - 9 * DAY);
        // Current week: 3W 1L (75%).
        won(&conn, me, version, NOW - 2 * DAY);
        won(&conn, me, version, NOW - 2 * DAY);
        won(&conn, me, version, NOW - DAY);
        lost(&conn, me, version, NOW - DAY);

        let (range, previous) = week();
        let overview = overview(
            &conn,
            Some(me),
            Platform::Mtgo,
            range,
            previous,
            &FormatFilter::All,
        )
        .expect("overview");
        assert_eq!(overview.match_win_rate_delta, 25);
    }

    #[test]
    fn a_worse_window_gives_a_negative_delta() {
        let (_dir, conn) = db("dashboard-delta-down");
        let (me, _, version) = setup(&conn);
        // Previous week: 3W 1L (75%).
        won(&conn, me, version, NOW - 10 * DAY);
        won(&conn, me, version, NOW - 10 * DAY);
        won(&conn, me, version, NOW - 9 * DAY);
        lost(&conn, me, version, NOW - 9 * DAY);
        // Current week: 1W 3L (25%).
        won(&conn, me, version, NOW - DAY);
        lost(&conn, me, version, NOW - DAY);
        lost(&conn, me, version, NOW - DAY);
        lost(&conn, me, version, NOW - DAY);

        let (range, previous) = week();
        let overview = overview(
            &conn,
            Some(me),
            Platform::Mtgo,
            range,
            previous,
            &FormatFilter::All,
        )
        .expect("overview");
        assert_eq!(overview.match_win_rate_delta, -50);
    }

    #[test]
    fn the_previous_window_starts_on_local_midnight() {
        let (_dir, conn) = db("dashboard-delta-tz");
        let (me, _, version) = setup(&conn);
        let tz: Tz = chrono_tz::America::Los_Angeles;
        // 2 Sep 00:00 PDT to 9 Sep 23:59:59.999 PDT.
        let range = Range {
            from_ms: at("2026-09-02T07:00:00Z"),
            to_ms: at("2026-09-10T06:59:59.999Z"),
        };
        let previous = previous_range("week", range.from_ms, tz);
        // Current: 1W 1L (50%).
        won(&conn, me, version, at("2026-09-05T12:00:00Z"));
        lost(&conn, me, version, at("2026-09-06T12:00:00Z"));
        // Previous: 25 Aug 00:00 PDT to 1 Sep 23:59:59.999 PDT.
        won(&conn, me, version, at("2026-08-27T12:00:00Z"));
        lost(&conn, me, version, at("2026-08-28T12:00:00Z"));
        // 05:00 PDT on 25 Aug: inside the local window, before UTC midnight
        // of 26 Aug.
        lost(&conn, me, version, at("2026-08-25T12:00:00Z"));

        let overview = overview(
            &conn,
            Some(me),
            Platform::Mtgo,
            range,
            previous,
            &FormatFilter::All,
        )
        .expect("overview");
        // Previous 1W 2L (33%), current 50%.
        assert_eq!(overview.match_win_rate_delta, 17);
    }

    #[test]
    fn draws_count_as_matches_played_in_the_delta() {
        let (_dir, conn) = db("dashboard-delta-draws");
        let (me, _, version) = setup(&conn);
        // Current: 1W 1D = 50%, not 100%. Nothing before.
        won(&conn, me, version, NOW - DAY);
        outcome_at(&conn, me, version, "draw", NOW - DAY);

        let (range, previous) = week();
        let overview = overview(
            &conn,
            Some(me),
            Platform::Mtgo,
            range,
            previous,
            &FormatFilter::All,
        )
        .expect("overview");
        assert_eq!(overview.match_win_rate_delta, 50);
    }

    #[test]
    fn the_game_delta_counts_every_game_of_the_windows() {
        let (_dir, conn) = db("dashboard-delta-games");
        let (me, _, version) = setup(&conn);
        let before = won(&conn, me, version, NOW - 10 * DAY);
        game(&conn, before.id, "win", None);
        game(&conn, before.id, "loss", None);
        let now = won(&conn, me, version, NOW - DAY);
        game(&conn, now.id, "win", None);
        game(&conn, now.id, "win", None);
        game(&conn, now.id, "loss", None);
        game(&conn, now.id, "win", None);

        let (range, previous) = week();
        let overview = overview(
            &conn,
            Some(me),
            Platform::Mtgo,
            range,
            previous,
            &FormatFilter::All,
        )
        .expect("overview");
        // 75% now, 50% before.
        assert_eq!(overview.game_win_rate_delta, 25);
        assert_eq!(overview.games, GameRecord::new(3, 1));
    }

    // Play/draw split (0.x GetPlayDrawSplitTest)

    #[test]
    fn no_account_gives_no_play_draw_rates() {
        let (_dir, conn) = db("dashboard-play-draw-none");
        let (range, previous) = week();
        let overview = overview(
            &conn,
            None,
            Platform::Mtgo,
            range,
            previous,
            &FormatFilter::All,
        )
        .expect("overview");
        assert_eq!(
            overview.play_draw,
            PlayDrawSplit {
                otp_win_rate: 0,
                otd_win_rate: 0
            }
        );
    }

    #[test]
    fn play_and_draw_win_rates() {
        let (_dir, conn) = db("dashboard-play-draw");
        let (me, _, version) = setup(&conn);
        let first = won(&conn, me, version, NOW - HOUR);
        let second = lost(&conn, me, version, NOW - HOUR);
        game(&conn, first.id, "win", Some(true));
        game(&conn, second.id, "loss", Some(true));
        game(&conn, first.id, "win", Some(false));
        game(&conn, second.id, "win", Some(false));

        let (range, previous) = week();
        let overview = overview(
            &conn,
            Some(me),
            Platform::Mtgo,
            range,
            previous,
            &FormatFilter::All,
        )
        .expect("overview");
        assert_eq!(overview.play_draw.otp_win_rate, 50);
        assert_eq!(overview.play_draw.otd_win_rate, 100);
    }

    #[test]
    fn play_draw_reads_only_the_local_players_side() {
        // v1 keeps only the local player's play or draw on the game.
        let (_dir, conn) = db("dashboard-play-draw-local");
        let (me, _, version) = setup(&conn);
        let played = won(&conn, me, version, NOW - HOUR);
        game(&conn, played.id, "win", Some(true));
        // Unknown play or draw counts nowhere; an unknown result is lost.
        game(&conn, played.id, "win", None);
        game(&conn, played.id, "unknown", Some(false));

        let (range, previous) = week();
        let overview = overview(
            &conn,
            Some(me),
            Platform::Mtgo,
            range,
            previous,
            &FormatFilter::All,
        )
        .expect("overview");
        assert_eq!(overview.play_draw.otp_win_rate, 100);
        assert_eq!(overview.play_draw.otd_win_rate, 0);
    }

    // Chart history and recent sessions (results chart trend spec)

    #[test]
    fn earlier_matches_are_the_latest_before_the_window_oldest_first() {
        let (_dir, conn) = db("dashboard-earlier");
        let (me, _, version) = setup(&conn);
        won(&conn, me, version, NOW - 30 * DAY);
        lost(&conn, me, version, NOW - 20 * DAY);
        won(&conn, me, version, NOW - HOUR);
        let range = Range {
            from_ms: NOW - 10 * DAY,
            to_ms: NOW,
        };
        let earlier = earlier_matches(&conn, Some(me), Platform::Mtgo, range, &FormatFilter::All)
            .expect("earlier");
        let outcomes: Vec<Outcome> = earlier.iter().map(|m| m.outcome).collect();
        assert_eq!(outcomes, vec![Outcome::Win, Outcome::Loss]);
        assert!(earlier[0].started_at_ms < earlier[1].started_at_ms);

        let all_time = earlier_matches(
            &conn,
            Some(me),
            Platform::Mtgo,
            Range::all_time(),
            &FormatFilter::All,
        )
        .expect("earlier");
        assert!(all_time.is_empty(), "nothing is before all time");
    }

    #[test]
    fn earlier_matches_follow_the_account_platform_and_format() {
        let (_dir, conn) = db("dashboard-earlier-scope");
        let (me, _, version) = setup(&conn);
        let other = account(&conn, "Qother");
        played_in(
            &conn,
            me,
            Some(version),
            Some("CModern"),
            "win",
            NOW - 30 * DAY,
        );
        played_in(&conn, me, None, Some("DQNWQNWQNW"), "loss", NOW - 29 * DAY);
        played_in(&conn, other, None, Some("CModern"), "loss", NOW - 28 * DAY);
        let range = Range {
            from_ms: NOW - 10 * DAY,
            to_ms: NOW,
        };
        let read = |account, platform, format: &FormatFilter| {
            earlier_matches(&conn, account, platform, range, format)
                .expect("earlier")
                .len()
        };
        assert_eq!(read(Some(me), Platform::Mtgo, &FormatFilter::All), 2);
        assert_eq!(read(Some(me), Platform::Mtgo, &FormatFilter::Limited), 1);
        assert_eq!(
            read(Some(me), Platform::Mtgo, &FormatFilter::Constructed),
            1
        );
        assert_eq!(read(Some(me), Platform::Arena, &FormatFilter::All), 0);
        assert_eq!(read(None, Platform::Mtgo, &FormatFilter::All), 3);
    }

    #[test]
    fn earlier_matches_stop_at_the_trend_history() {
        let (_dir, conn) = db("dashboard-earlier-cap");
        let (me, _, version) = setup(&conn);
        for minutes_ago in 1..=(i64::from(TREND_HISTORY) + 5) {
            won(&conn, me, version, NOW - 30 * DAY - minutes_ago * MINUTE);
        }
        let range = Range {
            from_ms: NOW - 10 * DAY,
            to_ms: NOW,
        };
        let earlier = earlier_matches(&conn, Some(me), Platform::Mtgo, range, &FormatFilter::All)
            .expect("earlier");
        assert_eq!(earlier.len(), TREND_HISTORY as usize);
        assert_eq!(
            earlier.last().map(|m| m.started_at_ms),
            Some(NOW - 30 * DAY - MINUTE),
            "the newest earlier match is kept"
        );
    }

    #[test]
    fn an_earlier_match_moves_the_home_charts_first_day_trend() {
        let (_dir, conn) = db("dashboard-earlier-trend");
        let (me, _, version) = setup(&conn);
        lost(&conn, me, version, NOW - 30 * DAY);
        won(&conn, me, version, NOW - DAY);
        let range = Range {
            from_ms: NOW - 10 * DAY,
            to_ms: NOW,
        };
        let all = FormatFilter::All;
        let window = window_matches(&conn, Some(me), Platform::Mtgo, range, &all).expect("window");
        let played: Vec<PlayedMatch> = window
            .iter()
            .map(|m| PlayedMatch {
                started_at_ms: m.started_at,
                outcome: m.outcome,
            })
            .collect();
        let earlier =
            earlier_matches(&conn, Some(me), Platform::Mtgo, range, &all).expect("earlier");
        assert_eq!(daily_results(&[], &played, UTC)[0].trend, Some(100));
        // A loss then a win: 1 / (1 + 49/51) = 0.51.
        assert_eq!(daily_results(&earlier, &played, UTC)[0].trend, Some(51));
    }

    #[test]
    fn recent_sessions_are_the_last_seven_days_with_matches_newest_first() {
        let (_dir, conn) = db("dashboard-sessions");
        let (me, _, version) = setup(&conn);
        // Nine days with a match, two matches on the newest.
        for days_ago in [20, 18, 15, 12, 9, 7, 3, 1, 0] {
            won(&conn, me, version, NOW - days_ago * DAY - 2 * HOUR);
        }
        lost(&conn, me, version, NOW - HOUR);
        let sessions = recent_sessions(
            &conn,
            Some(me),
            Platform::Mtgo,
            &FormatFilter::All,
            NOW,
            UTC,
        )
        .expect("sessions");
        assert_eq!(sessions.len(), RECENT_SESSIONS);
        assert_eq!(sessions[0].days_ago, 0);
        assert_eq!(sessions[0].record, Record::from_counts(1, 1, 0));
        let ago: Vec<u32> = sessions.iter().map(|s| s.days_ago).collect();
        assert_eq!(ago, vec![0, 1, 3, 7, 9, 12, 15]);
        assert_eq!(sessions[0].day, "2026-10-07");
    }

    #[test]
    fn recent_sessions_read_past_a_busy_day_to_find_seven_days() {
        let (_dir, conn) = db("dashboard-sessions-busy");
        let (me, _, version) = setup(&conn);
        // 150 matches today (more than the first read), then one a day for
        // the six days before it and one a week before that.
        for n in 0..150 {
            outcome_at(&conn, me, version, "win", NOW - 6 * HOUR + n * MINUTE);
        }
        for days_ago in [1, 2, 3, 4, 5, 6, 14] {
            lost(&conn, me, version, NOW - days_ago * DAY);
        }
        let sessions = recent_sessions(
            &conn,
            Some(me),
            Platform::Mtgo,
            &FormatFilter::All,
            NOW,
            UTC,
        )
        .expect("sessions");
        assert_eq!(sessions.len(), RECENT_SESSIONS);
        assert_eq!(sessions[0].record.total, 150, "the busy day is whole");
        assert_eq!(sessions[6].days_ago, 6, "the day 14 back is the eighth");
    }

    #[test]
    fn recent_sessions_follow_the_account_platform_and_format() {
        let (_dir, conn) = db("dashboard-sessions-scope");
        let (me, _, version) = setup(&conn);
        let other = account(&conn, "Qother");
        played_in(&conn, me, Some(version), Some("CModern"), "win", NOW - HOUR);
        played_in(&conn, me, None, Some("DQNWQNWQNW"), "loss", NOW - HOUR);
        played_in(&conn, other, None, Some("CModern"), "loss", NOW - DAY);
        let read = |account, platform, format: &FormatFilter| {
            recent_sessions(&conn, account, platform, format, NOW, UTC).expect("sessions")
        };
        let mine = read(Some(me), Platform::Mtgo, &FormatFilter::All);
        assert_eq!(mine.len(), 1);
        assert_eq!(mine[0].record, Record::from_counts(1, 1, 0));
        let modern = read(Some(me), Platform::Mtgo, &FormatFilter::Constructed);
        assert_eq!(modern[0].record, Record::from_counts(1, 0, 0));
        assert!(read(Some(me), Platform::Arena, &FormatFilter::All).is_empty());
        assert_eq!(read(None, Platform::Mtgo, &FormatFilter::All).len(), 2);
    }

    // Last session (0.x GetLastSessionTest)

    #[test]
    fn no_account_gives_no_last_session() {
        let (_dir, conn) = db("dashboard-session-none");
        assert_eq!(
            last_session(&conn, None, &FormatFilter::All, UTC).expect("session"),
            None
        );
        let (me, _, _) = setup(&conn);
        assert_eq!(
            last_session(&conn, Some(me), &FormatFilter::All, UTC).expect("session"),
            None
        );
    }

    #[test]
    fn matches_within_two_hours_are_one_session() {
        let (_dir, conn) = db("dashboard-session-group");
        let (me, _, version) = setup(&conn);
        let mut first = spec(me, version, "win", NOW - 3 * HOUR);
        first.ended_at = Some(NOW - 3 * HOUR + 30 * MINUTE);
        let first = store(&conn, first);
        let mut second = spec(me, version, "win", NOW - 2 * HOUR - 20 * MINUTE);
        second.ended_at = Some(NOW - 2 * HOUR + 10 * MINUTE);
        store(&conn, second);
        let mut third = spec(me, version, "loss", NOW - HOUR - 40 * MINUTE);
        third.ended_at = Some(NOW - HOUR - 10 * MINUTE);
        let third = store(&conn, third);
        game(&conn, first.id, "win", None);
        game(&conn, first.id, "win", None);
        game(&conn, third.id, "loss", None);
        game(&conn, third.id, "loss", None);

        let session = last_session(&conn, Some(me), &FormatFilter::All, UTC)
            .expect("session")
            .expect("a session");
        assert_eq!(session.matches.len(), 3);
        assert_eq!(session.record, "2-1");
        assert_eq!(session.matches[0].token, first.token);
        assert_eq!(session.matches[0].games_won, 2);
        assert_eq!(session.matches[2].games_lost, 2);
        // 0.x GetLastSession: the opponent archetype's name, else "Unknown".
        assert_eq!(session.matches[0].opponent_archetype, "Unknown");
        conn.execute_batch(&format!(
            "INSERT INTO archetypes (id, uuid, name, format, source, created_at, updated_at)
                 VALUES (50, 'q-burn', 'Qburn', 'modern', 'downloaded', 0, 0);
             INSERT INTO match_archetypes (match_id, is_opponent, archetype_id, confidence)
                 VALUES ({}, 1, 50, 1.0), ({}, 0, 50, 1.0);",
            third.id, first.id
        ))
        .expect("archetype");
        let named = last_session(&conn, Some(me), &FormatFilter::All, UTC)
            .expect("session")
            .expect("a session");
        assert_eq!(
            named.matches[0].opponent_archetype, "Unknown",
            "player seat"
        );
        assert_eq!(named.matches[2].opponent_archetype, "Qburn");
        assert_eq!(session.started_at_ms, NOW - 3 * HOUR);
        assert_eq!(session.ended_at_ms, NOW - HOUR - 10 * MINUTE);
        assert_eq!(session.started_label, "Oct 7, 2026");
        assert_eq!(session.duration, "~1h 50m");
    }

    #[test]
    fn a_gap_over_two_hours_starts_a_new_session() {
        let (_dir, conn) = db("dashboard-session-split");
        let (me, _, version) = setup(&conn);
        let mut old = spec(me, version, "win", NOW - 6 * HOUR);
        old.ended_at = Some(NOW - 6 * HOUR + 30 * MINUTE);
        store(&conn, old);
        let mut latest = spec(me, version, "loss", NOW - HOUR);
        latest.ended_at = Some(NOW - 30 * MINUTE);
        store(&conn, latest);

        let session = last_session(&conn, Some(me), &FormatFilter::All, UTC)
            .expect("session")
            .expect("a session");
        assert_eq!(session.matches.len(), 1);
        assert_eq!(session.record, "0-1");
        assert_eq!(session.duration, "~30m");
    }

    #[test]
    fn a_match_without_an_end_is_taken_to_last_an_hour() {
        let (_dir, conn) = db("dashboard-session-no-end");
        let (me, _, version) = setup(&conn);
        store(&conn, spec(me, version, "win", NOW - 2 * HOUR));
        let mut latest = spec(me, version, "win", NOW - HOUR);
        latest.ended_at = Some(NOW - 30 * MINUTE);
        store(&conn, latest);

        let session = last_session(&conn, Some(me), &FormatFilter::All, UTC)
            .expect("session")
            .expect("a session");
        assert_eq!(session.matches.len(), 2);
    }

    #[test]
    fn the_session_gap_rule() {
        // (start, end) newest first.
        let gap = |earlier_end: i64| {
            session_length(&[(NOW, Some(NOW + HOUR)), (NOW - 4 * HOUR, Some(earlier_end))])
        };
        assert_eq!(gap(NOW - 2 * HOUR), 2, "exactly two hours is one session");
        assert_eq!(gap(NOW - 2 * HOUR - 1), 1, "a millisecond more is not");
        assert_eq!(gap(NOW + HOUR), 2, "an overlap is one session");
        assert_eq!(session_length(&[]), 0);
    }

    // Recent matches (0.x ExistingWidgetsTest)

    #[test]
    fn recent_matches_are_the_latest_five_in_the_window() {
        let (_dir, conn) = db("dashboard-recent");
        let (me, _, version) = setup(&conn);
        let played = won(&conn, me, version, NOW - HOUR);
        game(&conn, played.id, "win", None);
        lost(&conn, me, version, NOW - 2 * 365 * DAY);

        let recent = recent_matches(
            &conn,
            Some(me),
            Platform::Mtgo,
            monthly(),
            &FormatFilter::All,
        )
        .expect("recent");
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].row.token, played.token);
        assert_eq!(recent[0].row.wins, 1);
        assert_eq!(recent[0].deck_name.as_deref(), Some("Qtron"));

        for hours_ago in 2..=12 {
            won(&conn, me, version, NOW - hours_ago * HOUR);
        }
        let recent = recent_matches(
            &conn,
            Some(me),
            Platform::Mtgo,
            monthly(),
            &FormatFilter::All,
        )
        .expect("recent");
        assert_eq!(recent.len(), 5);
        assert_eq!(recent[0].row.token, played.token, "newest first");
    }

    #[test]
    fn dashboard_reads_one_platform() {
        let (_dir, conn) = db("dashboard-arena");
        let (me, _, version) = setup(&conn);
        let mtgo = played_in(&conn, me, Some(version), Some("CModern"), "win", NOW - HOUR);
        let arena = played_in(&conn, me, None, Some("CPIONEER"), "loss", NOW - HOUR);
        crate::test_support::move_match_to_arena(&conn, &arena.token);

        for (platform, token, code) in [
            (Platform::Mtgo, &mtgo.token, "CModern"),
            (Platform::Arena, &arena.token, "CPIONEER"),
        ] {
            let all = &FormatFilter::All;
            let everyone =
                overview(&conn, None, platform, Range::all_time(), None, all).expect("overview");
            assert_eq!(everyone.record.total, 1, "{platform:?}");
            let recent = recent_matches(&conn, None, platform, monthly(), all).expect("recent");
            let tokens: Vec<&str> = recent.iter().map(|m| m.row.token.as_str()).collect();
            assert_eq!(tokens, [token.as_str()], "{platform:?}");
            let window = window_matches(&conn, None, platform, monthly(), all).expect("window");
            assert_eq!(window.len(), 1, "{platform:?}");
            let codes: Vec<String> = format_options(&conn, None, platform)
                .expect("formats")
                .into_iter()
                .flat_map(|option| option.codes)
                .collect();
            assert_eq!(codes, [code], "{platform:?}");
            assert!(has_matches(&conn, None, platform).expect("has matches"));
        }
        assert_eq!(
            last_played_format(&conn, None, Platform::Arena).expect("last"),
            "pioneer"
        );
        let decks = recent_decks(&conn, None, Platform::Arena, &FormatFilter::All).expect("decks");
        assert!(decks.is_empty(), "the Arena match has no deck");
        let decks = recent_decks(&conn, None, Platform::Mtgo, &FormatFilter::All).expect("decks");
        assert_eq!(decks.len(), 1);
    }

    #[test]
    fn recent_matches_include_matches_without_a_deck() {
        let (_dir, conn) = db("dashboard-recent-deckless");
        let (me, _, version) = setup(&conn);
        let mut deckless = spec(me, version, "win", NOW - HOUR);
        deckless.version = None;
        store(&conn, deckless);

        let mine = recent_matches(
            &conn,
            Some(me),
            Platform::Mtgo,
            monthly(),
            &FormatFilter::All,
        )
        .expect("recent");
        assert_eq!(mine.len(), 1);
        assert_eq!(mine[0].deck_id, None);
        let everyone = recent_matches(&conn, None, Platform::Mtgo, monthly(), &FormatFilter::All)
            .expect("recent");
        assert_eq!(everyone.len(), 1);
    }

    // Gameless imports (0.x KpiStripWidget)

    /// Make a stored match a gameless history import with its history's
    /// game counts.
    fn gameless_import(conn: &Connection, played: &Played, won: i64, lost: i64) {
        conn.execute(
            "UPDATE matches SET source = 'imported', games_won_imported = ?2,
                 games_lost_imported = ?3 WHERE id = ?1",
            params![played.id, won, lost],
        )
        .expect("imported counts");
    }

    #[test]
    fn the_overview_counts_game_rows_only() {
        // 0.x `KpiStripWidget` counts game rows: a gameless history import's
        // game counts are not folded, nor is a match without games counted.
        let (_dir, conn) = db("dashboard-gameless-imports");
        let (me, _, version) = setup(&conn);
        let tracked = won(&conn, me, version, NOW - HOUR);
        game(&conn, tracked.id, "win", Some(true));
        let gameless = lost(&conn, me, version, NOW - 2 * HOUR);
        gameless_import(&conn, &gameless, 1, 2);

        let kpis = overview(
            &conn,
            Some(me),
            Platform::Mtgo,
            monthly(),
            None,
            &FormatFilter::All,
        )
        .expect("overview");
        assert_eq!(kpis.games, GameRecord::new(1, 0));
        assert_eq!(kpis.record, Record::from_counts(1, 0, 0));
    }

    // Hidden matches (Plan K, "Remove from stats")

    #[test]
    fn a_hidden_match_counts_in_no_dashboard_read() {
        let (_dir, conn) = db("dashboard-hidden");
        let (me, _, version) = setup(&conn);
        let mut kept = spec(me, version, "win", NOW - 3 * HOUR);
        kept.ended_at = Some(NOW - 3 * HOUR + 30 * MINUTE);
        let kept = store(&conn, kept);
        game(&conn, kept.id, "win", Some(true));
        let mut gone = spec(me, version, "loss", NOW - HOUR);
        gone.ended_at = Some(NOW - HOUR + 30 * MINUTE);
        let gone = store(&conn, gone);
        game(&conn, gone.id, "loss", Some(true));
        hide(&conn, &gone.token);

        let before = Range {
            from_ms: 0,
            to_ms: 1,
        };
        for account in [Some(me), None] {
            let overview = overview(
                &conn,
                account,
                Platform::Mtgo,
                monthly(),
                Some(before),
                &FormatFilter::All,
            )
            .expect("overview");
            assert_eq!(overview.record, Record::from_counts(1, 0, 0), "{account:?}");
            assert_eq!(overview.games, GameRecord::new(1, 0), "{account:?}");
            let recent = recent_matches(
                &conn,
                account,
                Platform::Mtgo,
                monthly(),
                &FormatFilter::All,
            )
            .expect("recent");
            let tokens: Vec<&str> = recent.iter().map(|m| m.row.token.as_str()).collect();
            assert_eq!(tokens, vec![kept.token.as_str()], "{account:?}");
            let window = window_matches(
                &conn,
                account,
                Platform::Mtgo,
                monthly(),
                &FormatFilter::All,
            )
            .expect("window");
            let tokens: Vec<&str> = window.iter().map(|m| m.token.as_str()).collect();
            assert_eq!(tokens, vec![kept.token.as_str()], "{account:?}");
        }
        let overview = overview(
            &conn,
            Some(me),
            Platform::Mtgo,
            monthly(),
            Some(before),
            &FormatFilter::All,
        )
        .expect("overview");
        assert_eq!(overview.streak.current.as_deref(), Some("1W"));
        assert_eq!(overview.streak.best_loss, 0);
        assert_eq!(overview.match_win_rate_delta, 100);
        assert_eq!(overview.play_draw.otp_win_rate, 100);

        let sessions = recent_sessions(
            &conn,
            Some(me),
            Platform::Mtgo,
            &FormatFilter::All,
            NOW,
            UTC,
        )
        .expect("sessions");
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].record, Record::from_counts(1, 0, 0));
        let session = last_session(&conn, Some(me), &FormatFilter::All, UTC)
            .expect("session")
            .expect("a session");
        assert_eq!(session.matches.len(), 1);
        assert_eq!(session.matches[0].token, kept.token);
    }

    #[test]
    fn has_matches_is_an_existence_check_of_complete_matches() {
        let (_dir, conn) = db("dashboard-has-matches");
        let (me, _, version) = setup(&conn);
        assert!(!has_matches(&conn, Some(me), Platform::Mtgo).expect("empty"));
        assert!(!has_matches(&conn, None, Platform::Mtgo).expect("empty"));
        won(&conn, me, version, NOW - 2 * 365 * DAY);
        assert!(has_matches(&conn, Some(me), Platform::Mtgo).expect("old match"));
        assert!(has_matches(&conn, None, Platform::Mtgo).expect("no account"));
        assert!(!has_matches(&conn, Some(me + 100), Platform::Mtgo).expect("other account"));
        let query = plan(&conn, &has_matches_sql());
        assert!(!query.contains("SCAN g"), "plan:\n{query}");
    }

    // Query plans

    /// The query plan of `sql`, every parameter bound to 1.
    fn plan(conn: &Connection, sql: &str) -> String {
        let mut stmt = conn
            .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
            .expect("explain");
        let params = vec![1i64; stmt.parameter_count()];
        let details: Vec<String> = stmt
            .query_map(rusqlite::params_from_iter(params), |row| {
                row.get::<_, String>(3)
            })
            .expect("rows")
            .map(|row| row.expect("row"))
            .collect();
        details.join("\n")
    }

    #[test]
    fn windowed_queries_read_matches_by_start_and_never_scan_games_or_links() {
        let (_dir, conn) = db("dashboard-plans-window");
        for (name, sql) in [
            ("overview games", window_games_sql()),
            ("window record", window_record_sql()),
            ("recent matches", recent_matches_sql()),
            ("window matches", window_matches_sql()),
            ("earlier matches", earlier_matches_sql()),
        ] {
            let query = plan(&conn, &sql);
            assert!(
                query.contains("matches_started_at"),
                "{name} plan:\n{query}"
            );
            assert!(!query.contains("SCAN m"), "{name} plan:\n{query}");
            assert!(!query.contains("SCAN g"), "{name} plan:\n{query}");
            assert!(!query.contains("SCAN l"), "{name} plan:\n{query}");
            assert!(!query.contains("SCAN v"), "{name} plan:\n{query}");
        }
    }

    // The new cards' reads (dashboard redesign spec 4.1, 4.3, 4.7, 4.10)

    #[test]
    fn window_matches_are_the_complete_matches_of_the_window_and_format() {
        let (_dir, conn) = db("dashboard-window-matches");
        let (me, _, version) = setup(&conn);
        let modern = played_in(
            &conn,
            me,
            Some(version),
            Some("CModern"),
            "win",
            NOW - 2 * HOUR,
        );
        let draft = played_in(&conn, me, None, Some("DQNWQNWQNW"), "loss", NOW - HOUR);
        played_in(
            &conn,
            me,
            Some(version),
            Some("CModern"),
            "win",
            NOW - 2 * 365 * DAY,
        );
        let mut ongoing = spec(me, version, "unknown", NOW - HOUR);
        ongoing.state = "in_progress";
        store(&conn, ongoing);
        let hidden = played_in(&conn, me, None, Some("CModern"), "win", NOW - HOUR);
        hide(&conn, &hidden.token);

        let all = window_matches(
            &conn,
            Some(me),
            Platform::Mtgo,
            monthly(),
            &FormatFilter::All,
        )
        .expect("window");
        let tokens: Vec<&str> = all.iter().map(|played| played.token.as_str()).collect();
        assert_eq!(
            tokens,
            vec![modern.token.as_str(), draft.token.as_str()],
            "oldest first"
        );
        assert_eq!(all[1].format.as_deref(), Some("DQNWQNWQNW"));
        assert_eq!(all[1].outcome, Outcome::Loss);
        assert_eq!(all[1].started_at, NOW - HOUR);
        let limited = window_matches(
            &conn,
            Some(me),
            Platform::Mtgo,
            monthly(),
            &FormatFilter::Limited,
        )
        .expect("window");
        assert_eq!(limited.len(), 1);
    }

    #[test]
    fn window_matches_name_the_players_deck_by_archetype_else_by_deck() {
        let (_dir, conn) = db("dashboard-window-deck-labels");
        let (me, tron_deck, tron_version) = setup(&conn);
        let burn_deck = deck(&conn, Some(me), "Qburn", "CModern");
        let burn_version = add_version(&conn, burn_deck, 0);
        let second_burn_version = add_version(&conn, burn_deck, 1);
        let burn = archetype(&conn, "Burn");
        conn.execute(
            "UPDATE decks SET archetype_id = ?1 WHERE id = ?2",
            params![burn, burn_deck],
        )
        .expect("set archetype");
        played_in(
            &conn,
            me,
            Some(tron_version),
            Some("CModern"),
            "win",
            NOW - 4 * HOUR,
        );
        played_in(
            &conn,
            me,
            Some(burn_version),
            Some("CModern"),
            "win",
            NOW - 3 * HOUR,
        );
        played_in(
            &conn,
            me,
            Some(second_burn_version),
            Some("CModern"),
            "loss",
            NOW - 2 * HOUR,
        );
        played_in(&conn, me, None, Some("DQNWQNWQNW"), "win", NOW - HOUR);

        let found = window_matches(
            &conn,
            Some(me),
            Platform::Mtgo,
            monthly(),
            &FormatFilter::All,
        )
        .expect("window");
        let labels: Vec<Option<DeckLabel>> = found.into_iter().map(|m| m.deck).collect();
        assert_eq!(
            labels,
            vec![
                Some(DeckLabel::deck(tron_deck, "Qtron")),
                Some(DeckLabel::archetype(burn, "Burn")),
                Some(DeckLabel::archetype(burn, "Burn")),
                None,
            ]
        );
    }

    #[test]
    fn the_matchup_share_counts_every_match_the_spread_could_count() {
        let (_dir, conn) = db("dashboard-matchup-total");
        let (me, _, version) = setup(&conn);
        for i in 0..7 {
            let opponent = archetype(&conn, &format!("Qarch {i}"));
            against(&conn, &won(&conn, me, version, NOW - DAY), opponent);
        }
        // No opponent archetype: neither read counts it.
        let unknown = won(&conn, me, version, NOW - DAY);
        game(&conn, unknown.id, "win", Some(true));
        let (range, _) = week();
        assert_eq!(
            matchup_spread(&conn, Some(me), range, &FormatFilter::All)
                .expect("spread")
                .rows
                .len(),
            5
        );
        let all = matchup_spread(&conn, Some(me), range, &FormatFilter::All).expect("spread");
        assert_eq!(all.total, 7);
        let limited =
            matchup_spread(&conn, Some(me), range, &FormatFilter::Limited).expect("spread");
        assert_eq!(limited.total, 0);
        let none = matchup_spread(&conn, None, range, &FormatFilter::All).expect("spread");
        assert_eq!(none.total, 0);
    }

    #[test]
    fn recent_decks_are_the_two_last_played_in_the_format_whatever_the_window() {
        let (_dir, conn) = db("dashboard-recent-decks");
        let (me, tron, tron_version) = setup(&conn);
        let burn_version = add_version(&conn, deck(&conn, Some(me), "Qburn", "CPauper"), 0);
        let elves_version = add_version(&conn, deck(&conn, Some(me), "Qelves", "CModern"), 0);
        let shelved = deck(&conn, Some(me), "Qshelved", "CModern");
        let shelved_version = add_version(&conn, shelved, 0);
        conn.execute("UPDATE decks SET archived_at = 1 WHERE id = ?1", [shelved])
            .expect("archive");
        played_in(
            &conn,
            me,
            Some(tron_version),
            Some("CModern"),
            "win",
            NOW - 3 * 365 * DAY,
        );
        played_in(
            &conn,
            me,
            Some(tron_version),
            Some("CModern"),
            "loss",
            NOW - 2 * 365 * DAY,
        );
        played_in(
            &conn,
            me,
            Some(burn_version),
            Some("CPauper"),
            "win",
            NOW - DAY,
        );
        played_in(
            &conn,
            me,
            Some(elves_version),
            Some("CModern"),
            "win",
            NOW - 2 * DAY,
        );
        played_in(
            &conn,
            me,
            Some(shelved_version),
            Some("CModern"),
            "win",
            NOW - HOUR,
        );

        let names = |found: &[RecentDeck]| -> Vec<String> {
            found
                .iter()
                .map(|recent| recent.deck.name.clone())
                .collect()
        };
        let all = recent_decks(&conn, Some(me), Platform::Mtgo, &FormatFilter::All).expect("decks");
        assert_eq!(
            names(&all),
            vec!["Qburn", "Qelves"],
            "archived decks are left out"
        );
        let options = format_options(&conn, Some(me), Platform::Mtgo).expect("options");
        let modern = recent_decks(
            &conn,
            Some(me),
            Platform::Mtgo,
            &resolve_format("modern", &options).expect("offered").1,
        )
        .expect("decks");
        assert_eq!(names(&modern), vec!["Qelves", "Qtron"]);
        let numbers = &modern[1];
        assert_eq!(numbers.deck.id, tron);
        assert_eq!(
            numbers.deck.record,
            Record::from_counts(1, 1, 0),
            "all time"
        );
        assert_eq!(numbers.games, GameRecord::new(1, 1));
        assert_eq!(
            numbers.sparkline,
            vec![100.0, 50.0],
            "cumulative, oldest first"
        );
        assert!(
            recent_decks(&conn, Some(me), Platform::Mtgo, &FormatFilter::Limited)
                .expect("decks")
                .is_empty()
        );
    }

    #[test]
    fn all_time_queries_never_scan_games_or_links() {
        let (_dir, conn) = db("dashboard-plans-account");
        for (name, sql) in [
            ("outcomes", account_outcomes_sql()),
            ("latest", latest_matches_sql()),
            ("latest outcomes", latest_outcomes_sql()),
            ("played formats", played_formats_sql()),
        ] {
            let query = plan(&conn, &sql);
            assert!(!query.contains("SCAN g"), "{name} plan:\n{query}");
            assert!(!query.contains("SCAN l"), "{name} plan:\n{query}");
        }
    }

    // ---- Top matchups (0.x GetDashboardMatchupSpreadTest) ----

    fn archetype(conn: &Connection, name: &str) -> i64 {
        conn.query_row(
            "INSERT INTO archetypes (uuid, name, format, color_identity, source, created_at,
                 updated_at)
             VALUES ('q-' || ?1, ?1, 'modern', 'g,r', 'manual', 0, 0) RETURNING id",
            [name],
            |row| row.get(0),
        )
        .expect("archetype")
    }

    fn seat(conn: &Connection, match_id: i64, is_opponent: bool, archetype_id: i64) {
        conn.execute(
            "INSERT INTO match_archetypes (match_id, is_opponent, archetype_id, confidence)
             VALUES (?1, ?2, ?3, 1.0)",
            params![match_id, is_opponent, archetype_id],
        )
        .expect("seat");
    }

    /// A match against `opponent` with one game.
    fn against(conn: &Connection, played: &Played, opponent: i64) {
        seat(conn, played.id, true, opponent);
        let result = match played.outcome.as_str() {
            "win" => "win",
            "loss" => "loss",
            _ => "draw",
        };
        game(conn, played.id, result, Some(true));
    }

    #[test]
    fn returns_empty_array_when_no_matches() {
        let (_dir, conn) = db("dashboard-spread-empty");
        let (me, _, version) = setup(&conn);
        let (range, _) = week();
        assert!(
            matchup_spread(&conn, Some(me), range, &FormatFilter::All)
                .expect("spread")
                .rows
                .is_empty()
        );
        // 0.x: without an account there is no spread.
        let burn = archetype(&conn, "Qburn");
        against(&conn, &won(&conn, me, version, NOW - DAY), burn);
        assert!(
            matchup_spread(&conn, None, range, &FormatFilter::All)
                .expect("spread")
                .rows
                .is_empty()
        );
    }

    /// 0.x "returns top 5 opponent archetypes by match count": a win and a
    /// draw against Burn are two matches played, 50%. The player's own
    /// seat, another account's match, a match outside the window, a hidden
    /// one and one without games are left out.
    #[test]
    fn returns_top_5_opponent_archetypes_by_match_count() {
        let (_dir, conn) = db("dashboard-spread-burn");
        let (me, _, version) = setup(&conn);
        let them = account(&conn, "Qthem");
        let their_version = add_version(&conn, deck(&conn, Some(them), "Qother", "CModern"), 0);
        let burn = archetype(&conn, "Qburn");
        let own = archetype(&conn, "Qown");
        let elsewhere = archetype(&conn, "Qelsewhere");

        let win = won(&conn, me, version, NOW - DAY);
        against(&conn, &win, burn);
        seat(&conn, win.id, false, own);
        let draw = outcome_at(&conn, me, version, "draw", NOW - DAY);
        against(&conn, &draw, burn);
        against(
            &conn,
            &won(&conn, them, their_version, NOW - DAY),
            elsewhere,
        );
        against(&conn, &lost(&conn, me, version, NOW - 30 * DAY), elsewhere);
        let hidden = lost(&conn, me, version, NOW - DAY);
        against(&conn, &hidden, elsewhere);
        hide(&conn, &hidden.token);
        let gameless = lost(&conn, me, version, NOW - DAY);
        seat(&conn, gameless.id, true, elsewhere);

        let (range, _) = week();
        let spread = matchup_spread(&conn, Some(me), range, &FormatFilter::All)
            .expect("spread")
            .rows;
        assert_eq!(
            spread,
            vec![MatchupSpreadRow {
                archetype_id: burn,
                name: "Qburn".to_string(),
                record: Record::from_counts(1, 0, 1),
            }]
        );
        assert_eq!(spread[0].record.win_rate, 50);
        assert_eq!(spread[0].record.total, 2);
    }

    #[test]
    fn limits_to_top_5_results() {
        let (_dir, conn) = db("dashboard-spread-five");
        let (me, _, version) = setup(&conn);
        for i in 0..7 {
            let opponent = archetype(&conn, &format!("Archetype {i}"));
            // Archetype 6 is faced three times, 5 twice: they lead.
            let times = match i {
                6 => 3,
                5 => 2,
                _ => 1,
            };
            for _ in 0..times {
                against(&conn, &won(&conn, me, version, NOW - DAY), opponent);
            }
        }
        let (range, _) = week();
        let spread = matchup_spread(&conn, Some(me), range, &FormatFilter::All)
            .expect("spread")
            .rows;
        assert_eq!(spread.len(), MATCHUP_SPREAD_LIMIT as usize);
        let names: Vec<&str> = spread.iter().map(|row| row.name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "Archetype 6",
                "Archetype 5",
                "Archetype 0",
                "Archetype 1",
                "Archetype 2"
            ]
        );
    }
}
