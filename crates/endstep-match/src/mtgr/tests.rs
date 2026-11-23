//! The diff and keyframe paths the committed capture never shows, on small
//! made-up states. The user is seat 0 (`Player`), seat 1 is `Opponent`.

use std::collections::BTreeMap;

use ::mtgr::{
    CardType, Cause, Entering, LibraryEnd, MoveAction, ObjRef, Position, Statement, Zone,
};
use endstep_protocol::{GameState, PlayerState, StateCard, Zone as StateZone};

use super::MAX_OBJECTS;
use super::board::{Board, Written};
use super::shown::shown_cards;

pub(super) const PLAYER: &str = "Player";
pub(super) const OPPONENT: &str = "Opponent";

pub(super) fn board() -> Board {
    let mut names = BTreeMap::new();
    names.insert(0, PLAYER.to_string());
    names.insert(1, OPPONENT.to_string());
    Board::new(0, names)
}

/// (hand size, library size) for a seat at 20 life.
pub(super) fn player(seat: u32, hand: u32, library: u32) -> PlayerState {
    PlayerState {
        seat,
        life: Some(20),
        hand_size: Some(hand),
        library_size: Some(library),
        poison: Some(0),
    }
}

/// A full state of turn 3 with the user's and the opponent's (hand, library)
/// and the cards listed.
pub(super) fn state(user: (u32, u32), opponent: (u32, u32), cards: Vec<StateCard>) -> GameState {
    GameState {
        seq: None,
        match_id: None,
        game_id: "game".to_string(),
        game_number: Some(1),
        games_played: None,
        wins_by_seat: Vec::new(),
        is_match_over: false,
        turn_number: Some(3),
        active_player: Some(0),
        winner: None,
        players: vec![(0, PLAYER.to_string()), (1, OPPONENT.to_string())],
        player_states: vec![player(0, user.0, user.1), player(1, opponent.0, opponent.1)],
        sideboarding: false,
        prompt: None,
        cards,
        phase: Some("MAIN1".to_string()),
        step: Some("MAIN1".to_string()),
        priority_player: Some(0),
        server_now_ms: None,
        stack: Vec::new(),
    }
}

pub(super) fn card(id: u64, name: &str, zone: StateZone, owner: u32, types: &[&str]) -> StateCard {
    StateCard {
        id,
        name: name.to_string(),
        zone,
        owner_seat: Some(owner),
        is_token: false,
        face_down: false,
        has_types: true,
        set_code: String::new(),
        collector_number: String::new(),
        controller_seat: Some(owner),
        tapped: false,
        power: Some(0),
        toughness: Some(0),
        types: types.iter().map(|name| name.to_string()).collect(),
        counters: Vec::new(),
        attached_to: None,
    }
}

pub(super) fn permanent(id: u64, name: &str, owner: u32, types: &[&str]) -> StateCard {
    card(id, name, StateZone::Battlefield, owner, types)
}

/// A board that joined the game at `first`: its objects are introduced where
/// they are.
fn joined(first: &GameState) -> Board {
    let mut board = board();
    board.start(first, false);
    board.diff(first);
    board
}

pub(super) fn obj(id: u32, name: &str) -> ObjRef {
    ObjRef {
        id,
        name: Some(name.to_string()),
    }
}

pub(super) fn unknown(statement: Statement) -> Written {
    (statement, Some(Cause::Unknown))
}

/// The ids of the hidden objects a keyframe lists in the opponent's hand.
fn hidden_hand(entries: &[Statement]) -> Vec<u32> {
    let mut ids = Vec::new();
    for entry in entries {
        if let Statement::KeyframeObject {
            object,
            zone: Zone::Hand,
            player: Some(player),
            ..
        } = entry
            && object.name.is_none()
            && player == OPPONENT
        {
            ids.push(object.id);
        }
    }
    ids
}

#[test]
fn a_hidden_hand_that_shrank_unseen_is_repaired_at_the_keyframe() {
    let mut board = joined(&state((0, 50), (3, 40), Vec::new()));
    assert!(!board.drifted());
    assert_eq!(hidden_hand(&board.keyframe()), vec![1, 2, 3]);

    // The hand shrinks by one, the library is unchanged and nothing new is
    // named: a card went somewhere Forge hides (foretold, face down).
    let written = board.diff(&state((0, 50), (2, 40), Vec::new()));
    assert_eq!(written, Vec::new());
    assert!(board.drifted());

    // The keyframe after the `incomplete` ends the lowest hidden object.
    assert_eq!(hidden_hand(&board.keyframe()), vec![2, 3]);
    assert!(!board.drifted());
    // The record and Forge agree again: the same state is no drift.
    assert_eq!(board.diff(&state((0, 50), (2, 40), Vec::new())), Vec::new());
    assert!(!board.drifted());
}

#[test]
fn a_spell_being_paid_for_is_named_when_the_stack_shows_it() {
    let mut board = joined(&state((0, 50), (2, 40), Vec::new()));
    // Forge lists the card nowhere while its cost is paid.
    assert_eq!(board.diff(&state((0, 50), (1, 40), Vec::new())), Vec::new());
    assert!(board.drifted());

    let bolt = card(9, "Lightning Bolt", StateZone::Stack, 1, &["Instant"]);
    let written = board.diff(&state((0, 50), (1, 40), vec![bolt]));
    assert_eq!(
        written,
        vec![
            (
                Statement::Identify {
                    id: 1,
                    name: Some("Lightning Bolt".to_string()),
                    sources: Vec::new(),
                },
                None
            ),
            unknown(Statement::Move {
                object: obj(1, "Lightning Bolt"),
                from: Zone::Hand,
                to: Zone::Stack,
                action: None,
                position: None,
                entering: None,
                with: None,
            }),
        ]
    );
    assert!(!board.drifted());
}

#[test]
fn counters_are_written_by_the_standards_names() {
    let bear = permanent(5, "Bear", 0, &["Creature"]);
    let mut board = joined(&state((0, 50), (0, 40), vec![bear.clone()]));

    let mut grown = bear.clone();
    grown.counters = vec![("P1P1".to_string(), 2), ("STUN".to_string(), 1)];
    let written = board.diff(&state((0, 50), (0, 40), vec![grown.clone()]));
    assert_eq!(
        written,
        vec![
            unknown(Statement::Counters {
                object: obj(1, "Bear"),
                kind: "+1/+1".to_string(),
                from: 0,
                to: 2,
            }),
            unknown(Statement::Counters {
                object: obj(1, "Bear"),
                kind: "stun".to_string(),
                from: 0,
                to: 1,
            }),
        ]
    );

    grown.counters = vec![("P1P1".to_string(), 1), ("M1M1".to_string(), 1)];
    let written = board.diff(&state((0, 50), (0, 40), vec![grown]));
    assert_eq!(
        written,
        vec![
            unknown(Statement::Counters {
                object: obj(1, "Bear"),
                kind: "+1/+1".to_string(),
                from: 2,
                to: 1,
            }),
            unknown(Statement::Counters {
                object: obj(1, "Bear"),
                kind: "-1/-1".to_string(),
                from: 0,
                to: 1,
            }),
            unknown(Statement::Counters {
                object: obj(1, "Bear"),
                kind: "stun".to_string(),
                from: 1,
                to: 0,
            }),
        ]
    );
}

#[test]
fn a_control_change_is_written_and_kept_in_the_keyframe() {
    let bear = permanent(5, "Bear", 0, &["Creature"]);
    let mut board = joined(&state((0, 50), (0, 40), vec![bear.clone()]));

    let mut stolen = bear;
    stolen.controller_seat = Some(1);
    let written = board.diff(&state((0, 50), (0, 40), vec![stolen]));
    assert_eq!(
        written,
        vec![unknown(Statement::Control {
            object: obj(1, "Bear"),
            player: OPPONENT.to_string(),
        })]
    );
    let listed_under = board.keyframe().into_iter().find_map(|entry| match entry {
        Statement::KeyframeObject { player, .. } => player,
        _ => None,
    });
    assert_eq!(listed_under.as_deref(), Some(OPPONENT));
}

#[test]
fn an_attachment_is_written_and_then_ended() {
    let bear = permanent(5, "Bear", 0, &["Creature"]);
    let sword = permanent(6, "Sword", 0, &["Artifact"]);
    let mut board = joined(&state((0, 50), (0, 40), vec![bear.clone(), sword.clone()]));

    let mut worn = sword.clone();
    worn.attached_to = Some(5);
    let written = board.diff(&state((0, 50), (0, 40), vec![bear.clone(), worn]));
    assert_eq!(
        written,
        vec![unknown(Statement::Attach {
            object: obj(2, "Sword"),
            to: obj(1, "Bear"),
        })]
    );
    let attached = board.keyframe().into_iter().find_map(|entry| match entry {
        Statement::KeyframeObject { attached, .. } => attached,
        _ => None,
    });
    assert_eq!(attached, Some(1));

    let written = board.diff(&state((0, 50), (0, 40), vec![bear, sword]));
    assert_eq!(
        written,
        vec![unknown(Statement::Unattach {
            object: obj(2, "Sword"),
        })]
    );
}

#[test]
fn a_user_card_listed_nowhere_goes_back_when_the_library_grew() {
    let forest = card(5, "Forest", StateZone::Hand, 0, &["Land"]);
    let mut board = joined(&state((1, 50), (0, 40), vec![forest]));

    // Listed nowhere with the library unchanged: it stays (a cost is paid).
    assert_eq!(board.diff(&state((0, 50), (0, 40), Vec::new())), Vec::new());

    let written = board.diff(&state((0, 51), (0, 40), Vec::new()));
    assert_eq!(
        written,
        vec![unknown(Statement::Move {
            object: obj(1, "Forest"),
            from: Zone::Hand,
            to: Zone::Library,
            action: None,
            position: None,
            entering: None,
            with: None,
        })]
    );
    assert!(!board.drifted());
}

#[test]
fn a_hidden_card_goes_into_a_library_that_grew() {
    let mut board = joined(&state((0, 50), (3, 40), Vec::new()));
    let written = board.diff(&state((0, 50), (2, 41), Vec::new()));
    assert_eq!(
        written,
        vec![unknown(Statement::Move {
            object: ObjRef { id: 1, name: None },
            from: Zone::Hand,
            to: Zone::Library,
            action: None,
            position: None,
            entering: None,
            with: None,
        })]
    );
    assert!(!board.drifted());
    assert_eq!(hidden_hand(&board.keyframe()), vec![2, 3]);
}

fn new_card_in_library(id: u32, name: &str) -> Written {
    (
        Statement::NewCard {
            object: obj(id, name),
            owner: PLAYER.to_string(),
            zone: Zone::Library,
            position: None,
            entering: None,
            sources: Vec::new(),
        },
        None,
    )
}

fn plain_move(id: u32, name: &str, from: Zone, to: Zone, action: Option<MoveAction>) -> Statement {
    Statement::Move {
        object: obj(id, name),
        from,
        to,
        action,
        position: None,
        entering: None,
        with: None,
    }
}

/// A board in a turn (a draw is no longer an opening hand) that joined at
/// `first`.
fn in_a_turn(first: &GameState) -> Board {
    let mut board = joined(first);
    board.end_pregame();
    board
}

#[test]
fn a_card_a_reveal_named_is_found_not_drawn() {
    let mut board = in_a_turn(&state((0, 50), (0, 40), Vec::new()));
    let forest = card(5, "Forest", StateZone::Hand, 0, &["Land"]);
    let island = card(6, "Island", StateZone::Hand, 0, &["Land"]);
    let swamp = card(7, "Swamp", StateZone::Hand, 0, &["Land"]);
    let draw = Some(MoveAction::Draw);

    // Nothing says otherwise: a card out of the library into the hand is
    // drawn.
    let written = board.diff(&state((1, 49), (0, 40), vec![forest.clone()]));
    assert_eq!(
        written,
        vec![
            new_card_in_library(1, "Forest"),
            unknown(plain_move(1, "Forest", Zone::Library, Zone::Hand, draw)),
        ]
    );

    // A reveal into the hand named the card: a search found it. The move
    // has no action and takes the cause of what resolved.
    board.hints.found.insert(6);
    board.hints.cause = Some(Cause::Object(1));
    let cards = vec![forest.clone(), island.clone()];
    let written = board.diff(&state((2, 48), (0, 40), cards));
    assert_eq!(
        written,
        vec![
            new_card_in_library(2, "Island"),
            (
                plain_move(2, "Island", Zone::Library, Zone::Hand, None),
                Some(Cause::Object(1))
            ),
        ]
    );

    // The hints last for one state.
    let written = board.diff(&state((3, 47), (0, 40), vec![forest, island, swamp]));
    assert_eq!(
        written,
        vec![
            new_card_in_library(3, "Swamp"),
            unknown(plain_move(3, "Swamp", Zone::Library, Zone::Hand, draw)),
        ]
    );
}

#[test]
fn a_found_card_without_a_known_cause_is_moved_by_unknown() {
    let mut board = in_a_turn(&state((0, 50), (0, 40), Vec::new()));
    board.hints.found.insert(5);
    let forest = card(5, "Forest", StateZone::Hand, 0, &["Land"]);
    let written = board.diff(&state((1, 49), (0, 40), vec![forest]));
    assert_eq!(
        written,
        vec![
            new_card_in_library(1, "Forest"),
            unknown(plain_move(1, "Forest", Zone::Library, Zone::Hand, None)),
        ]
    );
}

#[test]
fn a_discard_and_a_mill_are_written_as_such() {
    let bolt = card(5, "Bolt", StateZone::Hand, 0, &["Instant"]);
    let shock = card(6, "Shock", StateZone::Hand, 0, &["Instant"]);
    let mut board = in_a_turn(&state((2, 50), (0, 40), vec![bolt, shock]));

    // An event said Bolt went from the hand to the graveyard; nothing said
    // so of Shock (it may have been cast and resolved unseen). Opt came out
    // of the library.
    board.hints.discarded.insert(5);
    let cards = vec![
        card(5, "Bolt", StateZone::Graveyard, 0, &["Instant"]),
        card(6, "Shock", StateZone::Graveyard, 0, &["Instant"]),
        card(7, "Opt", StateZone::Graveyard, 0, &["Instant"]),
    ];
    let written = board.diff(&state((0, 49), (0, 40), cards));
    let (hand, library, graveyard) = (Zone::Hand, Zone::Library, Zone::Graveyard);
    assert_eq!(
        written,
        vec![
            unknown(plain_move(
                1,
                "Bolt",
                hand,
                graveyard,
                Some(MoveAction::Discard)
            )),
            unknown(plain_move(2, "Shock", hand, graveyard, None)),
            new_card_in_library(3, "Opt"),
            unknown(plain_move(
                3,
                "Opt",
                library,
                graveyard,
                Some(MoveAction::Mill)
            )),
        ]
    );
}

#[test]
fn a_card_drawn_and_discarded_between_two_states_is_not_milled() {
    // Opt was drawn and discarded with no full state between: the library
    // shrank and an event named the discard, so it is not a mill.
    let mut board = in_a_turn(&state((0, 50), (0, 40), Vec::new()));
    board.hints.discarded.insert(7);
    let opt = card(7, "Opt", StateZone::Graveyard, 0, &["Instant"]);
    let written = board.diff(&state((0, 49), (0, 40), vec![opt]));
    assert_eq!(
        written,
        vec![
            new_card_in_library(1, "Opt"),
            unknown(plain_move(1, "Opt", Zone::Library, Zone::Graveyard, None)),
        ]
    );
}

#[test]
fn a_token_takes_the_cause_of_what_made_it() {
    let mut board = in_a_turn(&state((0, 50), (0, 40), Vec::new()));
    let mut spawn = permanent(8, "Eldrazi Spawn", 0, &["Creature"]);
    spawn.is_token = true;
    let mut second = spawn.clone();
    second.id = 9;
    let token = |id: u32| Statement::NewToken {
        object: obj(id, "Eldrazi Spawn"),
        owner: PLAYER.to_string(),
        zone: Zone::Battlefield,
        entering: Entering {
            face: None,
            tapped: false,
            types: vec![CardType::Creature],
            pt: Some((0, 0)),
        },
        sources: Vec::new(),
    };

    board.hints.token = true;
    board.hints.cause = Some(Cause::Object(4));
    let written = board.diff(&state((0, 50), (0, 40), vec![spawn.clone()]));
    assert_eq!(written, vec![(token(1), Some(Cause::Object(4)))]);

    // No `TOKEN_CREATED` came before this one.
    board.hints.cause = Some(Cause::Object(4));
    let written = board.diff(&state((0, 50), (0, 40), vec![spawn, second]));
    assert_eq!(written, vec![unknown(token(2))]);
}

/// The names of the objects a keyframe lists in the opponent's hand, hidden
/// ones as `None`.
fn opponent_hand(entries: &[Statement]) -> Vec<Option<String>> {
    let mut names = Vec::new();
    for entry in entries {
        if let Statement::KeyframeObject {
            object,
            zone: Zone::Hand,
            player: Some(player),
            ..
        } = entry
            && player == OPPONENT
        {
            names.push(object.name.clone());
        }
    }
    names
}

#[test]
fn a_named_hand_card_is_ended_when_the_hidden_ones_run_out() {
    // The opponent has one hidden card and a Bear that then goes back to
    // their hand: a named card in a hand Forge hides.
    let bear = permanent(5, "Bear", 1, &["Creature"]);
    let mut board = in_a_turn(&state((0, 50), (1, 40), vec![bear]));
    board.diff(&state((0, 50), (2, 40), Vec::new()));
    assert_eq!(
        opponent_hand(&board.keyframe()),
        vec![Some("Bear".to_string()), None]
    );

    // Both leave the hand with nothing to show for it.
    let empty = state((0, 50), (0, 40), Vec::new());
    assert_eq!(board.diff(&empty), Vec::new());
    assert!(board.drifted());
    assert_eq!(opponent_hand(&board.keyframe()), Vec::new());
    assert!(!board.drifted());
    assert_eq!(board.diff(&empty), Vec::new());
    assert!(!board.drifted());
}

#[test]
fn a_repair_keeps_a_card_bottomed_after_the_last_state() {
    // A pregame: the user holds Karn, the opponent three hidden cards.
    let karn = card(20, "Karn", StateZone::Hand, 0, &["Planeswalker"]);
    let first = state((1, 53), (3, 40), vec![karn.clone()]);
    let mut board = board();
    board.start(&first, true);
    board.diff(&first);

    // The opponent's hand shrinks unseen: drift, noted with the library
    // sizes of this state.
    assert_eq!(board.diff(&state((1, 53), (2, 40), vec![karn])), Vec::new());
    assert!(board.drifted());

    // The user then bottoms Karn. No state follows before the first turn.
    assert_eq!(board.bottom(0, &[20]).len(), 1);
    let entries = board.keyframe();
    let library = entries.iter().find_map(|entry| match entry {
        Statement::KeyframePlayer {
            player, library, ..
        } if player == PLAYER => Some(*library),
        _ => None,
    });
    assert_eq!(library, Some(54), "the bottomed card is in the library");
    let karn_at = entries.iter().find_map(|entry| match entry {
        Statement::KeyframeObject {
            object,
            zone,
            position,
            ..
        } if object.name.as_deref() == Some("Karn") => Some((*zone, *position)),
        _ => None,
    });
    let bottom = Position {
        end: LibraryEnd::Bottom,
        n: 1,
    };
    assert_eq!(karn_at, Some((Zone::Library, Some(bottom))));
    assert_eq!(hidden_hand(&entries), vec![3, 4]);
}

#[test]
fn poison_counters_are_written_not_drift() {
    let mut board = joined(&state((0, 50), (0, 40), Vec::new()));
    let mut poisoned = state((0, 50), (0, 40), Vec::new());
    poisoned.player_states[1].poison = Some(3);
    assert_eq!(
        board.diff(&poisoned),
        vec![unknown(Statement::Poison {
            player: OPPONENT.to_string(),
            from: 0,
            to: 3,
        })]
    );
    assert!(!board.drifted());
    let poison = board.keyframe().into_iter().find_map(|entry| match entry {
        Statement::KeyframePlayer { player, poison, .. } if player == OPPONENT => Some(poison),
        _ => None,
    });
    assert_eq!(poison, Some(3));
    // The same state again writes nothing.
    assert_eq!(board.diff(&poisoned), Vec::new());
}

/// The names a keyframe lists.
fn keyframe_names(entries: &[Statement]) -> Vec<String> {
    let mut names = Vec::new();
    for entry in entries {
        if let Statement::KeyframeObject { object, .. } = entry
            && let Some(name) = &object.name
        {
            names.push(name.clone());
        }
    }
    names
}

#[test]
fn a_named_card_listed_nowhere_is_drift_and_ends_at_the_keyframe() {
    let forest = card(5, "Forest", StateZone::Hand, 0, &["Land"]);
    let bear = permanent(6, "Bear", 1, &["Creature"]);
    let mut board = joined(&state((1, 50), (0, 40), vec![forest, bear]));
    assert!(!board.drifted());

    // Both are listed nowhere and no library or hand grew to take them.
    let empty = state((0, 50), (0, 40), Vec::new());
    assert_eq!(board.diff(&empty), Vec::new());
    assert!(board.drifted());

    // The keyframe ends them: a reader then holds what Forge shows.
    assert_eq!(keyframe_names(&board.keyframe()), Vec::<String>::new());
    assert!(!board.drifted());
    assert_eq!(board.diff(&empty), Vec::new());
    assert!(!board.drifted());
}

#[test]
fn a_card_listed_nowhere_that_shows_again_is_no_drift() {
    let forest = card(5, "Forest", StateZone::Hand, 0, &["Land"]);
    let mut board = joined(&state((1, 50), (0, 40), vec![forest.clone()]));
    // Forge lists a spell nowhere while its cost is paid.
    assert_eq!(board.diff(&state((0, 50), (0, 40), Vec::new())), Vec::new());
    assert!(board.drifted());

    // The next state shows it again: the drift is gone and it is kept.
    board.diff(&state((1, 50), (0, 40), vec![forest]));
    assert!(!board.drifted());
    assert_eq!(
        keyframe_names(&board.keyframe()),
        vec!["Forest".to_string()]
    );
}

#[test]
fn a_library_card_is_not_taken_for_a_card_being_paid_for() {
    // The opponent holds two hidden cards. One is being cast (listed
    // nowhere while its cost is paid) and a search puts a Mountain from the
    // library onto the battlefield, all before one state.
    let mut board = in_a_turn(&state((0, 50), (2, 40), Vec::new()));
    let mountain = permanent(9, "Mountain", 1, &["Land"]);
    let written = board.diff(&state((0, 50), (1, 39), vec![mountain]));
    assert_eq!(
        written[..2],
        [
            (
                Statement::NewCard {
                    object: obj(3, "Mountain"),
                    owner: OPPONENT.to_string(),
                    zone: Zone::Library,
                    position: None,
                    entering: None,
                    sources: Vec::new(),
                },
                None
            ),
            unknown(Statement::Move {
                object: obj(3, "Mountain"),
                from: Zone::Library,
                to: Zone::Battlefield,
                action: None,
                position: None,
                entering: Some(Entering {
                    face: None,
                    tapped: false,
                    types: vec![CardType::Land],
                    pt: None,
                }),
                with: None,
            }),
        ],
        "{written:?}"
    );
    // Both hidden cards are still in the hand: one is being paid for.
    assert_eq!(board.objects.hand_count(1), 2);
    assert_eq!(board.objects.lowest_hidden_in_hand(1), Some(1));
}

#[test]
fn a_reissued_id_does_not_move_a_named_hand_card() {
    // The opponent's Force is named in their hidden hand (a cast taken
    // back), keyed by Forge id 9. Forge then gives id 9 to an Island.
    let mut board = in_a_turn(&state((0, 50), (2, 40), Vec::new()));
    let mut out = Vec::new();
    board.object_of_event(&mut out, 9, "Force", 1, Zone::Hand);
    assert_eq!(
        opponent_hand(&board.keyframe()),
        vec![Some("Force".to_string()), None]
    );

    let island = permanent(9, "Island", 1, &["Land"]);
    let written = board.diff(&state((0, 50), (1, 40), vec![island]));
    assert!(
        written.iter().all(|(statement, _)| !matches!(
            statement,
            Statement::Move { object, .. } if object.name.as_deref() == Some("Force")
        )),
        "{written:?}"
    );
    assert_eq!(
        opponent_hand(&board.keyframe()),
        vec![Some("Force".to_string())]
    );
}

#[test]
fn an_event_naming_a_reissued_id_does_not_move_a_named_hand_card() {
    // The opponent's Force is named in their hidden hand under Forge id 9.
    // Forge gives id 9 to an Island, and an event moves it out of the hand
    // before any state shows it: that is not the Force.
    let mut board = in_a_turn(&state((0, 50), (2, 40), Vec::new()));
    let mut out = Vec::new();
    board.object_of_event(&mut out, 9, "Force", 1, Zone::Hand);
    let mut out = Vec::new();
    let id = board.object_of_event(&mut out, 9, "Island", 1, Zone::Hand);
    assert_eq!(id, Some(2), "the hidden card, not the Force");
    assert_eq!(
        out,
        vec![(
            Statement::Identify {
                id: 2,
                name: Some("Island".to_string()),
                sources: Vec::new(),
            },
            None
        )]
    );
    assert_eq!(board.objects.id_of(9), Some(2));
    assert_eq!(
        opponent_hand(&board.keyframe()),
        vec![Some("Force".to_string()), Some("Island".to_string())]
    );
}

#[test]
fn a_card_named_again_by_a_new_id_is_the_named_hand_card() {
    // The Force named in the hidden hand is cast again under a new id: it
    // is that card, not one more hidden card named Force.
    let mut board = in_a_turn(&state((0, 50), (2, 40), Vec::new()));
    let mut out = Vec::new();
    board.object_of_event(&mut out, 9, "Force", 1, Zone::Hand);
    let mut out = Vec::new();
    let id = board.object_of_event(&mut out, 12, "Force", 1, Zone::Hand);
    assert_eq!(id, Some(1));
    assert_eq!(out, Vec::new(), "no second identify");
    assert_eq!(board.objects.id_of(12), Some(1));
    assert_eq!(board.objects.id_of(9), None, "the old key is dropped");
}

#[test]
fn the_card_cap_counts_only_cards_the_record_tracks() {
    // A hostile state pads its card list with what the record skips: cards
    // of no seat, effects without types and repeats of one id.
    let mut cards = Vec::new();
    for id in 0..MAX_OBJECTS as u64 {
        let mut padding = card(1_000 + id, "Padding", StateZone::Exile, 0, &[]);
        match id % 3 {
            0 => padding.owner_seat = Some(7),
            1 => padding.has_types = false,
            _ => padding.id = 5,
        }
        cards.push(padding);
    }
    cards.push(permanent(9, "Bear", 0, &["Creature"]));
    let shown = shown_cards(&state((0, 50), (0, 40), cards));
    assert!(shown.iter().any(|card| card.name == "Bear"));

    // Real cards are still capped.
    let many: Vec<StateCard> = (0..=MAX_OBJECTS as u64)
        .map(|id| permanent(id, "Bear", 0, &["Creature"]))
        .collect();
    assert_eq!(
        shown_cards(&state((0, 50), (0, 40), many)).len(),
        MAX_OBJECTS
    );
}
