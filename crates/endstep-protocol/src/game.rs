//! Game frames: full states, deltas and events.
//!
//! The game socket wraps each message in an envelope
//! `{type, matchId, seq, protocolVersion, source, payload}`; `seq` counts per
//! match across units. The lobby socket repeats some game messages with its
//! own `seq` counter; only its `GAME_OVER` is read, and its `seq` is dropped
//! so a lobby counter never mixes with game socket ones.

use std::collections::BTreeMap;

use serde_json::Value;

use crate::frame::FrameKind;
use crate::json;

/// A full game state (`GAME_STATE` on the game socket, or the `GAME_OVER`
/// payload on the lobby socket).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameState {
    pub seq: Option<u64>,
    pub match_id: Option<String>,
    pub game_id: String,
    pub game_number: Option<u32>,
    pub games_played: Option<u32>,
    pub wins_by_seat: Vec<u32>,
    pub is_match_over: bool,
    pub turn_number: Option<u32>,
    pub active_player: Option<u32>,
    pub winner: Option<u32>,
    /// (seat, name) for each player.
    pub players: Vec<(u32, String)>,
    /// Each player's numbers (life and zone sizes), in frame order.
    pub player_states: Vec<PlayerState>,
    /// True while players are sideboarding (`sideboarding`).
    pub sideboarding: bool,
    /// The user's pending prompt (`pendingAction`), when it has a type.
    pub prompt: Option<Prompt>,
    /// Every named card in a player zone or on the stack, players first in
    /// frame order, then the stack's spells. Hidden cards and the sources of
    /// abilities on the stack are dropped (trap 23).
    pub cards: Vec<StateCard>,
    /// `phase` and `step`, such as `MAIN1`.
    pub phase: Option<String>,
    pub step: Option<String>,
    /// `priorityPlayerId`.
    pub priority_player: Option<u32>,
    /// `clock.serverNowMs`: the server's clock when it sent the state.
    pub server_now_ms: Option<i64>,
    /// Every stack entry, spells and abilities, top of the stack first (Forge's order).
    pub stack: Vec<StackEntry>,
}

/// One entry of the stack: a spell or an ability and what it targets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StackEntry {
    pub source_id: u64,
    pub source_name: String,
    pub controller_seat: Option<u32>,
    pub is_ability: bool,
    pub is_trigger: bool,
    /// `abilityDescription`.
    pub text: String,
    /// Target ids; a player target is `-1` (zone `Player`) and carries no
    /// owner, so the targeted player cannot be read from it.
    pub target_ids: Vec<i64>,
    /// `targetDescription`, such as `[Player], [Subtlety (109)]`: the only
    /// place a targeted player is named. Empty when absent.
    pub target_description: String,
}

/// The most counters kept from one card, targets from one stack entry and
/// attached cards from one host.
const MAX_CARD_LIST: usize = 32;
const MAX_TYPES: usize = 16;

/// The most prompt options, card ids and card names kept from one frame.
pub const MAX_PROMPT_CARDS: usize = 500;

/// One player's numbers in a full state. `hand_size` counts the hand even
/// though hidden hand cards are dropped from `GameState.cards`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayerState {
    pub seat: u32,
    pub life: Option<i32>,
    pub hand_size: Option<u32>,
    pub library_size: Option<u32>,
    pub poison: Option<u32>,
}

/// A pending prompt (`pendingAction`): a sideboard, a mulligan bottom, an
/// opening hand choice and so on. `context` is `contextType`, empty when
/// absent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prompt {
    pub kind: String,
    pub context: String,
    pub version: Option<u64>,
    pub main_count: Option<u32>,
    pub options: Vec<PromptCard>,
}

/// One entry of a prompt's `cardOptions`; `id` is the option id the user's
/// answer refers to, not a game card id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptCard {
    pub id: u64,
    pub name: String,
    pub set_code: String,
    pub collector_number: String,
}

/// The zone a state lists a card in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Zone {
    Hand,
    Battlefield,
    Graveyard,
    Exile,
    Stack,
    Command,
    Library,
}

/// One card in a full state. `id` is per game, not a card identity; the
/// printing is empty when Endstep sends none (opponent cards have none).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateCard {
    pub id: u64,
    pub name: String,
    pub zone: Zone,
    pub owner_seat: Option<u32>,
    pub is_token: bool,
    pub face_down: bool,
    /// Whether the card has any `types`. Forge effect objects (such as
    /// "Devourer of Destiny's Effect" in the command zone) have none.
    pub has_types: bool,
    pub set_code: String,
    pub collector_number: String,
    /// `controllerId`: who controls the card, which can differ from the owner.
    pub controller_seat: Option<u32>,
    pub tapped: bool,
    /// Forge sends 0/0 on every non-creature (lands, artifacts, spells), so
    /// a reader must gate on `types` before trusting these.
    pub power: Option<i32>,
    pub toughness: Option<i32>,
    pub types: Vec<String>,
    /// (counter name, count) from the `counters` object; empty for any other
    /// shape.
    pub counters: Vec<(String, u32)>,
    /// The card this one is attached to, set from the host's `attachedCards`.
    pub attached_to: Option<u64>,
}

/// The name Endstep gives a card the viewer cannot see.
const HIDDEN_CARD: &str = "Hidden card";

/// Each player list read for cards, with its zone.
const PLAYER_ZONES: [(&str, Zone); 6] = [
    ("hand", Zone::Hand),
    ("battlefield", Zone::Battlefield),
    ("graveyard", Zone::Graveyard),
    ("exile", Zone::Exile),
    ("commandZone", Zone::Command),
    ("libraryTop", Zone::Library),
];

/// A `GAME_DELTA`: the match score as (game number, wins by seat, is match
/// over), the life changes and the phase and step it carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameDelta {
    pub seq: Option<u64>,
    pub match_score: Option<(u32, Vec<u32>, bool)>,
    /// The delta's top-level `pendingAction`, replacing the state's prompt.
    pub prompt: Option<Prompt>,
    /// (seat, life) from `patch.players[]` entries with `i` and `life`. `i`
    /// is read as the seat; the fixture always lists players in seat order,
    /// so seat and list index agree (an assumption).
    pub life: Vec<(u32, i32)>,
    pub phase: Option<String>,
    pub step: Option<String>,
    /// `patch.state.priorityPlayerId`.
    pub priority_player: Option<u32>,
    /// `patch.state.clock.serverNowMs`.
    pub server_now_ms: Option<i64>,
}

/// A `GAME_EVENT` (`GAME_OUTCOME`, `MULLIGAN`, `HAND_KEPT` and so on). Every
/// kind decodes into the same fields: a `DISCARD_LOG`, for one, has the card
/// (`card_id`, `card_name`) and the player who discarded it (`player_index`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameEvent {
    pub seq: Option<u64>,
    pub game_id: String,
    pub kind: String,
    pub sequence_number: Option<u64>,
    pub player_index: Option<u32>,
    pub amount: Option<u32>,
    pub turn_number: Option<u32>,
    pub end_reason: Option<String>,
    /// The event's log text, such as `"Player cast Expedition Map"`.
    pub message: Option<String>,
    pub card_id: Option<u64>,
    pub card_name: Option<String>,
    pub from_zone: Option<String>,
    pub to_zone: Option<String>,
    /// `zoneOwnerIndex`: whose zone the card moved in or out of.
    pub zone_owner: Option<u32>,
    pub during_mulligan: bool,
    /// `mulligans` on `HAND_SETTLED`.
    pub mulligans: Option<u32>,
    /// True on a reveal of the viewer's own cards.
    pub owner_echo: bool,
    /// `cardIds` and `cardNames`, empty unless both have the same length.
    pub card_ids: Vec<u64>,
    pub card_names: Vec<String>,
    /// `phase` on `TURN_PHASE`.
    pub phase: Option<String>,
    /// `attackerIds` on `ATTACKERS_DECLARED`.
    pub attacker_ids: Vec<u64>,
    /// (blocker id, attacker id) from `blockerAssignments`.
    pub blockers: Vec<(u64, u64)>,
    pub target_card_id: Option<u64>,
    pub target_card_name: Option<String>,
    /// `oldValue` and `newValue` on `PLAYER_LIFE_CHANGED`.
    pub old_value: Option<i32>,
    pub new_value: Option<i32>,
    pub player_name: Option<String>,
    /// `tossPlayerIndex` on `GAME_STARTED`: who chose the starting player
    /// (`playerIndex` is the player who goes first).
    pub toss_player: Option<u32>,
}

/// An outgoing `GAME_ACTION` the user sent: a sideboard answer, a bottom
/// choice, a mulligan decision or a concession.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameAction {
    pub kind: String,
    pub ordered_cards: Vec<u64>,
    pub prompt_version: Option<u64>,
    pub match_id: Option<String>,
}

/// The action types read from outgoing frames.
const ACTION_KINDS: [&str; 5] = [
    "SIDEBOARD_SUBMIT",
    "CHOOSE_CARDS",
    "MULLIGAN",
    "KEEP_HAND",
    "CONCEDE",
];

pub(crate) fn decode_game_socket(body: &Value) -> FrameKind {
    let Some(payload) = body.get("payload") else {
        return FrameKind::Other;
    };
    let seq = json::u64_of(body.get("seq"));
    let match_id = json::string(body, "matchId");
    match body.get("type").and_then(Value::as_str) {
        Some("GAME_STATE") => match game_state(payload, seq, match_id) {
            Some(state) => FrameKind::GameState(state),
            None => FrameKind::Other,
        },
        Some("GAME_DELTA") => FrameKind::GameDelta(game_delta(payload, seq)),
        Some("GAME_EVENT") => match game_event(payload, seq) {
            Some(event) => FrameKind::GameEvent(event),
            None => FrameKind::Other,
        },
        _ => FrameKind::Other,
    }
}

pub(crate) fn decode_lobby_socket(body: &Value) -> FrameKind {
    if body.get("type").and_then(Value::as_str) != Some("GAME_OVER") {
        return FrameKind::Other;
    }
    let Some(payload) = body.get("payload") else {
        return FrameKind::Other;
    };
    let match_id = json::string(body, "matchId");
    match game_state(payload, None, match_id) {
        Some(state) => FrameKind::GameOver(state),
        None => FrameKind::Other,
    }
}

/// A state needs its `gameId`; everything else is optional.
fn game_state(payload: &Value, seq: Option<u64>, match_id: Option<String>) -> Option<GameState> {
    let game_id = json::string(payload, "gameId")?;
    let score = payload.get("matchScore");
    Some(GameState {
        seq,
        match_id,
        game_id,
        game_number: score.and_then(|s| json::u32_of(s.get("gameNumber"))),
        games_played: score.and_then(|s| json::u32_of(s.get("gamesPlayed"))),
        wins_by_seat: json::u32s(score.and_then(|s| s.get("winsBySeat"))),
        is_match_over: score.is_some_and(|s| json::flag(s, "isMatchOver")),
        turn_number: json::u32_of(payload.get("turnNumber")),
        active_player: json::u32_of(payload.get("activePlayerId")),
        winner: json::u32_of(payload.get("winnerId")),
        players: players(payload.get("players")),
        player_states: player_states(payload.get("players")),
        sideboarding: json::flag(payload, "sideboarding"),
        prompt: prompt(payload.get("pendingAction")),
        cards: cards(payload),
        phase: json::string(payload, "phase"),
        step: json::string(payload, "step"),
        priority_player: json::u32_of(payload.get("priorityPlayerId")),
        server_now_ms: json::i64_of(
            payload
                .get("clock")
                .and_then(|clock| clock.get("serverNowMs")),
        ),
        stack: stack(payload.get("stack")),
    })
}

/// The cards of every player zone, then the source cards of the spells on
/// the stack. An ability's source is skipped: it is a card listed in its own
/// zone (battlefield, graveyard and so on), not a card on the stack.
fn cards(payload: &Value) -> Vec<StateCard> {
    let mut out = Vec::new();
    if let Some(Value::Array(players)) = payload.get("players") {
        for player in players {
            for (key, zone) in PLAYER_ZONES {
                if let Some(Value::Array(items)) = player.get(key) {
                    for item in items {
                        push_card(&mut out, item, zone);
                    }
                }
            }
        }
    }
    if let Some(Value::Array(entries)) = payload.get("stack") {
        for entry in entries {
            if json::flag(entry, "isAbility") {
                continue;
            }
            if let Some(card) = entry.get("sourceCard") {
                push_card(&mut out, card, Zone::Stack);
            }
        }
    }
    attach_cards(&mut out, payload);
    out
}

/// Sets `attached_to` on each card named in another card's `attachedCards`;
/// when two hosts name one card, the last one read wins. One pass builds the
/// card to host map and one pass sets it, so a hostile state with many hosts
/// and cards stays linear.
fn attach_cards(cards: &mut [StateCard], payload: &Value) {
    let Some(Value::Array(players)) = payload.get("players") else {
        return;
    };
    let mut host_of: BTreeMap<u64, u64> = BTreeMap::new();
    for player in players {
        for (key, _) in PLAYER_ZONES {
            let Some(Value::Array(items)) = player.get(key) else {
                continue;
            };
            for item in items {
                let Some(host) = json::u64_of(item.get("id")) else {
                    continue;
                };
                for attached in json::u64s_limited(item.get("attachedCards"), MAX_CARD_LIST) {
                    host_of.insert(attached, host);
                }
            }
        }
    }
    if host_of.is_empty() {
        return;
    }
    for card in cards.iter_mut() {
        if let Some(host) = host_of.get(&card.id) {
            card.attached_to = Some(*host);
        }
    }
}

/// Every stack entry that has a source card with a numeric id.
fn stack(value: Option<&Value>) -> Vec<StackEntry> {
    let mut out = Vec::new();
    let Some(Value::Array(entries)) = value else {
        return out;
    };
    for entry in entries {
        if out.len() >= MAX_PROMPT_CARDS {
            break;
        }
        let Some(source) = entry.get("sourceCard") else {
            continue;
        };
        let Some(source_id) = json::u64_of(source.get("id")) else {
            continue;
        };
        out.push(StackEntry {
            source_id,
            source_name: json::string(source, "name").unwrap_or_default(),
            controller_seat: json::u32_of(entry.get("controllerId")),
            is_ability: json::flag(entry, "isAbility"),
            is_trigger: json::flag(entry, "isTrigger"),
            text: json::string(entry, "abilityDescription").unwrap_or_default(),
            target_ids: target_ids(entry.get("targets")),
            target_description: json::string(entry, "targetDescription").unwrap_or_default(),
        });
    }
    out
}

/// The `id` of each target; an entry without a numeric id is skipped.
fn target_ids(value: Option<&Value>) -> Vec<i64> {
    let mut out = Vec::new();
    if let Some(Value::Array(items)) = value {
        for item in items {
            if out.len() >= MAX_CARD_LIST {
                break;
            }
            if let Some(id) = json::i64_of(item.get("id")) {
                out.push(id);
            }
        }
    }
    out
}

/// (name, count) for each entry of a `counters` object with a whole count.
fn counters(value: Option<&Value>) -> Vec<(String, u32)> {
    let mut out = Vec::new();
    if let Some(Value::Object(map)) = value {
        for (name, count) in map {
            if out.len() >= MAX_CARD_LIST {
                break;
            }
            if let Some(count) = json::u32_of(Some(count)) {
                out.push((name.clone(), count));
            }
        }
    }
    out
}

/// The strings of a card's `types`, at most `MAX_TYPES`.
fn card_types(card: &Value) -> Vec<String> {
    json::strings(card.get("types"))
        .into_iter()
        .take(MAX_TYPES)
        .collect()
}

/// Adds a card that has a numeric `id` and a name other than the hidden one.
fn push_card(out: &mut Vec<StateCard>, card: &Value, zone: Zone) {
    let Some(id) = json::u64_of(card.get("id")) else {
        return;
    };
    let Some(name) = json::string(card, "name") else {
        return;
    };
    if name == HIDDEN_CARD {
        return;
    }
    out.push(StateCard {
        id,
        name,
        zone,
        owner_seat: json::u32_of(card.get("ownerId")),
        is_token: json::flag(card, "isToken"),
        face_down: json::flag(card, "faceDown"),
        has_types: has_types(card),
        set_code: json::string(card, "selectedSetCode").unwrap_or_default(),
        collector_number: json::string(card, "selectedCollectorNumber").unwrap_or_default(),
        controller_seat: json::u32_of(card.get("controllerId")),
        tapped: json::flag(card, "tapped"),
        power: json::i32_of(card.get("power")),
        toughness: json::i32_of(card.get("toughness")),
        types: card_types(card),
        counters: counters(card.get("counters")),
        attached_to: None,
    });
}

/// True when `types` is an array with at least one entry.
fn has_types(card: &Value) -> bool {
    match card.get("types") {
        Some(Value::Array(types)) => !types.is_empty(),
        _ => false,
    }
}

/// (seat, name) for every player with a numeric `id` and a string `name`.
fn players(value: Option<&Value>) -> Vec<(u32, String)> {
    let mut out = Vec::new();
    if let Some(Value::Array(items)) = value {
        for item in items {
            let seat = json::u32_of(item.get("id"));
            let name = json::string(item, "name");
            if let (Some(seat), Some(name)) = (seat, name) {
                out.push((seat, name));
            }
        }
    }
    out
}

fn game_delta(payload: &Value, seq: Option<u64>) -> GameDelta {
    let score = payload
        .get("patch")
        .and_then(|patch| patch.get("state"))
        .and_then(|state| state.get("matchScore"));
    let state = payload.get("patch").and_then(|patch| patch.get("state"));
    GameDelta {
        seq,
        match_score: score.and_then(match_score),
        prompt: prompt(payload.get("pendingAction")),
        life: delta_life(payload.get("patch").and_then(|patch| patch.get("players"))),
        phase: state.and_then(|state| json::string(state, "phase")),
        step: state.and_then(|state| json::string(state, "step")),
        priority_player: state.and_then(|state| json::u32_of(state.get("priorityPlayerId"))),
        server_now_ms: json::i64_of(
            state
                .and_then(|state| state.get("clock"))
                .and_then(|clock| clock.get("serverNowMs")),
        ),
    }
}

/// (seat, life) for each `patch.players` entry with both numbers.
fn delta_life(value: Option<&Value>) -> Vec<(u32, i32)> {
    let mut out = Vec::new();
    if let Some(Value::Array(items)) = value {
        for item in items {
            if out.len() >= MAX_PROMPT_CARDS {
                break;
            }
            if let (Some(seat), Some(life)) =
                (json::u32_of(item.get("i")), json::i32_of(item.get("life")))
            {
                out.push((seat, life));
            }
        }
    }
    out
}

/// (blocker id, attacker id) from a `blockerAssignments` object.
fn blocker_pairs(value: Option<&Value>) -> Vec<(u64, u64)> {
    let mut out = Vec::new();
    if let Some(Value::Object(map)) = value {
        for (blocker, attacker) in map {
            if out.len() >= MAX_PROMPT_CARDS {
                break;
            }
            let blocker = blocker.trim().parse().ok();
            if let (Some(blocker), Some(attacker)) = (blocker, json::u64_of(Some(attacker))) {
                out.push((blocker, attacker));
            }
        }
    }
    out
}

/// A delta's score counts only when it names its game number.
fn match_score(score: &Value) -> Option<(u32, Vec<u32>, bool)> {
    let game_number = json::u32_of(score.get("gameNumber"))?;
    let wins = json::u32s(score.get("winsBySeat"));
    let over = json::flag(score, "isMatchOver");
    Some((game_number, wins, over))
}

/// An event needs its `gameId` and `type`.
fn game_event(payload: &Value, seq: Option<u64>) -> Option<GameEvent> {
    let (card_ids, card_names) = reveal_lists(payload);
    Some(GameEvent {
        seq,
        game_id: json::string(payload, "gameId")?,
        kind: json::string(payload, "type")?,
        sequence_number: json::u64_of(payload.get("sequenceNumber")),
        player_index: json::u32_of(payload.get("playerIndex")),
        amount: json::u32_of(payload.get("amount")),
        turn_number: json::u32_of(payload.get("turnNumber")),
        end_reason: json::string(payload, "endReason"),
        message: json::string(payload, "message"),
        card_id: json::u64_of(payload.get("cardId")),
        card_name: json::string(payload, "cardName"),
        from_zone: json::string(payload, "fromZone"),
        to_zone: json::string(payload, "toZone"),
        zone_owner: json::u32_of(payload.get("zoneOwnerIndex")),
        during_mulligan: json::flag(payload, "duringMulligan"),
        mulligans: json::u32_of(payload.get("mulligans")),
        owner_echo: json::flag(payload, "ownerEcho"),
        card_ids,
        card_names,
        phase: json::string(payload, "phase"),
        attacker_ids: json::u64s_limited(payload.get("attackerIds"), MAX_PROMPT_CARDS),
        blockers: blocker_pairs(payload.get("blockerAssignments")),
        target_card_id: json::u64_of(payload.get("targetCardId")),
        target_card_name: json::string(payload, "targetCardName"),
        old_value: json::i32_of(payload.get("oldValue")),
        new_value: json::i32_of(payload.get("newValue")),
        player_name: json::string(payload, "playerName"),
        toss_player: json::u32_of(payload.get("tossPlayerIndex")),
    })
}

/// `cardIds` and `cardNames` as a pair: both empty unless the arrays have the
/// same length and every id and name decodes, then both cut to the cap.
fn reveal_lists(payload: &Value) -> (Vec<u64>, Vec<String>) {
    let (Some(Value::Array(ids)), Some(Value::Array(names))) =
        (payload.get("cardIds"), payload.get("cardNames"))
    else {
        return (Vec::new(), Vec::new());
    };
    if ids.len() != names.len() {
        return (Vec::new(), Vec::new());
    }
    let mut out_ids = Vec::new();
    let mut out_names = Vec::new();
    for (id, name) in ids.iter().zip(names) {
        let (Some(id), Some(name)) = (json::u64_of(Some(id)), name.as_str()) else {
            return (Vec::new(), Vec::new());
        };
        if out_ids.len() < MAX_PROMPT_CARDS {
            out_ids.push(id);
            out_names.push(name.to_string());
        }
    }
    (out_ids, out_names)
}

/// Each player with a numeric `id`: life and the zone sizes Endstep counts.
fn player_states(value: Option<&Value>) -> Vec<PlayerState> {
    let mut out = Vec::new();
    if let Some(Value::Array(items)) = value {
        for item in items {
            if let Some(seat) = json::u32_of(item.get("id")) {
                out.push(PlayerState {
                    seat,
                    life: json::i32_of(item.get("life")),
                    hand_size: json::u32_of(item.get("handSize")),
                    library_size: json::u32_of(item.get("librarySize")),
                    poison: json::u32_of(item.get("poisonCounters")),
                });
            }
        }
    }
    out
}

/// A prompt needs a string `type`; `null` or a missing `pendingAction` is none.
fn prompt(value: Option<&Value>) -> Option<Prompt> {
    let value = value?;
    let kind = json::string(value, "type")?;
    let mut options = Vec::new();
    if let Some(Value::Array(items)) = value.get("cardOptions") {
        for item in items {
            if options.len() >= MAX_PROMPT_CARDS {
                break;
            }
            let (Some(id), Some(name)) = (json::u64_of(item.get("id")), json::string(item, "name"))
            else {
                continue;
            };
            options.push(PromptCard {
                id,
                name,
                set_code: json::string(item, "selectedSetCode").unwrap_or_default(),
                collector_number: json::string(item, "selectedCollectorNumber").unwrap_or_default(),
            });
        }
    }
    Some(Prompt {
        kind,
        context: json::string(value, "contextType").unwrap_or_default(),
        version: json::u64_of(value.get("promptVersion")),
        main_count: json::u32_of(
            value
                .get("sideboardState")
                .and_then(|state| state.get("mainCount")),
        ),
        options,
    })
}

/// An outgoing frame's `GAME_ACTION` body: the `payload`, else the frame body
/// itself, with a `type` in `ACTION_KINDS`.
pub(crate) fn decode_action(body: &Value) -> Option<GameAction> {
    let inner = match body.get("payload") {
        Some(payload) if payload.is_object() => payload,
        _ => body,
    };
    let kind = json::string(inner, "type")?;
    if !ACTION_KINDS.contains(&kind.as_str()) {
        return None;
    }
    Some(GameAction {
        kind,
        ordered_cards: json::u64s_limited(inner.get("orderedCards"), MAX_PROMPT_CARDS),
        prompt_version: json::u64_of(inner.get("promptVersion")),
        match_id: json::string(inner, "matchId").or_else(|| json::string(body, "matchId")),
    })
}
