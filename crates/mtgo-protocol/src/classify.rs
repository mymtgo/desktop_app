//! Whitelist classification of events. Anything not listed returns `None` and is
//! never stored.

use std::sync::LazyLock;

use regex::Regex;
use serde::Serialize;

use crate::header::Header;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    MatchStateChanged,
    GameStateUpdate,
    DeckUsed,
    GameManagementJson,
    /// A registered decklist: a match's (`FlsMatchDeckGetRespMessage`, with
    /// its match token) or a tournament's (`FlsTournamentDeckRespMessage`,
    /// with its tournament token and no match token).
    MatchDeckRegistered,
    TournamentStateChanged,
    TournamentSync,
    TournamentRoundInfo,
    TournamentRoundResult,
    TournamentPlayerEliminated,
    TournamentEnded,
    DraftCreated,
    DraftStateChanged,
    DraftSelection,
    DraftLeagueStanding,
    DraftJoined,
    DraftPodState,
    DraftPackOpened,
    DraftPendingPick,
    DraftEnded,
    DraftPickCommitted,
    LeaguePoolGranted,
    LeagueJoinRequest,
    LeagueJoined,
    LeagueDropped,
    Login,
    /// `FlsPlayerEventStateChangedMessage` (`NewState`, `InPlayoffs`): kept in
    /// the store's `tournament_player_states`, never in `log_events` (not in
    /// `ALL`), and never shipped to the API.
    TournamentPlayerState,
}

impl EventKind {
    /// Every kind stored in `log_events`, for its `kind` CHECK list and its
    /// test. `TournamentPlayerState` is not one: it has its own table.
    pub const ALL: [EventKind; 26] = [
        EventKind::MatchStateChanged,
        EventKind::GameStateUpdate,
        EventKind::DeckUsed,
        EventKind::GameManagementJson,
        EventKind::MatchDeckRegistered,
        EventKind::TournamentStateChanged,
        EventKind::TournamentSync,
        EventKind::TournamentRoundInfo,
        EventKind::TournamentRoundResult,
        EventKind::TournamentPlayerEliminated,
        EventKind::TournamentEnded,
        EventKind::DraftCreated,
        EventKind::DraftStateChanged,
        EventKind::DraftSelection,
        EventKind::DraftLeagueStanding,
        EventKind::DraftJoined,
        EventKind::DraftPodState,
        EventKind::DraftPackOpened,
        EventKind::DraftPendingPick,
        EventKind::DraftEnded,
        EventKind::DraftPickCommitted,
        EventKind::LeaguePoolGranted,
        EventKind::LeagueJoinRequest,
        EventKind::LeagueJoined,
        EventKind::LeagueDropped,
        EventKind::Login,
    ];

    /// The stored name (`log_events.kind`), identical to the serde name.
    pub fn as_str(self) -> &'static str {
        match self {
            EventKind::MatchStateChanged => "match_state_changed",
            EventKind::GameStateUpdate => "game_state_update",
            EventKind::DeckUsed => "deck_used",
            EventKind::GameManagementJson => "game_management_json",
            EventKind::MatchDeckRegistered => "match_deck_registered",
            EventKind::TournamentStateChanged => "tournament_state_changed",
            EventKind::TournamentSync => "tournament_sync",
            EventKind::TournamentRoundInfo => "tournament_round_info",
            EventKind::TournamentRoundResult => "tournament_round_result",
            EventKind::TournamentPlayerEliminated => "tournament_player_eliminated",
            EventKind::TournamentEnded => "tournament_ended",
            EventKind::DraftCreated => "draft_created",
            EventKind::DraftStateChanged => "draft_state_changed",
            EventKind::DraftSelection => "draft_selection",
            EventKind::DraftLeagueStanding => "draft_league_standing",
            EventKind::DraftJoined => "draft_joined",
            EventKind::DraftPodState => "draft_pod_state",
            EventKind::DraftPackOpened => "draft_pack_opened",
            EventKind::DraftPendingPick => "draft_pending_pick",
            EventKind::DraftEnded => "draft_ended",
            EventKind::DraftPickCommitted => "draft_pick_committed",
            EventKind::LeaguePoolGranted => "league_pool_granted",
            EventKind::LeagueJoinRequest => "league_join_request",
            EventKind::LeagueJoined => "league_joined",
            EventKind::LeagueDropped => "league_dropped",
            EventKind::Login => "login",
            EventKind::TournamentPlayerState => "tournament_player_state",
        }
    }
}

/// The identifiers an event genuinely carries. Each kind fills only its own.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Subjects {
    pub match_token: Option<String>,
    pub match_id: Option<u64>,
    pub game_id: Option<u64>,
    pub tournament_token: Option<String>,
    pub league_token: Option<String>,
    pub league_event_id: Option<u64>,
    pub draft_token: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Classified {
    pub kind: EventKind,
    pub subjects: Subjects,
}

fn re(pattern: &str) -> Regex {
    Regex::new(pattern).expect("valid regex")
}

static MATCH_STATE_CHANGED: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?i)Match State Changed for ([a-f0-9\-]+)"));
static GAME_STATE_UPDATE: LazyLock<Regex> =
    LazyLock::new(|| re(r"Game ID:\s*(\d+), Match ID:\s*(\d+)"));
static DECK_USED: LazyLock<Regex> = LazyLock::new(|| re(r"Deck Used in Game ID:\s*(\d+)"));
static TOURNAMENT_STATE_CHANGED: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?i)Tournament State Changed for ([a-f0-9\-]{36}) from \S+ to \S+"));
static DRAFT_CREATED: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?i)Draft Created: ([a-f0-9\-]{36})"));
static DRAFT_STATE_CHANGED: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?i)Draft State Changed for ([a-f0-9\-]{36}) from \S+ to \S+"));
static JSON_MATCH_TOKEN: LazyLock<Regex> =
    LazyLock::new(|| re(r#"(?i)"MatchToken"\s*:\s*"([a-f0-9\-]{36})""#));
static JSON_MATCH_ID: LazyLock<Regex> = LazyLock::new(|| re(r#""MatchID"\s*:\s*(\d+)"#));
static JSON_GAME_ID: LazyLock<Regex> = LazyLock::new(|| re(r#""GameID"\s*:\s*(\d+)"#));
static JSON_DRAFT_TOKEN: LazyLock<Regex> =
    LazyLock::new(|| re(r#"(?i)"DraftToken"\s*:\s*"([a-f0-9\-]{36})""#));
static JSON_EVENT_TOKEN: LazyLock<Regex> =
    LazyLock::new(|| re(r#"(?i)"EventToken"\s*:\s*"([a-f0-9\-]{36})""#));
static JSON_TOURNAMENT_TOKEN: LazyLock<Regex> =
    LazyLock::new(|| re(r#"(?i)"TournamentToken"\s*:\s*"([a-f0-9\-]{36})""#));
static JSON_TOKEN: LazyLock<Regex> =
    LazyLock::new(|| re(r#"(?i)"Token"\s*:\s*"([a-f0-9\-]{36})""#));
static LEAGUE_PANEL: LazyLock<Regex> = LazyLock::new(|| {
    re(
        r"(?:Creating GameDetailsView|Join Event|Flip To Details Side|Flip To Game Side: (?:LeagueDraftViewModel|LeagueDeckBuildingViewModel))\) (?:Details: )?League\b",
    )
});
static EVENT_TOKEN_KV: LazyLock<Regex> = LazyLock::new(|| re(r"EventToken=(\S+)"));
static EVENT_ID_KV: LazyLock<Regex> = LazyLock::new(|| re(r"EventId=(\d+)"));

/// Tournament markers: (marker text, kind, which JSON key holds the token).
const TOURNAMENT_MARKERS: [(&str, EventKind, bool); 6] = [
    ("EventSyncData_t", EventKind::TournamentSync, true),
    (
        "FlsTournamentRoundInfoMessage",
        EventKind::TournamentRoundInfo,
        false,
    ),
    (
        "FlsTournamentRoundResultMessage",
        EventKind::TournamentRoundResult,
        false,
    ),
    (
        "FlsTournamentPlayerIsEliminatedMessage",
        EventKind::TournamentPlayerEliminated,
        false,
    ),
    (
        "FlsTournamentEndRespMessage",
        EventKind::TournamentEnded,
        false,
    ),
    (
        "FlsPlayerEventStateChangedMessage",
        EventKind::TournamentPlayerState,
        false,
    ),
];

const DRAFT_MARKERS: [(&str, EventKind); 6] = [
    ("LeagueStanding_t in Draft", EventKind::DraftLeagueStanding),
    (
        "Received FlsLeagueUserJoinDraftRespMessage",
        EventKind::DraftJoined,
    ),
    ("FlsBoosterDraftStateMessage", EventKind::DraftPodState),
    ("FlsBoosterDraftOpenPackMessage", EventKind::DraftPackOpened),
    (
        "FlsBoosterDraftPendingPickMessage",
        EventKind::DraftPendingPick,
    ),
    ("FlsBoosterDraftEndedMessage", EventKind::DraftEnded),
];

fn capture(regex: &Regex, text: &str) -> Option<String> {
    regex.captures(text).map(|caps| caps[1].to_string())
}

fn capture_u64(regex: &Regex, text: &str) -> Option<u64> {
    regex.captures(text).and_then(|caps| caps[1].parse().ok())
}

fn found(kind: EventKind, subjects: Subjects) -> Option<Classified> {
    Some(Classified { kind, subjects })
}

/// Classify one event. Order of checks matches 0.x `ClassifyLogEvent`.
///
/// Every marker and pattern counts only on the entry's header line: a UI
/// entry's .NET stack trace names message types in its frames
/// (`FlsTournamentRoundInfoHandler(FlsTournamentRoundInfoMessage msg, ...)`)
/// without being that message. Identifiers in JSON bodies are still read from
/// the whole text (some bodies run onto the next lines).
pub fn classify(header: &Header, text: &str) -> Option<Classified> {
    let header_line = text.split('\n').next().unwrap_or(text);

    if let Some(token) = capture(&MATCH_STATE_CHANGED, header_line) {
        let subjects = Subjects {
            match_token: Some(token),
            ..Subjects::default()
        };
        return found(EventKind::MatchStateChanged, subjects);
    }

    if let Some(caps) = GAME_STATE_UPDATE.captures(header_line) {
        let subjects = Subjects {
            game_id: caps[1].parse().ok(),
            match_id: caps[2].parse().ok(),
            ..Subjects::default()
        };
        return found(EventKind::GameStateUpdate, subjects);
    }

    if let Some(game_id) = capture_u64(&DECK_USED, header_line) {
        let subjects = Subjects {
            game_id: Some(game_id),
            ..Subjects::default()
        };
        return found(EventKind::DeckUsed, subjects);
    }

    if let Some(token) = capture(&TOURNAMENT_STATE_CHANGED, header_line) {
        let subjects = Subjects {
            tournament_token: Some(token),
            ..Subjects::default()
        };
        return found(EventKind::TournamentStateChanged, subjects);
    }

    for (marker, kind, uses_event_token) in TOURNAMENT_MARKERS {
        if header_line.contains(marker) {
            let token_regex = if uses_event_token {
                &JSON_EVENT_TOKEN
            } else {
                &JSON_TOKEN
            };
            let subjects = Subjects {
                tournament_token: capture(token_regex, text),
                ..Subjects::default()
            };
            return found(kind, subjects);
        }
    }

    if let Some(token) = capture(&DRAFT_CREATED, header_line) {
        let subjects = Subjects {
            draft_token: Some(token),
            ..Subjects::default()
        };
        return found(EventKind::DraftCreated, subjects);
    }

    if let Some(token) = capture(&DRAFT_STATE_CHANGED, header_line) {
        let subjects = Subjects {
            draft_token: Some(token),
            ..Subjects::default()
        };
        return found(EventKind::DraftStateChanged, subjects);
    }

    if header_line.contains("SubmitDraftSelectionsAction:") {
        return found(EventKind::DraftSelection, Subjects::default());
    }

    if header_line.contains("League User Initial Grant") {
        return found(EventKind::LeaguePoolGranted, Subjects::default());
    }

    if header_line.contains("Inbound: FlsMatchDeckGetRespMessage") {
        let subjects = Subjects {
            match_token: capture(&JSON_MATCH_TOKEN, text),
            match_id: capture_u64(&JSON_MATCH_ID, text),
            ..Subjects::default()
        };
        return found(EventKind::MatchDeckRegistered, subjects);
    }

    // Not in 0.x. The tournament's registered deck, logged when MTGO logs in
    // while the tournament runs: the only list a round has when its game 1
    // logs no deck. It names no match, and every match reader of this kind
    // filters by match token.
    if header_line.contains("FlsTournamentDeckRespMessage") {
        let subjects = Subjects {
            tournament_token: capture(&JSON_TOURNAMENT_TOKEN, text),
            ..Subjects::default()
        };
        return found(EventKind::MatchDeckRegistered, subjects);
    }

    for (marker, kind) in DRAFT_MARKERS {
        if header_line.contains(marker) {
            let subjects = Subjects {
                draft_token: capture(&JSON_DRAFT_TOKEN, text),
                ..Subjects::default()
            };
            return found(kind, subjects);
        }
    }

    if header_line.contains("FlsBoosterDraftPickSucceededMessage") {
        if !text.contains("\"IsReservation\":false") {
            return None;
        }
        let subjects = Subjects {
            draft_token: capture(&JSON_DRAFT_TOKEN, text),
            ..Subjects::default()
        };
        return found(EventKind::DraftPickCommitted, subjects);
    }

    if header_line.contains("Message:")
        && (header_line.contains("{\"MatchToken\"") || header_line.contains("{\"MatchID\""))
    {
        let subjects = Subjects {
            match_token: capture(&JSON_MATCH_TOKEN, text),
            match_id: capture_u64(&JSON_MATCH_ID, text),
            game_id: capture_u64(&JSON_GAME_ID, text),
            ..Subjects::default()
        };
        if subjects.match_token.is_some() || subjects.match_id.is_some() {
            return found(EventKind::GameManagementJson, subjects);
        }
    }

    if header_line.contains("FlsLeagueUserJoinReqMessage") {
        return found(EventKind::LeagueJoinRequest, Subjects::default());
    }

    if header_line.contains("FlsLeagueUserDropReqMessage") {
        return found(EventKind::LeagueDropped, Subjects::default());
    }

    if LEAGUE_PANEL.is_match(header_line) {
        let token = capture(&EVENT_TOKEN_KV, text);
        let event_id = capture_u64(&EVENT_ID_KV, text);
        if let (Some(token), Some(event_id)) = (token, event_id) {
            let subjects = Subjects {
                league_token: Some(token),
                league_event_id: Some(event_id),
                ..Subjects::default()
            };
            return found(EventKind::LeagueJoined, subjects);
        }
    }

    if header.category == "Login" && header.context == "MtGO Login Success" {
        return found(EventKind::Login, Subjects::default());
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::header::parse_header;

    #[test]
    fn every_kind_appears_once_in_all() {
        let mut seen = std::collections::HashSet::new();
        for kind in EventKind::ALL {
            assert!(seen.insert(kind), "{kind:?} listed twice");
        }
        assert_eq!(seen.len(), 26);
    }

    /// Where `kind` sits in `EventKind::ALL`. The `match` has no wildcard arm on
    /// purpose: a new variant fails to compile here. Give it the next index and
    /// append it to `ALL` (and to the `log_events.kind` CHECK list in the
    /// store's schema). The test below checks the order of `ALL`; it cannot see
    /// a variant missing from `ALL`.
    fn position_in_all(kind: EventKind) -> usize {
        match kind {
            EventKind::MatchStateChanged => 0,
            EventKind::GameStateUpdate => 1,
            EventKind::DeckUsed => 2,
            EventKind::GameManagementJson => 3,
            EventKind::MatchDeckRegistered => 4,
            EventKind::TournamentStateChanged => 5,
            EventKind::TournamentSync => 6,
            EventKind::TournamentRoundInfo => 7,
            EventKind::TournamentRoundResult => 8,
            EventKind::TournamentPlayerEliminated => 9,
            EventKind::TournamentEnded => 10,
            EventKind::DraftCreated => 11,
            EventKind::DraftStateChanged => 12,
            EventKind::DraftSelection => 13,
            EventKind::DraftLeagueStanding => 14,
            EventKind::DraftJoined => 15,
            EventKind::DraftPodState => 16,
            EventKind::DraftPackOpened => 17,
            EventKind::DraftPendingPick => 18,
            EventKind::DraftEnded => 19,
            EventKind::DraftPickCommitted => 20,
            EventKind::LeaguePoolGranted => 21,
            EventKind::LeagueJoinRequest => 22,
            EventKind::LeagueJoined => 23,
            EventKind::LeagueDropped => 24,
            EventKind::Login => 25,
            // Not stored in `log_events`, so not in `ALL`.
            EventKind::TournamentPlayerState => usize::MAX,
        }
    }

    #[test]
    fn all_lists_every_variant_in_order() {
        for (index, kind) in EventKind::ALL.into_iter().enumerate() {
            assert_eq!(position_in_all(kind), index, "{kind:?}");
        }
    }

    #[test]
    fn stored_kind_names_match_the_serde_names() {
        for kind in EventKind::ALL {
            assert_eq!(
                serde_json::to_value(kind).expect("json"),
                serde_json::Value::String(kind.as_str().to_string())
            );
        }
    }

    fn run(text: &str) -> Option<Classified> {
        classify(&parse_header(text).expect("header"), text)
    }

    /// The Challenge fixture's entries (event overlay spec 7a).
    fn challenge_entries() -> Vec<String> {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/tournaments/challenge_showcase.log"
        );
        let bytes = std::fs::read(path).expect("the Challenge fixture exists");
        crate::split_complete(&bytes)
            .into_iter()
            .map(|raw| raw.text)
            .collect()
    }

    /// A UI entry whose stack trace frames name `FlsTournamentRoundInfoMessage`
    /// is not round info: markers count only on the header line.
    #[test]
    fn a_marker_in_a_stack_trace_is_not_the_message() {
        let stacks: Vec<String> = challenge_entries()
            .into_iter()
            .filter(|text| text.contains("(UI|") && text.contains("FlsTournamentRoundInfoMessage"))
            .collect();
        assert_eq!(
            stacks.len(),
            4,
            "the fixture keeps the 4 UI stack trace entries"
        );
        for text in &stacks {
            assert_eq!(run(text), None, "{}", text.lines().next().unwrap_or(""));
        }
    }

    /// Every other marker is read from the header line only, too.
    #[test]
    fn no_marker_counts_outside_the_header_line() {
        let ui = challenge_entries()
            .into_iter()
            .find(|text| text.contains("(UI|CreateAndFlipToDuelScene)"))
            .expect("a UI entry");
        for marker in [
            "EventSyncData_t",
            "FlsTournamentRoundResultMessage",
            "FlsTournamentPlayerIsEliminatedMessage",
            "FlsTournamentEndRespMessage",
            "LeagueStanding_t in Draft",
            "Received FlsLeagueUserJoinDraftRespMessage",
            "FlsBoosterDraftStateMessage",
            "FlsBoosterDraftOpenPackMessage",
            "FlsBoosterDraftPendingPickMessage",
            "FlsBoosterDraftEndedMessage",
            "FlsBoosterDraftPickSucceededMessage \"IsReservation\":false",
            "FlsTournamentDeckRespMessage",
            "Inbound: FlsMatchDeckGetRespMessage",
            "FlsLeagueUserJoinReqMessage",
            "FlsLeagueUserDropReqMessage",
            "League User Initial Grant",
            "SubmitDraftSelectionsAction:",
            "Flip To Details Side) Details: League",
        ] {
            let text = ui.replace(
                "   en Shiny.Play.GameDetailsViewModel.CreateAndFlipToDuelScene(IGame game)",
                &format!("   en Shiny.Handler({marker} msg)"),
            );
            assert!(text.contains(marker), "{marker} reached the stack");
            assert_eq!(run(&text), None, "{marker}");
        }
    }

    /// `InPlayoffs` lines classify with their token, outside `ALL`.
    #[test]
    fn player_state_lines_have_their_own_kind() {
        let found: Vec<Classified> = challenge_entries()
            .iter()
            .filter(|text| text.contains("Handler for FlsPlayerEventStateChangedMessage"))
            .map(|text| run(text).expect("classified"))
            .collect();
        assert!(!found.is_empty());
        for classified in &found {
            assert_eq!(classified.kind, EventKind::TournamentPlayerState);
            assert!(classified.subjects.tournament_token.is_some());
        }
        assert!(!EventKind::ALL.contains(&EventKind::TournamentPlayerState));
        assert_eq!(
            EventKind::TournamentPlayerState.as_str(),
            "tournament_player_state"
        );
    }

    /// The real tournament lines of the fixture still classify, with their token.
    #[test]
    fn the_fixture_tournament_lines_still_classify() {
        let mut kinds: Vec<EventKind> = Vec::new();
        for text in challenge_entries() {
            if let Some(found) = run(&text)
                && found.kind == EventKind::TournamentRoundInfo
            {
                assert!(
                    found.subjects.tournament_token.is_some(),
                    "{}",
                    &text[..200]
                );
                kinds.push(found.kind);
            }
        }
        // Round 1 twice and round 2 of the account's event, rounds 6, 7 and 8
        // of the Modern Challenge, and the lobby noise event's round info.
        assert!(kinds.len() >= 6, "{}", kinds.len());
    }

    const TOKEN: &str = "f5e33a1f-c2e7-4678-b30d-309b63f17a40";

    #[test]
    fn match_state_changed_carries_the_token() {
        let text =
            format!("12:23:54 [INF] (Game Management|Match State Changed for {TOKEN} from A to B)");
        let c = run(&text).expect("classified");
        assert_eq!(c.kind, EventKind::MatchStateChanged);
        assert_eq!(c.subjects.match_token.as_deref(), Some(TOKEN));
    }

    #[test]
    fn game_state_update_carries_game_and_match_ids_only() {
        let text = "12:25:50 [INF] (Twitch Info|Game Play Status Update for Game ID: 945500844, Match ID: 285746550, Event ID: 285746550) {\"Players\":[]}";
        let c = run(text).expect("classified");
        assert_eq!(c.kind, EventKind::GameStateUpdate);
        assert_eq!(c.subjects.game_id, Some(945_500_844));
        assert_eq!(c.subjects.match_id, Some(285_746_550));
        assert_eq!(c.subjects.match_token, None);
    }

    #[test]
    fn deck_used_carries_the_game_id() {
        let text =
            "12:25:49 [INF] (Twitch Info|Username: Some Name Deck Used in Game ID: 945500844) []";
        let c = run(text).expect("classified");
        assert_eq!(c.kind, EventKind::DeckUsed);
        assert_eq!(c.subjects.game_id, Some(945_500_844));
    }

    #[test]
    fn game_management_json_carries_token_match_and_game_ids() {
        let text = format!(
            "12:25:49 [INF] (Game Management|Processing Registered Handler for GsMessageMessage in LeagueMatchJoinedEventUnderwayState) Processor: X Message: {{\"MatchToken\":\"{TOKEN}\",\"MatchID\":285746550,\"GameID\":945500844,\"MetaMessage\":[1,2]}} Receiver: Event Token={TOKEN}"
        );
        let c = run(&text).expect("classified");
        assert_eq!(c.kind, EventKind::GameManagementJson);
        assert_eq!(c.subjects.match_token.as_deref(), Some(TOKEN));
        assert_eq!(c.subjects.match_id, Some(285_746_550));
        assert_eq!(c.subjects.game_id, Some(945_500_844));
    }

    #[test]
    fn tournament_sync_is_checked_before_game_management_json() {
        let text = format!(
            "12:23:52 [INF] (Game Management|Processing Registered Handler for EventSyncData_t in TournamentUninitializedState) Message: {{\"EventToken\":\"{TOKEN}\",\"MatchCreateInfo\":{{\"MatchToken\":\"{TOKEN}\"}}}}"
        );
        let c = run(&text).expect("classified");
        assert_eq!(c.kind, EventKind::TournamentSync);
        assert_eq!(c.subjects.tournament_token.as_deref(), Some(TOKEN));
        assert_eq!(c.subjects.match_token, None);
    }

    /// A tournament deck response as MTGO logs it at a late login, with the
    /// `Receiver` tail on the following lines.
    fn tournament_deck_text(cards: &str) -> String {
        format!(
            "17:02:17 [INF] (Game Management|Processing Registered Handler for FlsTournamentDeckRespMessage in TournamentJoinedRoundInProgressState) Processor: TournamentJoinedRoundInProgressState Message: {{\"TournamentToken\":\"{TOKEN}\",\"TournamentID\":12857433,\"DeckCards\":{cards},\"DeckName\":\"Qdeck\",\"DeckBoxID\":1,\"ReturnCode\":0}}\n}} Receiver: WotC.MtGO.Client.Model.Play.TournamentEvent.Tournament"
        )
    }

    #[test]
    fn a_tournament_deck_response_is_a_registered_deck_without_a_match() {
        let full = tournament_deck_text(
            "[{\"CatalogID\":144586,\"Annotation\":0,\"PermissionTypeCode\":0,\"Quantity\":4,\"InSideboard\":false}]",
        );
        for text in [full, tournament_deck_text("[]")] {
            let c = run(&text).expect("classified");
            assert_eq!(c.kind, EventKind::MatchDeckRegistered);
            assert_eq!(
                c.subjects,
                Subjects {
                    tournament_token: Some(TOKEN.to_string()),
                    ..Subjects::default()
                },
                "keyed by its tournament token only: every match reader filters by match token"
            );
        }
    }

    #[test]
    fn league_joined_uses_league_subjects_never_match_token() {
        let text = format!(
            "12:20:00 [INF] (UI|Creating GameDetailsView) League EventToken={TOKEN} EventId=11039"
        );
        let c = run(&text).expect("classified");
        assert_eq!(c.kind, EventKind::LeagueJoined);
        assert_eq!(c.subjects.league_token.as_deref(), Some(TOKEN));
        assert_eq!(c.subjects.league_event_id, Some(11039));
        assert_eq!(c.subjects.match_token, None);
    }

    #[test]
    fn pick_reservations_are_ignored_but_commits_are_kept() {
        let reservation = format!(
            "12:00:00 [INF] (Draft|FlsBoosterDraftPickSucceededMessage) {{\"DraftToken\":\"{TOKEN}\",\"IsReservation\":true}}"
        );
        assert_eq!(run(&reservation), None);
        let commit = reservation.replace("\"IsReservation\":true", "\"IsReservation\":false");
        assert_eq!(
            run(&commit).expect("classified").kind,
            EventKind::DraftPickCommitted
        );
    }

    #[test]
    fn only_the_login_success_line_is_a_login() {
        let success = "12:23:25 [INF] (Login|MtGO Login Success) Username: Some_Name (6031972)";
        assert_eq!(run(success).expect("classified").kind, EventKind::Login);
        let last = "12:23:25 [INF] (Login|MtGO Login Last Success) Username: Some_Name";
        assert_eq!(run(last), None);
    }

    #[test]
    fn unlisted_lines_are_ignored() {
        assert_eq!(
            run("12:00:00 [INF] (UI|LastLoginName) Username=Some_Name"),
            None
        );
        assert_eq!(run("12:00:00 [DBG] (Telemetry|anything at all)"), None);
    }
}
