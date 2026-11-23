//! The parts of a GRE message the fold reads, as Arena writes them. Unknown
//! fields are ignored; a message that does not parse is skipped by the fold.
//! A state's objects, zones, players and annotations are parsed one by one:
//! a bad one is dropped, not the whole state (#572).

use serde::{Deserialize, Deserializer};

pub(crate) const STATE_MESSAGE: &str = "GREMessageType_GameStateMessage";
pub(crate) const QUEUED_STATE_MESSAGE: &str = "GREMessageType_QueuedGameStateMessage";
pub(crate) const GROUP_REQ: &str = "GREMessageType_GroupReq";
pub(crate) const SELECT_N_REQ: &str = "GREMessageType_SelectNReq";
const SURVEIL_CONTEXT: &str = "GroupingContext_Surveil";
const INSTANCE_ID_TYPE: &str = "IdType_InstanceId";
pub(crate) const FULL_STATE: &str = "GameStateType_Full";
pub(crate) const GAME_OVER: &str = "GameStage_GameOver";
pub(crate) const OBJECT_ID_CHANGED: &str = "AnnotationType_ObjectIdChanged";
pub(crate) const ZONE_TRANSFER: &str = "AnnotationType_ZoneTransfer";
pub(crate) const SHUFFLE: &str = "AnnotationType_Shuffle";
pub(crate) const COUNTER: &str = "AnnotationType_Counter";
pub(crate) const ATTACHMENT: &str = "AnnotationType_Attachment";

/// One GRE message (`greToClientMessages[]`).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GreMessage {
    #[serde(rename = "type", default)]
    pub kind: String,
    /// The recipients: the local seat.
    #[serde(default)]
    pub system_seat_ids: Vec<u32>,
    pub game_state_message: Option<RawState>,
    pub die_roll_results_resp: Option<RawDieRolls>,
    /// The state a prompt was sent in (`gameStateId` on the message).
    pub game_state_id: Option<u32>,
    /// `groupReq`: a prompt to split cards into groups (surveil, scry, a
    /// mulligan's bottom cards).
    pub group_req: Option<RawGroupReq>,
    /// `selectNReq`: a prompt to choose cards or objects.
    pub select_n_req: Option<RawSelectNReq>,
}

/// `groupReq`: the parts the fold reads.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RawGroupReq {
    #[serde(default)]
    pub instance_ids: Vec<u32>,
    /// `GroupingContext_Surveil`, `GroupingContext_Scry`, ...
    #[serde(default)]
    pub context: String,
    #[serde(default)]
    pub source_id: u32,
}

/// `selectNReq`: the parts the fold reads.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RawSelectNReq {
    /// The choices.
    #[serde(default)]
    pub ids: Vec<u32>,
    /// Every card shown, choosable or not (a "look at the top N" prompt).
    #[serde(default)]
    pub unfiltered_ids: Vec<u32>,
    /// `IdType_InstanceId` when the ids are instance ids.
    #[serde(default)]
    pub id_type: String,
    #[serde(default)]
    pub source_id: u32,
}

/// `dieRollResultsResp`: each seat's roll for who chooses to play first.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RawDieRolls {
    #[serde(default)]
    pub player_die_rolls: Vec<RawDieRoll>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RawDieRoll {
    #[serde(default)]
    pub system_seat_id: u32,
    #[serde(default)]
    pub roll_value: u32,
}

impl GreMessage {
    /// The cards a prompt to the local player shows them, with the object
    /// whose effect asks (0 when none): a surveil's `GroupReq` or a
    /// `SelectNReq` over instance ids (every card shown, `unfilteredIds`,
    /// when Arena sends it). Scry is read from its annotation instead, and
    /// a mulligan's `GroupReq` shows cards in a hand. Empty otherwise.
    pub fn shown_cards(&self) -> (u32, Vec<u32>) {
        if self.kind == GROUP_REQ
            && let Some(req) = &self.group_req
            && req.context == SURVEIL_CONTEXT
        {
            return (req.source_id, req.instance_ids.clone());
        }
        if self.kind == SELECT_N_REQ
            && let Some(req) = &self.select_n_req
            && req.id_type == INSTANCE_ID_TYPE
        {
            let ids = if req.unfiltered_ids.is_empty() {
                req.ids.clone()
            } else {
                req.unfiltered_ids.clone()
            };
            return (req.source_id, ids);
        }
        (0, Vec::new())
    }

    /// The game state this message carries, when it is a state message.
    pub fn state_mut(&mut self) -> Option<&mut RawState> {
        if self.kind == STATE_MESSAGE || self.kind == QUEUED_STATE_MESSAGE {
            self.game_state_message.as_mut()
        } else {
            None
        }
    }
}

/// A `gameStateMessage`, full or diff.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RawState {
    #[serde(rename = "type", default)]
    pub kind: String,
    #[serde(default)]
    pub game_state_id: u32,
    /// The state this one follows (`prevGameStateId`).
    pub prev_game_state_id: Option<u32>,
    pub game_info: Option<RawGameInfo>,
    #[serde(default, deserialize_with = "each_that_parses")]
    pub game_objects: Vec<RawObject>,
    #[serde(default, deserialize_with = "each_that_parses")]
    pub zones: Vec<RawZone>,
    #[serde(default, deserialize_with = "each_that_parses")]
    pub players: Vec<RawPlayer>,
    pub turn_info: Option<RawTurnInfo>,
    #[serde(default, deserialize_with = "each_that_parses")]
    pub annotations: Vec<RawAnnotation>,
    /// Annotations that stay until deleted (counters, attachments, targets).
    #[serde(default, deserialize_with = "each_that_parses")]
    pub persistent_annotations: Vec<RawAnnotation>,
    #[serde(default)]
    pub diff_deleted_instance_ids: Vec<u32>,
    #[serde(default)]
    pub diff_deleted_persistent_annotation_ids: Vec<u32>,
}

impl RawState {
    pub fn is_full(&self) -> bool {
        self.kind == FULL_STATE
    }

    /// `gameInfo.stage` is `GameStage_GameOver`.
    pub fn is_game_over(&self) -> bool {
        self.game_info
            .as_ref()
            .is_some_and(|info| info.stage == GAME_OVER)
    }
}

/// `gameInfo`: the parts the fold reads.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RawGameInfo {
    #[serde(default)]
    pub stage: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RawObject {
    pub instance_id: u32,
    #[serde(default)]
    pub grp_id: u32,
    /// The other face of a two-faced card.
    #[serde(default)]
    pub otherside_grp_id: u32,
    #[serde(rename = "type", default)]
    pub kind: String,
    #[serde(default)]
    pub zone_id: u32,
    #[serde(default)]
    pub visibility: String,
    #[serde(default)]
    pub owner_seat_id: u32,
    #[serde(default)]
    pub controller_seat_id: u32,
    #[serde(default)]
    pub card_types: Vec<String>,
    #[serde(default)]
    pub subtypes: Vec<String>,
    pub power: Option<RawValue>,
    pub toughness: Option<RawValue>,
    #[serde(default)]
    pub is_tapped: bool,
    #[serde(default)]
    pub attack_state: String,
    #[serde(default)]
    pub block_state: String,
    pub attack_info: Option<RawAttackInfo>,
    pub block_info: Option<RawBlockInfo>,
    /// For an ability: the object it came from.
    #[serde(default)]
    pub parent_id: u32,
    /// `isCopy`: a copy of a card or spell.
    #[serde(default)]
    pub is_copy: bool,
    /// For an ability: the grpId of the card it came from.
    #[serde(default)]
    pub object_source_grp_id: u32,
}

/// A number Arena wraps as `{ "value": n }` (power, toughness). Arena's
/// JSON leaves zero values out, so `{}` is 0.
#[derive(Debug, Deserialize)]
pub(crate) struct RawValue {
    #[serde(default)]
    pub value: i32,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RawAttackInfo {
    /// The attacked player's seat or planeswalker's instance id.
    pub target_id: Option<u32>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RawBlockInfo {
    #[serde(default)]
    pub attacker_ids: Vec<u32>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RawZone {
    pub zone_id: u32,
    #[serde(rename = "type", default)]
    pub kind: String,
    pub owner_seat_id: Option<u32>,
    #[serde(default)]
    pub object_instance_ids: Vec<u32>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RawPlayer {
    pub system_seat_number: u32,
    /// Player entries are sent whole and Arena leaves zero values out, so
    /// an entry without `lifeTotal` is a player at 0 life (seen in real
    /// logs on a `PlayerStatus_PendingLoss` entry after lethal damage).
    #[serde(default)]
    pub life_total: i32,
    pub mulligan_count: Option<u32>,
    /// The client message the GRE waits for from this seat
    /// (`ClientMessageType_MulliganResp`, ...); empty when none.
    #[serde(default)]
    pub pending_message_type: String,
    /// The seat's floating mana; left out when the pool is empty.
    #[serde(default)]
    pub mana_pool: Vec<RawMana>,
}

/// One entry of a player's `manaPool`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RawMana {
    #[serde(default)]
    pub mana_id: u32,
    #[serde(default)]
    pub color: String,
    #[serde(default)]
    pub src_instance_id: u32,
    #[serde(default)]
    pub ability_grp_id: u32,
    #[serde(default)]
    pub count: u32,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RawTurnInfo {
    pub turn_number: Option<u32>,
    pub active_player: Option<u32>,
    #[serde(default)]
    pub phase: String,
    #[serde(default)]
    pub step: String,
    #[serde(default)]
    pub priority_player: u32,
    #[serde(default)]
    pub decision_player: u32,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RawAnnotation {
    #[serde(default)]
    pub id: u32,
    #[serde(default)]
    pub affector_id: u32,
    #[serde(default)]
    pub affected_ids: Vec<u32>,
    #[serde(rename = "type", default)]
    pub kinds: Vec<String>,
    #[serde(default)]
    pub details: Vec<RawDetail>,
}

impl RawAnnotation {
    pub fn is(&self, kind: &str) -> bool {
        self.kinds.iter().any(|k| k == kind)
    }

    /// The first int value of detail `key`.
    pub fn int(&self, key: &str) -> Option<i64> {
        self.detail(key)?.value_int32.first().copied()
    }

    /// Every int value of detail `key`.
    pub fn ints(&self, key: &str) -> &[i64] {
        self.detail(key)
            .map_or(&[], |detail| detail.value_int32.as_slice())
    }

    /// The first string value of detail `key`.
    pub fn text(&self, key: &str) -> Option<&str> {
        self.detail(key)?.value_string.first().map(String::as_str)
    }

    fn detail(&self, key: &str) -> Option<&RawDetail> {
        self.details.iter().find(|detail| detail.key == key)
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RawDetail {
    #[serde(default)]
    pub key: String,
    #[serde(default)]
    pub value_int32: Vec<i64>,
    #[serde(default)]
    pub value_string: Vec<String>,
    /// The detail's other fields: `type` and any other `value*` list.
    #[serde(flatten)]
    pub other: serde_json::Map<String, serde_json::Value>,
}

impl RawDetail {
    /// Every value of the detail, ints first, then strings, then any other
    /// `value*` list (Arena sends one list per detail).
    pub fn values(&self) -> Vec<serde_json::Value> {
        let mut values = Vec::new();
        for int in &self.value_int32 {
            values.push(serde_json::Value::from(*int));
        }
        for text in &self.value_string {
            values.push(serde_json::Value::from(text.clone()));
        }
        for (key, value) in &self.other {
            if key.starts_with("value")
                && let Some(list) = value.as_array()
            {
                values.extend(list.iter().cloned());
            }
        }
        values
    }
}

/// A list's entries that parse, each on its own: a bad entry (an id that is
/// a string or negative) is left out, and anything other than a list is an
/// empty one. Log input is hostile, and one bad object must not cost the
/// whole state: the diffs after it would apply to a stale one.
fn each_that_parses<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: serde::de::DeserializeOwned,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    let serde_json::Value::Array(entries) = value else {
        return Ok(Vec::new());
    };
    let mut parsed = Vec::new();
    for entry in entries {
        if let Ok(item) = serde_json::from_value(entry) {
            parsed.push(item);
        }
    }
    Ok(parsed)
}

/// Parse one GRE message, or `None` when it is not valid JSON of that shape.
pub(crate) fn parse_gre(json: &str) -> Option<GreMessage> {
    serde_json::from_str(json).ok()
}

/// One client message (the `payload` of a `ClientToGREMessage`).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RawClient {
    #[serde(rename = "type", default)]
    pub kind: String,
    pub mulligan_resp: Option<RawMulliganResp>,
    pub choose_starting_player_resp: Option<RawChooseStartingPlayer>,
    pub submit_deck_resp: Option<RawSubmitDeck>,
    pub concede_req: Option<RawConcede>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct RawConcede {
    #[serde(default)]
    pub scope: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct RawMulliganResp {
    #[serde(default)]
    pub decision: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RawChooseStartingPlayer {
    #[serde(default)]
    pub system_seat_id: u32,
}

#[derive(Debug, Deserialize)]
pub(crate) struct RawSubmitDeck {
    pub deck: Option<RawDeck>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RawDeck {
    #[serde(default)]
    pub deck_cards: Vec<u32>,
    #[serde(default)]
    pub sideboard_cards: Vec<u32>,
}

/// Parse one client message, or `None` when it is not valid JSON of that
/// shape.
pub(crate) fn parse_client(json: &str) -> Option<RawClient> {
    serde_json::from_str(json).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A state message whose second object and zone have a bad id (a
    /// string, a negative number) keeps the good ones (#572).
    #[test]
    fn a_bad_object_or_zone_costs_only_itself() {
        let json = r#"{
            "type": "GREMessageType_GameStateMessage",
            "gameStateMessage": {
                "type": "GameStateType_Diff",
                "gameStateId": 7,
                "gameObjects": [
                    { "instanceId": 101, "grpId": 5, "zoneId": 31 },
                    { "instanceId": "bad", "grpId": 6, "zoneId": 31 },
                    { "instanceId": 103, "grpId": 7, "zoneId": -1 },
                    { "instanceId": 104, "grpId": 8, "zoneId": 31 }
                ],
                "zones": [
                    { "zoneId": 31, "type": "ZoneType_Hand", "objectInstanceIds": [101, 104] },
                    { "zoneId": -2, "type": "ZoneType_Library" },
                    { "type": "ZoneType_Graveyard" }
                ]
            }
        }"#;
        let mut message = parse_gre(json).expect("the message parses");
        let state = message.state_mut().expect("a state message");
        assert_eq!(state.game_state_id, 7);
        let objects: Vec<u32> = state.game_objects.iter().map(|o| o.instance_id).collect();
        assert_eq!(objects, vec![101, 104]);
        let zones: Vec<u32> = state.zones.iter().map(|z| z.zone_id).collect();
        assert_eq!(zones, vec![31]);
    }

    /// A bad player entry or annotation costs only itself (#572).
    #[test]
    fn a_bad_player_or_annotation_costs_only_itself() {
        let json = r#"{
            "type": "GREMessageType_GameStateMessage",
            "gameStateMessage": {
                "gameStateId": 9,
                "players": [
                    { "systemSeatNumber": 1, "lifeTotal": 20 },
                    { "systemSeatNumber": "two", "lifeTotal": 20 },
                    { "lifeTotal": 18 }
                ],
                "annotations": [
                    { "id": 5, "type": ["AnnotationType_ZoneTransfer"] },
                    { "id": "bad", "type": ["AnnotationType_ZoneTransfer"] }
                ],
                "persistentAnnotations": [
                    { "id": 6, "type": ["AnnotationType_Counter"] },
                    { "id": 7, "affectedIds": "none" }
                ]
            }
        }"#;
        let mut message = parse_gre(json).expect("the message parses");
        let state = message.state_mut().expect("a state message");
        let seats: Vec<u32> = state.players.iter().map(|p| p.system_seat_number).collect();
        assert_eq!(seats, vec![1]);
        let ids: Vec<u32> = state.annotations.iter().map(|a| a.id).collect();
        assert_eq!(ids, vec![5]);
        let ids: Vec<u32> = state.persistent_annotations.iter().map(|a| a.id).collect();
        assert_eq!(ids, vec![6]);
    }

    /// A list that is not a list at all is no list: the state still parses.
    #[test]
    fn objects_that_are_not_a_list_are_none() {
        let json = r#"{
            "type": "GREMessageType_GameStateMessage",
            "gameStateMessage": { "gameStateId": 8, "gameObjects": 5, "zones": {} }
        }"#;
        let mut message = parse_gre(json).expect("the message parses");
        let state = message.state_mut().expect("a state message");
        assert_eq!(state.game_state_id, 8);
        assert!(state.game_objects.is_empty());
        assert!(state.zones.is_empty());
    }
}
