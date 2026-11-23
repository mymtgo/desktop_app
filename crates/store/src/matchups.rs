//! Deck matchups (archetypes spec sections 8 and 12): a deck's results
//! against each opponent archetype (0.x `GetArchetypeMatchupSpread`), one
//! archetype in detail (0.x `GetArchetypeMatchupDetail`), the best and worst
//! of them (0.x `BuildDeckArchetypeHeader`, `KpiBestWorstMatchups`), and the
//! peer archetype chart of the deck dashboard (0.x `GetPeerArchetypeChartData`).
//!
//! Counting rules as 0.x: listed complete matches linked to the deck
//! versions given, the opponent seat's archetype row (`is_opponent = 1`),
//! games counted when their result is a win or a loss.

use std::collections::BTreeMap;

use chrono::TimeZone;
use chrono_tz::Tz;
use decks::php_round;
use decks::stats::{GameRecord, PlayedMatch, Range, Record, TREND_HISTORY, daily_results};
use rusqlite::{Connection, params};

use crate::StoreError;
use crate::leagues::SHOWN_IN_A_LEAGUE;
use crate::names::{game_won, stored_outcome};
use crate::platform::{BestOf, Platform, deck_of_platform};

/// Fewer matches than this and a matchup is not called best or worst (0.x
/// `BuildDeckArchetypeHeader::MIN_MATCHUP_MATCHES`, `KpiBestWorstMatchups`).
pub const MIN_MATCHUP_MATCHES: u32 = 3;

/// One opponent archetype of a deck's Matchups (0.x
/// `GetArchetypeMatchupSpread` row).
#[derive(Debug, Clone, PartialEq)]
pub struct MatchupRow {
    pub archetype_id: i64,
    pub name: String,
    /// Canonical comma form (`W,B,R`), 0.x `ColorIdentity::normalize`.
    pub color_identity: Option<String>,
    /// The matches against it, draws counted as played.
    pub record: Record,
    /// Games won and lost; the rate is 0.x `game_winrate`.
    pub games: GameRecord,
    /// Games with a result (0.x `total_games`).
    pub total_games: u32,
    /// Games won on the play over games on the play, whole percent (0
    /// without any).
    pub otp_win_rate: u32,
    /// Turns per game over games that know them, one decimal.
    pub avg_turns: Option<f64>,
}

/// Version ids as SQLite's JSON array text, for `json_each`.
pub(crate) fn ids_json(ids: &[i64]) -> String {
    let parts: Vec<String> = ids.iter().map(i64::to_string).collect();
    format!("[{}]", parts.join(","))
}

/// The versions a deck page reads: the one asked for when it exists (0.x
/// `DeckVersion::find`), else every version of the deck.
pub fn deck_version_ids(
    conn: &Connection,
    deck_id: i64,
    version: Option<i64>,
) -> Result<Vec<i64>, StoreError> {
    if let Some(id) = crate::deck_pages::existing_version(conn, version)? {
        return Ok(vec![id]);
    }
    let mut stmt = conn.prepare_cached("SELECT id FROM deck_versions WHERE deck_id = ?1")?;
    let rows = stmt.query_map([deck_id], |row| row.get(0))?;
    let mut ids = Vec::new();
    for row in rows {
        ids.push(row?);
    }
    Ok(ids)
}

/// Per opponent archetype: the matches (their outcomes) and their games
/// with a result, of the listed complete matches of versions `?1` started
/// from `?2` to `?3`. A match without game rows is left out (0.x joins its
/// game numbers).
const SPREAD: &str = "
WITH pairs AS (
    SELECT a.id AS archetype_id, a.name AS name, a.color_identity AS colors,
           m.id AS match_id, m.outcome AS outcome
    FROM deck_versions v
    JOIN listed_matches m ON m.deck_version_id = v.id
    JOIN match_archetypes ma ON ma.match_id = m.id AND ma.is_opponent = 1
    JOIN archetypes a ON a.id = ma.archetype_id
    WHERE v.id IN (SELECT value FROM json_each(?1))
      AND m.state = 'complete' AND m.started_at BETWEEN ?2 AND ?3
      AND (?4 IS NULL OR m.game_structure = ?4)),
gs AS (
    SELECT g.match_id AS match_id,
           SUM(g.result = 'win') AS won, SUM(g.result = 'loss') AS lost,
           SUM(g.result IN ('win', 'loss')) AS decided,
           SUM(g.on_play IS 1 AND g.result = 'win') AS otp_won,
           SUM(g.on_play IS 1 AND g.result IN ('win', 'loss')) AS otp_decided,
           SUM(COALESCE(g.turn_count, 0)) AS turns,
           SUM(g.turn_count IS NOT NULL) AS with_turns
    FROM games g
    WHERE g.match_id IN (SELECT match_id FROM pairs)
    GROUP BY g.match_id)
SELECT p.archetype_id, p.name, p.colors,
       SUM(p.outcome = 'win'), SUM(p.outcome = 'loss'), COUNT(*),
       SUM(gs.won), SUM(gs.lost), SUM(gs.decided),
       SUM(gs.otp_won), SUM(gs.otp_decided), SUM(gs.turns), SUM(gs.with_turns)
FROM pairs p JOIN gs ON gs.match_id = p.match_id
GROUP BY p.archetype_id";

/// A deck's results against each opponent archetype (0.x
/// `GetArchetypeMatchupSpread::forVersionIds`): by game win rate, best
/// first (then by name, so ties keep one order). With `best_of`, only the
/// matches of that Arena kind count.
pub fn matchup_spread(
    conn: &Connection,
    version_ids: &[i64],
    range: Range,
    best_of: Option<BestOf>,
) -> Result<Vec<MatchupRow>, StoreError> {
    if version_ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut stmt = conn.prepare_cached(SPREAD)?;
    let rows = stmt.query_map(
        params![ids_json(version_ids), range.from_ms, range.to_ms, best_of],
        |row| {
            let colors: Option<String> = row.get(2)?;
            let turns: i64 = row.get(11)?;
            let with_turns: i64 = row.get(12)?;
            let otp_won: u32 = row.get(9)?;
            let otp_decided: u32 = row.get(10)?;
            Ok(MatchupRow {
                archetype_id: row.get(0)?,
                name: row.get(1)?,
                color_identity: archetypes::normalize_colors(colors.as_deref()),
                record: Record::from_total(row.get(3)?, row.get(4)?, row.get(5)?),
                games: GameRecord::new(row.get(6)?, row.get(7)?),
                total_games: row.get(8)?,
                otp_win_rate: decks::stats::winrate_percent(
                    otp_won,
                    otp_decided.saturating_sub(otp_won),
                    0,
                ),
                avg_turns: if with_turns > 0 {
                    Some(php_round(turns as f64 / with_turns as f64, 1))
                } else {
                    None
                },
            })
        },
    )?;
    let mut spread = Vec::new();
    for row in rows {
        spread.push(row?);
    }
    spread.sort_by(|a, b| {
        b.games
            .win_rate
            .cmp(&a.games.win_rate)
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.archetype_id.cmp(&b.archetype_id))
    });
    Ok(spread)
}

/// The best and worst matchups (0.x `BuildDeckArchetypeHeader::matchups`):
/// among archetypes faced `MIN_MATCHUP_MATCHES` times or more, the highest
/// match win rate and the lowest, ties to the most matches. Fewer than two
/// such archetypes name neither.
pub fn best_and_worst(rows: &[MatchupRow]) -> (Option<MatchupRow>, Option<MatchupRow>) {
    let qualifying: Vec<&MatchupRow> = rows
        .iter()
        .filter(|row| row.record.total >= MIN_MATCHUP_MATCHES)
        .collect();
    if qualifying.len() < 2 {
        return (None, None);
    }
    let mut best = qualifying.clone();
    best.sort_by(|a, b| {
        b.record
            .win_rate
            .cmp(&a.record.win_rate)
            .then_with(|| b.record.total.cmp(&a.record.total))
    });
    let mut worst = qualifying;
    worst.sort_by(|a, b| {
        a.record
            .win_rate
            .cmp(&b.record.win_rate)
            .then_with(|| b.record.total.cmp(&a.record.total))
    });
    (
        best.first().map(|row| (*row).clone()),
        worst.first().map(|row| (*row).clone()),
    )
}

/// One game number's games (0.x `perGameWinrates` row).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PerGame {
    /// 1 to 3.
    pub game_number: u32,
    pub games: GameRecord,
}

/// One match of the drawer's history (0.x `matchHistory` row).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryMatch {
    pub token: String,
    pub started_at_ms: i64,
    /// The local day as `Mar 5` (0.x `M j`).
    pub date_label: String,
    pub is_league: bool,
    pub opponent: Option<String>,
    /// Games won and lost, `2-1`.
    pub score: String,
    /// `win`, `loss`, `draw` or `unknown`.
    pub outcome: String,
    /// Each game in order: won, lost, or neither (`None`).
    pub game_results: Vec<Option<bool>>,
}

/// The Matchups drawer for one archetype (0.x `GetArchetypeMatchupDetail`;
/// its best and worst card lists were always empty and are left out). The
/// default is 0.x's empty result.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MatchupDetail {
    pub record: Record,
    pub games: GameRecord,
    /// Games 1 to 3 that were played.
    pub per_game: Vec<PerGame>,
    /// Games on the play.
    pub otp: GameRecord,
    /// Games on the draw.
    pub otd: GameRecord,
    /// One decimal.
    pub avg_turns: Option<f64>,
    /// The local player's mulligans per game, one decimal.
    pub avg_mulligans: Option<f64>,
    /// Games on the play over games that know the side, whole percent.
    pub on_play_rate: u32,
    /// Newest first.
    pub history: Vec<HistoryMatch>,
}

/// The listed complete matches of versions `?1` started from `?2` to `?3`
/// whose opponent seat is archetype `?4`, newest first.
fn detail_matches_sql() -> String {
    format!(
        "
SELECT m.id, m.token, m.started_at, m.outcome,
       {SHOWN_IN_A_LEAGUE}, COALESCE(o.display_name, o.username)
FROM deck_versions v
JOIN listed_matches m ON m.deck_version_id = v.id
JOIN match_archetypes ma ON ma.match_id = m.id AND ma.is_opponent = 1
LEFT JOIN opponents o ON o.id = m.opponent_id
WHERE v.id IN (SELECT value FROM json_each(?1))
  AND m.state = 'complete' AND m.started_at BETWEEN ?2 AND ?3
  AND ma.archetype_id = ?4
  AND (?5 IS NULL OR m.game_structure = ?5)
ORDER BY m.started_at DESC, m.id DESC"
    )
}

/// The games of matches `?1` (a JSON array), in play order per match.
const DETAIL_GAMES: &str = "
SELECT g.match_id, g.result, g.on_play, g.turn_count, g.local_mulligans
FROM games g
WHERE g.match_id IN (SELECT value FROM json_each(?1))
ORDER BY g.match_id, g.started_at, g.id";

struct DetailGame {
    match_id: i64,
    result: String,
    on_play: Option<bool>,
    turns: Option<i64>,
    mulligans: Option<i64>,
}

/// The average of the known values, one decimal; `None` without any.
fn average(values: &[i64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let sum: i64 = values.iter().sum();
    Some(php_round(sum as f64 / values.len() as f64, 1))
}

/// One archetype's matchup against versions `version_ids` in `range`, dates
/// in `tz` (0.x `GetArchetypeMatchupDetail::run`).
pub fn matchup_detail(
    conn: &Connection,
    version_ids: &[i64],
    archetype_id: i64,
    range: Range,
    tz: Tz,
    best_of: Option<BestOf>,
) -> Result<MatchupDetail, StoreError> {
    let mut stmt = conn.prepare_cached(&detail_matches_sql())?;
    let rows = stmt.query_map(
        params![
            ids_json(version_ids),
            range.from_ms,
            range.to_ms,
            archetype_id,
            best_of
        ],
        |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, bool>(4)?,
                row.get::<_, Option<String>>(5)?,
            ))
        },
    )?;
    let mut matches = Vec::new();
    for row in rows {
        matches.push(row?);
    }
    if matches.is_empty() {
        return Ok(MatchupDetail::default());
    }

    let ids: Vec<i64> = matches.iter().map(|found| found.0).collect();
    let mut stmt = conn.prepare_cached(DETAIL_GAMES)?;
    let rows = stmt.query_map([ids_json(&ids)], |row| {
        Ok(DetailGame {
            match_id: row.get(0)?,
            result: row.get(1)?,
            on_play: row.get(2)?,
            turns: row.get(3)?,
            mulligans: row.get(4)?,
        })
    })?;
    let mut by_match: BTreeMap<i64, Vec<DetailGame>> = BTreeMap::new();
    for row in rows {
        let game = row?;
        by_match.entry(game.match_id).or_default().push(game);
    }

    let mut wins = 0;
    let mut losses = 0;
    for found in &matches {
        match found.3.as_str() {
            "win" => wins += 1,
            "loss" => losses += 1,
            _ => {}
        }
    }
    let record = Record::from_total(wins, losses, matches.len() as u32);

    let mut games = GameRecord::default();
    let mut otp = GameRecord::default();
    let mut otd = GameRecord::default();
    let mut per_game: BTreeMap<u32, GameRecord> = BTreeMap::new();
    let mut turns = Vec::new();
    let mut mulligans = Vec::new();
    let (mut on_play, mut on_draw) = (0, 0);
    for list in by_match.values() {
        // Games with a result, numbered in play order (0.x ROW_NUMBER over
        // games whose `won` is known).
        let mut number = 0;
        for game in list {
            let decided = match game.result.as_str() {
                "win" => Some(GameRecord::new(1, 0)),
                "loss" => Some(GameRecord::new(0, 1)),
                _ => None,
            };
            if let Some(one) = decided {
                games = games.plus(one);
                match game.on_play {
                    Some(true) => otp = otp.plus(one),
                    Some(false) => otd = otd.plus(one),
                    None => {}
                }
                number += 1;
                if number <= 3 {
                    let entry = per_game.entry(number).or_default();
                    *entry = entry.plus(one);
                }
            }
            match game.on_play {
                Some(true) => on_play += 1,
                Some(false) => on_draw += 1,
                None => {}
            }
            if let Some(count) = game.turns {
                turns.push(count);
            }
            if let Some(count) = game.mulligans {
                mulligans.push(count);
            }
        }
    }

    let mut history = Vec::new();
    for (id, token, started_at_ms, outcome, is_league, opponent) in matches {
        let list = by_match.get(&id).map(Vec::as_slice).unwrap_or(&[]);
        let won = list.iter().filter(|game| game.result == "win").count();
        let lost = list.iter().filter(|game| game.result == "loss").count();
        history.push(HistoryMatch {
            token,
            started_at_ms,
            date_label: day_label(started_at_ms, tz),
            is_league,
            opponent,
            score: format!("{won}-{lost}"),
            outcome,
            game_results: list.iter().map(|game| game_won(&game.result)).collect(),
        });
    }

    Ok(MatchupDetail {
        record,
        games,
        per_game: per_game
            .into_iter()
            .map(|(game_number, games)| PerGame { game_number, games })
            .collect(),
        otp,
        otd,
        avg_turns: average(&turns),
        avg_mulligans: average(&mulligans),
        on_play_rate: decks::stats::winrate_percent(on_play, on_draw, 0),
        history,
    })
}

/// `Mar 5` for the local day of `ms` in `tz` (0.x `M j`).
fn day_label(ms: i64, tz: Tz) -> String {
    match tz.timestamp_millis_opt(ms).single() {
        Some(local) => local.format("%b %-d").to_string(),
        None => String::new(),
    }
}

/// One local day of the peer chart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerDay {
    /// `YYYY-MM-DD`.
    pub date: String,
    pub record: Record,
    /// The peers' win rate trend after the day's last match, from their
    /// matches in time order (those before the range feed it too).
    pub trend: Option<u32>,
}

/// The other decks of a deck's archetype, day by day (0.x
/// `GetPeerArchetypeChartData`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerChart {
    pub archetype_name: String,
    /// The other decks of the archetype.
    pub deck_count: u32,
    /// Days with a match, oldest first.
    pub days: Vec<PeerDay>,
}

/// The peer chart of deck `deck_id`: the account's other decks (archived
/// too; every one of the platform without an account) with the same
/// archetype, their listed complete matches in `range`
/// by local day in `tz`. `None` without an archetype, without other decks or
/// without their matches in `range`. With `best_of`, only the peers'
/// matches of that Arena kind count.
pub fn peer_chart(
    conn: &Connection,
    deck_id: i64,
    account_id: Option<i64>,
    platform: Platform,
    range: Range,
    tz: Tz,
    best_of: Option<BestOf>,
) -> Result<Option<PeerChart>, StoreError> {
    let archetype: Option<(i64, String)> = {
        use rusqlite::OptionalExtension;
        conn.prepare_cached(
            "SELECT a.id, a.name FROM decks d JOIN archetypes a ON a.id = d.archetype_id
             WHERE d.id = ?1",
        )?
        .query_row([deck_id], |row| Ok((row.get(0)?, row.get(1)?)))
        .optional()?
    };
    let Some((archetype_id, archetype_name)) = archetype else {
        return Ok(None);
    };
    let deck_count: u32 = conn
        .prepare_cached(&format!(
            "SELECT COUNT(*) FROM decks
             WHERE archetype_id = ?1 AND id <> ?2 AND (?3 IS NULL OR account_id = ?3)
               AND {of_platform}",
            of_platform = deck_of_platform("decks", 4)
        ))?
        .query_row(
            params![archetype_id, deck_id, account_id, platform],
            |row| row.get(0),
        )?;
    if deck_count == 0 {
        return Ok(None);
    }
    let mut stmt = conn.prepare_cached(&peer_matches_sql(false))?;
    let rows = stmt.query_map(
        params![
            archetype_id,
            deck_id,
            account_id,
            range.from_ms,
            range.to_ms,
            platform,
            best_of
        ],
        peer_match,
    )?;
    let mut in_range = Vec::new();
    for row in rows {
        in_range.push(row?);
    }
    if in_range.is_empty() {
        return Ok(None);
    }
    let mut stmt = conn.prepare_cached(&peer_matches_sql(true))?;
    let rows = stmt.query_map(
        params![
            archetype_id,
            deck_id,
            account_id,
            range.from_ms,
            TREND_HISTORY,
            platform,
            best_of
        ],
        peer_match,
    )?;
    let mut earlier = Vec::new();
    for row in rows {
        earlier.push(row?);
    }
    earlier.reverse();
    let days = daily_results(&earlier, &in_range, tz)
        .into_iter()
        .filter(|day| day.win_rate.is_some())
        .map(|day| PeerDay {
            date: day.day,
            record: Record::from_counts(day.wins, day.losses, day.draws),
            trend: day.trend,
        })
        .collect();
    Ok(Some(PeerChart {
        archetype_name,
        deck_count,
        days,
    }))
}

fn peer_match(row: &rusqlite::Row) -> rusqlite::Result<PlayedMatch> {
    Ok(PlayedMatch {
        started_at_ms: row.get(0)?,
        outcome: stored_outcome(&row.get::<_, String>(1)?),
    })
}

/// The start and outcome of the peers' listed complete matches (the
/// parameters of `peer_chart`): those from `?4` to `?5`, or with `earlier`
/// the latest `?5` before `?4`, newest first.
fn peer_matches_sql(earlier: bool) -> String {
    let (when, tail) = if earlier {
        (
            "m.started_at < ?4",
            "ORDER BY m.started_at DESC, m.id DESC LIMIT ?5",
        )
    } else {
        ("m.started_at BETWEEN ?4 AND ?5", "")
    };
    format!(
        "SELECT m.started_at, m.outcome
         FROM decks d
         JOIN deck_versions v ON v.deck_id = d.id
         JOIN listed_matches m ON m.deck_version_id = v.id
         WHERE d.archetype_id = ?1 AND d.id <> ?2 AND (?3 IS NULL OR d.account_id = ?3)
           AND {of_platform}
           AND m.state = 'complete' AND {when}
           AND (?7 IS NULL OR m.game_structure = ?7)
         {tail}",
        of_platform = deck_of_platform("d", 6)
    )
}

#[cfg(test)]
pub(crate) mod seed {
    //! Rows for the archetype page tests: accounts, decks, versions, matches
    //! with games, archetypes and seats. Fake names start with Q.

    use rusqlite::{Connection, OptionalExtension, params};

    pub use crate::test_support::db;
    use crate::test_support::number as count;

    /// The account named `name`, made when missing.
    pub fn account(conn: &Connection, name: &str) -> i64 {
        let found: Option<i64> = conn
            .query_row(
                "SELECT id FROM accounts WHERE username = ?1",
                [name],
                |row| row.get(0),
            )
            .optional()
            .expect("account lookup");
        match found {
            Some(id) => id,
            None => conn
                .query_row(
                    "INSERT INTO accounts (username, first_seen_at) VALUES (?1, 0) RETURNING id",
                    [name],
                    |row| row.get(0),
                )
                .expect("account"),
        }
    }

    /// A deck of `format` (an MTGO code) for `account` with `archetype`.
    pub fn deck(
        conn: &Connection,
        account_id: Option<i64>,
        format: &str,
        archetype: Option<i64>,
    ) -> i64 {
        let n = count(conn, "SELECT COUNT(*) FROM decks") + 1;
        conn.query_row(
            "INSERT INTO decks (mtgo_id, account_id, name, format, created_at, updated_at, source,
                                archetype_id)
             VALUES (?1, ?2, ?3, ?4, 0, 0, 'file', ?5) RETURNING id",
            params![
                format!("deck-{n}"),
                account_id,
                format!("Qdeck {n}"),
                format,
                archetype
            ],
            |row| row.get(0),
        )
        .expect("deck")
    }

    pub fn archive(conn: &Connection, deck_id: i64) {
        conn.execute("UPDATE decks SET archived_at = 5 WHERE id = ?1", [deck_id])
            .expect("archive");
    }

    /// A version of the deck, newer than the ones before it.
    pub fn version(conn: &Connection, deck_id: i64) -> i64 {
        let n = count(conn, "SELECT COUNT(*) FROM deck_versions") + 1;
        conn.query_row(
            "INSERT INTO deck_versions (deck_id, signature, modified_at) VALUES (?1, ?2, ?3)
             RETURNING id",
            params![deck_id, format!("SIG-{n}"), n * 1000],
            |row| row.get(0),
        )
        .expect("version")
    }

    /// A manual archetype of `format` (an archetypes key such as `modern`).
    pub fn archetype(conn: &Connection, name: &str, format: &str, colors: Option<&str>) -> i64 {
        let n = count(conn, "SELECT COUNT(*) FROM archetypes") + 1;
        conn.query_row(
            "INSERT INTO archetypes (uuid, name, format, color_identity, source, created_at,
                                     updated_at)
             VALUES (?1, ?2, ?3, ?4, 'manual', 0, 0) RETURNING id",
            params![format!("qqqqqqqq-{n:04}"), name, format, colors],
            |row| row.get(0),
        )
        .expect("archetype")
    }

    pub fn homebrew(conn: &Connection) -> i64 {
        count(conn, "SELECT id FROM archetypes WHERE name = 'Homebrew'")
    }

    pub fn uuid_of(conn: &Connection, archetype_id: i64) -> String {
        conn.query_row(
            "SELECT uuid FROM archetypes WHERE id = ?1",
            [archetype_id],
            |row| row.get(0),
        )
        .expect("uuid")
    }

    /// A match linked to `version` (by the deck's account, or Qlocal).
    pub fn played(
        conn: &Connection,
        version: i64,
        state: &str,
        outcome: &str,
        started_at: i64,
    ) -> i64 {
        let n = count(conn, "SELECT COUNT(*) FROM matches") + 1;
        let deck_account: Option<i64> = conn
            .query_row(
                "SELECT d.account_id FROM deck_versions v JOIN decks d ON d.id = v.deck_id
                 WHERE v.id = ?1",
                [version],
                |row| row.get(0),
            )
            .expect("version's deck");
        let account_id = deck_account.unwrap_or_else(|| account(conn, "Qlocal"));
        conn.query_row(
            "INSERT INTO matches (token, account_id, source, format, state, outcome, started_at,
                 last_activity_at, revision, derived_version, deck_version_id)
             VALUES (?1, ?2, 'log', 'CMODERN', ?3, ?4, ?5, ?5, 1, 1, ?6) RETURNING id",
            params![
                format!("tok-{n}"),
                account_id,
                state,
                outcome,
                started_at,
                version
            ],
            |row| row.get(0),
        )
        .expect("match")
    }

    /// A complete match.
    pub fn complete(conn: &Connection, version: i64, outcome: &str, started_at: i64) -> i64 {
        played(conn, version, "complete", outcome, started_at)
    }

    pub fn token_of(conn: &Connection, match_id: i64) -> String {
        conn.query_row(
            "SELECT token FROM matches WHERE id = ?1",
            [match_id],
            |row| row.get(0),
        )
        .expect("token")
    }

    /// The match's opponent, by name.
    pub fn opponent(conn: &Connection, match_id: i64, name: &str) {
        conn.execute(
            "INSERT OR IGNORE INTO opponents (username, first_seen_at) VALUES (?1, 0)",
            [name],
        )
        .expect("opponent");
        conn.execute(
            "UPDATE matches SET opponent_id = (SELECT id FROM opponents WHERE username = ?2)
             WHERE id = ?1",
            params![match_id, name],
        )
        .expect("set opponent");
    }

    /// The opponent seat of the match is `archetype` (an auto row).
    pub fn opponent_seat(conn: &Connection, match_id: i64, archetype: i64) {
        conn.execute(
            "INSERT INTO match_archetypes (match_id, is_opponent, archetype_id, confidence, manual)
             VALUES (?1, 1, ?2, 1.0, 0)",
            params![match_id, archetype],
        )
        .expect("seat");
    }

    /// The player seat of the match is `archetype`.
    pub fn player_seat(conn: &Connection, match_id: i64, archetype: i64) {
        conn.execute(
            "INSERT INTO match_archetypes (match_id, is_opponent, archetype_id, confidence, manual)
             VALUES (?1, 0, ?2, 1.0, 0)",
            params![match_id, archetype],
        )
        .expect("seat");
    }

    /// One game of a match.
    #[derive(Debug, Clone, Copy)]
    pub struct Game {
        pub result: &'static str,
        pub on_play: Option<bool>,
        pub turns: Option<i64>,
        pub mulligans: Option<i64>,
        pub opponent_mulligans: Option<i64>,
    }

    /// A game won or lost on the play or the draw, nothing else known.
    pub fn g(result: &'static str, on_play: bool) -> Game {
        Game {
            result,
            on_play: Some(on_play),
            turns: None,
            mulligans: None,
            opponent_mulligans: None,
        }
    }

    /// The games of a match, numbered and started in order.
    pub fn games(conn: &Connection, match_id: i64, games: &[Game]) {
        let started: i64 = conn
            .query_row(
                "SELECT started_at FROM matches WHERE id = ?1",
                [match_id],
                |row| row.get(0),
            )
            .expect("match start");
        for (index, game) in games.iter().enumerate() {
            let number = index as i64 + 1;
            conn.execute(
                "INSERT INTO games (match_id, game_number, result, end_reason, on_play,
                                    local_mulligans, opp_mulligans, turn_count, started_at)
                 VALUES (?1, ?2, ?3, 'win', ?4, ?5, ?6, ?7, ?8)",
                params![
                    match_id,
                    number,
                    game.result,
                    game.on_play,
                    game.mulligans,
                    game.opponent_mulligans,
                    game.turns,
                    started + number * 60_000
                ],
            )
            .expect("game");
        }
    }

    /// A complete match against `archetype` with `games`.
    pub fn against(
        conn: &Connection,
        version: i64,
        archetype: i64,
        outcome: &str,
        started_at: i64,
        list: &[Game],
    ) -> i64 {
        let id = complete(conn, version, outcome, started_at);
        opponent_seat(conn, id, archetype);
        games(conn, id, list);
        id
    }
}

#[cfg(test)]
mod tests {
    use super::seed::*;
    use super::*;
    use decks::stats::Range;

    const DAY: i64 = 86_400_000;
    /// 2026-03-15T12:00:00Z.
    const MARCH_15: i64 = 1_773_576_000_000;
    const UTC: Tz = chrono_tz::UTC;

    fn spread_of(conn: &Connection, version: i64) -> Vec<MatchupRow> {
        matchup_spread(conn, &[version], Range::all_time(), None).expect("spread")
    }

    // 0.x GetArchetypeMatchupSpreadTest "returns existing fields for a matchup".
    #[test]
    fn a_matchup_row_has_its_records_and_games() {
        let (_dir, conn) = db("spread-fields");
        let deck = deck(&conn, None, "CMODERN", None);
        let version = version(&conn, deck);
        let qburn = archetype(&conn, "Qburn", "modern", Some("R"));
        let turns = |result, on_play, turns| Game {
            turns: Some(turns),
            ..g(result, on_play)
        };
        against(
            &conn,
            version,
            qburn,
            "win",
            MARCH_15,
            &[turns("win", true, 8), turns("win", false, 10)],
        );

        let rows = spread_of(&conn, version);
        assert_eq!(rows.len(), 1);
        let row = &rows[0];
        assert_eq!(row.archetype_id, qburn);
        assert_eq!(row.name, "Qburn");
        assert_eq!(row.color_identity.as_deref(), Some("R"));
        assert_eq!(row.record.total, 1);
        assert_eq!(row.record.wins, 1);
        assert_eq!((row.games.won, row.games.lost), (2, 0));
        assert_eq!(row.total_games, 2);
        assert_eq!(row.games.win_rate, 100);
    }

    // "computes otp_winrate as on-the-play game win rate".
    #[test]
    fn the_on_the_play_rate_is_the_games_won_on_the_play() {
        let (_dir, conn) = db("spread-otp");
        let version = version(&conn, deck(&conn, None, "CMODERN", None));
        let qburn = archetype(&conn, "Qburn", "modern", None);
        against(
            &conn,
            version,
            qburn,
            "win",
            MARCH_15,
            &[g("win", true), g("win", true)],
        );
        against(
            &conn,
            version,
            qburn,
            "loss",
            MARCH_15 + DAY,
            &[g("loss", true)],
        );

        assert_eq!(spread_of(&conn, version)[0].otp_win_rate, 67);
    }

    /// Games that do not know who was on the play (imported or old logs)
    /// still count; the on-the-play rate is then 0.
    #[test]
    fn games_without_a_play_side_still_count() {
        let (_dir, conn) = db("spread-no-play-side");
        let version = version(&conn, deck(&conn, None, "CMODERN", None));
        let qburn = archetype(&conn, "Qburn", "modern", None);
        let unknown = |result: &'static str| Game {
            result,
            on_play: None,
            turns: None,
            mulligans: None,
            opponent_mulligans: None,
        };
        against(
            &conn,
            version,
            qburn,
            "win",
            MARCH_15,
            &[unknown("win"), unknown("loss"), unknown("win")],
        );

        let row = spread_of(&conn, version).remove(0);
        assert_eq!((row.games.won, row.games.lost), (2, 1));
        assert_eq!(row.otp_win_rate, 0);
    }

    // "computes avg_turns across all games" and "returns null avg_turns when
    // no turn data exists".
    #[test]
    fn the_average_turns_are_over_games_with_turns_one_decimal() {
        let (_dir, conn) = db("spread-turns");
        let version = version(&conn, deck(&conn, None, "CMODERN", None));
        let qburn = archetype(&conn, "Qburn", "modern", None);
        let qtron = archetype(&conn, "Qtron", "modern", None);
        let turns = |turns| Game {
            turns,
            ..g("win", true)
        };
        against(
            &conn,
            version,
            qburn,
            "win",
            MARCH_15,
            &[turns(Some(6)), turns(Some(10))],
        );
        against(
            &conn,
            version,
            qtron,
            "win",
            MARCH_15,
            &[turns(None), turns(None)],
        );
        let qelves = archetype(&conn, "Qelves", "modern", None);
        against(
            &conn,
            version,
            qelves,
            "win",
            MARCH_15,
            &[turns(Some(7)), turns(Some(7)), turns(Some(8)), turns(None)],
        );

        let rows = spread_of(&conn, version);
        let by_name = |name: &str| rows.iter().find(|row| row.name == name).expect(name);
        assert_eq!(by_name("Qburn").avg_turns, Some(8.0));
        assert_eq!(by_name("Qtron").avg_turns, None);
        assert_eq!(by_name("Qelves").avg_turns, Some(7.3));
    }

    // "runs with bounded query count": five archetypes, one row each, in
    // game win rate order.
    #[test]
    fn the_spread_is_one_row_per_archetype_by_game_win_rate() {
        let (_dir, conn) = db("spread-order");
        let version = version(&conn, deck(&conn, None, "CMODERN", None));
        for (n, wins) in [0, 2, 1].into_iter().enumerate() {
            let archetype = archetype(&conn, &format!("Qarch {n}"), "modern", None);
            let mut list = vec![g("loss", false); 2 - wins];
            list.extend(vec![g("win", true); wins]);
            against(&conn, version, archetype, "win", MARCH_15, &list);
        }
        let names: Vec<String> = spread_of(&conn, version)
            .into_iter()
            .map(|row| row.name)
            .collect();
        assert_eq!(names, ["Qarch 1", "Qarch 2", "Qarch 0"]);
    }

    // "aggregates matchups across multiple deck versions via forVersionIds".
    #[test]
    fn the_spread_adds_every_version_given() {
        let (_dir, conn) = db("spread-versions");
        let qmurk = archetype(&conn, "Qmurktide", "modern", None);
        let one = version(&conn, deck(&conn, None, "CMODERN", None));
        let two = version(&conn, deck(&conn, None, "CMODERN", None));
        against(&conn, one, qmurk, "win", MARCH_15, &[g("win", true)]);
        against(&conn, two, qmurk, "loss", MARCH_15, &[g("loss", false)]);

        let rows = matchup_spread(&conn, &[one, two], Range::all_time(), None).expect("spread");
        assert_eq!(rows.len(), 1);
        let row = &rows[0];
        assert_eq!(row.record.total, 2);
        assert_eq!((row.record.wins, row.record.losses), (1, 1));
        assert_eq!((row.games.won, row.games.lost), (1, 1));
    }

    // "shows draws in the match record so it matches the all-matches winrate".
    #[test]
    fn a_draw_counts_as_a_match_played() {
        let (_dir, conn) = db("spread-draw");
        let version = version(&conn, deck(&conn, None, "CMODERN", None));
        let qburn = archetype(&conn, "Qburn", "modern", None);
        against(&conn, version, qburn, "win", MARCH_15, &[g("win", true)]);
        against(&conn, version, qburn, "loss", MARCH_15, &[g("loss", true)]);
        against(
            &conn,
            version,
            qburn,
            "draw",
            MARCH_15,
            &[g("win", true), g("loss", false)],
        );

        let row = &spread_of(&conn, version)[0];
        assert_eq!(row.record.total, 3);
        assert_eq!(row.record.win_rate, 33);
        assert_eq!(row.record.draws, 1);
        assert_eq!(decks::stats::record_label(&row.record), "1 - 1 - 1");
    }

    // "normalises the archetype colour identity read straight from SQL".
    #[test]
    fn the_colour_identity_is_normalised() {
        let (_dir, conn) = db("spread-colours");
        let version = version(&conn, deck(&conn, None, "CMODERN", None));
        let qmardu = archetype(&conn, "Qmardu", "modern", Some("WBR"));
        against(&conn, version, qmardu, "win", MARCH_15, &[g("win", true)]);
        assert_eq!(
            spread_of(&conn, version)[0].color_identity.as_deref(),
            Some("W,B,R")
        );
    }

    #[test]
    fn the_spread_counts_the_opponent_seat_in_the_window_only() {
        let (_dir, conn) = db("spread-scope");
        let version = version(&conn, deck(&conn, None, "CMODERN", None));
        let qburn = archetype(&conn, "Qburn", "modern", None);
        let mine = archetype(&conn, "Qmine", "modern", None);
        let inside = against(&conn, version, qburn, "win", MARCH_15, &[g("win", true)]);
        player_seat(&conn, inside, mine);
        against(
            &conn,
            version,
            qburn,
            "loss",
            MARCH_15 - 40 * DAY,
            &[g("loss", true)],
        );
        let pending = played(&conn, version, "in_progress", "unknown", MARCH_15);
        opponent_seat(&conn, pending, qburn);

        let window = Range {
            from_ms: MARCH_15 - DAY,
            to_ms: MARCH_15 + DAY,
        };
        let rows = matchup_spread(&conn, &[version], window, None).expect("spread");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "Qburn");
        assert_eq!(rows[0].record.total, 1);
        assert!(
            matchup_spread(&conn, &[], Range::all_time(), None)
                .expect("none")
                .is_empty()
        );
    }

    #[test]
    fn a_matchup_spread_reads_one_kind() {
        let (_dir, conn) = db("spread-kinds");
        let deck = deck(&conn, None, "Standard", None);
        let version = version(&conn, deck);
        let qburn = archetype(&conn, "Qburn", "standard", Some("R"));
        let one = against(&conn, version, qburn, "win", MARCH_15, &[g("win", true)]);
        let three = against(
            &conn,
            version,
            qburn,
            "loss",
            MARCH_15 - DAY,
            &[g("loss", true), g("loss", false)],
        );
        crate::test_support::set_game_structure(&conn, &token_of(&conn, one), "bo1");
        crate::test_support::set_game_structure(&conn, &token_of(&conn, three), "bo3");
        let read = |best_of| {
            matchup_spread(&conn, &[version], Range::all_time(), best_of).expect("spread")
        };
        assert_eq!(read(Some(BestOf::One))[0].record.total, 1);
        assert_eq!(read(Some(BestOf::One))[0].games, GameRecord::new(1, 0));
        assert_eq!(read(Some(BestOf::Three))[0].games, GameRecord::new(0, 2));
        assert_eq!(read(None)[0].record.total, 2);
        let detail = |best_of| {
            matchup_detail(&conn, &[version], qburn, Range::all_time(), UTC, best_of)
                .expect("detail")
        };
        assert_eq!(detail(Some(BestOf::One)).record.total, 1);
        assert_eq!(detail(Some(BestOf::Three)).record.total, 1);
        assert_eq!(detail(None).record.total, 2);
    }

    fn detail_of(conn: &Connection, version: i64, archetype: i64) -> MatchupDetail {
        matchup_detail(conn, &[version], archetype, Range::all_time(), UTC, None).expect("detail")
    }

    // 0.x GetArchetypeMatchupDetailTest "returns win rates and records".
    #[test]
    fn the_detail_has_the_match_and_game_records() {
        let (_dir, conn) = db("detail-records");
        let version = version(&conn, deck(&conn, None, "CMODERN", None));
        let qburn = archetype(&conn, "Qburn", "modern", None);
        against(
            &conn,
            version,
            qburn,
            "win",
            MARCH_15,
            &[g("win", true), g("loss", false), g("win", true)],
        );
        against(
            &conn,
            version,
            qburn,
            "loss",
            MARCH_15,
            &[g("loss", true), g("loss", false)],
        );

        let detail = detail_of(&conn, version, qburn);
        assert_eq!(detail.record.win_rate, 50);
        assert_eq!(decks::stats::record_label(&detail.record), "1 - 1");
        assert_eq!(detail.record.total, 2);
        assert_eq!(detail.games.win_rate, 40);
        assert_eq!((detail.games.won, detail.games.lost), (2, 3));
    }

    // "returns play/draw breakdown".
    #[test]
    fn the_detail_splits_games_on_the_play_and_the_draw() {
        let (_dir, conn) = db("detail-play-draw");
        let version = version(&conn, deck(&conn, None, "CMODERN", None));
        let qburn = archetype(&conn, "Qburn", "modern", None);
        against(
            &conn,
            version,
            qburn,
            "win",
            MARCH_15,
            &[g("win", true), g("win", true), g("loss", false)],
        );

        let detail = detail_of(&conn, version, qburn);
        assert_eq!(detail.otp.win_rate, 100);
        assert_eq!((detail.otp.won, detail.otp.lost), (2, 0));
        assert_eq!(detail.otd.win_rate, 0);
        assert_eq!((detail.otd.won, detail.otd.lost), (0, 1));
    }

    // "returns game stats".
    #[test]
    fn the_detail_averages_turns_and_mulligans() {
        let (_dir, conn) = db("detail-game-stats");
        let version = version(&conn, deck(&conn, None, "CMODERN", None));
        let qburn = archetype(&conn, "Qburn", "modern", None);
        let game = |on_play, turns, mulligans| Game {
            turns: Some(turns),
            mulligans: Some(mulligans),
            ..g("win", on_play)
        };
        against(
            &conn,
            version,
            qburn,
            "win",
            MARCH_15,
            &[game(true, 6, 1), game(false, 10, 0)],
        );

        let detail = detail_of(&conn, version, qburn);
        assert_eq!(detail.avg_turns, Some(8.0));
        assert_eq!(detail.avg_mulligans, Some(0.5));
        assert_eq!(detail.on_play_rate, 50);
    }

    // "returns match history in descending date order".
    #[test]
    fn the_history_is_newest_first_with_scores_and_game_results() {
        let (_dir, conn) = db("detail-history");
        let version = version(&conn, deck(&conn, None, "CMODERN", None));
        let qburn = archetype(&conn, "Qburn", "modern", None);
        // 2026-03-01T12:00Z and 2026-03-15T12:00Z.
        let older = against(
            &conn,
            version,
            qburn,
            "loss",
            MARCH_15 - 14 * DAY,
            &[g("loss", true), g("loss", false)],
        );
        opponent(&conn, older, "Qopponent");
        against(
            &conn,
            version,
            qburn,
            "win",
            MARCH_15,
            &[g("win", true), g("win", false)],
        );

        let history = detail_of(&conn, version, qburn).history;
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].outcome, "win");
        assert_eq!(history[0].score, "2-0");
        assert_eq!(history[0].date_label, "Mar 15");
        assert_eq!(history[0].opponent, None);
        assert_eq!(history[0].game_results, [Some(true), Some(true)]);
        assert_eq!(history[1].outcome, "loss");
        assert_eq!(history[1].score, "0-2");
        assert_eq!(history[1].date_label, "Mar 1");
        assert_eq!(history[1].opponent.as_deref(), Some("Qopponent"));
        assert_eq!(history[1].game_results, [Some(false), Some(false)]);
        assert_eq!(history[1].token, token_of(&conn, older));
    }

    // "counts draws as matches played and shows them in the record", and
    // the per game rows.
    #[test]
    fn the_detail_counts_draws_and_rates_each_game_number() {
        let (_dir, conn) = db("detail-draws");
        let version = version(&conn, deck(&conn, None, "CMODERN", None));
        let qburn = archetype(&conn, "Qburn", "modern", None);
        against(&conn, version, qburn, "win", MARCH_15, &[g("win", true)]);
        against(&conn, version, qburn, "loss", MARCH_15, &[g("loss", true)]);
        against(
            &conn,
            version,
            qburn,
            "draw",
            MARCH_15,
            &[g("win", true), g("loss", false)],
        );

        let detail = detail_of(&conn, version, qburn);
        assert_eq!(detail.record.win_rate, 33);
        assert_eq!(decks::stats::record_label(&detail.record), "1 - 1 - 1");
        assert_eq!(detail.record.total, 3);
        let per_game: Vec<(u32, u32, u32)> = detail
            .per_game
            .iter()
            .map(|game| (game.game_number, game.games.won, game.games.lost))
            .collect();
        assert_eq!(per_game, [(1, 2, 1), (2, 0, 1)]);
    }

    // "returns empty card arrays" (no best or worst cards in v1 either) and
    // MatchupDetailControllerTest "accepts timeframe query parameter".
    #[test]
    fn a_matchup_never_faced_has_an_empty_detail() {
        let (_dir, conn) = db("detail-empty");
        let version = version(&conn, deck(&conn, None, "CMODERN", None));
        let qburn = archetype(&conn, "Qburn", "modern", None);
        against(
            &conn,
            version,
            qburn,
            "win",
            MARCH_15 - 40 * DAY,
            &[g("win", true)],
        );
        let week = Range {
            from_ms: MARCH_15 - 7 * DAY,
            to_ms: MARCH_15,
        };
        let detail = matchup_detail(&conn, &[version], qburn, week, UTC, None).expect("detail");
        assert_eq!(detail, MatchupDetail::default());
        assert_eq!(decks::stats::record_label(&detail.record), "0 - 0");
    }

    // MatchupDetailControllerTest "returns matchup detail as json".
    #[test]
    fn the_detail_counts_a_match_whose_only_game_has_no_play_side() {
        let (_dir, conn) = db("detail-one");
        let version = version(&conn, deck(&conn, None, "CMODERN", None));
        let qburn = archetype(&conn, "Qburn", "modern", None);
        let game = Game {
            on_play: None,
            ..g("win", true)
        };
        against(&conn, version, qburn, "win", MARCH_15, &[game]);
        let detail = detail_of(&conn, version, qburn);
        assert_eq!(detail.record.win_rate, 100);
        assert_eq!(detail.record.total, 1);
    }

    fn rows(results: &[(u32, u32)]) -> Vec<MatchupRow> {
        results
            .iter()
            .enumerate()
            .map(|(n, (wins, losses))| MatchupRow {
                archetype_id: n as i64 + 1,
                name: format!("Q{n}"),
                color_identity: None,
                record: Record::from_counts(*wins, *losses, 0),
                games: GameRecord::default(),
                total_games: 0,
                otp_win_rate: 0,
                avg_turns: None,
            })
            .collect()
    }

    // BuildDeckArchetypeHeaderTest "picks best and worst matchups by win
    // rate with a three match floor", "breaks win rate ties by match count",
    // "returns null matchups when fewer than two opponents qualify".
    #[test]
    fn best_and_worst_need_three_matches_and_two_archetypes() {
        let (best, worst) = best_and_worst(&rows(&[(3, 1), (1, 3), (2, 0)]));
        assert_eq!(best.map(|row| row.name), Some("Q0".to_string()));
        assert_eq!(worst.map(|row| row.name), Some("Q1".to_string()));

        let (best, _) = best_and_worst(&rows(&[(3, 0), (4, 0)]));
        assert_eq!(best.map(|row| row.name), Some("Q1".to_string()));

        assert_eq!(best_and_worst(&rows(&[(2, 1), (1, 1)])), (None, None));
    }

    // 0.x GetPeerArchetypeChartDataTest, six cases.
    #[test]
    fn the_peer_chart_is_the_other_decks_of_the_archetype() {
        let (_dir, conn) = db("peer-chart");
        let me = account(&conn, "Qme");
        let other = account(&conn, "Qother");
        let window = Range {
            from_ms: MARCH_15 - 30 * DAY,
            to_ms: MARCH_15 + DAY,
        };

        // "returns null when deck has no archetype".
        let lone = deck(&conn, Some(me), "CMODERN", None);
        assert_eq!(
            peer_chart(&conn, lone, Some(me), Platform::Mtgo, window, UTC, None).expect("none"),
            None
        );

        // "returns null when no peer decks share the archetype".
        let qred = archetype(&conn, "Qmono red", "modern", None);
        let current = deck(&conn, Some(me), "CMODERN", Some(qred));
        let current_version = version(&conn, current);
        assert_eq!(
            peer_chart(&conn, current, Some(me), Platform::Mtgo, window, UTC, None).expect("none"),
            None
        );

        // "returns null when peers exist but have no matches in range".
        let peer_a = version(&conn, deck(&conn, Some(me), "CMODERN", Some(qred)));
        let peer_b = version(&conn, deck(&conn, Some(me), "CMODERN", Some(qred)));
        complete(&conn, peer_a, "win", MARCH_15 - 365 * DAY);
        assert_eq!(
            peer_chart(&conn, current, Some(me), Platform::Mtgo, window, UTC, None).expect("none"),
            None
        );

        // "aggregates daily wins/losses across peer decks excluding the
        // current deck"; another account's deck is not a peer.
        complete(&conn, peer_a, "win", MARCH_15);
        complete(&conn, peer_a, "loss", MARCH_15);
        complete(&conn, peer_b, "win", MARCH_15);
        complete(&conn, peer_b, "draw", MARCH_15);
        complete(&conn, current_version, "win", MARCH_15);
        let theirs = version(&conn, deck(&conn, Some(other), "CMODERN", Some(qred)));
        complete(&conn, theirs, "win", MARCH_15);
        let chart = peer_chart(&conn, current, Some(me), Platform::Mtgo, window, UTC, None)
            .expect("chart")
            .expect("peers");
        assert_eq!(chart.archetype_name, "Qmono red");
        assert_eq!(chart.deck_count, 2);
        assert_eq!(chart.days.len(), 1);
        assert_eq!(chart.days[0].date, "2026-03-15");
        let record = chart.days[0].record;
        assert_eq!((record.wins, record.losses, record.draws), (2, 1, 1));

        // "buckets matches by day in the system timezone rather than UTC":
        // 03:00 UTC on the 8th is 20:00 PDT on the 7th.
        let (_dir, conn) = db("peer-chart-tz");
        let qred = archetype(&conn, "Qmono red", "modern", None);
        let current = deck(&conn, None, "CMODERN", Some(qred));
        let peer = version(&conn, deck(&conn, None, "CMODERN", Some(qred)));
        // 2026-09-08T03:00:00Z.
        complete(&conn, peer, "win", 1_788_836_400_000);
        let september = Range {
            from_ms: 1_788_220_800_000,
            to_ms: 1_790_812_800_000,
        };
        let chart = peer_chart(
            &conn,
            current,
            None,
            Platform::Mtgo,
            september,
            chrono_tz::America::Los_Angeles,
            None,
        )
        .expect("chart")
        .expect("peers");
        assert_eq!(chart.days.len(), 1);
        assert_eq!(chart.days[0].date, "2026-09-07");
    }

    #[test]
    fn earlier_peer_matches_move_the_first_days_trend() {
        let (_dir, conn) = db("peer-chart-trend");
        let qred = archetype(&conn, "Qmono red", "modern", None);
        let current = deck(&conn, None, "CMODERN", Some(qred));
        let peer_deck = deck(&conn, None, "CMODERN", Some(qred));
        let peer = version(&conn, peer_deck);
        let other_archetype = archetype(&conn, "Qother", "modern", None);
        let stranger = version(&conn, deck(&conn, None, "CMODERN", Some(other_archetype)));
        // The current deck's and another archetype's earlier losses are not
        // the peers' form.
        complete(&conn, version(&conn, current), "loss", MARCH_15 - 40 * DAY);
        complete(&conn, stranger, "loss", MARCH_15 - 40 * DAY);
        let window = Range {
            from_ms: MARCH_15 - 10 * DAY,
            to_ms: MARCH_15 + DAY,
        };
        complete(&conn, peer, "win", MARCH_15);
        let read = |window| {
            peer_chart(&conn, current, None, Platform::Mtgo, window, UTC, None)
                .expect("chart")
                .expect("peers")
                .days[0]
                .trend
        };
        assert_eq!(read(window), Some(100));

        complete(&conn, peer, "loss", MARCH_15 - 40 * DAY);
        // A peer loss then a peer win: 1 / (1 + 49/51) = 0.51.
        assert_eq!(read(window), Some(51));
    }

    #[test]
    fn the_peer_chart_reads_one_platform() {
        let (_dir, conn) = db("peer-chart-arena");
        let qred = archetype(&conn, "Qmono red", "modern", None);
        let current = deck(&conn, None, "CMODERN", Some(qred));
        let peer = version(&conn, deck(&conn, None, "CMODERN", Some(qred)));
        let arena_deck = deck(&conn, None, "CMODERN", Some(qred));
        let arena = version(&conn, arena_deck);
        crate::test_support::move_deck_to_arena(&conn, arena_deck);
        complete(&conn, peer, "win", MARCH_15);
        complete(&conn, arena, "loss", MARCH_15);
        let window = Range {
            from_ms: MARCH_15 - DAY,
            to_ms: MARCH_15 + DAY,
        };
        let chart = peer_chart(&conn, current, None, Platform::Mtgo, window, UTC, None)
            .expect("chart")
            .expect("peers");
        assert_eq!(chart.deck_count, 1);
        let record = chart.days[0].record;
        assert_eq!((record.wins, record.losses), (1, 0));

        // An Arena deck's peers are the Arena decks only.
        let arena_current = deck(&conn, None, "CMODERN", Some(qred));
        crate::test_support::move_deck_to_arena(&conn, arena_current);
        let chart = peer_chart(
            &conn,
            arena_current,
            None,
            Platform::Arena,
            window,
            UTC,
            None,
        )
        .expect("chart")
        .expect("peers");
        assert_eq!(chart.deck_count, 1);
        let record = chart.days[0].record;
        assert_eq!((record.wins, record.losses), (0, 1));
    }

    #[test]
    fn the_peer_chart_reads_one_kind() {
        let (_dir, conn) = db("peer-chart-kind");
        let qred = archetype(&conn, "Qmono red", "standard", None);
        let current = deck(&conn, None, "Standard", Some(qred));
        let peer_deck = deck(&conn, None, "Standard", Some(qred));
        let peer = version(&conn, peer_deck);
        for id in [current, peer_deck] {
            crate::test_support::move_deck_to_arena(&conn, id);
        }
        let bo1 = complete(&conn, peer, "win", MARCH_15);
        let bo3 = complete(&conn, peer, "loss", MARCH_15);
        crate::test_support::set_game_structure(&conn, &token_of(&conn, bo1), "bo1");
        crate::test_support::set_game_structure(&conn, &token_of(&conn, bo3), "bo3");
        let window = Range {
            from_ms: MARCH_15 - DAY,
            to_ms: MARCH_15 + DAY,
        };
        let read = |best_of| {
            peer_chart(&conn, current, None, Platform::Arena, window, UTC, best_of)
                .expect("chart")
                .expect("peers")
                .days[0]
                .record
        };
        assert_eq!(read(Some(BestOf::One)), Record::from_counts(1, 0, 0));
        assert_eq!(read(Some(BestOf::Three)), Record::from_counts(0, 1, 0));
        assert_eq!(read(None), Record::from_counts(1, 1, 0));
    }

    #[test]
    fn the_detail_marks_league_matches_without_removed_leagues() {
        let (_dir, conn) = db("detail-removed-league");
        let version = version(&conn, deck(&conn, None, "CMODERN", None));
        let qburn = archetype(&conn, "Qburn", "modern", None);
        let in_league = against(&conn, version, qburn, "win", MARCH_15, &[g("win", true)]);
        let taken_out = against(
            &conn,
            version,
            qburn,
            "win",
            MARCH_15 + 1,
            &[g("win", true)],
        );
        crate::test_support::insert_manual_league(&conn, 4, None, None, "Q manual", 0, 0);
        crate::test_support::link_league_match(&conn, 4, &token_of(&conn, in_league));
        conn.execute(
            "UPDATE matches SET league_token = 'q-league', no_league = 1 WHERE id = ?1",
            [taken_out],
        )
        .expect("taken out of every league");
        let marks = |conn: &Connection| -> Vec<bool> {
            detail_of(conn, version, qburn)
                .history
                .iter()
                .map(|played| played.is_league)
                .collect()
        };
        assert_eq!(marks(&conn), vec![false, true], "newest first");
        conn.execute("UPDATE leagues SET removed_at = 5 WHERE id = 4", [])
            .expect("removed");
        assert_eq!(marks(&conn), vec![false, false]);
    }
}
