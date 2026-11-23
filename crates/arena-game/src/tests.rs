use crate::test_support::{fixture_game, fixture_match_game};
use crate::{CardKey, ClientAction, GameInput, Source, fold_game};

fn gre(json: &str) -> GameInput {
    GameInput {
        source: Source::Gre,
        logged_at: 1000,
        json: json.to_string(),
    }
}

const FULL: &str = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[2],
  "gameStateMessage":{"type":"GameStateType_Full","gameStateId":1,
    "players":[{"systemSeatNumber":1,"lifeTotal":20},{"systemSeatNumber":2,"lifeTotal":20}],
    "turnInfo":{"turnNumber":1,"activePlayer":1},
    "zones":[
      {"zoneId":31,"type":"ZoneType_Hand","ownerSeatId":1,"objectInstanceIds":[160]},
      {"zoneId":32,"type":"ZoneType_Library","ownerSeatId":1,"objectInstanceIds":[161,162]},
      {"zoneId":27,"type":"ZoneType_Stack","objectInstanceIds":[]},
      {"zoneId":28,"type":"ZoneType_Battlefield","objectInstanceIds":[]}],
    "gameObjects":[
      {"instanceId":160,"grpId":71291,"type":"GameObjectType_Card","zoneId":31,"visibility":"Visibility_Private","ownerSeatId":1,"controllerSeatId":1,"name":428579},
      {"instanceId":161,"grpId":5,"type":"GameObjectType_Card","zoneId":32,"visibility":"Visibility_Private","ownerSeatId":1,"controllerSeatId":1}]}}"#;

#[test]
fn full_state_then_diff() {
    let diff = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[2],
      "gameStateMessage":{"type":"GameStateType_Diff","gameStateId":2,"prevGameStateId":1,
        "players":[{"systemSeatNumber":2,"lifeTotal":17}],
        "zones":[{"zoneId":28,"type":"ZoneType_Battlefield","objectInstanceIds":[163]}],
        "gameObjects":[
          {"instanceId":163,"grpId":9,"type":"GameObjectType_Card","zoneId":28,"visibility":"Visibility_Public","ownerSeatId":2,"controllerSeatId":2},
          {"instanceId":161,"grpId":5,"type":"GameObjectType_Card","zoneId":32,"visibility":"Visibility_Public","ownerSeatId":1,"controllerSeatId":1}]}}"#;
    let fold = fold_game(&[gre(FULL), gre(diff)]);
    assert_eq!(fold.states.len(), 2);
    let first = &fold.states[0];
    assert_eq!(first.state_id, 1);
    assert_eq!(first.turn, 1);
    assert_eq!(first.active_seat, Some(1));
    assert_eq!(first.objects.len(), 2);
    assert_eq!(first.zones.len(), 4);
    assert_eq!(first.players[&2].life, 20);
    assert!(!first.objects[&161].public);

    let second = &fold.states[1];
    assert_eq!(second.state_id, 2);
    assert_eq!(
        second.turn, 1,
        "turn carries forward when the diff has none"
    );
    assert_eq!(second.active_seat, Some(1));
    assert_eq!(second.players[&1].life, 20);
    assert_eq!(second.players[&2].life, 17);
    assert_eq!(second.zones[&28].object_ids, vec![163]);
    assert_eq!(
        second.zones[&31].object_ids,
        vec![160],
        "untouched zones stay"
    );
    assert_eq!(second.objects.len(), 3);
    assert!(
        second.objects[&161].public,
        "a diff object replaces the whole object"
    );
    assert_eq!(second.objects[&163].kind, "GameObjectType_Card");
    assert_eq!(second.objects[&163].owner_seat, 2);
}

#[test]
fn diff_deletes_instances() {
    let diff = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[2],
      "gameStateMessage":{"type":"GameStateType_Diff","gameStateId":2,"prevGameStateId":1,
        "diffDeletedInstanceIds":[161]}}"#;
    let fold = fold_game(&[gre(FULL), gre(diff)]);
    assert_eq!(fold.states.len(), 2);
    assert!(fold.states[0].objects.contains_key(&161));
    assert!(!fold.states[1].objects.contains_key(&161));
    assert!(fold.states[1].objects.contains_key(&160));
}

fn id_change(state_id: u32, orig: u32, new: u32) -> String {
    format!(
        r#"{{"type":"GREMessageType_GameStateMessage","systemSeatIds":[2],
          "gameStateMessage":{{"type":"GameStateType_Diff","gameStateId":{state_id},
            "annotations":[{{"id":{state_id},"affectorId":2,"affectedIds":[{orig}],
              "type":["AnnotationType_ObjectIdChanged"],
              "details":[{{"key":"orig_id","type":"KeyValuePairValueType_int32","valueInt32":[{orig}]}},
                         {{"key":"new_id","type":"KeyValuePairValueType_int32","valueInt32":[{new}]}}]}}]}}}}"#
    )
}

#[test]
fn follows_object_id_changes() {
    let fold = fold_game(&[
        gre(FULL),
        gre(&id_change(2, 160, 170)),
        gre(&id_change(3, 170, 181)),
    ]);
    assert_eq!(fold.identity.key(160), CardKey(160));
    assert_eq!(fold.identity.key(170), CardKey(160));
    assert_eq!(fold.identity.key(181), CardKey(160));
    assert_eq!(
        fold.identity.key(999),
        CardKey(999),
        "unseen ids are their own card"
    );
}

#[test]
fn zone_transfer_has_category_and_zone_kinds() {
    let diff = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[2],
      "gameStateMessage":{"type":"GameStateType_Diff","gameStateId":2,"prevGameStateId":1,
        "zones":[
          {"zoneId":31,"type":"ZoneType_Hand","ownerSeatId":1,"objectInstanceIds":[]},
          {"zoneId":28,"type":"ZoneType_Battlefield","objectInstanceIds":[170]}],
        "gameObjects":[
          {"instanceId":170,"grpId":71291,"type":"GameObjectType_Card","zoneId":28,"visibility":"Visibility_Public","ownerSeatId":1,"controllerSeatId":1}],
        "diffDeletedInstanceIds":[160],
        "annotations":[
          {"id":7,"affectorId":1,"affectedIds":[160],"type":["AnnotationType_ObjectIdChanged"],
           "details":[{"key":"orig_id","type":"KeyValuePairValueType_int32","valueInt32":[160]},
                      {"key":"new_id","type":"KeyValuePairValueType_int32","valueInt32":[170]}]},
          {"id":8,"affectorId":1,"affectedIds":[170],"type":["AnnotationType_ZoneTransfer"],
           "details":[{"key":"zone_src","type":"KeyValuePairValueType_int32","valueInt32":[31]},
                      {"key":"zone_dest","type":"KeyValuePairValueType_int32","valueInt32":[28]},
                      {"key":"category","type":"KeyValuePairValueType_string","valueString":["PlayLand"]}]}]}}"#;
    let fold = fold_game(&[gre(FULL), gre(diff)]);
    assert_eq!(fold.transfers.len(), 1);
    let transfer = &fold.transfers[0];
    assert_eq!(transfer.state_id, 2);
    assert_eq!(transfer.logged_at, 1000);
    assert_eq!(transfer.card, CardKey(160));
    assert_eq!(transfer.grp_id, 71291);
    assert_eq!(transfer.owner_seat, 1);
    assert_eq!(transfer.from_zone, "ZoneType_Hand");
    assert_eq!(transfer.to_zone, "ZoneType_Battlefield");
    assert_eq!(transfer.category, "PlayLand");
}

#[test]
fn local_seat_from_system_seat_ids() {
    let connect = r#"{"type":"GREMessageType_ConnectResp","systemSeatIds":[2],"connectResp":{}}"#;
    let client = GameInput {
        source: Source::Client,
        logged_at: 1,
        json: r#"{"type":"ClientMessageType_MulliganResp","systemSeatIds":[1]}"#.to_string(),
    };
    let fold = fold_game(&[client, gre(connect), gre(FULL)]);
    assert_eq!(fold.local_seat, Some(2));
    assert_eq!(fold.states.len(), 1, "a connect response is not a state");
}

#[test]
fn malformed_message_is_skipped() {
    let fold = fold_game(&[gre("{not json"), gre(FULL), gre(r#"{"type":"#)]);
    assert_eq!(fold.local_seat, Some(2));
    assert_eq!(fold.states.len(), 1);
}

#[test]
fn queued_state_folds_like_a_state() {
    let queued = r#"{"type":"GREMessageType_QueuedGameStateMessage","systemSeatIds":[2],"gameStateId":2,
      "gameStateMessage":{"type":"GameStateType_Diff","gameStateId":2,"prevGameStateId":1,
        "turnInfo":{"turnNumber":2,"activePlayer":2}}}"#;
    let fold = fold_game(&[gre(FULL), gre(queued)]);
    assert_eq!(fold.states.len(), 2);
    assert_eq!(fold.states[1].turn, 2);
    assert_eq!(fold.states[1].active_seat, Some(2));
    assert_eq!(fold.states[1].objects.len(), 2);
}

/// The last `turnInfo.turnNumber` any message of the game carries.
fn last_turn_number(inputs: &[GameInput]) -> Option<u64> {
    let mut last = None;
    for input in inputs {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&input.json) else {
            continue;
        };
        let turn = value
            .pointer("/gameStateMessage/turnInfo/turnNumber")
            .and_then(|n| n.as_u64());
        if turn.is_some() {
            last = turn;
        }
    }
    last
}

#[test]
fn fixture_game_one_folds() {
    let game = fixture_game("bo3-loss-and-match-concede", 1);
    assert!(!game.inputs.is_empty());
    let fold = fold_game(&game.inputs);
    assert!(game.room_local_seat.is_some());
    assert_eq!(fold.local_seat, game.room_local_seat);

    let last = fold.states.last().expect("game one has states");
    assert_eq!(Some(u64::from(last.turn)), last_turn_number(&game.inputs));
    assert!(last.turn > 1);

    let local = fold.local_seat.unwrap();
    let local_transfers: Vec<_> = fold
        .transfers
        .iter()
        .filter(|t| t.owner_seat == local)
        .collect();
    assert!(local_transfers.iter().any(|t| t.category == "Draw"));
    assert!(local_transfers.iter().any(|t| t.category == "PlayLand"));
    for transfer in &fold.transfers {
        assert!(!transfer.from_zone.is_empty(), "{transfer:?}");
        assert!(!transfer.to_zone.is_empty(), "{transfer:?}");
    }
}

#[test]
fn fixture_three_games_each_fold() {
    for number in 1..=3 {
        let game = fixture_game("bo3-three-games", number);
        let fold = fold_game(&game.inputs);
        assert_eq!(fold.local_seat, game.room_local_seat, "game {number}");
        let last = fold.states.last().expect("every game has states");
        assert_eq!(Some(u64::from(last.turn)), last_turn_number(&game.inputs));
        assert!(fold.transfers.iter().any(|t| t.category == "PlayLand"));
    }
}

#[test]
fn local_seat_skips_broadcast_messages() {
    let timer = r#"{"type":"GREMessageType_TimerStateMessage","systemSeatIds":[1,2]}"#;
    let fold = fold_game(&[gre(timer), gre(FULL)]);
    assert_eq!(fold.local_seat, Some(2));
}

#[test]
fn local_seat_is_none_without_a_single_seat_message() {
    let timer = r#"{"type":"GREMessageType_TimerStateMessage","systemSeatIds":[1,2]}"#;
    let fold = fold_game(&[gre(timer)]);
    assert_eq!(fold.local_seat, None);
}

#[test]
fn fixture_second_match_seat_two_folds() {
    let game = fixture_match_game("bo3-loss-and-match-concede", 1, 1);
    assert_eq!(
        game.room_local_seat,
        Some(2),
        "the local player is seat 2 here"
    );
    let first_gre: serde_json::Value = game
        .inputs
        .iter()
        .filter(|input| input.source == Source::Gre)
        .find_map(|input| serde_json::from_str(&input.json).ok())
        .expect("the game has GRE messages");
    assert_eq!(first_gre["systemSeatIds"], serde_json::json!([2]));
    assert!(
        game.inputs
            .iter()
            .any(|input| input.json.contains(r#""systemSeatIds":[1,2]"#)),
        "the game has a broadcast message"
    );
    let fold = fold_game(&game.inputs);
    assert_eq!(fold.local_seat, game.room_local_seat);
    // Conceded at the mulligan: states, but no turn ever started.
    let last = fold.states.last().expect("the game has states");
    assert_eq!(last_turn_number(&game.inputs), None);
    assert_eq!(last.turn, 0);
    assert!(fold.states.len() >= 5);
    assert_eq!(last.players.len(), 2);
}

#[test]
fn first_full_state_drops_the_previous_games_tail() {
    // The previous game's last states reach this game after its intermission.
    let tail = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[2],
      "gameStateMessage":{"type":"GameStateType_Diff","gameStateId":154,
        "turnInfo":{"turnNumber":7,"activePlayer":2},
        "zones":[{"zoneId":28,"type":"ZoneType_Battlefield","objectInstanceIds":[161]}],
        "gameObjects":[{"instanceId":161,"grpId":9,"type":"GameObjectType_Card","zoneId":28,"visibility":"Visibility_Public","ownerSeatId":2}],
        "annotations":[
          {"id":7,"affectedIds":[500],"type":["AnnotationType_ObjectIdChanged"],
           "details":[{"key":"orig_id","valueInt32":[500]},{"key":"new_id","valueInt32":[161]}]},
          {"id":8,"affectedIds":[161],"type":["AnnotationType_ZoneTransfer"],
           "details":[{"key":"zone_src","valueInt32":[31]},{"key":"zone_dest","valueInt32":[28]},
                      {"key":"category","valueString":["PlayLand"]}]}]}}"#;
    let full = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[2],
      "gameStateMessage":{"type":"GameStateType_Full","gameStateId":1,
        "zones":[{"zoneId":32,"type":"ZoneType_Library","ownerSeatId":1,"objectInstanceIds":[161]}]}}"#;
    let fold = fold_game(&[gre(tail), gre(full)]);
    assert!(fold.started);
    assert_eq!(fold.states.len(), 1, "the tail's state is dropped");
    assert_eq!(fold.states[0].turn, 0, "no turn carries over from the tail");
    assert_eq!(fold.states[0].active_seat, None);
    assert!(fold.transfers.is_empty());
    assert_eq!(fold.identity.key(161), CardKey(161), "ids restart per game");
    assert_eq!(fold.local_seat, Some(2));
}

#[test]
fn a_game_without_a_full_state_has_not_started() {
    let fold = fold_game(&[gre(&id_change(2, 160, 170))]);
    assert!(!fold.started);
    assert_eq!(fold.states.len(), 1);
}

#[test]
fn transfer_knows_its_state_kind_and_front_face() {
    let diff = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[2],
      "gameStateMessage":{"type":"GameStateType_Diff","gameStateId":2,
        "zones":[{"zoneId":28,"type":"ZoneType_Battlefield","objectInstanceIds":[170]}],
        "gameObjects":[
          {"instanceId":170,"grpId":73477,"othersideGrpId":73476,"type":"GameObjectType_Card","zoneId":28,"visibility":"Visibility_Public","ownerSeatId":1}],
        "annotations":[
          {"id":8,"affectedIds":[170],"type":["AnnotationType_ZoneTransfer"],
           "details":[{"key":"zone_src","valueInt32":[31]},{"key":"zone_dest","valueInt32":[28]},
                      {"key":"category","valueString":["PlayLand"]}]}]}}"#;
    let fold = fold_game(&[gre(FULL), gre(diff)]);
    let transfer = &fold.transfers[0];
    assert_eq!(transfer.state_index, 1);
    assert_eq!(transfer.kind, "GameObjectType_Card");
    assert_eq!(
        transfer.grp_id, 73476,
        "a two-faced card is known by its front face"
    );
    assert_eq!(fold.states[1].objects[&170].card_grp_id(), 73476);
}

#[test]
fn mulligan_counts_carry_forward() {
    let mulligan = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[2],
      "gameStateMessage":{"type":"GameStateType_Diff","gameStateId":2,
        "players":[{"systemSeatNumber":1,"mulliganCount":2}]}}"#;
    let later = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[2],
      "gameStateMessage":{"type":"GameStateType_Diff","gameStateId":3,
        "players":[{"systemSeatNumber":2,"lifeTotal":18}]}}"#;
    let fold = fold_game(&[gre(FULL), gre(mulligan), gre(later)]);
    assert_eq!(fold.states[2].mulligans.get(&1), Some(&2));
    assert_eq!(fold.states[2].mulligans.get(&2), None);
}

/// A cast of 160 (hand 31 to stack 27 as 170), cancelled: Arena sends a
/// full state with 160 back in the hand; id 170 is free again (Arena
/// reissues it to the next card that changes zone).
#[test]
fn cancelled_action_is_rolled_back() {
    let cast = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[2],
      "gameStateMessage":{"type":"GameStateType_Diff","gameStateId":2,
        "zones":[{"zoneId":31,"type":"ZoneType_Hand","ownerSeatId":1,"objectInstanceIds":[]},
                 {"zoneId":27,"type":"ZoneType_Stack","objectInstanceIds":[170]}],
        "gameObjects":[{"instanceId":170,"grpId":71291,"type":"GameObjectType_Card","zoneId":27,"visibility":"Visibility_Public","ownerSeatId":1}],
        "annotations":[
          {"id":7,"affectedIds":[160],"type":["AnnotationType_ObjectIdChanged"],
           "details":[{"key":"orig_id","valueInt32":[160]},{"key":"new_id","valueInt32":[170]}]},
          {"id":8,"affectedIds":[170],"type":["AnnotationType_ZoneTransfer"],
           "details":[{"key":"zone_src","valueInt32":[31]},{"key":"zone_dest","valueInt32":[27]},
                      {"key":"category","valueString":["CastSpell"]}]}]}}"#;
    let rollback = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[2],
      "gameStateMessage":{"type":"GameStateType_Full","gameStateId":3,
        "zones":[
          {"zoneId":31,"type":"ZoneType_Hand","ownerSeatId":1,"objectInstanceIds":[160]},
          {"zoneId":32,"type":"ZoneType_Library","ownerSeatId":1,"objectInstanceIds":[161,162]},
          {"zoneId":27,"type":"ZoneType_Stack","objectInstanceIds":[]}],
        "gameObjects":[
          {"instanceId":160,"grpId":71291,"type":"GameObjectType_Card","zoneId":31,"visibility":"Visibility_Private","ownerSeatId":1}]}}"#;
    let fold = fold_game(&[gre(FULL), gre(cast), gre(rollback)]);
    assert!(fold.transfers.is_empty(), "{:?}", fold.transfers);
    assert_eq!(
        fold.states.len(),
        2,
        "the cancelled cast's state is dropped"
    );
    assert_eq!(fold.states[1].state_id, 3);
    assert_eq!(
        fold.identity.key(170),
        CardKey(170),
        "the id change is undone"
    );
}

/// A full state mid-game that undoes nothing (a reconnect) keeps every
/// transfer: the moved card is still under its new id.
#[test]
fn reconnect_full_state_keeps_transfers() {
    let cast = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[2],
      "gameStateMessage":{"type":"GameStateType_Diff","gameStateId":2,
        "zones":[{"zoneId":31,"type":"ZoneType_Hand","ownerSeatId":1,"objectInstanceIds":[]},
                 {"zoneId":27,"type":"ZoneType_Stack","objectInstanceIds":[170]}],
        "gameObjects":[{"instanceId":170,"grpId":71291,"type":"GameObjectType_Card","zoneId":27,"visibility":"Visibility_Public","ownerSeatId":1}],
        "annotations":[
          {"id":7,"affectedIds":[160],"type":["AnnotationType_ObjectIdChanged"],
           "details":[{"key":"orig_id","valueInt32":[160]},{"key":"new_id","valueInt32":[170]}]},
          {"id":8,"affectedIds":[170],"type":["AnnotationType_ZoneTransfer"],
           "details":[{"key":"zone_src","valueInt32":[31]},{"key":"zone_dest","valueInt32":[27]},
                      {"key":"category","valueString":["CastSpell"]}]}]}}"#;
    let reconnect = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[2],
      "gameStateMessage":{"type":"GameStateType_Full","gameStateId":3,
        "zones":[
          {"zoneId":31,"type":"ZoneType_Hand","ownerSeatId":1,"objectInstanceIds":[]},
          {"zoneId":30,"type":"ZoneType_Limbo","objectInstanceIds":[160]},
          {"zoneId":27,"type":"ZoneType_Stack","objectInstanceIds":[170]}],
        "gameObjects":[
          {"instanceId":170,"grpId":71291,"type":"GameObjectType_Card","zoneId":27,"visibility":"Visibility_Public","ownerSeatId":1}]}}"#;
    let fold = fold_game(&[gre(FULL), gre(cast), gre(reconnect)]);
    assert_eq!(fold.transfers.len(), 1);
    assert_eq!(fold.states.len(), 3);
    assert_eq!(fold.identity.key(170), CardKey(160));
}

/// A shuffle renumbers the library; each card keeps its key.
#[test]
fn shuffle_keeps_card_identity() {
    let shuffle = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[2],
      "gameStateMessage":{"type":"GameStateType_Diff","gameStateId":2,
        "annotations":[{"id":9,"affectedIds":[1],"type":["AnnotationType_Shuffle"],
          "details":[{"key":"OldIds","valueInt32":[161,162]},{"key":"NewIds","valueInt32":[180,181]}]}]}}"#;
    let fold = fold_game(&[gre(FULL), gre(&id_change(2, 160, 161)), gre(shuffle)]);
    assert_eq!(fold.identity.key(180), CardKey(160));
    assert_eq!(fold.identity.key(181), CardKey(162));
}

/// A player entry's mana pool is the seat's floating mana; an entry sent
/// without one empties it (player entries are sent whole).
#[test]
fn fold_keeps_mana_pools() {
    let float = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[2],
      "gameStateMessage":{"type":"GameStateType_Diff","gameStateId":2,
        "players":[{"systemSeatNumber":2,"lifeTotal":20,
          "manaPool":[{"manaId":265,"color":"ManaColor_Green","srcInstanceId":561,"abilityGrpId":92295,"count":2}]}]}}"#;
    let spent = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[2],
      "gameStateMessage":{"type":"GameStateType_Diff","gameStateId":3,
        "players":[{"systemSeatNumber":2,"lifeTotal":20}]}}"#;
    let fold = fold_game(&[gre(FULL), gre(float), gre(spent)]);
    let pool = &state_with_id(&fold, 2).mana_pool[&2];
    assert_eq!(
        pool,
        &vec![crate::PoolMana {
            mana_id: 265,
            color: "ManaColor_Green".to_string(),
            src_instance_id: 561,
            ability_grp_id: 92295,
            count: 2,
        }]
    );
    assert!(state_with_id(&fold, 3).mana_pool[&2].is_empty());
}

fn client(json: &str) -> GameInput {
    GameInput {
        source: Source::Client,
        logged_at: 1000,
        json: json.to_string(),
    }
}

/// The last state with game state id `id` (a rolled back state may repeat
/// an id; the kept one comes last).
fn state_with_id(fold: &crate::Fold, id: u32) -> &crate::GameState {
    fold.states
        .iter()
        .rev()
        .find(|state| state.state_id == id)
        .expect("the fold has that state")
}

/// Every client action of a fold, in order.
fn client_actions(fold: &crate::Fold) -> Vec<ClientAction> {
    fold.steps
        .iter()
        .flat_map(|step| step.client.iter().cloned())
        .collect()
}

/// Opponent's Human Werewolf 484 (grpId 78507) in game 1 of the three-games
/// fixture: declared as an attacker in state 105, tapped and attacking
/// player 1 in 106, untapped again in 139 (Arena drops `isTapped`).
#[test]
fn fold_keeps_object_details() {
    let game = fixture_game("bo3-three-games", 1);
    let fold = fold_game(&game.inputs);

    let declared = &state_with_id(&fold, 105).objects[&484];
    assert_eq!(declared.grp_id, 78507);
    assert_eq!(declared.card_types, vec!["CardType_Creature".to_string()]);
    assert_eq!(
        declared.subtypes,
        vec!["SubType_Human".to_string(), "SubType_Werewolf".to_string()]
    );
    assert_eq!(declared.power, Some(3));
    assert_eq!(declared.toughness, Some(3));
    assert!(declared.attacking);
    assert!(!declared.tapped);

    let attacking = &state_with_id(&fold, 106).objects[&484];
    assert!(attacking.tapped);
    assert!(attacking.attacking);
    assert!(!attacking.blocking);
    assert_eq!(attacking.attack_target, Some(1));

    let untapped = &state_with_id(&fold, 139).objects[&484];
    assert!(
        !untapped.tapped,
        "a resent object without isTapped is untapped"
    );
    assert!(!untapped.attacking);
    assert_eq!((untapped.power, untapped.toughness), (Some(3), Some(4)));

    let start = state_with_id(&fold, 1);
    assert_eq!(start.players[&1].life, 20);
    assert_eq!(start.players[&1].library_size, 60);
    assert_eq!(start.players[&2].library_size, 78);
}

#[test]
fn fold_keeps_turn_and_priority() {
    let game = fixture_game("bo3-three-games", 1);
    let fold = fold_game(&game.inputs);

    let attack = state_with_id(&fold, 106);
    assert_eq!(attack.turn, 5);
    assert_eq!(attack.phase, "Phase_Combat");
    assert_eq!(attack.step, "Step_DeclareAttack");
    assert_eq!(attack.active_seat, Some(2));
    assert_eq!(attack.priority_seat, 2);
    assert_eq!(attack.decision_seat, 2);

    let main = state_with_id(&fold, 114);
    assert_eq!(main.phase, "Phase_Main2");
    assert_eq!(main.step, "", "a phase without steps has no step");

    let upkeep = state_with_id(&fold, 118);
    assert_eq!(upkeep.step, "Step_Upkeep");
    assert_eq!(upkeep.active_seat, Some(1));
    assert_eq!(upkeep.priority_seat, 1);
    assert_eq!(upkeep.decision_seat, 0, "no decision player sent");

    assert_eq!(fold.die_rolls, vec![(1, 1), (2, 7)]);
}

#[test]
fn fold_keeps_annotations_in_order() {
    let game = fixture_game("bo3-three-games", 1);
    let fold = fold_game(&game.inputs);
    assert_eq!(fold.steps.len(), fold.states.len());
    for (index, step) in fold.steps.iter().enumerate() {
        assert_eq!(step.state_index, index);
    }

    let first = fold
        .steps
        .iter()
        .flat_map(|step| step.annotations.iter())
        .find(|annotation| {
            annotation
                .types
                .iter()
                .any(|kind| kind == "AnnotationType_ZoneTransfer")
        })
        .expect("a zone transfer");
    assert_eq!(first.id, 160);
    assert_eq!(first.affector_id, 2);
    assert_eq!(first.affected_ids, vec![461]);
    assert_eq!(first.details["category"], vec![serde_json::json!("Put")]);
    assert_eq!(first.details["zone_src"], vec![serde_json::json!(35)]);

    let step = fold
        .steps
        .iter()
        .find(|step| fold.states[step.state_index].state_id == 6)
        .unwrap();
    assert!(
        step.annotations
            .iter()
            .any(|annotation| annotation.id == 160),
        "annotations stay with the state that sent them"
    );
    let later = fold
        .steps
        .iter()
        .find(|step| fold.states[step.state_index].state_id == 7)
        .unwrap();
    assert!(
        later
            .annotations
            .iter()
            .all(|annotation| annotation.id != 160),
        "annotations are not cumulative"
    );
}

#[test]
fn fold_reads_client_actions() {
    let one = fold_game(&fixture_game("bo3-three-games", 1).inputs);
    let mulligans: Vec<ClientAction> = client_actions(&one)
        .into_iter()
        .filter(|action| matches!(action, ClientAction::Mulligan { .. }))
        .collect();
    assert_eq!(
        mulligans,
        vec![
            ClientAction::Mulligan { keep: false },
            ClientAction::Mulligan { keep: true },
        ]
    );
    assert!(client_actions(&one).contains(&ClientAction::Concede { match_scope: false }));
    assert!(client_actions(&one).contains(&ClientAction::Other(
        "ClientMessageType_PerformActionResp".to_string()
    )));

    let two = fold_game(&fixture_game("bo3-three-games", 2).inputs);
    let named: Vec<ClientAction> = client_actions(&two)
        .into_iter()
        .filter(|action| !matches!(action, ClientAction::Other(_)))
        .take(3)
        .collect();
    let main = vec![
        (69396, 4),
        (69793, 2),
        (70385, 1),
        (71820, 4),
        (73472, 4),
        (75061, 4),
        (75350, 4),
        (75662, 3),
        (76652, 3),
        (78598, 2),
        (79088, 4),
        (79448, 3),
        (79464, 3),
        (80165, 4),
        (82390, 3),
        (90348, 2),
        (90615, 4),
        (92090, 3),
        (95052, 3),
    ];
    let side = vec![
        (66175, 2),
        (67530, 2),
        (73905, 3),
        (75662, 1),
        (76465, 2),
        (77123, 2),
        (80397, 1),
        (82055, 2),
    ];
    assert_eq!(
        named,
        vec![
            ClientAction::SubmitDeck { main, side },
            ClientAction::ChooseStartingPlayer { seat: 1 },
            ClientAction::Mulligan { keep: true },
        ]
    );
    assert!(
        two.steps[0]
            .client
            .iter()
            .any(|action| matches!(action, ClientAction::SubmitDeck { .. })),
        "the deck sent before the game's first state belongs to that state"
    );
}

/// Diffs resend objects whole: a dropped `isTapped` or `attackState` means
/// untapped or not attacking. Persistent annotations give counters and
/// attachments; turn info carries forward when a diff has none, and a
/// client action after the last state stays with it.
#[test]
fn fold_reads_synthetic_details() {
    let tapped = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[2],
      "gameStateMessage":{"type":"GameStateType_Diff","gameStateId":2,
        "turnInfo":{"phase":"Phase_Combat","step":"Step_DeclareAttack","turnNumber":3,"activePlayer":2,"priorityPlayer":1,"decisionPlayer":2},
        "gameObjects":[
          {"instanceId":163,"grpId":9,"type":"GameObjectType_Card","zoneId":28,"visibility":"Visibility_Public","ownerSeatId":2,"controllerSeatId":2,
           "cardTypes":["CardType_Creature"],"power":{"value":2},"toughness":{"value":1},"isTapped":true,
           "attackState":"AttackState_Attacking","attackInfo":{"targetId":1}},
          {"instanceId":164,"grpId":10,"type":"GameObjectType_Card","zoneId":28,"visibility":"Visibility_Public","ownerSeatId":2,"controllerSeatId":2,
           "cardTypes":["CardType_Enchantment"],"subtypes":["SubType_Aura"]},
          {"instanceId":165,"grpId":1234,"type":"GameObjectType_Ability","zoneId":27,"visibility":"Visibility_Public","ownerSeatId":2,"controllerSeatId":2,
           "objectSourceGrpId":9,"parentId":163}],
        "persistentAnnotations":[
          {"id":40,"affectorId":4003,"affectedIds":[163],"type":["AnnotationType_Counter"],
           "details":[{"key":"count","valueInt32":[2]},{"key":"counter_type","valueInt32":[1]}]},
          {"id":41,"affectorId":164,"affectedIds":[163],"type":["AnnotationType_Attachment"]}]}}"#;
    let untapped = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[2],
      "gameStateMessage":{"type":"GameStateType_Diff","gameStateId":3,
        "gameObjects":[
          {"instanceId":163,"grpId":9,"type":"GameObjectType_Card","zoneId":28,"visibility":"Visibility_Public","ownerSeatId":2,"controllerSeatId":2,
           "cardTypes":["CardType_Creature"],"power":{"value":2},"toughness":{"value":1}}],
        "diffDeletedPersistentAnnotationIds":[40]}}"#;
    let block = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[2],
      "gameStateMessage":{"type":"GameStateType_Diff","gameStateId":4,
        "gameObjects":[
          {"instanceId":166,"grpId":11,"type":"GameObjectType_Card","zoneId":28,"visibility":"Visibility_Public","ownerSeatId":1,"controllerSeatId":1,
           "cardTypes":["CardType_Creature"],"blockState":"BlockState_Blocking","blockInfo":{"attackerIds":[163]}}]}}"#;
    let die = r#"{"type":"GREMessageType_DieRollResultsResp","systemSeatIds":[1,2],
      "dieRollResultsResp":{"playerDieRolls":[{"systemSeatId":1,"rollValue":12},{"systemSeatId":2,"rollValue":3}]}}"#;
    let fold = fold_game(&[
        gre(die),
        gre(FULL),
        gre(tapped),
        gre(untapped),
        gre(block),
        client(
            r#"{"type":"ClientMessageType_ConcedeReq","concedeReq":{"scope":"MatchScope_Game"}}"#,
        ),
    ]);
    assert_eq!(fold.die_rolls, vec![(1, 12), (2, 3)]);

    let second = &fold.states[1];
    let creature = &second.objects[&163];
    assert!(creature.tapped && creature.attacking);
    assert_eq!(creature.attack_target, Some(1));
    assert_eq!(creature.counters.get("1"), Some(&2));
    assert_eq!(second.objects[&164].attached_to, Some(163));
    let ability = &second.objects[&165];
    assert_eq!(ability.kind, "GameObjectType_Ability");
    assert_eq!(ability.grp_id, 1234);
    assert_eq!(ability.source_grp_id, 9);
    assert_eq!(ability.parent_id, 163);
    assert_eq!(second.priority_seat, 1);

    let third = &fold.states[2];
    let creature = &third.objects[&163];
    assert!(!creature.tapped && !creature.attacking);
    assert!(
        creature.counters.is_empty(),
        "the counter annotation was deleted"
    );
    assert_eq!(third.objects[&164].attached_to, Some(163));
    assert_eq!(third.phase, "Phase_Combat", "turn info carries forward");
    assert_eq!(third.step, "Step_DeclareAttack");
    assert_eq!(third.priority_seat, 1);
    assert_eq!(third.decision_seat, 2);
    assert_eq!(third.players[&1].library_size, 2);
    assert_eq!(third.players[&2].life, 20);

    let blocker = &fold.states[3].objects[&166];
    assert!(blocker.blocking);
    assert_eq!(blocker.block_attackers, vec![163]);
    assert!(!fold.states[3].objects[&163].blocking);

    assert_eq!(
        fold.steps[3].client,
        vec![ClientAction::Concede { match_scope: false }]
    );
    assert_eq!(fold.steps[1].logged_at, 1000);
}

/// The loss fixture's second match: a game concede, then a match concede
/// between games. Each game's concedes, by game number.
#[test]
fn fold_keeps_the_concede_scope() {
    let mut concedes = Vec::new();
    for number in 1..=3 {
        let inputs = fixture_match_game("bo3-loss-and-match-concede", 1, number).inputs;
        for action in client_actions(&fold_game(&inputs)) {
            if let ClientAction::Concede { match_scope } = action {
                concedes.push((number, match_scope));
            }
        }
    }
    assert_eq!(concedes, vec![(1, false), (2, true)]);
}

/// Arena leaves zero values out of its JSON: a player entry without
/// `lifeTotal` (lethal damage) is a player at 0 life.
#[test]
fn fold_reads_a_dropped_life_total_as_zero() {
    let lethal = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[2],
      "gameStateMessage":{"type":"GameStateType_Diff","gameStateId":2,
        "players":[{"systemSeatNumber":1,"status":"PlayerStatus_PendingLoss","teamId":1}]}}"#;
    let fold = fold_game(&[gre(FULL), gre(lethal)]);
    assert_eq!(fold.states[0].players[&1].life, 20);
    assert_eq!(fold.states[1].players[&1].life, 0);
    assert_eq!(fold.states[1].players[&2].life, 20, "seat 2 was not resent");
}

/// Each seat's pending client message, from its player entry: an entry
/// without `pendingMessageType` clears it.
#[test]
fn fold_keeps_pending_messages() {
    let fold = fold_game(&fixture_match_game("bo3-loss-and-match-concede", 1, 1).inputs);
    let pending: Vec<(u32, Option<String>)> = fold
        .states
        .iter()
        .take(3)
        .map(|state| (state.state_id, state.pending.get(&1).cloned()))
        .collect();
    let mulligan = Some("ClientMessageType_MulliganResp".to_string());
    assert_eq!(pending[1], (2, mulligan));
    assert_eq!(
        pending[2],
        (3, Some(String::new())),
        "seat 1 kept in gsId 3"
    );
}

/// A token that leaves the battlefield gets a new id and ceases in the
/// same state: its transfer takes the kind it had under its old id.
#[test]
fn fold_reads_the_kind_of_a_token_that_ceases_as_it_moves() {
    let fold = fold_game(&fixture_match_game("bo3-loss-and-match-concede", 0, 2).inputs);
    let map = fold
        .transfers
        .iter()
        .find(|transfer| transfer.card == CardKey(395))
        .expect("the Map token is sacrificed");
    assert_eq!(map.category, "Sacrifice");
    assert_eq!(map.kind, "GameObjectType_Token");
}

/// Arena's undo: a full state that rewinds to an earlier state (its
/// `prevGameStateId`) sends that state's annotations again, under the same
/// ids (owner data: a shock land's life payment). They were already folded,
/// so the full state's step does not repeat them; the fold keeps each
/// state's `prevGameStateId`.
#[test]
fn undo_full_state_does_not_repeat_annotations() {
    let pay = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[2],
      "gameStateMessage":{"type":"GameStateType_Diff","gameStateId":2,"prevGameStateId":1,
        "players":[{"systemSeatNumber":1,"lifeTotal":18}],
        "annotations":[{"id":7,"affectorId":9010,"affectedIds":[1],"type":["AnnotationType_ModifiedLife"],
          "details":[{"key":"life","valueInt32":[-2]}]}]}}"#;
    let cast = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[2],
      "gameStateMessage":{"type":"GameStateType_Diff","gameStateId":3,"prevGameStateId":2,
        "zones":[{"zoneId":31,"type":"ZoneType_Hand","ownerSeatId":1,"objectInstanceIds":[]},
                 {"zoneId":27,"type":"ZoneType_Stack","objectInstanceIds":[170]}],
        "gameObjects":[{"instanceId":170,"grpId":71291,"type":"GameObjectType_Card","zoneId":27,"visibility":"Visibility_Public","ownerSeatId":1}],
        "annotations":[
          {"id":8,"affectedIds":[160],"type":["AnnotationType_ObjectIdChanged"],
           "details":[{"key":"orig_id","valueInt32":[160]},{"key":"new_id","valueInt32":[170]}]},
          {"id":9,"affectedIds":[170],"type":["AnnotationType_ZoneTransfer"],
           "details":[{"key":"zone_src","valueInt32":[31]},{"key":"zone_dest","valueInt32":[27]},
                      {"key":"category","valueString":["CastSpell"]}]}]}}"#;
    let undo = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[2],
      "gameStateMessage":{"type":"GameStateType_Full","gameStateId":4,"prevGameStateId":2,
        "players":[{"systemSeatNumber":1,"lifeTotal":18},{"systemSeatNumber":2,"lifeTotal":20}],
        "zones":[
          {"zoneId":31,"type":"ZoneType_Hand","ownerSeatId":1,"objectInstanceIds":[160]},
          {"zoneId":32,"type":"ZoneType_Library","ownerSeatId":1,"objectInstanceIds":[161,162]},
          {"zoneId":27,"type":"ZoneType_Stack","objectInstanceIds":[]}],
        "gameObjects":[
          {"instanceId":160,"grpId":71291,"type":"GameObjectType_Card","zoneId":31,"visibility":"Visibility_Private","ownerSeatId":1}],
        "annotations":[{"id":7,"affectorId":9010,"affectedIds":[1],"type":["AnnotationType_ModifiedLife"],
          "details":[{"key":"life","valueInt32":[-2]}]},
          {"id":10,"affectedIds":[1],"type":["AnnotationType_PhaseOrStepModified"],
          "details":[{"key":"phase","valueInt32":[2]},{"key":"step","valueInt32":[0]}]}]}}"#;
    let fold = fold_game(&[gre(FULL), gre(pay), gre(cast), gre(undo)]);
    let ids: Vec<Vec<u32>> = fold
        .steps
        .iter()
        .map(|step| step.annotations.iter().map(|a| a.id).collect())
        .collect();
    assert_eq!(ids, vec![vec![], vec![7], vec![10]]);
    let previous: Vec<Option<u32>> = fold.states.iter().map(|s| s.prev_state_id).collect();
    assert_eq!(previous, vec![None, Some(1), Some(2)]);
    assert!(fold.states[2].full && !fold.states[1].full);
}

/// `gameInfo.stage` `GameStage_GameOver` marks the state and every state
/// after it (Arena sends a few more states as the match ends).
#[test]
fn game_over_stays_once_seen() {
    let over = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[2],
      "gameStateMessage":{"type":"GameStateType_Diff","gameStateId":2,"prevGameStateId":1,
        "gameInfo":{"stage":"GameStage_GameOver"}}}"#;
    let after = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[2],
      "gameStateMessage":{"type":"GameStateType_Diff","gameStateId":3,"prevGameStateId":2}}"#;
    let fold = fold_game(&[gre(FULL), gre(over), gre(after)]);
    let over: Vec<bool> = fold.states.iter().map(|s| s.game_over).collect();
    assert_eq!(over, vec![false, true, true]);
}

fn brawl_summary() -> (crate::Fold, Vec<crate::GameInput>) {
    let game = fixture_game("brawl/brawl_queue", 1);
    (fold_game(&game.inputs), game.inputs)
}

#[test]
fn commanders_come_from_the_first_command_zone() {
    let (fold, inputs) = brawl_summary();
    let summary = crate::summarise_game(&fold, &inputs, None);
    assert_eq!(
        summary.commanders.local,
        vec![cards::ARENA_ID_OFFSET + 83902]
    );
    assert_eq!(
        summary.commanders.opponent,
        vec![cards::ARENA_ID_OFFSET + 73444]
    );
}

#[test]
fn a_later_command_zone_object_is_not_a_commander() {
    let (mut fold, inputs) = brawl_summary();
    let local = fold.local_seat.expect("a local seat");
    let mut state = fold.states.last().expect("states").clone();
    let zone = state
        .zones
        .values_mut()
        .find(|zone| zone.kind == "ZoneType_Command" && zone.owner_seat != Some(local))
        .expect("the opponent's command zone");
    zone.object_ids.push(99_999);
    let mut extra = state
        .objects
        .values()
        .find(|object| object.kind == crate::CARD)
        .expect("a card")
        .clone();
    extra.instance_id = 99_999;
    extra.grp_id = 12_345;
    extra.owner_seat = local + 1;
    state.objects.insert(99_999, extra);
    fold.states.push(state);
    let summary = crate::summarise_game(&fold, &inputs, None);
    assert_eq!(
        summary.commanders.opponent,
        vec![cards::ARENA_ID_OFFSET + 73444]
    );
}

#[test]
fn a_game_with_no_command_zone_has_no_commanders() {
    let game = fixture_game("bo3-three-games", 1);
    let fold = fold_game(&game.inputs);
    let summary = crate::summarise_game(&fold, &game.inputs, None);
    assert_eq!(summary.commanders, crate::Commanders::default());
}
