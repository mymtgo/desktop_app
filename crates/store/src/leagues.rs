//! League reads and edits (leagues spec sections 4 to 6, target schema
//! section 8).
//!
//! The pages read the stored runs (`stored_runs`: `leagues` rows with their
//! listed members, `matches.league_id`, and their games) and tournaments
//! (`stored_tournaments`), which the league sync (`league_sync`) writes. The
//! Limited pages read the stored draft and sealed runs
//! (`stored_limited_runs`). Every read is one query per kind of row (runs,
//! matches, games, events), never one per match.
//!
//! Edits (`LeagueEdit`) are user actions whose caller needs an answer, so,
//! like `DeckEdit`, they go through `WriterHandle::edit_league` and commit in
//! their own transaction. A move, a removal from a league, a status, a
//! delete, an Undo and a Restore change the fold's input or what the pages
//! show, so they sync the owners they touch before they commit: the pages
//! show the edit at once. A refused edit changes nothing and carries its
//! message (0.x's where 0.x had the edit). League self-management spec
//! section 4 has the rules.

use std::collections::{BTreeSet, HashMap};

use chrono_tz::Tz;
/// The kind of a stored run (`RunFilter::kinds`, `StoredRun::kind`).
pub use leagues::LeagueKind;
use leagues::{
    GameResult, LeagueInput, LeagueMatch, OrderedInput, PanelFormat, Run, RunInput, RunMatch,
    RunState, TournamentMatch,
};
use leagues::{arena_course_id, is_arena_run_key, is_limited_format_code, panel_round_count};
use limited::{
    ABANDON_AFTER_MS, CardFacts, CardFactsMap, Draft, DraftState, RegisteredDeck, panel_play_format,
};
use mtgo_protocol::parse_login;
use rusqlite::{Connection, OptionalExtension, ToSql, Transaction, params};

use crate::league_sync::{LeagueOwner, sync_owner};
use crate::limited::{registered, stored_cards, stored_drafts};
use crate::names::{state_from_str, stored_game_result, stored_outcome, stored_state};
use crate::write::decode_raw;
use crate::{Platform, StoreError};

/// The most matches a manual league holds (0.x `LinkMatchController`).
pub const MANUAL_MAX_MATCHES: usize = 5;
/// The longest manual league name, in characters (0.x `max:100`).
pub const MAX_LEAGUE_NAME_CHARS: usize = 100;
/// The longest league note, in characters (0.x `max:5000`).
pub const MAX_NOTES_CHARS: usize = 5000;
/// At most this many matches are offered by "Add matches…" (0.x
/// `AvailableMatchesController`).
pub const AVAILABLE_LIMIT: u32 = 100;
const AVAILABLE_LIMIT_SQL: &str = "LIMIT 100";

/// Where a match sits among the log lines of its start millisecond: after
/// every one of them (the panel view that preceded it is logged first), but
/// before the fold's completion steps. The matches table keeps no log
/// position, and a compacted match has no events left to give one.
pub const MATCH_POSITION: (u64, u64) = (u64::MAX - 1, 0);

/// The run key of a manual league.
pub fn manual_run_key(id: i64) -> String {
    format!("manual:{id}")
}

/// The run key of an imported run: a 0.x league (its 0.x `leagues.id`) or a
/// league pulled from the cloud (its synthetic legacy id). The `0x:` prefix is
/// only a key, never shown to the player.
pub fn imported_run_key(id: i64) -> String {
    format!("0x:{id}")
}

/// One match a league run or a tournament can hold, with its deck and games.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeagueMatchRow {
    pub token: String,
    /// `log`, `imported` or `manual`.
    pub source: String,
    pub league_token: Option<String>,
    /// `PlayFormatCd` (`CMODERN`).
    pub format: Option<String>,
    pub game_structure: Option<String>,
    pub tournament_event_id: Option<i64>,
    pub tournament_round: Option<i64>,
    pub state: String,
    pub outcome: String,
    pub started_at: i64,
    pub ended_at: Option<i64>,
    pub opponent: Option<String>,
    pub deck_version_id: Option<i64>,
    pub deck_id: Option<i64>,
    /// In game order.
    pub games: Vec<GameResult>,
    /// The opponent seat's archetype (0.x `opponentArchetypeId`,
    /// `opponentArchetype`).
    pub opponent_archetype_id: Option<i64>,
    pub opponent_archetype: Option<String>,
}

impl LeagueMatchRow {
    /// The match as the league numbers take it.
    pub fn run_match(&self) -> RunMatch {
        RunMatch {
            token: self.token.clone(),
            state: stored_state(&self.state),
            outcome: stored_outcome(&self.outcome),
            started_at: self.started_at,
            ended_at: self.ended_at,
            deck_version_id: self.deck_version_id,
            deck_id: self.deck_id,
            games: self.games.clone(),
            opponent_archetype: self.opponent_archetype.clone(),
        }
    }

    /// The match as the tournament numbers take it.
    pub fn tournament_match(&self) -> TournamentMatch {
        TournamentMatch {
            token: self.token.clone(),
            state: stored_state(&self.state),
            outcome: stored_outcome(&self.outcome),
            started_at: self.started_at,
            ended_at: self.ended_at,
            deck_version_id: self.deck_version_id,
            round: self.tournament_round,
            games: self.games.clone(),
        }
    }
}

/// Which stored runs a read takes (`stored_runs`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunFilter {
    /// Whose runs: an account's, or (`Nobody`) those with no account.
    pub owners: Vec<LeagueOwner>,
    pub kinds: Vec<LeagueKind>,
    /// Only runs of this deck (`leagues.deck_id`).
    pub deck_id: Option<i64>,
    /// Only the run with this key.
    pub run_key: Option<String>,
}

/// One stored league run (a `leagues` row that is not removed
/// (`listed_leagues`) and not detached) with its listed matches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredRun {
    pub id: i64,
    /// The run key: notes, drops and links name the run by it.
    pub key: String,
    pub account_id: Option<i64>,
    /// `log`, `imported` or `manual`.
    pub source: String,
    pub kind: LeagueKind,
    pub state: RunState,
    pub name: String,
    /// `None` for a manual league.
    pub token: Option<String>,
    pub event_id: Option<i64>,
    pub mtgo_course_id: Option<i64>,
    pub set_code: Option<String>,
    /// The format code (`CMODERN`).
    pub format: Option<String>,
    pub deck_version_id: Option<i64>,
    pub deck_id: Option<i64>,
    pub started_at: i64,
    pub joined_at: Option<i64>,
    pub completed_at: Option<i64>,
    pub dropped_at: Option<i64>,
    pub created_at: i64,
    pub notes: Option<String>,
    /// The player's status (`leagues.user_state`); `None` is Automatic.
    pub user_state: Option<UserState>,
    /// The matches its league's panel states (`leagues.rounds`); `None`
    /// takes the kind's default.
    pub rounds: Option<usize>,
    /// An Arena course run's stated record (`course_wins`, `course_losses`),
    /// matches played elsewhere included; `None` for every other run.
    pub course_record: Option<(u32, u32)>,
    /// Its listed matches (`matches.league_id`), by start, with their games.
    pub matches: Vec<LeagueMatchRow>,
}

impl StoredRun {
    /// The state the pages show: the player's status, else the fold's
    /// (spec 3.2). Overlays and the "current run" rules read `state`.
    pub fn shown_state(&self) -> RunState {
        self.user_state.map_or(self.state, UserState::run_state)
    }

    /// The run as the league numbers read it: the shown state (the
    /// player's status, else the fold's). The "current run" rules reset it
    /// to the fold's `state` (`fold_input` in the app).
    pub fn run_input(&self) -> RunInput {
        RunInput {
            course_record: self.course_record,
            state: self.shown_state(),
            manual: self.source == "manual",
            uncapped: false,
            format: self.format.clone(),
            started_at: self.started_at,
            matches: self.matches.iter().map(LeagueMatchRow::run_match).collect(),
        }
    }
}

/// One tournament of an account (a `tournaments` row): its matches (only the
/// deck's when a deck was asked for).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TournamentGroup {
    pub event_id: i64,
    /// The row's name: while `name_synthesized`, `Tournament {eventId}
    /// ({d-m-Y})`.
    pub name: String,
    /// The account's first match in the tournament, whatever its deck.
    pub started_at: i64,
    /// That first match's format code.
    pub format: Option<String>,
    /// By start.
    pub matches: Vec<LeagueMatchRow>,
}

/// A user edit of league data (spec 5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LeagueEdit {
    /// 0.x `StoreController`: a manual league of one of the account's decks
    /// that is not archived. `name` is trimmed.
    CreateManual {
        account_id: i64,
        deck_id: i64,
        name: String,
        /// The league's `manual_` token (`manual_league_token`).
        token: String,
        started_at: i64,
        now_ms: i64,
    },
    /// "Add matches…" (spec 4.1): move complete or abandoned matches of the
    /// league's account and deck into a league that is not removed, from any
    /// other league or none. A manual league holds at most five.
    LinkMatches {
        run_key: String,
        match_tokens: Vec<String>,
        /// The time the edit's league sync folds at, and the matches'
        /// `edited_at`.
        now_ms: i64,
    },
    /// "Remove from league" (spec 4.2): the match leaves `run_key` and every
    /// league (`no_league`), until it is moved into one again.
    UnlinkMatch {
        run_key: String,
        match_token: String,
        now_ms: i64,
    },
    /// 0.x `UpdateNotesController`: trimmed; empty clears.
    SetNotes {
        run_key: String,
        notes: String,
        now_ms: i64,
    },
    /// "Set status" (spec 4.3): `None` is Automatic. Dropped goes through
    /// the fold (`user_state = 'dropped'` with `user_dropped_at`, the first
    /// drop time kept; this does not drop the player on MTGO); the others
    /// are read-time only.
    SetStatus {
        run_key: String,
        status: Option<UserState>,
        now_ms: i64,
    },
    /// "Delete league…" (spec 4.4), refused while it shows Active.
    Delete {
        run_key: String,
        matches: DeleteMatches,
        now_ms: i64,
    },
    /// The Undo of the toast after a delete (spec 4.5): `removed_at` is the
    /// delete's time. It names the league by its row id, which survives a
    /// rekey (sync identity spec 2.5): a run key does not. The caller reads
    /// the row's run key now (`read::league_run_key`) and puts a previous
    /// status back with `SetStatus`.
    UndoDelete {
        league_id: i64,
        removed_at: i64,
        now_ms: i64,
    },
    /// "Restore" in Settings, Archives, Deleted (spec 4.5): the league and
    /// the matches its "Delete them too" hid come back, as with Undo.
    Restore { run_key: String, now_ms: i64 },
    /// "Move to league…" on a match row (spec 5.4): into `to_run_key` as
    /// `LinkMatches` does, or (`None`) out of every league as `UnlinkMatch`
    /// does.
    MoveMatch {
        match_token: String,
        to_run_key: Option<String>,
        now_ms: i64,
    },
}

/// What happened to a `LeagueEdit`. Only `Done` changed anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeagueEditOutcome {
    Done,
    /// There is no league with this run key.
    NotFound,
    NameRequired,
    NameTooLong,
    /// The deck is not one of the account's, or it is archived.
    InvalidDeck,
    NoMatches,
    TooManyMatches,
    MatchNotFound,
    WrongDeck,
    NotInLeague,
    NotesTooLong,
    /// The league shows Active: drop it or set another status first.
    StillActive,
    /// The player deleted the league.
    LeagueRemoved,
    /// A tournament match belongs to its tournament.
    Tournament,
    /// Only complete or abandoned matches move.
    NotFinished,
    /// The match is another account's than the league.
    WrongAccount,
    /// A constructed match and a draft or sealed run, or the other way round.
    WrongKind,
    /// A Restore or Undo that would leave the league unlisted (detached or
    /// gone): no match is left in it (each was restored on its own and came
    /// back non-league, or is still deleted on its own), so no fold run or
    /// linked match holds it. Refused, so it stays in Archives.
    NothingToShow,
    /// The edit touches an Arena run, which takes only a status for now
    /// (Arena limited runs spec, call 4; #834).
    ArenaRun,
}

impl LeagueEditOutcome {
    /// The message to show for a refusal (0.x's where 0.x has one), `None`
    /// for `Done`.
    pub fn refusal(self) -> Option<&'static str> {
        match self {
            LeagueEditOutcome::Done => None,
            LeagueEditOutcome::NotFound => Some("This league no longer exists."),
            LeagueEditOutcome::NameRequired => Some("The name field is required."),
            LeagueEditOutcome::NameTooLong => {
                Some("The name field must not be greater than 100 characters.")
            }
            LeagueEditOutcome::InvalidDeck => Some("The selected deck id is invalid."),
            LeagueEditOutcome::NoMatches => Some("No matches selected."),
            LeagueEditOutcome::TooManyMatches => Some("League would exceed 5 matches."),
            LeagueEditOutcome::MatchNotFound => Some("One or more matches not found."),
            LeagueEditOutcome::WrongDeck => {
                Some("A selected match does not match the league deck.")
            }
            LeagueEditOutcome::NotInLeague => Some("Match does not belong to this league."),
            LeagueEditOutcome::NotesTooLong => {
                Some("The notes field must not be greater than 5000 characters.")
            }
            LeagueEditOutcome::StillActive => Some("Drop it first"),
            LeagueEditOutcome::LeagueRemoved => Some("That league was deleted"),
            LeagueEditOutcome::Tournament => Some("Tournament matches stay in their tournament"),
            LeagueEditOutcome::NotFinished => Some("Only finished matches can be moved"),
            LeagueEditOutcome::WrongAccount => Some("That match is another account's"),
            LeagueEditOutcome::WrongKind => Some("That match is a different kind of league match"),
            LeagueEditOutcome::NothingToShow => {
                Some("No matches are left in this league, so it has nothing to show.")
            }
            LeagueEditOutcome::ArenaRun => Some("Arena runs cannot be edited yet."),
        }
    }
}

/// The player's status for a league (`leagues.user_state`, spec 3.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserState {
    Active,
    Complete,
    Partial,
    Dropped,
}

impl UserState {
    pub fn as_str(self) -> &'static str {
        match self {
            UserState::Active => "active",
            UserState::Complete => "complete",
            UserState::Partial => "partial",
            UserState::Dropped => "dropped",
        }
    }

    /// The stored text's status; `None` for any other text.
    pub fn parse(text: &str) -> Option<UserState> {
        match text {
            "active" => Some(UserState::Active),
            "complete" => Some(UserState::Complete),
            "partial" => Some(UserState::Partial),
            "dropped" => Some(UserState::Dropped),
            _ => None,
        }
    }

    /// The run state a league with this status shows.
    pub fn run_state(self) -> RunState {
        match self {
            UserState::Active => RunState::Active,
            UserState::Complete => RunState::Complete,
            UserState::Partial => RunState::Partial,
            UserState::Dropped => RunState::Dropped,
        }
    }
}

/// A stored `user_state` (NULL is Automatic).
fn stored_user_state(text: Option<String>) -> Result<Option<UserState>, StoreError> {
    match text {
        None => Ok(None),
        Some(text) => match UserState::parse(&text) {
            Some(state) => Ok(Some(state)),
            None => Err(StoreError::Corrupt(format!("league user state {text:?}"))),
        },
    }
}

/// What "Delete league…" does with the league's matches (spec 4.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeleteMatches {
    /// They stay as they are; every read shows them as non-league matches
    /// while the league is removed.
    KeepAsNonLeague,
    /// Each is deleted (hidden) with the league's time.
    DeleteToo,
}

/// The rows one read takes: `FROM {from} WHERE {filter} {order} {limit}`,
/// where `m` is the match and `group` names the run or league a row belongs
/// to.
pub(crate) struct Scope {
    pub(crate) from: &'static str,
    pub(crate) filter: String,
    pub(crate) order: &'static str,
    pub(crate) limit: &'static str,
    pub(crate) group: &'static str,
}

/// On `listed_leagues tl` and `listed_matches m`: the matches a move into
/// league `?1` may take (spec 4.1) as SQL can say it: the league's account
/// (any for a league without one), not a tournament round, complete or
/// abandoned, not in this league already. The kind and deck are added by
/// each scope; `fit_refusal` settles the set code in Rust.
const AVAILABLE_RULES: &str = "tl.id = ?1 AND tl.detached_at IS NULL
     AND (tl.account_id IS NULL OR m.account_id = tl.account_id)
     AND m.tournament_event_id IS NULL AND m.state IN ('complete', 'abandoned')
     AND m.league_id IS NOT tl.id";

/// Whether the listed match `m` reads as a league match (league
/// self-management spec 3.3): it points at a league the player has not
/// deleted and the league pages list (not detached), or it is a league
/// match the sync places by its token (not placed yet) that the player has
/// not taken out of every league. A match whose league is deleted is a
/// non-league match to every read.
pub(crate) const SHOWN_IN_A_LEAGUE: &str = "(EXISTS (SELECT 1 FROM listed_leagues sl
         WHERE sl.id = m.league_id AND sl.detached_at IS NULL)
     OR (m.league_id IS NULL AND m.no_league = 0 AND m.league_token IS NOT NULL))";

/// A constructed league with a deck: matches of any version of its deck
/// (0.x `AvailableMatchesController`) that are not limited (`fit_refusal`'s
/// kind rule), newest first. Every one fits, so the limit can be SQL's. The
/// group is the league the match is in now.
fn available_by_deck_scope() -> Scope {
    Scope {
        from: "listed_leagues tl
         JOIN deck_versions dv ON dv.deck_id = tl.deck_id
         JOIN listed_matches m ON m.deck_version_id = dv.id",
        filter: format!("{AVAILABLE_RULES} AND NOT {LIMITED_MATCH_SQL}"),
        order: "ORDER BY m.started_at DESC, m.id DESC",
        limit: AVAILABLE_LIMIT_SQL,
        group: "m.league_id",
    }
}

/// On `listed_matches m`: whether the match reads as limited play, as
/// `decks::is_limited_match_format` (`^(D[A-Z]|S[0-9])`). A match with no
/// format is not limited.
const LIMITED_MATCH_SQL: &str = "COALESCE(
         (substr(m.format, 1, 1) = 'D' AND substr(m.format, 2, 1) BETWEEN 'A' AND 'Z')
         OR (substr(m.format, 1, 1) = 'S' AND substr(m.format, 2, 1) BETWEEN '0' AND '9'),
         0)";

/// A league without a deck, or a draft or sealed run (`fit_refusal`'s kind
/// rule in SQL): a constructed league takes matches that are not limited,
/// a limited run takes limited ones. A limited run that knows its set also
/// needs the set code inside the match's format code (`?2`); SQL cannot
/// derive the code the way `set_code_from_play_format` does, so that case
/// is narrowed here and settled in Rust, with its limit. The rest fits
/// whole, so SQL keeps the limit. Such leagues are rare (0.x leagues
/// without a deck, limited runs) and this read runs only when the picker
/// opens. `LeagueFit::sql_set_code` decides both the set rule here and
/// whether `?2` is bound.
fn available_any_scope(fit: &LeagueFit) -> Scope {
    let kind_rule = if fit.kind != LeagueKind::Constructed {
        LIMITED_MATCH_SQL.to_string()
    } else {
        format!("NOT {LIMITED_MATCH_SQL}")
    };
    let by_set = fit.sql_set_code().is_some();
    let set_rule = if by_set {
        " AND instr(m.format, ?2) > 0"
    } else {
        ""
    };
    Scope {
        from: "listed_leagues tl JOIN listed_matches m",
        filter: format!("{AVAILABLE_RULES} AND {kind_rule}{set_rule}"),
        order: "ORDER BY m.started_at DESC, m.id DESC",
        limit: if by_set { "" } else { AVAILABLE_LIMIT_SQL },
        group: "m.league_id",
    }
}

/// League join and drop events, with their file's inherited username.
const LEAGUE_EVENTS: &str = "
SELECT e.log_file_id, e.byte_offset, e.kind, e.league_token, e.league_event_id, e.logged_at,
       f.inherited_username, CASE WHEN e.kind = 'league_joined' THEN e.raw END, e.compressed
FROM log_events e JOIN log_files f ON f.id = e.log_file_id
WHERE e.kind IN ('league_joined', 'league_dropped', 'league_join_request')
  AND e.quarantined = 0";

/// Every Login event, to find the local player at a league event.
const ALL_LOGINS: &str = "SELECT log_file_id, byte_offset, raw, compressed FROM log_events
     WHERE kind = 'login' AND quarantined = 0";

fn rows_sql(scope: &Scope) -> String {
    format!(
        "SELECT m.id, m.token, m.source, m.league_token, m.format, m.game_structure,
             m.tournament_event_id, m.tournament_round, m.state, m.outcome, m.started_at,
             m.ended_at, COALESCE(o.display_name, o.username), m.deck_version_id, v.deck_id, {group}, oa.id, oa.name
         FROM {from}
         LEFT JOIN opponents o ON o.id = m.opponent_id
         LEFT JOIN deck_versions v ON v.id = m.deck_version_id
         LEFT JOIN match_archetypes ma ON ma.match_id = m.id AND ma.is_opponent = 1
         LEFT JOIN archetypes oa ON oa.id = ma.archetype_id
         WHERE {filter}
         {order} {limit}",
        group = scope.group,
        from = scope.from,
        filter = scope.filter,
        order = scope.order,
        limit = scope.limit,
    )
}

fn games_sql(scope: &Scope) -> String {
    // The order matters to the subquery only when a limit picks the rows.
    let order_limit = if scope.limit.is_empty() {
        String::new()
    } else {
        format!("{} {}", scope.order, scope.limit)
    };
    format!(
        "SELECT g.match_id, g.result, g.on_play, g.started_at, g.ended_at
         FROM games g
         WHERE g.match_id IN (SELECT m.id FROM {from} WHERE {filter} {order_limit})
         ORDER BY g.match_id, g.game_number, g.id",
        from = scope.from,
        filter = scope.filter,
    )
}

/// A match row read by `rows_sql`, before its games.
struct ReadRow {
    match_id: i64,
    group: Option<i64>,
    row: LeagueMatchRow,
}

/// The rows of one scope with their games: two queries, whatever the count.
/// Each row comes with its group (its league or run, `None` for no group).
pub(crate) fn match_rows(
    conn: &Connection,
    scope: &Scope,
    params: &[&dyn ToSql],
) -> Result<Vec<(Option<i64>, LeagueMatchRow)>, StoreError> {
    let mut read: Vec<ReadRow> = Vec::new();
    {
        let mut stmt = conn.prepare_cached(&rows_sql(scope))?;
        let rows = stmt.query_map(params, |row| {
            Ok(ReadRow {
                match_id: row.get(0)?,
                group: row.get(15)?,
                row: LeagueMatchRow {
                    token: row.get(1)?,
                    source: row.get(2)?,
                    league_token: row.get(3)?,
                    format: row.get(4)?,
                    game_structure: row.get(5)?,
                    tournament_event_id: row.get(6)?,
                    tournament_round: row.get(7)?,
                    state: row.get(8)?,
                    outcome: row.get(9)?,
                    started_at: row.get(10)?,
                    ended_at: row.get(11)?,
                    opponent: row.get(12)?,
                    deck_version_id: row.get(13)?,
                    deck_id: row.get(14)?,
                    games: Vec::new(),
                    opponent_archetype_id: row.get(16)?,
                    opponent_archetype: row.get(17)?,
                },
            })
        })?;
        for row in rows {
            read.push(row?);
        }
    }
    if read.is_empty() {
        return Ok(Vec::new());
    }

    let mut games: HashMap<i64, Vec<GameResult>> = HashMap::new();
    {
        let mut stmt = conn.prepare_cached(&games_sql(scope))?;
        let rows = stmt.query_map(params, |row| {
            Ok((
                row.get::<_, i64>(0)?,
                GameResult {
                    result: stored_game_result(&row.get::<_, String>(1)?),
                    on_play: row.get(2)?,
                    started_at: row.get(3)?,
                    ended_at: row.get(4)?,
                },
            ))
        })?;
        for row in rows {
            let (match_id, game) = row?;
            games.entry(match_id).or_default().push(game);
        }
    }

    let mut found = Vec::with_capacity(read.len());
    for mut entry in read {
        if let Some(match_games) = games.remove(&entry.match_id) {
            entry.row.games = match_games;
        }
        found.push((entry.group, entry.row));
    }
    Ok(found)
}

/// The match as the fold with the limited rules takes it: `None` when its
/// league token is empty or it is `pending` (0.x `Started`: the fold takes a
/// match once it is in progress).
pub(crate) fn any_league_match(row: &LeagueMatchRow) -> Option<LeagueMatch> {
    let league_token = row.league_token.as_deref()?.trim();
    if league_token.is_empty() {
        return None;
    }
    // `pending` is the one stored state `MatchState` lacks.
    let state = state_from_str(&row.state)?;
    Some(LeagueMatch {
        token: row.token.clone(),
        league_token: league_token.to_string(),
        started_at: row.started_at,
        ended_at: row.ended_at,
        state,
        outcome: stored_outcome(&row.outcome),
        format: row.format.clone(),
        game_structure: row.game_structure.clone(),
        deck_version_id: row.deck_version_id,
        games: row.games.clone(),
    })
}

/// A member of an imported run as the fold takes it. Membership is the
/// run's own, so its league token (an imported copy has none) only
/// defaults to the run's.
pub(crate) fn member_match(row: &LeagueMatchRow, run_token: &str) -> LeagueMatch {
    let league_token = row
        .league_token
        .as_deref()
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .unwrap_or(run_token);
    LeagueMatch {
        token: row.token.clone(),
        league_token: league_token.to_string(),
        started_at: row.started_at,
        ended_at: row.ended_at,
        state: stored_state(&row.state),
        outcome: stored_outcome(&row.outcome),
        format: row.format.clone(),
        game_structure: row.game_structure.clone(),
        deck_version_id: row.deck_version_id,
        games: row.games.clone(),
    }
}

/// The `league_joined` (with a token and an event id, and the matches its
/// panel states), `league_join_request` and `league_dropped` events of the
/// account. An event's player is the latest Login in its file
/// at or before it, else the file's inherited player (as for match history).
pub(crate) fn league_events(
    conn: &Connection,
    account_id: i64,
) -> Result<Vec<OrderedInput>, StoreError> {
    let username: Option<String> = conn
        .query_row(
            "SELECT username FROM accounts WHERE id = ?1",
            [account_id],
            |row| row.get(0),
        )
        .optional()?;
    let Some(username) = username else {
        return Ok(Vec::new());
    };

    struct LeagueEvent {
        file: i64,
        offset: i64,
        kind: String,
        token: Option<String>,
        event_id: Option<i64>,
        logged_at: i64,
        inherited: Option<String>,
        /// Only a panel's: its `FormatDescription` is the one thing read.
        raw: Option<Vec<u8>>,
        compressed: bool,
    }
    let mut events: Vec<LeagueEvent> = Vec::new();
    {
        let mut stmt = conn.prepare_cached(LEAGUE_EVENTS)?;
        let rows = stmt.query_map([], |row| {
            Ok(LeagueEvent {
                file: row.get(0)?,
                offset: row.get(1)?,
                kind: row.get(2)?,
                token: row.get(3)?,
                event_id: row.get(4)?,
                logged_at: row.get(5)?,
                inherited: row.get(6)?,
                raw: row.get(7)?,
                compressed: row.get(8)?,
            })
        })?;
        for row in rows {
            events.push(row?);
        }
    }
    if events.is_empty() {
        return Ok(Vec::new());
    }

    let logins = logins_by_file(conn)?;
    let mut inputs = Vec::new();
    for event in events {
        let player = logins
            .get(&event.file)
            .and_then(|file_logins| player_at(file_logins, event.offset))
            .or(event.inherited.as_deref());
        if player != Some(username.as_str()) {
            continue;
        }
        let input = match event.kind.as_str() {
            "league_dropped" => LeagueInput::Dropped {
                at_ms: event.logged_at,
            },
            "league_join_request" => LeagueInput::JoinRequested {
                at_ms: event.logged_at,
            },
            _ => {
                let (Some(token), Some(event_id)) = (event.token, event.event_id) else {
                    continue;
                };
                LeagueInput::Joined {
                    token,
                    event_id,
                    at_ms: event.logged_at,
                    rounds: match event.raw {
                        Some(raw) => panel_round_count(&decode_raw(raw, event.compressed)?),
                        None => None,
                    },
                }
            }
        };
        inputs.push((
            event.logged_at,
            event.file as u64,
            event.offset as u64,
            input,
        ));
    }
    Ok(inputs)
}

/// Every file's Login players, `(offset, username)` in offset order.
pub(crate) fn logins_by_file(
    conn: &Connection,
) -> Result<HashMap<i64, Vec<(i64, String)>>, StoreError> {
    let mut logins: HashMap<i64, Vec<(i64, String)>> = HashMap::new();
    let mut stmt = conn.prepare_cached(ALL_LOGINS)?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, Vec<u8>>(2)?,
            row.get::<_, bool>(3)?,
        ))
    })?;
    for row in rows {
        let (file, offset, raw, compressed) = row?;
        if let Some(login) = parse_login(&decode_raw(raw, compressed)?) {
            logins
                .entry(file)
                .or_default()
                .push((offset, login.username));
        }
    }
    for file_logins in logins.values_mut() {
        file_logins.sort_by_key(|(offset, _)| *offset);
    }
    Ok(logins)
}

/// The player of the latest Login at or before `offset`.
pub(crate) fn player_at(logins: &[(i64, String)], offset: i64) -> Option<&str> {
    logins
        .iter()
        .rev()
        .find(|(login_offset, _)| *login_offset <= offset)
        .map(|(_, name)| name.as_str())
}

/// Rows by their group, keeping their order.
fn grouped(rows: Vec<(Option<i64>, LeagueMatchRow)>) -> HashMap<i64, Vec<LeagueMatchRow>> {
    let mut groups: HashMap<i64, Vec<LeagueMatchRow>> = HashMap::new();
    for (group, row) in rows {
        if let Some(group) = group {
            groups.entry(group).or_default().push(row);
        }
    }
    groups
}

/// A stored imported run state.
pub(crate) fn parse_state(text: &str) -> Result<RunState, StoreError> {
    match text {
        "active" => Ok(RunState::Active),
        "complete" => Ok(RunState::Complete),
        "partial" => Ok(RunState::Partial),
        "dropped" => Ok(RunState::Dropped),
        other => Err(StoreError::Corrupt(format!("league run state {other:?}"))),
    }
}

/// Every league note, by run key (detached runs' too: a Limited page finds a
/// retired run's note by its former key).
pub fn notes_by_run(conn: &Connection) -> Result<HashMap<String, String>, StoreError> {
    let mut stmt =
        conn.prepare_cached("SELECT run_key, notes FROM leagues WHERE notes IS NOT NULL")?;
    let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
    let mut notes = HashMap::new();
    for row in rows {
        let (run_key, text) = row?;
        notes.insert(run_key, text);
    }
    Ok(notes)
}

/// Every manual drop (`leagues.user_dropped_at` where `user_state` is
/// `dropped`), by run key: derived, draft and imported runs (also among the
/// fold's inputs).
pub fn manual_drops(conn: &Connection) -> Result<HashMap<String, i64>, StoreError> {
    let mut stmt = conn.prepare_cached(
        "SELECT run_key, user_dropped_at FROM leagues
         WHERE user_dropped_at IS NOT NULL AND user_state = 'dropped'",
    )?;
    let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
    let mut drops = HashMap::new();
    for row in rows {
        let (run_key, at_ms) = row?;
        drops.insert(run_key, at_ms);
    }
    Ok(drops)
}

/// A deck as a league card shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeagueDeck {
    pub id: i64,
    pub account_id: Option<i64>,
    pub name: String,
    /// The format code (`CMODERN`).
    pub format: String,
    /// `U,R`, or `None`.
    pub color_identity: Option<String>,
    /// The cover card's art crop URL.
    pub cover_art: Option<String>,
    /// Its file in v1's images folder.
    pub local_cover_art: Option<String>,
    /// The deck's own cover image, a file in the deck covers folder.
    pub cover_file: Option<String>,
    pub archived: bool,
    /// The deck's archetype (the Leagues archetype filter).
    pub archetype_id: Option<i64>,
    pub archetype_name: Option<String>,
}

/// Every deck, archived ones too (a run of an archived deck still shows
/// it), by name then id.
pub fn league_decks(conn: &Connection) -> Result<Vec<LeagueDeck>, StoreError> {
    let mut stmt = conn.prepare_cached(
        "SELECT d.id, d.account_id, d.name, d.format, d.color_identity, c.art_crop,
             d.archived_at IS NOT NULL, c.local_art_crop, d.archetype_id, a.name,
             d.cover_file
         FROM decks d LEFT JOIN cards c ON c.catalog_id = d.cover_catalog_id
         LEFT JOIN archetypes a ON a.id = d.archetype_id
         ORDER BY d.name, d.id",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(LeagueDeck {
            id: row.get(0)?,
            account_id: row.get(1)?,
            name: row.get(2)?,
            format: row.get(3)?,
            color_identity: row.get(4)?,
            cover_art: row.get(5)?,
            local_cover_art: row.get(7)?,
            cover_file: row.get(10)?,
            archived: row.get(6)?,
            archetype_id: row.get(8)?,
            archetype_name: row.get(9)?,
        })
    })?;
    let mut decks = Vec::new();
    for row in rows {
        decks.push(row?);
    }
    Ok(decks)
}

/// A deck version and when it was saved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VersionDate {
    pub id: i64,
    pub deck_id: i64,
    pub modified_at: i64,
}

/// Every deck version, oldest first per deck (by save time, then id).
pub fn version_dates(conn: &Connection) -> Result<Vec<VersionDate>, StoreError> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, deck_id, modified_at FROM deck_versions ORDER BY deck_id, modified_at, id",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(VersionDate {
            id: row.get(0)?,
            deck_id: row.get(1)?,
            modified_at: row.get(2)?,
        })
    })?;
    let mut versions = Vec::new();
    for row in rows {
        versions.push(row?);
    }
    Ok(versions)
}

/// Every match note, by match token.
pub fn notes_by_match(conn: &Connection) -> Result<HashMap<String, String>, StoreError> {
    let mut stmt =
        conn.prepare_cached("SELECT token, notes FROM matches WHERE notes IS NOT NULL")?;
    let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
    let mut notes = HashMap::new();
    for row in rows {
        let (token, body) = row?;
        notes.insert(token, body);
    }
    Ok(notes)
}

/// Every account id of `platform`, in id order.
pub fn account_ids(conn: &Connection, platform: Platform) -> Result<Vec<i64>, StoreError> {
    let mut stmt =
        conn.prepare_cached("SELECT id FROM accounts WHERE platform = ?1 ORDER BY id")?;
    let rows = stmt.query_map([platform.as_str()], |row| row.get(0))?;
    let mut ids = Vec::new();
    for row in rows {
        ids.push(row?);
    }
    Ok(ids)
}

/// A match "Add matches…" offers (spec 5.3) and where it is now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AvailableMatch {
    pub row: LeagueMatchRow,
    /// The listed league it is in now; `None` for none (or a removed one).
    pub now_in: Option<LeagueRef>,
}

/// A league as a match row names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeagueRef {
    pub run_key: String,
    pub name: String,
    pub started_at: i64,
}

/// What a league takes (spec 4.1): its kind, deck and set code.
struct LeagueFit {
    kind: LeagueKind,
    deck_id: Option<i64>,
    set_code: Option<String>,
}

impl LeagueFit {
    /// The set code `available_any_scope` narrows by (bound as `?2`): a
    /// limited run's, when it knows it. The one place that decides both the
    /// SQL and its parameters.
    fn sql_set_code(&self) -> Option<&String> {
        if self.kind == LeagueKind::Constructed {
            None
        } else {
            self.set_code.as_ref()
        }
    }
}

/// The matches league `run_key` can take (spec 4.1 and 5.3): at most
/// `AVAILABLE_LIMIT`, newest first, each with the league it is in now. None
/// for a league that is removed, detached or missing.
pub fn available_matches(
    conn: &Connection,
    run_key: &str,
) -> Result<Vec<AvailableMatch>, StoreError> {
    let target = conn
        .prepare_cached(
            "SELECT id, kind, deck_id, set_code FROM listed_leagues
             WHERE run_key = ?1 AND detached_at IS NULL",
        )?
        .query_row([run_key], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<i64>>(2)?,
                row.get::<_, Option<String>>(3)?,
            ))
        })
        .optional()?;
    let Some((league_id, kind, deck_id, set_code)) = target else {
        return Ok(Vec::new());
    };
    let fit = LeagueFit {
        kind: parse_kind(&kind)?,
        deck_id,
        set_code,
    };
    let rows = if fit.kind == LeagueKind::Constructed && fit.deck_id.is_some() {
        match_rows(conn, &available_by_deck_scope(), &[&league_id])?
    } else {
        let scope = available_any_scope(&fit);
        match fit.sql_set_code() {
            Some(code) => match_rows(conn, &scope, &[&league_id, code])?,
            None => match_rows(conn, &scope, &[&league_id])?,
        }
    };
    let mut held: Vec<i64> = rows.iter().filter_map(|(id, _)| *id).collect();
    held.sort_unstable();
    held.dedup();
    let names = league_refs(conn, &held)?;
    let mut found = Vec::new();
    for (held_by, row) in rows {
        if found.len() >= AVAILABLE_LIMIT as usize {
            break;
        }
        if fit_refusal(&fit, row.format.as_deref(), row.deck_id).is_some() {
            continue;
        }
        let now_in = held_by.and_then(|id| names.get(&id).cloned());
        found.push(AvailableMatch { row, now_in });
    }
    Ok(found)
}

/// A listed league's row id, the fold's state and the player's status
/// (league self-management spec 3.2). The app reads it before a delete
/// for the status Undo puts back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LeagueStatus {
    pub id: i64,
    pub state: RunState,
    pub user_state: Option<UserState>,
}

/// The listed league with this run key; `None` when there is none or it
/// was deleted.
pub fn league_status(conn: &Connection, run_key: &str) -> Result<Option<LeagueStatus>, StoreError> {
    let row = conn
        .query_row(
            "SELECT id, state, user_state FROM listed_leagues WHERE run_key = ?1",
            [run_key],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            },
        )
        .optional()?;
    let Some((id, state, user_state)) = row else {
        return Ok(None);
    };
    Ok(Some(LeagueStatus {
        id,
        state: parse_state(&state)?,
        user_state: stored_user_state(user_state)?,
    }))
}

/// The listed league each of `tokens` sits in now (`matches.league_id`). A
/// match in no league, or in a deleted one (a non-league match to every
/// read, spec 3.3), is left out.
pub fn match_leagues(
    conn: &Connection,
    tokens: &[String],
) -> Result<HashMap<String, LeagueRef>, StoreError> {
    let mut found = HashMap::new();
    if tokens.is_empty() {
        return Ok(found);
    }
    let list =
        serde_json::to_string(tokens).map_err(|error| StoreError::Corrupt(error.to_string()))?;
    let mut stmt = conn.prepare_cached(
        "SELECT m.token, l.run_key, l.name, l.started_at
         FROM listed_matches m JOIN listed_leagues l ON l.id = m.league_id
         WHERE m.token IN (SELECT value FROM json_each(?1))",
    )?;
    let rows = stmt.query_map([list], |row| {
        Ok((
            row.get::<_, String>(0)?,
            LeagueRef {
                run_key: row.get(1)?,
                name: row.get(2)?,
                started_at: row.get(3)?,
            },
        ))
    })?;
    for row in rows {
        let (token, league) = row?;
        found.insert(token, league);
    }
    Ok(found)
}

/// A league a match can move to, with what the picker shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoveTarget {
    pub league: LeagueRef,
    /// `COALESCE(user_state, state)`: what the league's card shows.
    pub state: String,
    /// `log`, `imported` or `manual`.
    pub source: String,
    /// Its listed matches.
    pub match_count: u32,
}

/// The leagues the match `token` may move into ("Move to league...",
/// league self-management spec 5.4), newest first, at most
/// `AVAILABLE_LIMIT`: the rules of spec 4.1 (a complete or abandoned
/// match, not a tournament match, a listed league that is not detached, of
/// the match's account (or of none), a manual league with room), never the
/// league it is in. Whether the match fits the league is `fit_refusal`,
/// the rule `LeagueEdit::MoveMatch` checks again: a constructed match gets
/// the constructed leagues with no deck or its deck, a limited match the
/// draft and sealed runs of its set (and runs that do not know their set).
pub fn move_targets(conn: &Connection, token: &str) -> Result<Vec<MoveTarget>, StoreError> {
    let played: Option<(Option<String>, Option<i64>)> = conn
        .query_row(
            "SELECT m.format, dv.deck_id FROM listed_matches m
             LEFT JOIN deck_versions dv ON dv.id = m.deck_version_id
             WHERE m.token = ?1",
            [token],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let Some((format, deck_id)) = played else {
        return Ok(Vec::new());
    };
    let sql = format!(
        "SELECT l.run_key, l.name, l.started_at, COALESCE(l.user_state, l.state), l.source,
             (SELECT COUNT(*) FROM listed_matches lm WHERE lm.league_id = l.id),
             l.kind, l.deck_id, l.set_code
         FROM listed_matches m
         JOIN listed_leagues l ON (l.account_id IS NULL OR l.account_id = m.account_id)
         WHERE m.token = ?1
           AND l.detached_at IS NULL
           AND m.state IN ('complete', 'abandoned')
           AND m.tournament_event_id IS NULL
           AND l.id IS NOT m.league_id
           AND (l.source <> 'manual'
               OR (SELECT COUNT(*) FROM listed_matches lm WHERE lm.league_id = l.id)
                   < {MANUAL_MAX_MATCHES})
         ORDER BY l.started_at DESC, l.id DESC"
    );
    let mut stmt = conn.prepare_cached(&sql)?;
    let rows = stmt.query_map([token], |row| {
        let target = MoveTarget {
            league: LeagueRef {
                run_key: row.get(0)?,
                name: row.get(1)?,
                started_at: row.get(2)?,
            },
            state: row.get(3)?,
            source: row.get(4)?,
            match_count: row.get(5)?,
        };
        let kind: String = row.get(6)?;
        let deck_id: Option<i64> = row.get(7)?;
        let set_code: Option<String> = row.get(8)?;
        Ok((target, kind, deck_id, set_code))
    })?;
    let mut targets = Vec::new();
    for row in rows {
        if targets.len() >= AVAILABLE_LIMIT as usize {
            break;
        }
        let (target, kind, league_deck_id, set_code) = row?;
        let fit = LeagueFit {
            kind: parse_kind(&kind)?,
            deck_id: league_deck_id,
            set_code,
        };
        if fit_refusal(&fit, format.as_deref(), deck_id).is_none() {
            targets.push(target);
        }
    }
    Ok(targets)
}

/// The listed leagues with these row ids, by id. Ids are integers, so the
/// list is safe to inline.
fn league_refs(conn: &Connection, ids: &[i64]) -> Result<HashMap<i64, LeagueRef>, StoreError> {
    let mut refs = HashMap::new();
    if ids.is_empty() {
        return Ok(refs);
    }
    let list: Vec<String> = ids.iter().map(i64::to_string).collect();
    let mut stmt = conn.prepare(&format!(
        "SELECT id, run_key, name, started_at FROM listed_leagues WHERE id IN ({})",
        list.join(", ")
    ))?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            LeagueRef {
                run_key: row.get(1)?,
                name: row.get(2)?,
                started_at: row.get(3)?,
            },
        ))
    })?;
    for row in rows {
        let (id, found) = row?;
        refs.insert(id, found);
    }
    Ok(refs)
}

/// Why a match of `format`, played with deck `deck_id`, does not fit the
/// league, if it does not: a draft or sealed match only in a draft or sealed
/// run of its set (when the run knows its set), a constructed match only in
/// a constructed league of its deck (when the league knows its deck).
fn fit_refusal(
    fit: &LeagueFit,
    format: Option<&str>,
    deck_id: Option<i64>,
) -> Option<LeagueEditOutcome> {
    let limited_match = format.is_some_and(is_limited_format_code);
    let limited_league = fit.kind != LeagueKind::Constructed;
    if limited_match != limited_league {
        return Some(LeagueEditOutcome::WrongKind);
    }
    if limited_league {
        let code = fit.set_code.as_deref()?;
        let match_code = format.and_then(limited::set_code::set_code_from_play_format);
        if match_code.as_deref() != Some(code) {
            return Some(LeagueEditOutcome::WrongDeck);
        }
        return None;
    }
    match fit.deck_id {
        Some(league_deck) if deck_id != Some(league_deck) => Some(LeagueEditOutcome::WrongDeck),
        _ => None,
    }
}

/// The account's tournaments: the rows its matches point at (through the
/// matches' tournament index). A tournament two accounts played is one row,
/// so each account's start and format come from its own matches.
const TOURNAMENTS_SQL: &str = "
SELECT t.id, t.mtgo_event_id, t.name FROM tournaments t
WHERE t.id IN (
    SELECT m.tournament_id FROM matches m
    WHERE m.account_id = ?1 AND m.tournament_event_id IS NOT NULL)";

/// The account's listed tournament matches that reached in progress, with
/// their tournament row.
fn tournament_scope() -> Scope {
    Scope {
        from: "listed_matches m JOIN tournaments t ON t.id = m.tournament_id",
        filter: "m.account_id = ?1 AND m.tournament_event_id IS NOT NULL AND m.state <> 'pending'"
            .to_string(),
        order: "ORDER BY m.started_at, m.id",
        limit: "",
        group: "t.id",
    }
}

/// The account's stored tournaments (spec 6, `tournaments`) that have a
/// match of `deck_id` (any deck when `None`) started within `from_ms..=to_ms`
/// (open ends when `None`), newest first. Each holds its listed matches of
/// that deck, in progress ones included, whatever their start. Three
/// queries: tournaments, matches, games.
pub fn stored_tournaments(
    conn: &Connection,
    account_id: i64,
    deck_id: Option<i64>,
    from_ms: Option<i64>,
    to_ms: Option<i64>,
) -> Result<Vec<TournamentGroup>, StoreError> {
    let mut groups: Vec<(i64, TournamentGroup)> = Vec::new();
    {
        let mut stmt = conn.prepare_cached(TOURNAMENTS_SQL)?;
        let rows = stmt.query_map([account_id], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                TournamentGroup {
                    event_id: row.get(1)?,
                    name: row.get(2)?,
                    format: None,
                    started_at: 0,
                    matches: Vec::new(),
                },
            ))
        })?;
        for row in rows {
            groups.push(row?);
        }
    }
    if groups.is_empty() {
        return Ok(Vec::new());
    }
    let mut members = grouped(match_rows(conn, &tournament_scope(), &[&account_id])?);
    let mut kept = Vec::new();
    for (id, mut group) in groups {
        let rows = members.remove(&id).unwrap_or_default();
        // Rows come in start order: the account's first match, whatever its
        // deck, is the tournament's start and format.
        let Some(first) = rows.first() else {
            continue;
        };
        group.started_at = first.started_at;
        group.format = first.format.clone();
        let mut in_range = false;
        for row in rows {
            if deck_id.is_some() && row.deck_id != deck_id {
                continue;
            }
            let after_start = from_ms.is_none_or(|from| row.started_at >= from);
            let before_end = to_ms.is_none_or(|to| row.started_at <= to);
            if after_start && before_end {
                in_range = true;
            }
            group.matches.push(row);
        }
        if in_range {
            kept.push(group);
        }
    }
    kept.sort_by(|a, b| {
        b.started_at
            .cmp(&a.started_at)
            .then(b.event_id.cmp(&a.event_id))
    });
    Ok(kept)
}

/// The rows of these owners, as SQL on `column` (an `account_id`): account
/// ids are integers, so they are inlined. No owner takes nothing.
fn owners_sql(column: &str, owners: &[LeagueOwner]) -> String {
    let mut accounts: Vec<String> = Vec::new();
    let mut nobody = false;
    for owner in owners {
        match owner {
            LeagueOwner::Account(id) => accounts.push(id.to_string()),
            LeagueOwner::Nobody => nobody = true,
        }
    }
    let mut parts: Vec<String> = Vec::new();
    if !accounts.is_empty() {
        parts.push(format!("{column} IN ({})", accounts.join(", ")));
    }
    if nobody {
        parts.push(format!("{column} IS NULL"));
    }
    if parts.is_empty() {
        parts.push("0".to_string());
    }
    format!("({})", parts.join(" OR "))
}

/// `RunFilter` as SQL on `leagues l`, with what it binds. Account ids and
/// deck ids are integers, so they are inlined; the run key is bound as `?1`.
fn run_filter_sql(filter: &RunFilter) -> (String, Vec<String>) {
    let mut parts = vec!["l.detached_at IS NULL".to_string()];
    parts.push(owners_sql("l.account_id", &filter.owners));
    let kinds: Vec<String> = filter
        .kinds
        .iter()
        .map(|kind| format!("'{}'", kind.as_str()))
        .collect();
    if kinds.is_empty() {
        parts.push("0".to_string());
    } else {
        parts.push(format!("l.kind IN ({})", kinds.join(", ")));
    }
    if let Some(deck_id) = filter.deck_id {
        parts.push(format!("l.deck_id = {deck_id}"));
    }
    let mut bound = Vec::new();
    if let Some(run_key) = &filter.run_key {
        parts.push("l.run_key = ?1".to_string());
        bound.push(run_key.clone());
    }
    (parts.join(" AND "), bound)
}

pub(crate) fn parse_kind(text: &str) -> Result<LeagueKind, StoreError> {
    match text {
        "constructed" => Ok(LeagueKind::Constructed),
        "draft" => Ok(LeagueKind::Draft),
        "sealed" => Ok(LeagueKind::Sealed),
        other => Err(StoreError::Corrupt(format!("league kind {other:?}"))),
    }
}

/// The stored runs `filter` takes (target schema section 8), each with its
/// listed matches and their games: three queries (runs, matches, games),
/// whatever the count. Order: log runs, then manual leagues, then imported
/// runs; each by account (no account last), log runs oldest first (as the
/// fold mints them), manual leagues and imported runs newest first.
pub fn stored_runs(conn: &Connection, filter: &RunFilter) -> Result<Vec<StoredRun>, StoreError> {
    let (sql_filter, bound) = run_filter_sql(filter);
    let params: Vec<&dyn ToSql> = bound.iter().map(|value| value as &dyn ToSql).collect();
    let mut runs: Vec<StoredRun> = Vec::new();
    {
        let mut stmt = conn.prepare_cached(&format!(
            "SELECT l.id, l.run_key, l.account_id, l.source, l.kind, l.state, l.name, l.token,
                 l.event_id, l.mtgo_course_id, l.set_code, l.format, l.deck_version_id, l.deck_id,
                 l.started_at, l.joined_at, l.completed_at, l.dropped_at, l.notes, l.created_at, l.user_state,
                 l.rounds, l.course_wins, l.course_losses
             FROM listed_leagues l
             WHERE {sql_filter}
             ORDER BY CASE l.source WHEN 'log' THEN 0 WHEN 'manual' THEN 1 ELSE 2 END,
                 l.account_id IS NULL, l.account_id,
                 CASE l.source WHEN 'log' THEN l.started_at ELSE -l.started_at END,
                 CASE l.source WHEN 'log' THEN l.id WHEN 'imported' THEN -l.imported_from
                     ELSE -l.id END"
        ))?;
        let rows = stmt.query_map(params.as_slice(), |row| {
            Ok((
                StoredRun {
                    id: row.get(0)?,
                    key: row.get(1)?,
                    account_id: row.get(2)?,
                    source: row.get(3)?,
                    kind: LeagueKind::Constructed,
                    state: RunState::Active,
                    name: row.get(6)?,
                    token: row.get(7)?,
                    event_id: row.get(8)?,
                    mtgo_course_id: row.get(9)?,
                    set_code: row.get(10)?,
                    format: row.get(11)?,
                    deck_version_id: row.get(12)?,
                    deck_id: row.get(13)?,
                    started_at: row.get(14)?,
                    joined_at: row.get(15)?,
                    completed_at: row.get(16)?,
                    dropped_at: row.get(17)?,
                    created_at: row.get(19)?,
                    notes: row.get(18)?,
                    user_state: None,
                    rounds: row.get::<_, Option<i64>>(21)?.map(|rounds| rounds as usize),
                    course_record: match (row.get(22)?, row.get(23)?) {
                        (Some(wins), Some(losses)) => Some((wins, losses)),
                        _ => None,
                    },
                    matches: Vec::new(),
                },
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, Option<String>>(20)?,
            ))
        })?;
        for row in rows {
            let (mut run, kind, state, user_state) = row?;
            run.kind = parse_kind(&kind)?;
            run.state = parse_state(&state)?;
            run.user_state = stored_user_state(user_state)?;
            runs.push(run);
        }
    }
    if runs.is_empty() {
        return Ok(runs);
    }
    let scope = Scope {
        from: "listed_leagues l JOIN listed_matches m ON m.league_id = l.id",
        filter: sql_filter,
        order: "ORDER BY m.started_at, m.id",
        limit: "",
        group: "l.id",
    };
    let mut members = grouped(match_rows(conn, &scope, &params)?);
    for run in &mut runs {
        run.matches = members.remove(&run.id).unwrap_or_default();
    }
    Ok(runs)
}

/// A draft or sealed run with what the Limited pages read (limited spec
/// section 5).
#[derive(Debug, Clone, PartialEq)]
pub struct LimitedRun {
    /// The run as the fold made it: its key, kind, set code, course id,
    /// token, state and match tokens.
    pub run: Run,
    /// The run's draft, when one is linked.
    pub draft: Option<Draft>,
    /// The run's matches still stored, in the run's order, with their games.
    pub matches: Vec<LeagueMatchRow>,
    /// The registered deck of each of its matches that has one, in capture
    /// order.
    pub registered: Vec<RegisteredDeck>,
    /// `log` or `imported` (`leagues.source`).
    pub source: String,
    /// The run's note.
    pub notes: Option<String>,
    /// The player's status (`leagues.user_state`); `None` is Automatic.
    pub user_state: Option<UserState>,
    /// The pool Arena stated for the run's course (its newest observation),
    /// one catalog id per card; `None` for an MTGO run or an Arena orphan.
    pub stated_pool: Option<Vec<u32>>,
    /// The record Arena stated for the run's course, newest of this and
    /// every other device (matches played elsewhere included); `None` for
    /// an MTGO run or an Arena orphan. The pages show `shown_record`.
    pub course_record: Option<(u32, u32)>,
}

impl LimitedRun {
    /// An Arena run: read-only, uncapped, with a stated pool.
    pub fn is_arena(&self) -> bool {
        is_arena_run_key(&self.run.key)
    }

    /// The state the pages show: the player's status, else the fold's
    /// (spec 3.2). Overlays and the "current run" rules read `run.state`.
    pub fn shown_state(&self) -> RunState {
        self.user_state.map_or(self.run.state, UserState::run_state)
    }

    /// The run as the numbers read it (`leagues::summarize_limited_run` with
    /// `self.run.kind`): the shown state (the player's status, else the
    /// fold's), like `StoredRun::run_input`.
    pub fn run_input(&self) -> RunInput {
        RunInput {
            course_record: self.course_record,
            state: self.shown_state(),
            manual: false,
            uncapped: self.is_arena(),
            format: self.run.format.clone(),
            started_at: self.run.started_at,
            matches: self.matches.iter().map(LeagueMatchRow::run_match).collect(),
        }
    }
}

/// One account's (or the account-less 0.x data's) limited runs and the
/// drafts no run holds.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LimitedRuns {
    /// Draft and sealed runs, newest first (by start; on a tie the later
    /// stored first).
    pub runs: Vec<LimitedRun>,
    /// Drafts linked to no run (0.x "Draft (league unknown)" rows), newest
    /// first by start.
    pub unlinked_drafts: Vec<Draft>,
}

/// The stored draft and sealed runs `filter` takes (its `kinds` are ignored),
/// newest first by start (on a tie the later stored first), each rebuilt from
/// its row (`former_keys` empty: the sync already moved a retired run's notes
/// and drop to the run that took its draft) with its draft (`drafts.league_id`),
/// its listed matches and their registered decks by capture time (the sync's
/// snapshot, else 0.x's). Without a run key or deck in the filter, also the
/// owners' drafts no run holds, newest first. An Arena run also gets the pool
/// its course's newest observation stated. A fixed number of queries plus
/// one per Arena run; no log event is read. A removed run and its draft are
/// left out (its draft is held by a run, so it is not among the unlinked
/// drafts either).
pub fn stored_limited_runs(
    conn: &Connection,
    filter: &RunFilter,
) -> Result<LimitedRuns, StoreError> {
    let filter = RunFilter {
        kinds: vec![LeagueKind::Draft, LeagueKind::Sealed],
        ..filter.clone()
    };
    let stored = stored_runs(conn, &filter)?;

    let mut drafts: HashMap<i64, Draft> = HashMap::new();
    if !stored.is_empty() {
        // Row ids are integers, so the list is safe to inline.
        let ids: Vec<String> = stored.iter().map(|run| run.id.to_string()).collect();
        let held = stored_drafts(conn, &format!("d.league_id IN ({})", ids.join(", ")), &[])?;
        for found in held {
            if let Some(run_id) = found.league_id {
                drafts.insert(run_id, found.draft);
            }
        }
    }

    // The registered decks of the runs' listed matches: the sync's
    // (`registered`) wins over 0.x's (`imported`).
    let mut decks: HashMap<String, RegisteredDeck> = HashMap::new();
    if !stored.is_empty() {
        let (sql_filter, bound) = run_filter_sql(&filter);
        let params: Vec<&dyn ToSql> = bound.iter().map(|value| value as &dyn ToSql).collect();
        let mut stmt = conn.prepare_cached(&format!(
            "SELECT m.token, s.cards, s.captured_at FROM listed_leagues l
             JOIN listed_matches m ON m.league_id = l.id
             JOIN limited_deck_snapshots s ON s.match_id = m.id
             WHERE {sql_filter}
             ORDER BY m.id, s.source = 'registered'"
        ))?;
        let rows = stmt.query_map(params.as_slice(), |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })?;
        for row in rows {
            let (token, cards, captured_at) = row?;
            let deck = registered(&token, captured_at, stored_cards(&cards)?);
            // The registered row comes last, so it wins.
            decks.insert(token, deck);
        }
    }

    let mut runs: Vec<(i64, LimitedRun)> = Vec::new();
    for row in stored {
        let id = row.id;
        let draft = drafts.remove(&id);
        let stated_pool = match arena_course_id(&row.key) {
            Some(course_id) => {
                crate::arena_courses::latest_pool(conn, course_id)?.filter(|pool| !pool.is_empty())
            }
            None => None,
        };
        let mut limited = limited_run(row, draft, &decks);
        limited.stated_pool = stated_pool;
        runs.push((id, limited));
    }
    runs.sort_by_key(|(id, limited)| std::cmp::Reverse((limited.run.started_at, *id)));
    let runs = runs.into_iter().map(|(_, limited)| limited).collect();

    let mut unlinked_drafts = Vec::new();
    if filter.run_key.is_none() && filter.deck_id.is_none() {
        let mut found = stored_drafts(
            conn,
            &format!(
                "d.league_id IS NULL AND {}",
                owners_sql("d.account_id", &filter.owners)
            ),
            &[],
        )?;
        found.reverse();
        found.sort_by_key(|draft| std::cmp::Reverse(draft.draft.started_at_ms));
        unlinked_drafts = found.into_iter().map(|draft| draft.draft).collect();
    }
    Ok(LimitedRuns {
        runs,
        unlinked_drafts,
    })
}

/// A stored limited run as the Limited pages read it.
fn limited_run(
    row: StoredRun,
    draft: Option<Draft>,
    decks: &HashMap<String, RegisteredDeck>,
) -> LimitedRun {
    let match_tokens: Vec<String> = row.matches.iter().map(|m| m.token.clone()).collect();
    let mut registered: Vec<RegisteredDeck> = match_tokens
        .iter()
        .filter_map(|token| decks.get(token).cloned())
        .collect();
    registered.sort_by_key(|deck| deck.captured_at_ms);
    LimitedRun {
        run: Run {
            key: row.key,
            token: row.token.unwrap_or_default(),
            event_id: row.event_id,
            joined_at: row.joined_at,
            name: row.name,
            format: row.format,
            deck_version_id: row.deck_version_id,
            started_at: row.started_at,
            state: row.state,
            dropped_at: row.dropped_at,
            completed_at: row.completed_at,
            match_tokens,
            kind: row.kind,
            set_code: row.set_code,
            course_id: row.mtgo_course_id,
            draft_token: draft.as_ref().map(|draft| draft.token.clone()),
            former_keys: Vec::new(),
            rounds: row.rounds,
        },
        draft,
        matches: row.matches,
        registered,
        source: row.source,
        notes: row.notes,
        user_state: row.user_state,
        stated_pool: None,
        course_record: row.course_record,
    }
}

/// Card facts for these catalog ids, from the `cards` table, read once per
/// call. A card with no row, or a stub without a name, is missing from the
/// map (neutral everywhere).
pub fn card_facts(
    conn: &Connection,
    catalog_ids: &BTreeSet<u32>,
) -> Result<CardFactsMap, StoreError> {
    let mut cards = CardFactsMap::new();
    let ids: Vec<u32> = catalog_ids.iter().copied().collect();
    // Plain integers, so the list is safe to inline; chunks keep each
    // statement small.
    for chunk in ids.chunks(500) {
        let list: Vec<String> = chunk.iter().map(u32::to_string).collect();
        let mut stmt = conn.prepare(&format!(
            "SELECT catalog_id, name, type, colors, color_identity, rarity, set_code, set_name,
                 oracle_id, cmc
             FROM cards WHERE catalog_id IN ({}) AND name IS NOT NULL",
            list.join(", ")
        ))?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                CardFacts {
                    name: row.get(1)?,
                    card_type: row.get(2)?,
                    colors: row.get(3)?,
                    color_identity: row.get(4)?,
                    rarity: row.get(5)?,
                    set_code: row.get(6)?,
                    set_name: row.get(7)?,
                    oracle_id: row.get(8)?,
                    cmc: row.get(9)?,
                },
            ))
        })?;
        for row in rows {
            let (catalog_id, facts) = row?;
            if let Ok(catalog_id) = u32::try_from(catalog_id) {
                cards.insert(catalog_id, facts);
            }
        }
    }
    Ok(cards)
}

/// Where a live draft sits in the fold: after every log line of its first
/// line's millisecond, before a match starting then (0.x processed league
/// events, then drafts, then matches in a tick).
pub(crate) const DRAFT_POSITION: (u64, u64) = (u64::MAX - 2, 0);

/// The newest shown or picked time over the picks, else the start (0.x
/// `AbandonStaleDrafts::lastActivity`, whose last fallback, the row's
/// creation, an imported draft does not have).
pub(crate) fn last_activity(draft: &Draft) -> Option<i64> {
    let mut latest: Option<i64> = None;
    for pick in &draft.picks {
        latest = latest.max(pick.shown_at_ms).max(pick.picked_at_ms);
    }
    latest.or(draft.started_at_ms)
}

/// A connecting or picking draft idle for more than 30 minutes before
/// `now_ms` (one with no time at all is long idle).
pub(crate) fn is_stale(draft: &Draft, now_ms: i64) -> bool {
    let open = matches!(draft.state, DraftState::Connecting | DraftState::Picking);
    open && last_activity(draft).is_none_or(|at| at < now_ms - ABANDON_AFTER_MS)
}

/// League panel views with an event id, and the `PlayFormatCd` they show,
/// logged while the account was the local player.
const LEAGUE_PANELS: &str = "
SELECT e.log_file_id, e.byte_offset, e.league_event_id, e.logged_at, e.raw, e.compressed,
       f.inherited_username
FROM log_events e JOIN log_files f ON f.id = e.log_file_id
WHERE e.kind = 'league_joined' AND e.league_event_id IS NOT NULL AND e.quarantined = 0";

/// The account's league panel views and their format (`HOBx3`), for set
/// codes (0.x `ResolveLeagueSetCode::fromLeaguePanel`).
pub(crate) fn league_panels(
    conn: &Connection,
    account_id: i64,
) -> Result<Vec<PanelFormat>, StoreError> {
    let username: Option<String> = conn
        .query_row(
            "SELECT username FROM accounts WHERE id = ?1",
            [account_id],
            |row| row.get(0),
        )
        .optional()?;
    let Some(username) = username else {
        return Ok(Vec::new());
    };
    struct Panel {
        file: i64,
        offset: i64,
        event_id: i64,
        logged_at: i64,
        raw: Vec<u8>,
        compressed: bool,
        inherited: Option<String>,
    }
    let mut read: Vec<Panel> = Vec::new();
    {
        let mut stmt = conn.prepare_cached(LEAGUE_PANELS)?;
        let rows = stmt.query_map([], |row| {
            Ok(Panel {
                file: row.get(0)?,
                offset: row.get(1)?,
                event_id: row.get(2)?,
                logged_at: row.get(3)?,
                raw: row.get(4)?,
                compressed: row.get(5)?,
                inherited: row.get(6)?,
            })
        })?;
        for row in rows {
            read.push(row?);
        }
    }
    if read.is_empty() {
        return Ok(Vec::new());
    }
    let logins = logins_by_file(conn)?;
    let mut panels = Vec::new();
    for panel in read {
        let player = logins
            .get(&panel.file)
            .and_then(|file_logins| player_at(file_logins, panel.offset))
            .or(panel.inherited.as_deref());
        if player != Some(username.as_str()) {
            continue;
        }
        if let Some(play_format) = panel_play_format(&decode_raw(panel.raw, panel.compressed)?) {
            panels.push(PanelFormat {
                event_id: panel.event_id,
                at_ms: panel.logged_at,
                play_format,
            });
        }
    }
    Ok(panels)
}

/// An attached `leagues` row an edit names by its key.
struct EditRow {
    id: i64,
    source: String,
    /// `constructed`, `draft` or `sealed`.
    kind: String,
    /// The fold's state.
    state: String,
    account_id: Option<i64>,
    deck_id: Option<i64>,
    set_code: Option<String>,
    removed_at: Option<i64>,
    user_state: Option<String>,
}

impl EditRow {
    fn fit(&self) -> Result<LeagueFit, StoreError> {
        Ok(LeagueFit {
            kind: parse_kind(&self.kind)?,
            deck_id: self.deck_id,
            set_code: self.set_code.clone(),
        })
    }
}

/// The attached row with this key.
fn edit_row(tx: &Transaction, run_key: &str) -> Result<Option<EditRow>, StoreError> {
    attached_row(tx, "run_key = ?1", &run_key)
}

/// The attached row `filter` (one `?1`) names: by key for the edits, by id
/// for `move_refusal`.
fn attached_row(
    tx: &Transaction,
    filter: &str,
    value: &dyn ToSql,
) -> Result<Option<EditRow>, StoreError> {
    Ok(tx
        .prepare_cached(&format!(
            "SELECT id, source, kind, state, account_id, deck_id, set_code, removed_at,
                 user_state
             FROM leagues WHERE {filter} AND detached_at IS NULL"
        ))?
        .query_row([value], |row| {
            Ok(EditRow {
                id: row.get(0)?,
                source: row.get(1)?,
                kind: row.get(2)?,
                state: row.get(3)?,
                account_id: row.get(4)?,
                deck_id: row.get(5)?,
                set_code: row.get(6)?,
                removed_at: row.get(7)?,
                user_state: row.get(8)?,
            })
        })
        .optional()?)
}

/// Any `leagues` row by its key (Restore) or id (Undo), detached too: they bring back
/// the row a fully deleted run left detached (spec 4.4).
struct KeyRow {
    id: i64,
    account_id: Option<i64>,
    removed_at: Option<i64>,
}

fn key_row(tx: &Transaction, run_key: &str) -> Result<Option<KeyRow>, StoreError> {
    Ok(tx
        .prepare_cached("SELECT id, account_id, removed_at FROM leagues WHERE run_key = ?1")?
        .query_row([run_key], |row| {
            Ok(KeyRow {
                id: row.get(0)?,
                account_id: row.get(1)?,
                removed_at: row.get(2)?,
            })
        })
        .optional()?)
}

/// `key_row` by the row id.
fn id_row(tx: &Transaction, id: i64) -> Result<Option<KeyRow>, StoreError> {
    Ok(tx
        .prepare_cached("SELECT id, account_id, removed_at FROM leagues WHERE id = ?1")?
        .query_row([id], |row| {
            Ok(KeyRow {
                id: row.get(0)?,
                account_id: row.get(1)?,
                removed_at: row.get(2)?,
            })
        })
        .optional()?)
}

fn owner_of(account_id: Option<i64>) -> LeagueOwner {
    account_id.map_or(LeagueOwner::Nobody, LeagueOwner::Account)
}

/// Sync each owner once, in the edit's transaction.
fn sync_owners(
    tx: &Transaction,
    owners: &[LeagueOwner],
    tz: Tz,
    now_ms: i64,
) -> Result<(), StoreError> {
    let mut done: Vec<LeagueOwner> = Vec::new();
    for owner in owners {
        if !done.contains(owner) {
            sync_owner(tx, *owner, tz, now_ms)?;
            done.push(*owner);
        }
    }
    Ok(())
}

/// Apply one user edit in its own transaction (on the writer thread). A
/// move, a removal, a status, a delete, an Undo, a Restore or a drop syncs
/// the owners it touches (`tz` is the OS timezone) before it commits. A
/// refused edit changes nothing.
pub fn edit_league(
    conn: &mut Connection,
    edit: &LeagueEdit,
    tz: Tz,
) -> Result<LeagueEditOutcome, StoreError> {
    let tx = conn.transaction()?;
    if touches_arena_run(&tx, edit)? {
        return Ok(LeagueEditOutcome::ArenaRun);
    }
    let outcome = match edit {
        LeagueEdit::CreateManual {
            account_id,
            deck_id,
            name,
            token,
            started_at,
            now_ms,
        } => create_manual(
            &tx,
            &ManualRow {
                account_id: Some(*account_id),
                deck_id: Some(*deck_id),
                format: None,
                name: name.clone(),
                token: token.clone(),
                started_at: *started_at,
                created_at: *now_ms,
                imported_from: None,
            },
        )?,
        LeagueEdit::LinkMatches {
            run_key,
            match_tokens,
            now_ms,
        } => link_matches(&tx, run_key, match_tokens, tz, *now_ms)?,
        LeagueEdit::UnlinkMatch {
            run_key,
            match_token,
            now_ms,
        } => unlink_match(&tx, run_key, match_token, tz, *now_ms)?,
        LeagueEdit::SetNotes {
            run_key,
            notes,
            now_ms,
        } => set_notes(&tx, run_key, notes, *now_ms)?,
        LeagueEdit::SetStatus {
            run_key,
            status,
            now_ms,
        } => set_status(&tx, run_key, *status, tz, *now_ms)?,
        LeagueEdit::Delete {
            run_key,
            matches,
            now_ms,
        } => delete_league(&tx, run_key, *matches, tz, *now_ms)?,
        LeagueEdit::UndoDelete {
            league_id,
            removed_at,
            now_ms,
        } => undo_delete(&tx, *league_id, *removed_at, tz, *now_ms)?,
        LeagueEdit::Restore { run_key, now_ms } => restore_league(&tx, run_key, tz, *now_ms)?,
        LeagueEdit::MoveMatch {
            match_token,
            to_run_key,
            now_ms,
        } => match to_run_key {
            Some(run_key) => {
                link_matches(&tx, run_key, std::slice::from_ref(match_token), tz, *now_ms)?
            }
            None => leave_every_league(&tx, match_token, tz, *now_ms)?,
        },
    };
    // A refusal is rolled back: most write nothing before refusing, but
    // `NothingToShow` is known only after the league sync has run.
    if outcome == LeagueEditOutcome::Done {
        tx.commit()?;
    } else {
        tx.rollback()?;
    }
    Ok(outcome)
}

/// Whether `edit` changes an Arena run other than by its status, or moves a
/// match out of one.
fn touches_arena_run(tx: &Transaction, edit: &LeagueEdit) -> Result<bool, StoreError> {
    let touched = match edit {
        LeagueEdit::CreateManual { .. } => false,
        LeagueEdit::LinkMatches {
            run_key,
            match_tokens,
            ..
        } => is_arena_run_key(run_key) || any_in_arena_run(tx, match_tokens)?,
        LeagueEdit::UnlinkMatch {
            run_key,
            match_token,
            ..
        } => is_arena_run_key(run_key) || any_in_arena_run(tx, std::slice::from_ref(match_token))?,
        // An Arena run takes the player's status (#834): `user_state` and
        // its edit time, which its bundle carries, and the shown state reads
        // it over the course's.
        LeagueEdit::SetStatus { .. } => false,
        LeagueEdit::SetNotes { run_key, .. }
        | LeagueEdit::Delete { run_key, .. }
        | LeagueEdit::Restore { run_key, .. } => is_arena_run_key(run_key),
        LeagueEdit::UndoDelete { league_id, .. } => tx
            .prepare_cached("SELECT run_key FROM leagues WHERE id = ?1")?
            .query_row([league_id], |row| row.get::<_, String>(0))
            .optional()?
            .is_some_and(|run_key| is_arena_run_key(&run_key)),
        LeagueEdit::MoveMatch {
            match_token,
            to_run_key,
            ..
        } => {
            to_run_key.as_deref().is_some_and(is_arena_run_key)
                || any_in_arena_run(tx, std::slice::from_ref(match_token))?
        }
    };
    Ok(touched)
}

/// Whether any of these matches is in an Arena run now.
fn any_in_arena_run(tx: &Transaction, match_tokens: &[String]) -> Result<bool, StoreError> {
    let mut stmt = tx.prepare_cached(
        "SELECT l.run_key FROM matches m JOIN leagues l ON l.id = m.league_id
         WHERE m.token = ?1",
    )?;
    for token in match_tokens {
        let run_key: Option<String> = stmt.query_row([token], |row| row.get(0)).optional()?;
        if run_key.is_some_and(|run_key| is_arena_run_key(&run_key)) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// 0.x `StoreManualLeagueRequest` and `StoreController`: a `manual` row with
/// `id` = MAX(id) + 1 (so `run_key` = `manual:{id}` is written once), the
/// deck's format and its newest version.
fn create_manual(tx: &Transaction, wanted: &ManualRow) -> Result<LeagueEditOutcome, StoreError> {
    let (Some(account_id), Some(deck_id)) = (wanted.account_id, wanted.deck_id) else {
        return Ok(LeagueEditOutcome::InvalidDeck);
    };
    let live_deck: Option<String> = tx
        .query_row(
            "SELECT format FROM decks WHERE id = ?1 AND account_id = ?2 AND archived_at IS NULL",
            params![deck_id, account_id],
            |row| row.get(0),
        )
        .optional()?;
    let Some(format) = live_deck else {
        return Ok(LeagueEditOutcome::InvalidDeck);
    };
    let name = wanted.name.trim();
    if name.is_empty() {
        return Ok(LeagueEditOutcome::NameRequired);
    }
    if name.chars().count() > MAX_LEAGUE_NAME_CHARS {
        return Ok(LeagueEditOutcome::NameTooLong);
    }
    insert_manual_row(
        tx,
        &ManualRow {
            format: Some(format),
            name: name.to_string(),
            ..wanted.clone()
        },
    )?;
    Ok(LeagueEditOutcome::Done)
}

/// A manual league to store.
#[derive(Debug, Clone)]
pub(crate) struct ManualRow {
    pub(crate) account_id: Option<i64>,
    pub(crate) deck_id: Option<i64>,
    /// The deck's format code.
    pub(crate) format: Option<String>,
    pub(crate) name: String,
    /// `manual_` and 24 letters and digits (0.x's token, or
    /// `manual_league_token`'s).
    pub(crate) token: String,
    pub(crate) started_at: i64,
    pub(crate) created_at: i64,
    /// The 0.x `leagues.id` of one imported from 0.x.
    pub(crate) imported_from: Option<i64>,
}

/// A new manual league's token (0.x `StoreController`: `manual_` and
/// `Str::random(24)`), from random bytes (64 are plenty). Should the bytes
/// run out, the rest are taken from all of them modulo 62, so the token is
/// always whole.
pub fn manual_league_token(random: &[u8]) -> String {
    const LETTERS: &[u8; 62] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    let letters = match mymtgo_api::oauth::alphanumeric(random, 24) {
        Some(letters) => letters,
        None => (0..24)
            .map(|index| {
                let byte = random
                    .get(index % random.len().max(1))
                    .copied()
                    .unwrap_or(0);
                LETTERS[(usize::from(byte) + index) % 62] as char
            })
            .collect(),
    };
    format!("manual_{letters}")
}

/// The deck's version a manual league made at `created_at` holds (0.x stored
/// the latest): its latest saved at or before then, else its first.
pub(crate) fn manual_version(
    tx: &Transaction,
    deck_id: i64,
    created_at: i64,
) -> Result<Option<i64>, StoreError> {
    let saved_by_then: Option<i64> = tx
        .prepare_cached(
            "SELECT id FROM deck_versions WHERE deck_id = ?1 AND modified_at <= ?2
             ORDER BY modified_at DESC, id DESC LIMIT 1",
        )?
        .query_row(params![deck_id, created_at], |row| row.get(0))
        .optional()?;
    if saved_by_then.is_some() {
        return Ok(saved_by_then);
    }
    Ok(tx
        .prepare_cached(
            "SELECT id FROM deck_versions WHERE deck_id = ?1 ORDER BY modified_at, id LIMIT 1",
        )?
        .query_row([deck_id], |row| row.get(0))
        .optional()?)
}

/// Insert a manual league row; returns its id.
pub(crate) fn insert_manual_row(tx: &Transaction, row: &ManualRow) -> Result<i64, StoreError> {
    let id: i64 = tx.query_row("SELECT COALESCE(MAX(id), 0) + 1 FROM leagues", [], |row| {
        row.get(0)
    })?;
    let version = match row.deck_id {
        Some(deck_id) => manual_version(tx, deck_id, row.created_at)?,
        None => None,
    };
    tx.prepare_cached(
        "INSERT INTO leagues (id, run_key, account_id, source, kind, state, name, format,
             deck_version_id, deck_id, started_at, created_at, imported_from, token)
         VALUES (?1, ?2, ?3, 'manual', 'constructed', 'complete', ?4, ?5, ?6, ?7, ?8, ?9, ?10,
             ?11)",
    )?
    .execute(params![
        id,
        manual_run_key(id),
        row.account_id,
        row.name,
        row.format,
        version,
        row.deck_id,
        row.started_at,
        row.created_at,
        row.imported_from,
        row.token
    ])?;
    Ok(id)
}

/// What a move needs to know about one listed match.
struct MoveCandidate {
    id: i64,
    state: String,
    deck_id: Option<i64>,
    account_id: i64,
    tournament: bool,
    format: Option<String>,
    league_id: Option<i64>,
    /// The owner of the league it is in now, synced too.
    held_by: Option<LeagueOwner>,
}

/// The listed match with this token.
fn move_candidate(tx: &Transaction, token: &str) -> Result<Option<MoveCandidate>, StoreError> {
    candidate_row(tx, false, "m.token = ?1", &token)
}

/// A move candidate from the listed matches (the player's moves), up to its
/// `WHERE`.
const LISTED_CANDIDATE_SQL: &str = "SELECT m.id, m.state, v.deck_id, m.account_id,
         m.tournament_event_id IS NOT NULL, m.format, m.league_id, cl.account_id
     FROM listed_matches m
     LEFT JOIN deck_versions v ON v.id = m.deck_version_id
     LEFT JOIN leagues cl ON cl.id = m.league_id
     WHERE";

/// A move candidate from every derived match, hidden ones too, judged as if
/// listed (`move_refusal_judged_listed`), up to the end of its `WHERE`'s
/// first condition.
const DERIVED_CANDIDATE_SQL: &str = "SELECT m.id, m.state, v.deck_id, m.account_id,
         m.tournament_event_id IS NOT NULL, m.format, m.league_id, cl.account_id
     FROM matches m
     LEFT JOIN deck_versions v ON v.id = m.deck_version_id
     LEFT JOIN leagues cl ON cl.id = m.league_id
     WHERE m.detached_at IS NULL AND";

/// The match `filter` (one `?1`, on `m`) names: a listed one, or with
/// `hidden_too` any derived one (`DERIVED_CANDIDATE_SQL`).
fn candidate_row(
    tx: &Transaction,
    hidden_too: bool,
    filter: &str,
    value: &dyn ToSql,
) -> Result<Option<MoveCandidate>, StoreError> {
    let select = if hidden_too {
        DERIVED_CANDIDATE_SQL
    } else {
        LISTED_CANDIDATE_SQL
    };
    Ok(tx
        .prepare_cached(&format!("{select} {filter}"))?
        .query_row([value], |row| {
            let league_id: Option<i64> = row.get(6)?;
            let league_account: Option<i64> = row.get(7)?;
            Ok(MoveCandidate {
                id: row.get(0)?,
                state: row.get(1)?,
                deck_id: row.get(2)?,
                account_id: row.get(3)?,
                tournament: row.get(4)?,
                format: row.get(5)?,
                league_id,
                held_by: league_id.map(|_| owner_of(league_account)),
            })
        })
        .optional()?)
}

/// Why listed match `match_id` cannot move into league row `league_id`
/// (spec 4.1), `None` when it can. `MoveMatch` and `LinkMatches` call it;
/// the rules live in `refusal_of`. A match
/// already in the league is allowed (nothing to do). A missing or detached
/// league is `NotFound`.
pub(crate) fn move_refusal(
    tx: &Transaction,
    match_id: i64,
    league_id: i64,
) -> Result<Option<LeagueEditOutcome>, StoreError> {
    let candidate = candidate_row(tx, false, "m.id = ?1", &match_id)?;
    refusal_of(tx, candidate, league_id)
}

/// `move_refusal` for match `match_id` judged as if it were listed, whatever
/// its hidden mark: every pulled league link (`import::link_pulled` for a
/// copy, `sync_player::link_target` for a player edit), since the other
/// device may have moved it, then deleted it, or restored and moved it.
/// Judging it in place leaves its `hidden_at`, and so the league it was
/// deleted from, alone.
pub(crate) fn move_refusal_judged_listed(
    tx: &Transaction,
    match_id: i64,
    league_id: i64,
) -> Result<Option<LeagueEditOutcome>, StoreError> {
    let candidate = candidate_row(tx, true, "m.id = ?1", &match_id)?;
    refusal_of(tx, candidate, league_id)
}

/// The move rules for `candidate` (`None`: no such match) and league row
/// `league_id`: the one place they live.
fn refusal_of(
    tx: &Transaction,
    candidate: Option<MoveCandidate>,
    league_id: i64,
) -> Result<Option<LeagueEditOutcome>, StoreError> {
    let Some(league) = attached_row(tx, "id = ?1", &league_id)? else {
        return Ok(Some(LeagueEditOutcome::NotFound));
    };
    let Some(candidate) = candidate else {
        return Ok(Some(LeagueEditOutcome::MatchNotFound));
    };
    if league.removed_at.is_some() {
        return Ok(Some(LeagueEditOutcome::LeagueRemoved));
    }
    if candidate.tournament {
        return Ok(Some(LeagueEditOutcome::Tournament));
    }
    if candidate.state != "complete" && candidate.state != "abandoned" {
        return Ok(Some(LeagueEditOutcome::NotFinished));
    }
    if league
        .account_id
        .is_some_and(|account| account != candidate.account_id)
    {
        return Ok(Some(LeagueEditOutcome::WrongAccount));
    }
    if let Some(refusal) = fit_refusal(
        &league.fit()?,
        candidate.format.as_deref(),
        candidate.deck_id,
    ) {
        return Ok(Some(refusal));
    }
    if league.source == "manual" && candidate.league_id != Some(league.id) {
        let held = manual_count(tx, league.id)?;
        if held + 1 > MANUAL_MAX_MATCHES {
            return Ok(Some(LeagueEditOutcome::TooManyMatches));
        }
    }
    Ok(None)
}

/// How many listed matches manual league row `id` holds.
fn manual_count(tx: &Transaction, id: i64) -> Result<usize, StoreError> {
    let held: i64 = tx.query_row(
        "SELECT COUNT(*) FROM listed_matches WHERE league_id = ?1",
        [id],
        |row| row.get(0),
    )?;
    Ok(held as usize)
}

/// Move matches into league `run_key` (spec 4.1). Every check comes before
/// the first write, so a refusal writes nothing. A match already in the
/// league stays as it is.
fn link_matches(
    tx: &Transaction,
    run_key: &str,
    match_tokens: &[String],
    tz: Tz,
    now_ms: i64,
) -> Result<LeagueEditOutcome, StoreError> {
    let mut wanted: Vec<&String> = Vec::new();
    for token in match_tokens {
        if !wanted.contains(&token) {
            wanted.push(token);
        }
    }
    if wanted.is_empty() {
        return Ok(LeagueEditOutcome::NoMatches);
    }
    let Some(league) = edit_row(tx, run_key)? else {
        return Ok(LeagueEditOutcome::NotFound);
    };
    if league.removed_at.is_some() {
        return Ok(LeagueEditOutcome::LeagueRemoved);
    }
    let mut candidates: Vec<MoveCandidate> = Vec::new();
    for token in &wanted {
        let Some(candidate) = move_candidate(tx, token)? else {
            return Ok(LeagueEditOutcome::MatchNotFound);
        };
        if let Some(refusal) = move_refusal(tx, candidate.id, league.id)? {
            return Ok(refusal);
        }
        candidates.push(candidate);
    }
    candidates.retain(|candidate| candidate.league_id != Some(league.id));
    // `move_refusal` checks one match against the cap; the batch counts
    // together.
    if league.source == "manual"
        && manual_count(tx, league.id)? + candidates.len() > MANUAL_MAX_MATCHES
    {
        return Ok(LeagueEditOutcome::TooManyMatches);
    }

    let mut owners = vec![owner_of(league.account_id)];
    for candidate in &candidates {
        tx.prepare_cached(
            "UPDATE matches SET league_id = ?2, league_linked = 1, no_league = 0, edited_at = ?3
             WHERE id = ?1",
        )?
        .execute(params![candidate.id, league.id, now_ms])?;
        owners.push(LeagueOwner::Account(candidate.account_id));
        if let Some(owner) = candidate.held_by {
            owners.push(owner);
        }
    }
    sync_owners(tx, &owners, tz, now_ms)?;
    Ok(LeagueEditOutcome::Done)
}

/// "Remove from league" (spec 4.2): the listed match of league `run_key`
/// leaves every league; it is no league fold input from now on.
fn unlink_match(
    tx: &Transaction,
    run_key: &str,
    match_token: &str,
    tz: Tz,
    now_ms: i64,
) -> Result<LeagueEditOutcome, StoreError> {
    let Some(league) = edit_row(tx, run_key)? else {
        return Ok(LeagueEditOutcome::NotFound);
    };
    if league.removed_at.is_some() {
        return Ok(LeagueEditOutcome::LeagueRemoved);
    }
    let account: Option<i64> = tx
        .query_row(
            "UPDATE matches SET no_league = 1, league_linked = 0, league_id = NULL, edited_at = ?3
             WHERE token = ?1 AND league_id = ?2 AND hidden_at IS NULL AND detached_at IS NULL
             RETURNING account_id",
            params![match_token, league.id, now_ms],
            |row| row.get(0),
        )
        .optional()?;
    let Some(account) = account else {
        return Ok(LeagueEditOutcome::NotInLeague);
    };
    let owners = [owner_of(league.account_id), LeagueOwner::Account(account)];
    sync_owners(tx, &owners, tz, now_ms)?;
    Ok(LeagueEditOutcome::Done)
}

/// "Move to league…, No league" (spec 4.2): the listed match leaves
/// whatever league it is in, and every league from now on. A tournament
/// match stays in its tournament.
fn leave_every_league(
    tx: &Transaction,
    match_token: &str,
    tz: Tz,
    now_ms: i64,
) -> Result<LeagueEditOutcome, StoreError> {
    let Some(candidate) = move_candidate(tx, match_token)? else {
        return Ok(LeagueEditOutcome::MatchNotFound);
    };
    if candidate.tournament {
        return Ok(LeagueEditOutcome::Tournament);
    }
    tx.prepare_cached(
        "UPDATE matches SET no_league = 1, league_linked = 0, league_id = NULL, edited_at = ?2
         WHERE id = ?1",
    )?
    .execute(params![candidate.id, now_ms])?;
    let mut owners = vec![LeagueOwner::Account(candidate.account_id)];
    if let Some(owner) = candidate.held_by {
        owners.push(owner);
    }
    sync_owners(tx, &owners, tz, now_ms)?;
    Ok(LeagueEditOutcome::Done)
}

/// 0.x `UpdateNotesController` (its input trimmed, as Laravel trims every
/// string and turns an empty one into null).
fn set_notes(
    tx: &Transaction,
    run_key: &str,
    notes: &str,
    now_ms: i64,
) -> Result<LeagueEditOutcome, StoreError> {
    let Some(row) = edit_row(tx, run_key)? else {
        return Ok(LeagueEditOutcome::NotFound);
    };
    let notes = notes.trim();
    if notes.chars().count() > MAX_NOTES_CHARS {
        return Ok(LeagueEditOutcome::NotesTooLong);
    }
    let notes = (!notes.is_empty()).then_some(notes);
    tx.prepare_cached("UPDATE leagues SET notes = ?2, notes_edited_at = ?3 WHERE id = ?1")?
        .execute(params![row.id, notes, now_ms])?;
    Ok(LeagueEditOutcome::Done)
}

/// Record the player's drop (spec 3.2): status Dropped, dropped at
/// `now_ms` unless the row was dropped by hand already (its first time
/// stays). The league sync's fold reads it as a manual drop.
fn write_drop(tx: &Transaction, id: i64, now_ms: i64) -> Result<(), StoreError> {
    tx.prepare_cached(
        "UPDATE leagues SET
             user_dropped_at = CASE WHEN user_state = 'dropped' AND user_dropped_at IS NOT NULL
                 THEN user_dropped_at ELSE ?2 END,
             user_state = 'dropped', edited_at = ?2
         WHERE id = ?1",
    )?
    .execute(params![id, now_ms])?;
    Ok(())
}

/// "Set status" (spec 4.3). Dropped records a drop the fold reads; the
/// others and Automatic only set `user_state` (the fold ignores an old
/// `user_dropped_at` once the status is not Dropped).
fn set_status(
    tx: &Transaction,
    run_key: &str,
    status: Option<UserState>,
    tz: Tz,
    now_ms: i64,
) -> Result<LeagueEditOutcome, StoreError> {
    let Some(row) = edit_row(tx, run_key)? else {
        return Ok(LeagueEditOutcome::NotFound);
    };
    if row.removed_at.is_some() {
        return Ok(LeagueEditOutcome::LeagueRemoved);
    }
    match status {
        Some(UserState::Dropped) => write_drop(tx, row.id, now_ms)?,
        other => {
            tx.prepare_cached("UPDATE leagues SET user_state = ?2, edited_at = ?3 WHERE id = ?1")?
                .execute(params![row.id, other.map(UserState::as_str), now_ms])?;
        }
    }
    sync_owners(tx, &[owner_of(row.account_id)], tz, now_ms)?;
    Ok(LeagueEditOutcome::Done)
}

/// "Delete league…" (spec 4.4): refused while the league shows Active. A
/// run the fold still has Active (the player marked it Complete or Partial)
/// is dropped too, so the MTGO league's later matches start a new league.
/// "Delete them too" hides each listed member with exactly the league's
/// `removed_at`, which is how Undo finds them again.
fn delete_league(
    tx: &Transaction,
    run_key: &str,
    matches: DeleteMatches,
    tz: Tz,
    now_ms: i64,
) -> Result<LeagueEditOutcome, StoreError> {
    let Some(row) = edit_row(tx, run_key)? else {
        return Ok(LeagueEditOutcome::NotFound);
    };
    if row.removed_at.is_some() {
        return Ok(LeagueEditOutcome::LeagueRemoved);
    }
    let shown = row.user_state.as_deref().unwrap_or(row.state.as_str());
    if shown == "active" {
        return Ok(LeagueEditOutcome::StillActive);
    }
    if row.state == "active" && row.user_state.as_deref() != Some("dropped") {
        write_drop(tx, row.id, now_ms)?;
    }
    tx.prepare_cached("UPDATE leagues SET removed_at = ?2, edited_at = ?2 WHERE id = ?1")?
        .execute(params![row.id, now_ms])?;
    if matches == DeleteMatches::DeleteToo {
        tx.prepare_cached(
            "UPDATE matches SET hidden_at = ?2, edited_at = ?2
             WHERE league_id = ?1 AND hidden_at IS NULL AND detached_at IS NULL",
        )?
        .execute(params![row.id, now_ms])?;
    }
    sync_owners(tx, &[owner_of(row.account_id)], tz, now_ms)?;
    Ok(LeagueEditOutcome::Done)
}

/// Whether match `m` was deleted together with league `l` (spec 4.5 and
/// section 13): an SQL condition over the aliases `m` (`matches`) and `l`
/// (`leagues`). The match was in the league when it was hidden
/// (`deleted_from_league_id`, recorded by the schema's triggers) and was
/// hidden at exactly the league's `removed_at` ("Delete them too" hides
/// each member with the league's time). The equal time also says the
/// league is still deleted: a listed league's `removed_at` is NULL, and
/// comparing NULL is never true. The column says which league; the equal
/// time says the one action hid both. The one rule for league Restore and Undo (`bring_back`), a match's
/// Restore (refused for these, issue #418) and the Deleted list
/// (`deleted::deleted_items`).
pub(crate) const DELETED_WITH_LEAGUE: &str =
    "m.deleted_from_league_id = l.id AND m.hidden_at = l.removed_at";

/// Whether match `match_id` was deleted together with its league
/// (`DELETED_WITH_LEAGUE`).
pub(crate) fn deleted_with_league(conn: &Connection, match_id: i64) -> Result<bool, StoreError> {
    let sql = format!(
        "SELECT EXISTS (SELECT 1 FROM matches m JOIN leagues l ON {DELETED_WITH_LEAGUE}
                        WHERE m.id = ?1)"
    );
    let found = conn
        .prepare_cached(&sql)?
        .query_row([match_id], |row| row.get(0))?;
    Ok(found)
}

/// The Undo of a delete at `removed_at` (spec 4.5) of the league row `id`:
/// the league is listed again and the matches its "Delete them too" hid
/// come back (`bring_back`). The row is found by id, so a rekey since the
/// delete (its first match deleted on its own) cannot lose it. A league
/// restored or deleted again since is left as it is. Refused when it would
/// leave the league unlisted (`listed_or_refused`).
fn undo_delete(
    tx: &Transaction,
    id: i64,
    removed_at: i64,
    tz: Tz,
    now_ms: i64,
) -> Result<LeagueEditOutcome, StoreError> {
    let Some(row) = id_row(tx, id)? else {
        return Ok(LeagueEditOutcome::NotFound);
    };
    if row.removed_at != Some(removed_at) {
        return Ok(LeagueEditOutcome::Done);
    }
    bring_back(tx, row.id, now_ms)?;
    sync_owners(tx, &[owner_of(row.account_id)], tz, now_ms)?;
    listed_or_refused(tx, row.id)
}

/// After a league Restore or Undo and its sync: `Done` when the row is
/// listed again, else `NothingToShow`. A league with no match left in it
/// (each restored on its own and back non-league, or still deleted on its
/// own) has nothing holding it: the sync leaves it detached (or deletes it),
/// and it would be in neither Leagues nor Archives. `edit_league` rolls the refusal back, so it
/// stays deleted, with its notes, in Archives.
fn listed_or_refused(tx: &Transaction, id: i64) -> Result<LeagueEditOutcome, StoreError> {
    let listed: bool = tx
        .prepare_cached(
            "SELECT EXISTS (SELECT 1 FROM listed_leagues WHERE id = ?1 AND detached_at IS NULL)",
        )?
        .query_row([id], |row| row.get(0))?;
    if listed {
        Ok(LeagueEditOutcome::Done)
    } else {
        Ok(LeagueEditOutcome::NothingToShow)
    }
}

/// Lists the deleted league `id` again (`removed_at` cleared, `edited_at =
/// now`) and brings back the matches its "Delete them too" hid
/// (`DELETED_WITH_LEAGUE`, found before the league is listed again). Each
/// gets `edited_at = now`.
fn bring_back(tx: &Transaction, id: i64, now_ms: i64) -> Result<(), StoreError> {
    let deleted_too = format!(
        "UPDATE matches SET hidden_at = NULL, edited_at = ?2
         WHERE id IN (SELECT m.id FROM matches m JOIN leagues l
                      ON l.id = ?1 AND {DELETED_WITH_LEAGUE})"
    );
    tx.prepare_cached(&deleted_too)?
        .execute(params![id, now_ms])?;
    tx.prepare_cached("UPDATE leagues SET removed_at = NULL, edited_at = ?2 WHERE id = ?1")?
        .execute(params![id, now_ms])?;
    Ok(())
}

/// "Restore" (spec 4.5): the league is listed again with the matches it
/// kept (their `league_id` never changed) and the matches its "Delete them
/// too" hid, as Undo does. Refused when that would leave it unlisted
/// (`listed_or_refused`).
fn restore_league(
    tx: &Transaction,
    run_key: &str,
    tz: Tz,
    now_ms: i64,
) -> Result<LeagueEditOutcome, StoreError> {
    let Some(row) = key_row(tx, run_key)? else {
        return Ok(LeagueEditOutcome::NotFound);
    };
    if row.removed_at.is_none() {
        return Ok(LeagueEditOutcome::Done);
    }
    bring_back(tx, row.id, now_ms)?;
    sync_owners(tx, &[owner_of(row.account_id)], tz, now_ms)?;
    listed_or_refused(tx, row.id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::league_sync::{
        FoldInputs, IMPORTED_SEEDS, ImportedSeed, fold_inputs, members_scope, plain_scope,
    };
    use crate::test_support::number as count;
    use crate::test_support::*;
    use crate::write::{NewLogFile, WriteBatch, commit, insert_log_file};
    use fixture_tools::TestDir;
    use leagues::{ImportedRunSeed, fold_runs};
    use mtgo_match::{GameResult as GameOutcome, MatchState, Outcome};
    use mtgo_protocol::{EventKind, Subjects};
    use rusqlite::params;

    const LEAGUE: &str = "d2050286-53fd-4072-804f-190d6a3c030a";
    const LEAGUE_2: &str = "7f1e2d3c-4b5a-4968-8776-655443322110";
    const T0: i64 = 1_780_000_000_000;
    const MINUTE: i64 = 60_000;

    /// A match token: `n` makes it unique.
    fn tok(n: u32) -> String {
        format!("00000000-0000-4000-8000-{n:012}")
    }

    fn account(conn: &Connection, name: &str) -> i64 {
        conn.execute(
            "INSERT INTO accounts (username, first_seen_at) VALUES (?1, 0)
             ON CONFLICT (username) DO NOTHING",
            [name],
        )
        .expect("account");
        conn.query_row(
            "SELECT id FROM accounts WHERE username = ?1",
            [name],
            |row| row.get(0),
        )
        .expect("account id")
    }

    /// A stored match, built with `played` and changed field by field.
    #[derive(Clone)]
    struct Played {
        token: String,
        account: &'static str,
        source: &'static str,
        league: Option<&'static str>,
        format: &'static str,
        tournament: Option<(i64, i64)>,
        state: &'static str,
        outcome: &'static str,
        started_at: i64,
        ended_at: Option<i64>,
    }

    /// A complete log win by Qlocal in `LEAGUE`, started `minutes` after T0,
    /// lasting a minute.
    fn played(n: u32, minutes: i64) -> Played {
        let started_at = T0 + minutes * MINUTE;
        Played {
            token: tok(n),
            account: "Qlocal",
            source: "log",
            league: Some(LEAGUE),
            format: "CMODERN",
            tournament: None,
            state: "complete",
            outcome: "win",
            started_at,
            ended_at: Some(started_at + MINUTE),
        }
    }

    fn insert(conn: &Connection, m: &Played) -> i64 {
        let account_id = account(conn, m.account);
        conn.execute(
            "INSERT INTO opponents (username, first_seen_at) VALUES ('Qopp', 0)
             ON CONFLICT (username) DO NOTHING",
            [],
        )
        .expect("opponent");
        conn.query_row(
            "INSERT INTO matches (token, account_id, opponent_id, source, format, match_type,
                 tournament_event_id, tournament_round, state, outcome, started_at, ended_at,
                 last_activity_at, revision, derived_version, league_token, game_structure)
             VALUES (?1, ?2, (SELECT id FROM opponents WHERE username = 'Qopp'), ?3, ?4,
                 'Modern', ?5, ?6, ?7, ?8, ?9, ?10, ?9, 1, 4, ?11, 'Modern')
             RETURNING id",
            params![
                m.token,
                account_id,
                m.source,
                m.format,
                m.tournament.map(|(event, _)| event),
                m.tournament.map(|(_, round)| round),
                m.state,
                m.outcome,
                m.started_at,
                m.ended_at,
                m.league,
            ],
            |row| row.get(0),
        )
        .expect("match")
    }

    fn game(conn: &Connection, match_id: i64, number: i64, result: &str, on_play: bool) {
        conn.execute(
            "INSERT INTO games (match_id, game_number, result, end_reason, on_play, started_at,
                 ended_at)
             VALUES (?1, ?2, ?3, 'win', ?4, ?5, ?6)",
            params![
                match_id,
                number,
                result,
                on_play,
                number * 10,
                number * 10 + 5
            ],
        )
        .expect("game");
    }

    fn deck(conn: &Connection, id: i64, account_id: Option<i64>, archived: bool) {
        conn.execute(
            "INSERT INTO decks (id, mtgo_id, account_id, name, format, archived_at, created_at,
                 updated_at, source)
             VALUES (?1, 'Q-deck-' || ?1, ?2, 'Qdeck', 'CMODERN', ?3, 1, 1, 'file')",
            params![id, account_id, archived.then_some(5_i64)],
        )
        .expect("deck");
    }

    fn version(conn: &Connection, id: i64, deck_id: i64) {
        conn.execute(
            "INSERT INTO deck_versions (id, deck_id, signature, modified_at)
             VALUES (?1, ?2, 'Q-sig-' || ?1, ?1)",
            params![id, deck_id],
        )
        .expect("version");
    }

    fn link(conn: &Connection, token: &str, version_id: i64) {
        conn.execute(
            "UPDATE matches SET deck_version_id = ?2 WHERE token = ?1",
            params![token, version_id],
        )
        .expect("link");
    }

    fn manual(conn: &Connection, id: i64, account_id: Option<i64>, deck_id: Option<i64>) {
        insert_manual_league(
            conn,
            id,
            account_id,
            deck_id,
            &format!("Qmanual {id}"),
            id * 100,
            id * 100 + 1,
        );
    }

    fn manual_member(conn: &Connection, id: i64, token: &str) {
        link_league_match(conn, id, token);
    }

    /// A 0.x constructed run `0x:{id}` as the import stores it.
    fn seed(state: &str) -> ImportedSeed {
        ImportedSeed {
            token: "Q-0x-token".to_string(),
            event_id: Some(9),
            name: "Qimported".to_string(),
            format: Some("CMODERN".to_string()),
            state: state.to_string(),
            started_at: 100,
            joined_at: Some(90),
            completed_at: None,
            dropped_at: Some(150),
            deck_version_id: None,
            kind: "constructed".to_string(),
            set_code: None,
            mtgo_course_id: None,
            legacy_id: None,
        }
    }

    fn imported_run(conn: &Connection, id: i64, account_id: i64, state: &str) {
        insert_imported_run(conn, id, Some(account_id), &seed(state));
    }

    fn imported_member(conn: &Connection, id: i64, token: &str) {
        link_imported_match(conn, id, token);
    }

    /// The stored constructed runs of `owner` from `source`.
    fn runs_of(conn: &Connection, owner: LeagueOwner, source: &str) -> Vec<StoredRun> {
        let filter = RunFilter {
            owners: vec![owner],
            kinds: vec![LeagueKind::Constructed],
            deck_id: None,
            run_key: None,
        };
        stored_runs(conn, &filter)
            .expect("stored runs")
            .into_iter()
            .filter(|run| run.source == source)
            .collect()
    }

    #[test]
    fn league_account_ids_skip_arena_accounts() {
        let (_dir, conn) = db("leagues-arena-ids");
        let mtgo = account(&conn, "Qmtgo");
        conn.execute(
            "INSERT INTO accounts (username, first_seen_at, platform, arena_user_id)
             VALUES ('Qarena#12345', 0, 'arena', 'Qarena-id')",
            [],
        )
        .expect("arena account");
        assert_eq!(account_ids(&conn, Platform::Mtgo).expect("ids"), vec![mtgo]);
    }

    #[test]
    fn the_fold_never_takes_a_pending_match() {
        let row = |state: &str| LeagueMatchRow {
            token: tok(1),
            source: "log".to_string(),
            league_token: Some(LEAGUE.to_string()),
            format: Some("CMODERN".to_string()),
            game_structure: None,
            tournament_event_id: None,
            tournament_round: None,
            state: state.to_string(),
            outcome: "unknown".to_string(),
            started_at: T0,
            ended_at: None,
            opponent: None,
            deck_version_id: None,
            deck_id: None,
            games: Vec::new(),
            opponent_archetype_id: None,
            opponent_archetype: None,
        };

        assert_eq!(any_league_match(&row("pending")), None);
        let taken = any_league_match(&row("in_progress")).expect("taken");
        assert_eq!(taken.state, MatchState::InProgress);
    }

    fn result(result: GameOutcome, on_play: bool, number: i64) -> GameResult {
        GameResult {
            result,
            on_play: Some(on_play),
            started_at: Some(number * 10),
            ended_at: Some(number * 10 + 5),
        }
    }

    fn input_matches(inputs: &FoldInputs) -> Vec<LeagueMatch> {
        inputs
            .inputs
            .iter()
            .filter_map(|(_, _, _, input)| match input {
                LeagueInput::Match(played) => Some(played.clone()),
                _ => None,
            })
            .collect()
    }

    fn non_match_inputs(inputs: &FoldInputs) -> Vec<(i64, u64, u64, LeagueInput)> {
        let mut found: Vec<(i64, u64, u64, LeagueInput)> = inputs
            .inputs
            .iter()
            .filter(|(_, _, _, input)| !matches!(input, LeagueInput::Match(_)))
            .cloned()
            .collect();
        found.sort_by_key(|(at, file, offset, _)| (*at, *file, *offset));
        found
    }

    fn tokens(rows: &[LeagueMatchRow]) -> Vec<String> {
        rows.iter().map(|row| row.token.clone()).collect()
    }

    // League inputs: matches

    #[test]
    fn league_inputs_are_the_accounts_league_matches_with_their_decks_and_games() {
        let (_dir, conn) = db("leagues-inputs-matches");
        let local = account(&conn, "Qlocal");
        deck(&conn, 1, Some(local), false);
        version(&conn, 10, 1);
        let first = insert(&conn, &played(1, 0));
        // Stored out of order: read back in game order.
        game(&conn, first, 2, "win", false);
        game(&conn, first, 1, "loss", true);
        link(&conn, &tok(1), 10);
        insert(&conn, &played(2, 10));
        insert(
            &conn,
            &Played {
                account: "Qother",
                ..played(3, 20)
            },
        );

        let inputs = fold_inputs(&conn, LeagueOwner::Account(local)).expect("inputs");
        let started = T0;
        assert_eq!(
            input_matches(&inputs),
            vec![
                LeagueMatch {
                    token: tok(1),
                    league_token: LEAGUE.to_string(),
                    started_at: started,
                    ended_at: Some(started + MINUTE),
                    state: MatchState::Complete,
                    outcome: Outcome::Win,
                    format: Some("CMODERN".to_string()),
                    game_structure: Some("Modern".to_string()),
                    deck_version_id: Some(10),
                    games: vec![
                        result(GameOutcome::Loss, true, 1),
                        result(GameOutcome::Win, false, 2)
                    ],
                },
                LeagueMatch {
                    token: tok(2),
                    league_token: LEAGUE.to_string(),
                    started_at: started + 10 * MINUTE,
                    ended_at: Some(started + 11 * MINUTE),
                    state: MatchState::Complete,
                    outcome: Outcome::Win,
                    format: Some("CMODERN".to_string()),
                    game_structure: Some("Modern".to_string()),
                    deck_version_id: None,
                    games: Vec::new(),
                },
            ]
        );
        for (order, file, offset, input) in &inputs.inputs {
            if let LeagueInput::Match(played) = input {
                assert_eq!(
                    (*order, *file, *offset),
                    (played.started_at, MATCH_POSITION.0, MATCH_POSITION.1)
                );
            }
        }
        assert_eq!(tokens(&inputs.matches), vec![tok(1), tok(2)]);
        let row = &inputs.matches[0];
        assert_eq!((row.deck_version_id, row.deck_id), (Some(10), Some(1)));
        assert_eq!(row.opponent.as_deref(), Some("Qopp"));
        let run_match = row.run_match();
        assert_eq!(run_match.deck_id, Some(1));
        assert_eq!(run_match.games.len(), 2);
        assert_eq!(inputs.matches[1].deck_id, None);

        let runs = fold_runs(inputs.inputs, chrono_tz::UTC);
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].match_tokens, vec![tok(1), tok(2)]);
        assert_eq!(runs[0].key, format!("{LEAGUE}:{}", tok(1)));
    }

    /// An Arena opponent's username is its user id (`arena:...`): its row
    /// shows the display name. An MTGO opponent has none and shows its
    /// username.
    #[test]
    fn a_match_row_names_an_arena_opponent_by_its_display_name() {
        let (_dir, conn) = db("leagues-arena-opponent");
        let local = account(&conn, "Qlocal");
        insert(&conn, &played(1, 0));
        insert(&conn, &played(2, 10));
        conn.execute(
            "INSERT INTO opponents (username, first_seen_at, platform, display_name)
             VALUES ('arena:QOPPONENTID', 0, 'arena', 'Qrival')",
            [],
        )
        .expect("arena opponent");
        conn.execute(
            "UPDATE matches SET opponent_id = (SELECT id FROM opponents
                 WHERE username = 'arena:QOPPONENTID')
             WHERE token = ?1",
            [tok(2)],
        )
        .expect("arena opponent's match");

        let inputs = fold_inputs(&conn, LeagueOwner::Account(local)).expect("inputs");
        let names: Vec<Option<&str>> = inputs
            .matches
            .iter()
            .map(|row| row.opponent.as_deref())
            .collect();
        assert_eq!(names, vec![Some("Qopp"), Some("Qrival")]);
    }

    #[test]
    fn league_inputs_leave_out_tournament_imported_early_and_tokenless_matches() {
        let (_dir, conn) = db("leagues-inputs-filters");
        let local = account(&conn, "Qlocal");
        insert(&conn, &played(1, 0));
        insert(
            &conn,
            &Played {
                tournament: Some((500, 1)),
                ..played(2, 1)
            },
        );
        for (n, format) in [(3, "DMKM"), (4, "S1QQ")] {
            insert(
                &conn,
                &Played {
                    format,
                    ..played(n, 2)
                },
            );
        }
        insert(
            &conn,
            &Played {
                source: "imported",
                ..played(5, 3)
            },
        );
        insert(
            &conn,
            &Played {
                state: "pending",
                ended_at: None,
                ..played(6, 4)
            },
        );
        insert(
            &conn,
            &Played {
                league: None,
                ..played(7, 5)
            },
        );
        insert(
            &conn,
            &Played {
                league: Some(""),
                ..played(8, 6)
            },
        );
        for (n, state) in [(9, "in_progress"), (10, "ended"), (11, "abandoned")] {
            insert(
                &conn,
                &Played {
                    state,
                    ..played(n, 7 + i64::from(n))
                },
            );
        }

        let inputs = fold_inputs(&conn, LeagueOwner::Account(local)).expect("inputs");
        let kept: Vec<String> = input_matches(&inputs)
            .into_iter()
            .map(|played| played.token)
            .collect();
        // Limited matches (3, 4) are inputs too: the sync folds them with the
        // limited rules.
        assert_eq!(kept, vec![tok(1), tok(3), tok(4), tok(9), tok(10), tok(11)]);
        assert_eq!(tokens(&inputs.matches), kept);
    }

    // League inputs: events

    fn league_event(
        file: i64,
        offset: u64,
        kind: EventKind,
        joined: Option<(&str, u64)>,
    ) -> crate::write::NewEvent {
        let subjects = Subjects {
            league_token: joined.map(|(token, _)| token.to_string()),
            league_event_id: joined.map(|(_, id)| id),
            ..Subjects::default()
        };
        event_with(file, offset, kind, subjects, "12:00:00 [INF] (UI|Qleague)")
    }

    fn login(file: i64, offset: u64, name: &str) -> crate::write::NewEvent {
        event_with(
            file,
            offset,
            EventKind::Login,
            Subjects::default(),
            &format!("12:00:00 [INF] (Login|MtGO Login Success) Username: {name} (7)"),
        )
    }

    fn log_file(conn: &Connection, path: &str, inherited: Option<&str>) -> i64 {
        insert_log_file(
            conn,
            &NewLogFile {
                inherited_username: inherited.map(str::to_string),
                ..new_log_file(path)
            },
        )
        .expect("file")
    }

    #[test]
    fn league_events_belong_to_the_player_logged_in_at_them() {
        let (_dir, mut conn) = db("leagues-inputs-events");
        let local = account(&conn, "Qlocal");
        let other = account(&conn, "Qother");
        let a = log_file(&conn, "C:/a/mtgo.log", Some("Qlocal"));
        let b = log_file(&conn, "C:/b/mtgo.log", Some("Qlocal"));
        let c = log_file(&conn, "C:/c/mtgo.log", None);
        let mut quarantined = league_event(a, 500, EventKind::LeagueDropped, None);
        quarantined.quarantined = true;
        let events = vec![
            login(a, 0, "Qlocal"),
            league_event(a, 100, EventKind::LeagueJoined, Some((LEAGUE, 777))),
            // A panel view without an event id is not a join.
            league_event(a, 150, EventKind::LeagueJoined, None),
            login(a, 200, "Qother"),
            league_event(a, 300, EventKind::LeagueJoined, Some((LEAGUE_2, 888))),
            league_event(a, 400, EventKind::LeagueDropped, None),
            quarantined,
            // No Login in this file: its inherited player.
            league_event(b, 50, EventKind::LeagueDropped, None),
            // Nobody known.
            league_event(c, 60, EventKind::LeagueDropped, None),
        ];
        commit(
            &mut conn,
            &WriteBatch {
                events,
                now_ms: 1,
                ..WriteBatch::default()
            },
        )
        .expect("events");

        let at = |offset: i64| 1_000 + offset;
        assert_eq!(
            non_match_inputs(&fold_inputs(&conn, LeagueOwner::Account(local)).expect("local")),
            vec![
                (at(50), b as u64, 50, LeagueInput::Dropped { at_ms: at(50) }),
                (
                    at(100),
                    a as u64,
                    100,
                    LeagueInput::Joined {
                        token: LEAGUE.to_string(),
                        event_id: 777,
                        at_ms: at(100),
                        rounds: None,
                    }
                ),
            ]
        );
        assert_eq!(
            non_match_inputs(&fold_inputs(&conn, LeagueOwner::Account(other)).expect("other")),
            vec![
                (
                    at(300),
                    a as u64,
                    300,
                    LeagueInput::Joined {
                        token: LEAGUE_2.to_string(),
                        event_id: 888,
                        at_ms: at(300),
                        rounds: None,
                    }
                ),
                (
                    at(400),
                    a as u64,
                    400,
                    LeagueInput::Dropped { at_ms: at(400) }
                ),
            ]
        );
    }

    #[test]
    fn manual_drops_of_derived_and_imported_runs_are_inputs() {
        let (_dir, conn) = db("leagues-inputs-drops");
        let local = account(&conn, "Qlocal");
        // A dropped derived run, a manual league (never dropped) and a 0.x run.
        conn.execute(
            "INSERT INTO leagues (id, run_key, account_id, source, kind, state, token, name,
                 started_at, created_at, user_dropped_at, user_state)
             VALUES (50, ?1, ?2, 'log', 'constructed', 'dropped', ?3, 'Qrun', 1, 1, 5, 'dropped')",
            params![format!("{LEAGUE}:{}", tok(1)), local, LEAGUE],
        )
        .expect("a dropped run");
        manual(&conn, 1, Some(local), None);
        conn.execute(
            "UPDATE leagues SET user_dropped_at = 6, user_state = 'dropped' WHERE run_key = 'manual:1'",
            [],
        )
        .expect("a manual league's drop");
        // Another account's, so it is no seed here: its drop is still read.
        let other = account(&conn, "Qother");
        imported_run(&conn, 2, other, "active");
        conn.execute(
            "UPDATE leagues SET user_dropped_at = 7, user_state = 'dropped' WHERE run_key = '0x:2'",
            [],
        )
        .expect("a 0.x run's drop");
        assert_eq!(
            non_match_inputs(&fold_inputs(&conn, LeagueOwner::Account(local)).expect("inputs")),
            vec![
                (
                    5,
                    0,
                    0,
                    LeagueInput::ManualDrop {
                        run_key: format!("{LEAGUE}:{}", tok(1)),
                        at_ms: 5
                    }
                ),
                (
                    7,
                    0,
                    0,
                    LeagueInput::ManualDrop {
                        run_key: "0x:2".to_string(),
                        at_ms: 7
                    }
                )
            ]
        );
        let drops = manual_drops(&conn).expect("drops");
        assert_eq!(drops.len(), 3);
        assert_eq!(drops.get("0x:2"), Some(&7));
    }

    // Imported runs are fold seeds (final review Important 1)

    fn seeds(inputs: &FoldInputs) -> Vec<(i64, u64, u64, ImportedRunSeed)> {
        inputs
            .inputs
            .iter()
            .filter_map(|(at, file, offset, input)| match input {
                LeagueInput::ImportedRun(seed) => Some((*at, *file, *offset, seed.clone())),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn league_inputs_seed_the_accounts_imported_runs_and_keep_their_members_out_of_the_plain_matches()
     {
        let (_dir, conn) = db("leagues-inputs-imported");
        let local = account(&conn, "Qlocal");
        let other = account(&conn, "Qother");
        deck(&conn, 1, Some(local), false);
        version(&conn, 10, 1);
        imported_run(&conn, 42, local, "active");
        set_seed(&conn, 42, "$.deck_version_id", 10.into());
        imported_run(&conn, 43, other, "complete");
        // A 0.x match v1 only has from the import, and one v1 tracked live
        // (with its league token) that the import claimed by its token.
        let copy = insert(
            &conn,
            &Played {
                source: "imported",
                league: None,
                ..played(1, 0)
            },
        );
        game(&conn, copy, 1, "win", true);
        insert(&conn, &played(2, 10));
        link(&conn, &tok(2), 10);
        imported_member(&conn, 42, &tok(1));
        imported_member(&conn, 42, &tok(2));
        // A live league match no imported run holds.
        insert(&conn, &played(3, 20));
        conn.execute(
            "UPDATE leagues SET user_dropped_at = 500, user_state = 'dropped' WHERE run_key = '0x:42'",
            [],
        )
        .expect("drop");

        let inputs = fold_inputs(&conn, LeagueOwner::Account(local)).expect("inputs");

        let plain: Vec<String> = input_matches(&inputs)
            .into_iter()
            .map(|played| played.token)
            .collect();
        assert_eq!(plain, vec![tok(3)], "a claimed match is not also plain");
        let found = seeds(&inputs);
        assert_eq!(found.len(), 1, "only the account's runs");
        let (at, file, offset, seed) = &found[0];
        assert_eq!(
            (*at, *file, *offset),
            (100, MATCH_POSITION.0, MATCH_POSITION.1)
        );
        let member_tokens: Vec<String> = seed
            .matches
            .iter()
            .map(|played| played.token.clone())
            .collect();
        assert_eq!(member_tokens, vec![tok(1), tok(2)]);
        assert_eq!(
            ImportedRunSeed {
                matches: Vec::new(),
                ..seed.clone()
            },
            ImportedRunSeed {
                key: "0x:42".to_string(),
                token: "Q-0x-token".to_string(),
                event_id: Some(9),
                joined_at: Some(90),
                name: "Qimported".to_string(),
                format: Some("CMODERN".to_string()),
                deck_version_id: Some(10),
                started_at: 100,
                state: RunState::Active,
                dropped_at: Some(150),
                completed_at: None,
                matches: Vec::new(),
            }
        );
        assert_eq!(
            seed.matches[0].games,
            vec![result(GameOutcome::Win, true, 1)]
        );
        assert_eq!(seed.matches[1].deck_version_id, Some(10));
        let mut rows = tokens(&inputs.matches);
        rows.sort();
        assert_eq!(rows, vec![tok(1), tok(2), tok(3)], "members are rows too");
        assert!(
            non_match_inputs(&inputs)
                .iter()
                .any(|(_, _, _, input)| matches!(
                    input,
                    LeagueInput::ManualDrop { run_key, .. } if run_key == "0x:42"
                )),
            "the imported run's manual drop is an input"
        );

        let runs = fold_runs(inputs.inputs, chrono_tz::UTC);
        let keys: Vec<&str> = runs.iter().map(|run| run.key.as_str()).collect();
        assert_eq!(
            keys,
            vec!["0x:42".to_string(), format!("{LEAGUE}:{}", tok(3))]
        );
    }

    #[test]
    fn league_inputs_without_an_account_seed_only_the_runs_with_no_account() {
        let (_dir, conn) = db("leagues-inputs-imported-nobody");
        let local = account(&conn, "Qlocal");
        imported_run(&conn, 42, local, "complete");
        imported_run(&conn, 43, local, "dropped");
        conn.execute(
            "UPDATE leagues SET account_id = NULL WHERE imported_from = 43",
            [],
        )
        .expect("no account");
        insert(&conn, &played(1, 0));

        let inputs = fold_inputs(&conn, LeagueOwner::Nobody).expect("inputs");

        let keys: Vec<String> = seeds(&inputs)
            .into_iter()
            .map(|(_, _, _, seed)| seed.key)
            .collect();
        assert_eq!(keys, vec!["0x:43".to_string()]);
        assert!(input_matches(&inputs).is_empty(), "no account, no matches");
        assert_eq!(
            seeds(&fold_inputs(&conn, LeagueOwner::Account(local)).expect("inputs")).len(),
            1,
            "the account's own run only"
        );
    }

    #[test]
    fn an_imported_run_finished_by_live_matches_is_complete_to_the_edits() {
        let (_dir, mut conn) = edit_db("leagues-edit-imported-finished");
        imported_run(&conn, 42, 1, "active");
        for n in 1..=2 {
            insert(
                &conn,
                &Played {
                    source: "imported",
                    league: None,
                    ..played(n, i64::from(n))
                },
            );
            imported_member(&conn, 42, &tok(n));
        }
        // Three live matches on its token after the import.
        for n in 3..=5 {
            insert(
                &conn,
                &Played {
                    league: Some("Q-0x-token"),
                    ..played(n, i64::from(n) * 10)
                },
            );
        }
        sync_leagues(&conn);
        // Before any drop, a derived key for its first live match names no run: it joined
        // the imported one.
        assert_eq!(
            edit(
                &mut conn,
                drop_edit(&format!("Q-0x-token:{}", tok(3)), 9_000)
            ),
            LeagueEditOutcome::NotFound
        );
        assert_eq!(
            edit(&mut conn, drop_edit("0x:42", 9_000)),
            LeagueEditOutcome::Done
        );
        assert_eq!(
            edit(
                &mut conn,
                LeagueEdit::SetNotes {
                    run_key: "0x:42".to_string(),
                    notes: "Q note".to_string(),
                    now_ms: 9_000
                }
            ),
            LeagueEditOutcome::Done
        );
    }

    // Manual leagues, imported runs, notes

    #[test]
    fn manual_leagues_list_the_accounts_leagues_with_their_matches() {
        let (_dir, conn) = db("leagues-manual-read");
        let local = account(&conn, "Qlocal");
        let other = account(&conn, "Qother");
        deck(&conn, 1, Some(local), false);
        version(&conn, 10, 1);
        let id = insert(
            &conn,
            &Played {
                league: None,
                ..played(1, 0)
            },
        );
        game(&conn, id, 1, "win", true);
        link(&conn, &tok(1), 10);
        manual(&conn, 1, Some(local), Some(1));
        manual(&conn, 2, Some(local), Some(1));
        manual(&conn, 3, Some(other), None);
        manual_member(&conn, 1, &tok(1));
        // A member whose match row is gone is left out.
        manual_member(&conn, 1, &tok(99));

        let leagues = runs_of(&conn, LeagueOwner::Account(local), "manual");
        assert_eq!(
            leagues.iter().map(|l| l.key.clone()).collect::<Vec<_>>(),
            vec!["manual:2".to_string(), "manual:1".to_string()],
            "newest first"
        );
        let first = &leagues[1];
        assert_eq!(
            (
                first.id,
                first.account_id,
                first.deck_id,
                first.name.as_str()
            ),
            (1, Some(local), Some(1), "Qmanual 1")
        );
        assert_eq!(
            (first.state, first.format.as_deref(), first.deck_version_id),
            (RunState::Complete, Some("CMODERN"), Some(10)),
            "stored when it was made"
        );
        assert!(first.run_input().manual);
        assert_eq!((first.started_at, first.created_at), (100, 101));
        assert_eq!(tokens(&first.matches), vec![tok(1)]);
        assert_eq!(first.matches[0].deck_id, Some(1));
        assert_eq!(
            first.matches[0].games,
            vec![result(GameOutcome::Win, true, 1)]
        );
        assert!(leagues[0].matches.is_empty());
    }

    #[test]
    fn a_run_row_names_its_opponent_archetype_and_a_deck_its_archetype() {
        // 0.x FormatLeagueRuns `opponentArchetype` / `opponentArchetypeId`
        // (the opponent seat only) and the Leagues filter's deck archetype.
        let (_dir, conn) = db("leagues-archetypes-read");
        let local = account(&conn, "Qlocal");
        deck(&conn, 1, Some(local), false);
        version(&conn, 10, 1);
        let first = insert(
            &conn,
            &Played {
                league: None,
                ..played(1, 0)
            },
        );
        let second = insert(
            &conn,
            &Played {
                league: None,
                ..played(2, 0)
            },
        );
        link(&conn, &tok(1), 10);
        link(&conn, &tok(2), 10);
        conn.execute_batch(&format!(
            "INSERT INTO archetypes (id, uuid, name, format, source, created_at, updated_at)
                 VALUES (50, 'q-yawg', 'Qyawgmoth', 'modern', 'downloaded', 0, 0),
                        (51, 'q-mine', 'Qmine', 'modern', 'manual', 0, 0);
             INSERT INTO match_archetypes (match_id, is_opponent, archetype_id, confidence)
                 VALUES ({first}, 1, 50, 1.0), ({first}, 0, 51, 1.0), ({second}, 0, 51, 1.0);
             UPDATE decks SET archetype_id = 51 WHERE id = 1;"
        ))
        .expect("archetypes");
        manual(&conn, 1, Some(local), Some(1));
        manual_member(&conn, 1, &tok(1));
        manual_member(&conn, 1, &tok(2));

        let leagues = runs_of(&conn, LeagueOwner::Account(local), "manual");
        let rows = &leagues[0].matches;
        let named: Vec<(Option<i64>, Option<&str>)> = rows
            .iter()
            .map(|row| (row.opponent_archetype_id, row.opponent_archetype.as_deref()))
            .collect();
        assert_eq!(named, vec![(Some(50), Some("Qyawgmoth")), (None, None)]);
        assert_eq!(
            rows[0].run_match().opponent_archetype.as_deref(),
            Some("Qyawgmoth")
        );
        let decks = league_decks(&conn).expect("decks");
        assert_eq!(decks[0].archetype_id, Some(51));
        assert_eq!(decks[0].archetype_name.as_deref(), Some("Qmine"));
    }

    #[test]
    fn imported_runs_list_the_accounts_runs_with_their_matches() {
        let (_dir, conn) = db("leagues-imported-read");
        let local = account(&conn, "Qlocal");
        let other = account(&conn, "Qother");
        deck(&conn, 1, Some(local), false);
        version(&conn, 10, 1);
        insert_imported_run(
            &conn,
            42,
            Some(local),
            &ImportedSeed {
                deck_version_id: Some(10),
                ..seed("dropped")
            },
        );
        imported_run(&conn, 43, other, "complete");
        insert(
            &conn,
            &Played {
                source: "imported",
                league: None,
                ..played(1, 0)
            },
        );
        imported_member(&conn, 42, &tok(1));
        sync_leagues(&conn);

        let runs = runs_of(&conn, LeagueOwner::Account(local), "imported");
        assert_eq!(
            runs,
            vec![StoredRun {
                id: 1042,
                key: "0x:42".to_string(),
                account_id: Some(local),
                source: "imported".to_string(),
                kind: LeagueKind::Constructed,
                token: Some("Q-0x-token".to_string()),
                event_id: Some(9),
                mtgo_course_id: None,
                set_code: None,
                name: "Qimported".to_string(),
                format: Some("CMODERN".to_string()),
                state: RunState::Dropped,
                started_at: 100,
                joined_at: Some(90),
                completed_at: None,
                dropped_at: Some(150),
                deck_version_id: Some(10),
                deck_id: Some(1),
                created_at: 7,
                notes: None,
                user_state: None,
                rounds: None,
                course_record: None,
                matches: runs[0].matches.clone(),
            }]
        );
        assert_eq!(tokens(&runs[0].matches), vec![tok(1)]);
        assert_eq!(runs[0].matches[0].source, "imported");
    }

    #[test]
    fn leagues_and_runs_without_an_account_are_read_apart() {
        let (_dir, conn) = db("leagues-without-account");
        let local = account(&conn, "Qlocal");
        deck(&conn, 1, None, false);
        version(&conn, 10, 1);
        manual(&conn, 1, Some(local), Some(1));
        manual(&conn, 2, None, Some(1));
        imported_run(&conn, 42, local, "complete");
        imported_run(&conn, 43, local, "dropped");
        conn.execute(
            "UPDATE leagues SET account_id = NULL WHERE imported_from = 43",
            [],
        )
        .expect("no account");
        for (n, minutes) in [(1, 0), (2, 10)] {
            insert(
                &conn,
                &Played {
                    source: "imported",
                    league: None,
                    ..played(n, minutes)
                },
            );
        }
        manual_member(&conn, 2, &tok(1));
        imported_member(&conn, 43, &tok(2));

        let manual_keys: Vec<String> = runs_of(&conn, LeagueOwner::Nobody, "manual")
            .iter()
            .map(|league| league.key.clone())
            .collect();
        assert_eq!(manual_keys, vec!["manual:2".to_string()]);
        let manual_rows = runs_of(&conn, LeagueOwner::Nobody, "manual");
        assert_eq!(tokens(&manual_rows[0].matches), vec![tok(1)]);
        let runs = runs_of(&conn, LeagueOwner::Nobody, "imported");
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].key, "0x:43");
        assert_eq!(tokens(&runs[0].matches), vec![tok(2)]);
        // The account's reads leave them out.
        let local_owner = LeagueOwner::Account(local);
        assert_eq!(runs_of(&conn, local_owner, "manual").len(), 1);
        assert_eq!(runs_of(&conn, local_owner, "imported").len(), 1);
    }

    #[test]
    fn decks_versions_match_notes_and_accounts_are_read_whole() {
        let (_dir, conn) = db("leagues-card-reads");
        let local = account(&conn, "Qlocal");
        let other = account(&conn, "Qother");
        deck(&conn, 2, Some(local), true);
        deck(&conn, 1, None, false);
        conn.execute_batch(&format!(
            "UPDATE decks SET name = 'Qalpha', color_identity = 'U,R', cover_catalog_id = 77,
                 cover_file = '2-9.webp'
                 WHERE id = 2;
             INSERT INTO cards (catalog_id, name, art_crop, local_art_crop)
                 VALUES (77, 'Qcard', 'https://q/77.jpg', '77_art.jpg');
             INSERT INTO matches (token, account_id, source, state, outcome, started_at,
                 last_activity_at, revision, derived_version, notes)
             VALUES ('Q-token', {local}, 'log', 'complete', 'win', 1, 1, 1, 0, 'Q note');"
        ))
        .expect("deck details");
        version(&conn, 11, 1);
        version(&conn, 10, 1);
        version(&conn, 20, 2);

        let decks = league_decks(&conn).expect("decks");
        assert_eq!(
            decks[0],
            LeagueDeck {
                id: 2,
                account_id: Some(local),
                name: "Qalpha".to_string(),
                format: "CMODERN".to_string(),
                color_identity: Some("U,R".to_string()),
                cover_art: Some("https://q/77.jpg".to_string()),
                local_cover_art: Some("77_art.jpg".to_string()),
                cover_file: Some("2-9.webp".to_string()),
                archived: true,
                archetype_id: None,
                archetype_name: None,
            },
            "by name"
        );
        assert_eq!((decks[1].id, decks[1].cover_art.clone()), (1, None));
        let versions: Vec<(i64, i64, i64)> = version_dates(&conn)
            .expect("versions")
            .iter()
            .map(|version| (version.id, version.deck_id, version.modified_at))
            .collect();
        assert_eq!(versions, vec![(10, 1, 10), (11, 1, 11), (20, 2, 20)]);
        let notes = notes_by_match(&conn).expect("notes");
        assert_eq!(notes.get("Q-token").map(String::as_str), Some("Q note"));
        assert_eq!(
            account_ids(&conn, Platform::Mtgo).expect("accounts"),
            vec![local, other]
        );
    }

    #[test]
    fn notes_are_read_by_run_key() {
        let (_dir, conn) = db("leagues-notes-read");
        let local = account(&conn, "Qlocal");
        manual(&conn, 1, Some(local), None);
        imported_run(&conn, 2, local, "complete");
        imported_run(&conn, 3, local, "complete");
        conn.execute_batch(
            "UPDATE leagues SET notes = 'Q one' WHERE run_key = 'manual:1';
             UPDATE leagues SET notes = 'Q two' WHERE run_key = '0x:2';",
        )
        .expect("notes");
        let notes = notes_by_run(&conn).expect("notes");
        assert_eq!(notes.len(), 2);
        assert_eq!(notes.get("0x:2").map(String::as_str), Some("Q two"));
    }

    // Available matches

    #[test]
    fn available_matches_are_the_moves_a_league_allows_newest_first() {
        let (_dir, conn) = db("leagues-available");
        let local = account(&conn, "Qlocal");
        deck(&conn, 1, Some(local), false);
        deck(&conn, 2, Some(local), false);
        version(&conn, 10, 1);
        version(&conn, 11, 1);
        version(&conn, 20, 2);
        manual(&conn, 1, Some(local), Some(1));
        manual(&conn, 2, Some(local), Some(1));
        manual(&conn, 3, Some(local), None);
        imported_run(&conn, 42, local, "complete");
        let unleagued = |n: u32, minutes: i64| Played {
            league: None,
            ..played(n, minutes)
        };
        // Available to manual league 1: 1, 2 (imported), 6 (in manual league
        // 2), 7 (in run 42), 8 (in its log run), 10 (abandoned). Not: 3 (a
        // tournament round), 4 (deck 2), 5 (in progress), 9 (no deck), 11
        // (in manual league 1 already).
        insert(&conn, &unleagued(1, 0));
        link(&conn, &tok(1), 10);
        insert(
            &conn,
            &Played {
                source: "imported",
                ..unleagued(2, 10)
            },
        );
        link(&conn, &tok(2), 11);
        insert(
            &conn,
            &Played {
                tournament: Some((500, 1)),
                ..unleagued(3, 20)
            },
        );
        link(&conn, &tok(3), 10);
        insert(&conn, &unleagued(4, 30));
        link(&conn, &tok(4), 20);
        insert(
            &conn,
            &Played {
                state: "in_progress",
                ..unleagued(5, 40)
            },
        );
        link(&conn, &tok(5), 10);
        insert(&conn, &unleagued(6, 50));
        link(&conn, &tok(6), 10);
        manual_member(&conn, 2, &tok(6));
        insert(&conn, &unleagued(7, 60));
        link(&conn, &tok(7), 10);
        imported_member(&conn, 42, &tok(7));
        insert(&conn, &played(8, 70));
        link(&conn, &tok(8), 10);
        insert(&conn, &unleagued(9, 80));
        insert(
            &conn,
            &Played {
                state: "abandoned",
                ..unleagued(10, 90)
            },
        );
        link(&conn, &tok(10), 10);
        insert(&conn, &unleagued(11, 100));
        link(&conn, &tok(11), 10);
        manual_member(&conn, 1, &tok(11));
        sync_leagues(&conn);

        let found = available_matches(&conn, "manual:1").expect("available");
        let offered: Vec<(String, Option<String>)> = found
            .iter()
            .map(|m| {
                (
                    m.row.token.clone(),
                    m.now_in.as_ref().map(|l| l.run_key.clone()),
                )
            })
            .collect();
        let log_run = format!("{LEAGUE}:{}", tok(8));
        assert_eq!(
            offered,
            vec![
                (tok(10), None),
                (tok(8), Some(log_run.clone())),
                (tok(7), Some("0x:42".to_string())),
                (tok(6), Some("manual:2".to_string())),
                (tok(2), None),
                (tok(1), None),
            ]
        );
        assert_eq!(
            found[2].now_in.as_ref().map(|l| l.name.as_str()),
            Some("Qimported")
        );

        let any_deck = available_tokens(&conn, "manual:3");
        assert!(
            any_deck.contains(&tok(4)) && any_deck.contains(&tok(9)),
            "{any_deck:?}"
        );
        assert!(!any_deck.contains(&tok(3)), "a tournament round");
        let into_log_run = available_tokens(&conn, &log_run);
        assert!(into_log_run.contains(&tok(1)) && !into_log_run.contains(&tok(8)));
        assert!(available_tokens(&conn, "manual:99").is_empty());
        conn.execute("UPDATE leagues SET removed_at = 1 WHERE id = 2", [])
            .expect("removed");
        assert!(
            available_tokens(&conn, "manual:2").is_empty(),
            "a deleted league takes nothing"
        );
        assert_eq!(AVAILABLE_LIMIT_SQL, format!("LIMIT {AVAILABLE_LIMIT}"));
    }

    /// The pool of a limited or deck-less league (spec 4.1): the account's
    /// complete or abandoned, non-tournament, listed matches of its kind (and
    /// of its set when the run knows it), not already in it.
    #[test]
    fn available_matches_for_limited_and_deck_less_leagues_follow_the_fit_rules() {
        let (_dir, conn) = limited_0x_db("leagues-available-any");
        manual(&conn, 100, Some(1), None);
        let m = |n: u32, format: &'static str| Played {
            league: None,
            format,
            ..played(n, i64::from(n))
        };
        insert(&conn, &m(1, "DHOBHOBHOB"));
        insert(&conn, &m(2, "DFRAFRAFRA"));
        insert(&conn, &m(3, "S6FRA"));
        insert(&conn, &m(4, "CMODERN"));
        insert(&conn, &m(5, "CMODERN"));
        conn.execute(
            "UPDATE matches SET format = NULL WHERE token = ?1",
            [tok(5)],
        )
        .expect("no format");
        insert(
            &conn,
            &Played {
                state: "in_progress",
                ..m(6, "DHOBHOBHOB")
            },
        );
        insert(
            &conn,
            &Played {
                tournament: Some((500, 1)),
                ..m(7, "DHOBHOBHOB")
            },
        );
        insert(
            &conn,
            &Played {
                account: "Qother",
                ..m(8, "DHOBHOBHOB")
            },
        );
        insert(&conn, &m(9, "DHOBHOBHOB"));
        hide(&conn, &tok(9));
        insert(&conn, &m(10, "HOBx3"));
        insert(&conn, &m(11, "DXYZ"));
        insert(
            &conn,
            &Played {
                state: "abandoned",
                ..m(12, "DHOBHOBHOB")
            },
        );

        // A draft of HOB: its set only (the imported q-d is in it already).
        assert_eq!(available_tokens(&conn, "0x:29"), vec![tok(12), tok(1)]);
        // A sealed run of FRA.
        assert_eq!(available_tokens(&conn, "0x:30"), vec![tok(3), tok(2)]);
        // A limited run that does not know its set takes any limited match.
        conn.execute(
            "UPDATE leagues SET set_code = NULL WHERE run_key = '0x:29'",
            [],
        )
        .expect("no set code");
        assert_eq!(
            available_tokens(&conn, "0x:29"),
            vec![tok(12), tok(11), tok(3), tok(2), tok(1)]
        );
        // A constructed league without a deck: any match that is not limited,
        // one with no format included.
        assert_eq!(
            available_tokens(&conn, "manual:100"),
            vec![tok(10), tok(5), tok(4), "q-c".to_string()]
        );
    }

    /// More than `AVAILABLE_LIMIT` newer matches that do not fit never hide
    /// an older one that does: for a limited run that knows its set (whose
    /// limit is applied in Rust, after `fit_refusal`) and for a league with
    /// a deck.
    #[test]
    fn available_matches_look_past_a_page_of_newer_matches_that_do_not_fit() {
        let (_dir, conn) = limited_0x_db("leagues-available-past-limit");
        let newer = AVAILABLE_LIMIT + 5;
        let m = |n: u32, format: &'static str| Played {
            league: None,
            format,
            ..played(n, i64::from(n))
        };
        // The draft of HOB: the fitting match first, then newer ones whose
        // format holds HOB but is not a HOB draft.
        insert(&conn, &m(1, "DHOBHOBHOB"));
        for n in 0..newer {
            insert(&conn, &m(1000 + n, "DFRAHOBHOB"));
        }
        assert_eq!(available_tokens(&conn, "0x:29"), vec![tok(1)]);

        // A league with a deck: newer matches of another deck and limited
        // matches of its deck do not fit.
        local_deck(&conn, 1, 10);
        local_deck(&conn, 2, 20);
        manual(&conn, 100, Some(1), Some(1));
        insert(&conn, &m(2, "CMODERN"));
        link(&conn, &tok(2), 10);
        for n in 0..newer {
            insert(&conn, &m(2000 + n, "CMODERN"));
            link(&conn, &tok(2000 + n), 20);
            insert(&conn, &m(3000 + n, "DHOBHOBHOB"));
            link(&conn, &tok(3000 + n), 10);
        }
        assert_eq!(available_tokens(&conn, "manual:100"), vec![tok(2)]);
    }

    /// Deck `deck_id` of account 1 with version `version_id`.
    fn local_deck(conn: &Connection, deck_id: i64, version_id: i64) {
        deck(conn, deck_id, Some(1), false);
        version(conn, version_id, deck_id);
    }

    /// Plan L: a match entered by hand with a run's league token is a league
    /// match like a log one, placed by its start (it has no join line).
    #[test]
    fn a_manual_match_with_a_league_token_is_in_its_derived_run() {
        let (_dir, mut conn) = edit_db("leagues-manual-match");
        let by_hand = |n: u32, minutes: i64| Played {
            source: "manual",
            ..played(n, minutes)
        };
        insert(&conn, &played(1, 0));
        link(&conn, &tok(1), 10);
        insert(&conn, &by_hand(2, 10));
        link(&conn, &tok(2), 10);
        // A run whose first match was entered by hand.
        insert(
            &conn,
            &Played {
                league: Some(LEAGUE_2),
                ..by_hand(3, 20)
            },
        );
        link(&conn, &tok(3), 10);
        // In no league: a manual league can take it.
        insert(
            &conn,
            &Played {
                league: None,
                ..by_hand(4, 30)
            },
        );
        link(&conn, &tok(4), 10);
        sync_leagues(&conn);

        let inputs = fold_inputs(&conn, LeagueOwner::Account(1)).expect("inputs");
        let joined: Vec<String> = input_matches(&inputs)
            .iter()
            .map(|played| played.token.clone())
            .collect();
        assert_eq!(joined, vec![tok(1), tok(2), tok(3)]);
        let runs = fold_runs(inputs.inputs, chrono_tz::UTC);
        assert_eq!(runs[0].match_tokens, vec![tok(1), tok(2)]);

        assert_eq!(
            available_tokens(&conn, "manual:1"),
            vec![tok(4), tok(3), tok(2), tok(1)],
            "league matches can move too"
        );

        // The run its manual first match names is found for the edits.
        let by_hand_run = format!("{LEAGUE_2}:{}", tok(3));
        assert_eq!(
            edit(&mut conn, drop_edit(&by_hand_run, T0 + 25 * MINUTE)),
            LeagueEditOutcome::Done
        );
    }

    /// A league match the sync has not placed yet (it runs a second after
    /// the match) can be moved like any other; once linked, the sync leaves
    /// it where the player put it.
    #[test]
    fn a_league_match_not_synced_yet_can_be_moved() {
        let (_dir, mut conn) = edit_db("leagues-unsynced-league-match");
        insert(&conn, &played(1, 0));
        link(&conn, &tok(1), 10);
        let found = available_matches(&conn, "manual:1").expect("available");
        assert_eq!(found.len(), 1);
        assert_eq!(
            (found[0].row.token.clone(), found[0].now_in.clone()),
            (tok(1), None)
        );
        assert_eq!(
            edit(&mut conn, link_edit("manual:1", &[tok(1)])),
            LeagueEditOutcome::Done
        );
        sync_leagues(&conn);
        assert_eq!(members(&conn, 1), vec![tok(1)]);
    }

    // Tournaments

    #[test]
    fn tournament_matches_group_the_accounts_tournaments_with_a_deck_match_in_range() {
        let (_dir, conn) = db("leagues-tournaments");
        let local = account(&conn, "Qlocal");
        deck(&conn, 1, Some(local), false);
        deck(&conn, 2, Some(local), false);
        version(&conn, 10, 1);
        version(&conn, 11, 1);
        version(&conn, 20, 2);
        let round = |n: u32, minutes: i64, event: i64, round: i64| Played {
            league: None,
            tournament: Some((event, round)),
            ..played(n, minutes)
        };
        let first = insert(&conn, &round(1, 0, 500, 1));
        game(&conn, first, 1, "win", true);
        link(&conn, &tok(1), 10);
        insert(&conn, &round(2, 10, 500, 2));
        insert(&conn, &round(3, 20, 500, 3));
        link(&conn, &tok(3), 11);
        insert(
            &conn,
            &Played {
                state: "pending",
                ..round(4, 25, 500, 4)
            },
        );
        insert(&conn, &round(5, 30, 600, 1));
        link(&conn, &tok(5), 20);
        insert(&conn, &round(6, 90, 700, 1));
        link(&conn, &tok(6), 10);
        insert(
            &conn,
            &Played {
                account: "Qother",
                ..round(7, 5, 500, 1)
            },
        );

        sync_leagues(&conn);

        let minute = |m: i64| T0 + m * MINUTE;
        let groups = stored_tournaments(&conn, local, Some(1), Some(minute(0)), Some(minute(60)))
            .expect("read");
        assert_eq!(groups.len(), 1);
        assert_eq!(
            (
                groups[0].event_id,
                groups[0].started_at,
                groups[0].format.as_deref()
            ),
            (500, minute(0), Some("CMODERN"))
        );
        assert_eq!(groups[0].name, "Tournament 500 (28-05-2026)");
        assert_eq!(tokens(&groups[0].matches), vec![tok(1), tok(3)]);
        let row = &groups[0].matches[0];
        assert_eq!(
            (row.tournament_event_id, row.tournament_round, row.deck_id),
            (Some(500), Some(1), Some(1))
        );
        let tournament_match = row.tournament_match();
        assert_eq!(tournament_match.round, Some(1));
        assert_eq!(tournament_match.games.len(), 1);

        // One deck match in range is enough; the tournament keeps all of them.
        let later = stored_tournaments(&conn, local, Some(1), Some(minute(15)), Some(minute(60)))
            .expect("read");
        assert_eq!(later.len(), 1);
        assert_eq!(later[0].started_at, minute(0), "the account's first match");
        assert_eq!(tokens(&later[0].matches), vec![tok(1), tok(3)]);

        let all = stored_tournaments(&conn, local, None, None, None).expect("all");
        assert_eq!(
            all.iter().map(|group| group.event_id).collect::<Vec<_>>(),
            vec![700, 600, 500],
            "newest first"
        );
        assert_eq!(tokens(&all[2].matches), vec![tok(1), tok(2), tok(3)]);
    }

    // Query plans

    fn plan(conn: &Connection, sql: &str) -> String {
        let mut stmt = conn
            .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
            .expect("explain");
        let count = stmt.parameter_count();
        let values: Vec<i64> = vec![1; count];
        let details: Vec<String> = stmt
            .query_map(rusqlite::params_from_iter(values), |row| {
                row.get::<_, String>(3)
            })
            .expect("rows")
            .map(|row| row.expect("row"))
            .collect();
        details.join("\n")
    }

    #[test]
    fn league_reads_walk_their_indexes() {
        let (_dir, conn) = db("leagues-plans");
        for (name, sql, index) in [
            (
                "league matches",
                rows_sql(&plain_scope()),
                "matches_account_league",
            ),
            (
                "league games",
                games_sql(&plain_scope()),
                "matches_account_league",
            ),
            (
                "league events",
                LEAGUE_EVENTS.to_string(),
                "log_events_leagues",
            ),
            ("logins", ALL_LOGINS.to_string(), "log_events_logins"),
            (
                "imported runs",
                IMPORTED_SEEDS.to_string(),
                "leagues_account_kind",
            ),
            (
                "imported matches",
                rows_sql(&members_scope()),
                "matches_league_id",
            ),
            (
                "imported games",
                games_sql(&members_scope()),
                "matches_league_id",
            ),
            (
                "stored runs",
                stored_runs_plan_sql(),
                "leagues_account_kind",
            ),
            (
                "stored run matches",
                rows_sql(&stored_runs_scope()),
                "matches_league_id",
            ),
            (
                "stored run games",
                games_sql(&stored_runs_scope()),
                "matches_league_id",
            ),
            (
                "tournaments",
                TOURNAMENTS_SQL.to_string(),
                "matches_account_tournament",
            ),
            (
                "tournament matches",
                rows_sql(&tournament_scope()),
                "matches_account_tournament",
            ),
            (
                "tournament games",
                games_sql(&tournament_scope()),
                "matches_account_tournament",
            ),
            (
                "available matches",
                rows_sql(&available_by_deck_scope()),
                "matches_deck_version_id",
            ),
            (
                "available games",
                games_sql(&available_by_deck_scope()),
                "matches_deck_version_id",
            ),
        ] {
            let found = plan(&conn, &sql);
            assert!(found.contains(index), "{name}: {found}");
            for line in found.lines() {
                let full_scan = line.starts_with("SCAN ") && !line.contains("USING");
                assert!(!full_scan, "{name} scans a whole table: {found}");
            }
        }
    }

    /// The page's stored runs filter: two accounts and the account-less runs.
    fn page_filter() -> RunFilter {
        RunFilter {
            owners: vec![
                LeagueOwner::Account(1),
                LeagueOwner::Account(2),
                LeagueOwner::Nobody,
            ],
            kinds: vec![LeagueKind::Constructed],
            deck_id: None,
            run_key: None,
        }
    }

    fn stored_runs_plan_sql() -> String {
        let (filter, _) = run_filter_sql(&page_filter());
        format!("SELECT l.id FROM listed_leagues l WHERE {filter}")
    }

    fn stored_runs_scope() -> Scope {
        Scope {
            from: "listed_leagues l JOIN listed_matches m ON m.league_id = l.id",
            filter: run_filter_sql(&page_filter()).0,
            order: "ORDER BY m.started_at, m.id",
            limit: "",
            group: "l.id",
        }
    }

    // Edits

    /// Qlocal (1) has decks 1 (versions 10, 11) and 2 (version 20) and an
    /// archived deck 4 (version 40); Qother (2) has deck 3 (version 30).
    /// Manual league 1 is of deck 1.
    fn edit_db(label: &str) -> (TestDir, Connection) {
        let (dir, conn) = db(label);
        let local = account(&conn, "Qlocal");
        let other = account(&conn, "Qother");
        assert_eq!((local, other), (1, 2));
        deck(&conn, 1, Some(local), false);
        deck(&conn, 2, Some(local), false);
        deck(&conn, 3, Some(other), false);
        deck(&conn, 4, Some(local), true);
        for (version_id, deck_id) in [(10, 1), (11, 1), (20, 2), (30, 3), (40, 4)] {
            version(&conn, version_id, deck_id);
        }
        manual(&conn, 1, Some(local), Some(1));
        (dir, conn)
    }

    fn edit(conn: &mut Connection, edit: LeagueEdit) -> LeagueEditOutcome {
        edit_league(conn, &edit, chrono_tz::UTC).expect("edit")
    }

    fn create(name: &str, deck_id: i64) -> LeagueEdit {
        LeagueEdit::CreateManual {
            account_id: 1,
            deck_id,
            name: name.to_string(),
            token: "manual_QqQqQqQqQqQqQqQqQqQqQqQq".to_string(),
            started_at: 5_000,
            now_ms: 9_000,
        }
    }

    #[test]
    fn a_manual_league_token_is_manual_and_24_alphanumerics() {
        let bytes: Vec<u8> = (0u8..=255).collect();
        let token = manual_league_token(&bytes);
        assert_eq!(token, "manual_ABCDEFGHIJKLMNOPQRSTUVWX");
        // Too few usable bytes still make a whole token.
        let short = manual_league_token(&[255, 1]);
        assert!(short.starts_with("manual_"));
        assert_eq!(short.len(), "manual_".len() + 24);
        assert!(
            short["manual_".len()..]
                .chars()
                .all(|c| c.is_ascii_alphanumeric())
        );
    }

    #[test]
    fn a_new_manual_league_gets_a_manual_token() {
        let (_dir, mut conn) = edit_db("leagues-edit-create-token");
        assert_eq!(
            edit(&mut conn, create("Q token league", 1)),
            LeagueEditOutcome::Done
        );
        let token: String = conn
            .query_row(
                "SELECT token FROM leagues WHERE source = 'manual' AND id <> 1",
                [],
                |row| row.get(0),
            )
            .expect("token");
        assert_eq!(token, "manual_QqQqQqQqQqQqQqQqQqQqQqQq");
    }

    #[test]
    fn the_manual_league_check_refuses_a_row_without_a_manual_token() {
        let (_dir, conn) = db("leagues-manual-token-check");
        let insert = |token: Option<&str>| {
            conn.execute(
                "INSERT INTO leagues (run_key, source, kind, state, token, name, started_at,
                     created_at)
                 VALUES ('manual:9', 'manual', 'constructed', 'complete', ?1, 'Qmanual', 1, 1)",
                [token],
            )
        };
        assert!(insert(None).is_err());
        assert!(insert(Some("q-log-token")).is_err());
        assert!(insert(Some("manual_QqQqQqQqQqQqQqQqQqQqQqQq")).is_ok());
    }

    #[test]
    fn creating_a_manual_league_needs_a_live_deck_of_the_account_and_a_name() {
        let (_dir, mut conn) = edit_db("leagues-edit-create");
        assert_eq!(
            edit(&mut conn, create("  Q Friday league  ", 1)),
            LeagueEditOutcome::Done
        );
        let row: (i64, i64, String, i64, i64) = conn
            .query_row(
                "SELECT account_id, deck_id, name, started_at, created_at FROM leagues
                 WHERE id <> 1",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .expect("created");
        assert_eq!(row, (1, 1, "Q Friday league".to_string(), 5_000, 9_000));
        let stored: (String, String, String, String, Option<String>, i64) = conn
            .query_row(
                "SELECT run_key, source, kind, state, format, deck_version_id FROM leagues
                 WHERE id <> 1",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                    ))
                },
            )
            .expect("its row");
        assert_eq!(
            stored,
            (
                "manual:2".to_string(),
                "manual".to_string(),
                "constructed".to_string(),
                "complete".to_string(),
                Some("CMODERN".to_string()),
                11
            ),
            "the next id, the deck's format and newest version"
        );

        for (deck_id, why) in [(3, "another account's"), (4, "archived"), (99, "missing")] {
            assert_eq!(
                edit(&mut conn, create("Qname", deck_id)),
                LeagueEditOutcome::InvalidDeck,
                "{why}"
            );
        }
        assert_eq!(
            edit(&mut conn, create("   ", 1)),
            LeagueEditOutcome::NameRequired
        );
        let long: String = "é".repeat(MAX_LEAGUE_NAME_CHARS + 1);
        assert_eq!(
            edit(&mut conn, create(&long, 1)),
            LeagueEditOutcome::NameTooLong
        );
        assert_eq!(
            count(
                &conn,
                "SELECT COUNT(*) FROM leagues WHERE source = 'manual'"
            ),
            2
        );
        let longest: String = "é".repeat(MAX_LEAGUE_NAME_CHARS);
        assert_eq!(
            edit(&mut conn, create(&longest, 2)),
            LeagueEditOutcome::Done
        );
    }

    fn link_edit(run_key: &str, matches: &[String]) -> LeagueEdit {
        LeagueEdit::LinkMatches {
            run_key: run_key.to_string(),
            match_tokens: matches.to_vec(),
            now_ms: 9_000,
        }
    }

    fn members(conn: &Connection, id: i64) -> Vec<String> {
        let mut stmt = conn
            .prepare(
                "SELECT token FROM matches WHERE league_id = ?1 AND league_linked = 1
                 ORDER BY token",
            )
            .expect("prepare");
        stmt.query_map([id], |row| row.get::<_, String>(0))
            .expect("query")
            .map(|row| row.expect("row"))
            .collect()
    }

    #[test]
    fn moving_matches_into_a_league_follows_the_spec_rules() {
        let (_dir, mut conn) = edit_db("leagues-edit-link");
        manual(&conn, 2, Some(1), Some(1));
        manual(&conn, 3, Some(1), None);
        manual(&conn, 4, Some(1), Some(1));
        conn.execute("UPDATE leagues SET removed_at = 1 WHERE id = 4", [])
            .expect("a removed league");
        imported_run(&conn, 42, 1, "complete");
        let unleagued = |n: u32| Played {
            league: None,
            ..played(n, i64::from(n))
        };
        // 1, 2, 8 to 11: fine (two versions of deck 1); 3: deck 2; 4: in
        // progress; 5: in imported run 42; 6: in a log run; 7: in manual
        // league 2; 12: a tournament round; 13: Qother's; 14: a draft match;
        // 15: abandoned; 16: hidden.
        for n in 1..=16 {
            let row = match n {
                4 => Played {
                    state: "in_progress",
                    ..unleagued(n)
                },
                6 => played(n, i64::from(n)),
                12 => Played {
                    tournament: Some((500, 1)),
                    ..unleagued(n)
                },
                13 => Played {
                    account: "Qother",
                    ..unleagued(n)
                },
                14 => Played {
                    format: "DHOBHOBHOB",
                    ..unleagued(n)
                },
                15 => Played {
                    state: "abandoned",
                    ..unleagued(n)
                },
                _ => unleagued(n),
            };
            insert(&conn, &row);
            let version_id = match n {
                2 => 11,
                3 => 20,
                13 => 30,
                _ => 10,
            };
            link(&conn, &tok(n), version_id);
        }
        imported_member(&conn, 42, &tok(5));
        manual_member(&conn, 2, &tok(7));
        hide(&conn, &tok(16));
        sync_leagues(&conn);

        let refusals = [
            (link_edit("manual:1", &[]), LeagueEditOutcome::NoMatches),
            (
                link_edit("manual:99", &[tok(1)]),
                LeagueEditOutcome::NotFound,
            ),
            (
                link_edit("manual:4", &[tok(1)]),
                LeagueEditOutcome::LeagueRemoved,
            ),
            (
                link_edit("manual:1", &[tok(1), tok(99)]),
                LeagueEditOutcome::MatchNotFound,
            ),
            (
                link_edit("manual:1", &[tok(1), tok(16)]),
                LeagueEditOutcome::MatchNotFound,
            ),
            (
                link_edit("manual:1", &[tok(1), tok(3)]),
                LeagueEditOutcome::WrongDeck,
            ),
            (
                link_edit("manual:1", &[tok(4)]),
                LeagueEditOutcome::NotFinished,
            ),
            (
                link_edit("manual:1", &[tok(12)]),
                LeagueEditOutcome::Tournament,
            ),
            (
                link_edit("manual:1", &[tok(13)]),
                LeagueEditOutcome::WrongAccount,
            ),
            (
                link_edit("manual:1", &[tok(14)]),
                LeagueEditOutcome::WrongKind,
            ),
            (
                link_edit(
                    "manual:1",
                    &[tok(1), tok(2), tok(8), tok(9), tok(10), tok(11)],
                ),
                LeagueEditOutcome::TooManyMatches,
            ),
        ];
        for (request, expected) in refusals {
            assert_eq!(edit(&mut conn, request.clone()), expected, "{request:?}");
            assert!(members(&conn, 1).is_empty(), "refused: {request:?}");
            assert!(expected.refusal().is_some());
            assert_eq!(
                count(
                    &conn,
                    "SELECT COUNT(*) FROM matches WHERE edited_at IS NOT NULL"
                ),
                0,
                "refused: {request:?}"
            );
        }

        // A league without a deck takes a match of any deck.
        assert_eq!(
            edit(&mut conn, link_edit("manual:3", &[tok(3)])),
            LeagueEditOutcome::Done
        );
        assert_eq!(members(&conn, 3), vec![tok(3)]);

        // Matches in other leagues move: from a 0.x run, a log run and a
        // manual league.
        assert_eq!(
            edit(&mut conn, link_edit("manual:1", &[tok(5), tok(6), tok(7)])),
            LeagueEditOutcome::Done
        );
        assert_eq!(members(&conn, 1), vec![tok(5), tok(6), tok(7)]);
        assert!(members(&conn, 1042).is_empty());
        assert!(members(&conn, 2).is_empty());
        assert_eq!(
            match_columns(&conn, &tok(6)),
            MatchColumns {
                league_id: Some(1),
                league_linked: true,
                no_league: false,
                hidden_at: None,
                edited_at: Some(9_000),
            }
        );

        // An abandoned match moves too; one already in the league counts once.
        assert_eq!(
            edit(&mut conn, link_edit("manual:1", &[tok(15), tok(5)])),
            LeagueEditOutcome::Done
        );
        assert_eq!(
            edit(&mut conn, link_edit("manual:1", &[tok(1)])),
            LeagueEditOutcome::Done
        );
        assert_eq!(members(&conn, 1).len(), MANUAL_MAX_MATCHES);
        assert_eq!(
            edit(&mut conn, link_edit("manual:1", &[tok(2)])),
            LeagueEditOutcome::TooManyMatches
        );

        // Log and 0.x runs have no cap, and a log run keeps a linked member
        // through the sync.
        insert(
            &conn,
            &Played {
                league: Some(LEAGUE_2),
                ..played(20, 200)
            },
        );
        link(&conn, &tok(20), 10);
        sync_leagues(&conn);
        let log_run = format!("{LEAGUE_2}:{}", tok(20));
        assert_eq!(
            edit(&mut conn, link_edit(&log_run, &[tok(8)])),
            LeagueEditOutcome::Done
        );
        assert_eq!(
            tokens(&stored(&conn, &log_run).matches),
            vec![tok(8), tok(20)]
        );
        sync_leagues(&conn);
        assert_eq!(
            tokens(&stored(&conn, &log_run).matches),
            vec![tok(8), tok(20)]
        );
        assert_eq!(
            edit(
                &mut conn,
                link_edit("0x:42", &[tok(9), tok(10), tok(11), tok(2)])
            ),
            LeagueEditOutcome::Done
        );
        assert_eq!(members(&conn, 1042), vec![tok(2), tok(9), tok(10), tok(11)]);
    }

    fn notes_edit(run_key: &str, notes: &str) -> LeagueEdit {
        LeagueEdit::SetNotes {
            run_key: run_key.to_string(),
            notes: notes.to_string(),
            now_ms: 9_000,
        }
    }

    fn notes_edited_at(conn: &Connection, run_key: &str) -> Option<i64> {
        conn.query_row(
            "SELECT notes_edited_at FROM leagues WHERE run_key = ?1",
            [run_key],
            |row| row.get(0),
        )
        .expect("notes_edited_at")
    }

    fn note(conn: &Connection, run_key: &str) -> Option<String> {
        use rusqlite::OptionalExtension;
        conn.query_row(
            "SELECT notes FROM leagues WHERE run_key = ?1",
            [run_key],
            |row| row.get(0),
        )
        .optional()
        .expect("note")
        .flatten()
    }

    // Hidden matches (Plan K, "Remove from stats")

    #[test]
    fn hidden_matches_are_left_out_of_every_league_read() {
        let (_dir, conn) = db("leagues-hidden");
        let local = account(&conn, "Qlocal");
        deck(&conn, 1, Some(local), false);
        version(&conn, 10, 1);
        let unleagued = |n: u32, minutes: i64| Played {
            league: None,
            ..played(n, minutes)
        };
        let round = |n: u32, minutes: i64, number: i64| Played {
            league: None,
            tournament: Some((500, number)),
            ..played(n, minutes)
        };
        let imported = |n: u32, minutes: i64| Played {
            source: "imported",
            league: None,
            ..played(n, minutes)
        };
        // 1, 2: a derived run; 3, 4: manual league 1; 5, 6: imported run 42;
        // 7, 8: tournament rounds; 9, 10: available to manual league 2. The
        // second of each pair is hidden.
        let rows = [
            played(1, 0),
            played(2, 10),
            unleagued(3, 20),
            unleagued(4, 30),
            imported(5, 40),
            imported(6, 50),
            round(7, 60, 1),
            round(8, 70, 2),
            unleagued(9, 80),
            unleagued(10, 90),
        ];
        for (index, row) in rows.iter().enumerate() {
            insert(&conn, row);
            link(&conn, &tok(index as u32 + 1), 10);
        }
        manual(&conn, 1, Some(local), Some(1));
        manual(&conn, 2, Some(local), Some(1));
        manual_member(&conn, 1, &tok(3));
        manual_member(&conn, 1, &tok(4));
        imported_run(&conn, 42, local, "complete");
        imported_member(&conn, 42, &tok(5));
        imported_member(&conn, 42, &tok(6));
        for n in [2, 4, 6, 8, 10] {
            hide(&conn, &tok(n));
        }
        sync_leagues(&conn);

        let inputs = fold_inputs(&conn, LeagueOwner::Account(local)).expect("inputs");
        assert_eq!(tokens(&inputs.matches), vec![tok(1), tok(5)]);
        let played: Vec<String> = input_matches(&inputs)
            .iter()
            .map(|m| m.token.clone())
            .collect();
        assert_eq!(played, vec![tok(1)]);
        let seeded = seeds(&inputs);
        let members: Vec<String> = seeded[0]
            .3
            .matches
            .iter()
            .map(|m| m.token.clone())
            .collect();
        assert_eq!(members, vec![tok(5)], "the imported run's members");

        let leagues = runs_of(&conn, LeagueOwner::Account(local), "manual");
        let first = leagues.iter().find(|l| l.id == 1).expect("league 1");
        assert_eq!(tokens(&first.matches), vec![tok(3)]);
        let runs = runs_of(&conn, LeagueOwner::Account(local), "imported");
        assert_eq!(tokens(&runs[0].matches), vec![tok(5)]);
        let derived = runs_of(&conn, LeagueOwner::Account(local), "log");
        assert_eq!(tokens(&derived[0].matches), vec![tok(1)]);
        let groups = stored_tournaments(&conn, local, None, None, None).expect("tournaments");
        assert_eq!(groups.len(), 1);
        assert_eq!(tokens(&groups[0].matches), vec![tok(7)]);
        // Not the tournament round 7; the league matches 1, 3, 5 can move.
        assert_eq!(
            available_tokens(&conn, "manual:2"),
            vec![tok(9), tok(5), tok(3), tok(1)]
        );
    }

    #[test]
    fn a_hidden_match_cannot_be_linked_and_does_not_fill_a_manual_league() {
        let (_dir, mut conn) = edit_db("leagues-edit-hidden");
        for n in 1..=7 {
            insert(
                &conn,
                &Played {
                    league: None,
                    ..played(n, i64::from(n))
                },
            );
            link(&conn, &tok(n), 10);
        }
        hide(&conn, &tok(6));
        assert_eq!(
            edit(&mut conn, link_edit("manual:1", &[tok(6)])),
            LeagueEditOutcome::MatchNotFound
        );
        // Five members, one of them hidden: there is room for one more.
        for n in 1..=5 {
            manual_member(&conn, 1, &tok(n));
        }
        hide(&conn, &tok(5));
        assert_eq!(
            edit(&mut conn, link_edit("manual:1", &[tok(7)])),
            LeagueEditOutcome::Done
        );
    }

    #[test]
    fn notes_are_trimmed_limited_and_cleared_when_empty() {
        let (_dir, mut conn) = edit_db("leagues-edit-notes");
        imported_run(&conn, 42, 1, "complete");
        insert(&conn, &played(1, 0));
        sync_leagues(&conn);
        let derived = format!("{LEAGUE}:{}", tok(1));

        assert_eq!(
            edit(&mut conn, notes_edit("manual:1", "  Q good run  ")),
            LeagueEditOutcome::Done
        );
        assert_eq!(note(&conn, "manual:1").as_deref(), Some("Q good run"));
        assert_eq!(
            notes_edited_at(&conn, "manual:1"),
            Some(9_000),
            "a set stamps the edit time"
        );
        let too_long = "é".repeat(MAX_NOTES_CHARS + 1);
        assert_eq!(
            edit(&mut conn, notes_edit("manual:1", &too_long)),
            LeagueEditOutcome::NotesTooLong
        );
        assert_eq!(note(&conn, "manual:1").as_deref(), Some("Q good run"));
        let longest = "é".repeat(MAX_NOTES_CHARS);
        assert_eq!(
            edit(&mut conn, notes_edit("manual:1", &longest)),
            LeagueEditOutcome::Done
        );
        assert_eq!(
            edit(&mut conn, notes_edit("manual:1", "   ")),
            LeagueEditOutcome::Done
        );
        assert_eq!(note(&conn, "manual:1"), None, "an empty note clears it");
        assert_eq!(
            notes_edited_at(&conn, "manual:1"),
            Some(9_000),
            "a clear stamps the edit time too"
        );

        for run_key in [derived.as_str(), "0x:42"] {
            assert_eq!(
                edit(&mut conn, notes_edit(run_key, "Q note")),
                LeagueEditOutcome::Done,
                "{run_key}"
            );
            assert_eq!(note(&conn, run_key).as_deref(), Some("Q note"));
        }
        for run_key in [
            "manual:99",
            "0x:99",
            &format!("{LEAGUE}:{}", tok(99)),
            &format!("{LEAGUE_2}:{}", tok(1)),
            "nonsense",
        ] {
            assert_eq!(
                edit(&mut conn, notes_edit(run_key, "Q note")),
                LeagueEditOutcome::NotFound,
                "{run_key}"
            );
        }
    }

    fn drop_edit(run_key: &str, now_ms: i64) -> LeagueEdit {
        LeagueEdit::SetStatus {
            run_key: run_key.to_string(),
            status: Some(UserState::Dropped),
            now_ms,
        }
    }

    #[test]
    fn a_dropped_status_applies_to_any_run_complete_ones_too() {
        let (_dir, mut conn) = edit_db("leagues-edit-drop");
        imported_run(&conn, 42, 1, "complete");
        imported_run(&conn, 43, 1, "active");
        // An Active derived run on LEAGUE.
        insert(&conn, &played(1, 0));
        // A Complete derived run on LEAGUE_2: five complete matches.
        for n in 2..=6 {
            insert(
                &conn,
                &Played {
                    league: Some(LEAGUE_2),
                    ..played(n, i64::from(n) * 10)
                },
            );
        }
        // A Partial run: a third token whose second match is of another
        // version starts a new run.
        const LEAGUE_3: &str = "2a3b4c5d-6e7f-4a8b-9c0d-1e2f3a4b5c6d";
        for (n, version_id) in [(7, 10), (8, 11)] {
            insert(
                &conn,
                &Played {
                    league: Some(LEAGUE_3),
                    state: "in_progress",
                    ended_at: None,
                    ..played(n, 100 + i64::from(n))
                },
            );
            link(&conn, &tok(n), version_id);
        }
        sync_leagues(&conn);

        let active = format!("{LEAGUE}:{}", tok(1));
        assert_eq!(
            edit(&mut conn, drop_edit(&active, 7_000)),
            LeagueEditOutcome::Done
        );
        assert_eq!(
            edit(&mut conn, drop_edit(&active, 8_000)),
            LeagueEditOutcome::Done,
            "dropping again keeps the first time"
        );
        let partial = format!("{LEAGUE_3}:{}", tok(7));
        assert_eq!(
            edit(&mut conn, drop_edit(&partial, 7_500)),
            LeagueEditOutcome::Done
        );
        assert_eq!(
            edit(&mut conn, drop_edit("0x:43", 7_600)),
            LeagueEditOutcome::Done
        );
        let drops = manual_drops(&conn).expect("drops");
        assert_eq!(drops.get(&active), Some(&7_000));
        assert_eq!(drops.get(&partial), Some(&7_500));
        assert_eq!(drops.get("0x:43"), Some(&7_600));

        for run_key in ["manual:1", "0x:42", &format!("{LEAGUE_2}:{}", tok(2))] {
            assert_eq!(
                edit(&mut conn, drop_edit(run_key, 7_000)),
                LeagueEditOutcome::Done,
                "{run_key}"
            );
        }
        for run_key in ["manual:99", "0x:99", &format!("{LEAGUE}:{}", tok(99))] {
            assert_eq!(
                edit(&mut conn, drop_edit(run_key, 7_000)),
                LeagueEditOutcome::NotFound,
                "{run_key}"
            );
        }
        assert_eq!(drops.len(), 3);
        assert_eq!(
            manual_drops(&conn).expect("drops").len(),
            6,
            "complete runs take a drop too"
        );
    }

    /// The stored run `run_key` (attached).
    fn stored(conn: &Connection, run_key: &str) -> StoredRun {
        let filter = RunFilter {
            owners: vec![LeagueOwner::Account(1), LeagueOwner::Nobody],
            kinds: vec![LeagueKind::Constructed],
            deck_id: None,
            run_key: Some(run_key.to_string()),
        };
        let mut runs = stored_runs(conn, &filter).expect("stored runs");
        assert_eq!(runs.len(), 1, "{run_key}");
        runs.remove(0)
    }

    /// The player's columns of a match (spec 3.1).
    #[derive(Debug, PartialEq, Eq)]
    struct MatchColumns {
        league_id: Option<i64>,
        league_linked: bool,
        no_league: bool,
        hidden_at: Option<i64>,
        edited_at: Option<i64>,
    }

    fn match_columns(conn: &Connection, token: &str) -> MatchColumns {
        conn.query_row(
            "SELECT league_id, league_linked, no_league, hidden_at, edited_at
             FROM matches WHERE token = ?1",
            [token],
            |row| {
                Ok(MatchColumns {
                    league_id: row.get(0)?,
                    league_linked: row.get(1)?,
                    no_league: row.get(2)?,
                    hidden_at: row.get(3)?,
                    edited_at: row.get(4)?,
                })
            },
        )
        .expect("match columns")
    }

    /// The player's columns of a league row (spec 3.2), and whether it is
    /// detached.
    #[derive(Debug, PartialEq, Eq)]
    struct LeagueColumns {
        removed_at: Option<i64>,
        user_state: Option<String>,
        user_dropped_at: Option<i64>,
        edited_at: Option<i64>,
        detached: bool,
    }

    fn league_columns(conn: &Connection, run_key: &str) -> LeagueColumns {
        conn.query_row(
            "SELECT removed_at, user_state, user_dropped_at, edited_at, detached_at IS NOT NULL
             FROM leagues WHERE run_key = ?1",
            [run_key],
            |row| {
                Ok(LeagueColumns {
                    removed_at: row.get(0)?,
                    user_state: row.get(1)?,
                    user_dropped_at: row.get(2)?,
                    edited_at: row.get(3)?,
                    detached: row.get(4)?,
                })
            },
        )
        .expect("league columns")
    }

    fn row_id(conn: &Connection, run_key: &str) -> i64 {
        count(
            conn,
            &format!("SELECT id FROM leagues WHERE run_key = '{run_key}'"),
        )
    }

    fn status_edit(run_key: &str, status: Option<UserState>, now_ms: i64) -> LeagueEdit {
        LeagueEdit::SetStatus {
            run_key: run_key.to_string(),
            status,
            now_ms,
        }
    }

    fn delete_edit(run_key: &str, matches: DeleteMatches, now_ms: i64) -> LeagueEdit {
        LeagueEdit::Delete {
            run_key: run_key.to_string(),
            matches,
            now_ms,
        }
    }

    fn undo_edit(conn: &Connection, run_key: &str, removed_at: i64, now_ms: i64) -> LeagueEdit {
        LeagueEdit::UndoDelete {
            league_id: row_id(conn, run_key),
            removed_at,
            now_ms,
        }
    }

    fn restore_edit(run_key: &str, now_ms: i64) -> LeagueEdit {
        LeagueEdit::Restore {
            run_key: run_key.to_string(),
            now_ms,
        }
    }

    fn move_edit(token: &str, to_run_key: Option<&str>, now_ms: i64) -> LeagueEdit {
        LeagueEdit::MoveMatch {
            match_token: token.to_string(),
            to_run_key: to_run_key.map(str::to_string),
            now_ms,
        }
    }

    fn available_tokens(conn: &Connection, run_key: &str) -> Vec<String> {
        available_matches(conn, run_key)
            .expect("available")
            .into_iter()
            .map(|found| found.row.token)
            .collect()
    }

    #[test]
    fn a_link_survives_a_rebuild_and_a_sync() {
        let (_dir, mut conn) = edit_db("leagues-link-survives");
        let derive = |conn: &mut Connection, revision: u64| {
            commit(
                conn,
                &WriteBatch {
                    matches: vec![crate::write::MatchWrite::Upsert {
                        view: Box::new(view(TOKEN, "Qopp", T0)),
                        revision,
                    }],
                    now_ms: T0,
                    ..WriteBatch::default()
                },
            )
            .expect("derived");
        };
        derive(&mut conn, 1);
        link(&conn, TOKEN, 10);
        assert_eq!(
            edit(&mut conn, link_edit("manual:1", &[TOKEN.to_string()])),
            LeagueEditOutcome::Done
        );
        assert_eq!(tokens(&stored(&conn, "manual:1").matches), vec![TOKEN]);

        // The projection derives the match again, then the league sync runs.
        derive(&mut conn, 2);
        sync_leagues(&conn);
        let linked: (i64, bool) = conn
            .query_row(
                "SELECT league_id, league_linked FROM matches WHERE token = ?1",
                [TOKEN],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("the match");
        assert_eq!(linked, (1, true));
        assert_eq!(tokens(&stored(&conn, "manual:1").matches), vec![TOKEN]);
    }

    #[test]
    fn a_drop_shows_at_once_after_the_edit() {
        let (_dir, mut conn) = edit_db("leagues-drop-at-once");
        insert(&conn, &played(1, 0));
        sync_leagues(&conn);
        let key = format!("{LEAGUE}:{}", tok(1));
        assert_eq!(stored(&conn, &key).state, RunState::Active);

        assert_eq!(
            edit(&mut conn, drop_edit(&key, T0 + 5 * MINUTE)),
            LeagueEditOutcome::Done
        );
        // No sync since: the edit synced its owner before it committed.
        let run = stored(&conn, &key);
        assert_eq!(
            (run.state, run.dropped_at),
            (RunState::Dropped, Some(T0 + 5 * MINUTE))
        );
        let user_dropped_at = count(
            &conn,
            &format!("SELECT user_dropped_at FROM leagues WHERE run_key = '{key}'"),
        );
        assert_eq!(user_dropped_at, T0 + 5 * MINUTE);
        let status: (String, i64) = conn
            .query_row(
                "SELECT user_state, edited_at FROM leagues WHERE run_key = ?1",
                [&key],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("the player's status");
        assert_eq!(status, ("dropped".to_string(), T0 + 5 * MINUTE));
    }

    #[test]
    fn user_states_are_stored_as_text() {
        for state in [
            UserState::Active,
            UserState::Complete,
            UserState::Partial,
            UserState::Dropped,
        ] {
            assert_eq!(UserState::parse(state.as_str()), Some(state));
        }
        assert_eq!(UserState::Dropped.as_str(), "dropped");
        assert_eq!(UserState::parse("automatic"), None);
        assert_eq!(UserState::Partial.run_state(), RunState::Partial);
    }

    #[test]
    fn a_status_shows_at_read_time_and_only_dropped_reaches_the_fold() {
        let (_dir, mut conn) = edit_db("leagues-edit-status");
        insert(&conn, &played(1, 0));
        sync_leagues(&conn);
        let key = format!("{LEAGUE}:{}", tok(1));

        assert_eq!(
            edit(
                &mut conn,
                status_edit(&key, Some(UserState::Complete), T0 + 2 * MINUTE)
            ),
            LeagueEditOutcome::Done
        );
        let run = stored(&conn, &key);
        assert_eq!(
            (run.state, run.user_state, run.shown_state()),
            (
                RunState::Active,
                Some(UserState::Complete),
                RunState::Complete
            ),
            "the fold's state stays"
        );
        assert_eq!(league_columns(&conn, &key).edited_at, Some(T0 + 2 * MINUTE));

        assert_eq!(
            edit(
                &mut conn,
                status_edit(&key, Some(UserState::Dropped), T0 + 5 * MINUTE)
            ),
            LeagueEditOutcome::Done
        );
        let run = stored(&conn, &key);
        assert_eq!(
            (run.state, run.dropped_at),
            (RunState::Dropped, Some(T0 + 5 * MINUTE))
        );
        assert_eq!(
            edit(
                &mut conn,
                status_edit(&key, Some(UserState::Dropped), T0 + 9 * MINUTE)
            ),
            LeagueEditOutcome::Done
        );
        assert_eq!(
            league_columns(&conn, &key).user_dropped_at,
            Some(T0 + 5 * MINUTE),
            "the first drop time stays"
        );

        assert_eq!(
            edit(&mut conn, status_edit(&key, None, T0 + 10 * MINUTE)),
            LeagueEditOutcome::Done
        );
        let columns = league_columns(&conn, &key);
        assert_eq!(
            (
                columns.user_state,
                columns.user_dropped_at,
                columns.edited_at
            ),
            (None, Some(T0 + 5 * MINUTE), Some(T0 + 10 * MINUTE))
        );
        assert_eq!(
            stored(&conn, &key).state,
            RunState::Active,
            "Automatic: the fold ignores the old drop time"
        );

        // Any status on a manual league; its stored state stays complete.
        assert_eq!(
            edit(
                &mut conn,
                status_edit("manual:1", Some(UserState::Dropped), 9_000)
            ),
            LeagueEditOutcome::Done
        );
        let manual = stored(&conn, "manual:1");
        assert_eq!(
            (manual.state, manual.shown_state()),
            (RunState::Complete, RunState::Dropped)
        );
        assert_eq!(
            edit(&mut conn, status_edit("manual:99", None, 9_000)),
            LeagueEditOutcome::NotFound
        );
    }

    #[test]
    fn deleting_needs_a_league_that_is_not_active_and_drops_a_run_marked_done() {
        let (_dir, mut conn) = edit_db("leagues-edit-delete-active");
        insert(&conn, &played(1, 0));
        sync_leagues(&conn);
        let key = format!("{LEAGUE}:{}", tok(1));
        let keep = DeleteMatches::KeepAsNonLeague;

        assert_eq!(
            edit(&mut conn, delete_edit(&key, keep, T0 + 2 * MINUTE)),
            LeagueEditOutcome::StillActive
        );
        assert_eq!(
            LeagueEditOutcome::StillActive.refusal(),
            Some("Drop it first")
        );
        assert_eq!(league_columns(&conn, &key).removed_at, None);

        assert_eq!(
            edit(
                &mut conn,
                status_edit(&key, Some(UserState::Complete), T0 + 3 * MINUTE)
            ),
            LeagueEditOutcome::Done
        );
        let deleted_at = T0 + 4 * MINUTE;
        assert_eq!(
            edit(&mut conn, delete_edit(&key, keep, deleted_at)),
            LeagueEditOutcome::Done
        );
        assert_eq!(
            league_columns(&conn, &key),
            LeagueColumns {
                removed_at: Some(deleted_at),
                user_state: Some("dropped".to_string()),
                user_dropped_at: Some(deleted_at),
                edited_at: Some(deleted_at),
                detached: false,
            },
            "a run the fold still has Active is dropped too"
        );
        let kept = match_columns(&conn, &tok(1));
        assert_eq!(
            (kept.league_id, kept.hidden_at, kept.edited_at),
            (Some(row_id(&conn, &key)), None, None),
            "Keep writes no match"
        );
        assert_eq!(
            edit(&mut conn, delete_edit(&key, keep, deleted_at + 1)),
            LeagueEditOutcome::LeagueRemoved
        );
        assert_eq!(
            edit(&mut conn, status_edit(&key, None, deleted_at + 1)),
            LeagueEditOutcome::LeagueRemoved
        );

        // The drop makes the MTGO league's next match start a new league
        // (decision 2).
        insert(&conn, &played(2, 30));
        sync_leagues(&conn);
        let next = format!("{LEAGUE}:{}", tok(2));
        assert_eq!(
            match_columns(&conn, &tok(2)).league_id,
            Some(row_id(&conn, &next))
        );
        assert_eq!(
            match_columns(&conn, &tok(1)).league_id,
            Some(row_id(&conn, &key))
        );
    }

    #[test]
    fn delete_them_too_hides_the_matches_and_undo_brings_everything_back() {
        let (_dir, mut conn) = edit_db("leagues-edit-delete-too");
        for n in [3, 4, 5] {
            insert(
                &conn,
                &Played {
                    league: None,
                    ..played(n, i64::from(n))
                },
            );
            link(&conn, &tok(n), 10);
        }
        manual_member(&conn, 1, &tok(3));
        manual_member(&conn, 1, &tok(4));

        assert_eq!(
            edit(
                &mut conn,
                delete_edit("manual:1", DeleteMatches::DeleteToo, 9_000)
            ),
            LeagueEditOutcome::Done
        );
        for n in [3, 4] {
            let hidden = match_columns(&conn, &tok(n));
            assert_eq!(
                (hidden.hidden_at, hidden.edited_at),
                (Some(9_000), Some(9_000))
            );
        }
        assert_eq!(
            match_columns(&conn, &tok(5)).hidden_at,
            None,
            "not a member"
        );
        assert_eq!(league_columns(&conn, "manual:1").removed_at, Some(9_000));

        // An Undo of another delete changes nothing.
        let undo = undo_edit(&conn, "manual:1", 8_000, 9_200);
        assert_eq!(edit(&mut conn, undo), LeagueEditOutcome::Done);
        assert_eq!(league_columns(&conn, "manual:1").removed_at, Some(9_000));

        let undo = undo_edit(&conn, "manual:1", 9_000, 9_500);
        assert_eq!(edit(&mut conn, undo), LeagueEditOutcome::Done);
        let league = league_columns(&conn, "manual:1");
        assert_eq!((league.removed_at, league.edited_at), (None, Some(9_500)));
        for n in [3, 4] {
            let back = match_columns(&conn, &tok(n));
            assert_eq!((back.hidden_at, back.edited_at), (None, Some(9_500)));
        }
        assert_eq!(members(&conn, 1), vec![tok(3), tok(4)]);
        assert_eq!(
            edit(
                &mut conn,
                LeagueEdit::UndoDelete {
                    league_id: 99,
                    removed_at: 9_000,
                    now_ms: 9_600,
                }
            ),
            LeagueEditOutcome::NotFound
        );
    }

    #[test]
    fn a_fully_deleted_run_stays_detached_and_comes_back_on_undo() {
        let (_dir, mut conn) = edit_db("leagues-edit-delete-run");
        for n in 2..=6 {
            insert(
                &conn,
                &Played {
                    league: Some(LEAGUE_2),
                    ..played(n, i64::from(n) * 10)
                },
            );
        }
        sync_leagues(&conn);
        let key = format!("{LEAGUE_2}:{}", tok(2));
        assert_eq!(stored(&conn, &key).state, RunState::Complete);

        let deleted_at = T0 + 90 * MINUTE;
        assert_eq!(
            edit(
                &mut conn,
                delete_edit(&key, DeleteMatches::DeleteToo, deleted_at)
            ),
            LeagueEditOutcome::Done
        );
        let league = league_columns(&conn, &key);
        assert_eq!(
            (league.removed_at, league.detached),
            (Some(deleted_at), true),
            "no fold run is left: detached, not deleted"
        );

        let undo = undo_edit(&conn, &key, deleted_at, deleted_at + 1);
        assert_eq!(edit(&mut conn, undo), LeagueEditOutcome::Done);
        let league = league_columns(&conn, &key);
        assert_eq!((league.removed_at, league.detached), (None, false));
        assert_eq!(stored(&conn, &key).matches.len(), 5);
        for n in 2..=6 {
            assert_eq!(match_columns(&conn, &tok(n)).hidden_at, None);
        }
    }

    /// Undo goes by the row id (sync identity spec 2.5): a delete of the
    /// deleted league's first match on its own rekeys the row, and Undo
    /// still finds it, with its status, under the key it has now.
    #[test]
    fn undo_restores_a_league_rekeyed_after_its_delete() {
        use crate::match_edits::{MatchEdit, MatchEditOutcome, edit_match};
        let (_dir, mut conn) = edit_db("leagues-undo-after-rekey");
        for n in 2..=6 {
            insert(
                &conn,
                &Played {
                    league: Some(LEAGUE_2),
                    ..played(n, i64::from(n) * 10)
                },
            );
            link(&conn, &tok(n), 10);
        }
        sync_leagues(&conn);
        let key = format!("{LEAGUE_2}:{}", tok(2));
        let id = row_id(&conn, &key);
        let deleted_at = T0 + 90 * MINUTE;
        assert_eq!(
            edit(
                &mut conn,
                status_edit(&key, Some(UserState::Partial), deleted_at - MINUTE)
            ),
            LeagueEditOutcome::Done
        );
        assert_eq!(
            edit(
                &mut conn,
                delete_edit(&key, DeleteMatches::KeepAsNonLeague, deleted_at)
            ),
            LeagueEditOutcome::Done
        );
        let hide = MatchEdit::Hide {
            token: tok(2),
            now_ms: deleted_at + MINUTE,
        };
        assert_eq!(
            edit_match(&mut conn, &hide).expect("hide"),
            MatchEditOutcome::Done
        );
        sync_leagues(&conn);
        let rekeyed = format!("{LEAGUE_2}:{}", tok(3));
        assert_eq!(row_id(&conn, &rekeyed), id, "the same row, rekeyed");

        assert_eq!(
            edit(
                &mut conn,
                LeagueEdit::UndoDelete {
                    league_id: id,
                    removed_at: deleted_at,
                    now_ms: deleted_at + 2 * MINUTE,
                }
            ),
            LeagueEditOutcome::Done
        );
        let league = league_columns(&conn, &rekeyed);
        assert_eq!(
            (
                league.removed_at,
                league.user_state.as_deref(),
                league.detached
            ),
            (None, Some("partial"), false)
        );
        assert_eq!(
            crate::read::league_run_key(&conn, id).expect("run key"),
            Some(rekeyed)
        );
        assert_eq!(crate::read::league_run_key(&conn, 99).expect("none"), None);
    }

    #[test]
    fn restoring_a_league_shows_its_kept_matches_again() {
        let (_dir, mut conn) = edit_db("leagues-edit-restore");
        insert(
            &conn,
            &Played {
                league: None,
                ..played(1, 1)
            },
        );
        link(&conn, &tok(1), 10);
        manual_member(&conn, 1, &tok(1));
        assert_eq!(
            edit(
                &mut conn,
                delete_edit("manual:1", DeleteMatches::KeepAsNonLeague, 9_000)
            ),
            LeagueEditOutcome::Done
        );
        assert_eq!(
            edit(&mut conn, restore_edit("manual:1", 9_500)),
            LeagueEditOutcome::Done
        );
        let league = league_columns(&conn, "manual:1");
        assert_eq!((league.removed_at, league.edited_at), (None, Some(9_500)));
        assert_eq!(tokens(&stored(&conn, "manual:1").matches), vec![tok(1)]);
        assert_eq!(
            match_columns(&conn, &tok(1)).edited_at,
            None,
            "Keep and Restore never write the matches"
        );
        assert_eq!(
            edit(&mut conn, restore_edit("manual:1", 9_600)),
            LeagueEditOutcome::Done,
            "restoring a listed league changes nothing"
        );
        assert_eq!(league_columns(&conn, "manual:1").edited_at, Some(9_500));
        assert_eq!(
            edit(&mut conn, restore_edit("manual:99", 9_600)),
            LeagueEditOutcome::NotFound
        );
    }

    #[test]
    fn a_removed_league_leaves_every_league_read_and_its_matches_read_as_in_none() {
        let (_dir, mut conn) = edit_db("leagues-removed-reads");
        insert(
            &conn,
            &Played {
                league: None,
                ..played(1, 1)
            },
        );
        link(&conn, &tok(1), 10);
        manual_member(&conn, 1, &tok(1));
        manual(&conn, 2, Some(1), Some(1));
        assert_eq!(
            edit(
                &mut conn,
                delete_edit("manual:1", DeleteMatches::KeepAsNonLeague, 9_000)
            ),
            LeagueEditOutcome::Done
        );
        let listed: Vec<String> = runs_of(&conn, LeagueOwner::Account(1), "manual")
            .into_iter()
            .map(|run| run.key)
            .collect();
        assert_eq!(listed, vec!["manual:2".to_string()]);
        assert!(available_tokens(&conn, "manual:1").is_empty());
        let offered = available_matches(&conn, "manual:2").expect("available");
        assert_eq!(offered.len(), 1);
        assert_eq!(offered[0].now_in, None, "its league is deleted");

        assert_eq!(
            edit(&mut conn, restore_edit("manual:1", 9_500)),
            LeagueEditOutcome::Done
        );
        assert_eq!(tokens(&stored(&conn, "manual:1").matches), vec![tok(1)]);
    }

    #[test]
    fn a_removed_limited_run_takes_its_draft_off_the_limited_pages() {
        let (_dir, conn) = limited_0x_db("leagues-removed-limited");
        // Set by hand: the edit refuses an Active run; the read is tested here.
        conn.execute(
            "UPDATE leagues SET removed_at = 5 WHERE run_key = '0x:29'",
            [],
        )
        .expect("removed");
        let found = limited_at(&conn, LeagueOwner::Account(1), 1_000 + 31 * MINUTE);
        let keys: Vec<&str> = found
            .runs
            .iter()
            .map(|limited| limited.run.key.as_str())
            .collect();
        assert_eq!(keys, vec!["0x:30"]);
        let unlinked: Vec<&str> = found
            .unlinked_drafts
            .iter()
            .map(|draft| draft.token.as_str())
            .collect();
        assert_eq!(
            unlinked,
            vec!["q-draft-b"],
            "its draft does not become a draft of an unknown league"
        );
    }

    /// Review focus 4, issues #416 and #418: a match deleted on its own,
    /// whose league is deleted later, was not deleted with the league, so
    /// it can be restored on its own. It is never linked to the deleted
    /// league: it comes back as a non-league match and reads as one
    /// everywhere (the Matches page's League and Casual filters and the
    /// match header), never a league name, never a new league.
    #[test]
    fn a_restored_match_of_a_removed_league_reads_as_non_league() {
        use crate::match_edits::{MatchEdit, MatchEditOutcome, edit_match};
        use crate::match_list::{KindFilter, MatchListFilter, match_list};
        let (_dir, mut conn) = edit_db("leagues-restored-match-removed-league");
        insert(
            &conn,
            &Played {
                league: None,
                ..played(4, 40)
            },
        );
        link(&conn, &tok(4), 10);
        link_league_match(&conn, 1, &tok(4));
        sync_leagues(&conn);
        let deleted_at = T0 + 90 * MINUTE;
        let hide = MatchEdit::Hide {
            token: tok(4),
            now_ms: deleted_at - MINUTE,
        };
        assert_eq!(
            edit_match(&mut conn, &hide).expect("hide"),
            MatchEditOutcome::Done
        );
        sync_leagues(&conn);
        assert_eq!(
            edit(
                &mut conn,
                delete_edit("manual:1", DeleteMatches::DeleteToo, deleted_at)
            ),
            LeagueEditOutcome::Done
        );

        let restore = MatchEdit::Restore {
            token: tok(4),
            now_ms: deleted_at + MINUTE,
        };
        assert_eq!(
            edit_match(&mut conn, &restore).expect("restore"),
            MatchEditOutcome::Done
        );
        sync_leagues(&conn);
        let alone = match_columns(&conn, &tok(4));
        assert_eq!(
            (alone.hidden_at, alone.league_id, alone.no_league),
            (None, None, true)
        );

        let header = crate::match_detail::match_header(&conn, &tok(4))
            .expect("read")
            .expect("listed again");
        assert!(!header.in_league, "the match header shows no league");
        assert_eq!(header.league_name, None);
        let listed = |kind: KindFilter| -> Vec<String> {
            let filter = MatchListFilter {
                kind: Some(kind),
                ..MatchListFilter::newest_first(decks::stats::Range::all_time())
            };
            match_list(
                &conn,
                crate::match_list::Scope::All(crate::Platform::Mtgo),
                &filter,
                1,
                50,
            )
            .expect("list")
            .rows
            .into_iter()
            .map(|row| row.token)
            .collect()
        };
        assert!(listed(KindFilter::League).is_empty(), "not a league match");
        assert_eq!(listed(KindFilter::Casual), vec![tok(4)]);
        assert_eq!(
            count(
                &conn,
                "SELECT COUNT(*) FROM listed_leagues WHERE source = 'log'"
            ),
            0,
            "the fold made no new league of it"
        );
    }

    /// Spec 4.5 (ruling after the final review): Restore of a league
    /// deleted with "Delete them too" brings back the matches deleted with
    /// it, as Undo does, each with a fresh `edited_at`. A match of the same
    /// account deleted on its own at another time stays deleted.
    #[test]
    fn restoring_a_league_restores_the_matches_deleted_with_it() {
        let (_dir, mut conn) = edit_db("leagues-restore-deleted-too");
        for n in 2..=6 {
            insert(
                &conn,
                &Played {
                    league: Some(LEAGUE_2),
                    ..played(n, i64::from(n) * 10)
                },
            );
            link(&conn, &tok(n), 10);
        }
        insert(
            &conn,
            &Played {
                league: None,
                ..played(7, 200)
            },
        );
        conn.execute(
            "UPDATE matches SET hidden_at = 5, edited_at = 5 WHERE token = ?1",
            [tok(7)],
        )
        .expect("deleted on its own");
        sync_leagues(&conn);
        let key = format!("{LEAGUE_2}:{}", tok(2));
        let deleted_at = T0 + 90 * MINUTE;
        assert_eq!(
            edit(
                &mut conn,
                delete_edit(&key, DeleteMatches::DeleteToo, deleted_at)
            ),
            LeagueEditOutcome::Done
        );

        let restored_at = deleted_at + MINUTE;
        assert_eq!(
            edit(&mut conn, restore_edit(&key, restored_at)),
            LeagueEditOutcome::Done
        );
        let league = league_columns(&conn, &key);
        assert_eq!(
            (league.removed_at, league.edited_at, league.detached),
            (None, Some(restored_at), false)
        );
        for n in 2..=6 {
            let back = match_columns(&conn, &tok(n));
            assert_eq!((back.hidden_at, back.edited_at), (None, Some(restored_at)));
        }
        assert_eq!(stored(&conn, &key).matches.len(), 5);
        let alone = match_columns(&conn, &tok(7));
        assert_eq!((alone.hidden_at, alone.edited_at), (Some(5), Some(5)));
    }

    /// Issue #418 (owner ruling): a match deleted with its log league
    /// cannot be restored on its own while the league stays deleted, not
    /// even the run's first match: Restore is refused and writes nothing.
    /// The league's Restore then brings it back whole: one league, the
    /// same run key and row, its notes and status kept, and all its
    /// matches.
    #[test]
    fn a_log_league_restored_after_one_of_its_matches_is_one_league_again() {
        use crate::match_edits::{MatchEdit, MatchEditOutcome, edit_match};
        let (_dir, mut conn) = edit_db("leagues-restore-one-then-league");
        for n in 2..=6 {
            insert(
                &conn,
                &Played {
                    league: Some(LEAGUE_2),
                    ..played(n, i64::from(n) * 10)
                },
            );
            link(&conn, &tok(n), 10);
        }
        sync_leagues(&conn);
        let key = format!("{LEAGUE_2}:{}", tok(2));
        let id = row_id(&conn, &key);
        let deleted_at = T0 + 90 * MINUTE;
        assert_eq!(
            edit(&mut conn, notes_edit(&key, "Q notes")),
            LeagueEditOutcome::Done
        );
        assert_eq!(
            edit(
                &mut conn,
                status_edit(&key, Some(UserState::Partial), deleted_at - MINUTE)
            ),
            LeagueEditOutcome::Done
        );
        assert_eq!(
            edit(
                &mut conn,
                delete_edit(&key, DeleteMatches::DeleteToo, deleted_at)
            ),
            LeagueEditOutcome::Done
        );
        let before = match_columns(&conn, &tok(2));

        let restore = MatchEdit::Restore {
            token: tok(2),
            now_ms: deleted_at + MINUTE,
        };
        assert_eq!(
            edit_match(&mut conn, &restore).expect("restore"),
            MatchEditOutcome::DeletedWithLeague
        );
        sync_leagues(&conn);
        assert_eq!(match_columns(&conn, &tok(2)), before, "refused: unchanged");
        assert_eq!(before.hidden_at, Some(deleted_at));

        assert_eq!(
            edit(&mut conn, restore_edit(&key, deleted_at + 2 * MINUTE)),
            LeagueEditOutcome::Done
        );
        assert_eq!(
            count(
                &conn,
                "SELECT COUNT(*) FROM listed_leagues WHERE source = 'log'"
            ),
            1,
            "one league, never two"
        );
        assert_eq!(row_id(&conn, &key), id, "the same row");
        let league = league_columns(&conn, &key);
        assert_eq!(
            (
                league.removed_at,
                league.user_state.as_deref(),
                league.detached
            ),
            (None, Some("partial"), false)
        );
        assert_eq!(note(&conn, &key).as_deref(), Some("Q notes"));
        assert_eq!(stored(&conn, &key).matches.len(), 5);
        for n in 2..=6 {
            let back = match_columns(&conn, &tok(n));
            assert_eq!(
                (back.hidden_at, back.league_id, back.no_league),
                (None, Some(id), false)
            );
        }
    }

    /// Issues #419 and #416: a log match deleted on its own (the league sync
    /// clears its `league_id`), restored after its league is deleted, comes
    /// back non-league: its league is the one it was deleted from
    /// (`deleted_from_league_id`), deleted since. Whether it was the run's
    /// first match (the league rekeys to the second, same row) or not, and
    /// whether the league kept or deleted its other matches. The Archives
    /// row names the league, removed; no new league appears; the league's
    /// later Restore brings it back with its other matches and the restored
    /// match stays non-league.
    #[test]
    fn a_match_deleted_before_its_league_is_restored_non_league() {
        use crate::match_edits::{MatchEdit, MatchEditOutcome, edit_match};
        let cases = [
            (4, DeleteMatches::DeleteToo),
            (2, DeleteMatches::DeleteToo),
            (2, DeleteMatches::KeepAsNonLeague),
        ];
        for (alone, choice) in cases {
            let label = format!("leagues-restore-alone-{alone}-{choice:?}");
            let (_dir, mut conn) = edit_db(&label);
            for n in 2..=6 {
                insert(
                    &conn,
                    &Played {
                        league: Some(LEAGUE_2),
                        ..played(n, i64::from(n) * 10)
                    },
                );
                link(&conn, &tok(n), 10);
            }
            sync_leagues(&conn);
            let id = row_id(&conn, &format!("{LEAGUE_2}:{}", tok(2)));
            let key_now = |conn: &Connection| -> String {
                conn.query_row("SELECT run_key FROM leagues WHERE id = ?1", [id], |row| {
                    row.get(0)
                })
                .expect("the league's key")
            };
            let hide = MatchEdit::Hide {
                token: tok(alone),
                now_ms: T0 + 80 * MINUTE,
            };
            assert_eq!(
                edit_match(&mut conn, &hide).expect("hide"),
                MatchEditOutcome::Done
            );
            sync_leagues(&conn);
            assert_eq!(deleted_from(&conn, &tok(alone)), Some(id), "{label}");
            let deleted_at = T0 + 90 * MINUTE;
            let key = key_now(&conn);
            assert_eq!(
                edit(
                    &mut conn,
                    status_edit(&key, Some(UserState::Complete), deleted_at - MINUTE)
                ),
                LeagueEditOutcome::Done
            );
            assert_eq!(
                edit(&mut conn, delete_edit(&key, choice, deleted_at)),
                LeagueEditOutcome::Done
            );

            let listed =
                crate::deleted::deleted_items(&conn, crate::Platform::Mtgo).expect("deleted items");
            let row = listed
                .matches
                .iter()
                .find(|row| row.token == tok(alone))
                .expect("listed");
            let name: String = conn
                .query_row("SELECT name FROM leagues WHERE id = ?1", [id], |row| {
                    row.get(0)
                })
                .expect("name");
            assert_eq!(row.league_name.as_deref(), Some(name.as_str()), "{label}");
            assert!(row.league_removed, "{label}");
            assert_eq!(row.deleted_with_league, None, "{label}");

            let restore = MatchEdit::Restore {
                token: tok(alone),
                now_ms: deleted_at + MINUTE,
            };
            assert_eq!(
                edit_match(&mut conn, &restore).expect("restore"),
                MatchEditOutcome::Done
            );
            sync_leagues(&conn);
            let back = match_columns(&conn, &tok(alone));
            assert_eq!(
                (
                    back.hidden_at,
                    back.league_id,
                    back.league_linked,
                    back.no_league,
                    back.edited_at
                ),
                (None, None, false, true, Some(deleted_at + MINUTE)),
                "{label}"
            );
            assert_eq!(
                count(
                    &conn,
                    "SELECT COUNT(*) FROM listed_leagues WHERE source = 'log'"
                ),
                0,
                "no new league of the restored match: {label}"
            );
            assert_eq!(
                count(&conn, "SELECT COUNT(*) FROM leagues WHERE source = 'log'"),
                1,
                "only the deleted one: {label}"
            );

            let key = key_now(&conn);
            assert_eq!(
                edit(&mut conn, restore_edit(&key, deleted_at + 2 * MINUTE)),
                LeagueEditOutcome::Done,
                "{label}"
            );
            sync_leagues(&conn);
            assert_eq!(
                count(
                    &conn,
                    "SELECT COUNT(*) FROM listed_leagues WHERE source = 'log'"
                ),
                1,
                "{label}"
            );
            assert_eq!(stored(&conn, &key_now(&conn)).matches.len(), 4, "{label}");
            let still = match_columns(&conn, &tok(alone));
            assert_eq!((still.league_id, still.no_league), (None, true), "{label}");
        }
    }

    /// Spec section 13: a match's Delete and a league's "Delete them too"
    /// record the league each match was in, which survives the league sync
    /// clearing their `league_id`; a match's Restore and the league's
    /// Restore clear it.
    #[test]
    fn every_delete_records_the_league_the_match_was_in_and_restore_clears_it() {
        use crate::match_edits::{MatchEdit, MatchEditOutcome, edit_match};
        let (_dir, mut conn) = edit_db("leagues-deleted-from-recorded");
        for n in 2..=6 {
            insert(
                &conn,
                &Played {
                    league: Some(LEAGUE_2),
                    ..played(n, i64::from(n) * 10)
                },
            );
            link(&conn, &tok(n), 10);
        }
        sync_leagues(&conn);
        let key = format!("{LEAGUE_2}:{}", tok(2));
        let id = row_id(&conn, &key);

        let hide = MatchEdit::Hide {
            token: tok(6),
            now_ms: T0 + 80 * MINUTE,
        };
        assert_eq!(
            edit_match(&mut conn, &hide).expect("hide"),
            MatchEditOutcome::Done
        );
        sync_leagues(&conn);
        assert_eq!(match_columns(&conn, &tok(6)).league_id, None);
        assert_eq!(
            deleted_from(&conn, &tok(6)),
            Some(id),
            "a Delete records it"
        );
        let restore = MatchEdit::Restore {
            token: tok(6),
            now_ms: T0 + 81 * MINUTE,
        };
        assert_eq!(
            edit_match(&mut conn, &restore).expect("restore"),
            MatchEditOutcome::Done
        );
        sync_leagues(&conn);
        assert_eq!(deleted_from(&conn, &tok(6)), None, "a Restore clears it");
        assert_eq!(match_columns(&conn, &tok(6)).league_id, Some(id));

        let deleted_at = T0 + 90 * MINUTE;
        assert_eq!(
            edit(
                &mut conn,
                status_edit(&key, Some(UserState::Complete), deleted_at - MINUTE)
            ),
            LeagueEditOutcome::Done
        );
        assert_eq!(
            edit(
                &mut conn,
                delete_edit(&key, DeleteMatches::DeleteToo, deleted_at)
            ),
            LeagueEditOutcome::Done
        );
        for n in 2..=6 {
            assert_eq!(match_columns(&conn, &tok(n)).league_id, None);
            assert_eq!(
                deleted_from(&conn, &tok(n)),
                Some(id),
                "\"Delete them too\" records it"
            );
        }
        assert_eq!(
            edit(&mut conn, restore_edit(&key, deleted_at + MINUTE)),
            LeagueEditOutcome::Done
        );
        for n in 2..=6 {
            assert_eq!(deleted_from(&conn, &tok(n)), None);
            assert_eq!(match_columns(&conn, &tok(n)).league_id, Some(id));
        }
    }

    /// Two runs of one MTGO league (the first dropped, then a new run): the
    /// second run's first match deleted on its own (the run rekeys and now
    /// starts after it), then the second run deleted. Restoring the match
    /// resolves to the second run, the one it was in, so it comes back
    /// non-league; the first run, listed and of the same league token, is
    /// left alone.
    #[test]
    fn a_match_restored_after_its_run_is_deleted_never_joins_an_earlier_run_of_its_league() {
        use crate::match_edits::{MatchEdit, MatchEditOutcome, edit_match};
        let (_dir, mut conn) = edit_db("leagues-restore-second-run");
        for n in [2, 3] {
            insert(
                &conn,
                &Played {
                    league: Some(LEAGUE_2),
                    ..played(n, i64::from(n) * 10)
                },
            );
            link(&conn, &tok(n), 10);
        }
        sync_leagues(&conn);
        let first = format!("{LEAGUE_2}:{}", tok(2));
        assert_eq!(
            edit(
                &mut conn,
                status_edit(&first, Some(UserState::Dropped), T0 + 35 * MINUTE)
            ),
            LeagueEditOutcome::Done
        );
        for n in 4..=6 {
            insert(
                &conn,
                &Played {
                    league: Some(LEAGUE_2),
                    ..played(n, i64::from(n) * 10)
                },
            );
            link(&conn, &tok(n), 10);
        }
        sync_leagues(&conn);
        let first_id = row_id(&conn, &first);
        assert_eq!(stored(&conn, &first).matches.len(), 2);

        let hide = MatchEdit::Hide {
            token: tok(4),
            now_ms: T0 + 80 * MINUTE,
        };
        assert_eq!(
            edit_match(&mut conn, &hide).expect("hide"),
            MatchEditOutcome::Done
        );
        sync_leagues(&conn);
        let second = format!("{LEAGUE_2}:{}", tok(5));
        let second_id = row_id(&conn, &second);
        assert_eq!(deleted_from(&conn, &tok(4)), Some(second_id));
        let deleted_at = T0 + 90 * MINUTE;
        assert_eq!(
            edit(
                &mut conn,
                status_edit(&second, Some(UserState::Complete), deleted_at - MINUTE)
            ),
            LeagueEditOutcome::Done
        );
        assert_eq!(
            edit(
                &mut conn,
                delete_edit(&second, DeleteMatches::KeepAsNonLeague, deleted_at)
            ),
            LeagueEditOutcome::Done
        );

        let restore = MatchEdit::Restore {
            token: tok(4),
            now_ms: deleted_at + MINUTE,
        };
        assert_eq!(
            edit_match(&mut conn, &restore).expect("restore"),
            MatchEditOutcome::Done
        );
        sync_leagues(&conn);
        let back = match_columns(&conn, &tok(4));
        assert_eq!(
            (back.hidden_at, back.league_id, back.no_league),
            (None, None, true),
            "its run is deleted: non-league"
        );
        assert_eq!(row_id(&conn, &first), first_id);
        assert_eq!(stored(&conn, &first).matches.len(), 2, "the first run");
        assert_eq!(
            count(
                &conn,
                "SELECT COUNT(*) FROM listed_leagues WHERE source = 'log'"
            ),
            1,
            "only the first run is listed"
        );
    }

    /// Restore puts a match back as it was just before its delete (ruling
    /// after the final branch review). A league deleted with "Keep", whose
    /// kept matches are then each deleted on their own, holds no match: its
    /// Restore and Undo are refused and write nothing. A kept match restored
    /// comes back as the kept match it was (still in the deleted league,
    /// read as non-league), so the league's Restore then succeeds with it,
    /// its notes and its status.
    #[test]
    fn a_league_with_no_match_left_is_not_restored_until_a_kept_match_is() {
        use crate::match_edits::{MatchEdit, MatchEditOutcome, edit_match};
        let (_dir, mut conn) = edit_db("leagues-restore-nothing-to-show");
        for n in 2..=6 {
            insert(
                &conn,
                &Played {
                    league: Some(LEAGUE_2),
                    ..played(n, i64::from(n) * 10)
                },
            );
            link(&conn, &tok(n), 10);
        }
        sync_leagues(&conn);
        let key = format!("{LEAGUE_2}:{}", tok(2));
        let id = row_id(&conn, &key);
        let deleted_at = T0 + 90 * MINUTE;
        assert_eq!(
            edit(&mut conn, notes_edit(&key, "Q kept note")),
            LeagueEditOutcome::Done
        );
        assert_eq!(
            edit(
                &mut conn,
                status_edit(&key, Some(UserState::Partial), deleted_at - MINUTE)
            ),
            LeagueEditOutcome::Done
        );
        assert_eq!(
            edit(
                &mut conn,
                delete_edit(&key, DeleteMatches::KeepAsNonLeague, deleted_at)
            ),
            LeagueEditOutcome::Done
        );
        for n in 2..=6 {
            let hide = MatchEdit::Hide {
                token: tok(n),
                now_ms: deleted_at + i64::from(n) * MINUTE,
            };
            assert_eq!(
                edit_match(&mut conn, &hide).expect("hide"),
                MatchEditOutcome::Done
            );
            sync_leagues(&conn);
        }
        // Each delete of the run's first match rekeyed the same row to the
        // next (spec 4.4): it is the last match's key now, detached.
        let key = format!("{LEAGUE_2}:{}", tok(6));
        assert_eq!(row_id(&conn, &key), id);
        assert!(league_columns(&conn, &key).detached);
        let before = league_columns(&conn, &key);

        assert_eq!(
            edit(&mut conn, restore_edit(&key, deleted_at + 30 * MINUTE)),
            LeagueEditOutcome::NothingToShow
        );
        assert_eq!(
            LeagueEditOutcome::NothingToShow.refusal(),
            Some("No matches are left in this league, so it has nothing to show.")
        );
        assert_eq!(league_columns(&conn, &key), before, "nothing written");
        let undo = undo_edit(&conn, &key, deleted_at, deleted_at + 30 * MINUTE);
        assert_eq!(
            edit(&mut conn, undo),
            LeagueEditOutcome::NothingToShow,
            "Undo is refused the same way"
        );
        assert_eq!(league_columns(&conn, &key), before, "nothing written");
        let listed =
            crate::deleted::deleted_items(&conn, crate::Platform::Mtgo).expect("deleted items");
        assert!(listed.leagues.iter().any(|league| league.run_key == key));

        let restore = MatchEdit::Restore {
            token: tok(4),
            now_ms: deleted_at + 40 * MINUTE,
        };
        assert_eq!(
            edit_match(&mut conn, &restore).expect("restore"),
            MatchEditOutcome::Done
        );
        sync_leagues(&conn);
        let kept = match_columns(&conn, &tok(4));
        assert_eq!(
            (kept.hidden_at, kept.league_id, kept.no_league),
            (None, Some(id), false),
            "a kept match of the deleted league again"
        );
        assert_eq!(
            count(&conn, "SELECT COUNT(*) FROM listed_leagues"),
            1,
            "only the manual league: no new league"
        );

        let key = format!("{LEAGUE_2}:{}", tok(4));
        assert_eq!(row_id(&conn, &key), id, "the same row, rekeyed");
        assert_eq!(
            edit(&mut conn, restore_edit(&key, deleted_at + 50 * MINUTE)),
            LeagueEditOutcome::Done
        );
        let league = league_columns(&conn, &key);
        assert_eq!(
            (
                league.removed_at,
                league.user_state.as_deref(),
                league.detached
            ),
            (None, Some("partial"), false)
        );
        assert_eq!(note(&conn, &key).as_deref(), Some("Q kept note"));
        assert_eq!(stored(&conn, &key).matches.len(), 1);
    }

    /// A match whose league row is detached reads as a non-league match, as
    /// the league pages, which leave detached rows out, show it.
    #[test]
    fn a_match_of_a_detached_league_reads_as_non_league() {
        use crate::match_list::{KindFilter, MatchListFilter, match_list};
        let (_dir, conn) = edit_db("leagues-detached-league-match");
        insert(&conn, &played(1, 1));
        sync_leagues(&conn);
        let key = format!("{LEAGUE}:{}", tok(1));
        conn.execute(
            "UPDATE leagues SET detached_at = 5 WHERE run_key = ?1",
            [&key],
        )
        .expect("detached");
        let listed = |kind: KindFilter| -> Vec<String> {
            let filter = MatchListFilter {
                kind: Some(kind),
                ..MatchListFilter::newest_first(decks::stats::Range::all_time())
            };
            match_list(
                &conn,
                crate::match_list::Scope::All(crate::Platform::Mtgo),
                &filter,
                1,
                50,
            )
            .expect("list")
            .rows
            .into_iter()
            .map(|row| row.token)
            .collect()
        };
        assert!(listed(KindFilter::League).is_empty(), "not a league match");
        assert_eq!(listed(KindFilter::Casual), vec![tok(1)]);
    }

    /// Issue #418: a log league deleted with "Delete them too" whose
    /// matches are each asked to come back on their own before the league
    /// does: every Restore is refused, so the league's Restore finds them
    /// all and the league is listed again with its record, never detached
    /// and never a second league.
    #[test]
    fn a_log_league_whose_matches_are_refused_alone_comes_back_whole() {
        use crate::match_edits::{MatchEdit, MatchEditOutcome, edit_match};
        let (_dir, mut conn) = edit_db("leagues-restore-matches-then-league");
        for n in 2..=6 {
            insert(
                &conn,
                &Played {
                    league: Some(LEAGUE_2),
                    ..played(n, i64::from(n) * 10)
                },
            );
            link(&conn, &tok(n), 10);
        }
        sync_leagues(&conn);
        let key = format!("{LEAGUE_2}:{}", tok(2));
        let deleted_at = T0 + 90 * MINUTE;
        assert_eq!(
            edit(
                &mut conn,
                delete_edit(&key, DeleteMatches::DeleteToo, deleted_at)
            ),
            LeagueEditOutcome::Done
        );
        for n in 2..=6 {
            let restore = MatchEdit::Restore {
                token: tok(n),
                now_ms: deleted_at + MINUTE,
            };
            assert_eq!(
                edit_match(&mut conn, &restore).expect("restore"),
                MatchEditOutcome::DeletedWithLeague
            );
        }
        sync_leagues(&conn);

        assert_eq!(
            edit(&mut conn, restore_edit(&key, deleted_at + 2 * MINUTE)),
            LeagueEditOutcome::Done
        );
        let league = league_columns(&conn, &key);
        assert_eq!((league.removed_at, league.detached), (None, false));
        let run = stored(&conn, &key);
        assert_eq!(run.matches.len(), 5, "listed with every match");
        assert_eq!(run.state, RunState::Complete);
        assert_eq!(
            count(
                &conn,
                "SELECT COUNT(*) FROM listed_leagues WHERE source = 'log'"
            ),
            1,
            "no second league"
        );
    }

    /// Issue #415: a log row kept attached only by a match moved into it
    /// (every folded match deleted) has no fold run to write its state, so
    /// the stale Active becomes Partial: never the run in progress, and
    /// Delete is allowed. The player's own status (`user_state`) is left
    /// alone.
    #[test]
    fn a_log_league_kept_only_by_a_moved_in_match_is_partial() {
        use crate::match_edits::{MatchEdit, MatchEditOutcome, edit_match};
        let (_dir, mut conn) = edit_db("leagues-moved-in-only-partial");
        insert(&conn, &played(1, 0));
        link(&conn, &tok(1), 10);
        insert(
            &conn,
            &Played {
                league: None,
                ..played(2, 10)
            },
        );
        link(&conn, &tok(2), 10);
        sync_leagues(&conn);
        let key = format!("{LEAGUE}:{}", tok(1));
        assert_eq!(stored(&conn, &key).state, RunState::Active);
        assert_eq!(
            edit(&mut conn, move_edit(&tok(2), Some(&key), T0 + 20 * MINUTE)),
            LeagueEditOutcome::Done
        );
        let hide = MatchEdit::Hide {
            token: tok(1),
            now_ms: T0 + 21 * MINUTE,
        };
        assert_eq!(
            edit_match(&mut conn, &hide).expect("delete"),
            MatchEditOutcome::Done
        );
        sync_leagues(&conn);

        let run = stored(&conn, &key);
        assert_eq!(tokens(&run.matches), vec![tok(2)]);
        assert_eq!(run.state, RunState::Partial);
        assert_eq!(league_columns(&conn, &key).user_state, None);
        let card =
            crate::overlay::league_card_inputs(&conn, 1, T0 + 22 * MINUTE, overlay::GRACE_MS)
                .expect("card");
        assert!(
            card.runs.iter().all(|card_run| card_run.id != run.id),
            "not the run in progress"
        );

        // Live equals rebuild: syncing again from the stored rows changes
        // nothing.
        let tx = conn.unchecked_transaction().expect("transaction");
        let again = crate::league_sync::sync_every_owner(&tx, chrono_tz::UTC, 0).expect("sync");
        tx.commit().expect("commit");
        assert!(!again.leagues_changed, "a second sync writes nothing");
        assert_eq!(stored(&conn, &key).state, RunState::Partial);

        assert_eq!(
            edit(
                &mut conn,
                delete_edit(&key, DeleteMatches::KeepAsNonLeague, T0 + 23 * MINUTE)
            ),
            LeagueEditOutcome::Done
        );
    }

    #[test]
    fn move_refusal_holds_the_move_rules_for_a_match_and_a_league_row() {
        let (_dir, mut conn) = edit_db("leagues-move-refusal");
        for n in 1..=6 {
            insert(
                &conn,
                &Played {
                    league: None,
                    ..played(n, i64::from(n))
                },
            );
            link(&conn, &tok(n), 10);
        }
        insert(
            &conn,
            &Played {
                league: None,
                tournament: Some((500, 1)),
                ..played(7, 7)
            },
        );
        link(&conn, &tok(7), 10);
        for n in 1..=5 {
            manual_member(&conn, 1, &tok(n));
        }
        let id_of = |conn: &Connection, n: u32| {
            count(
                conn,
                &format!("SELECT id FROM matches WHERE token = '{}'", tok(n)),
            )
        };
        let refusal = |conn: &mut Connection, n: u32, league_id: i64| {
            let match_id = id_of(conn, n);
            let tx = conn.transaction().expect("transaction");
            move_refusal(&tx, match_id, league_id).expect("rules")
        };
        assert_eq!(refusal(&mut conn, 1, 1), None, "already in it");
        assert_eq!(
            refusal(&mut conn, 6, 1),
            Some(LeagueEditOutcome::TooManyMatches)
        );
        assert_eq!(
            refusal(&mut conn, 7, 1),
            Some(LeagueEditOutcome::Tournament)
        );
        assert_eq!(refusal(&mut conn, 6, 99), Some(LeagueEditOutcome::NotFound));
        hide(&conn, &tok(6));
        assert_eq!(
            refusal(&mut conn, 6, 1),
            Some(LeagueEditOutcome::MatchNotFound)
        );
        conn.execute("UPDATE leagues SET removed_at = 1 WHERE id = 1", [])
            .expect("removed");
        assert_eq!(
            refusal(&mut conn, 1, 1),
            Some(LeagueEditOutcome::LeagueRemoved)
        );
    }

    #[test]
    fn move_targets_offer_the_leagues_a_match_may_move_into() {
        let (_dir, conn) = edit_db("leagues-move-targets");
        // Manual 1 (deck 1) is in `edit_db`; 2 is of deck 2, 3 has no account
        // and no deck, 5 is detached, 6 of deck 1 is deleted.
        manual(&conn, 2, Some(1), Some(2));
        manual(&conn, 3, None, None);
        manual(&conn, 5, Some(1), Some(1));
        manual(&conn, 6, Some(1), Some(1));
        conn.execute_batch(
            "UPDATE leagues SET detached_at = 1 WHERE id = 5;
             UPDATE leagues SET removed_at = 1, user_state = 'partial' WHERE id = 6;
             UPDATE leagues SET user_state = 'partial' WHERE id = 3;",
        )
        .expect("detached and deleted");
        insert(
            &conn,
            &Played {
                league: None,
                ..played(1, 1)
            },
        );
        link(&conn, &tok(1), 10);
        let keys = |conn: &Connection| {
            move_targets(conn, &tok(1))
                .expect("targets")
                .into_iter()
                .map(|target| (target.league.run_key, target.state, target.match_count))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            keys(&conn),
            vec![
                ("manual:3".to_string(), "partial".to_string(), 0),
                ("manual:1".to_string(), "complete".to_string(), 0),
            ],
            "newest first, the shown state"
        );
        assert!(move_targets(&conn, "q-none").expect("targets").is_empty());
        assert!(match_leagues(&conn, &[tok(1)]).expect("leagues").is_empty());

        manual_member(&conn, 1, &tok(1));
        assert_eq!(
            keys(&conn),
            vec![("manual:3".to_string(), "partial".to_string(), 0)],
            "never the league it is in"
        );
        let now_in = match_leagues(&conn, &[tok(1), tok(2)]).expect("leagues");
        assert_eq!(now_in.len(), 1);
        assert_eq!(now_in[&tok(1)].run_key, "manual:1");

        assert_eq!(
            league_status(&conn, "manual:3").expect("status"),
            Some(LeagueStatus {
                id: 3,
                state: RunState::Complete,
                user_state: Some(UserState::Partial),
            })
        );
        assert_eq!(league_status(&conn, "manual:6").expect("status"), None);
        assert_eq!(league_status(&conn, "manual:404").expect("status"), None);
    }

    #[test]
    fn a_limited_run_takes_only_limited_matches_of_its_set() {
        let (_dir, mut conn) = limited_0x_db("leagues-edit-link-limited");
        conn.execute_batch(
            "INSERT INTO matches (token, account_id, source, format, state, outcome, started_at,
                 last_activity_at, revision, derived_version)
             VALUES ('q-s1', 1, 'imported', 'S6FRA', 'complete', 'win', 8, 8, 1, 0),
                    ('q-s2', 1, 'imported', 'S6MKM', 'complete', 'win', 9, 9, 1, 0),
                    ('q-c2', 1, 'imported', 'CMODERN', 'complete', 'win', 10, 10, 1, 0);",
        )
        .expect("matches");
        sync_leagues(&conn);
        let into = |run_key: &str, token: &str| link_edit(run_key, &[token.to_string()]);
        assert_eq!(
            edit(&mut conn, into("0x:30", "q-s2")),
            LeagueEditOutcome::WrongDeck
        );
        assert_eq!(
            edit(&mut conn, into("0x:30", "q-c2")),
            LeagueEditOutcome::WrongKind
        );
        assert_eq!(
            edit(&mut conn, into("0x:42", "q-s1")),
            LeagueEditOutcome::WrongKind
        );
        assert_eq!(
            edit(&mut conn, into("0x:30", "q-s1")),
            LeagueEditOutcome::Done
        );
        assert_eq!(members(&conn, 1030), vec!["q-s1".to_string()]);
    }

    #[test]
    fn a_limited_run_reads_the_players_status_beside_the_folds() {
        let (_dir, mut conn) = limited_0x_db("leagues-limited-user-state");
        sync_leagues(&conn);
        assert_eq!(
            edit(
                &mut conn,
                status_edit("0x:29", Some(UserState::Partial), 9_000)
            ),
            LeagueEditOutcome::Done
        );
        let found = limited_at(&conn, LeagueOwner::Account(1), 10_000);
        let run = found
            .runs
            .iter()
            .find(|limited| limited.run.key == "0x:29")
            .expect("run");
        assert_eq!(run.user_state, Some(UserState::Partial));
        assert_eq!(run.shown_state(), RunState::Partial);
        assert_ne!(run.run.state, RunState::Partial);
        let other = found
            .runs
            .iter()
            .find(|limited| limited.run.key == "0x:30")
            .expect("run");
        assert_eq!(other.user_state, None);
        assert_eq!(other.shown_state(), other.run.state);
    }

    #[test]
    fn move_targets_offer_a_limited_match_the_runs_of_its_set() {
        let (_dir, conn) = limited_0x_db("leagues-move-targets-limited");
        conn.execute_batch(
            "INSERT INTO matches (token, account_id, source, format, state, outcome, started_at,
                 last_activity_at, revision, derived_version)
             VALUES ('q-h', 1, 'imported', 'DHOBHOBHOB', 'complete', 'win', 20, 20, 1, 0),
                    ('q-sh', 1, 'imported', 'S6HOB', 'complete', 'win', 21, 21, 1, 0),
                    ('q-nosets', 1, 'imported', 'DXYZW', 'complete', 'win', 22, 22, 1, 0);",
        )
        .expect("matches");
        // 0x:29 is a draft run of HOB and 0x:30 a sealed run of FRA. Add a
        // sealed run of HOB, a draft run of FRA and a draft run with no set.
        let extra = [
            (31, "q-r1", 7003, "S6HOB", 50, ("sealed", Some("HOB"), None)),
            (
                32,
                "q-r2",
                7004,
                "Limited",
                60,
                ("draft", Some("FRA"), Some(36)),
            ),
            (33, "q-r3", 7005, "Limited", 70, ("draft", None, Some(37))),
        ];
        for (id, token, event, format, started, kind) in extra {
            let seed = limited_seed(token, event, "Run", format, ("complete", started), kind);
            insert_imported_run(&conn, id, Some(1), &seed);
        }
        sync_leagues(&conn);
        let keys = |token: &str| {
            move_targets(&conn, token)
                .expect("targets")
                .into_iter()
                .map(|target| target.league.run_key)
                .collect::<Vec<_>>()
        };
        // Newest first: the run with no set, the sealed HOB run, the draft
        // HOB run. Not the FRA runs, not the constructed league.
        assert_eq!(
            keys("q-h"),
            vec![
                "0x:33".to_string(),
                "0x:31".to_string(),
                "0x:29".to_string()
            ]
        );
        // A sealed match of the set is offered the draft run too.
        assert_eq!(
            keys("q-sh"),
            vec![
                "0x:33".to_string(),
                "0x:31".to_string(),
                "0x:29".to_string()
            ]
        );
        // A limited match whose format yields no set code is offered only the
        // runs with no set code.
        assert_eq!(keys("q-nosets"), vec!["0x:33".to_string()]);
        // A limited match already in a run is not offered its own run.
        assert_eq!(keys("q-d"), vec!["0x:33".to_string(), "0x:31".to_string()]);
    }

    #[test]
    fn move_targets_never_offer_a_constructed_match_a_limited_run() {
        let (_dir, conn) = limited_0x_db("leagues-move-targets-constructed");
        conn.execute_batch(
            "INSERT INTO matches (token, account_id, source, format, state, outcome, started_at,
                 last_activity_at, revision, derived_version)
             VALUES ('q-c3', 1, 'imported', 'CMODERN', 'complete', 'win', 20, 20, 1, 0);",
        )
        .expect("match");
        sync_leagues(&conn);
        let keys: Vec<String> = move_targets(&conn, "q-c3")
            .expect("targets")
            .into_iter()
            .map(|target| target.league.run_key)
            .collect();
        assert_eq!(keys, vec!["0x:42".to_string()]);
    }

    #[test]
    fn removing_a_match_from_its_league_and_moving_it_back() {
        let (_dir, mut conn) = edit_db("leagues-edit-remove");
        insert(&conn, &played(1, 0));
        insert(&conn, &played(2, 10));
        insert(
            &conn,
            &Played {
                league: None,
                ..played(3, 20)
            },
        );
        insert(
            &conn,
            &Played {
                league: None,
                tournament: Some((500, 1)),
                ..played(4, 30)
            },
        );
        for n in 1..=4 {
            link(&conn, &tok(n), 10);
        }
        manual_member(&conn, 1, &tok(3));
        sync_leagues(&conn);
        let key = format!("{LEAGUE}:{}", tok(1));
        let unlink = |run_key: &str, token: &str| LeagueEdit::UnlinkMatch {
            run_key: run_key.to_string(),
            match_token: token.to_string(),
            now_ms: 9_000,
        };
        assert_eq!(tokens(&stored(&conn, &key).matches), vec![tok(1), tok(2)]);

        assert_eq!(
            edit(&mut conn, unlink(&key, &tok(2))),
            LeagueEditOutcome::Done
        );
        assert_eq!(
            match_columns(&conn, &tok(2)),
            MatchColumns {
                league_id: None,
                league_linked: false,
                no_league: true,
                hidden_at: None,
                edited_at: Some(9_000),
            }
        );
        assert_eq!(tokens(&stored(&conn, &key).matches), vec![tok(1)]);
        sync_leagues(&conn);
        assert_eq!(
            match_columns(&conn, &tok(2)).league_id,
            None,
            "no fold input any more"
        );
        assert_eq!(
            edit(&mut conn, unlink(&key, &tok(2))),
            LeagueEditOutcome::NotInLeague
        );
        assert_eq!(
            edit(&mut conn, unlink("manual:99", &tok(3))),
            LeagueEditOutcome::NotFound
        );
        assert_eq!(
            edit(&mut conn, unlink("manual:1", &tok(3))),
            LeagueEditOutcome::Done
        );
        assert!(members(&conn, 1).is_empty());
        assert!(match_columns(&conn, &tok(3)).no_league);

        // Moved back in, then out to no league.
        assert_eq!(
            edit(&mut conn, move_edit(&tok(2), Some(&key), 9_100)),
            LeagueEditOutcome::Done
        );
        assert_eq!(
            match_columns(&conn, &tok(2)),
            MatchColumns {
                league_id: Some(row_id(&conn, &key)),
                league_linked: true,
                no_league: false,
                hidden_at: None,
                edited_at: Some(9_100),
            }
        );
        assert_eq!(tokens(&stored(&conn, &key).matches), vec![tok(1), tok(2)]);
        assert_eq!(
            edit(&mut conn, move_edit(&tok(2), None, 9_200)),
            LeagueEditOutcome::Done
        );
        let out = match_columns(&conn, &tok(2));
        assert_eq!(
            (
                out.league_id,
                out.league_linked,
                out.no_league,
                out.edited_at
            ),
            (None, false, true, Some(9_200))
        );
        assert_eq!(
            edit(&mut conn, move_edit(&tok(4), None, 9_300)),
            LeagueEditOutcome::Tournament
        );
        assert_eq!(
            edit(&mut conn, move_edit(&tok(99), None, 9_300)),
            LeagueEditOutcome::MatchNotFound
        );
        assert_eq!(
            edit(&mut conn, move_edit(&tok(99), Some(&key), 9_300)),
            LeagueEditOutcome::MatchNotFound
        );
    }

    /// Review focus 3: the first match of a log run moved out of every
    /// league and back leaves the run's record exactly as it started: the
    /// same row, matches and results, one row per match and no detached
    /// duplicate.
    #[test]
    fn moving_a_match_out_and_back_restores_the_record() {
        let (_dir, mut conn) = edit_db("leagues-move-out-and-back");
        for (n, outcome) in [(1, "win"), (2, "loss"), (3, "win")] {
            insert(
                &conn,
                &Played {
                    outcome,
                    ..played(n, i64::from(n) * 10)
                },
            );
            link(&conn, &tok(n), 10);
        }
        sync_leagues(&conn);
        let key = format!("{LEAGUE}:{}", tok(1));
        let record = |run: &StoredRun| -> Vec<(String, String)> {
            run.matches
                .iter()
                .map(|row| (row.token.clone(), row.outcome.clone()))
                .collect()
        };
        let log_rows =
            |conn: &Connection| count(conn, "SELECT COUNT(*) FROM leagues WHERE source = 'log'");
        let before = stored(&conn, &key);
        assert_eq!(log_rows(&conn), 1);

        assert_eq!(
            edit(&mut conn, move_edit(&tok(1), None, T0 + 60 * MINUTE)),
            LeagueEditOutcome::Done
        );
        // The fold's key moves to the second match; the row goes with it.
        let rekeyed = format!("{LEAGUE}:{}", tok(2));
        let out = stored(&conn, &rekeyed);
        assert_eq!(out.id, before.id);
        assert_eq!(tokens(&out.matches), vec![tok(2), tok(3)]);

        assert_eq!(
            edit(
                &mut conn,
                move_edit(&tok(1), Some(&rekeyed), T0 + 61 * MINUTE)
            ),
            LeagueEditOutcome::Done
        );
        sync_leagues(&conn);
        let back = stored(&conn, &rekeyed);
        assert_eq!((back.id, back.state), (before.id, before.state));
        assert_eq!(record(&back), record(&before));
        assert_eq!(log_rows(&conn), 1, "one row: no duplicate");
        assert_eq!(
            count(
                &conn,
                "SELECT COUNT(*) FROM leagues WHERE detached_at IS NOT NULL"
            ),
            0,
            "nothing detached"
        );
        assert_eq!(
            count(
                &conn,
                &format!(
                    "SELECT COUNT(*) FROM matches WHERE league_id = {}",
                    before.id
                )
            ),
            3
        );
    }

    #[test]
    fn every_refusal_has_a_message_and_done_has_none() {
        use LeagueEditOutcome::*;
        assert_eq!(Done.refusal(), None);
        for outcome in [
            NotFound,
            NameRequired,
            NameTooLong,
            InvalidDeck,
            NoMatches,
            TooManyMatches,
            MatchNotFound,
            WrongDeck,
            NotInLeague,
            NotesTooLong,
            StillActive,
            LeagueRemoved,
            Tournament,
            NotFinished,
            WrongAccount,
            WrongKind,
            NothingToShow,
        ] {
            assert!(outcome.refusal().is_some(), "{outcome:?}");
        }
        assert_eq!(
            Tournament.refusal(),
            Some("Tournament matches stay in their tournament")
        );
        assert_eq!(
            TooManyMatches.refusal(),
            Some("League would exceed 5 matches.")
        );
    }

    // Limited runs (limited spec section 5)

    /// Qlocal (1) with 0.x runs: draft 29 (Active, HOB, course 35) holding
    /// the imported match `q-d` and its registered deck, sealed 30
    /// (Complete), constructed 42; 0.x draft `q-draft-a` linked to run 29
    /// and `q-draft-b` linked to none, last seen picking long ago.
    /// A 0.x league as the import seeds it, with only what the limited
    /// tests set.
    fn limited_seed(
        token: &str,
        event_id: i64,
        name: &str,
        format: &str,
        (state, started_at): (&str, i64),
        (kind, set_code, course_id): (&str, Option<&str>, Option<i64>),
    ) -> ImportedSeed {
        ImportedSeed {
            token: token.to_string(),
            event_id: Some(event_id),
            name: name.to_string(),
            format: Some(format.to_string()),
            state: state.to_string(),
            started_at,
            joined_at: None,
            completed_at: None,
            dropped_at: None,
            deck_version_id: None,
            kind: kind.to_string(),
            set_code: set_code.map(str::to_string),
            mtgo_course_id: course_id,
            legacy_id: None,
        }
    }

    fn limited_0x_db(label: &str) -> (TestDir, Connection) {
        let (dir, conn) = db(label);
        conn.execute_batch(
            "INSERT INTO accounts (id, username, first_seen_at) VALUES (1, 'Qlocal', 0);
             INSERT INTO matches (token, account_id, source, format, state, outcome, started_at,
                 last_activity_at, revision, derived_version)
             VALUES ('q-c', 1, 'imported', 'CMODERN', 'complete', 'win', 5, 5, 1, 0),
                    ('q-d', 1, 'imported', 'DHOBHOBHOB', 'complete', 'win', 6, 6, 1, 0);",
        )
        .expect("0.x matches");
        let runs = [
            (
                42,
                limited_seed(
                    "q-con",
                    1,
                    "Q League",
                    "CMODERN",
                    ("complete", 5),
                    ("constructed", None, None),
                ),
            ),
            (
                29,
                limited_seed(
                    "q-lim",
                    7001,
                    "Draft League 7001",
                    "Limited",
                    ("active", 6),
                    ("draft", Some("HOB"), Some(35)),
                ),
            ),
            (
                30,
                limited_seed(
                    "q-sea",
                    7002,
                    "FRAx6 League",
                    "S6FRA",
                    ("complete", 4),
                    ("sealed", Some("FRA"), None),
                ),
            ),
        ];
        for (id, seed) in &runs {
            insert_imported_run(&conn, *id, Some(1), seed);
        }
        link_imported_match(&conn, 42, "q-c");
        link_imported_match(&conn, 29, "q-d");
        conn.execute_batch(
            "INSERT INTO limited_deck_snapshots (match_id, source, cards, signature, captured_at)
             SELECT id, 'imported', '[{\"catalog_id\":1000,\"quantity\":1,\"sideboard\":false}]',
                 '', 7 FROM matches WHERE token = 'q-d';
             INSERT INTO drafts (id, draft_token, account_id, source, league_event_id,
                 mtgo_course_id, state, started_at, imported_from, imported_league_from)
             VALUES (1, 'q-draft-a', 1, 'imported', 7001, 35, 'finished', 3, 1, 29),
                    (2, 'q-draft-b', 1, 'imported', NULL, NULL, 'picking', 1000, 2, NULL);
             INSERT INTO draft_picks (draft_id, ordinal, pack_number, pick_number,
                 cards_available, picked_catalog_id, reservations)
             VALUES (1, 1, 1, 1, '[1000]', 1000, '[]');",
        )
        .expect("0.x limited data");
        (dir, conn)
    }

    /// The limited runs of `owner` as the pages read them, after a sync at
    /// `now_ms`.
    fn limited_at(conn: &Connection, owner: LeagueOwner, now_ms: i64) -> LimitedRuns {
        let tx = conn.unchecked_transaction().expect("transaction");
        crate::league_sync::sync_every_owner(&tx, chrono_tz::UTC, now_ms).expect("league sync");
        tx.commit().expect("commit");
        stored_limited_runs(
            conn,
            &RunFilter {
                owners: vec![owner],
                ..RunFilter::default()
            },
        )
        .expect("limited runs")
    }

    #[test]
    fn limited_runs_seed_0x_draft_and_sealed_runs_with_their_drafts() {
        let (_dir, conn) = limited_0x_db("leagues-limited-0x");
        // Within 30 minutes of its last activity it is still picking.
        let early = limited_at(&conn, LeagueOwner::Account(1), 1_000 + 29 * MINUTE);
        assert_eq!(early.unlinked_drafts[0].state, DraftState::Picking);
        let now = 1_000 + 31 * MINUTE;
        let found = limited_at(&conn, LeagueOwner::Account(1), now);
        let keys: Vec<&str> = found
            .runs
            .iter()
            .map(|limited| limited.run.key.as_str())
            .collect();
        assert_eq!(
            keys,
            vec!["0x:29", "0x:30"],
            "newest first, no constructed run"
        );

        let draft_run = &found.runs[0];
        assert_eq!(draft_run.run.kind, LeagueKind::Draft);
        assert_eq!(draft_run.run.state, RunState::Active);
        assert_eq!(draft_run.run.set_code.as_deref(), Some("HOB"));
        assert_eq!(draft_run.run.course_id, Some(35));
        assert_eq!(draft_run.run.event_id, Some(7001));
        let draft = draft_run.draft.as_ref().expect("its 0.x draft");
        assert_eq!(draft.token, "q-draft-a");
        assert_eq!(draft.state, DraftState::Finished);
        assert_eq!(tokens(&draft_run.matches), vec!["q-d".to_string()]);
        assert_eq!(draft_run.registered.len(), 1);
        assert_eq!(draft_run.registered[0].cards[0].catalog_id, 1000);
        assert_eq!(draft_run.run_input().matches.len(), 1);

        let sealed = &found.runs[1];
        assert_eq!(sealed.run.kind, LeagueKind::Sealed);
        assert_eq!(sealed.run.state, RunState::Complete);
        assert_eq!(sealed.draft, None);

        let unlinked: Vec<(&str, DraftState)> = found
            .unlinked_drafts
            .iter()
            .map(|draft| (draft.token.as_str(), draft.state))
            .collect();
        assert_eq!(unlinked, vec![("q-draft-b", DraftState::Abandoned)]);

        assert_eq!(
            limited_at(&conn, LeagueOwner::Nobody, now),
            LimitedRuns::default()
        );
    }

    #[test]
    fn live_limited_matches_fold_into_limited_runs_and_stay_off_the_constructed_reads() {
        let (_dir, conn) = db("leagues-limited-live");
        let local = account(&conn, "Qlocal");
        for n in 1..=3 {
            let id = insert(
                &conn,
                &Played {
                    format: "DHOBHOBHOB",
                    ..played(n, i64::from(n) * 30)
                },
            );
            game(&conn, id, 1, "win", true);
        }
        insert(
            &conn,
            &Played {
                league: Some(LEAGUE_2),
                ..played(4, 200)
            },
        );

        let found = limited_at(&conn, LeagueOwner::Account(local), T0);
        assert_eq!(found.runs.len(), 1);
        let run = &found.runs[0];
        assert_eq!(run.run.key, format!("{LEAGUE}:{}", tok(1)));
        assert_eq!(run.run.kind, LeagueKind::Draft);
        assert_eq!(run.run.set_code.as_deref(), Some("HOB"));
        assert_eq!(run.run.state, RunState::Complete);
        assert_eq!(tokens(&run.matches), vec![tok(1), tok(2), tok(3)]);
        assert_eq!(run.matches[0].games.len(), 1);
        assert!(found.unlinked_drafts.is_empty());
        let summary =
            leagues::summarize_limited_run(&run.run_input(), run.run.round_count(), chrono_tz::UTC);
        assert_eq!(summary.classification, leagues::Classification::Trophy);

        // The limited matches are inputs too; the fold makes them the draft run.
        let constructed = fold_inputs(&conn, LeagueOwner::Account(local))
            .expect("inputs")
            .constructed_runs(chrono_tz::UTC);
        assert_eq!(constructed.len(), 1);
        assert_eq!(constructed[0].match_tokens, vec![tok(4)]);
    }

    #[test]
    fn drops_and_notes_reach_limited_runs() {
        let (_dir, mut conn) = limited_0x_db("leagues-limited-edits");
        for n in 1..=2 {
            insert(
                &conn,
                &Played {
                    format: "S6FRA",
                    ..played(n, i64::from(n) * 30)
                },
            );
        }
        sync_leagues(&conn);
        let live_key = format!("{LEAGUE}:{}", tok(1));
        for run_key in ["0x:29", live_key.as_str()] {
            assert_eq!(
                edit(&mut conn, notes_edit(run_key, "Q note")),
                LeagueEditOutcome::Done,
                "{run_key}"
            );
            assert_eq!(note(&conn, run_key).as_deref(), Some("Q note"));
            assert_eq!(
                edit(&mut conn, drop_edit(run_key, 7_000)),
                LeagueEditOutcome::Done,
                "{run_key}"
            );
        }
        assert_eq!(
            edit(&mut conn, drop_edit("0x:30", 7_000)),
            LeagueEditOutcome::Done
        );
        for run_key in ["draft:q-nope", "draft:", &format!("{LEAGUE}:{}", tok(2))] {
            assert_eq!(
                edit(&mut conn, notes_edit(run_key, "Q note")),
                LeagueEditOutcome::NotFound,
                "{run_key}"
            );
        }

        let found = limited_at(&conn, LeagueOwner::Account(1), 0);
        for limited in &found.runs {
            let want = if limited.run.key == "0x:30" {
                RunState::Complete
            } else {
                RunState::Dropped
            };
            assert_eq!(limited.run.state, want, "{}", limited.run.key);
        }
        assert_eq!(found.runs.len(), 3);
    }

    #[test]
    fn a_changed_registered_deck_reads_at_once() {
        let (_dir, conn) = limited_0x_db("leagues-limited-registered-changed");
        let read = |conn: &Connection| {
            limited_at(conn, LeagueOwner::Account(1), 0).runs[0]
                .registered
                .clone()
        };
        let first = read(&conn);
        assert_eq!(first.len(), 1);
        assert_eq!(read(&conn), first, "a second read is the same");
        conn.execute(
            "UPDATE limited_deck_snapshots SET cards = ?1",
            ["[{\"catalog_id\":1000,\"quantity\":1,\"sideboard\":false},\
              {\"catalog_id\":1001,\"quantity\":2,\"sideboard\":true}]"],
        )
        .expect("a 0.x import rerun changed it");
        let changed = read(&conn);
        assert_eq!(changed[0].cards.len(), 2);
    }

    #[test]
    fn notes_and_drops_on_a_run_retired_by_adoption_reach_the_run_that_took_its_draft() {
        // 0.x run 31 holds the draft and no match; 0.x run 32, of the same
        // league with no course, holds the match its pool built.
        let (_dir, mut conn) = limited_0x_db("leagues-limited-adopted-keys");
        conn.execute_batch(
            "INSERT INTO matches (token, account_id, source, format, state, outcome, started_at,
                 last_activity_at, revision, derived_version)
             VALUES ('q-e', 1, 'imported', 'DHOBHOBHOB', 'complete', 'win', 500, 500, 1, 0);
             INSERT INTO drafts (id, draft_token, account_id, source, league_event_id,
                 mtgo_course_id, state, started_at, imported_from, imported_league_from)
             VALUES (3, 'q-draft-c', 1, 'imported', 7003, 36, 'finished', 100, 3, 31);
             WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 42)
             INSERT INTO draft_picks (draft_id, ordinal, pack_number, pick_number,
                 cards_available, picked_catalog_id, reservations)
             SELECT 3, i, 1, i, '[]', 1999 + i, '[]' FROM n;",
        )
        .expect("a split 0.x run");
        insert_imported_run(
            &conn,
            31,
            Some(1),
            &limited_seed(
                "q-lim-31",
                7003,
                "Draft League 7003",
                "Limited",
                ("active", 100),
                ("draft", None, Some(36)),
            ),
        );
        insert_imported_run(
            &conn,
            32,
            Some(1),
            &limited_seed(
                "q-lim-32",
                7003,
                "HOBx3 League",
                "DHOBHOBHOB",
                ("active", 400),
                ("draft", None, None),
            ),
        );
        link_imported_match(&conn, 32, "q-e");
        let deck: Vec<String> = (2000..=2022)
            .map(|id| format!("{{\"catalog_id\":{id},\"quantity\":1,\"sideboard\":false}}"))
            .collect();
        conn.execute(
            "INSERT INTO limited_deck_snapshots (match_id, source, cards, signature, captured_at)
             SELECT id, 'imported', ?1, '', 500 FROM matches WHERE token = 'q-e'",
            [format!("[{}]", deck.join(","))],
        )
        .expect("its registered deck");

        // Edits name the run by the key the player saw (an edit needs the
        // row, so they come before a sync retires it).
        assert_eq!(
            edit(&mut conn, notes_edit("0x:31", "Q note")),
            LeagueEditOutcome::Done
        );
        assert_eq!(
            edit(&mut conn, drop_edit("0x:31", 7_000)),
            LeagueEditOutcome::Done
        );
        let found = limited_at(&conn, LeagueOwner::Account(1), 0);
        let adopted = found
            .runs
            .iter()
            .find(|limited| limited.run.key == "0x:32")
            .expect("run 32");
        // The stored run has no former keys: the sync gives it the retired
        // run's notes and drop.
        assert_eq!(adopted.run.draft_token.as_deref(), Some("q-draft-c"));
        assert!(found.runs.iter().all(|limited| limited.run.key != "0x:31"));
        // The adopting run shows its match; the retired run held none (a run
        // with a match is never adopted), so no listed match is left on a
        // detached row.
        assert_eq!(tokens(&adopted.matches), vec!["q-e".to_string()]);
        let stranded: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM listed_matches m JOIN leagues l ON l.id = m.league_id
                 WHERE l.detached_at IS NOT NULL",
                [],
                |row| row.get(0),
            )
            .expect("count");
        assert_eq!(stranded, 0);
        assert_eq!(adopted.run.state, RunState::Dropped);
        assert_eq!(adopted.run.dropped_at, Some(7_000));
    }
}
