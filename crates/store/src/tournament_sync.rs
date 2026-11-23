//! The event overlay's tournaments (event overlay spec 7.1): the stored
//! rows the tournament sync writes and the card reads.
//!
//! - `tournament_standings`: one row per tournament the account joined (the
//!   standing fold's output, `leagues::standing::TournamentStanding`), with
//!   the sync's bookkeeping (the newest log position whose round lines are
//!   decoded).
//! - `tournament_rounds`: each round's decoded round info line with its log
//!   position, so a refold never parses a 220 KB round info line twice.
//!   Facts only: the round's stage and result come from the fold.
//! - `tournament_player_states`: the `FlsPlayerEventStateChangedMessage`
//!   facts (`InPlayoffs`) of every event, keyed by log position.
//!
//! Per device: never synced between devices, never made from a pulled or
//! imported match. Writes compare first and report whether a row changed,
//! so a sync that finds nothing new writes nothing and raises no event.

use std::collections::HashSet;

use leagues::standing::{
    OwnMatch, STANDING_VERSION, StandingFact, StandingFinish, StandingStage, StandingStatus,
    TournamentStanding, fold_standing,
};
use mtgo_protocol::EventKind;
use mtgo_protocol::tournament::{RoundPairing, TournamentFact, decode_fact};
use rusqlite::{Connection, OptionalExtension, Row, params};

use crate::StoreError;
use crate::write::decode_raw;

/// The derivation name of the tournament sync (`derivations.name`); its
/// version is the fold's `STANDING_VERSION`.
const SYNC_DERIVATION: &str = "tournament_standing";

/// Where an event sits in the log: `(log_file_id, byte_offset)`.
pub type LogPosition = (i64, i64);

fn status_text(status: StandingStatus) -> Result<&'static str, StoreError> {
    match status {
        StandingStatus::Registered => Ok("registered"),
        StandingStatus::Playing => Ok("playing"),
        StandingStatus::WaitingForRound => Ok("waiting_for_round"),
        StandingStatus::Bye => Ok("bye"),
        StandingStatus::MadeCut => Ok("made_cut"),
        StandingStatus::Finished => Ok("finished"),
        // An unregistered event has no row: the sync deletes it.
        StandingStatus::Unregistered => Err(StoreError::Corrupt(
            "an unregistered tournament is not stored".to_string(),
        )),
    }
}

pub(crate) fn parse_status(text: &str) -> Result<StandingStatus, StoreError> {
    match text {
        "registered" => Ok(StandingStatus::Registered),
        "playing" => Ok(StandingStatus::Playing),
        "waiting_for_round" => Ok(StandingStatus::WaitingForRound),
        "bye" => Ok(StandingStatus::Bye),
        "made_cut" => Ok(StandingStatus::MadeCut),
        "finished" => Ok(StandingStatus::Finished),
        other => Err(StoreError::Corrupt(format!("tournament status {other}"))),
    }
}

fn finish_text(finish: StandingFinish) -> &'static str {
    match finish {
        StandingFinish::Trophied => "trophied",
        StandingFinish::TopCut => "top_cut",
        StandingFinish::MissedCut => "missed_cut",
        StandingFinish::Complete => "complete",
        StandingFinish::Dropped => "dropped",
        StandingFinish::Eliminated => "eliminated",
    }
}

pub(crate) fn parse_finish(text: &str) -> Result<StandingFinish, StoreError> {
    match text {
        "trophied" => Ok(StandingFinish::Trophied),
        "top_cut" => Ok(StandingFinish::TopCut),
        "missed_cut" => Ok(StandingFinish::MissedCut),
        "complete" => Ok(StandingFinish::Complete),
        "dropped" => Ok(StandingFinish::Dropped),
        "eliminated" => Ok(StandingFinish::Eliminated),
        other => Err(StoreError::Corrupt(format!("tournament finish {other}"))),
    }
}

fn stage_text(stage: StandingStage) -> &'static str {
    match stage {
        StandingStage::Swiss => "swiss",
        StandingStage::Top8 => "top8",
        StandingStage::Semifinal => "semifinal",
        StandingStage::Final => "final",
    }
}

pub(crate) fn parse_stage(text: &str) -> Result<StandingStage, StoreError> {
    match text {
        "swiss" => Ok(StandingStage::Swiss),
        "top8" => Ok(StandingStage::Top8),
        "semifinal" => Ok(StandingStage::Semifinal),
        "final" => Ok(StandingStage::Final),
        other => Err(StoreError::Corrupt(format!("tournament stage {other}"))),
    }
}

/// One `tournament_standings` row (without the sync's bookkeeping). Times
/// are log times in UTC unix milliseconds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StandingRow {
    pub account_id: i64,
    pub mtgo_event_id: i64,
    pub token: String,
    /// `EventSyncData_t` `Description`, from the log (never synthesized).
    pub name: Option<String>,
    /// `GameStructureCd`.
    pub format: Option<String>,
    /// The cut's player count; `None`: no cut or unknown.
    pub cut_size: Option<i64>,
    pub swiss_rounds: Option<i64>,
    pub round_minutes: Option<i64>,
    pub starts_at: Option<i64>,
    /// Never `Unregistered`: such an event has no row.
    pub status: StandingStatus,
    /// The current round (playoff rounds continue the count).
    pub round: Option<i64>,
    pub stage: StandingStage,
    pub in_playoffs: bool,
    /// When the current round was paired.
    pub round_started_at: Option<i64>,
    /// When the current round's result was logged.
    pub round_result_at: Option<i64>,
    pub rank: Option<i64>,
    pub players: Option<i64>,
    /// Open matches of the current round, every player's.
    pub matches_left: Option<i64>,
    pub byes: i64,
    /// `Some` exactly when `status` is `Finished`.
    pub finish: Option<StandingFinish>,
    pub place: Option<i64>,
    pub finished_at: Option<i64>,
    /// The newest real transition (event overlay spec 6).
    pub last_transition_at: i64,
    /// The newest fact of any kind (staleness).
    pub last_event_at: i64,
}

/// The row the fold's `standing` makes for the account, `None` when it is
/// not stored: not joined, unregistered, or without an event id (the key).
pub fn standing_row(account_id: i64, standing: &TournamentStanding) -> Option<StandingRow> {
    if !standing.joined || standing.status == StandingStatus::Unregistered {
        return None;
    }
    let mtgo_event_id = standing.event_id?;
    let current = standing
        .round
        .and_then(|number| standing.rounds.iter().find(|round| round.number == number));
    Some(StandingRow {
        account_id,
        mtgo_event_id,
        token: standing.token.clone(),
        name: standing.name.clone(),
        format: standing.format.clone(),
        cut_size: standing.cut_size.map(i64::from),
        swiss_rounds: standing.swiss_rounds.map(i64::from),
        round_minutes: standing.round_minutes.map(i64::from),
        starts_at: standing.starts_at_ms,
        status: standing.status,
        round: standing.round.map(i64::from),
        stage: standing.stage,
        in_playoffs: standing.in_playoffs,
        round_started_at: current.and_then(|round| round.paired_at_ms),
        round_result_at: current.and_then(|round| round.result_at_ms),
        rank: standing.rank.map(i64::from),
        players: standing.players.map(i64::from),
        matches_left: standing.matches_left.map(i64::from),
        byes: i64::from(standing.byes),
        finish: standing.finish,
        place: standing.place.map(i64::from),
        finished_at: standing.finished_at_ms,
        last_transition_at: standing.last_transition_at_ms,
        last_event_at: standing.last_event_at_ms,
    })
}

/// One `tournament_rounds` row: a decoded round info line of the account's
/// tournament and where it was logged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoundRow {
    pub account_id: i64,
    pub mtgo_event_id: i64,
    pub round: i64,
    pub bye_count: i64,
    /// Every match token of the round (`Round.Matches[].EventToken`).
    pub match_tokens: Vec<String>,
    /// The account's pairing: its match token this round.
    pub account_match_token: Option<String>,
    pub account_bye: bool,
    /// The round's earliest match `StartDate`.
    pub started_at: Option<i64>,
    /// The round's `MatchTimeInMinutes`.
    pub match_minutes: Option<i64>,
    /// The log position of the round info line, so a refold puts the stored
    /// round back in log order.
    pub paired_position: LogPosition,
    /// When the round info was logged.
    pub paired_at: i64,
}

impl RoundRow {
    /// The row for a decoded round info line of `mtgo_event_id`.
    pub fn from_pairing(
        account_id: i64,
        mtgo_event_id: i64,
        pairing: &RoundPairing,
        position: LogPosition,
        logged_at: i64,
    ) -> RoundRow {
        RoundRow {
            account_id,
            mtgo_event_id,
            round: i64::from(pairing.number),
            bye_count: i64::from(pairing.bye_count),
            match_tokens: pairing.match_tokens.clone(),
            account_match_token: pairing.account_match_token.clone(),
            account_bye: pairing.account_bye,
            started_at: pairing.started_at_ms,
            match_minutes: pairing.match_minutes.map(i64::from),
            paired_position: position,
            paired_at: logged_at,
        }
    }

    /// The stored round as the fold's fact, at the line's log position. A
    /// number or length that does not fit is corrupt.
    pub fn fact(&self) -> Result<StandingFact, StoreError> {
        let number = u32::try_from(self.round)
            .map_err(|_| StoreError::Corrupt(format!("tournament round {}", self.round)))?;
        let bye_count = u32::try_from(self.bye_count)
            .map_err(|_| StoreError::Corrupt(format!("round byes {}", self.bye_count)))?;
        let match_minutes = match self.match_minutes {
            Some(minutes) => Some(
                u32::try_from(minutes)
                    .map_err(|_| StoreError::Corrupt(format!("round minutes {minutes}")))?,
            ),
            None => None,
        };
        Ok(StandingFact {
            log_file_id: self.paired_position.0,
            byte_offset: offset_of(self.paired_position.1)?,
            logged_at_ms: self.paired_at,
            fact: TournamentFact::Round(RoundPairing {
                event_id: Some(self.mtgo_event_id),
                number,
                match_tokens: self.match_tokens.clone(),
                bye_count,
                account_match_token: self.account_match_token.clone(),
                account_bye: self.account_bye,
                match_minutes,
                started_at_ms: self.started_at,
            }),
        })
    }
}

/// One `FlsPlayerEventStateChangedMessage` line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayerStateRow {
    pub log_file_id: i64,
    pub byte_offset: i64,
    pub tournament_token: String,
    /// MTGO's `NewState` number.
    pub new_state: i64,
    pub in_playoffs: bool,
    pub logged_at: i64,
}

const STANDING_COLUMNS: &str = "account_id, mtgo_event_id, token, name, format, cut_size,
    swiss_rounds, round_minutes, starts_at, status, round, stage, in_playoffs,
    round_started_at, round_result_at, rank, players, matches_left, byes, finish, place,
    finished_at, last_transition_at, last_event_at";

/// A standing row as read, with its status, stage and finish still text.
struct StandingRead {
    row: StandingRow,
    status: String,
    stage: String,
    finish: Option<String>,
}

fn standing_from(row: &Row) -> rusqlite::Result<StandingRead> {
    Ok(StandingRead {
        row: StandingRow {
            account_id: row.get(0)?,
            mtgo_event_id: row.get(1)?,
            token: row.get(2)?,
            name: row.get(3)?,
            format: row.get(4)?,
            cut_size: row.get(5)?,
            swiss_rounds: row.get(6)?,
            round_minutes: row.get(7)?,
            starts_at: row.get(8)?,
            // Set from the texts by `parse_standing`.
            status: StandingStatus::Registered,
            round: row.get(10)?,
            stage: StandingStage::Swiss,
            in_playoffs: row.get(12)?,
            round_started_at: row.get(13)?,
            round_result_at: row.get(14)?,
            rank: row.get(15)?,
            players: row.get(16)?,
            matches_left: row.get(17)?,
            byes: row.get(18)?,
            finish: None,
            place: row.get(20)?,
            finished_at: row.get(21)?,
            last_transition_at: row.get(22)?,
            last_event_at: row.get(23)?,
        },
        status: row.get(9)?,
        stage: row.get(11)?,
        finish: row.get(19)?,
    })
}

fn parse_standing(read: StandingRead) -> Result<StandingRow, StoreError> {
    let mut row = read.row;
    row.status = parse_status(&read.status)?;
    row.stage = parse_stage(&read.stage)?;
    row.finish = match read.finish {
        Some(text) => Some(parse_finish(&text)?),
        None => None,
    };
    Ok(row)
}

/// The account's standing in tournament `mtgo_event_id`, if stored.
pub fn standing(
    conn: &Connection,
    account_id: i64,
    mtgo_event_id: i64,
) -> Result<Option<StandingRow>, StoreError> {
    let found = conn
        .prepare_cached(&format!(
            "SELECT {STANDING_COLUMNS} FROM tournament_standings
             WHERE account_id = ?1 AND mtgo_event_id = ?2"
        ))?
        .query_row([account_id, mtgo_event_id], standing_from)
        .optional()?;
    found.map(parse_standing).transpose()
}

/// Every stored standing of the account, by event id.
pub fn standings(conn: &Connection, account_id: i64) -> Result<Vec<StandingRow>, StoreError> {
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT {STANDING_COLUMNS} FROM tournament_standings
         WHERE account_id = ?1 ORDER BY mtgo_event_id"
    ))?;
    let rows = stmt.query_map([account_id], standing_from)?;
    let mut found = Vec::new();
    for row in rows {
        found.push(parse_standing(row?)?);
    }
    Ok(found)
}

/// Writes `row` when it differs from the stored one. True when it changed.
/// The sync's bookkeeping (`rounds_read_through`) is kept.
pub fn write_standing(conn: &Connection, row: &StandingRow) -> Result<bool, StoreError> {
    if standing(conn, row.account_id, row.mtgo_event_id)?.as_ref() == Some(row) {
        return Ok(false);
    }
    conn.prepare_cached(&format!(
        "INSERT INTO tournament_standings ({STANDING_COLUMNS})
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17,
             ?18, ?19, ?20, ?21, ?22, ?23, ?24)
         ON CONFLICT (account_id, mtgo_event_id) DO UPDATE SET
             token = excluded.token, name = excluded.name, format = excluded.format,
             cut_size = excluded.cut_size, swiss_rounds = excluded.swiss_rounds,
             round_minutes = excluded.round_minutes, starts_at = excluded.starts_at,
             status = excluded.status, round = excluded.round, stage = excluded.stage,
             in_playoffs = excluded.in_playoffs, round_started_at = excluded.round_started_at,
             round_result_at = excluded.round_result_at, rank = excluded.rank,
             players = excluded.players, matches_left = excluded.matches_left,
             byes = excluded.byes, finish = excluded.finish, place = excluded.place,
             finished_at = excluded.finished_at,
             last_transition_at = excluded.last_transition_at,
             last_event_at = excluded.last_event_at"
    ))?
    .execute(params![
        row.account_id,
        row.mtgo_event_id,
        row.token,
        row.name,
        row.format,
        row.cut_size,
        row.swiss_rounds,
        row.round_minutes,
        row.starts_at,
        status_text(row.status)?,
        row.round,
        stage_text(row.stage),
        row.in_playoffs,
        row.round_started_at,
        row.round_result_at,
        row.rank,
        row.players,
        row.matches_left,
        row.byes,
        row.finish.map(finish_text),
        row.place,
        row.finished_at,
        row.last_transition_at,
        row.last_event_at,
    ])?;
    Ok(true)
}

/// Deletes the account's standing and rounds of `mtgo_event_id` (a
/// tournament that left the window, or that the account unregistered
/// from). True when a row went.
pub fn delete_tournament(
    conn: &Connection,
    account_id: i64,
    mtgo_event_id: i64,
) -> Result<bool, StoreError> {
    let rounds = conn
        .prepare_cached(
            "DELETE FROM tournament_rounds WHERE account_id = ?1 AND mtgo_event_id = ?2",
        )?
        .execute([account_id, mtgo_event_id])?;
    let standings = conn
        .prepare_cached(
            "DELETE FROM tournament_standings WHERE account_id = ?1 AND mtgo_event_id = ?2",
        )?
        .execute([account_id, mtgo_event_id])?;
    Ok(rounds + standings > 0)
}

/// How far a tournament's round lines are decoded into `tournament_rounds`:
/// the newest log position read, and the account login id they were decoded
/// for (the account's pairing is found by it).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RoundsRead {
    pub position: LogPosition,
    pub login_id: Option<i64>,
}

/// How far the account's round lines of `mtgo_event_id` are decoded, `None`
/// before the first.
pub fn rounds_read_through(
    conn: &Connection,
    account_id: i64,
    mtgo_event_id: i64,
) -> Result<Option<RoundsRead>, StoreError> {
    let found: Option<(Option<i64>, Option<i64>, Option<i64>)> = conn
        .prepare_cached(
            "SELECT rounds_read_file_id, rounds_read_offset, rounds_login_id
             FROM tournament_standings WHERE account_id = ?1 AND mtgo_event_id = ?2",
        )?
        .query_row([account_id, mtgo_event_id], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .optional()?;
    Ok(match found {
        Some((Some(file), Some(offset), login_id)) => Some(RoundsRead {
            position: (file, offset),
            login_id,
        }),
        _ => None,
    })
}

/// Records how far the round lines are decoded, on a stored standing. This
/// is bookkeeping, not a change of the standing.
pub fn set_rounds_read_through(
    conn: &Connection,
    account_id: i64,
    mtgo_event_id: i64,
    read: RoundsRead,
) -> Result<(), StoreError> {
    conn.prepare_cached(
        "UPDATE tournament_standings SET rounds_read_file_id = ?3, rounds_read_offset = ?4,
             rounds_login_id = ?5
         WHERE account_id = ?1 AND mtgo_event_id = ?2",
    )?
    .execute(params![
        account_id,
        mtgo_event_id,
        read.position.0,
        read.position.1,
        read.login_id
    ])?;
    Ok(())
}

const ROUND_COLUMNS: &str = "account_id, mtgo_event_id, round, bye_count, match_tokens,
    account_match_token, account_bye, started_at, match_minutes, paired_file_id, paired_offset,
    paired_at";

/// A round row as read, with its tokens still JSON text.
struct RoundRead {
    row: RoundRow,
    tokens: String,
}

fn round_from(row: &Row) -> rusqlite::Result<RoundRead> {
    Ok(RoundRead {
        row: RoundRow {
            account_id: row.get(0)?,
            mtgo_event_id: row.get(1)?,
            round: row.get(2)?,
            bye_count: row.get(3)?,
            // Set from the text by `parse_round`.
            match_tokens: Vec::new(),
            account_match_token: row.get(5)?,
            account_bye: row.get(6)?,
            started_at: row.get(7)?,
            match_minutes: row.get(8)?,
            paired_position: (row.get(9)?, row.get(10)?),
            paired_at: row.get(11)?,
        },
        tokens: row.get(4)?,
    })
}

fn parse_round(read: RoundRead) -> Result<RoundRow, StoreError> {
    let mut row = read.row;
    row.match_tokens = serde_json::from_str(&read.tokens)
        .map_err(|error| StoreError::Corrupt(format!("round match tokens: {error}")))?;
    Ok(row)
}

/// The account's stored rounds of `mtgo_event_id`, by round number.
pub fn rounds(
    conn: &Connection,
    account_id: i64,
    mtgo_event_id: i64,
) -> Result<Vec<RoundRow>, StoreError> {
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT {ROUND_COLUMNS} FROM tournament_rounds
         WHERE account_id = ?1 AND mtgo_event_id = ?2 ORDER BY round"
    ))?;
    let rows = stmt.query_map([account_id, mtgo_event_id], round_from)?;
    let mut found = Vec::new();
    for row in rows {
        found.push(parse_round(row?)?);
    }
    Ok(found)
}

/// Stores a decoded round info line. The round's earliest line in log order
/// wins, as in the fold: a line logged after the stored one (a repeat, or a
/// re-read) writes nothing. True when the row changed.
pub fn write_round(conn: &Connection, row: &RoundRow) -> Result<bool, StoreError> {
    let tokens = serde_json::to_string(&row.match_tokens)
        .map_err(|error| StoreError::Corrupt(format!("round match tokens: {error}")))?;
    let changed = conn
        .prepare_cached(&format!(
            "INSERT INTO tournament_rounds ({ROUND_COLUMNS})
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
             ON CONFLICT (account_id, mtgo_event_id, round) DO UPDATE SET
                 bye_count = excluded.bye_count, match_tokens = excluded.match_tokens,
                 account_match_token = excluded.account_match_token,
                 account_bye = excluded.account_bye, started_at = excluded.started_at,
                 match_minutes = excluded.match_minutes,
                 paired_file_id = excluded.paired_file_id,
                 paired_offset = excluded.paired_offset, paired_at = excluded.paired_at
             WHERE (excluded.paired_file_id, excluded.paired_offset)
                 < (paired_file_id, paired_offset)"
        ))?
        .execute(params![
            row.account_id,
            row.mtgo_event_id,
            row.round,
            row.bye_count,
            tokens,
            row.account_match_token,
            row.account_bye,
            row.started_at,
            row.match_minutes,
            row.paired_position.0,
            row.paired_position.1,
            row.paired_at,
        ])?;
    Ok(changed > 0)
}

/// The stored player state lines of tournament `token`, in log order.
pub fn player_states(conn: &Connection, token: &str) -> Result<Vec<PlayerStateRow>, StoreError> {
    let mut stmt = conn.prepare_cached(
        "SELECT log_file_id, byte_offset, tournament_token, new_state, in_playoffs, logged_at
         FROM tournament_player_states WHERE tournament_token = ?1
         ORDER BY log_file_id, byte_offset",
    )?;
    let rows = stmt.query_map([token], |row| {
        Ok(PlayerStateRow {
            log_file_id: row.get(0)?,
            byte_offset: row.get(1)?,
            tournament_token: row.get(2)?,
            new_state: row.get(3)?,
            in_playoffs: row.get(4)?,
            logged_at: row.get(5)?,
        })
    })?;
    let mut found = Vec::new();
    for row in rows {
        found.push(row?);
    }
    Ok(found)
}

/// The state MTGO moves a tournament match to once it is over for everyone.
const CLOSED_STATE: &str = "to TournamentMatchClosedState";

/// The stored changes of `tokens` (a tournament's round match tokens) to
/// `TournamentMatchClosedState`, the first per match, as the fold's facts.
/// Another player's match is open until then; the account's own match is
/// read from its `matches` row instead, since compaction moves its events
/// out of `log_events` (spec 7.1).
pub fn closed_match_facts(
    conn: &Connection,
    tokens: &[String],
) -> Result<Vec<StandingFact>, StoreError> {
    let mut facts = Vec::new();
    if tokens.is_empty() {
        return Ok(facts);
    }
    let list = serde_json::to_string(tokens)
        .map_err(|error| StoreError::Corrupt(format!("match tokens: {error}")))?;
    // A compressed row cannot be searched, so it is read and checked.
    let mut stmt = conn.prepare_cached(
        "SELECT log_file_id, byte_offset, logged_at, raw, compressed FROM log_events
         WHERE kind = 'match_state_changed'
           AND match_token IN (SELECT value FROM json_each(?1))
           AND (compressed = 1 OR instr(raw, ?2) > 0)
         ORDER BY log_file_id, byte_offset",
    )?;
    let rows = stmt.query_map(params![list, CLOSED_STATE], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, i64>(2)?,
            row.get::<_, Vec<u8>>(3)?,
            row.get::<_, bool>(4)?,
        ))
    })?;
    let mut closed = HashSet::new();
    for row in rows {
        let (file, offset, logged_at, raw, compressed) = row?;
        let text = decode_raw(raw, compressed)?;
        let Some(TournamentFact::MatchState { match_token, to }) =
            decode_fact(EventKind::MatchStateChanged, &text, None)
        else {
            continue;
        };
        if to != "TournamentMatchClosedState" || !closed.insert(match_token.clone()) {
            continue;
        }
        facts.push(StandingFact {
            log_file_id: file,
            byte_offset: offset_of(offset)?,
            logged_at_ms: logged_at,
            fact: TournamentFact::MatchState { match_token, to },
        });
    }
    Ok(facts)
}

fn offset_of(offset: i64) -> Result<u64, StoreError> {
    u64::try_from(offset).map_err(|_| StoreError::Corrupt(format!("byte offset {offset}")))
}

/// The fold version stored with the last full refold, `None` before the
/// first one.
pub fn sync_version(conn: &Connection) -> Result<Option<i64>, StoreError> {
    Ok(conn
        .prepare_cached("SELECT version FROM derivations WHERE name = ?1")?
        .query_row([SYNC_DERIVATION], |row| row.get(0))
        .optional()?)
}

/// Records that every tournament in the window is folded at the fold's
/// `STANDING_VERSION`.
pub fn record_sync_version(conn: &Connection, now_ms: i64) -> Result<(), StoreError> {
    conn.prepare_cached(
        "INSERT INTO derivations (name, version, rebuilt_at) VALUES (?1, ?2, ?3)
         ON CONFLICT (name) DO UPDATE SET version = excluded.version,
             rebuilt_at = excluded.rebuilt_at",
    )?
    .execute(params![SYNC_DERIVATION, STANDING_VERSION, now_ms])?;
    Ok(())
}

/// How long a tournament stays in the window after its newest event (spec
/// 7.1): the overlay is not history.
pub const WINDOW_MS: i64 = 48 * 60 * 60 * 1000;

/// A tournament the account joined, as the sync finds it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Candidate {
    account_id: i64,
    username: String,
    login_id: Option<u64>,
    token: String,
}

/// The account's file user: the login's name, else the name the file took
/// over from the file before it (lines before the login).
const FILE_USER: &str = "COALESCE(f.local_username, f.inherited_username)";

/// Tokens with a stored change to a joined state (`PremierJoined*`,
/// `TournamentJoined*`, never `NotJoined`) in a file of a known account,
/// logged within two windows of `now_ms`.
/// Lobby events (only `NotJoined` states) are never candidates.
fn candidates(conn: &Connection, now_ms: i64) -> Result<Vec<Candidate>, StoreError> {
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT a.id, a.username, COALESCE(f.local_login_id, a.login_id), e.tournament_token,
             e.raw, e.compressed
         FROM log_events e
         JOIN log_files f ON f.id = e.log_file_id
         JOIN accounts a ON a.username = {FILE_USER}
         WHERE e.kind = 'tournament_state_changed' AND e.tournament_token IS NOT NULL
           AND e.quarantined = 0 AND e.logged_at >= ?1
         ORDER BY e.log_file_id, e.byte_offset"
    ))?;
    // An event lasts less than a day, so its joins are logged within two
    // windows of its newest event: older state changes are not read.
    let rows = stmt.query_map([now_ms - 2 * WINDOW_MS], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, Option<i64>>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, Vec<u8>>(4)?,
            row.get::<_, bool>(5)?,
        ))
    })?;
    let mut found: Vec<Candidate> = Vec::new();
    for row in rows {
        let (account_id, username, login_id, token, raw, compressed) = row?;
        let text = decode_raw(raw, compressed)?;
        let Some(TournamentFact::StateChanged { to }) =
            decode_fact(EventKind::TournamentStateChanged, &text, None)
        else {
            continue;
        };
        if !mtgo_protocol::tournament::is_joined_state(&to) {
            continue;
        }
        let known = found
            .iter()
            .any(|seen| seen.account_id == account_id && seen.token == token);
        if !known {
            found.push(Candidate {
                account_id,
                username,
                login_id: login_id.and_then(|id| u64::try_from(id).ok()),
                token,
            });
        }
    }
    Ok(found)
}

/// The log time of the newest stored line of `token` in the account's files,
/// of the kinds the fold reads (so a finished row's `last_event_at` reaches
/// it once folded).
fn newest_event_at(conn: &Connection, candidate: &Candidate) -> Result<Option<i64>, StoreError> {
    Ok(conn
        .prepare_cached(&format!(
            "SELECT MAX(e.logged_at) FROM log_events e
             JOIN log_files f ON f.id = e.log_file_id
             WHERE e.tournament_token = ?1 AND {FILE_USER} = ?2 AND e.quarantined = 0
               AND e.kind IN ('tournament_state_changed', 'tournament_sync',
                   'tournament_round_info', 'tournament_round_result',
                   'tournament_player_eliminated', 'tournament_ended')"
        ))?
        .query_row(params![candidate.token, candidate.username], |row| {
            row.get(0)
        })?)
}

/// The tournament event kinds the fold reads from `log_events`.
fn tournament_kind(kind: &str) -> Option<EventKind> {
    match kind {
        "tournament_state_changed" => Some(EventKind::TournamentStateChanged),
        "tournament_sync" => Some(EventKind::TournamentSync),
        "tournament_round_info" => Some(EventKind::TournamentRoundInfo),
        "tournament_round_result" => Some(EventKind::TournamentRoundResult),
        "tournament_player_eliminated" => Some(EventKind::TournamentPlayerEliminated),
        "tournament_ended" => Some(EventKind::TournamentEnded),
        _ => None,
    }
}

/// The tournament's facts from `log_events` and the round info lines decoded
/// in this read (with their log position and time). Round info lines at or
/// before `read_through` are not read: their rounds come from
/// `tournament_rounds`. (MTGO repeats a round's info in the same second, so
/// a skipped repeat moves no time.)
struct TokenFacts {
    facts: Vec<StandingFact>,
    new_rounds: Vec<(RoundPairing, LogPosition, i64)>,
    newest_round_line: Option<LogPosition>,
}

fn token_facts(
    conn: &Connection,
    candidate: &Candidate,
    read_through: Option<LogPosition>,
) -> Result<TokenFacts, StoreError> {
    let (through_file, through_offset) = match read_through {
        Some((file, offset)) => (Some(file), Some(offset)),
        None => (None, None),
    };
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT e.log_file_id, e.byte_offset, e.kind, e.logged_at,
             CASE WHEN e.kind = 'tournament_round_info'
                     AND (e.log_file_id, e.byte_offset) <= (?3, ?4)
                 THEN NULL ELSE e.raw END,
             e.compressed
         FROM log_events e
         JOIN log_files f ON f.id = e.log_file_id
         WHERE e.tournament_token = ?1 AND {FILE_USER} = ?2 AND e.quarantined = 0
         ORDER BY e.log_file_id, e.byte_offset"
    ))?;
    let rows = stmt.query_map(
        params![
            candidate.token,
            candidate.username,
            through_file,
            through_offset
        ],
        |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, Option<Vec<u8>>>(4)?,
                row.get::<_, bool>(5)?,
            ))
        },
    )?;
    let mut found = TokenFacts {
        facts: Vec::new(),
        new_rounds: Vec::new(),
        newest_round_line: read_through,
    };
    for row in rows {
        let (file, offset, kind, logged_at, raw, compressed) = row?;
        let Some(kind) = kind.as_deref().and_then(tournament_kind) else {
            continue;
        };
        let Some(raw) = raw else {
            continue;
        };
        if kind == EventKind::TournamentRoundInfo {
            found.newest_round_line = Some((file, offset));
        }
        let text = decode_raw(raw, compressed)?;
        let Some(fact) = decode_fact(kind, &text, candidate.login_id) else {
            continue;
        };
        if let TournamentFact::Round(pairing) = &fact {
            found
                .new_rounds
                .push((pairing.clone(), (file, offset), logged_at));
        }
        found.facts.push(StandingFact {
            log_file_id: file,
            byte_offset: offset_of(offset)?,
            logged_at_ms: logged_at,
            fact,
        });
    }
    Ok(found)
}

/// The account's own matches of the tournament: those linked to its token
/// and those its pairings name. Only this device's log matches.
fn own_matches(
    conn: &Connection,
    candidate: &Candidate,
    pairing_tokens: &[String],
) -> Result<Vec<OwnMatch>, StoreError> {
    let list = serde_json::to_string(pairing_tokens)
        .map_err(|error| StoreError::Corrupt(format!("match tokens: {error}")))?;
    let mut stmt = conn.prepare_cached(
        "SELECT token, state, started_at, ended_at FROM matches
         WHERE account_id = ?1 AND source = 'log'
           AND (tournament_token = ?2 OR token IN (SELECT value FROM json_each(?3)))
         ORDER BY started_at, id",
    )?;
    let rows = stmt.query_map(
        params![candidate.account_id, candidate.token, list],
        |row| {
            let state: String = row.get(1)?;
            Ok(OwnMatch {
                token: row.get(0)?,
                over: state == "complete" || state == "abandoned",
                started_at_ms: row.get(2)?,
                ended_at_ms: row.get(3)?,
            })
        },
    )?;
    let mut found = Vec::new();
    for row in rows {
        found.push(row?);
    }
    Ok(found)
}

/// What the sync needs of a stored standing.
#[derive(Debug, Clone)]
struct Stored {
    mtgo_event_id: i64,
    finished: bool,
    last_event_at: i64,
    rounds_read: Option<RoundsRead>,
}

/// The stored standing of the account's tournament `token`.
fn stored_by_token(
    conn: &Connection,
    account_id: i64,
    token: &str,
) -> Result<Option<Stored>, StoreError> {
    let found: Option<(i64, String, i64)> = conn
        .prepare_cached(
            "SELECT mtgo_event_id, status, last_event_at
             FROM tournament_standings WHERE account_id = ?1 AND token = ?2",
        )?
        .query_row(params![account_id, token], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .optional()?;
    let Some((mtgo_event_id, status, last_event_at)) = found else {
        return Ok(None);
    };
    Ok(Some(Stored {
        mtgo_event_id,
        finished: status == "finished",
        last_event_at,
        rounds_read: rounds_read_through(conn, account_id, mtgo_event_id)?,
    }))
}

/// Folds one candidate and writes what changed. True when a row changed.
fn sync_candidate(
    conn: &Connection,
    candidate: &Candidate,
    stored: Option<Stored>,
    newest: i64,
) -> Result<bool, StoreError> {
    let stored_event = stored.as_ref().map(|stored| stored.mtgo_event_id);
    let login_id = candidate.login_id.and_then(|id| i64::try_from(id).ok());
    let mut rounds_read = stored.as_ref().and_then(|stored| stored.rounds_read);
    if let (Some(event), Some(read)) = (stored_event, rounds_read)
        && read.login_id != login_id
    {
        // The account's pairing was found by another login id: decode the
        // round lines again.
        conn.prepare_cached(
            "DELETE FROM tournament_rounds WHERE account_id = ?1 AND mtgo_event_id = ?2",
        )?
        .execute([candidate.account_id, event])?;
        rounds_read = None;
    }
    let read_through = rounds_read.map(|read| read.position);
    let stored_rounds = match stored_event {
        Some(event) if read_through.is_some() => rounds(conn, candidate.account_id, event)?,
        _ => Vec::new(),
    };
    let read = token_facts(conn, candidate, read_through)?;
    let mut facts = read.facts;
    for round in &stored_rounds {
        facts.push(round.fact()?);
    }
    for state in player_states(conn, &candidate.token)? {
        facts.push(StandingFact {
            log_file_id: state.log_file_id,
            byte_offset: offset_of(state.byte_offset)?,
            logged_at_ms: state.logged_at,
            fact: TournamentFact::PlayerState {
                new_state: u32::try_from(state.new_state).unwrap_or(0),
                in_playoffs: state.in_playoffs,
            },
        });
    }
    // The fold counts open matches of the current (newest) round only, and
    // an older round's closes all come before the newest round's pairing, so
    // they move no time: only the newest round's closes are read.
    let mut newest_round: Option<&RoundPairing> = None;
    let mut own_tokens: Vec<String> = Vec::new();
    for fact in &facts {
        if let TournamentFact::Round(pairing) = &fact.fact {
            own_tokens.extend(pairing.account_match_token.iter().cloned());
            if newest_round.is_none_or(|newest| pairing.number > newest.number) {
                newest_round = Some(pairing);
            }
        }
    }
    let round_tokens = newest_round
        .map(|pairing| pairing.match_tokens.clone())
        .unwrap_or_default();
    facts.extend(closed_match_facts(conn, &round_tokens)?);
    let own = own_matches(conn, candidate, &own_tokens)?;
    let standing = fold_standing(&candidate.token, &facts, &own);

    let Some(mut row) = standing_row(candidate.account_id, &standing) else {
        return match stored_event {
            Some(event) => delete_tournament(conn, candidate.account_id, event),
            None => Ok(false),
        };
    };
    // The newest line of the tournament, even one the fold did not read (it
    // does not decode, or it repeats a round info line already decoded), so
    // a finished row is settled once nothing newer is logged.
    row.last_event_at = row.last_event_at.max(newest);
    let mut changed = false;
    if let Some(event) = stored_event
        && event != row.mtgo_event_id
    {
        // Its stored rounds went with it, so the next sync reads every round
        // line again: no read position.
        delete_tournament(conn, candidate.account_id, event)?;
        write_standing(conn, &row)?;
        return Ok(true);
    }
    changed |= write_standing(conn, &row)?;
    for (pairing, position, logged_at) in &read.new_rounds {
        let round = RoundRow::from_pairing(
            candidate.account_id,
            row.mtgo_event_id,
            pairing,
            *position,
            *logged_at,
        );
        write_round(conn, &round)?;
    }
    if let Some(position) = read.newest_round_line {
        let now_read = RoundsRead { position, login_id };
        if rounds_read_through(conn, candidate.account_id, row.mtgo_event_id)? != Some(now_read) {
            set_rounds_read_through(conn, candidate.account_id, row.mtgo_event_id, now_read)?;
        }
    }
    Ok(changed)
}

/// Brings the stored tournaments up to date with the log (spec 7.1): every
/// tournament an account joined whose newest event is inside the window is
/// folded and written when it changed; rows that left the window, or whose
/// tournament is no longer a candidate, are deleted, and so are player state
/// lines older than any window could need. A finished tournament is not
/// folded again at the current version while nothing new was logged for it
/// (a top cut loser's place comes with a later round result). A raise of the fold's
/// version folds the whole window again from the log. True when a stored
/// standing changed (round rows and read positions are bookkeeping).
pub fn sync(conn: &Connection, now_ms: i64) -> Result<bool, StoreError> {
    let current = sync_version(conn)? == Some(STANDING_VERSION);
    let mut changed = false;
    if !current {
        conn.execute("DELETE FROM tournament_rounds", [])?;
        conn.execute(
            "UPDATE tournament_standings SET rounds_read_file_id = NULL,
                 rounds_read_offset = NULL, rounds_login_id = NULL",
            [],
        )?;
    }
    conn.prepare_cached("DELETE FROM tournament_player_states WHERE logged_at < ?1")?
        .execute([now_ms - 2 * WINDOW_MS])?;
    let mut kept: Vec<(i64, String)> = Vec::new();
    for candidate in candidates(conn, now_ms)? {
        let Some(newest) = newest_event_at(conn, &candidate)? else {
            continue;
        };
        if newest < now_ms - WINDOW_MS {
            continue;
        }
        let stored = stored_by_token(conn, candidate.account_id, &candidate.token)?;
        kept.push((candidate.account_id, candidate.token.clone()));
        // A finished row moves only with new lines (a top cut loser's place
        // comes with a later round result), so it is folded again only then.
        let settled = stored
            .as_ref()
            .is_some_and(|stored| stored.finished && stored.last_event_at >= newest);
        if settled && current {
            continue;
        }
        changed |= sync_candidate(conn, &candidate, stored, newest)?;
    }
    let mut stale: Vec<(i64, i64)> = Vec::new();
    {
        let mut stmt = conn
            .prepare_cached("SELECT account_id, mtgo_event_id, token FROM tournament_standings")?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        for row in rows {
            let (account_id, event, token) = row?;
            if !kept.contains(&(account_id, token)) {
                stale.push((account_id, event));
            }
        }
    }
    for (account_id, event) in stale {
        changed |= delete_tournament(conn, account_id, event)?;
    }
    if !current {
        record_sync_version(conn, now_ms)?;
    }
    Ok(changed)
}

/// `sync` in one transaction.
pub fn sync_all(conn: &mut Connection, now_ms: i64) -> Result<bool, StoreError> {
    let tx = conn.transaction()?;
    let changed = sync(&tx, now_ms)?;
    tx.commit()?;
    Ok(changed)
}

/// What the tracker watches between syncs: the tokens of unfinished stored
/// tournaments (and of a top cut or missed cut finish still waiting for its
/// place, which a later round result gives; a drop or removal has none), and
/// the match tokens of each one's newest stored round (a
/// close there changes the matches-left count).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Watched {
    pub tournament_tokens: HashSet<String>,
    pub match_tokens: HashSet<String>,
}

pub fn watched(conn: &Connection) -> Result<Watched, StoreError> {
    let mut found = Watched::default();
    let mut stmt = conn.prepare_cached(
        "SELECT s.token, r.match_tokens FROM tournament_standings s
         LEFT JOIN tournament_rounds r ON r.account_id = s.account_id
             AND r.mtgo_event_id = s.mtgo_event_id
             AND r.round = (SELECT MAX(round) FROM tournament_rounds n
                 WHERE n.account_id = s.account_id AND n.mtgo_event_id = s.mtgo_event_id)
         WHERE s.status != 'finished'
            OR (s.finish IN ('top_cut', 'missed_cut') AND s.place IS NULL)",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
    })?;
    for row in rows {
        let (token, tokens) = row?;
        found.tournament_tokens.insert(token);
        if let Some(tokens) = tokens {
            let tokens: Vec<String> = serde_json::from_str(&tokens)
                .map_err(|error| StoreError::Corrupt(format!("round match tokens: {error}")))?;
            found.match_tokens.extend(tokens);
        }
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{account, db, event_with, new_log_file, number, store_events};
    use crate::write::{NewEvent, insert_log_file};
    use leagues::standing::{RoundOutcome, StandingRound};
    use mtgo_protocol::Subjects;

    const EVENT: i64 = 12_859_509;
    const T_TOKEN: &str = "271adf3d-46f7-4885-b5cd-52b2c24e0a93";

    fn running(account_id: i64) -> StandingRow {
        StandingRow {
            account_id,
            mtgo_event_id: EVENT,
            token: T_TOKEN.to_string(),
            name: Some("Legacy Showcase Challenge".to_string()),
            format: Some("CLEGACY".to_string()),
            cut_size: Some(8),
            swiss_rounds: Some(9),
            round_minutes: Some(50),
            starts_at: Some(1_000),
            status: StandingStatus::WaitingForRound,
            round: Some(2),
            stage: StandingStage::Swiss,
            in_playoffs: false,
            round_started_at: Some(5_000),
            round_result_at: None,
            rank: Some(139),
            players: Some(302),
            matches_left: Some(12),
            byes: 0,
            finish: None,
            place: None,
            finished_at: None,
            last_transition_at: 5_000,
            last_event_at: 5_100,
        }
    }

    fn round_one(account_id: i64) -> RoundRow {
        RoundRow {
            account_id,
            mtgo_event_id: EVENT,
            round: 1,
            bye_count: 0,
            match_tokens: vec!["q-r1-a".to_string(), "q-r1-b".to_string()],
            account_match_token: Some("q-r1-a".to_string()),
            account_bye: false,
            started_at: Some(2_000),
            match_minutes: Some(50),
            paired_position: (1, 4_096),
            paired_at: 2_000,
        }
    }

    fn fold_output() -> TournamentStanding {
        let round = |number: u32, paired: i64, result: Option<i64>| StandingRound {
            number,
            stage: StandingStage::Swiss,
            match_count: 151,
            bye_count: 0,
            account_match_token: Some(format!("q-r{number}")),
            account_bye: false,
            paired_at_ms: Some(paired),
            match_minutes: Some(50),
            result_at_ms: result,
            rank: result.map(|_| 139),
            result_rows: result.map(|_| 302),
            outcome: result.map(|_| RoundOutcome::Win),
            open_match_tokens: Vec::new(),
        };
        TournamentStanding {
            token: T_TOKEN.to_string(),
            event_id: Some(EVENT),
            name: Some("Legacy Showcase Challenge".to_string()),
            format: Some("CLEGACY".to_string()),
            cut_size: Some(8),
            swiss_rounds: Some(9),
            round_minutes: Some(50),
            starts_at_ms: Some(1_000),
            joined: true,
            status: StandingStatus::WaitingForRound,
            finish: None,
            round: Some(2),
            stage: StandingStage::Swiss,
            in_playoffs: false,
            rank: Some(139),
            players: Some(302),
            matches_left: Some(12),
            byes: 0,
            place: None,
            next_round_at_ms: None,
            finished_at_ms: None,
            last_transition_at_ms: 5_000,
            last_event_at_ms: 5_100,
            rounds: vec![round(1, 2_000, Some(4_000)), round(2, 5_000, None)],
        }
    }

    #[test]
    fn the_fold_output_maps_to_a_row_with_the_current_rounds_times() {
        let row = standing_row(7, &fold_output()).expect("a joined event");
        assert_eq!(row, running(7));
    }

    #[test]
    fn an_unjoined_unregistered_or_unkeyed_event_has_no_row() {
        let lobby = TournamentStanding {
            joined: false,
            ..fold_output()
        };
        let left = TournamentStanding {
            status: StandingStatus::Unregistered,
            ..fold_output()
        };
        let unkeyed = TournamentStanding {
            event_id: None,
            ..fold_output()
        };
        assert_eq!(standing_row(7, &lobby), None);
        assert_eq!(standing_row(7, &left), None);
        assert_eq!(standing_row(7, &unkeyed), None);
    }

    #[test]
    fn a_standing_round_trips_and_an_unchanged_write_writes_nothing() {
        let (_dir, conn) = db("store-tournament-standing");
        let account_id = account(&conn, "Qlocal");
        let row = running(account_id);

        assert!(write_standing(&conn, &row).expect("write"), "a new row");
        assert_eq!(
            standing(&conn, account_id, EVENT).expect("read"),
            Some(row.clone())
        );
        assert!(
            !write_standing(&conn, &row).expect("write again"),
            "the same row changes nothing"
        );

        let finished = StandingRow {
            status: StandingStatus::Finished,
            stage: StandingStage::Semifinal,
            finish: Some(StandingFinish::TopCut),
            place: Some(4),
            finished_at: Some(9_000),
            in_playoffs: true,
            ..row.clone()
        };
        assert!(write_standing(&conn, &finished).expect("finish"));
        assert_eq!(standings(&conn, account_id).expect("read"), vec![finished]);
        assert_eq!(standing(&conn, account_id, 1).expect("read"), None);
    }

    #[test]
    fn an_unregistered_standing_is_refused() {
        let (_dir, conn) = db("store-tournament-unregistered");
        let account_id = account(&conn, "Qlocal");
        let left = StandingRow {
            status: StandingStatus::Unregistered,
            ..running(account_id)
        };
        assert!(write_standing(&conn, &left).is_err());
        assert_eq!(
            number(&conn, "SELECT COUNT(*) FROM tournament_standings"),
            0
        );
    }

    #[test]
    fn a_standing_write_keeps_the_rounds_read_position() {
        let (_dir, conn) = db("store-tournament-read-through");
        let account_id = account(&conn, "Qlocal");
        assert_eq!(
            rounds_read_through(&conn, account_id, EVENT).expect("read"),
            None
        );
        let row = running(account_id);
        write_standing(&conn, &row).expect("write");
        let read = RoundsRead {
            position: (3, 4_096),
            login_id: Some(7),
        };
        set_rounds_read_through(&conn, account_id, EVENT, read).expect("set");

        let fewer_left = StandingRow {
            matches_left: Some(11),
            ..row
        };
        assert!(write_standing(&conn, &fewer_left).expect("write"));
        assert_eq!(
            rounds_read_through(&conn, account_id, EVENT).expect("read"),
            Some(read)
        );
        assert!(
            !write_standing(&conn, &fewer_left).expect("again"),
            "the read position is not a change of the standing"
        );
    }

    #[test]
    fn a_round_keeps_its_earliest_line_and_turns_back_into_the_folds_fact() {
        let (_dir, conn) = db("store-tournament-round");
        let account_id = account(&conn, "Qlocal");
        let info = round_one(account_id);
        assert!(write_round(&conn, &info).expect("write"));
        assert!(!write_round(&conn, &info).expect("again"), "a re-read");
        let repeat = RoundRow {
            match_tokens: vec!["q-r1-a".to_string()],
            paired_position: (1, 9_000),
            paired_at: 2_500,
            ..info.clone()
        };
        assert!(
            !write_round(&conn, &repeat).expect("repeat"),
            "a later repeat"
        );
        let earlier = RoundRow {
            paired_position: (1, 1_024),
            paired_at: 1_900,
            ..info.clone()
        };
        assert!(
            write_round(&conn, &earlier).expect("earlier"),
            "an earlier line"
        );
        assert_eq!(
            rounds(&conn, account_id, EVENT).expect("read"),
            vec![earlier.clone()]
        );

        let pairing = RoundPairing {
            event_id: Some(EVENT),
            number: 1,
            match_tokens: earlier.match_tokens.clone(),
            bye_count: 0,
            account_match_token: earlier.account_match_token.clone(),
            account_bye: false,
            match_minutes: Some(50),
            started_at_ms: Some(2_000),
        };
        assert_eq!(
            RoundRow::from_pairing(account_id, EVENT, &pairing, (1, 1_024), 1_900),
            earlier
        );
        assert_eq!(
            earlier.fact().expect("fact"),
            StandingFact {
                log_file_id: 1,
                byte_offset: 1_024,
                logged_at_ms: 1_900,
                fact: TournamentFact::Round(pairing),
            }
        );
    }

    #[test]
    fn deleting_a_tournament_takes_its_rounds_and_leaves_other_events() {
        let (_dir, conn) = db("store-tournament-delete");
        let account_id = account(&conn, "Qlocal");
        write_standing(&conn, &running(account_id)).expect("standing");
        write_round(&conn, &round_one(account_id)).expect("round");
        let other = StandingRow {
            mtgo_event_id: 7,
            ..running(account_id)
        };
        write_standing(&conn, &other).expect("other");

        assert!(delete_tournament(&conn, account_id, EVENT).expect("delete"));
        assert!(!delete_tournament(&conn, account_id, EVENT).expect("again"));
        assert_eq!(standings(&conn, account_id).expect("read"), vec![other]);
        assert_eq!(number(&conn, "SELECT COUNT(*) FROM tournament_rounds"), 0);
    }

    #[test]
    fn player_states_read_one_tournament_in_log_order() {
        let (_dir, conn) = db("store-tournament-player-states");
        let file = insert_log_file(&conn, &new_log_file("q.log")).expect("file");
        let later = PlayerStateRow {
            log_file_id: file,
            byte_offset: 900,
            tournament_token: T_TOKEN.to_string(),
            new_state: 15,
            in_playoffs: true,
            logged_at: 9_000,
        };
        let earlier = PlayerStateRow {
            byte_offset: 100,
            new_state: 16,
            in_playoffs: false,
            logged_at: 1_000,
            ..later.clone()
        };
        let lobby = PlayerStateRow {
            byte_offset: 500,
            tournament_token: "q-lobby".to_string(),
            ..later.clone()
        };
        for row in [&later, &earlier, &lobby] {
            conn.execute(
                "INSERT INTO tournament_player_states (log_file_id, byte_offset,
                     tournament_token, new_state, in_playoffs, logged_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    row.log_file_id,
                    row.byte_offset,
                    row.tournament_token,
                    row.new_state,
                    row.in_playoffs,
                    row.logged_at
                ],
            )
            .expect("state");
        }
        assert_eq!(
            player_states(&conn, T_TOKEN).expect("read"),
            vec![earlier, later]
        );
    }

    fn state_change(file: i64, offset: u64, token: &str, to: &str) -> NewEvent {
        let subjects = Subjects {
            match_token: Some(token.to_string()),
            ..Subjects::default()
        };
        event_with(
            file,
            offset,
            EventKind::MatchStateChanged,
            subjects,
            &format!(
                "(Game Management|Match State Changed for {token} from \
                 TournamentMatchNotJoinedEventUnderwayState to {to}) "
            ),
        )
    }

    #[test]
    fn only_a_change_to_the_closed_state_closes_a_round_match() {
        const A: &str = "11111111-1111-4111-8111-111111111111";
        const B: &str = "22222222-2222-4222-8222-222222222222";
        const C: &str = "33333333-3333-4333-8333-333333333333";
        const OTHER: &str = "44444444-4444-4444-8444-444444444444";
        let (_dir, mut conn) = db("store-tournament-closed");
        let file = insert_log_file(&conn, &new_log_file("q.log")).expect("file");
        store_events(
            &mut conn,
            vec![
                state_change(file, 10, A, "TournamentMatchNotJoinedSideboardingState"),
                state_change(file, 20, A, "TournamentMatchClosedState"),
                state_change(file, 30, B, "TournamentMatchNotJoinedSideboardingState"),
                state_change(file, 40, OTHER, "TournamentMatchClosedState"),
            ],
        );
        let round = vec![A.to_string(), B.to_string(), C.to_string()];

        let closed = closed_match_facts(&conn, &round).expect("read");
        assert_eq!(
            closed,
            vec![StandingFact {
                log_file_id: file,
                byte_offset: 20,
                logged_at_ms: 1_020,
                fact: TournamentFact::MatchState {
                    match_token: A.to_string(),
                    to: "TournamentMatchClosedState".to_string(),
                },
            }]
        );
        assert!(closed_match_facts(&conn, &[]).expect("none").is_empty());
    }

    const SHOWCASE: &str = "6ab6648d-3371-4daa-ae6b-e81bb8001148";
    /// The fixture account's login id.
    const LOGIN_ID: i64 = 900_001;

    /// The Modern Challenge, seen from the lobby in the fixture.
    const MODERN: &str = "63c90ead-8c2c-407d-9f10-67378e0b96d0";
    /// A Modern Challenge player who lost in the top 8 and placed 5th.
    const MODERN_LOGIN_ID: i64 = 9_000_641;

    /// Stores the Challenge fixture as the tracker does (the account's own
    /// Showcase Challenge), syncing after every tournament event (`each`) or
    /// once at the end. Returns the account and the fixture's `now`.
    fn ingest_challenge(conn: &mut Connection, each: bool) -> (i64, i64) {
        ingest_challenge_as(conn, each, LOGIN_ID, None)
    }

    /// `ingest_challenge` with `login_id` as the account's, and `joined`'s
    /// lobby state changes made joined ones (`NotJoined` becomes `XxxJoined`,
    /// the same length), so another player's event reads as the account's.
    fn ingest_challenge_as(
        conn: &mut Connection,
        each: bool,
        login_id: i64,
        joined: Option<&str>,
    ) -> (i64, i64) {
        let mut fixture = fixture_tools::load_fixture("../tournaments/challenge_showcase");
        if let Some(token) = joined {
            let text = String::from_utf8(fixture.bytes.clone()).expect("utf-8");
            let marker = format!("Tournament State Changed for {token}");
            let lines: Vec<String> = text
                .split_inclusive('\n')
                .map(|line| {
                    if line.contains(&marker) {
                        line.replace("NotJoined", "XxxJoined")
                    } else {
                        line.to_string()
                    }
                })
                .collect();
            fixture.bytes = lines.concat().into_bytes();
        }
        let now = fixture.meta.now_ms();
        let mut file = new_log_file("Q/mtgo.log");
        file.inherited_username = Some(fixture.meta.local_username.clone());
        file.timezone = fixture.meta.timezone.clone();
        let file_id = insert_log_file(conn, &file).expect("file");
        conn.execute(
            "UPDATE log_files SET local_login_id = ?1 WHERE id = ?2",
            [login_id, file_id],
        )
        .expect("login id");
        let account_id = account(conn, &fixture.meta.local_username);
        for event in fixture_tools::prepared_events(&fixture) {
            let tournament = event.subjects.tournament_token.is_some();
            store_events(
                conn,
                vec![NewEvent {
                    log_file_id: file_id,
                    byte_offset: event.offset,
                    kind: Some(event.kind),
                    subjects: event.subjects.clone(),
                    logged_at_ms: event.at_ms,
                    raw: event.text.clone(),
                    quarantined: false,
                }],
            );
            if each && tournament {
                sync_all(conn, now).expect("sync");
            }
        }
        sync_all(conn, now).expect("sync");
        (account_id, now)
    }

    /// Every stored tournament row, as text, for comparing two databases.
    fn dump(conn: &Connection) -> Vec<String> {
        let mut found = Vec::new();
        for query in [
            "SELECT * FROM tournament_standings ORDER BY account_id, mtgo_event_id",
            "SELECT * FROM tournament_rounds ORDER BY account_id, mtgo_event_id, round",
        ] {
            let mut stmt = conn.prepare(query).expect("query");
            let columns = stmt.column_count();
            let rows = stmt
                .query_map([], |row| {
                    let mut cells = Vec::new();
                    for index in 0..columns {
                        let cell: rusqlite::types::Value = row.get(index)?;
                        cells.push(format!("{cell:?}"));
                    }
                    Ok(cells.join("|"))
                })
                .expect("rows");
            for row in rows {
                found.push(row.expect("row"));
            }
        }
        found
    }

    #[test]
    fn the_sync_stores_the_joined_challenge_only_and_a_second_sync_writes_nothing() {
        let (_dir, mut conn) = db("store-tournament-sync");
        let (account_id, now) = ingest_challenge(&mut conn, false);
        let rows = standings(&conn, account_id).expect("read");
        assert_eq!(rows.len(), 1, "the lobby events have no row: {rows:?}");
        let showcase = &rows[0];
        assert_eq!(showcase.token, SHOWCASE);
        assert!(showcase.round.is_some_and(|round| round >= 1));
        let stored = rounds(&conn, account_id, showcase.mtgo_event_id).expect("rounds");
        assert!(!stored.is_empty());
        assert!(
            stored
                .iter()
                .any(|round| round.account_match_token.is_some())
        );
        assert!(
            rounds_read_through(&conn, account_id, showcase.mtgo_event_id)
                .expect("position")
                .is_some()
        );

        let before = dump(&conn);
        assert!(!sync_all(&mut conn, now).expect("again"), "nothing new");
        assert_eq!(dump(&conn), before);
    }

    #[test]
    fn a_sync_per_event_equals_one_sync_at_the_end() {
        let (_one_dir, mut once) = db("store-tournament-once");
        ingest_challenge(&mut once, false);
        let (_each_dir, mut each) = db("store-tournament-each");
        ingest_challenge(&mut each, true);
        assert_eq!(dump(&each), dump(&once));
    }

    #[test]
    fn a_top_cut_finish_gets_its_place_from_a_later_result_live_as_rebuilt() {
        let (_one_dir, mut once) = db("store-tournament-place-once");
        let (account_id, _) = ingest_challenge_as(&mut once, false, MODERN_LOGIN_ID, Some(MODERN));
        let (_each_dir, mut each) = db("store-tournament-place-each");
        ingest_challenge_as(&mut each, true, MODERN_LOGIN_ID, Some(MODERN));
        assert_eq!(dump(&each), dump(&once));
        let modern = standings(&each, account_id)
            .expect("read")
            .into_iter()
            .find(|row| row.token == MODERN)
            .expect("the Modern Challenge");
        assert_eq!(
            (modern.status, modern.finish, modern.place),
            (
                StandingStatus::Finished,
                Some(StandingFinish::TopCut),
                Some(5)
            )
        );
    }

    #[test]
    fn another_login_id_decodes_the_rounds_again() {
        let (_dir, mut conn) = db("store-tournament-login");
        let (account_id, now) = ingest_challenge(&mut conn, false);
        let event = standings(&conn, account_id).expect("read")[0].mtgo_event_id;
        let paired = |conn: &Connection| {
            rounds(conn, account_id, event)
                .expect("rounds")
                .iter()
                .filter(|round| round.account_match_token.is_some())
                .count()
        };
        assert!(paired(&conn) > 0);

        conn.execute("UPDATE log_files SET local_login_id = 1", [])
            .expect("another login id");
        sync_all(&mut conn, now).expect("sync");
        assert_eq!(paired(&conn), 0, "no pairing names login id 1");
        assert_eq!(
            rounds_read_through(&conn, account_id, event)
                .expect("read")
                .map(|read| read.login_id),
            Some(Some(1))
        );
    }

    #[test]
    fn the_candidate_scan_uses_the_state_change_index() {
        let (_dir, conn) = db("store-tournament-plan");
        let plan: Vec<String> = conn
            .prepare(
                "EXPLAIN QUERY PLAN SELECT e.tournament_token FROM log_events e
                 WHERE e.kind = 'tournament_state_changed' AND e.tournament_token IS NOT NULL
                   AND e.quarantined = 0 AND e.logged_at >= ?1",
            )
            .expect("plan")
            .query_map([0], |row| row.get::<_, String>(3))
            .expect("rows")
            .map(|row| row.expect("row"))
            .collect();
        assert!(
            plan.iter()
                .any(|step| step.contains("log_events_tournament_state_changes")),
            "{plan:?}"
        );
    }

    #[test]
    fn a_finished_row_is_not_refolded_until_the_version_rises() {
        let (_dir, mut conn) = db("store-tournament-finished");
        let (account_id, now) = ingest_challenge(&mut conn, false);
        let folded = dump(&conn);
        conn.execute(
            "UPDATE tournament_standings SET status = 'finished', finish = 'missed_cut',
                 place = 21, name = 'q-kept'",
            [],
        )
        .expect("finish it");
        assert!(!sync_all(&mut conn, now).expect("sync"));
        assert_eq!(
            standings(&conn, account_id).expect("read")[0]
                .name
                .as_deref(),
            Some("q-kept"),
            "a finished row at the current version is not folded again"
        );

        conn.execute(
            "UPDATE derivations SET version = 0 WHERE name = ?1",
            [SYNC_DERIVATION],
        )
        .expect("an older version");
        assert!(sync_all(&mut conn, now).expect("refold"));
        assert_eq!(
            dump(&conn),
            folded,
            "a version raise folds the window again"
        );
        assert_eq!(
            sync_version(&conn).expect("version"),
            Some(STANDING_VERSION)
        );
    }

    #[test]
    fn a_tournament_leaves_with_the_window() {
        let (_dir, mut conn) = db("store-tournament-window");
        let (account_id, now) = ingest_challenge(&mut conn, false);
        assert!(!standings(&conn, account_id).expect("read").is_empty());
        assert!(sync_all(&mut conn, now + WINDOW_MS + 24 * 60 * 60 * 1000).expect("later"));
        assert!(standings(&conn, account_id).expect("read").is_empty());
        assert_eq!(number(&conn, "SELECT COUNT(*) FROM tournament_rounds"), 0);
    }

    #[test]
    fn the_tracker_watches_unfinished_tournaments_and_their_newest_round() {
        let (_dir, mut conn) = db("store-tournament-watched");
        let (account_id, _now) = ingest_challenge(&mut conn, false);
        let showcase = standings(&conn, account_id).expect("read").remove(0);
        let newest = rounds(&conn, account_id, showcase.mtgo_event_id)
            .expect("rounds")
            .pop()
            .expect("a round");
        let found = watched(&conn).expect("watched");
        assert_eq!(
            found.tournament_tokens,
            HashSet::from([SHOWCASE.to_string()])
        );
        assert_eq!(
            found.match_tokens,
            newest.match_tokens.into_iter().collect::<HashSet<_>>()
        );

        conn.execute(
            "UPDATE tournament_standings SET status = 'finished', finish = 'missed_cut'",
            [],
        )
        .expect("finish it");
        assert_eq!(
            watched(&conn).expect("watched").tournament_tokens,
            HashSet::from([SHOWCASE.to_string()]),
            "a finish waiting for its place is still watched"
        );
        conn.execute("UPDATE tournament_standings SET place = 21", [])
            .expect("its place");
        assert_eq!(watched(&conn).expect("watched"), Watched::default());
        conn.execute(
            "UPDATE tournament_standings SET finish = 'dropped', place = NULL",
            [],
        )
        .expect("dropped");
        assert_eq!(
            watched(&conn).expect("watched"),
            Watched::default(),
            "a drop never gets a place: not watched"
        );
    }

    #[test]
    fn a_finished_row_settles_on_a_newest_line_the_fold_does_not_read() {
        let (_dir, mut conn) = db("store-tournament-settle");
        let (account_id, now) = ingest_challenge(&mut conn, false);
        let newest = number(&conn, "SELECT MAX(logged_at) FROM log_events") + 1_000;
        conn.execute(
            "INSERT INTO log_events (log_file_id, byte_offset, kind, tournament_token,
                 logged_at, raw)
             VALUES ((SELECT MIN(id) FROM log_files), 99999999, 'tournament_round_result',
                 ?1, ?2, CAST('a line that does not decode' AS BLOB))",
            params![SHOWCASE, newest],
        )
        .expect("an undecodable line");
        sync_all(&mut conn, now).expect("sync");
        let stored = standings(&conn, account_id).expect("read").remove(0);
        assert_eq!(stored.last_event_at, newest, "the newest line, read or not");

        conn.execute(
            "UPDATE tournament_standings SET status = 'finished', finish = 'dropped',
                 name = 'q-kept'",
            [],
        )
        .expect("finish it");
        assert!(!sync_all(&mut conn, now).expect("sync"));
        assert_eq!(
            standings(&conn, account_id).expect("read")[0]
                .name
                .as_deref(),
            Some("q-kept"),
            "settled: nothing newer than its last event"
        );
    }

    #[test]
    fn the_sync_version_is_the_folds() {
        let (_dir, conn) = db("store-tournament-version");
        assert_eq!(sync_version(&conn).expect("read"), None);
        record_sync_version(&conn, 77).expect("record");
        assert_eq!(sync_version(&conn).expect("read"), Some(STANDING_VERSION));
    }
}
