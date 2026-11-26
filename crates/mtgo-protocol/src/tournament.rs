//! Tournament facts for the event card (event overlay spec 7a): the stored
//! tournament lines of one Challenge, Preliminary or other Swiss event,
//! decoded into typed facts.
//!
//! Only the account's own numbers leave the decoder: a round result keeps the
//! account's row and the row count, an elimination says whether it was the
//! account's, a pairing keeps the account's match. No other player's name or
//! login id is kept.
//!
//! MTGO's JSON is hostile: round info misses its outer `}` on every line, so
//! every body is read with `json_repair::first_object`, and round info with
//! exactly that one closer at most (a line cut inside `Matches[]` must not
//! decode as a partial round). A line that still does not decode gives `None`
//! (the caller skips it).

use chrono::DateTime;
use serde_json::Value;

use crate::classify::EventKind;
use crate::json_repair::{first_object, first_object_padded};
use crate::state_change::parse_state_change;

/// One decoded tournament fact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TournamentFact {
    /// `EventSyncData_t`: the event's settings, and at a restart its rounds.
    Snapshot(EventSnapshot),
    /// `Tournament State Changed for T from A to B`: the state moved to `to`.
    StateChanged { to: String },
    /// `FlsTournamentRoundInfoMessage`: a round was paired.
    Round(RoundPairing),
    /// `FlsTournamentRoundResultMessage`: a round's standings.
    Result(RoundResult),
    /// `FlsTournamentPlayerIsEliminatedMessage`.
    Eliminated {
        of_account: bool,
        /// `EliminationReason` (2 looked like a drop, 1 like removal by the
        /// event; inferred, not documented).
        reason: u32,
    },
    /// `FlsPlayerEventStateChangedMessage`.
    PlayerState { new_state: u32, in_playoffs: bool },
    /// `FlsTournamentEndRespMessage`, or a change to `TournamentCompletedState`.
    Ended,
    /// `Match State Changed for M from A to B` of a match of the event.
    MatchState { match_token: String, to: String },
}

/// `EventSyncData_t` for one tournament.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct EventSnapshot {
    pub event_id: Option<i64>,
    /// `Description` (`Legacy Showcase Challenge`).
    pub name: Option<String>,
    /// `GameStructureCd` (`CLEGACY`).
    pub format: Option<String>,
    /// `StartDate` with its offset, as UTC ms.
    pub starts_at_ms: Option<i64>,
    /// `Playoff[0].PlayerCount`; `None` with no playoff (a Preliminary).
    pub cut_size: Option<u32>,
    /// `NumberOfRounds` when above 0.
    pub number_of_rounds: Option<u32>,
    /// The prize text's whole `A-B N` ranges (`Rounds: 129-212 8; ...`).
    pub rounds_table: Vec<RoundsRange>,
    /// `RoundLengthInMinutes`.
    pub round_minutes: Option<u32>,
    /// `PremiereEventSyncData.InPlayoffs`.
    pub in_playoffs: bool,
    /// The rounds a restart snapshot carries (empty at registration).
    pub rounds: Vec<RoundPairing>,
    pub results: Vec<RoundResult>,
    /// The account is in `TournamentSyncData.EliminatedPlayers`.
    pub account_eliminated: bool,
    /// A draft event's draft (`TournamentSyncData.DraftToken`), once fired.
    pub draft_token: Option<String>,
    /// Its MTGO draft id (`TournamentSyncData.DraftID`); `None` for 0, which
    /// MTGO sends before the draft fires.
    pub draft_id: Option<u64>,
}

/// One `A-B N` range of the prize text: `min` to `max` players play `rounds`
/// Swiss rounds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RoundsRange {
    pub min: u32,
    pub max: u32,
    pub rounds: u32,
}

/// One round's pairings.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RoundPairing {
    /// The round info's `ID` (the event id).
    pub event_id: Option<i64>,
    pub number: u32,
    /// The round's match tokens (`Matches[].EventToken`), every player's.
    pub match_tokens: Vec<String>,
    pub bye_count: u32,
    /// The account's match token, when it was paired.
    pub account_match_token: Option<String>,
    /// The account is in `ByeList`.
    pub account_bye: bool,
    /// The round's own `MatchTimeInMinutes` (not the template's).
    pub match_minutes: Option<u32>,
    /// The round's earliest match `StartDate`, UTC ms.
    pub started_at_ms: Option<i64>,
}

/// One round's result.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RoundResult {
    pub number: u32,
    /// `Results[]` rows (dropped and eliminated players included).
    pub rows: u32,
    /// The account's row.
    pub account: Option<AccountResult>,
}

/// The account's row of a round result.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AccountResult {
    pub rank: u32,
    pub points: u32,
    /// Games per round so far (`OpponentResults[]`, without the opponent).
    pub rounds: Vec<GamesInRound>,
}

/// The account's games in one round.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GamesInRound {
    pub round: u32,
    pub won: u32,
    pub lost: u32,
    pub drawn: u32,
    pub bye: bool,
}

/// Round info lines miss exactly their outer `}` (fixture, every line).
const ROUND_INFO_MISSING_CLOSERS: usize = 1;

/// MTGO cuts a prize text at this many characters.
const PRIZE_TEXT_MAX_CHARS: usize = 255;

/// Decode one stored tournament line. `kind` is its classified kind,
/// `account_login_id` the login id of the file's account (the only player whose
/// numbers are kept). `None` for kinds this module does not read and for lines
/// that do not decode.
pub fn decode_fact(
    kind: EventKind,
    text: &str,
    account_login_id: Option<u64>,
) -> Option<TournamentFact> {
    match kind {
        EventKind::TournamentSync => {
            snapshot(&first_object(text)?, account_login_id).map(TournamentFact::Snapshot)
        }
        EventKind::TournamentStateChanged => state_changed(text),
        EventKind::TournamentRoundInfo => {
            let body = first_object_padded(text, ROUND_INFO_MISSING_CLOSERS)?;
            let round = body.get("Round")?;
            let mut pairing = pairing(round, account_login_id)?;
            pairing.event_id = body.get("ID").and_then(Value::as_i64);
            Some(TournamentFact::Round(pairing))
        }
        EventKind::TournamentRoundResult => {
            let body = first_object(text)?;
            let number = as_u32(body.get("Round"))?;
            result(number, body.get("Results")?, account_login_id).map(TournamentFact::Result)
        }
        EventKind::TournamentPlayerEliminated => {
            let body = first_object(text)?;
            let login_id = body.get("LoginID").and_then(Value::as_u64)?;
            Some(TournamentFact::Eliminated {
                of_account: Some(login_id) == account_login_id,
                reason: as_u32(body.get("EliminationReason")).unwrap_or(0),
            })
        }
        EventKind::TournamentEnded => Some(TournamentFact::Ended),
        EventKind::MatchStateChanged => {
            let change = parse_state_change(text)?;
            Some(TournamentFact::MatchState {
                match_token: change.token,
                to: change.to,
            })
        }
        _ => None,
    }
}

/// Decode a `FlsPlayerEventStateChangedMessage` line (`NewState`, `InPlayoffs`).
pub fn decode_player_state(text: &str) -> Option<TournamentFact> {
    let body = first_object(text)?;
    Some(TournamentFact::PlayerState {
        new_state: as_u32(body.get("NewState"))?,
        in_playoffs: body.get("InPlayoffs")?.as_bool()?,
    })
}

/// Whether a tournament state means the account is in the event (`PremierJoined*`,
/// `TournamentJoined*`). `NotJoined` states are other players' events seen
/// from the lobby.
pub fn is_joined_state(state: &str) -> bool {
    state.contains("Joined") && !state.contains("NotJoined")
}

/// Whether a tournament state means the account is not in the event.
pub fn is_not_joined_state(state: &str) -> bool {
    state.contains("NotJoined")
}

/// Whether a match state ends the match: another player's match closes
/// (`TournamentMatchClosedState`), the account's completes
/// (`TournamentMatchJoinedCompletedState`).
pub fn is_match_over_state(state: &str) -> bool {
    state.ends_with("ClosedState") || state.ends_with("CompletedState")
}

fn state_changed(text: &str) -> Option<TournamentFact> {
    let (_, rest) = text.split_once(" from ")?;
    let (_, to) = rest.split_once(" to ")?;
    let to: String = to
        .chars()
        .take_while(|ch| ch.is_ascii_alphanumeric())
        .collect();
    if to.is_empty() {
        return None;
    }
    if to == "TournamentCompletedState" {
        return Some(TournamentFact::Ended);
    }
    Some(TournamentFact::StateChanged { to })
}

fn snapshot(body: &Value, account_login_id: Option<u64>) -> Option<EventSnapshot> {
    let premiere = body.get("PremiereEventSyncData").filter(|v| v.is_object());
    let sync = premiere
        .and_then(|p| p.get("TournamentSyncData"))
        .or_else(|| body.get("TournamentSyncData"))
        .filter(|v| v.is_object());
    let mut found = EventSnapshot {
        event_id: body.get("EventID").and_then(Value::as_i64),
        name: text(body.get("Description")),
        format: text(body.get("GameStructureCd")),
        starts_at_ms: body.get("StartDate").and_then(time_ms),
        ..EventSnapshot::default()
    };
    if let Some(premiere) = premiere {
        found.cut_size = premiere
            .get("Playoff")
            .and_then(Value::as_array)
            .and_then(|playoffs| playoffs.first())
            .and_then(|playoff| as_u32(playoff.get("PlayerCount")))
            .filter(|count| *count > 0);
        found.in_playoffs = premiere
            .get("InPlayoffs")
            .and_then(Value::as_bool)
            .unwrap_or(false);
    }
    if let Some(sync) = sync {
        found.number_of_rounds = as_u32(sync.get("NumberOfRounds")).filter(|n| *n > 0);
        found.round_minutes = as_u32(sync.get("RoundLengthInMinutes")).filter(|n| *n > 0);
        found.rounds_table = sync
            .get("PrizeStructure")
            .and_then(|prizes| prizes.get("Description"))
            .and_then(Value::as_str)
            .map(rounds_table)
            .unwrap_or_default();
        for round in sync
            .get("Rounds")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(pairing) = pairing(round, account_login_id) {
                if let Some(results) = round.get("Results").filter(|r| is_non_empty(r))
                    && let Some(result) = result(pairing.number, results, account_login_id)
                {
                    found.results.push(result);
                }
                found.rounds.push(pairing);
            }
        }
        found.draft_token = text(sync.get("DraftToken"));
        found.draft_id = sync
            .get("DraftID")
            .and_then(Value::as_u64)
            .filter(|id| *id > 0);
        found.account_eliminated = match account_login_id {
            Some(account) => sync
                .get("EliminatedPlayers")
                .and_then(Value::as_array)
                .is_some_and(|ids| ids.iter().any(|id| id.as_u64() == Some(account))),
            None => false,
        };
    }
    Some(found)
}

fn is_non_empty(value: &Value) -> bool {
    value.as_array().is_some_and(|rows| !rows.is_empty())
}

/// A round object (round info's `Round`, a snapshot's `Rounds[]`).
fn pairing(round: &Value, account_login_id: Option<u64>) -> Option<RoundPairing> {
    let number = as_u32(round.get("Number"))?;
    let mut found = RoundPairing {
        number,
        ..RoundPairing::default()
    };
    for game in round
        .get("Matches")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(token) = text(game.get("EventToken")) else {
            continue;
        };
        let create = game
            .get("MatchSyncData")
            .and_then(|sync| sync.get("MatchCreateInfo"));
        if found.match_minutes.is_none() {
            found.match_minutes = create
                .and_then(|info| as_u32(info.get("MatchTimeInMinutes")))
                .filter(|minutes| *minutes > 0);
        }
        if let Some(started) = game.get("StartDate").and_then(time_ms) {
            found.started_at_ms = Some(found.started_at_ms.map_or(started, |at| at.min(started)));
        }
        let is_account = account_login_id.is_some_and(|account| {
            game.get("Players")
                .and_then(Value::as_array)
                .is_some_and(|players| {
                    players.iter().any(|player| {
                        player.get("LoginID").and_then(Value::as_u64) == Some(account)
                    })
                })
        });
        if is_account {
            found.account_match_token = Some(token.clone());
        }
        found.match_tokens.push(token);
    }
    let byes = round.get("ByeList").and_then(Value::as_array);
    found.bye_count = byes.map_or(0, |list| list.len() as u32);
    found.account_bye = account_login_id.is_some_and(|account| {
        byes.is_some_and(|list| list.iter().any(|id| id.as_u64() == Some(account)))
    });
    Some(found)
}

fn result(number: u32, rows: &Value, account_login_id: Option<u64>) -> Option<RoundResult> {
    let rows = rows.as_array()?;
    let account = account_login_id.and_then(|account| {
        rows.iter()
            .find(|row| row.get("LoginID").and_then(Value::as_u64) == Some(account))
            .and_then(account_row)
    });
    Some(RoundResult {
        number,
        rows: rows.len() as u32,
        account,
    })
}

fn account_row(row: &Value) -> Option<AccountResult> {
    let mut rounds = Vec::new();
    for opponent in row
        .get("OpponentResults")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(round) = as_u32(opponent.get("Round")) else {
            continue;
        };
        rounds.push(GamesInRound {
            round,
            won: as_u32(opponent.get("Win")).unwrap_or(0),
            lost: as_u32(opponent.get("Loss")).unwrap_or(0),
            drawn: as_u32(opponent.get("Draw")).unwrap_or(0),
            bye: as_u32(opponent.get("Bye")).unwrap_or(0) > 0,
        });
    }
    Some(AccountResult {
        rank: as_u32(row.get("Rank"))?,
        points: as_u32(row.get("Points")).unwrap_or(0),
        rounds,
    })
}

/// The whole `A-B N` pairs after `Rounds:` in a prize text. The text is cut
/// at 255 characters, so in a text of that length the last pair is left out:
/// it may be cut short, even inside its round count (`10` cut to `1`).
pub fn rounds_table(description: &str) -> Vec<RoundsRange> {
    let Some((_, table)) = description.split_once("Rounds:") else {
        return Vec::new();
    };
    let mut parts: Vec<&str> = table.split(';').collect();
    if description.chars().count() >= PRIZE_TEXT_MAX_CHARS {
        parts.pop();
    }
    let mut ranges = Vec::new();
    for part in parts {
        let mut words = part.split_whitespace();
        let (Some(span), Some(rounds), None) = (words.next(), words.next(), words.next()) else {
            continue;
        };
        let Some((min, max)) = span.split_once('-') else {
            continue;
        };
        if let (Ok(min), Ok(max), Ok(rounds)) = (min.parse(), max.parse(), rounds.parse()) {
            ranges.push(RoundsRange { min, max, rounds });
        }
    }
    ranges
}

/// An MTGO JSON time with its offset (`2026-11-22T17:00:09.6680000+01:00`) as
/// UTC ms.
fn time_ms(value: &Value) -> Option<i64> {
    DateTime::parse_from_rfc3339(value.as_str()?)
        .ok()
        .map(|at| at.timestamp_millis())
}

fn text(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
}

fn as_u32(value: Option<&Value>) -> Option<u32> {
    value
        .and_then(Value::as_u64)
        .and_then(|number| u32::try_from(number).ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{classify, parse_header, split_complete};

    const ACCOUNT: u64 = 900_001;
    const SHOWCASE: &str = "6ab6648d-3371-4daa-ae6b-e81bb8001148";
    const MODERN: &str = "63c90ead-8c2c-407d-9f10-67378e0b96d0";

    /// The Challenge fixture's classified entries: (kind, tournament token, text).
    fn fixture() -> Vec<(EventKind, Option<String>, String)> {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/tournaments/challenge_showcase.log"
        );
        let bytes = std::fs::read(path).expect("the Challenge fixture exists");
        split_complete(&bytes)
            .into_iter()
            .filter_map(|raw| {
                let header = parse_header(&raw.text)?;
                let found = classify(&header, &raw.text)?;
                Some((found.kind, found.subjects.tournament_token, raw.text))
            })
            .collect()
    }

    fn facts_of(kind: EventKind, token: &str, account: Option<u64>) -> Vec<TournamentFact> {
        fixture()
            .into_iter()
            .filter(|(k, t, _)| *k == kind && t.as_deref() == Some(token))
            .map(|(k, _, text)| decode_fact(k, &text, account).expect("decodes"))
            .collect()
    }

    #[test]
    fn every_round_info_needs_its_missing_brace() {
        let lines: Vec<String> = fixture()
            .into_iter()
            .filter(|(kind, _, _)| *kind == EventKind::TournamentRoundInfo)
            .map(|(_, _, text)| text)
            .collect();
        assert!(lines.len() >= 6);
        for text in &lines {
            let start = text.find('{').expect("json");
            let end = text.rfind('}').expect("json");
            assert!(
                serde_json::from_str::<Value>(&text[start..=end]).is_err(),
                "a round info parsed without repair"
            );
            assert!(
                first_object_padded(text, 1).is_some(),
                "a round info did not repair with one closer"
            );
        }
    }

    #[test]
    fn the_account_snapshot_gives_the_event_settings() {
        let snapshots = facts_of(EventKind::TournamentSync, SHOWCASE, Some(ACCOUNT));
        let TournamentFact::Snapshot(first) = &snapshots[0] else {
            panic!("a snapshot")
        };
        assert_eq!(first.event_id, Some(12_858_913));
        assert_eq!(first.name.as_deref(), Some("Legacy Showcase Challenge"));
        assert_eq!(first.format.as_deref(), Some("CLEGACY"));
        assert_eq!(first.cut_size, Some(8));
        assert_eq!(first.number_of_rounds, None);
        assert_eq!(first.round_minutes, Some(50));
        assert_eq!(
            first.starts_at_ms,
            DateTime::parse_from_rfc3339("2026-11-22T16:00:00Z")
                .map(|at| at.timestamp_millis())
                .ok()
        );
        assert_eq!(
            first.rounds_table,
            vec![
                RoundsRange {
                    min: 129,
                    max: 212,
                    rounds: 8
                },
                RoundsRange {
                    min: 213,
                    max: 384,
                    rounds: 9
                },
            ]
        );
        assert!(!first.in_playoffs);
        assert!(first.rounds.is_empty());
        assert_eq!(snapshots.len(), 3, "the snapshot repeats at login");
    }

    /// A Swiss Draft's `EventSyncData_t` line, trimmed (no players, no
    /// prizes).
    const SWISS_DRAFT_SYNC: &str = concat!(
        "15:27:13 [INF] (Game Management|Processing Registered Handler for EventSyncData_t in ",
        "TournamentUninitializedState) Processor: TournamentUninitializedState Message: ",
        "{\"EventToken\":\"aaaaaaaa-1111-4111-8111-111111111111\",\"EventID\":19990050,",
        "\"PlayerEventType\":3,\"GameStructureCd\":\"DFRAFRAFRA\",\"Players\":[],",
        "\"Description\":\"Reality Fracture Swiss Draft\",",
        "\"TournamentSyncData\":{\"TournamentStructureCd\":\"SWISS\",",
        "\"DraftServerID\":3999737,\"DraftToken\":\"bbbbbbbb-2222-4222-8222-222222222222\",",
        "\"DraftID\":7999556,\"DeckBuildingLength\":10}} ",
        "Receiver: WotC.MtGO.Client.Model.Play.TournamentEvent.Tournament"
    );

    #[test]
    fn a_draft_event_snapshot_names_its_draft() {
        let Some(TournamentFact::Snapshot(snapshot)) =
            decode_fact(EventKind::TournamentSync, SWISS_DRAFT_SYNC, None)
        else {
            panic!("a snapshot")
        };
        assert_eq!(snapshot.event_id, Some(19_990_050));
        assert_eq!(
            snapshot.name.as_deref(),
            Some("Reality Fracture Swiss Draft")
        );
        assert_eq!(snapshot.format.as_deref(), Some("DFRAFRAFRA"));
        assert_eq!(
            snapshot.draft_token.as_deref(),
            Some("bbbbbbbb-2222-4222-8222-222222222222")
        );
        assert_eq!(snapshot.draft_id, Some(7_999_556));

        let unfired = SWISS_DRAFT_SYNC
            .replace("\"DraftID\":7999556", "\"DraftID\":0")
            .replace("bbbbbbbb-2222-4222-8222-222222222222", "");
        let Some(TournamentFact::Snapshot(unfired)) =
            decode_fact(EventKind::TournamentSync, &unfired, None)
        else {
            panic!("a snapshot")
        };
        assert_eq!((unfired.draft_token, unfired.draft_id), (None, None));
    }

    #[test]
    fn round_info_gives_the_pairing_and_the_account_match() {
        let rounds = facts_of(EventKind::TournamentRoundInfo, SHOWCASE, Some(ACCOUNT));
        let numbers: Vec<u32> = rounds
            .iter()
            .map(|fact| match fact {
                TournamentFact::Round(round) => round.number,
                _ => panic!("a round"),
            })
            .collect();
        assert_eq!(numbers, vec![1, 1, 2]);
        let TournamentFact::Round(one) = &rounds[0] else {
            panic!()
        };
        assert_eq!(one.event_id, Some(12_858_913));
        assert_eq!(one.match_tokens.len(), 151);
        assert_eq!(one.bye_count, 0);
        assert_eq!(
            one.account_match_token.as_deref().map(|t| &t[..8]),
            Some("3550e0e4")
        );
        assert!(!one.account_bye);
        assert_eq!(
            one.match_minutes,
            Some(50),
            "the round's 50, not the template's 25"
        );
        assert!(one.started_at_ms.is_some());
        assert_eq!(rounds[0], rounds[1], "round 1 is logged twice, identical");
        let TournamentFact::Round(two) = &rounds[2] else {
            panic!()
        };
        assert_eq!((two.match_tokens.len(), two.bye_count), (143, 1));
        assert_eq!(
            two.account_match_token.as_deref().map(|t| &t[..8]),
            Some("f6413b0c")
        );
    }

    #[test]
    fn a_round_result_keeps_only_the_account_row() {
        let results = facts_of(EventKind::TournamentRoundResult, SHOWCASE, Some(ACCOUNT));
        let [TournamentFact::Result(one)] = results.as_slice() else {
            panic!("one result: {results:?}")
        };
        assert_eq!(one.number, 1);
        assert_eq!(one.rows, 302);
        let account = one.account.as_ref().expect("the account row");
        assert_eq!((account.rank, account.points), (139, 3));
        assert_eq!(
            account.rounds,
            vec![GamesInRound {
                round: 1,
                won: 2,
                lost: 1,
                drawn: 0,
                bye: false
            }]
        );
        let other = facts_of(EventKind::TournamentRoundResult, SHOWCASE, Some(1));
        let [TournamentFact::Result(other)] = other.as_slice() else {
            panic!()
        };
        assert_eq!(other.account, None);
    }

    #[test]
    fn eliminations_say_only_whether_they_were_the_account() {
        let eliminations = facts_of(
            EventKind::TournamentPlayerEliminated,
            MODERN,
            Some(9_000_130),
        );
        assert!(eliminations.contains(&TournamentFact::Eliminated {
            of_account: true,
            reason: 2
        }));
        assert!(eliminations.contains(&TournamentFact::Eliminated {
            of_account: false,
            reason: 1
        }));
    }

    #[test]
    fn state_changes_and_the_end() {
        let changes = facts_of(EventKind::TournamentStateChanged, SHOWCASE, Some(ACCOUNT));
        assert_eq!(
            changes[0],
            TournamentFact::StateChanged {
                to: "PremierJoinedAwaitingMaxPlayersState".to_string()
            }
        );
        assert!(changes.iter().all(|fact| match fact {
            TournamentFact::StateChanged { to } => is_joined_state(to),
            _ => false,
        }));
        let ended = state_changed(
            "16:57:47 [INF] (Game Management|Tournament State Changed for 9c34834e-4445-449b-829d-beb2122c465f from TournamentNotJoinedBetweenRoundsState to TournamentCompletedState) ",
        );
        assert_eq!(ended, Some(TournamentFact::Ended));
        assert!(!is_joined_state("TournamentNotJoinedRoundInProgressState"));
        assert!(is_not_joined_state(
            "PremierNotJoinedAwaitingMinPlayersState"
        ));
    }

    #[test]
    fn player_state_lines_give_in_playoffs() {
        let text = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/tournaments/challenge_showcase.log"
        ))
        .expect("fixture");
        let line = text
            .lines()
            .find(|text| {
                text.contains("Handler for FlsPlayerEventStateChangedMessage")
                    && text.contains(MODERN)
                    && text.contains("\"InPlayoffs\":true")
            })
            .expect("an InPlayoffs line");
        assert_eq!(
            decode_player_state(line),
            Some(TournamentFact::PlayerState {
                new_state: 24,
                in_playoffs: true
            })
        );
    }

    #[test]
    fn a_finished_snapshot_carries_its_rounds_and_results() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/tournaments/challenge_showcase.log"
        );
        let text = std::fs::read_to_string(path).expect("fixture");
        let line = text
            .lines()
            .find(|line| {
                line.contains("Handler for EventSyncData_t")
                    && line.contains("Pioneer Challenge 16")
                    && line.contains("\"PlayerEventState\":25")
            })
            .expect("the finished Pioneer snapshot");
        let Some(TournamentFact::Snapshot(found)) =
            decode_fact(EventKind::TournamentSync, line, Some(1))
        else {
            panic!("a snapshot")
        };
        let counts: Vec<usize> = found.rounds.iter().map(|r| r.match_tokens.len()).collect();
        assert_eq!(counts, vec![16, 15, 14, 12, 11, 4, 2, 1]);
        assert_eq!(found.results.len(), 8);
        assert!(found.in_playoffs);
        assert_eq!(found.cut_size, Some(8));
        assert_eq!(found.results[0].rows, 32);
    }

    #[test]
    fn damaged_lines_give_nothing() {
        let line = "17:00:08 [INF] (Game Management|Processing Registered Handler for FlsTournamentRoundInfoMessage in X) Processor: X Message: {\"Token\":\"t\",\"Round\":{\"Number\":";
        assert_eq!(
            decode_fact(EventKind::TournamentRoundInfo, line, Some(ACCOUNT)),
            None
        );
        assert_eq!(decode_player_state("no json"), None);
    }

    #[test]
    fn a_round_info_cut_inside_its_matches_gives_nothing() {
        let (_, _, text) = fixture()
            .into_iter()
            .find(|(kind, _, _)| *kind == EventKind::TournamentRoundInfo)
            .expect("a round info");
        // Cut after the first match object: a whole match, then nothing.
        let cut_at = text.find("},{\"EventToken\"").expect("two matches") + 1;
        let cut = &text[..cut_at];
        assert!(
            first_object(cut).is_some(),
            "three closers would make a partial round of it"
        );
        assert_eq!(
            decode_fact(EventKind::TournamentRoundInfo, cut, Some(ACCOUNT)),
            None
        );
    }

    /// Every result row of the fixture: `Points = 3W + D`, a bye a win.
    #[test]
    fn points_are_three_per_win_and_one_per_draw() {
        let mut rows = 0;
        for (kind, _, text) in fixture() {
            if kind != EventKind::TournamentRoundResult {
                continue;
            }
            let body = first_object(&text).expect("json");
            let number = as_u32(body.get("Round")).expect("round");
            for row in body["Results"].as_array().expect("rows") {
                let login_id = row["LoginID"].as_u64().expect("login id");
                let Some(TournamentFact::Result(result)) = decode_fact(kind, &text, Some(login_id))
                else {
                    panic!("a result")
                };
                assert_eq!(result.number, number);
                let account = result.account.expect("the row");
                let (mut wins, mut draws) = (0, 0);
                for games in &account.rounds {
                    if games.bye || games.won > games.lost {
                        wins += 1;
                    } else if games.won == games.lost && games.won + games.lost + games.drawn > 0 {
                        draws += 1;
                    }
                }
                assert_eq!(
                    account.points,
                    3 * wins + draws,
                    "{login_id} in round {number}"
                );
                rows += 1;
            }
        }
        assert!(rows > 300);
    }

    #[test]
    fn a_full_length_prize_text_leaves_out_its_last_pair() {
        let head = "Treasure. Rounds: 129-212 8; 213-384 9; 385-1000 1";
        let full = format!(
            "{}{head}",
            "x".repeat(PRIZE_TEXT_MAX_CHARS - head.chars().count())
        );
        assert_eq!(full.chars().count(), PRIZE_TEXT_MAX_CHARS);
        assert_eq!(rounds_table(&full).len(), 2, "the 1 may be a cut 10");
        assert_eq!(rounds_table(head).len(), 3, "a short text was not cut");
    }

    #[test]
    fn a_cut_prize_text_keeps_only_whole_pairs() {
        assert_eq!(
            rounds_table("Treasure. Rounds: 129-212 8; 213-384 9; 3"),
            vec![
                RoundsRange {
                    min: 129,
                    max: 212,
                    rounds: 8
                },
                RoundsRange {
                    min: 213,
                    max: 384,
                    rounds: 9
                },
            ]
        );
        assert!(rounds_table("Top 8 earn a Token").is_empty());
    }
}
