//! The writer on small made-up games: what the events add that the committed
//! capture never shows, and what they must not add. The user is seat 0
//! (`Player`), seat 1 is `Opponent`. Each game is joined in turn 3.

use std::collections::BTreeMap;

use ::mtgr::{Game, GameResult, Line, Record, write};
use endstep_protocol::{GameEvent, GameState, StackEntry, StateCard, Zone as StateZone};

use super::events::{CardNames, card_names};
use super::stack::rules_text;
use super::tests::{OPPONENT, PLAYER, card, permanent, state};
use super::{GameWriter, Limits};
use crate::fold::ReplayGame;
use crate::games::{GameRecord, KeptFrame, KeptKind, game_mut};

/// The body lines the writer gives these frames, as written text without
/// the time and the indent.
fn written(frames: Vec<KeptKind>) -> Vec<String> {
    written_with(frames, |_| {})
}

/// `written`, with the game's record changed first (the fold's outcome).
fn written_with(frames: Vec<KeptKind>, change: impl Fn(&mut GameRecord)) -> Vec<String> {
    written_by(frames, change, |_| {})
}

/// `written_with`, with the writer's limits changed too.
fn written_by(
    frames: Vec<KeptKind>,
    change: impl Fn(&mut GameRecord),
    limit: impl Fn(&mut Limits),
) -> Vec<String> {
    let mut games = Vec::new();
    let record = game_mut(&mut games, 1, 0);
    change(record);
    for kind in frames {
        record.frames.push(KeptFrame { at_ms: 0, kind });
    }
    let game = ReplayGame {
        record: games.remove(0),
        deck: None,
    };
    let mut names = BTreeMap::new();
    names.insert(0, PLAYER.to_string());
    names.insert(1, OPPONENT.to_string());
    let mut writer = GameWriter::new(0, &game, names);
    limit(&mut writer.limits);
    assert!(writer.write());
    body(std::mem::take(&mut writer.lines))
}

fn body(lines: Vec<Line>) -> Vec<String> {
    let record = Record {
        games: vec![Game {
            tags: Vec::new(),
            lines,
            result: GameResult::Unknown,
        }],
    };
    let mut out = Vec::new();
    for line in write(&record).lines() {
        if let Some(rest) = line.strip_prefix("0:00 ") {
            out.push(rest.trim_start().to_string());
        }
    }
    out
}

/// Drops the `incomplete` line every one of these games ends with (no end
/// was sent), and the step line written only to hold it.
fn without_ending(mut body: Vec<String>) -> Vec<String> {
    body.pop();
    if matches!(
        body.last().map(String::as_str),
        Some("untap" | "precombat-main")
    ) {
        body.pop();
    }
    body
}

/// What the frames after `first` add to the game that `first` alone writes.
fn added(first: &GameState, rest: Vec<KeptKind>) -> Vec<String> {
    let alone = without_ending(written(vec![KeptKind::State(first.clone())]));
    let mut frames = vec![KeptKind::State(first.clone())];
    frames.extend(rest);
    let mut all = without_ending(written(frames));
    assert_eq!(all[..alone.len()], alone[..]);
    all.split_off(alone.len())
}

fn event(kind: &str) -> GameEvent {
    GameEvent {
        seq: None,
        game_id: "game".to_string(),
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

/// An event of `kind` about the card `id`, by seat `seat`.
fn card_event(kind: &str, id: u64, name: &str, seat: u32) -> KeptKind {
    let mut event = event(kind);
    event.card_id = Some(id);
    event.card_name = Some(name.to_string());
    event.player_index = Some(seat);
    KeptKind::Event(event)
}

fn zone_change(id: u64, name: &str, from: &str, to: &str, owner: u32) -> KeptKind {
    let mut event = event("CARD_ZONE_CHANGE");
    event.card_id = Some(id);
    event.card_name = Some(name.to_string());
    event.from_zone = Some(from.to_string());
    event.to_zone = Some(to.to_string());
    event.zone_owner = Some(owner);
    KeptKind::Event(event)
}

fn turn_began(number: u32, seat: u32) -> KeptKind {
    let mut event = event("TURN_BEGAN");
    event.turn_number = Some(number);
    event.player_index = Some(seat);
    KeptKind::Event(event)
}

fn resolved(id: u64, name: &str, message: &str) -> KeptKind {
    let mut event = event("SPELL_RESOLVED");
    event.card_id = Some(id);
    event.card_name = Some(name.to_string());
    event.message = Some(message.to_string());
    KeptKind::Event(event)
}

fn shuffle(message: &str) -> KeptKind {
    let mut event = event("SHUFFLE");
    event.message = Some(message.to_string());
    KeptKind::Event(event)
}

fn stack_entry(source: &StateCard, ability: bool, text: &str, targets: &[i64]) -> StackEntry {
    StackEntry {
        source_id: source.id,
        source_name: source.name.clone(),
        controller_seat: source.owner_seat,
        is_ability: ability,
        is_trigger: ability,
        text: text.to_string(),
        target_ids: targets.to_vec(),
        target_description: String::new(),
    }
}

fn with_stack(mut state: GameState, entries: Vec<StackEntry>) -> KeptKind {
    state.stack = entries;
    KeptKind::State(state)
}

fn on_stack(card: &StateCard) -> StateCard {
    let mut card = card.clone();
    card.zone = StateZone::Stack;
    card
}

fn in_graveyard(card: &StateCard) -> StateCard {
    let mut card = card.clone();
    card.zone = StateZone::Graveyard;
    card
}

fn lines(text: &[&str]) -> Vec<String> {
    text.iter().map(|line| line.to_string()).collect()
}

#[test]
fn incomplete_comes_before_a_keyframe_that_repaired_drift() {
    let first = state((0, 50), (3, 40), Vec::new());
    // The opponent's hand shrinks with nothing to show for it, then the
    // next turn begins.
    let rest = vec![
        KeptKind::State(state((0, 50), (2, 40), Vec::new())),
        turn_began(4, 1),
    ];
    assert_eq!(
        added(&first, rest),
        lines(&[
            "precombat-main",
            "incomplete \"Endstep did not show every card that changed zone\"",
            "turn 4 \"Opponent\"",
            "keyframe",
            "player \"Player\" life 20 poison 0 library 50",
            "player \"Opponent\" life 20 poison 0 library 40",
            "#2 hand \"Opponent\"",
            "#3 hand \"Opponent\"",
        ])
    );
}

#[test]
fn a_spell_with_targets_that_leaves_unresolved_fizzles() {
    let bolt = card(5, "Bolt", StateZone::Hand, 0, &["Instant"]);
    let bear = permanent(6, "Bear", 1, &["Creature"]);
    let first = state((1, 50), (0, 40), vec![bolt.clone(), bear.clone()]);
    let cast = state((0, 50), (0, 40), vec![on_stack(&bolt), bear.clone()]);
    let gone = state((0, 50), (0, 40), vec![in_graveyard(&bolt), bear.clone()]);
    let rest = vec![
        zone_change(5, "Bolt", "Hand", "Stack", 0),
        card_event("SPELL_CAST", 5, "Bolt", 0),
        with_stack(cast, vec![stack_entry(&bolt, false, "Bolt", &[6])]),
        zone_change(5, "Bolt", "Stack", "Graveyard", 0),
        KeptKind::State(gone),
    ];
    assert_eq!(
        added(&first, rest),
        lines(&[
            "precombat-main",
            "#1 \"Bolt\" cast from hand",
            "#1 \"Bolt\" target #2 \"Bear\"",
            "#1 \"Bolt\" fizzle",
            "#1 \"Bolt\" move stack -> graveyard by rule:fizzle",
        ])
    );
}

#[test]
fn a_spell_without_targets_that_leaves_unresolved_only_moves() {
    let opt = card(5, "Opt", StateZone::Hand, 0, &["Instant"]);
    let first = state((1, 50), (0, 40), vec![opt.clone()]);
    let cast = state((0, 50), (0, 40), vec![on_stack(&opt)]);
    let rest = vec![
        card_event("SPELL_CAST", 5, "Opt", 0),
        with_stack(cast, vec![stack_entry(&opt, false, "Opt", &[])]),
        zone_change(5, "Opt", "Stack", "Graveyard", 0),
    ];
    assert_eq!(
        added(&first, rest),
        lines(&[
            "precombat-main",
            "#1 \"Opt\" cast from hand",
            "#1 \"Opt\" move stack -> graveyard by ?",
        ])
    );
}

#[test]
fn a_targeted_player_is_named_by_the_description() {
    let bolt = card(5, "Bolt", StateZone::Hand, 0, &["Instant"]);
    let first = state((1, 50), (0, 40), vec![bolt.clone()]);
    let cast = state((0, 50), (0, 40), vec![on_stack(&bolt)]);
    let targeting = |description: &str| {
        let mut entry = stack_entry(&bolt, false, "Bolt", &[-1]);
        entry.target_description = description.to_string();
        vec![
            card_event("SPELL_CAST", 5, "Bolt", 0),
            with_stack(cast.clone(), vec![entry]),
        ]
    };
    assert_eq!(
        added(&first, targeting("[Opponent]")),
        lines(&[
            "precombat-main",
            "#1 \"Bolt\" cast from hand",
            "#1 \"Bolt\" target \"Opponent\"",
        ])
    );
    // A description that names no player of the game: the target is left
    // out.
    assert_eq!(
        added(&first, targeting("[Somebody]")),
        lines(&["precombat-main", "#1 \"Bolt\" cast from hand"])
    );
}

#[test]
fn an_event_about_an_unknown_card_writes_nothing() {
    let first = state((0, 50), (0, 40), Vec::new());
    let mut attack = event("ATTACKERS_DECLARED");
    attack.player_index = Some(1);
    attack.attacker_ids = vec![99];
    let mut block = event("BLOCKERS_DECLARED");
    block.player_index = Some(0);
    block.blockers = vec![(98, 99)];
    let mut player_damage = event("PLAYER_DAMAGED");
    player_damage.card_id = Some(99);
    player_damage.amount = Some(2);
    player_damage.player_index = Some(0);
    let mut card_damage = event("CARD_DAMAGED");
    card_damage.card_id = Some(99);
    card_damage.target_card_id = Some(98);
    card_damage.amount = Some(2);
    let rest = vec![
        KeptKind::Event(attack),
        KeptKind::Event(block),
        KeptKind::Event(player_damage),
        KeptKind::Event(card_damage),
        card_event("SPELL_CAST", 99, "Unseen", 0),
        card_event("ABILITY_ACTIVATED", 99, "Unseen", 1),
        card_event("TRIGGER_FIRED", 99, "Unseen", 1),
        resolved(99, "Unseen", "Unseen"),
        zone_change(99, "Unseen", "Graveyard", "Exile", 1),
        zone_change(99, "Unseen", "Hand", "Stack", 0),
        shuffle("Somebody shuffled their library"),
        KeptKind::State(first.clone()),
    ];
    assert_eq!(added(&first, rest), Vec::<String>::new());
}

#[test]
fn a_shuffle_names_its_player_and_ends_the_library_objects() {
    let first = state((0, 50), (0, 40), Vec::new());
    let mut look = event("CARD_REVEALED");
    look.player_index = Some(0);
    look.owner_echo = true;
    look.to_zone = Some("Library".to_string());
    look.card_ids = vec![7];
    look.card_names = vec!["Island".to_string()];
    let rest = vec![
        KeptKind::Event(look),
        shuffle("Player shuffled their library"),
        turn_began(4, 1),
    ];
    assert_eq!(
        added(&first, rest),
        lines(&[
            "precombat-main",
            "new card #1 \"Island\" owner \"Player\" in library",
            "#1 \"Island\" look \"Player\" by ?",
            "\"Player\" shuffle by ?",
            "turn 4 \"Opponent\"",
            "keyframe",
            "player \"Player\" life 20 poison 0 library 50",
            "player \"Opponent\" life 20 poison 0 library 40",
        ])
    );
}

#[test]
fn an_ability_takes_its_text_from_the_stack_and_ceases_as_it_resolves() {
    let bear = permanent(5, "Bear", 0, &["Creature"]);
    let first = state((0, 50), (0, 40), vec![bear.clone()]);
    let same = state((0, 50), (0, 40), vec![bear.clone()]);
    let entry = stack_entry(&bear, true, "Draw a card.", &[]);
    let rest = vec![
        card_event("TRIGGER_FIRED", 5, "Bear", 0),
        with_stack(same.clone(), vec![entry]),
        resolved(5, "Bear", "Draw a card."),
        KeptKind::State(same),
    ];
    assert_eq!(
        added(&first, rest),
        lines(&[
            "precombat-main",
            "new ability #2 triggered of #1 \"Bear\" \"Draw a card.\" controller \"Player\" in stack",
            "#2 resolve",
            "#2 cease by rule:resolve",
        ])
    );
}

#[test]
fn an_ability_gone_from_the_stack_ceases() {
    let bear = permanent(5, "Bear", 0, &["Creature"]);
    let first = state((0, 50), (0, 40), vec![bear.clone()]);
    // No stack entry follows: the text is unknown, and the next state has
    // no such ability any more.
    let rest = vec![
        card_event("ABILITY_ACTIVATED", 5, "Bear", 0),
        KeptKind::State(first.clone()),
    ];
    assert_eq!(
        added(&first, rest),
        lines(&[
            "precombat-main",
            "new ability #2 activated of #1 \"Bear\" ? controller \"Player\" in stack",
            "#2 cease by ?",
        ])
    );
}

#[test]
fn nothing_is_left_on_the_stack_when_a_turn_begins() {
    let bear = permanent(5, "Bear", 0, &["Creature"]);
    let bolt = card(6, "Bolt", StateZone::Hand, 0, &["Instant"]);
    let first = state((1, 50), (0, 40), vec![bear.clone(), bolt]);
    // An ability and a spell go on the stack, and the next turn begins with
    // no word of either: the ability ceases, and the spell (a card cannot
    // cease) makes the record incomplete up to the keyframe.
    let rest = vec![
        card_event("TRIGGER_FIRED", 5, "Bear", 0),
        card_event("SPELL_CAST", 6, "Bolt", 0),
        turn_began(4, 1),
    ];
    assert_eq!(
        added(&first, rest),
        lines(&[
            "precombat-main",
            "new ability #3 triggered of #1 \"Bear\" ? controller \"Player\" in stack",
            "#2 \"Bolt\" cast from hand",
            "#3 cease by ?",
            "incomplete \"Endstep did not show every card that changed zone\"",
            "turn 4 \"Opponent\"",
            "keyframe",
            "player \"Player\" life 20 poison 0 library 50",
            "player \"Opponent\" life 20 poison 0 library 40",
            "#1 \"Bear\" battlefield \"Player\" types creature pt 0/0",
        ])
    );
}

#[test]
fn a_flood_of_triggers_adds_a_bounded_number_of_abilities() {
    let bear = permanent(5, "Bear", 0, &["Creature"]);
    let first = state((0, 50), (0, 40), vec![bear]);
    let mut rest = Vec::new();
    for _ in 0..250 {
        rest.push(card_event("TRIGGER_FIRED", 5, "Bear", 0));
    }
    let abilities = added(&first, rest)
        .iter()
        .filter(|line| line.starts_with("new ability"))
        .count();
    assert_eq!(abilities, 200);
}

#[test]
fn an_event_does_not_look_past_a_thousand_frames_for_a_state() {
    let bear = permanent(5, "Bear", 0, &["Creature"]);
    let first = state((0, 50), (0, 40), vec![bear.clone()]);
    let entry = stack_entry(&bear, true, "Draw a card.", &[]);
    let shown = with_stack(first.clone(), vec![entry]);
    // The state that has the ability's text is too far ahead to be read.
    let mut rest = vec![card_event("TRIGGER_FIRED", 5, "Bear", 0)];
    for _ in 0..1_000 {
        rest.push(KeptKind::Event(event("MANA_LOG")));
    }
    rest.push(shown);
    assert_eq!(
        added(&first, rest),
        lines(&[
            "precombat-main",
            "new ability #2 triggered of #1 \"Bear\" ? controller \"Player\" in stack",
        ])
    );
}

#[test]
fn a_move_onto_the_stack_alone_is_not_a_cast() {
    // The user starts to cast Bolt and takes it back: the card is listed
    // nowhere while its cost is paid, then in the hand again.
    let bolt = card(5, "Bolt", StateZone::Hand, 0, &["Instant"]);
    let first = state((1, 50), (2, 40), vec![bolt.clone()]);
    let paying = state((0, 50), (2, 40), Vec::new());
    let rest = vec![
        zone_change(5, "Bolt", "Hand", "Stack", 0),
        KeptKind::State(paying),
        KeptKind::State(first.clone()),
    ];
    assert_eq!(added(&first, rest), Vec::<String>::new());

    // The opponent does the same: the card was shown, so the hidden hand
    // object is named, and that is all.
    let shown = state((1, 50), (1, 40), vec![bolt.clone()]);
    let rest = vec![
        zone_change(9, "Shock", "Hand", "Stack", 1),
        KeptKind::State(shown),
        KeptKind::State(first.clone()),
        turn_began(4, 1),
    ];
    assert_eq!(
        added(&first, rest),
        lines(&[
            "precombat-main",
            "#2 identify \"Shock\"",
            "turn 4 \"Opponent\"",
            "keyframe",
            "player \"Player\" life 20 poison 0 library 50",
            "player \"Opponent\" life 20 poison 0 library 40",
            "#1 \"Bolt\" hand \"Player\"",
            "#2 \"Shock\" hand \"Opponent\"",
            "#3 hand \"Opponent\"",
        ])
    );
}

#[test]
fn a_token_is_new_at_the_event_that_creates_it() {
    let first = state((0, 50), (0, 40), Vec::new());
    let mut spawn = permanent(8, "Spawn", 0, &["Creature"]);
    spawn.is_token = true;
    let mut created = event("CARD_ZONE_CHANGE");
    created.card_id = Some(8);
    created.card_name = Some("Spawn".to_string());
    created.to_zone = Some("Battlefield".to_string());
    created.zone_owner = Some(0);
    let rest = vec![
        KeptKind::Event(created.clone()),
        shuffle("Player shuffled their library"),
        KeptKind::State(state((0, 50), (0, 40), vec![spawn])),
    ];
    assert_eq!(
        added(&first, rest),
        lines(&[
            "precombat-main",
            "new token #1 \"Spawn\" owner \"Player\" in battlefield types creature pt 0/0 by ?",
            "\"Player\" shuffle by ?",
        ])
    );

    // A created object the next state does not show as a token on the
    // battlefield (a Forge effect, a token already gone) writes nothing.
    let rest = vec![KeptKind::Event(created), KeptKind::State(first.clone())];
    assert_eq!(added(&first, rest), Vec::<String>::new());
}

#[test]
fn an_outcome_without_a_winner_ends_the_game() {
    let first = state((0, 50), (0, 40), Vec::new());
    let body = written(vec![
        KeptKind::State(first),
        KeptKind::Event(event("GAME_OUTCOME")),
    ]);
    assert_eq!(
        body.last().map(String::as_str),
        Some("incomplete \"Endstep sent the end of the game without a winner\"")
    );
    assert_eq!(
        body.iter()
            .filter(|line| line.starts_with("incomplete"))
            .count(),
        2,
        "the joined game's start and the end without a winner: {body:?}"
    );
}

#[test]
fn poison_is_written_and_the_turn_is_complete() {
    let first = state((0, 50), (0, 40), Vec::new());
    let mut poisoned = first.clone();
    poisoned.player_states[1].poison = Some(1);
    let rest = vec![KeptKind::State(poisoned), turn_began(4, 1)];
    assert_eq!(
        added(&first, rest),
        lines(&[
            "precombat-main",
            "\"Opponent\" poison 0 -> 1 by ?",
            "turn 4 \"Opponent\"",
            "keyframe",
            "player \"Player\" life 20 poison 0 library 50",
            "player \"Opponent\" life 20 poison 1 library 40",
        ])
    );
}

#[test]
fn a_damage_cause_takes_one_life_change_only() {
    let bear = permanent(5, "Bear", 0, &["Creature"]);
    let first = state((0, 50), (0, 40), vec![bear]);
    let mut damage = event("PLAYER_DAMAGED");
    damage.card_id = Some(5);
    damage.amount = Some(2);
    damage.player_index = Some(1);
    let life = |to: i32| {
        let mut life = event("PLAYER_LIFE_CHANGED");
        life.player_index = Some(1);
        life.new_value = Some(to);
        KeptKind::Event(life)
    };
    let rest = vec![KeptKind::Event(damage), life(18), life(17)];
    assert_eq!(
        added(&first, rest),
        lines(&[
            "precombat-main",
            "#1 \"Bear\" damage \"Opponent\" 2 by #1",
            "\"Opponent\" life 20 -> 18 by #1",
            "\"Opponent\" life 18 -> 17 by ?",
        ])
    );
}

#[test]
fn a_spell_cast_again_reads_its_targets_again() {
    let bolt = card(5, "Bolt", StateZone::Hand, 0, &["Instant"]);
    let bear = permanent(6, "Bear", 1, &["Creature"]);
    let first = state((1, 50), (0, 40), vec![bolt.clone(), bear.clone()]);
    let cast = state((0, 50), (0, 40), vec![on_stack(&bolt), bear.clone()]);
    let gone = state((0, 50), (0, 40), vec![in_graveyard(&bolt), bear]);
    let at_bear = vec![stack_entry(&bolt, false, "Bolt", &[6])];
    let first_cast = vec![
        card_event("SPELL_CAST", 5, "Bolt", 0),
        with_stack(cast.clone(), at_bear.clone()),
        resolved(5, "Bolt", "Bolt"),
        zone_change(5, "Bolt", "Stack", "Graveyard", 0),
        KeptKind::State(gone),
    ];

    // Cast again from the graveyard at the same target.
    let mut rest = first_cast.clone();
    rest.push(card_event("SPELL_CAST", 5, "Bolt", 0));
    rest.push(with_stack(cast.clone(), at_bear));
    let body = added(&first, rest);
    let target = "#1 \"Bolt\" target #2 \"Bear\"";
    assert_eq!(
        body.iter().filter(|line| line.as_str() == target).count(),
        2,
        "{body:?}"
    );

    // Cast again with no target and gone with nothing resolving: it did
    // not fizzle.
    let mut rest = first_cast;
    rest.push(card_event("SPELL_CAST", 5, "Bolt", 0));
    rest.push(with_stack(
        cast,
        vec![stack_entry(&bolt, false, "Bolt", &[])],
    ));
    rest.push(zone_change(5, "Bolt", "Stack", "Graveyard", 0));
    let body = added(&first, rest);
    assert!(body.iter().all(|line| !line.contains("fizzle")), "{body:?}");
}

#[test]
fn forge_notes_and_ids_are_cut_from_ability_text() {
    // The names the game knows each card id under: an id is cut only after
    // its card's name.
    let known = [
        (78, "Hedge Maze"),
        (85, "Violent Outburst"),
        (118, "Oliphaunt"),
        (94, "Thundering Falls"),
        (16, "Sowing Mycospawn"),
        (2, "Bear"),
    ];
    let mut cards: CardNames = BTreeMap::new();
    for (id, name) in known {
        cards.entry(id).or_default().insert(name.to_string());
    }
    let cut = |text: &str| rules_text(text, &cards);
    assert_eq!(
        cut(
            "When Hedge Maze enters, surveil 1. (Look at the top card.) [Zone Changer: Hedge Maze (78)]"
        ),
        Some("When Hedge Maze enters, surveil 1. (Look at the top card.)".to_string())
    );
    assert_eq!(
        cut(
            "Cascade - Violent Outburst [Card: Violent Outburst (85), Activator: Opponent, SpellAbility: Creatures you control get +1/+0 until end of turn.]"
        ),
        Some("Cascade - Violent Outburst".to_string())
    );
    assert_eq!(
        cut("Oliphaunt (118) - Opponent searches their library for a Mountain card."),
        Some("Oliphaunt - Opponent searches their library for a Mountain card.".to_string())
    );
    assert_eq!(
        cut(
            "Exile target land. (Targeting: [[Thundering Falls (94)]]) [Card: Sowing Mycospawn (16), Activator: Player, SpellAbility: Sowing Mycospawn - Creature 3 / 3]"
        ),
        Some("Exile target land.".to_string())
    );
    // Rules text keeps its own brackets, numbers and reminder text.
    assert_eq!(
        cut("Scry 2. (Look at the top two cards.)"),
        Some("Scry 2. (Look at the top two cards.)".to_string())
    );
    assert_eq!(cut("[Zone Changer: Bear (5)]"), None);
    // A number in parentheses that does not follow its card's name is rules
    // text, even when it is a card id of the game.
    assert_eq!(
        cut("Each player discards a card (2)."),
        Some("Each player discards a card (2).".to_string())
    );
    assert_eq!(
        cut("Bear (2) fights Bear (3)."),
        Some("Bear fights Bear (3).".to_string())
    );
    // A name that itself holds ` (` still matches across the earlier cut.
    let mut odd: CardNames = BTreeMap::new();
    odd.entry(7).or_default().insert("Kill (Shot".to_string());
    assert_eq!(
        rules_text("Exile Kill (Shot (7) now.", &odd),
        Some("Exile Kill (Shot now.".to_string())
    );
    assert_eq!(cut("unclosed ] note"), Some("unclosed ] note".to_string()));
    assert_eq!(cut(""), None);
}

#[test]
fn the_ending_line_names_the_folds_winner() {
    // A capture with two outcomes for one game: the fold keeps the first
    // (the user won), so the ending line must say the same, whatever the
    // outcome the writer reads says.
    let first = state((0, 50), (0, 40), Vec::new());
    let mut outcome = event("GAME_OUTCOME");
    outcome.player_index = Some(1);
    outcome.end_reason = Some("concede".to_string());
    let body = written_with(
        vec![KeptKind::State(first), KeptKind::Event(outcome)],
        |record| {
            record.decided = true;
            record.winner_seat = Some(0);
            record.end_reason = Some("concede".to_string());
        },
    );
    assert_eq!(
        body.last().map(String::as_str),
        Some("\"Opponent\" concede")
    );
}

#[test]
fn a_turn_with_no_active_player_is_not_given_a_seat() {
    // Turn 4's `TURN_BEGAN` was missed and its state names no active
    // player: no seat is guessed. The record is incomplete until turn 5,
    // whose `TURN_BEGAN` names its player.
    let first = state((0, 50), (0, 40), Vec::new());
    let mut fourth = first.clone();
    fourth.turn_number = Some(4);
    fourth.active_player = None;
    let rest = vec![KeptKind::State(fourth), turn_began(5, 0)];
    let body = added(&first, rest);
    assert_eq!(
        body[..2],
        lines(&[
            "precombat-main",
            "incomplete \"Endstep did not say who began turn 4\"",
        ])[..],
        "{body:?}"
    );
    assert!(
        body.iter().all(|line| !line.starts_with("turn 4")),
        "{body:?}"
    );
    assert_eq!(body[2], "turn 5 \"Player\"");
}

/// A state of turn 3 with `count` of the user's permanents.
fn bears(count: u64) -> KeptKind {
    let cards = (1..=count)
        .map(|id| permanent(id, "Bear", 0, &["Creature"]))
        .collect();
    KeptKind::State(state((0, 50), (0, 40), cards))
}

const TOO_BIG: &str = "incomplete \"The game is too big to write whole\"";

#[test]
fn a_game_with_too_many_objects_is_cut_short() {
    let frames = vec![bears(2), bears(4), bears(6)];
    let body = written_by(frames, |_| {}, |limits| limits.objects = 3);
    assert_eq!(body.last().map(String::as_str), Some(TOO_BIG), "{body:?}");
    assert_eq!(
        body.iter()
            .filter(|line| line.starts_with("incomplete"))
            .count(),
        2
    );
    // The third state is not read.
    assert!(body.iter().all(|line| !line.contains("#5")), "{body:?}");
}

#[test]
fn a_game_with_too_many_lines_is_cut_short() {
    let frames = vec![bears(1), bears(2), bears(3), bears(4)];
    let whole = written(frames.clone());
    let body = written_by(frames, |_| {}, |limits| limits.lines = whole.len() - 2);
    assert_eq!(body.last().map(String::as_str), Some(TOO_BIG), "{body:?}");
    assert!(body.len() < whole.len(), "{body:?}");
}

fn phase(name: &str) -> KeptKind {
    let mut event = event("TURN_PHASE");
    event.phase = Some(name.to_string());
    KeptKind::Event(event)
}

#[test]
fn the_active_players_untaps_are_in_the_untap_step() {
    // The state that shows the Forest untapped comes after the step events
    // of the turn's start: its untap is still the untap step's.
    let mut forest = permanent(5, "Forest", 0, &["Land"]);
    forest.tapped = true;
    let mut bear = permanent(6, "Bear", 1, &["Creature"]);
    bear.tapped = true;
    let first = state((0, 50), (0, 40), vec![forest.clone(), bear.clone()]);
    forest.tapped = false;
    bear.tapped = false;
    let mut fourth = state((0, 50), (0, 40), vec![forest, bear]);
    fourth.turn_number = Some(4);
    let rest = vec![
        turn_began(4, 0),
        phase("UNTAP"),
        phase("UPKEEP"),
        phase("DRAW"),
        phase("MAIN1"),
        KeptKind::State(fourth),
    ];
    let body = added(&first, rest);
    let at = body
        .iter()
        .position(|line| line == "untap")
        .expect("an untap step");
    assert_eq!(
        body[at..],
        lines(&[
            "untap",
            "#1 \"Forest\" untap by rule:untap",
            "upkeep",
            "draw",
            "precombat-main",
            // Not the active player's: an effect untapped it, not the step.
            "#2 \"Bear\" untap by ?",
        ])[..],
        "{body:?}"
    );
}

#[test]
fn a_reveal_of_a_reissued_id_is_not_the_named_hand_card() {
    // The opponent starts casting Force (id 9) and takes it back: it is
    // named in their hidden hand. A reveal then names id 9 as an Island.
    let first = state((0, 50), (2, 40), Vec::new());
    let mut reveal = event("CARD_REVEALED");
    reveal.player_index = Some(0);
    reveal.to_zone = Some("Hand".to_string());
    reveal.card_ids = vec![9];
    reveal.card_names = vec!["Island".to_string()];
    let rest = vec![
        zone_change(9, "Force", "Hand", "Stack", 1),
        KeptKind::State(first.clone()),
        KeptKind::Event(reveal),
    ];
    let body = added(&first, rest);
    assert!(
        body.iter().all(|line| !line.contains("\"Force\" reveal")),
        "{body:?}"
    );
    assert!(
        body.iter().any(|line| line == "#2 \"Island\" reveal by ?"),
        "{body:?}"
    );
}

#[test]
fn the_steps_of_a_turn_with_no_player_are_not_written() {
    // Turn 4 has no named player: its steps would land under turn 3 after
    // the `incomplete` line, so they are not written; its draw is.
    let first = state((0, 50), (0, 40), Vec::new());
    let mut fourth = first.clone();
    fourth.turn_number = Some(4);
    fourth.active_player = None;
    let mut draw = event("CARD_ZONE_CHANGE");
    draw.from_zone = Some("Library".to_string());
    draw.to_zone = Some("Hand".to_string());
    draw.zone_owner = Some(1);
    let rest = vec![
        KeptKind::State(fourth),
        phase("UPKEEP"),
        phase("DRAW"),
        KeptKind::Event(draw),
        turn_began(5, 0),
    ];
    let body = added(&first, rest);
    let turn = body
        .iter()
        .position(|line| line == "turn 5 \"Player\"")
        .expect("turn 5");
    assert_eq!(
        body[..turn],
        lines(&[
            "precombat-main",
            "incomplete \"Endstep did not say who began turn 4\"",
            "new hidden #1 owner \"Opponent\" in library",
            "#1 move library -> hand as draw by ?",
        ])[..],
        "{body:?}"
    );
}

#[test]
fn card_names_come_from_states_and_events() {
    // Id 40 is never in a state: only an event names it.
    let first = state(
        (0, 50),
        (0, 40),
        vec![permanent(5, "Bear", 0, &["Creature"])],
    );
    let mut games = Vec::new();
    let record = game_mut(&mut games, 1, 0);
    for kind in [
        KeptKind::State(first),
        zone_change(40, "Force", "Hand", "Stack", 1),
    ] {
        record.frames.push(KeptFrame { at_ms: 0, kind });
    }
    let game = ReplayGame {
        record: games.remove(0),
        deck: None,
    };
    let names = card_names(&game);
    let named = |id: u64| {
        names
            .get(&id)
            .map(|names| names.iter().cloned().collect::<Vec<_>>())
    };
    assert_eq!(named(5), Some(vec!["Bear".to_string()]));
    assert_eq!(named(40), Some(vec!["Force".to_string()]));
    assert_eq!(
        rules_text("Force (40) - counter target spell.", &names),
        Some("Force - counter target spell.".to_string())
    );
}
