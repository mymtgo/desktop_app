//! The committed ranked Modern Bo3 capture as a `.mtgr` record, and a
//! synthetic match for what the capture never shows.

use std::collections::{BTreeMap, BTreeSet};

use endstep_match::{ReplayNames, match_record};
use endstep_protocol::{
    Frame, FrameKind, GameEvent, GameState, MatchBody, Participant, PlayerState, StateCard, Unit,
    Zone as StateZone, decode_unit,
};
use mtgr::{
    AbilityKind, Cause, Choice, DeckCard, Game, LibraryPlace, Line, MoveAction, ObjRef, Record,
    Statement, TargetRef, Zone, read_header, write,
};

const UNIT_01: &str =
    include_str!("../../../fixtures/endstep/ranked-modern-bo3/01-session-a7a2056a.jsonl");
const UNIT_02: &str =
    include_str!("../../../fixtures/endstep/ranked-modern-bo3/02-match-329f3d80.jsonl");
const UNIT_03: &str =
    include_str!("../../../fixtures/endstep/ranked-modern-bo3/03-session-e3f486f1.jsonl");
const UNIT_04: &str =
    include_str!("../../../fixtures/endstep/ranked-modern-bo3/04-match-946164b1.jsonl");
const UNIT_05: &str =
    include_str!("../../../fixtures/endstep/ranked-modern-bo3/05-session-1a930f54.jsonl");

const FORGE_GAME: &str = "00000000-0000-4000-8000-e0000000003e";
const MATCH_ID: &str = "00000000-0000-4000-8000-e0000000003a";
const PLAYER: &str = "Player";

fn unit(text: &str) -> Unit {
    decode_unit(text).expect("unit decodes")
}

fn all_units() -> Vec<Unit> {
    vec![
        unit(UNIT_01),
        unit(UNIT_02),
        unit(UNIT_03),
        unit(UNIT_04),
        unit(UNIT_05),
    ]
}

fn record_of(units: &[Unit]) -> Record {
    match_record(FORGE_GAME, units, &ReplayNames::default()).expect("a record")
}

fn tag<'a>(game: &'a Game, name: &str) -> &'a str {
    game.tags
        .iter()
        .find(|tag| tag.name == name)
        .map(|tag| tag.value.as_str())
        .unwrap_or_else(|| panic!("no {name} tag"))
}

/// The game with `Game` tag `number`.
fn game(record: &Record, number: u32) -> &Game {
    record
        .games
        .iter()
        .find(|game| tag(game, "Game") == number.to_string())
        .unwrap_or_else(|| panic!("no game {number}"))
}

/// The id a `new` line introduces.
fn introduced_id(statement: &Statement) -> Option<u32> {
    match statement {
        Statement::NewCard { object, .. } | Statement::NewToken { object, .. } => Some(object.id),
        Statement::NewHidden { id, .. }
        | Statement::NewCopy { id, .. }
        | Statement::NewEffect { id, .. }
        | Statement::NewAbility { id, .. } => Some(*id),
        _ => None,
    }
}

fn target_id(target: &TargetRef) -> Option<u32> {
    match target {
        TargetRef::Object(object) => Some(object.id),
        TargetRef::Player(_) => None,
    }
}

fn choice_ids(choices: &[Choice]) -> Vec<u32> {
    let mut ids = Vec::new();
    for choice in choices {
        match choice {
            Choice::Target(target) => ids.extend(target_id(target)),
            Choice::Object(object) => ids.push(object.id),
            Choice::X(_) => {}
        }
    }
    ids
}

/// Every object id a statement refers to, other than one it introduces.
fn used_ids(statement: &Statement) -> Vec<u32> {
    match statement {
        Statement::KeyframeObject {
            object,
            attached,
            with,
            ..
        } => {
            let mut ids = vec![object.id];
            ids.extend(*attached);
            ids.extend(*with);
            ids
        }
        Statement::KeyframeEffect { id, of, .. } => vec![*id, of.id],
        Statement::StackEntry {
            object, choices, ..
        } => {
            let mut ids = vec![object.id];
            ids.extend(choice_ids(choices));
            ids
        }
        Statement::StackAbility {
            id, of, choices, ..
        } => {
            let mut ids = vec![*id, of.id];
            ids.extend(choice_ids(choices));
            ids
        }
        Statement::NewCopy { of, .. }
        | Statement::NewEffect { of, .. }
        | Statement::NewAbility { of, .. } => vec![of.id],
        Statement::Target { object, target } | Statement::Attack { object, target } => {
            let mut ids = vec![object.id];
            ids.extend(target_id(target));
            ids
        }
        Statement::Damage { object, target, .. } => {
            let mut ids = vec![object.id];
            ids.extend(target_id(target));
            ids
        }
        Statement::ChooseObject { object, chosen } => vec![object.id, chosen.id],
        Statement::Block { object, attacker } => vec![object.id, attacker.id],
        Statement::Attach { object, to } => vec![object.id, to.id],
        Statement::Move { object, with, .. } => {
            let mut ids = vec![object.id];
            ids.extend(*with);
            ids
        }
        Statement::Identify { id, .. } => vec![*id],
        Statement::Play { object, .. }
        | Statement::Cast { object, .. }
        | Statement::ChooseX { object, .. }
        | Statement::Tap { object }
        | Statement::Untap { object }
        | Statement::Control { object, .. }
        | Statement::Counters { object, .. }
        | Statement::PtChange { object, .. }
        | Statement::TypesChange { object, .. }
        | Statement::Resolve { object }
        | Statement::Transform { object }
        | Statement::Special { object, .. }
        | Statement::Fizzle { object }
        | Statement::Countered { object }
        | Statement::Unattach { object }
        | Statement::Cease { object }
        | Statement::Reveal { object }
        | Statement::Look { object, .. }
        | Statement::Untracked { object } => vec![object.id],
        _ => Vec::new(),
    }
}

/// What is wrong with a game's object ids: an id used before the `new` line
/// that introduces it, or `new` lines whose ids do not count up from 1.
fn id_problems(game: &Game) -> Vec<String> {
    let mut problems = Vec::new();
    let mut introduced: BTreeSet<u32> = BTreeSet::new();
    for (index, line) in game.lines.iter().enumerate() {
        let mut used = used_ids(&line.statement);
        if let Some(Cause::Object(id) | Cause::Cost(id)) = &line.cause {
            used.push(*id);
        }
        for id in used {
            if !introduced.contains(&id) {
                problems.push(format!(
                    "line {index}: #{id} is used before it is introduced"
                ));
            }
        }
        if let Some(id) = introduced_id(&line.statement) {
            let expected = u32::try_from(introduced.len()).unwrap_or(u32::MAX) + 1;
            if id != expected {
                problems.push(format!(
                    "line {index}: #{id} is introduced after #{}",
                    expected - 1
                ));
            }
            introduced.insert(id);
        }
    }
    problems
}

/// The lines of the pregame: from `pregame` up to the first turn.
fn pregame_lines(game: &Game) -> Vec<&Line> {
    game.lines
        .iter()
        .skip_while(|line| line.statement != Statement::Pregame)
        .take_while(|line| !matches!(line.statement, Statement::Turn { .. }))
        .collect()
}

/// The last full state of game `number` before the next game begins, read
/// from the units in the order given.
fn last_state_of_game(units: &[&Unit], number: u32) -> GameState {
    let mut last = None;
    for unit in units {
        for frame in &unit.frames {
            if let FrameKind::GameState(state) = &frame.kind
                && state.game_id == FORGE_GAME
                && state.game_number == Some(number)
                && !state.sideboarding
            {
                last = Some(state.clone());
            }
        }
    }
    last.expect("a full state")
}

/// The names of the cards (not tokens) on the battlefield once every line of
/// the game is replayed: a keyframe replaces what is known, the lines after
/// it move, play, cast, introduce and end objects.
fn replayed_battlefield(game: &Game) -> Vec<String> {
    let mut names: BTreeMap<u32, Option<String>> = BTreeMap::new();
    let mut zones: BTreeMap<u32, Zone> = BTreeMap::new();
    let mut tokens: BTreeSet<u32> = BTreeSet::new();
    for line in &game.lines {
        match &line.statement {
            Statement::Keyframe => zones.clear(),
            Statement::KeyframeObject { object, zone, .. } => {
                zones.insert(object.id, *zone);
            }
            Statement::NewCard { object, zone, .. } => {
                names.insert(object.id, object.name.clone());
                zones.insert(object.id, *zone);
            }
            Statement::NewToken { object, zone, .. } => {
                tokens.insert(object.id);
                names.insert(object.id, object.name.clone());
                zones.insert(object.id, *zone);
            }
            Statement::NewHidden { id, zone, .. } => {
                names.insert(*id, None);
                zones.insert(*id, *zone);
            }
            Statement::Identify { id, name, .. } => {
                names.insert(*id, name.clone());
            }
            Statement::Move { object, to, .. } => {
                zones.insert(object.id, *to);
            }
            Statement::Play { object, .. } => {
                zones.insert(object.id, Zone::Battlefield);
            }
            Statement::Cast { object, .. } => {
                zones.insert(object.id, Zone::Stack);
            }
            Statement::Cease { object } | Statement::Untracked { object } => {
                zones.remove(&object.id);
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    for (id, zone) in &zones {
        if *zone == Zone::Battlefield && !tokens.contains(id) {
            out.push(names.get(id).cloned().flatten().unwrap_or_default());
        }
    }
    out.sort();
    out
}

#[test]
fn the_bo3_writes_three_games() {
    let record = record_of(&all_units());
    assert_eq!(record.games.len(), 3);

    let text = write(&record);
    let header = read_header(&text).expect("a header");
    assert_eq!(header.match_token, MATCH_ID);
    let numbers: Vec<u32> = header.games.iter().map(|game| game.game_number).collect();
    assert_eq!(numbers, vec![1, 2, 3]);

    // The header reader has no `Site` or `Result`: they are read from the
    // record's tags and the written tag lines.
    let results: Vec<&str> = record
        .games
        .iter()
        .map(|game| tag(game, "Result"))
        .collect();
    assert_eq!(results, vec!["1-0", "0-1", "0-1"]);
    for game in &record.games {
        assert_eq!(tag(game, "Mtgr"), "0.3.0");
        assert_eq!(tag(game, "Site"), "Endstep");
        assert_eq!(tag(game, "Event"), "Ranked");
        assert_eq!(tag(game, "Date"), "2026.11.10");
        assert_eq!(tag(game, "Round"), "-");
        assert_eq!(tag(game, "Player1"), PLAYER);
        assert_eq!(tag(game, "Player2"), "Opponent");
        assert_eq!(tag(game, "Format"), "Modern");
        assert_eq!(tag(game, "Match"), MATCH_ID);
        assert_eq!(tag(game, "Perspective"), PLAYER);
        assert_eq!(tag(game, "ENDSTEP_MatchId"), MATCH_ID);
        assert!(game.tags.iter().all(|tag| !tag.name.ends_with("Archetype")));
    }
    assert_eq!(text.matches("[Site \"Endstep\"]\n").count(), 3);
    assert!(text.contains("[Result \"1-0\"]\n"));
    // Each game ends with the loser conceding.
    for (number, loser) in [(1, "Opponent"), (2, PLAYER), (3, PLAYER)] {
        let last = game(&record, number).lines.last().expect("a line");
        assert_eq!(
            last.statement,
            Statement::Concede {
                player: loser.to_string()
            }
        );
    }
}

#[test]
fn archetype_names_become_tags_by_seat() {
    let names = ReplayNames {
        local_archetype: Some("Eldrazi Tron".to_string()),
        opponent_archetype: Some("Living End".to_string()),
    };
    let record = match_record(FORGE_GAME, &all_units(), &names).expect("a record");
    for game in &record.games {
        assert_eq!(tag(game, "Player1Archetype"), "Eldrazi Tron");
        assert_eq!(tag(game, "Player2Archetype"), "Living End");
    }
}

#[test]
fn every_object_is_introduced_before_use() {
    let record = record_of(&all_units());
    for game in &record.games {
        assert_eq!(id_problems(game), Vec::<String>::new());
    }
}

#[test]
fn the_last_keyframe_matches_the_last_state() {
    let units = all_units();
    let record = record_of(&units);
    let state = last_state_of_game(&[&units[1], &units[3]], 1);
    let mut expected: Vec<String> = state
        .cards
        .iter()
        .filter(|card| card.zone == StateZone::Battlefield && card.has_types && !card.is_token)
        .map(|card| card.name.clone())
        .collect();
    expected.sort();
    assert!(expected.len() > 10, "the game ended with a full board");
    // The last keyframe is the start of turn 12; the game went on (a land,
    // a spell), so the lines after it are replayed onto it.
    assert_eq!(replayed_battlefield(game(&record, 1)), expected);
}

/// The index of the first line from `start` on whose statement passes
/// `wanted`.
fn line_from(game: &Game, start: usize, wanted: impl Fn(&Statement) -> bool) -> Option<usize> {
    (start..game.lines.len()).find(|index| wanted(&game.lines[*index].statement))
}

fn named(object: &ObjRef, name: &str) -> bool {
    object.name.as_deref() == Some(name)
}

/// The index of the first cast of the card `name`, and the object's id.
fn cast_of(game: &Game, name: &str) -> (usize, u32) {
    let is_cast = |statement: &Statement| matches!(statement, Statement::Cast { object, .. } if named(object, name));
    let index = line_from(game, 0, is_cast).unwrap_or_else(|| panic!("no cast of {name}"));
    let Statement::Cast { object, .. } = &game.lines[index].statement else {
        panic!("a cast");
    };
    (index, object.id)
}

#[test]
fn casts_and_resolves_are_written() {
    let record = record_of(&all_units());
    let first = game(&record, 1);
    let (cast, id) = cast_of(first, "Expedition Map");
    assert_eq!(
        first.lines[cast].statement,
        Statement::Cast {
            object: ObjRef {
                id,
                name: Some("Expedition Map".to_string())
            },
            from: Zone::Hand,
            face: None,
        }
    );
    assert_eq!(first.lines[cast].cause, None);

    let resolved = |statement: &Statement| matches!(statement, Statement::Resolve { object } if object.id == id);
    let resolve = line_from(first, cast, resolved).expect("the map resolves");
    // The card leaves the stack because it resolved, and only once: the
    // state diff does not move a card an event already moved.
    let onto_battlefield = &first.lines[resolve + 1];
    assert!(matches!(
        &onto_battlefield.statement,
        Statement::Move { object, from: Zone::Stack, to: Zone::Battlefield, .. } if object.id == id
    ));
    assert_eq!(
        onto_battlefield.cause,
        Some(Cause::Rule("resolve".to_string()))
    );
    let moved_to_stack = |statement: &Statement| matches!(statement, Statement::Move { object, to: Zone::Stack, .. } if object.id == id);
    assert_eq!(line_from(first, 0, moved_to_stack), None);

    // A land is played, not moved.
    let played = |statement: &Statement| matches!(statement, Statement::Play { object, from: Zone::Hand, .. } if named(object, "Urza's Mine"));
    assert!(line_from(first, 0, played).is_some());
}

#[test]
fn an_opponent_cast_identifies_a_hidden_card() {
    let record = record_of(&all_units());
    let first = game(&record, 1);
    let (cast, id) = cast_of(first, "Force of Negation");
    let identified = |statement: &Statement| matches!(statement, Statement::Identify { id: found, name: Some(name), .. } if *found == id && name == "Force of Negation");
    let identify = line_from(first, 0, identified).expect("the hidden card is named");
    assert!(identify < cast);
    // The object was one of the opponent's hidden hand cards.
    let hidden = |statement: &Statement| matches!(statement, Statement::NewHidden { id: found, owner, .. } if *found == id && owner == "Opponent");
    assert!(line_from(first, 0, hidden).is_some_and(|index| index < identify));
    // The card is named when it goes onto the stack and cast once its cost
    // is paid: the card pitched to it is exiled in between.
    let pitched = |statement: &Statement| {
        matches!(
            statement,
            Statement::Move {
                from: Zone::Hand,
                to: Zone::Exile,
                ..
            }
        )
    };
    let pitch = line_from(first, identify, pitched).expect("the pitched card");
    assert!(identify < pitch && pitch < cast);
}

fn is_cast_of(statement: &Statement, name: &str) -> bool {
    matches!(statement, Statement::Cast { object, .. } if named(object, name))
}

#[test]
fn a_cast_taken_back_is_not_written() {
    let record = record_of(&all_units());
    // Game 2: the user put Talisman of Resilience on the stack and
    // cancelled. Game 3: the opponent did the same with Force of Negation.
    // Neither was cast, and no card goes from the stack back to a hand.
    let second = game(&record, 2);
    let third = game(&record, 3);
    let talisman = |statement: &Statement| is_cast_of(statement, "Talisman of Resilience");
    assert_eq!(line_from(second, 0, talisman), None);
    let force = |statement: &Statement| is_cast_of(statement, "Force of Negation");
    assert_eq!(line_from(third, 0, force), None);
    for game in &record.games {
        let back = |statement: &Statement| {
            matches!(
                statement,
                Statement::Move {
                    from: Zone::Stack,
                    to: Zone::Hand,
                    ..
                }
            )
        };
        assert_eq!(line_from(game, 0, back), None);
    }

    // The opponent's card was shown: the hidden hand object is named, and
    // the keyframes after list it in their hand by name.
    let shown = |statement: &Statement| matches!(statement, Statement::Identify { name: Some(name), .. } if name == "Force of Negation");
    let identify = line_from(third, 0, shown).expect("the card is named");
    let Statement::Identify { id, .. } = &third.lines[identify].statement else {
        panic!("an identify");
    };
    let in_hand = |statement: &Statement| matches!(statement, Statement::KeyframeObject { object, zone: Zone::Hand, player: Some(player), .. } if object.id == *id && named(object, "Force of Negation") && player == "Opponent");
    assert!(line_from(third, identify, in_hand).is_some());
}

#[test]
fn a_token_is_new_while_its_spell_resolves() {
    let record = record_of(&all_units());
    let first = game(&record, 1);
    let (cast, command) = cast_of(first, "Kozilek's Command");
    let is_token = |statement: &Statement| matches!(statement, Statement::NewToken { .. });
    let token = line_from(first, cast, is_token).expect("a token");
    assert_eq!(first.lines[token].cause, Some(Cause::Object(command)));
    let resolved = |statement: &Statement| matches!(statement, Statement::Resolve { object } if object.id == command);
    let left = |statement: &Statement| matches!(statement, Statement::Move { object, from: Zone::Stack, .. } if object.id == command);
    let resolve = line_from(first, cast, resolved).expect("the spell resolves");
    let leaves = line_from(first, cast, left).expect("the spell leaves the stack");
    assert!(resolve < token && token < leaves);
}

#[test]
fn ability_text_has_no_forge_notes() {
    let record = record_of(&all_units());
    let mut texts = Vec::new();
    for game in &record.games {
        for line in &game.lines {
            if let Statement::NewAbility {
                text: Some(text), ..
            } = &line.statement
            {
                texts.push(text.clone());
            }
        }
    }
    assert!(texts.len() > 10);
    for text in &texts {
        assert!(!text.contains('['), "{text}");
        assert!(!text.contains("Targeting"), "{text}");
        // A Forge card id is written `Name (78)`.
        let id_follows = |part: &str| part.starts_with(|ch: char| ch.is_ascii_digit());
        assert!(!text.split(" (").skip(1).any(id_follows), "{text}");
    }
    assert!(
        texts
            .iter()
            .any(|text| text == "Cascade - Violent Outburst")
    );
}

#[test]
fn a_countered_spell_names_what_countered_it() {
    let record = record_of(&all_units());
    let first = game(&record, 1);
    let (_, trinisphere) = cast_of(first, "Trinisphere");
    let (_, force) = cast_of(first, "Force of Negation");
    let was_countered = |statement: &Statement| matches!(statement, Statement::Countered { object } if object.id == trinisphere);
    let countered = line_from(first, 0, was_countered).expect("the spell is countered");
    assert_eq!(first.lines[countered].cause, Some(Cause::Object(force)));
    // The counterspell targeted it, and resolved before it countered.
    let targets = |statement: &Statement| matches!(statement, Statement::Target { object, target: TargetRef::Object(target) } if object.id == force && target.id == trinisphere);
    assert!(line_from(first, 0, targets).is_some_and(|index| index < countered));
    let resolved = |statement: &Statement| matches!(statement, Statement::Resolve { object } if object.id == force);
    assert!(line_from(first, 0, resolved).is_some_and(|index| index < countered));
}

#[test]
fn abilities_go_on_the_stack_and_cease() {
    let record = record_of(&all_units());
    let first = game(&record, 1);
    let map_ability = |statement: &Statement| matches!(statement, Statement::NewAbility { kind: AbilityKind::Activated, of, text: Some(_), controller, in_stack: true, .. } if named(of, "Expedition Map") && controller == PLAYER);
    let new = line_from(first, 0, map_ability).expect("the map's ability");
    let Statement::NewAbility { id, .. } = &first.lines[new].statement else {
        panic!("an ability");
    };
    let id = *id;
    let resolved = |statement: &Statement| matches!(statement, Statement::Resolve { object } if object.id == id);
    let resolve = line_from(first, new, resolved).expect("the ability resolves");
    let ceased =
        |statement: &Statement| matches!(statement, Statement::Cease { object } if object.id == id);
    let cease = line_from(first, resolve, ceased).expect("the ability ceases");
    assert_eq!(
        first.lines[cease].cause,
        Some(Cause::Rule("resolve".to_string()))
    );
    assert!(first.lines.iter().any(|line| matches!(
        line.statement,
        Statement::NewAbility {
            kind: AbilityKind::Triggered,
            ..
        }
    )));

    // No ability is left on the stack when a turn begins: each `new
    // ability` has its `cease` before the next `turn` line.
    for game in &record.games {
        let mut open: BTreeSet<u32> = BTreeSet::new();
        for line in &game.lines {
            match &line.statement {
                Statement::NewAbility { id, .. } => {
                    open.insert(*id);
                }
                Statement::Cease { object } => {
                    open.remove(&object.id);
                }
                Statement::Turn { number, .. } => {
                    assert_eq!(open, BTreeSet::new(), "at turn {number}");
                }
                _ => {}
            }
        }
    }
}

#[test]
fn a_tutored_card_is_not_drawn() {
    let record = record_of(&all_units());
    let first = game(&record, 1);
    // Expedition Map finds Urza's Tower: a move out of the library caused by
    // the map's ability, then the reveal, then the shuffle.
    let found = |statement: &Statement| matches!(statement, Statement::Move { object, from: Zone::Library, to: Zone::Hand, .. } if named(object, "Urza's Tower"));
    let index = line_from(first, 0, found).expect("the tower is found");
    let Statement::Move { action, .. } = &first.lines[index].statement else {
        panic!("a move");
    };
    assert_eq!(*action, None);
    assert!(matches!(first.lines[index].cause, Some(Cause::Object(_))));
    assert!(matches!(
        &first.lines[index + 1].statement,
        Statement::Reveal { object } if named(object, "Urza's Tower")
    ));
    assert_eq!(
        first.lines[index + 2].statement,
        Statement::Shuffle {
            player: PLAYER.to_string()
        }
    );

    // The draw step's card is still a draw, by the rule.
    let drawn = |statement: &Statement| matches!(statement, Statement::Move { object, action: Some(MoveAction::Draw), .. } if named(object, "Sowing Mycospawn"));
    let draw = line_from(first, 0, drawn).expect("the first draw");
    assert_eq!(
        first.lines[draw].cause,
        Some(Cause::Rule("draw".to_string()))
    );
    // A card put from the library into the graveyard is milled.
    let milled = |statement: &Statement| matches!(statement, Statement::Move { object, action: Some(MoveAction::Mill), .. } if named(object, "Shardless Agent"));
    assert!(line_from(first, 0, milled).is_some());
    // A card put from the hand into the graveyard is discarded.
    let discarded = |statement: &Statement| matches!(statement, Statement::Move { object, action: Some(MoveAction::Discard), .. } if named(object, "Oliphaunt"));
    assert!(line_from(first, 0, discarded).is_some());
}

#[test]
fn combat_is_written() {
    let record = record_of(&all_units());
    let first = game(&record, 1);
    let mut attackers: BTreeSet<u32> = BTreeSet::new();
    let (mut attacks, mut blocks) = (0, 0);
    for line in &first.lines {
        match &line.statement {
            Statement::Turn { .. } => attackers.clear(),
            Statement::Attack { object, target } => {
                attacks += 1;
                attackers.insert(object.id);
                assert!(matches!(target, TargetRef::Player(_)));
                assert_eq!(line.cause, None);
            }
            Statement::Block { attacker, .. } => {
                blocks += 1;
                assert!(
                    attackers.contains(&attacker.id),
                    "#{} is blocked but did not attack this turn",
                    attacker.id
                );
            }
            _ => {}
        }
    }
    assert!(attacks >= 1, "{attacks} attacks");
    assert!(blocks >= 1, "{blocks} blocks");

    // Combat damage to a player and to a creature, and the life it took.
    let combat = Some(Cause::Rule("combat-damage".to_string()));
    let hit_player = |statement: &Statement| matches!(statement, Statement::Damage { target: TargetRef::Player(player), amount: 2, .. } if player == PLAYER);
    let damage = line_from(first, 0, hit_player).expect("damage to the player");
    assert_eq!(first.lines[damage].cause, combat);
    assert_eq!(
        first.lines[damage + 1].statement,
        Statement::Life {
            player: PLAYER.to_string(),
            from: 20,
            to: 18
        }
    );
    assert_eq!(first.lines[damage + 1].cause, combat);
    assert!(first.lines.iter().any(|line| matches!(
        line.statement,
        Statement::Damage {
            target: TargetRef::Object(_),
            ..
        }
    )));
}

#[test]
fn life_follows_the_states() {
    let units = all_units();
    let record = record_of(&units);
    let first = game(&record, 1);
    let mut life: BTreeMap<String, i32> = BTreeMap::new();
    for line in &first.lines {
        if let Statement::Life { player, from, to } = &line.statement {
            let before = life.get(player).copied().unwrap_or(20);
            assert_eq!(*from, before, "{player}'s life before the change");
            life.insert(player.clone(), *to);
        }
    }
    let state = last_state_of_game(&[&units[1], &units[3]], 1);
    for (seat, name) in &state.players {
        let shown = state
            .player_states
            .iter()
            .find(|player| player.seat == *seat)
            .and_then(|player| player.life)
            .expect("a life total");
        assert_eq!(life.get(name).copied().unwrap_or(20), shown, "{name}");
    }
    assert!(life.values().any(|total| *total != 20));
}

#[test]
fn a_library_look_identifies_its_cards() {
    let record = record_of(&all_units());
    let first = game(&record, 1);
    let mut by_time: BTreeMap<i64, Vec<u32>> = BTreeMap::new();
    for line in &first.lines {
        if let Statement::Look { object, player } = &line.statement {
            assert_eq!(player, PLAYER);
            assert!(object.name.is_some());
            by_time.entry(line.time_ms).or_default().push(object.id);
        }
    }
    let looked = by_time
        .values()
        .max_by_key(|ids| ids.len())
        .expect("a look");
    assert_eq!(looked.len(), 21);
    // Each was a card of the opponent's library the record had not seen.
    for id in looked {
        let introduced = |statement: &Statement| matches!(statement, Statement::NewCard { object, owner, zone: Zone::Library, .. } if object.id == *id && owner == "Opponent");
        assert!(line_from(first, 0, introduced).is_some(), "#{id}");
    }
}

#[test]
fn the_tags_name_the_gaps() {
    let record = record_of(&all_units());
    for game in &record.games {
        assert_eq!(tag(game, "Omits"), "mana costs priority effects");
        assert_eq!(tag(game, "Partial"), "damage library");
    }
    let late = record_of(&[unit(UNIT_01), unit(UNIT_03), unit(UNIT_04), unit(UNIT_05)]);
    for game in &late.games {
        assert_eq!(tag(game, "Omits"), "mana costs priority effects");
        assert_eq!(tag(game, "Partial"), "damage library");
    }
}

#[test]
fn game_three_shows_the_mulligan() {
    let record = record_of(&all_units());
    let pregame = pregame_lines(game(&record, 3));
    let mulligans: Vec<&Line> = pregame
        .iter()
        .filter(|line| matches!(line.statement, Statement::Mulligan { .. }))
        .copied()
        .collect();
    assert_eq!(mulligans.len(), 1);
    assert_eq!(
        mulligans[0].statement,
        Statement::Mulligan {
            player: PLAYER.to_string()
        }
    );
    let keeps = pregame
        .iter()
        .filter(|line| matches!(line.statement, Statement::Keep { .. }))
        .count();
    assert_eq!(keeps, 2);
    let bottomed: Vec<&Statement> = pregame
        .iter()
        .map(|line| &line.statement)
        .filter(|statement| {
            matches!(
                statement,
                Statement::Move {
                    position: Some(LibraryPlace::Bottom),
                    ..
                }
            )
        })
        .collect();
    assert_eq!(bottomed.len(), 1);
    let Statement::Move {
        object, from, to, ..
    } = bottomed[0]
    else {
        panic!("a move");
    };
    assert_eq!(object.name.as_deref(), Some("Karn, the Great Creator"));
    assert_eq!((*from, *to), (Zone::Hand, Zone::Library));
}

#[test]
fn game_two_lists_its_own_deck() {
    let record = record_of(&all_units());
    let main_count = |number: u32, name: &str| -> u32 {
        game(&record, number)
            .lines
            .iter()
            .filter_map(|line| match &line.statement {
                Statement::DeckEntry {
                    side: false,
                    count,
                    card: DeckCard::Named(card),
                } if card == name => Some(*count),
                _ => None,
            })
            .sum()
    };
    assert_eq!(main_count(1, "Chalice of the Void"), 0);
    assert!(main_count(2, "Chalice of the Void") > 0);
    assert!(main_count(1, "Giant's Boulder") > main_count(2, "Giant's Boulder"));
    for number in 1..=3 {
        let first = &game(&record, number).lines[0];
        assert_eq!(
            first.statement,
            Statement::Deck {
                player: PLAYER.to_string(),
                name: None,
                unknown: false
            }
        );
    }
}

#[test]
fn a_game_joined_late_opens_with_a_keyframe() {
    let units = vec![unit(UNIT_01), unit(UNIT_03), unit(UNIT_04), unit(UNIT_05)];
    let record = record_of(&units);
    let first = game(&record, 1);
    let pregame = pregame_lines(first);
    assert_eq!(pregame[0].statement, Statement::Pregame);
    assert_eq!(pregame[1].statement, Statement::Keyframe);
    let first_turn = first.lines.iter().find_map(|line| match &line.statement {
        Statement::Turn { number, .. } => Some(*number),
        _ => None,
    });
    assert_eq!(first_turn, Some(9));
    assert_eq!(id_problems(first), Vec::<String>::new());
    // Nothing was drawn or kept in a pregame the capture never saw.
    assert!(pregame.iter().all(|line| !matches!(
        line.statement,
        Statement::Keep { .. } | Statement::Mulligan { .. } | Statement::Move { .. }
    )));
    assert_eq!(record.games.len(), 3);
}

#[test]
fn the_record_does_not_depend_on_unit_order() {
    let units = all_units();
    let mut reversed = all_units();
    reversed.reverse();
    assert_eq!(write(&record_of(&units)), write(&record_of(&reversed)));
}

#[test]
fn forge_ids_are_not_written() {
    let record = record_of(&all_units());
    let first = game(&record, 1);
    let mut highest = 0;
    for line in &first.lines {
        for id in used_ids(&line.statement) {
            highest = highest.max(id);
        }
        highest = highest.max(introduced_id(&line.statement).unwrap_or(0));
    }
    assert!(highest > 0);
    assert!(highest <= 2_000, "highest id {highest}");
    // The ids count up from 1 in each game (checked line by line above), so
    // game 2 starts again at #1 whatever Forge numbered its cards.
    let second = game(&record, 2);
    let first_new = second
        .lines
        .iter()
        .find_map(|line| introduced_id(&line.statement));
    assert_eq!(first_new, Some(1));
}

#[test]
fn no_match_body_or_a_spectator_writes_nothing() {
    assert_eq!(
        match_record("another-game", &all_units(), &ReplayNames::default()),
        None
    );
    assert_eq!(
        match_record(FORGE_GAME, &[unit(UNIT_02)], &ReplayNames::default()),
        None
    );
}

#[test]
fn a_match_with_no_game_state_writes_nothing() {
    // The session units hold the match body but none of the game's frames.
    let units = vec![unit(UNIT_01), unit(UNIT_03), unit(UNIT_05)];
    assert_eq!(
        match_record(FORGE_GAME, &units, &ReplayNames::default()),
        None
    );
}

/// The record's text for a snapshot file. Forge's ability text has a long
/// dash (`Imprint`), a character this repository keeps out of every file, so
/// the snapshot has a hyphen there. The record itself keeps Forge's text.
fn snapshot_text(record: &Record) -> String {
    write(record).replace('\u{2014}', "-")
}

#[test]
fn the_ranked_bo3_record() {
    let record = record_of(&all_units());
    insta::assert_snapshot!("ranked_bo3_record", snapshot_text(&record));
}

/// The same match without the unit that holds the start of game 1: the
/// record joins that game in turn 9.
#[test]
fn the_ranked_bo3_record_joined_late() {
    let units = vec![unit(UNIT_01), unit(UNIT_03), unit(UNIT_04), unit(UNIT_05)];
    let record = record_of(&units);
    insta::assert_snapshot!("ranked_bo3_record_joined_late", snapshot_text(&record));
}

// A synthetic Bo1 through the whole writer: what the committed capture never
// shows (counters, an attachment, a control change, a token that dies,
// poison) in one record the vendored validator checks (`goldens.test.ts`).

const SYNTH_GAME: &str = "synthetic-game";
const OPPONENT: &str = "Opponent";

/// The frames of one made-up match unit, one second apart.
#[derive(Default)]
struct Synth {
    frames: Vec<Frame>,
}

impl Synth {
    fn push(&mut self, kind: FrameKind) {
        let at = 1_800_000_000_000 + 1_000 * self.frames.len() as i64;
        self.frames.push(Frame { at, kind });
    }

    fn event(&mut self, event: GameEvent) {
        self.push(FrameKind::GameEvent(event));
    }

    fn state(&mut self, state: GameState) {
        self.push(FrameKind::GameState(state));
    }

    fn turn(&mut self, number: u32, seat: u32) {
        let mut began = synth_event("TURN_BEGAN");
        began.turn_number = Some(number);
        began.player_index = Some(seat);
        self.event(began);
    }

    fn phases(&mut self, names: &[&str]) {
        for name in names {
            let mut phase = synth_event("TURN_PHASE");
            phase.phase = Some(name.to_string());
            self.event(phase);
        }
    }

    fn zone_change(
        &mut self,
        id: Option<u64>,
        name: &str,
        from: Option<&str>,
        to: &str,
        owner: u32,
    ) {
        let mut change = synth_event("CARD_ZONE_CHANGE");
        change.card_id = id;
        change.card_name = id.map(|_| name.to_string());
        change.from_zone = from.map(str::to_string);
        change.to_zone = Some(to.to_string());
        change.zone_owner = Some(owner);
        self.event(change);
    }

    fn card_event(&mut self, kind: &str, id: u64, name: &str, seat: u32) {
        let mut event = synth_event(kind);
        event.card_id = Some(id);
        event.card_name = Some(name.to_string());
        event.player_index = Some(seat);
        self.event(event);
    }
}

fn synth_event(kind: &str) -> GameEvent {
    GameEvent {
        seq: None,
        game_id: SYNTH_GAME.to_string(),
        kind: kind.to_string(),
        sequence_number: None,
        player_index: None,
        amount: None,
        turn_number: None,
        end_reason: None,
        message: None,
        card_id: None,
        card_name: None,
        from_zone: None,
        to_zone: None,
        zone_owner: None,
        during_mulligan: false,
        mulligans: None,
        owner_echo: false,
        card_ids: Vec::new(),
        card_names: Vec::new(),
        phase: None,
        attacker_ids: Vec::new(),
        blockers: Vec::new(),
        target_card_id: None,
        target_card_name: None,
        old_value: None,
        new_value: None,
        player_name: None,
        toss_player: None,
    }
}

/// One seat's numbers: (life, poison, hand size, library size).
type Numbers = (i32, u32, u32, u32);

fn synth_state(turn: u32, step: &str, numbers: [Numbers; 2], cards: Vec<StateCard>) -> GameState {
    let player_states = numbers
        .iter()
        .enumerate()
        .map(|(seat, (life, poison, hand, library))| PlayerState {
            seat: seat as u32,
            life: Some(*life),
            hand_size: Some(*hand),
            library_size: Some(*library),
            poison: Some(*poison),
        })
        .collect();
    GameState {
        seq: None,
        match_id: Some("synthetic-match".to_string()),
        game_id: SYNTH_GAME.to_string(),
        game_number: Some(1),
        games_played: None,
        wins_by_seat: Vec::new(),
        is_match_over: false,
        turn_number: Some(turn),
        active_player: (turn > 0).then_some(if turn % 2 == 1 { 0 } else { 1 }),
        winner: None,
        players: vec![(0, PLAYER.to_string()), (1, OPPONENT.to_string())],
        player_states,
        sideboarding: false,
        prompt: None,
        cards,
        phase: Some(step.to_string()),
        step: Some(step.to_string()),
        priority_player: Some(0),
        server_now_ms: None,
        stack: Vec::new(),
    }
}

fn synth_card(id: u64, name: &str, zone: StateZone, owner: u32, types: &[&str]) -> StateCard {
    let creature = types.contains(&"Creature");
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
        power: Some(if creature { 2 } else { 0 }),
        toughness: Some(if creature { 2 } else { 0 }),
        types: types.iter().map(|name| name.to_string()).collect(),
        counters: Vec::new(),
        attached_to: None,
    }
}

fn in_zone(card: &StateCard, zone: StateZone) -> StateCard {
    let mut card = card.clone();
    card.zone = zone;
    card
}

fn synth_body() -> MatchBody {
    let person = |id: &str, name: &str| Participant {
        user_id: id.to_string(),
        username: name.to_string(),
        bot: false,
    };
    MatchBody {
        id: "synthetic-match".to_string(),
        db_id: None,
        lobby_id: None,
        forge_game_id: SYNTH_GAME.to_string(),
        format_id: Some("modern".to_string()),
        ranked: false,
        games_per_match: Some(1),
        started_at_ms: Some(1_800_000_000_000),
        my_player_index: Some(0),
        my_deck: ["Forest", "Bear", "Sword", "Island"]
            .iter()
            .map(|name| name.to_string())
            .collect(),
        my_sideboard: Vec::new(),
        my_deck_art: Vec::new(),
        participants: vec![person("u0", PLAYER), person("u1", OPPONENT)],
    }
}

/// The synthetic match: a pregame of three cards each, then five turns.
fn synthetic_unit() -> Unit {
    use StateZone::{Battlefield, Hand, Stack};
    let forest = synth_card(1, "Forest", Hand, 0, &["Land"]);
    let bear = synth_card(2, "Bear", Hand, 0, &["Creature"]);
    let sword = synth_card(3, "Sword", Hand, 0, &["Artifact"]);
    let drawn = synth_card(4, "Island", Hand, 0, &["Land"]);
    let swamp = synth_card(20, "Swamp", Battlefield, 1, &["Land"]);
    let mut zombie = synth_card(30, "Zombie Token", Battlefield, 1, &["Creature"]);
    zombie.is_token = true;

    let mut synth = Synth::default();
    synth.push(FrameKind::MatchBody(synth_body()));
    let mut started = synth_event("GAME_STARTED");
    started.player_index = Some(0);
    started.toss_player = Some(0);
    synth.event(started);

    // The pregame: both keep three cards.
    let hand = vec![forest.clone(), bear.clone(), sword.clone()];
    let pregame = [(20, 0, 3, 7), (20, 0, 3, 7)];
    synth.state(synth_state(0, "UNKNOWN", pregame, hand.clone()));
    for seat in [0, 1] {
        let mut kept = synth_event("HAND_KEPT");
        kept.player_index = Some(seat);
        synth.event(kept);
    }

    // Turn 1: the user plays a Forest.
    synth.turn(1, 0);
    synth.phases(&["UNTAP", "UPKEEP", "MAIN1"]);
    synth.zone_change(Some(1), "Forest", Some("Hand"), "Battlefield", 0);
    synth.card_event("LAND_PLAYED", 1, "Forest", 0);
    let mut board = vec![in_zone(&forest, Battlefield)];
    let mut user_hand = vec![bear.clone(), sword.clone()];
    let cards = |board: &[StateCard], hand: &[StateCard]| [board, hand].concat();
    synth.state(synth_state(
        1,
        "MAIN1",
        [(20, 0, 2, 7), (20, 0, 3, 7)],
        cards(&board, &user_hand),
    ));

    // Turn 2: the opponent draws unseen and plays a Swamp.
    synth.turn(2, 1);
    synth.phases(&["UNTAP", "UPKEEP", "DRAW"]);
    synth.zone_change(None, "", Some("Library"), "Hand", 1);
    synth.state(synth_state(
        2,
        "DRAW",
        [(20, 0, 2, 7), (20, 0, 4, 6)],
        cards(&board, &user_hand),
    ));
    synth.phases(&["MAIN1"]);
    synth.zone_change(Some(20), "Swamp", Some("Hand"), "Battlefield", 1);
    synth.card_event("LAND_PLAYED", 20, "Swamp", 1);
    board.push(swamp.clone());
    synth.state(synth_state(
        2,
        "MAIN1",
        [(20, 0, 2, 7), (20, 0, 3, 6)],
        cards(&board, &user_hand),
    ));

    // Turn 3: the user draws an Island and casts the Bear, which gets a
    // +1/+1 counter.
    synth.turn(3, 0);
    synth.phases(&["UNTAP", "UPKEEP", "DRAW"]);
    synth.zone_change(None, "", Some("Library"), "Hand", 0);
    user_hand.push(drawn.clone());
    synth.state(synth_state(
        3,
        "DRAW",
        [(20, 0, 3, 6), (20, 0, 3, 6)],
        cards(&board, &user_hand),
    ));
    synth.phases(&["MAIN1"]);
    synth.zone_change(Some(2), "Bear", Some("Hand"), "Stack", 0);
    synth.card_event("SPELL_CAST", 2, "Bear", 0);
    user_hand.retain(|card| card.id != 2);
    board[0].tapped = true;
    let mut cast = cards(&board, &user_hand);
    cast.push(in_zone(&bear, Stack));
    synth.state(synth_state(
        3,
        "MAIN1",
        [(20, 0, 2, 6), (20, 0, 3, 6)],
        cast,
    ));
    synth.card_event("SPELL_RESOLVED", 2, "Bear", 0);
    synth.zone_change(Some(2), "Bear", Some("Stack"), "Battlefield", 0);
    board.push(in_zone(&bear, Battlefield));
    synth.state(synth_state(
        3,
        "MAIN1",
        [(20, 0, 2, 6), (20, 0, 3, 6)],
        cards(&board, &user_hand),
    ));
    board[2].counters = vec![("P1P1".to_string(), 1)];
    board[2].power = Some(3);
    board[2].toughness = Some(3);
    synth.state(synth_state(
        3,
        "MAIN1",
        [(20, 0, 2, 6), (20, 0, 3, 6)],
        cards(&board, &user_hand),
    ));

    // Turn 4: the opponent takes the Bear and makes a Zombie token; the
    // user loses life and gets a poison counter.
    synth.turn(4, 1);
    synth.phases(&["UNTAP", "UPKEEP", "DRAW"]);
    synth.zone_change(None, "", Some("Library"), "Hand", 1);
    synth.state(synth_state(
        4,
        "DRAW",
        [(20, 0, 2, 6), (20, 0, 4, 5)],
        cards(&board, &user_hand),
    ));
    synth.phases(&["MAIN1"]);
    synth.event(synth_event("TOKEN_CREATED"));
    synth.zone_change(Some(30), "Zombie Token", None, "Battlefield", 1);
    board[2].controller_seat = Some(1);
    board.push(zombie.clone());
    synth.state(synth_state(
        4,
        "MAIN1",
        [(18, 1, 2, 6), (20, 0, 4, 5)],
        cards(&board, &user_hand),
    ));

    // Turn 5: the Forest untaps; the user draws, the Sword enters and is
    // attached to the Zombie, comes off it, and the Zombie dies.
    synth.turn(5, 0);
    synth.phases(&["UNTAP", "UPKEEP", "DRAW"]);
    synth.zone_change(None, "", Some("Library"), "Hand", 0);
    board[0].tapped = false;
    let island_two = synth_card(5, "Island", Hand, 0, &["Land"]);
    user_hand.push(island_two);
    synth.state(synth_state(
        5,
        "DRAW",
        [(18, 1, 4, 5), (20, 0, 4, 5)],
        cards(&board, &user_hand),
    ));
    synth.phases(&["MAIN1"]);
    synth.zone_change(Some(3), "Sword", Some("Hand"), "Battlefield", 0);
    user_hand.retain(|card| card.id != 3);
    let mut worn = in_zone(&sword, Battlefield);
    worn.attached_to = Some(30);
    board.push(worn);
    synth.state(synth_state(
        5,
        "MAIN1",
        [(18, 1, 3, 5), (20, 0, 4, 5)],
        cards(&board, &user_hand),
    ));
    board[4].attached_to = None;
    synth.state(synth_state(
        5,
        "MAIN1",
        [(18, 1, 3, 5), (20, 0, 4, 5)],
        cards(&board, &user_hand),
    ));
    board.retain(|card| card.id != 30);
    synth.state(synth_state(
        5,
        "MAIN1",
        [(18, 1, 3, 5), (20, 0, 4, 5)],
        cards(&board, &user_hand),
    ));

    // The opponent concedes.
    let mut outcome = synth_event("GAME_OUTCOME");
    outcome.player_index = Some(0);
    outcome.end_reason = Some("concede".to_string());
    synth.event(outcome);

    Unit {
        unit_id: "synthetic-unit".to_string(),
        started_at: 1_800_000_000_000,
        counter: 1,
        frames: synth.frames,
    }
}

#[test]
fn the_synthetic_board_record() {
    let units = vec![synthetic_unit()];
    let record = match_record(SYNTH_GAME, &units, &ReplayNames::default()).expect("a record");
    assert_eq!(record.games.len(), 1);
    insta::assert_snapshot!("synthetic_board_record", snapshot_text(&record));
}
