//! Decodes [`RawEntry`]s into the events the tracker reads. Every lookup is a
//! `serde_json::Value` lookup that gives up quietly: a missing field, a wrong
//! type or broken JSON means no event, never a panic.
//!
//! One log line can carry several GRE messages (match 1's connect line holds
//! the game 1 deck and the game 1 start; a game over line can hold two game
//! states), so [`decode_all`] returns every event of an entry, in log order.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::clock::timestamp_to_ms;
use crate::event_name::parse_event_name;
use crate::split::RawEntry;

/// An event read from `Player.log`. Serialised with a `kind` tag that is the
/// `arena_log_events.kind` value (see [`ArenaEvent::kind`]).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ArenaEvent {
    /// `DETAILED LOGS: ENABLED` (or another value when off).
    DetailedLogs { enabled: bool },
    /// `[Accounts - Login] Logged in successfully. Display Name: Name#12345`.
    Login { display_name: String },
    /// `authenticateResponse`: `clientId` is the local user id, and
    /// `screenName` the display name, with or without its `#` tag (trap 29).
    /// A fast login (a remembered token) writes no login line, so this is how
    /// such a session finds its account. Events stored before the screen name
    /// was read have none, and events stored before untagged names were kept
    /// have none for an untagged name.
    Authenticated {
        user_id: String,
        #[serde(default)]
        screen_name: Option<String>,
    },
    /// The `==> EventSetDeckV3` request: the deck chosen for an event.
    DeckSet {
        event_name: String,
        deck_id: String,
        deck_name: String,
        main: Vec<CardCount>,
        side: Vec<CardCount>,
        companion: Option<u32>,
        /// The `CommandZone` cards' grpIds, in order (Brawl: the commander;
        /// empty for any other deck). Events stored before this was read have
        /// none.
        #[serde(default)]
        commanders: Vec<u32>,
        /// The `Summary.Attributes` entry named `Format` (`HistoricBrawl`,
        /// `Historic`...). Events stored before this was read have none.
        #[serde(default)]
        format: Option<String>,
    },
    /// `matchGameRoomStateChangedEvent`: seats, and the final results once
    /// `completed`.
    RoomState {
        match_id: String,
        completed: bool,
        players: Vec<Seat>,
        results: Vec<ResultEntry>,
        at_ms: Option<i64>,
    },
    /// A game state's `gameInfo`. `results` is cumulative (trap 2). `at_ms`
    /// is the line's GRE time, 0 if the line has none (never seen).
    GameInfo {
        match_id: String,
        game_number: u32,
        over: bool,
        best_of_three: bool,
        /// The raw `matchWinCondition` (`MatchWinCondition_Best2of3` for
        /// Bo3, `MatchWinCondition_SingleElimination` for Bo1), `None` when
        /// absent. Events stored before it was read have none.
        #[serde(default)]
        win_condition: Option<String>,
        results: Vec<ResultEntry>,
        at_ms: i64,
    },
    /// A turn 1 `turnInfo` with an active player. `match_id` and
    /// `game_number` come from the same message's `gameInfo`; without one
    /// they are empty and 0 (the projector applies the match's current game).
    FirstTurn {
        match_id: String,
        game_number: u32,
        active_seat: u32,
    },
    /// A deck list as grpIds, one per copy: `connectResp.deckMessage` (the
    /// game 1 list, `game_deck: true`) or the client's `SubmitDeckResp`
    /// (after sideboarding, `game_deck: false`).
    DeckSubmitted {
        game_deck: bool,
        main: Vec<u32>,
        side: Vec<u32>,
        at_ms: Option<i64>,
    },
    /// The client's `MulliganResp`. Client messages name no seat, so `seat` is
    /// 0 for the local player unless the message has a `systemSeatId`.
    Mulligan {
        seat: u32,
        kept: bool,
        at_ms: Option<i64>,
    },
    /// A `RankGetCombinedRankInfo` response. It names no account (trap 6).
    Rank {
        constructed: RankReading,
        limited: RankReading,
    },
    /// A `RankGetSeasonAndRankDetails` response: the current season, its
    /// start and end as written (no zone).
    Season {
        ordinal: u32,
        start_text: String,
        end_text: String,
    },
    /// An `EventGetCoursesV2` response: the account's limited courses (event
    /// runs) as they stand. A snapshot, not a log event: the tracker never
    /// stores it in `arena_log_events`.
    Courses { courses: Vec<Course> },
}

impl ArenaEvent {
    /// The `arena_log_events.kind` value, also the serialised `kind` tag.
    pub fn kind(&self) -> &'static str {
        match self {
            ArenaEvent::DetailedLogs { .. } => "detailed_logs",
            ArenaEvent::Login { .. } => "login",
            ArenaEvent::Authenticated { .. } => "authenticated",
            ArenaEvent::DeckSet { .. } => "deck_set",
            ArenaEvent::RoomState { .. } => "room_state",
            ArenaEvent::GameInfo { .. } => "game_info",
            ArenaEvent::FirstTurn { .. } => "first_turn",
            ArenaEvent::DeckSubmitted { .. } => "deck_submitted",
            ArenaEvent::Mulligan { .. } => "mulligan",
            ArenaEvent::Rank { .. } => "rank",
            ArenaEvent::Season { .. } => "season",
            ArenaEvent::Courses { .. } => "courses",
        }
    }
}

/// One limited course (a draft or sealed run) of an `EventGetCoursesV2`
/// response. Missing win and loss counts are 0 (trap 4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Course {
    pub course_id: String,
    pub event_name: String,
    pub module: String,
    pub wins: u32,
    pub losses: u32,
    pub deck_id: String,
    pub deck_name: String,
    pub main: Vec<CardCount>,
    pub side: Vec<CardCount>,
    /// The card pool as Arena grpIds, one per copy.
    pub pool: Vec<u32>,
}

/// One deck line: a card (Arena grpId) and its copies.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CardCount {
    pub grp_id: u32,
    pub quantity: u32,
}

/// One reserved seat of a match room. Seats change per match (trap 1): find
/// the local one by `user_id`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Seat {
    pub seat: u32,
    pub team: u32,
    pub user_id: String,
    /// `playerName`: the display name without its `#` tag.
    pub name: String,
    /// The `InternalEventName`, for example `Traditional_Explorer_Ladder`.
    pub event_id: String,
}

/// What a result entry is about.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    Game,
    Match,
}

/// One entry of a result list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResultEntry {
    pub scope: Scope,
    /// `None` when the entry names no winner (a draw).
    pub winning_team: Option<u32>,
    /// As written, for example `ResultReason_Concede`.
    pub reason: String,
}

/// One ladder of a rank response. Arena omits fields at their default
/// (trap 4): `class` is then `"Bronze"` and numbers 0.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RankReading {
    pub season: u32,
    pub class: String,
    pub level: u32,
    pub step: u32,
    pub won: u32,
    pub lost: u32,
    pub mythic_percentile: Option<f64>,
    pub mythic_placement: Option<u32>,
}

/// The first event of an entry (see [`decode_all`] for entries with several).
/// Only the tests read one event.
#[cfg(test)]
fn decode(entry: &RawEntry) -> Option<ArenaEvent> {
    decode_all(entry).into_iter().next()
}

/// Every event of an entry, in log order. Empty for entries the tracker does
/// not read and for anything malformed.
pub fn decode_all(entry: &RawEntry) -> Vec<ArenaEvent> {
    let head = entry.head.as_str();
    if let Some(value) = head.strip_prefix("DETAILED LOGS:") {
        return vec![ArenaEvent::DetailedLogs {
            enabled: value.trim() == "ENABLED",
        }];
    }
    if head.starts_with("[Accounts - Login]") {
        return decode_login(head).into_iter().collect();
    }
    let Some(json) = &entry.json else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_str::<Value>(json) else {
        return Vec::new();
    };
    if let Some(name) = response_name(head) {
        let event = match name {
            "RankGetCombinedRankInfo" => decode_rank(&value),
            "RankGetSeasonAndRankDetails" => decode_season(&value),
            "EventGetCoursesV2" => decode_courses(&value),
            _ => None,
        };
        return event.into_iter().collect();
    }
    if let Some(name) = request_name(head) {
        let event = match name {
            "EventSetDeckV3" => decode_deck_set(&value),
            _ => None,
        };
        return event.into_iter().collect();
    }
    if let Some(response) = value.get("authenticateResponse") {
        return decode_authenticated(response).into_iter().collect();
    }
    if let Some(room) = value.get("matchGameRoomStateChangedEvent") {
        return decode_room_state(room, &value).into_iter().collect();
    }
    if let Some(gre) = value.get("greToClientEvent") {
        return decode_gre(gre, &value);
    }
    if let Some(payload) = value.get("payload") {
        return decode_client(payload, &value).into_iter().collect();
    }
    Vec::new()
}

/// `Name` from a head holding `<== Name(id)`.
fn response_name(head: &str) -> Option<&str> {
    let after = &head[head.find("<== ")? + 4..];
    let end = after.find('(').unwrap_or(after.len());
    Some(after[..end].trim())
}

/// `Name` from a head holding `==> Name`.
fn request_name(head: &str) -> Option<&str> {
    let after = &head[head.find("==> ")? + 4..];
    after.split_whitespace().next()
}

fn decode_login(head: &str) -> Option<ArenaEvent> {
    let marker = "Logged in successfully. Display Name:";
    let name = head[head.find(marker)? + marker.len()..].trim();
    // Arena names always carry a `#12345` tag; a name without one could
    // match an MTGO username and pull the match onto that account.
    if !name.contains('#') {
        return None;
    }
    Some(ArenaEvent::Login {
        display_name: name.to_string(),
    })
}

fn decode_authenticated(response: &Value) -> Option<ArenaEvent> {
    let user_id = text(response, "clientId")?;
    if user_id.is_empty() {
        return None;
    }
    // An untagged name is kept: the projector knows such an account by its
    // user id, so the name never matches an MTGO username.
    let screen_name = text(response, "screenName").filter(|name| !name.is_empty());
    Some(ArenaEvent::Authenticated {
        user_id,
        screen_name,
    })
}

fn decode_rank(value: &Value) -> Option<ArenaEvent> {
    let has_season = value.get("constructedSeasonOrdinal").is_some()
        || value.get("limitedSeasonOrdinal").is_some();
    if !has_season {
        return None;
    }
    Some(ArenaEvent::Rank {
        constructed: rank_reading(value, "constructed"),
        limited: rank_reading(value, "limited"),
    })
}

fn rank_reading(value: &Value, ladder: &str) -> RankReading {
    let number = |field: &str| number(value, &format!("{ladder}{field}")).unwrap_or(0);
    let placement = number("LeaderboardPlace");
    RankReading {
        season: number("SeasonOrdinal"),
        class: text(value, &format!("{ladder}Class")).unwrap_or_else(|| "Bronze".to_string()),
        level: number("Level"),
        step: number("Step"),
        won: number("MatchesWon"),
        lost: number("MatchesLost"),
        mythic_percentile: value
            .get(format!("{ladder}Percentile"))
            .and_then(Value::as_f64),
        mythic_placement: (placement > 0).then_some(placement),
    }
}

fn decode_season(value: &Value) -> Option<ArenaEvent> {
    let season = value.get("currentSeason")?;
    Some(ArenaEvent::Season {
        ordinal: number(season, "seasonOrdinal")?,
        start_text: text(season, "seasonStartTime").unwrap_or_default(),
        end_text: text(season, "seasonEndTime").unwrap_or_default(),
    })
}

/// `==> EventSetDeckV3 {"id":..,"request":"<json text>"}`: the request is JSON
/// inside a string.
fn decode_deck_set(value: &Value) -> Option<ArenaEvent> {
    let request: Value = serde_json::from_str(value.get("request")?.as_str()?).ok()?;
    let summary = request.get("Summary")?;
    let deck = request.get("Deck")?;
    let deck_id = text(summary, "DeckId")?;
    let event_name = text(&request, "EventName")?;
    if deck_id.is_empty() || event_name.is_empty() {
        return None;
    }
    let companion = deck
        .get("Companions")
        .and_then(|companions| companions.get(0))
        .and_then(|card| number(card, "cardId"));
    Some(ArenaEvent::DeckSet {
        event_name,
        deck_id,
        deck_name: text(summary, "Name").unwrap_or_default(),
        main: card_counts(deck.get("MainDeck")),
        side: card_counts(deck.get("Sideboard")),
        companion,
        commanders: card_counts(deck.get("CommandZone"))
            .into_iter()
            .map(|card| card.grp_id)
            .collect(),
        format: deck_format(summary),
    })
}

/// The `Summary.Attributes` entry named `Format`: a list of
/// `{"name":..,"value":..}` pairs.
fn deck_format(summary: &Value) -> Option<String> {
    let attributes = summary.get("Attributes")?.as_array()?;
    attributes
        .iter()
        .find(|attribute| text(attribute, "name").as_deref() == Some("Format"))
        .and_then(|attribute| text(attribute, "value"))
}

/// `[{"cardId":..,"quantity":..}]`; lines missing either are skipped.
fn card_counts(list: Option<&Value>) -> Vec<CardCount> {
    let mut counts = Vec::new();
    for item in list.and_then(Value::as_array).into_iter().flatten() {
        if let (Some(grp_id), Some(quantity)) = (number(item, "cardId"), number(item, "quantity")) {
            counts.push(CardCount { grp_id, quantity });
        }
    }
    counts
}

fn decode_courses(value: &Value) -> Option<ArenaEvent> {
    let list = value.get("Courses")?.as_array()?;
    let courses = list.iter().filter_map(decode_course).collect();
    Some(ArenaEvent::Courses { courses })
}

/// A limited course; `None` for any other event or a course without an id.
fn decode_course(course: &Value) -> Option<Course> {
    let event_name = text(course, "InternalEventName")?;
    if !parse_event_name(&event_name)?.limited {
        return None;
    }
    let summary = course.get("CourseDeckSummary");
    let deck = course.get("CourseDeck");
    let pool = course.get("CardPool").and_then(Value::as_array);
    Some(Course {
        course_id: text(course, "CourseId")?,
        event_name,
        module: text(course, "CurrentModule").unwrap_or_default(),
        wins: number(course, "CurrentWins").unwrap_or(0),
        losses: number(course, "CurrentLosses").unwrap_or(0),
        deck_id: summary
            .and_then(|summary| text(summary, "DeckId"))
            .unwrap_or_default(),
        deck_name: summary
            .and_then(|summary| text(summary, "Name"))
            .unwrap_or_default(),
        main: card_counts(deck.and_then(|deck| deck.get("MainDeck"))),
        side: card_counts(deck.and_then(|deck| deck.get("Sideboard"))),
        pool: pool
            .into_iter()
            .flatten()
            .filter_map(|id| u32::try_from(id.as_u64()?).ok())
            .collect(),
    })
}

fn decode_room_state(room: &Value, line: &Value) -> Option<ArenaEvent> {
    let info = room.get("gameRoomInfo")?;
    let config = info.get("gameRoomConfig");
    let final_result = info.get("finalMatchResult");
    let match_id = config
        .and_then(|config| text(config, "matchId"))
        .or_else(|| final_result.and_then(|result| text(result, "matchId")))?;
    let mut players = Vec::new();
    let seats = config
        .and_then(|config| config.get("reservedPlayers"))
        .or_else(|| info.get("players"));
    for player in seats.and_then(Value::as_array).into_iter().flatten() {
        if let Some(seat) = read_seat(player) {
            players.push(seat);
        }
    }
    let results = results(final_result.and_then(|result| result.get("resultList")));
    Some(ArenaEvent::RoomState {
        match_id,
        completed: text(info, "stateType").as_deref()
            == Some("MatchGameRoomStateType_MatchCompleted"),
        players,
        results,
        at_ms: line_time(line),
    })
}

fn read_seat(player: &Value) -> Option<Seat> {
    Some(Seat {
        seat: number(player, "systemSeatId")?,
        team: number(player, "teamId")?,
        user_id: text(player, "userId")?,
        name: text(player, "playerName").unwrap_or_default(),
        event_id: text(player, "eventId").unwrap_or_default(),
    })
}

/// A result list; entries with an unknown scope are skipped.
fn results(list: Option<&Value>) -> Vec<ResultEntry> {
    let mut entries = Vec::new();
    for item in list.and_then(Value::as_array).into_iter().flatten() {
        let scope = match text(item, "scope").as_deref() {
            Some("MatchScope_Game") => Scope::Game,
            Some("MatchScope_Match") => Scope::Match,
            _ => continue,
        };
        entries.push(ResultEntry {
            scope,
            winning_team: number(item, "winningTeamId"),
            reason: text(item, "reason").unwrap_or_default(),
        });
    }
    entries
}

/// Every event of a `greToClientEvent` line. A turn 1 `turnInfo` repeats in
/// every diff of turn 1, so one line gives at most one `FirstTurn`.
fn decode_gre(gre: &Value, line: &Value) -> Vec<ArenaEvent> {
    let mut events = Vec::new();
    let at_ms = line_time(line);
    let mut first_turn_seen = false;
    let messages = gre.get("greToClientMessages").and_then(Value::as_array);
    for message in messages.into_iter().flatten() {
        if let Some(deck) = message
            .get("connectResp")
            .and_then(|connect| connect.get("deckMessage"))
        {
            events.push(ArenaEvent::DeckSubmitted {
                game_deck: true,
                main: grp_ids(deck.get("deckCards")),
                side: grp_ids(deck.get("sideboardCards")),
                at_ms,
            });
        }
        let Some(state) = message.get("gameStateMessage") else {
            continue;
        };
        let game_info = state.get("gameInfo").filter(|info| info.is_object());
        if let Some(info) = game_info {
            events.push(game_info_event(info, at_ms.unwrap_or(0)));
        }
        if !first_turn_seen && let Some(active_seat) = first_turn_active_seat(state) {
            first_turn_seen = true;
            events.push(ArenaEvent::FirstTurn {
                match_id: game_info
                    .and_then(|info| text(info, "matchID"))
                    .unwrap_or_default(),
                game_number: game_info
                    .and_then(|info| number(info, "gameNumber"))
                    .unwrap_or(0),
                active_seat,
            });
        }
    }
    events
}

fn game_info_event(info: &Value, at_ms: i64) -> ArenaEvent {
    let win_condition = text(info, "matchWinCondition");
    ArenaEvent::GameInfo {
        match_id: text(info, "matchID").unwrap_or_default(),
        game_number: number(info, "gameNumber").unwrap_or(0),
        over: text(info, "stage").as_deref() == Some("GameStage_GameOver"),
        best_of_three: win_condition.as_deref() == Some("MatchWinCondition_Best2of3"),
        win_condition,
        results: results(info.get("results")),
        at_ms,
    }
}

/// The active seat of a turn 1 `turnInfo`.
fn first_turn_active_seat(state: &Value) -> Option<u32> {
    let turn = state.get("turnInfo")?;
    if number(turn, "turnNumber")? != 1 {
        return None;
    }
    number(turn, "activePlayer")
}

/// A client to server message, by its `payload.type`.
fn decode_client(payload: &Value, line: &Value) -> Option<ArenaEvent> {
    let at_ms = line_time(line);
    match text(payload, "type")?.as_str() {
        "ClientMessageType_SubmitDeckResp" => {
            let deck = payload.get("submitDeckResp")?.get("deck")?;
            Some(ArenaEvent::DeckSubmitted {
                game_deck: false,
                main: grp_ids(deck.get("deckCards")),
                side: grp_ids(deck.get("sideboardCards")),
                at_ms,
            })
        }
        "ClientMessageType_MulliganResp" => {
            let decision = text(payload.get("mulliganResp")?, "decision")?;
            Some(ArenaEvent::Mulligan {
                seat: number(payload, "systemSeatId").unwrap_or(0),
                kept: decision == "MulliganOption_AcceptHand",
                at_ms,
            })
        }
        _ => None,
    }
}

/// A list of grpIds; anything that is not one is skipped.
fn grp_ids(list: Option<&Value>) -> Vec<u32> {
    let mut ids = Vec::new();
    for item in list.and_then(Value::as_array).into_iter().flatten() {
        if let Some(id) = item.as_u64().and_then(|id| u32::try_from(id).ok()) {
            ids.push(id);
        }
    }
    ids
}

/// The line's `timestamp` (unix ms or .NET ticks, as a string or a number)
/// in unix ms.
fn line_time(line: &Value) -> Option<i64> {
    let value = line.get("timestamp")?;
    let raw = match value {
        Value::String(text) => text.parse().ok()?,
        Value::Number(number) => number.as_i64()?,
        _ => return None,
    };
    Some(timestamp_to_ms(raw))
}

fn text(value: &Value, key: &str) -> Option<String> {
    value.get(key)?.as_str().map(str::to_string)
}

fn number(value: &Value, key: &str) -> Option<u32> {
    u32::try_from(value.get(key)?.as_u64()?).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::split::{RawEntry, split_complete};

    fn fixture_entries() -> Vec<RawEntry> {
        split_complete(&fixture_tools::arena_fixture("bo3-loss-and-match-concede").bytes)
    }

    fn fixture_events() -> Vec<ArenaEvent> {
        fixture_entries().iter().flat_map(decode_all).collect()
    }

    fn entry(head: &str, json: &str) -> RawEntry {
        RawEntry {
            offset: 0,
            len: 0,
            head: head.to_string(),
            json: Some(json.to_string()),
        }
    }

    fn room_states() -> Vec<(String, bool, Vec<Seat>, Vec<ResultEntry>)> {
        fixture_events()
            .into_iter()
            .filter_map(|event| match event {
                ArenaEvent::RoomState {
                    match_id,
                    completed,
                    players,
                    results,
                    ..
                } => Some((match_id, completed, players, results)),
                _ => None,
            })
            .collect()
    }

    fn summary(results: &[ResultEntry]) -> Vec<(Scope, Option<u32>)> {
        results
            .iter()
            .map(|result| (result.scope.clone(), result.winning_team))
            .collect()
    }

    #[test]
    fn decode_reads_the_login() {
        let logins: Vec<ArenaEvent> = fixture_events()
            .into_iter()
            .filter(|event| matches!(event, ArenaEvent::Login { .. }))
            .collect();
        assert_eq!(
            logins,
            vec![ArenaEvent::Login {
                display_name: "Player#00001".to_string()
            }]
        );
    }

    #[test]
    fn a_login_without_a_tag_is_no_login() {
        let login = |name: &str| RawEntry {
            offset: 0,
            len: 0,
            head: format!("[Accounts - Login] Logged in successfully. Display Name: {name}"),
            json: None,
        };
        assert_eq!(decode(&login("Qlocal")), None);
        assert_eq!(
            decode(&login("Qlocal#12345")),
            Some(ArenaEvent::Login {
                display_name: "Qlocal#12345".to_string()
            })
        );
    }

    #[test]
    fn decode_reads_detailed_logs() {
        let events = fixture_events();
        assert_eq!(events[0], ArenaEvent::DetailedLogs { enabled: true });
        let off = RawEntry {
            offset: 0,
            len: 0,
            head: "DETAILED LOGS: DISABLED".to_string(),
            json: None,
        };
        assert_eq!(
            decode(&off),
            Some(ArenaEvent::DetailedLogs { enabled: false })
        );
    }

    #[test]
    fn decode_reads_the_authenticated_user() {
        let users: Vec<ArenaEvent> = fixture_events()
            .into_iter()
            .filter(|event| matches!(event, ArenaEvent::Authenticated { .. }))
            .collect();
        // The Bo3 fixture's screen name has no `#` tag (trap 29).
        let expected = ArenaEvent::Authenticated {
            user_id: "PLAYER0000000000000000000A".to_string(),
            screen_name: Some("Player".to_string()),
        };
        assert_eq!(users, vec![expected.clone(), expected]);
    }

    #[test]
    fn decode_reads_room_state_seats() {
        let (match_id, completed, players, results) = room_states().remove(0);
        assert_eq!(match_id, "bf62b620-6924-4a6b-901a-275c36778f67");
        assert!(!completed);
        assert!(results.is_empty());
        assert_eq!(
            players,
            vec![
                Seat {
                    seat: 1,
                    team: 1,
                    user_id: "PLAYER0000000000000000000A".to_string(),
                    name: "Player".to_string(),
                    event_id: "Traditional_Explorer_Ladder".to_string(),
                },
                Seat {
                    seat: 2,
                    team: 2,
                    user_id: "OPPONENT00000000000000000B".to_string(),
                    name: "Opponent".to_string(),
                    event_id: "Traditional_Explorer_Ladder".to_string(),
                },
            ]
        );
    }

    #[test]
    fn decode_reads_seat_two_in_match_two() {
        let states = room_states();
        assert_eq!(states.len(), 4);
        let (match_id, _, players, _) = &states[2];
        assert_eq!(match_id, "85d265dc-f113-429f-a8c4-89e89c258b66");
        let local = players
            .iter()
            .find(|seat| seat.user_id == "PLAYER0000000000000000000A")
            .unwrap();
        assert_eq!((local.seat, local.team), (2, 2));
    }

    #[test]
    fn decode_reads_cumulative_results() {
        let (_, completed, _, results) = room_states().remove(1);
        assert!(completed);
        assert_eq!(
            summary(&results),
            vec![
                (Scope::Game, Some(2)),
                (Scope::Game, Some(1)),
                (Scope::Game, Some(2)),
                (Scope::Match, Some(2)),
            ]
        );
        assert_eq!(results[0].reason, "ResultReason_Concede");
    }

    #[test]
    fn decode_reads_match_concede_scope() {
        let (_, completed, _, results) = room_states().remove(3);
        assert!(completed);
        assert_eq!(
            summary(&results),
            vec![(Scope::Game, Some(1)), (Scope::Match, Some(1))]
        );
    }

    #[test]
    fn decode_fills_rank_defaults() {
        let response = entry(
            "<== RankGetCombinedRankInfo(x)",
            r#"{"constructedSeasonOrdinal":95,"constructedLevel":4,"limitedSeasonOrdinal":95,"limitedLevel":4}"#,
        );
        let bronze = RankReading {
            season: 95,
            class: "Bronze".to_string(),
            level: 4,
            step: 0,
            won: 0,
            lost: 0,
            mythic_percentile: None,
            mythic_placement: None,
        };
        assert_eq!(
            decode(&response),
            Some(ArenaEvent::Rank {
                constructed: bronze.clone(),
                limited: bronze
            })
        );
    }

    #[test]
    fn decode_reads_every_rank_field() {
        let response = entry(
            "[UnityCrossThreadLogger]03/11/2026 12:28:25\n<== RankGetCombinedRankInfo(x)",
            r#"{"constructedSeasonOrdinal":95,"constructedClass":"Mythic","constructedLevel":1,"constructedStep":2,"constructedMatchesWon":7,"constructedMatchesLost":3,"constructedPercentile":91.5,"constructedLeaderboardPlace":120,"limitedSeasonOrdinal":95,"limitedClass":"Gold","limitedLevel":2}"#,
        );
        let Some(ArenaEvent::Rank {
            constructed,
            limited,
        }) = decode(&response)
        else {
            panic!("not a rank");
        };
        assert_eq!(
            constructed,
            RankReading {
                season: 95,
                class: "Mythic".to_string(),
                level: 1,
                step: 2,
                won: 7,
                lost: 3,
                mythic_percentile: Some(91.5),
                mythic_placement: Some(120),
            }
        );
        assert_eq!(limited.class, "Gold");
    }

    #[test]
    fn decode_keeps_stale_rank_responses() {
        // Trap 5: the projector drops them, the decoder reports what was said.
        let seasons: Vec<u32> = fixture_events()
            .into_iter()
            .filter_map(|event| match event {
                ArenaEvent::Rank { constructed, .. } => Some(constructed.season),
                _ => None,
            })
            .collect();
        assert_eq!(seasons, vec![89, 95, 95, 95]);
    }

    #[test]
    fn decode_reads_the_season() {
        let season = fixture_events()
            .into_iter()
            .find(|event| matches!(event, ArenaEvent::Season { .. }));
        assert_eq!(
            season,
            Some(ArenaEvent::Season {
                ordinal: 95,
                start_text: "2026-10-31T19:05:00".to_string(),
                end_text: "2026-11-30T20:00:00".to_string(),
            })
        );
    }

    #[test]
    fn decode_reads_deck_set() {
        let Some(ArenaEvent::DeckSet {
            event_name,
            deck_id,
            deck_name,
            main,
            side,
            companion,
            ..
        }) = fixture_events()
            .into_iter()
            .find(|event| matches!(event, ArenaEvent::DeckSet { .. }))
        else {
            panic!("no deck set");
        };
        assert_eq!(event_name, "Traditional_Explorer_Ladder");
        assert_eq!(deck_id, "7a208116-3d8d-4cff-92b7-fe6747707f59");
        assert_eq!(deck_name, "AZ Control");
        assert_eq!(main.len(), 27);
        assert_eq!(main.iter().map(|card| card.quantity).sum::<u32>(), 61);
        assert_eq!(
            main[0],
            CardCount {
                grp_id: 91538,
                quantity: 2
            }
        );
        assert_eq!(side.iter().map(|card| card.quantity).sum::<u32>(), 15);
        assert_eq!(companion, Some(71291));
    }

    fn brawl_deck_sets(fixture: &str) -> Vec<ArenaEvent> {
        let bytes = fixture_tools::arena_fixture(fixture).bytes;
        split_complete(&bytes)
            .iter()
            .flat_map(decode_all)
            .filter(|event| matches!(event, ArenaEvent::DeckSet { .. }))
            .collect()
    }

    #[test]
    fn decode_reads_the_commander_and_format_of_a_brawl_deck() {
        let sets = brawl_deck_sets("brawl/brawl_queue");
        let Some(ArenaEvent::DeckSet {
            event_name,
            main,
            commanders,
            format,
            ..
        }) = sets.first()
        else {
            panic!("no deck set");
        };
        assert_eq!(event_name, "Play_Brawl_Historic");
        assert_eq!(commanders, &vec![83902]);
        assert_eq!(format.as_deref(), Some("HistoricBrawl"));
        assert_eq!(main.iter().map(|card| card.quantity).sum::<u32>(), 99);
    }

    #[test]
    fn decode_reads_the_ladder_brawl_deck() {
        let sets = brawl_deck_sets("brawl/brawl_ladder");
        let Some(ArenaEvent::DeckSet {
            event_name,
            commanders,
            format,
            ..
        }) = sets.first()
        else {
            panic!("no deck set");
        };
        assert_eq!(event_name, "Brawl_Ladder");
        assert_eq!(commanders.len(), 1);
        assert!(format.is_some());
    }

    #[test]
    fn a_non_brawl_deck_has_no_commanders() {
        let commanders: Vec<Vec<u32>> = fixture_events()
            .into_iter()
            .filter_map(|event| match event {
                ArenaEvent::DeckSet { commanders, .. } => Some(commanders),
                _ => None,
            })
            .collect();
        assert!(!commanders.is_empty());
        assert!(commanders.iter().all(Vec::is_empty));
    }

    #[test]
    fn an_old_deck_set_without_the_new_keys_still_deserializes() {
        let old = r#"{"kind":"deck_set","event_name":"Ladder","deck_id":"d","deck_name":"n","main":[],"side":[],"companion":null}"#;
        let event: ArenaEvent = serde_json::from_str(old).expect("old deck set");
        let ArenaEvent::DeckSet {
            commanders, format, ..
        } = event
        else {
            panic!("not a deck set");
        };
        assert!(commanders.is_empty());
        assert_eq!(format, None);
    }

    #[test]
    fn decode_reads_a_deck_set_without_companion() {
        let sets: Vec<Option<u32>> = fixture_events()
            .into_iter()
            .filter_map(|event| match event {
                ArenaEvent::DeckSet { companion, .. } => Some(companion),
                _ => None,
            })
            .collect();
        assert_eq!(sets, vec![Some(71291), None]);
    }

    #[test]
    fn decode_reads_best_of_three() {
        let message = |condition: &str| {
            entry(
                "",
                &format!(
                    r#"{{ "timestamp": "1793709133068", "greToClientEvent": {{ "greToClientMessages": [ {{ "type": "GREMessageType_GameStateMessage", "gameStateMessage": {{ "gameInfo": {{ "matchID": "m", "gameNumber": 1, "stage": "GameStage_Start", "matchWinCondition": "{condition}" }} }} }} ] }} }}"#
                ),
            )
        };
        let best_of_three = |condition: &str| match decode(&message(condition)) {
            Some(ArenaEvent::GameInfo { best_of_three, .. }) => best_of_three,
            other => panic!("not game info: {other:?}"),
        };
        assert!(best_of_three("MatchWinCondition_Best2of3"));
        assert!(!best_of_three("MatchWinCondition_SingleElimination"));
        for event in fixture_events() {
            if let ArenaEvent::GameInfo { best_of_three, .. } = event {
                assert!(best_of_three);
            }
        }
    }

    #[test]
    fn decode_reads_the_win_condition() {
        let message = |info: &str| {
            entry(
                "",
                &format!(
                    r#"{{ "timestamp": "1793709133068", "greToClientEvent": {{ "greToClientMessages": [ {{ "type": "GREMessageType_GameStateMessage", "gameStateMessage": {{ "gameInfo": {{ "matchID": "m", "gameNumber": 1, "stage": "GameStage_Start"{info} }} }} }} ] }} }}"#
                ),
            )
        };
        let read = |info: &str| match decode(&message(info)) {
            Some(ArenaEvent::GameInfo {
                win_condition,
                best_of_three,
                ..
            }) => (win_condition, best_of_three),
            other => panic!("not game info: {other:?}"),
        };
        assert_eq!(
            read(r#", "matchWinCondition": "MatchWinCondition_Best2of3""#),
            (Some("MatchWinCondition_Best2of3".to_string()), true)
        );
        assert_eq!(
            read(r#", "matchWinCondition": "MatchWinCondition_SingleElimination""#),
            (
                Some("MatchWinCondition_SingleElimination".to_string()),
                false
            )
        );
        assert_eq!(read(""), (None, false));
    }

    #[test]
    fn authenticated_reads_the_screen_name() {
        let authenticated = |response: &str| {
            decode(&entry(
                "",
                &format!("{{\"authenticateResponse\":{{\"clientId\":\"USER_A\"{response}}}}}"),
            ))
        };
        let with = |screen_name: Option<&str>| {
            Some(ArenaEvent::Authenticated {
                user_id: "USER_A".to_string(),
                screen_name: screen_name.map(str::to_string),
            })
        };
        assert_eq!(
            authenticated(",\"screenName\":\"Player#00001\""),
            with(Some("Player#00001"))
        );
        // Trap 29: some sessions only ever log the name without its tag.
        assert_eq!(
            authenticated(",\"screenName\":\"Player\""),
            with(Some("Player"))
        );
        assert_eq!(authenticated(",\"screenName\":\"\""), with(None));
        assert_eq!(authenticated(""), with(None));
    }

    #[test]
    fn an_authenticated_event_stored_without_screen_name_still_reads() {
        let stored = r#"{"kind":"authenticated","user_id":"USER_A"}"#;
        let event: ArenaEvent = serde_json::from_str(stored).expect("an old stored event reads");
        assert_eq!(
            event,
            ArenaEvent::Authenticated {
                user_id: "USER_A".to_string(),
                screen_name: None,
            }
        );
    }

    #[test]
    fn an_event_stored_without_win_condition_still_reads() {
        let stored = r#"{"kind":"game_info","match_id":"m","game_number":2,"over":false,"best_of_three":true,"results":[],"at_ms":5}"#;
        let event: ArenaEvent = serde_json::from_str(stored).expect("an old stored event reads");
        assert_eq!(
            event,
            ArenaEvent::GameInfo {
                match_id: "m".to_string(),
                game_number: 2,
                over: false,
                best_of_three: true,
                win_condition: None,
                results: Vec::new(),
                at_ms: 5,
            }
        );
    }

    /// Match id prefix, game number, results summary, time.
    type GameOver = (String, u32, Vec<(Scope, Option<u32>)>, i64);

    #[test]
    fn decode_reads_game_over_results_from_diffs() {
        let over: Vec<GameOver> = fixture_events()
            .into_iter()
            .filter_map(|event| match event {
                ArenaEvent::GameInfo {
                    match_id,
                    game_number,
                    over: true,
                    results,
                    at_ms,
                    ..
                } => Some((
                    match_id[..8].to_string(),
                    game_number,
                    summary(&results),
                    at_ms,
                )),
                _ => None,
            })
            .collect();
        let game = |team| (Scope::Game, Some(team));
        let whole = |team| (Scope::Match, Some(team));
        assert_eq!(over.len(), 6);
        assert_eq!(over[0].0, "bf62b620");
        assert_eq!((over[0].1, over[0].2.clone()), (1, vec![game(2)]));
        assert_eq!((over[1].1, over[1].2.clone()), (2, vec![game(2), game(1)]));
        assert_eq!(over[2].2, vec![game(2), game(1), game(2)]);
        assert_eq!(over[3].2, vec![game(2), game(1), game(2), whole(2)]);
        assert_eq!(over[4].0, "85d265dc");
        assert_eq!(over[5].2, vec![game(1), whole(1)]);
        assert!(
            over.iter()
                .all(|(_, _, _, at_ms)| *at_ms > 1_793_700_000_000)
        );
    }

    #[test]
    fn decode_reads_first_turn_with_its_game() {
        let turns: Vec<(u32, u32, bool)> = fixture_events()
            .into_iter()
            .filter_map(|event| match event {
                ArenaEvent::FirstTurn {
                    match_id,
                    game_number,
                    active_seat,
                } => Some((game_number, active_seat, match_id.is_empty())),
                _ => None,
            })
            .collect();
        // The message carrying gameInfo names its game; later turn 1 diffs on
        // other lines carry no gameInfo, so game 0 and no match id.
        assert_eq!(
            turns,
            vec![
                (1, 2, false),
                (0, 2, true),
                (2, 1, false),
                (0, 1, true),
                (3, 2, false),
                (0, 2, true),
            ]
        );
    }

    #[test]
    fn decode_reads_deck_submissions() {
        let decks: Vec<(bool, usize, usize, bool)> = fixture_events()
            .into_iter()
            .filter_map(|event| match event {
                ArenaEvent::DeckSubmitted {
                    game_deck,
                    main,
                    side,
                    at_ms,
                } => Some((game_deck, main.len(), side.len(), at_ms.is_some())),
                _ => None,
            })
            .collect();
        assert_eq!(
            decks,
            vec![
                (true, 61, 15, true),
                (false, 61, 15, true),
                (false, 61, 15, true),
                // Match 2 was conceded at its sideboarding prompt: no
                // SubmitDeckResp.
                (true, 60, 15, true),
            ]
        );
    }

    #[test]
    fn decode_reads_mulligans() {
        let mulligans: Vec<(u32, bool)> = fixture_events()
            .into_iter()
            .filter_map(|event| match event {
                ArenaEvent::Mulligan { seat, kept, at_ms } => {
                    assert!(at_ms.is_some());
                    Some((seat, kept))
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            mulligans,
            vec![(0, false), (0, false), (0, true), (0, true), (0, true)]
        );
    }

    #[test]
    fn decode_all_reads_every_message_of_a_line() {
        // Match 1's connect line carries the game 1 deck and the game 1 start.
        let entries = fixture_entries();
        let connect = entries
            .iter()
            .find(|entry| {
                entry
                    .json
                    .as_deref()
                    .is_some_and(|json| json.contains("\"connectResp\""))
            })
            .unwrap();
        let events = decode_all(connect);
        assert_eq!(events.len(), 2);
        assert!(matches!(
            events[0],
            ArenaEvent::DeckSubmitted {
                game_deck: true,
                ..
            }
        ));
        assert!(matches!(
            events[1],
            ArenaEvent::GameInfo {
                game_number: 1,
                over: false,
                ..
            }
        ));
        assert_eq!(decode(connect), Some(events[0].clone()));
    }

    #[test]
    fn decode_skips_garbage() {
        let cases = [
            entry(
                "<== RankGetCombinedRankInfo(x)",
                "{\"constructedSeasonOrdinal\":9",
            ),
            entry("<== RankGetCombinedRankInfo(x)", "[]"),
            entry("<== RankGetCombinedRankInfo(x)", "{}"),
            entry(
                "<== RankGetSeasonAndRankDetails(x)",
                "{\"currentSeason\":7}",
            ),
            entry("<== SomethingNew(x)", "{\"a\":1}"),
            entry(
                "[UnityCrossThreadLogger]==> EventSetDeckV3",
                "{\"request\":\"{bad\"}",
            ),
            entry(
                "[UnityCrossThreadLogger]==> EventSetDeckV3",
                "{\"request\":7}",
            ),
            entry(
                "",
                "{\"greToClientEvent\":{\"greToClientMessages\":[{\"gameStateMessage\":{\"gameInfo\":\"x\"}}]}}",
            ),
            entry("", "{\"greToClientEvent\":{\"greToClientMessages\":7}}"),
            entry(
                "",
                "{\"matchGameRoomStateChangedEvent\":{\"gameRoomInfo\":[]}}",
            ),
            entry(
                "",
                "{\"payload\":{\"type\":\"ClientMessageType_MulliganResp\"}}",
            ),
            entry("", "{\"authenticateResponse\":{\"clientId\":5}}"),
            entry("", "not json"),
            entry(
                "[Accounts - Login] Logged in successfully. Display Name: ",
                "",
            ),
        ];
        for case in &cases {
            assert_eq!(decode_all(case), Vec::new(), "{case:?}");
        }
        let login = RawEntry {
            offset: 0,
            len: 0,
            head: "[Accounts - Login] Attempting Login with credentials.".to_string(),
            json: None,
        };
        assert_eq!(decode(&login), None);
    }

    #[test]
    fn decode_never_panics_on_cut_fixture_entries() {
        for entry in fixture_entries() {
            if let Some(json) = &entry.json {
                for cut in [1, json.len() / 2, json.len().saturating_sub(1)] {
                    let mut cut_entry = entry.clone();
                    let mut end = cut.min(json.len());
                    while !json.is_char_boundary(end) {
                        end -= 1;
                    }
                    cut_entry.json = Some(json[..end].to_string());
                    let _ = decode_all(&cut_entry);
                }
            }
        }
    }

    #[test]
    fn arena_event_round_trips_through_json() {
        let events = fixture_events();
        assert!(events.len() > 40);
        for event in events {
            let text = serde_json::to_string(&event).unwrap();
            let back: ArenaEvent = serde_json::from_str(&text).unwrap();
            assert_eq!(back, event);
        }
    }

    #[test]
    fn event_kinds_are_the_store_kinds() {
        // `courses` is a snapshot the tracker never stores.
        let kinds: std::collections::BTreeSet<&str> = fixture_events()
            .iter()
            .map(ArenaEvent::kind)
            .filter(|kind| *kind != "courses")
            .collect();
        let expected = [
            "authenticated",
            "deck_set",
            "deck_submitted",
            "detailed_logs",
            "first_turn",
            "game_info",
            "login",
            "mulligan",
            "rank",
            "room_state",
            "season",
        ];
        assert_eq!(kinds.into_iter().collect::<Vec<_>>(), expected);
        let json = serde_json::to_value(ArenaEvent::DetailedLogs { enabled: true }).unwrap();
        assert_eq!(json["kind"], "detailed_logs");
    }

    fn sealed_courses() -> Vec<Vec<Course>> {
        let entries =
            split_complete(&fixture_tools::arena_fixture("sealed-direct-fast-login").bytes);
        entries
            .iter()
            .flat_map(decode_all)
            .filter_map(|event| match event {
                ArenaEvent::Courses { courses } => Some(courses),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn decode_reads_limited_courses() {
        let all = sealed_courses();
        assert_eq!(all.len(), 14);
        let first = &all[0];
        assert_eq!(first.len(), 2);
        let direct = first
            .iter()
            .find(|course| course.event_name.contains("Sealed"))
            .unwrap();
        assert_eq!(direct.wins, 0);
        assert_eq!(direct.losses, 0);
        assert_eq!(direct.module, "CreateMatch");
        assert_eq!(direct.deck_name, "Sealed Deck");
        assert_eq!(direct.pool.len(), 78);
        let draft = first
            .iter()
            .find(|course| course.event_name.starts_with("PremierDraft"))
            .unwrap();
        assert_eq!((draft.wins, draft.losses), (1, 1));
        assert_eq!(draft.pool.len(), 42);
        let last = all.last().unwrap();
        let last_direct = last
            .iter()
            .find(|course| course.event_name.contains("Sealed"))
            .unwrap();
        assert_eq!(last_direct.wins, 5);
    }

    #[test]
    fn decode_reads_an_empty_course_list() {
        let events = decode_all(&entry("<== EventGetCoursesV2(x)", r#"{"Courses":[]}"#));
        assert_eq!(events, vec![ArenaEvent::Courses { courses: vec![] }]);
    }
}
