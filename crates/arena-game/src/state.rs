//! Game states: a full state replaces the current one, a diff applies onto it.
//!
//! What a diff's absence means, checked against the Arena fixtures:
//! - `gameObjects[]`, `zones[]` and `players[]` entries are always sent
//!   whole. A diff that resends an object replaces it, so a field it leaves
//!   out is false or none: Arena drops `isTapped` when a permanent untaps
//!   (161 times in the two fixtures, never `"isTapped": false`), and drops
//!   `attackState`, `blockState`, `attackInfo` and `blockInfo` after combat.
//!   Arena's JSON also leaves zero values out: a player entry without
//!   `lifeTotal` is at 0 life.
//!   An object, zone or player the diff does not resend is unchanged.
//! - `turnInfo` is sent whole too: within it a missing `step`,
//!   `priorityPlayer` or `decisionPlayer` means none (main phases have no
//!   step). A diff without `turnInfo` leaves the turn as it was; `turnNumber`
//!   and `activePlayer` carry forward when missing (pregame sends no
//!   `turnNumber`).
//! - `persistentAnnotations[]` add or replace by id and stay until
//!   `diffDeletedPersistentAnnotationIds` names them. Counters and
//!   attachments come from them (objects carry neither).

use std::collections::BTreeMap;

use crate::message::{ATTACHMENT, COUNTER, RawAnnotation, RawObject, RawState, RawZone};

const LIBRARY: &str = "ZoneType_Library";

/// One annotation (`annotations[]` or `persistentAnnotations[]`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Annotation {
    pub id: u32,
    /// `AnnotationType_*`.
    pub types: Vec<String>,
    /// The object or seat that caused it (0 when none).
    pub affector_id: u32,
    pub affected_ids: Vec<u32>,
    /// Each detail's values by key (`valueInt32` as numbers, `valueString`
    /// as strings).
    pub details: BTreeMap<String, Vec<serde_json::Value>>,
}

impl Annotation {
    pub fn is(&self, kind: &str) -> bool {
        self.types.iter().any(|k| k == kind)
    }

    /// The first value of detail `key` as an int.
    pub fn int(&self, key: &str) -> Option<i64> {
        self.details.get(key)?.first()?.as_i64()
    }

    /// Every value of detail `key` that is an int, in order.
    pub fn ints(&self, key: &str) -> Vec<i64> {
        self.details
            .get(key)
            .map(|values| values.iter().filter_map(|value| value.as_i64()).collect())
            .unwrap_or_default()
    }
}

pub(crate) fn annotation_from(raw: &RawAnnotation) -> Annotation {
    let mut details: BTreeMap<String, Vec<serde_json::Value>> = BTreeMap::new();
    for detail in &raw.details {
        details
            .entry(detail.key.clone())
            .or_default()
            .extend(detail.values());
    }
    Annotation {
        id: raw.id,
        types: raw.kinds.clone(),
        affector_id: raw.affector_id,
        affected_ids: raw.affected_ids.clone(),
        details,
    }
}

/// One player as of a state.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlayerState {
    pub life: i32,
    /// Cards in the seat's library zone.
    pub library_size: u32,
}

/// Floating mana in a player's pool (`players[].manaPool[]`): `count` mana
/// of one `ManaColor_*`, added by ability `ability_grp_id` of instance
/// `src_instance_id`. Arena keeps the mana's id while it floats.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PoolMana {
    pub mana_id: u32,
    pub color: String,
    pub src_instance_id: u32,
    pub ability_grp_id: u32,
    pub count: u32,
}

/// One game object (`gameObjects[]`): a card, token, ability and so on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameObject {
    pub instance_id: u32,
    pub grp_id: u32,
    /// The other face of a two-faced card (0 for a one-faced card).
    pub otherside_grp_id: u32,
    /// `GameObjectType_*`.
    pub kind: String,
    pub zone_id: u32,
    pub owner_seat: u32,
    pub controller_seat: u32,
    /// `visibility` is `Visibility_Public`.
    pub public: bool,
    /// `CardType_*`.
    pub card_types: Vec<String>,
    /// `SubType_*`.
    pub subtypes: Vec<String>,
    pub power: Option<i32>,
    pub toughness: Option<i32>,
    pub tapped: bool,
    /// Declared or attacking (`AttackState_Declared`, `AttackState_Attacking`).
    pub attacking: bool,
    /// Attacking (`AttackState_Attacking`): the declaration was submitted.
    /// `AttackState_Declared` is a choice the attacker can still change.
    pub attack_submitted: bool,
    /// What it attacks (`attackInfo.targetId`): a seat or an instance id.
    pub attack_target: Option<u32>,
    /// Declared or blocking (`BlockState_Declared`, `BlockState_Blocking`;
    /// `Blocked` and `Unblocked` describe an attacker).
    pub blocking: bool,
    /// Blocking (`BlockState_Blocking`): the declaration was submitted.
    pub block_submitted: bool,
    /// The attackers it blocks (`blockInfo.attackerIds`).
    pub block_attackers: Vec<u32>,
    /// Counters by Arena's `counter_type` number (as text), from the
    /// `Counter` persistent annotations.
    pub counters: BTreeMap<String, u32>,
    /// The object it is attached to, from an `Attachment` persistent
    /// annotation.
    pub attached_to: Option<u32>,
    /// `objectSourceGrpId`: an ability's source card (0 when not sent).
    pub source_grp_id: u32,
    /// `parentId`: an ability's source object (0 when not sent).
    pub parent_id: u32,
    /// `isCopy`: a copy of a card or spell (CR 707.10).
    pub is_copy: bool,
}

impl GameObject {
    /// The card's grpId, by its front face: a two-faced card shows the face
    /// it is on, and Arena numbers the back face after the front (decks
    /// list the front).
    pub fn card_grp_id(&self) -> u32 {
        front_face(self.grp_id, self.otherside_grp_id)
    }
}

/// The front face of a card shown as `grp_id` with other face
/// `otherside_grp_id` (0 for none): the smaller id, as Arena numbers the
/// back face after the front. A heuristic: callers that know the deck
/// prefer the face the deck lists.
pub fn front_face(grp_id: u32, otherside_grp_id: u32) -> u32 {
    if otherside_grp_id != 0 && otherside_grp_id < grp_id {
        otherside_grp_id
    } else {
        grp_id
    }
}

/// One zone (`zones[]`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Zone {
    pub zone_id: u32,
    /// `ZoneType_*`.
    pub kind: String,
    pub owner_seat: Option<u32>,
    pub object_ids: Vec<u32>,
}

/// The game as of one state message.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GameState {
    pub state_id: u32,
    /// `prevGameStateId`: the state this one follows, when sent.
    pub prev_state_id: Option<u32>,
    /// The message was a full state (`GameStateType_Full`).
    pub full: bool,
    /// `gameInfo.stage` was `GameStage_GameOver` in this state or an
    /// earlier one (Arena sends a few states more as a match ends).
    pub game_over: bool,
    /// The last `turnInfo.turnNumber` seen (0 before the first turn).
    pub turn: u32,
    /// The last `turnInfo.activePlayer` seen.
    pub active_seat: Option<u32>,
    /// Objects by instance id.
    pub objects: BTreeMap<u32, GameObject>,
    /// Zones by zone id.
    pub zones: BTreeMap<u32, Zone>,
    /// Mulligans taken by seat (`players[].mulliganCount`), when stated.
    pub mulligans: BTreeMap<u32, u32>,
    /// The client message each seat with a player entry is to send
    /// (`players[].pendingMessageType`, for example
    /// `ClientMessageType_MulliganResp`); empty when none. Player entries
    /// are sent whole, so an entry without it clears it.
    pub pending: BTreeMap<u32, String>,
    /// Each seat with a known life total (kept across diffs), with its
    /// library size.
    pub players: BTreeMap<u32, PlayerState>,
    /// Each seat's floating mana. Player entries are sent whole, so an
    /// entry without a pool empties it.
    pub mana_pool: BTreeMap<u32, Vec<PoolMana>>,
    /// `turnInfo.phase` (`Phase_*`), empty before the first turn.
    pub phase: String,
    /// `turnInfo.step` (`Step_*`), empty in a phase without steps.
    pub step: String,
    /// `turnInfo.priorityPlayer` (0 for none).
    pub priority_seat: u32,
    /// `turnInfo.decisionPlayer` (0 for none).
    pub decision_seat: u32,
    /// The persistent annotations in force, by id.
    pub persistent: BTreeMap<u32, Annotation>,
}

impl GameState {
    /// The state after `raw`: a full state replaces this one, a diff applies
    /// onto it (zones and objects replaced whole by id, deleted instances
    /// removed). Turn info and persistent annotations follow the rules in
    /// this module's header.
    pub(crate) fn next(&self, raw: &RawState) -> GameState {
        let mut state = if raw.is_full() {
            GameState {
                turn: self.turn,
                active_seat: self.active_seat,
                phase: self.phase.clone(),
                step: self.step.clone(),
                priority_seat: self.priority_seat,
                decision_seat: self.decision_seat,
                ..GameState::default()
            }
        } else {
            self.clone()
        };
        state.state_id = raw.game_state_id;
        state.prev_state_id = raw.prev_game_state_id;
        state.full = raw.is_full();
        state.game_over = self.game_over || raw.is_game_over();
        if let Some(turn_info) = &raw.turn_info {
            if let Some(turn) = turn_info.turn_number {
                state.turn = turn;
            }
            if let Some(seat) = turn_info.active_player {
                state.active_seat = Some(seat);
            }
            state.phase = turn_info.phase.clone();
            state.step = turn_info.step.clone();
            state.priority_seat = turn_info.priority_player;
            state.decision_seat = turn_info.decision_player;
        }
        for zone in &raw.zones {
            state.zones.insert(zone.zone_id, zone_from(zone));
        }
        for object in &raw.game_objects {
            state
                .objects
                .insert(object.instance_id, object_from(object));
        }
        for id in &raw.diff_deleted_instance_ids {
            state.objects.remove(id);
        }
        for player in &raw.players {
            state
                .players
                .entry(player.system_seat_number)
                .or_default()
                .life = player.life_total;
            state.pending.insert(
                player.system_seat_number,
                player.pending_message_type.clone(),
            );
            if let Some(count) = player.mulligan_count {
                state.mulligans.insert(player.system_seat_number, count);
            }
            let pool = player
                .mana_pool
                .iter()
                .map(|mana| PoolMana {
                    mana_id: mana.mana_id,
                    color: mana.color.clone(),
                    src_instance_id: mana.src_instance_id,
                    ability_grp_id: mana.ability_grp_id,
                    count: mana.count,
                })
                .collect();
            state.mana_pool.insert(player.system_seat_number, pool);
        }
        for id in &raw.diff_deleted_persistent_annotation_ids {
            state.persistent.remove(id);
        }
        for annotation in &raw.persistent_annotations {
            state
                .persistent
                .insert(annotation.id, annotation_from(annotation));
        }
        state.apply_persistent();
        state.refresh_library_sizes();
        state
    }

    /// Set each object's counters and attachment from the persistent
    /// annotations in force.
    fn apply_persistent(&mut self) {
        for object in self.objects.values_mut() {
            object.counters.clear();
            object.attached_to = None;
        }
        for annotation in self.persistent.values() {
            if annotation.is(COUNTER) {
                let kind = annotation.int("counter_type").unwrap_or(0);
                let count = annotation
                    .int("count")
                    .and_then(|count| u32::try_from(count).ok())
                    .unwrap_or(0);
                for id in &annotation.affected_ids {
                    if let Some(object) = self.objects.get_mut(id) {
                        object.counters.insert(kind.to_string(), count);
                    }
                }
            } else if annotation.is(ATTACHMENT)
                && let Some(object) = self.objects.get_mut(&annotation.affector_id)
            {
                object.attached_to = annotation.affected_ids.first().copied();
            }
        }
    }

    /// Set each known seat's library size from the size of its library zone.
    fn refresh_library_sizes(&mut self) {
        let sizes: Vec<(u32, u32)> = self
            .players
            .keys()
            .map(|seat| {
                let size = self
                    .zones
                    .values()
                    .filter(|zone| zone.kind == LIBRARY && zone.owner_seat == Some(*seat))
                    .map(|zone| zone.object_ids.len())
                    .sum::<usize>();
                (*seat, u32::try_from(size).unwrap_or(u32::MAX))
            })
            .collect();
        for (seat, size) in sizes {
            if let Some(player) = self.players.get_mut(&seat) {
                player.library_size = size;
            }
        }
    }

    /// The kind (`ZoneType_*`) of zone `zone_id`, empty when unknown.
    pub fn zone_kind(&self, zone_id: u32) -> String {
        self.zones
            .get(&zone_id)
            .map(|zone| zone.kind.clone())
            .unwrap_or_default()
    }
}

fn zone_from(raw: &RawZone) -> Zone {
    Zone {
        zone_id: raw.zone_id,
        kind: raw.kind.clone(),
        owner_seat: raw.owner_seat_id,
        object_ids: raw.object_instance_ids.clone(),
    }
}

fn object_from(raw: &RawObject) -> GameObject {
    GameObject {
        instance_id: raw.instance_id,
        grp_id: raw.grp_id,
        otherside_grp_id: raw.otherside_grp_id,
        kind: raw.kind.clone(),
        zone_id: raw.zone_id,
        owner_seat: raw.owner_seat_id,
        controller_seat: raw.controller_seat_id,
        public: raw.visibility == "Visibility_Public",
        card_types: raw.card_types.clone(),
        subtypes: raw.subtypes.clone(),
        power: raw.power.as_ref().map(|power| power.value),
        toughness: raw.toughness.as_ref().map(|toughness| toughness.value),
        tapped: raw.is_tapped,
        attacking: raw.attack_state == "AttackState_Declared"
            || raw.attack_state == "AttackState_Attacking",
        attack_submitted: raw.attack_state == "AttackState_Attacking",
        attack_target: raw.attack_info.as_ref().and_then(|info| info.target_id),
        blocking: raw.block_state == "BlockState_Declared"
            || raw.block_state == "BlockState_Blocking",
        block_submitted: raw.block_state == "BlockState_Blocking",
        block_attackers: raw
            .block_info
            .as_ref()
            .map(|info| info.attacker_ids.clone())
            .unwrap_or_default(),
        counters: BTreeMap::new(),
        attached_to: None,
        source_grp_id: raw.object_source_grp_id,
        parent_id: raw.parent_id,
        is_copy: raw.is_copy,
    }
}
