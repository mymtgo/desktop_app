//! The writer must reproduce the conformance files byte for byte.

use mtgr::{
    AbilityKind, CardType, Cause, Choice, DeckCard, Entering, ExtArg, Game, GameResult, LibraryEnd,
    LibraryPlace, Line, Mana, MoveAction, ObjRef, Position, PtValue, Record, SpecialAction,
    Statement, Step, Tag, TargetRef, Zone, write,
};

const A: &str = "PlayerA";
const B: &str = "PlayerB";

fn tag(name: &str, value: &str) -> Tag {
    Tag {
        name: name.to_string(),
        value: value.to_string(),
    }
}

fn line(secs: i64, level: u8, statement: Statement) -> Line {
    Line {
        time_ms: secs * 1000,
        level,
        statement,
        cause: None,
    }
}

fn line_by(secs: i64, level: u8, statement: Statement, cause: Cause) -> Line {
    Line {
        time_ms: secs * 1000,
        level,
        statement,
        cause: Some(cause),
    }
}

fn obj(id: u32, name: &str) -> ObjRef {
    ObjRef {
        id,
        name: Some(name.to_string()),
    }
}

fn hidden_obj(id: u32) -> ObjRef {
    ObjRef { id, name: None }
}

fn life_entry(player: &str, life: i32, library: u32) -> Statement {
    Statement::KeyframePlayer {
        player: player.to_string(),
        life,
        poison: 0,
        library,
    }
}

fn zone_entry(
    object: ObjRef,
    zone: Zone,
    player: &str,
    types: Vec<CardType>,
    tapped: bool,
) -> Statement {
    Statement::KeyframeObject {
        object,
        zone,
        player: Some(player.to_string()),
        face: None,
        types,
        pt: None,
        tapped,
        attached: None,
        with: None,
        adventure: None,
        companion: false,
        counters: Vec::new(),
        position: None,
    }
}

fn mv(object: ObjRef, from: Zone, to: Zone, action: Option<MoveAction>) -> Statement {
    Statement::Move {
        object,
        from,
        to,
        action,
        entering: None,
        position: None,
        with: None,
    }
}

const OPENING: [(u32, &str); 7] = [
    (1, "Swamp"),
    (2, "Thoughtseize"),
    (3, "Island"),
    (4, "Counterspell"),
    (5, "Polluted Delta"),
    (6, "Consider"),
    (7, "Murktide Regent"),
];

const REVEALED: [(u32, &str); 7] = [
    (8, "Ragavan, Nimble Pilferer"),
    (9, "Lightning Bolt"),
    (10, "Mountain"),
    (11, "Sacred Foundry"),
    (12, "Arid Mesa"),
    (13, "Guide of Souls"),
    (14, "Mountain"),
];

fn opening_hand_lines(lines: &mut Vec<Line>) {
    for (id, name) in OPENING {
        lines.push(line(
            3,
            1,
            Statement::NewCard {
                object: obj(id, name),
                owner: A.to_string(),
                zone: Zone::Library,
                entering: None,
                position: None,
                sources: vec![],
            },
        ));
        lines.push(line_by(
            3,
            1,
            mv(
                obj(id, name),
                Zone::Library,
                Zone::Hand,
                Some(MoveAction::Draw),
            ),
            Cause::Rule("opening-hand".to_string()),
        ));
    }
    for id in 8..=14 {
        lines.push(line(
            3,
            1,
            Statement::NewHidden {
                id,
                owner: B.to_string(),
                zone: Zone::Library,
                position: None,
            },
        ));
        lines.push(line_by(
            3,
            1,
            mv(
                hidden_obj(id),
                Zone::Library,
                Zone::Hand,
                Some(MoveAction::Draw),
            ),
            Cause::Rule("opening-hand".to_string()),
        ));
    }
}

fn turn_one_keyframe(lines: &mut Vec<Line>) {
    lines.push(line(12, 1, Statement::Keyframe));
    lines.push(line(12, 2, life_entry(A, 20, 53)));
    lines.push(line(12, 2, life_entry(B, 20, 53)));
    for (id, name) in OPENING {
        lines.push(line(
            12,
            2,
            zone_entry(obj(id, name), Zone::Hand, A, vec![], false),
        ));
    }
    for id in 8..=14 {
        lines.push(line(
            12,
            2,
            zone_entry(hidden_obj(id), Zone::Hand, B, vec![], false),
        ));
    }
}

fn turn_one_steps(lines: &mut Vec<Line>) {
    lines.push(line(12, 1, Statement::Step(Step::Untap)));
    lines.push(line(12, 1, Statement::Step(Step::Upkeep)));
    lines.push(line(12, 1, Statement::Step(Step::PrecombatMain)));
}

fn thoughtseize_turn(lines: &mut Vec<Line>) {
    let swamp = obj(1, "Swamp");
    let seize = obj(2, "Thoughtseize");
    lines.push(line(
        15,
        2,
        Statement::Play {
            object: swamp.clone(),
            from: Zone::Hand,
            entering: Entering {
                face: None,
                tapped: false,
                types: vec![CardType::Land],
                pt: None,
            },
        },
    ));
    lines.push(line(
        16,
        2,
        Statement::NewAbility {
            id: 15,
            kind: AbilityKind::Mana,
            of: swamp.clone(),
            text: Some("({T}: Add {B}.)".to_string()),
            controller: A.to_string(),
            in_stack: false,
        },
    ));
    lines.push(line_by(
        16,
        2,
        Statement::Tap { object: swamp },
        Cause::Cost(15),
    ));
    lines.push(line_by(
        16,
        2,
        Statement::Add {
            player: A.to_string(),
            mana: vec![Mana::B],
            unknown_restriction: false,
        },
        Cause::Object(15),
    ));
    lines.push(line(
        17,
        2,
        Statement::Cast {
            object: seize.clone(),
            from: Zone::Hand,
            face: None,
        },
    ));
    lines.push(line(
        17,
        2,
        Statement::Target {
            object: seize.clone(),
            target: TargetRef::Player(B.to_string()),
        },
    ));
    lines.push(line_by(
        17,
        2,
        Statement::Pay {
            player: A.to_string(),
            mana: vec![Mana::B],
        },
        Cause::Cost(2),
    ));
    for (secs, who) in [(17, A), (17, B)] {
        lines.push(line(
            17,
            2,
            Statement::Priority {
                player: who.to_string(),
            },
        ));
        lines.push(line(17, 3, Statement::Stack));
        lines.push(line(
            17,
            4,
            Statement::StackEntry {
                object: seize.clone(),
                face: None,
                controller: A.to_string(),
                choices: vec![Choice::Target(TargetRef::Player(B.to_string()))],
            },
        ));
        let pass_time = if who == A { secs } else { 20 };
        lines.push(line(
            pass_time,
            2,
            Statement::Pass {
                player: who.to_string(),
            },
        ));
    }
    lines.push(line(
        20,
        2,
        Statement::Resolve {
            object: seize.clone(),
        },
    ));
    for (id, name) in REVEALED {
        lines.push(line(
            20,
            2,
            Statement::Identify {
                id,
                name: Some(name.to_string()),
                sources: vec![],
            },
        ));
        lines.push(line_by(
            20,
            2,
            Statement::Reveal {
                object: obj(id, name),
            },
            Cause::Object(2),
        ));
    }
    lines.push(line(
        25,
        2,
        Statement::ChooseObject {
            object: seize.clone(),
            chosen: obj(8, "Ragavan, Nimble Pilferer"),
        },
    ));
    lines.push(line_by(
        25,
        2,
        mv(
            obj(8, "Ragavan, Nimble Pilferer"),
            Zone::Hand,
            Zone::Graveyard,
            Some(MoveAction::Discard),
        ),
        Cause::Object(2),
    ));
    lines.push(line_by(
        25,
        2,
        Statement::Life {
            player: A.to_string(),
            from: 20,
            to: 18,
        },
        Cause::Object(2),
    ));
    lines.push(line_by(
        25,
        2,
        mv(seize, Zone::Stack, Zone::Graveyard, None),
        Cause::Rule("resolve".to_string()),
    ));
}

fn end_of_turn_one(lines: &mut Vec<Line>) {
    for step in [
        Step::BeginningOfCombat,
        Step::DeclareAttackers,
        Step::EndOfCombat,
        Step::PostcombatMain,
        Step::End,
        Step::Cleanup,
    ] {
        lines.push(line(28, 1, Statement::Step(step)));
    }
}

fn turn_two(lines: &mut Vec<Line>, conceder: &str) {
    lines.push(line(
        30,
        0,
        Statement::Turn {
            number: 2,
            player: B.to_string(),
        },
    ));
    lines.push(line(30, 1, Statement::Keyframe));
    lines.push(line(30, 2, life_entry(A, 18, 53)));
    lines.push(line(30, 2, life_entry(B, 20, 53)));
    lines.push(line(
        30,
        2,
        zone_entry(
            obj(1, "Swamp"),
            Zone::Battlefield,
            A,
            vec![CardType::Land],
            true,
        ),
    ));
    lines.push(line(
        30,
        2,
        zone_entry(obj(2, "Thoughtseize"), Zone::Graveyard, A, vec![], false),
    ));
    for (id, name) in &OPENING[2..] {
        lines.push(line(
            30,
            2,
            zone_entry(obj(*id, name), Zone::Hand, A, vec![], false),
        ));
    }
    lines.push(line(
        30,
        2,
        zone_entry(
            obj(8, "Ragavan, Nimble Pilferer"),
            Zone::Graveyard,
            B,
            vec![],
            false,
        ),
    ));
    for (id, name) in &REVEALED[1..] {
        lines.push(line(
            30,
            2,
            zone_entry(obj(*id, name), Zone::Hand, B, vec![], false),
        ));
    }
    lines.push(line(30, 1, Statement::Step(Step::Untap)));
    lines.push(line(30, 1, Statement::Step(Step::Upkeep)));
    lines.push(line(31, 1, Statement::Step(Step::Draw)));
    lines.push(line(
        31,
        2,
        Statement::NewHidden {
            id: 16,
            owner: B.to_string(),
            zone: Zone::Library,
            position: None,
        },
    ));
    lines.push(line_by(
        31,
        2,
        mv(
            hidden_obj(16),
            Zone::Library,
            Zone::Hand,
            Some(MoveAction::Draw),
        ),
        Cause::Rule("draw".to_string()),
    ));
    lines.push(line(31, 1, Statement::Step(Step::PrecombatMain)));
    lines.push(line(
        35,
        2,
        Statement::Concede {
            player: conceder.to_string(),
        },
    ));
}

struct Opts {
    number: &'static str,
    match_id: &'static str,
    chooser: &'static str,
    conceder: &'static str,
    result: GameResult,
    note: bool,
    rolls: bool,
    escape_and_seen: bool,
}

fn conformance_game(o: Opts) -> Game {
    let result_text = if o.result == GameResult::Player1 {
        "1-0"
    } else {
        "0-1"
    };
    // Deliberately out of order: the writer must sort the tags.
    let mut tags = vec![
        tag("Termination", "concession"),
        tag("Perspective", A),
        tag("Game", o.number),
        tag("Match", o.match_id),
        tag("Format", "Modern"),
        tag("Result", result_text),
        tag("Player2", B),
        tag("Player1", A),
        tag("Round", "-"),
        tag("Date", "2026.11.02"),
        tag("Site", "?"),
        tag("Event", "Conformance"),
        tag("Mtgr", "0.3.0"),
    ];
    if o.note {
        tags.push(tag("MYMTGO_Note", "fixture"));
    }

    let mut lines = vec![line(
        0,
        0,
        Statement::Deck {
            player: A.to_string(),
            name: None,
            unknown: true,
        },
    )];
    if o.escape_and_seen {
        lines.push(line(
            0,
            0,
            Statement::Escape("escape line: parsers ignore this".to_string()),
        ));
    }
    lines.push(line(0, 0, Statement::Pregame));
    lines.push(line(0, 1, Statement::Keyframe));
    lines.push(line(0, 2, life_entry(A, 20, 60)));
    lines.push(line(0, 2, life_entry(B, 20, 60)));
    if o.rolls {
        lines.push(line(
            1,
            1,
            Statement::Roll {
                player: A.to_string(),
                sides: Some(6),
                value: 5,
            },
        ));
        lines.push(line(
            1,
            1,
            Statement::Roll {
                player: B.to_string(),
                sides: Some(6),
                value: 2,
            },
        ));
    }
    lines.push(line(
        2,
        1,
        Statement::ChooseFirst {
            player: o.chooser.to_string(),
            first: A.to_string(),
        },
    ));
    opening_hand_lines(&mut lines);
    lines.push(line(
        10,
        1,
        Statement::Keep {
            player: A.to_string(),
        },
    ));
    if o.escape_and_seen {
        lines.push(line(
            10,
            1,
            Statement::Extension {
                name: "MYMTGO_seen".to_string(),
                args: vec![
                    ExtArg::Text(A.to_string()),
                    ExtArg::Int(3),
                    ExtArg::Word("hand".to_string()),
                ],
            },
        ));
    }
    lines.push(line(
        12,
        1,
        Statement::Keep {
            player: B.to_string(),
        },
    ));
    lines.push(line(
        12,
        0,
        Statement::Turn {
            number: 1,
            player: A.to_string(),
        },
    ));
    turn_one_keyframe(&mut lines);
    turn_one_steps(&mut lines);
    thoughtseize_turn(&mut lines);
    end_of_turn_one(&mut lines);
    turn_two(&mut lines, o.conceder);
    Game {
        tags,
        lines,
        result: o.result,
    }
}

#[test]
fn writes_basic_conformance_file() {
    let game = conformance_game(Opts {
        number: "1",
        match_id: "conformance-basic",
        chooser: A,
        conceder: B,
        result: GameResult::Player1,
        note: false,
        rolls: true,
        escape_and_seen: false,
    });
    let text = write(&Record { games: vec![game] });
    assert_eq!(text, include_str!("fixtures/basic.mtgr"));
}

#[test]
fn writes_two_games_conformance_file() {
    let game_one = conformance_game(Opts {
        number: "1",
        match_id: "conformance-two-games",
        chooser: A,
        conceder: B,
        result: GameResult::Player1,
        note: true,
        rolls: true,
        escape_and_seen: true,
    });
    let game_two = conformance_game(Opts {
        number: "2",
        match_id: "conformance-two-games",
        chooser: B,
        conceder: A,
        result: GameResult::Player2,
        note: true,
        rolls: false,
        escape_and_seen: false,
    });
    let text = write(&Record {
        games: vec![game_one, game_two],
    });
    assert_eq!(text, include_str!("fixtures/two-games.mtgr"));
}

#[test]
fn quotes_and_escapes_strings() {
    let game = Game {
        tags: vec![tag("Event", "A \"quoted\" \\ name\nline")],
        lines: vec![line(61, 0, Statement::Pregame)],
        result: GameResult::Unknown,
    };
    let text = write(&Record { games: vec![game] });
    assert!(text.contains("[Event \"A \\\"quoted\\\" \\\\ name\\nline\"]\n"));
    assert!(text.contains("\n1:01 pregame\n*\n"));
}

fn pregame_game(statement: Statement) -> String {
    let game = Game {
        tags: vec![tag("Event", "x\ty\rz")],
        lines: vec![line(0, 0, statement)],
        result: GameResult::Unknown,
    };
    write(&Record { games: vec![game] })
}

#[test]
fn escape_text_cannot_start_a_second_line() {
    let text = pregame_game(Statement::Escape("a\nb\r\tc".to_string()));
    assert!(text.contains("\n% a b  c\n*\n"));
    assert_eq!(text.lines().count(), 4);
}

#[test]
fn control_characters_in_strings_become_spaces() {
    let text = pregame_game(Statement::Deck {
        player: "Play\ter\r\u{7}X".to_string(),
        name: None,
        unknown: true,
    });
    assert!(text.contains("[Event \"x y z\"]\n"));
    assert!(text.contains("0:00 deck \"Play er  X\" unknown\n"));
    assert!(!text.contains('\t') && !text.contains('\r'));
}

#[test]
#[should_panic(expected = "bad rule name")]
fn bad_rule_name_is_caught_in_debug_builds() {
    let mut l = line(0, 0, Statement::Pregame);
    l.cause = Some(Cause::Rule("Bad Rule".to_string()));
    let game = Game {
        tags: vec![],
        lines: vec![l],
        result: GameResult::Unknown,
    };
    write(&Record { games: vec![game] });
}

/// The body line one statement writes, without time and indent.
fn statement_line(statement: Statement, cause: Option<Cause>) -> String {
    let game = Game {
        tags: vec![],
        lines: vec![Line {
            time_ms: 0,
            level: 0,
            statement,
            cause,
        }],
        result: GameResult::Unknown,
    };
    let text = write(&Record { games: vec![game] });
    let body = text.lines().nth(1).expect("a body line");
    body.strip_prefix("0:00 ").expect("a time").to_string()
}

#[test]
fn writes_object_event_statements() {
    let bear = obj(5, "Grizzly Bears");
    let cases = vec![
        (
            Statement::NewToken {
                object: obj(22, "Samurai Token"),
                owner: A.to_string(),
                zone: Zone::Battlefield,
                entering: Entering {
                    face: None,
                    tapped: false,
                    types: vec![CardType::Creature],
                    pt: Some((2, 2)),
                },
                sources: vec![],
            },
            Some(Cause::Object(21)),
            "new token #22 \"Samurai Token\" owner \"PlayerA\" in battlefield types creature pt 2/2 by #21",
        ),
        (
            Statement::NewAbility {
                id: 23,
                kind: AbilityKind::Triggered,
                of: bear.clone(),
                text: None,
                controller: A.to_string(),
                in_stack: true,
            },
            None,
            "new ability #23 triggered of #5 \"Grizzly Bears\" ? controller \"PlayerA\" in stack",
        ),
        (
            Statement::Untap {
                object: bear.clone(),
            },
            Some(Cause::Rule("untap".to_string())),
            "#5 \"Grizzly Bears\" untap by rule:untap",
        ),
        (
            Statement::Attack {
                object: bear.clone(),
                target: TargetRef::Player(B.to_string()),
            },
            None,
            "#5 \"Grizzly Bears\" attack \"PlayerB\"",
        ),
        (
            Statement::Block {
                object: obj(6, "Wall"),
                attacker: bear.clone(),
            },
            None,
            "#6 \"Wall\" block #5 \"Grizzly Bears\"",
        ),
        (
            Statement::Damage {
                object: bear.clone(),
                target: TargetRef::Player(B.to_string()),
                amount: 2,
            },
            Some(Cause::Rule("combat-damage".to_string())),
            "#5 \"Grizzly Bears\" damage \"PlayerB\" 2 by rule:combat-damage",
        ),
        (
            Statement::Counters {
                object: bear.clone(),
                kind: "+1/+1".to_string(),
                from: 0,
                to: 2,
            },
            Some(Cause::Unknown),
            "#5 \"Grizzly Bears\" counters \"+1/+1\" 0 -> 2 by ?",
        ),
        (
            Statement::PtChange {
                object: bear.clone(),
                from: PtValue::None,
                to: PtValue::Pt(-1, 3),
            },
            Some(Cause::Object(9)),
            "#5 \"Grizzly Bears\" pt none -> -1/3 by #9",
        ),
        (
            Statement::PtChange {
                object: bear.clone(),
                from: PtValue::Unknown,
                to: PtValue::None,
            },
            Some(Cause::Object(9)),
            "#5 \"Grizzly Bears\" pt ? -> none by #9",
        ),
        (
            Statement::TypesChange {
                object: bear.clone(),
                from: vec![CardType::Land],
                to: vec![CardType::Creature, CardType::Land],
            },
            Some(Cause::Object(9)),
            "#5 \"Grizzly Bears\" types land -> creature land by #9",
        ),
        (
            Statement::TypesChange {
                object: bear.clone(),
                from: vec![],
                to: vec![CardType::Artifact],
            },
            Some(Cause::Unknown),
            "#5 \"Grizzly Bears\" types ? -> artifact by ?",
        ),
        (
            Statement::Countered {
                object: obj(9, "Lightning Bolt"),
            },
            Some(Cause::Object(8)),
            "#9 \"Lightning Bolt\" countered by #8",
        ),
        (
            Statement::Attach {
                object: obj(12, "Pacifism"),
                to: bear.clone(),
            },
            Some(Cause::Rule("resolve".to_string())),
            "#12 \"Pacifism\" attach #5 \"Grizzly Bears\" by rule:resolve",
        ),
        (
            Statement::Unattach {
                object: obj(14, "Bonesplitter"),
            },
            Some(Cause::Rule("sba".to_string())),
            "#14 \"Bonesplitter\" unattach by rule:sba",
        ),
        (
            Statement::NewCard {
                object: obj(1, "Kaheera, the Orphanguard"),
                owner: A.to_string(),
                zone: Zone::OutsideSideboard,
                entering: None,
                position: None,
                sources: vec![],
            },
            None,
            "new card #1 \"Kaheera, the Orphanguard\" owner \"PlayerA\" in outside \"sideboard\"",
        ),
        (
            Statement::Special {
                object: obj(1, "Kaheera, the Orphanguard"),
                action: SpecialAction::Companion,
            },
            None,
            "#1 \"Kaheera, the Orphanguard\" special companion",
        ),
        (
            Statement::Incomplete {
                reason: "a \"zone\"".to_string(),
            },
            None,
            "incomplete \"a \\\"zone\\\"\"",
        ),
        (
            Statement::Cease {
                object: hidden_obj(23),
            },
            Some(Cause::Rule("resolve".to_string())),
            "#23 cease by rule:resolve",
        ),
    ];
    for (statement, cause, expected) in cases {
        assert_eq!(statement_line(statement, cause), expected);
    }
}

#[test]
fn writes_source_ids_after_the_card() {
    let new_card = Statement::NewCard {
        object: obj(3, "Grizzly Bears"),
        owner: A.to_string(),
        zone: Zone::Battlefield,
        position: None,
        entering: Some(Entering {
            types: vec![CardType::Creature],
            pt: Some((2, 2)),
            ..Entering::default()
        }),
        sources: vec!["arena:75000".to_string()],
    };
    assert_eq!(
        statement_line(new_card, None),
        "new card #3 \"Grizzly Bears\" owner \"PlayerA\" in battlefield types creature pt 2/2 src arena:75000"
    );
    let identify = Statement::Identify {
        id: 8,
        name: Some("Thoughtseize".to_string()),
        sources: vec!["arena:1".to_string(), "mtgo:2".to_string()],
    };
    assert_eq!(
        statement_line(identify, None),
        "#8 identify \"Thoughtseize\" src arena:1 src mtgo:2"
    );
}

/// Decision 0048: a card whose name the writer cannot determine is `?`,
/// with its source id, on `new card`, `new token`, `new copy`, `identify`
/// and deck entries; references to it are by id only.
#[test]
fn writes_unknown_card_names() {
    let cases = vec![
        (
            Statement::NewCard {
                object: hidden_obj(24),
                owner: A.to_string(),
                zone: Zone::Library,
                position: None,
                entering: None,
                sources: vec!["arena:106504".to_string()],
            },
            None,
            "new card #24 ? owner \"PlayerA\" in library src arena:106504",
        ),
        (
            Statement::NewToken {
                object: hidden_obj(17),
                owner: B.to_string(),
                zone: Zone::Battlefield,
                entering: Entering {
                    types: vec![CardType::Creature],
                    pt: Some((1, 1)),
                    ..Entering::default()
                },
                sources: vec!["arena:106778".to_string()],
            },
            Some(Cause::Object(16)),
            "new token #17 ? owner \"PlayerB\" in battlefield types creature pt 1/1 src arena:106778 by #16",
        ),
        (
            Statement::NewCopy {
                id: 40,
                of: hidden_obj(24),
                name: None,
                owner: A.to_string(),
                zone: Zone::Stack,
                entering: None,
            },
            Some(Cause::Object(39)),
            "new copy #40 of #24 ? owner \"PlayerA\" in stack by #39",
        ),
        (
            Statement::Identify {
                id: 30,
                name: None,
                sources: vec!["arena:106282".to_string()],
            },
            None,
            "#30 identify ? src arena:106282",
        ),
        (
            Statement::DeckEntry {
                side: false,
                count: 4,
                card: DeckCard::Unknown("arena:106282".to_string()),
            },
            None,
            "main 4 ? src arena:106282",
        ),
        (
            Statement::DeckEntry {
                side: true,
                count: 2,
                card: DeckCard::Named("Abrade".to_string()),
            },
            None,
            "side 2 \"Abrade\"",
        ),
        (
            Statement::Cast {
                object: hidden_obj(24),
                from: Zone::Hand,
                face: None,
            },
            None,
            "#24 cast from hand",
        ),
    ];
    for (statement, cause, expected) in cases {
        assert_eq!(statement_line(statement, cause), expected);
    }
}

#[test]
fn keyframe_objects_list_attached_then_counters() {
    let entry = Statement::KeyframeObject {
        object: obj(7, "The Wandering Emperor"),
        zone: Zone::Battlefield,
        player: Some(A.to_string()),
        face: None,
        types: vec![CardType::Planeswalker],
        pt: None,
        tapped: true,
        attached: Some(3),
        with: None,
        adventure: None,
        companion: false,
        counters: vec![("loyalty".to_string(), 2)],
        position: None,
    };
    assert_eq!(
        statement_line(entry, None),
        "#7 \"The Wandering Emperor\" battlefield \"PlayerA\" types planeswalker tapped attached #3 counters \"loyalty\" 2"
    );
}

#[test]
fn keyframe_companions_are_flagged() {
    let entry = Statement::KeyframeObject {
        object: obj(1, "Kaheera, the Orphanguard"),
        zone: Zone::OutsideSideboard,
        player: Some(A.to_string()),
        face: None,
        types: vec![],
        pt: None,
        tapped: false,
        attached: None,
        with: None,
        adventure: None,
        companion: true,
        counters: vec![],
        position: None,
    };
    assert_eq!(
        statement_line(entry, None),
        "#1 \"Kaheera, the Orphanguard\" outside \"sideboard\" \"PlayerA\" companion"
    );
}

#[test]
fn writes_mana_priority_and_library_statements() {
    let bear = obj(5, "Grizzly Bears");
    let cases = vec![
        (
            Statement::Add {
                player: A.to_string(),
                mana: vec![Mana::C, Mana::G, Mana::W],
                unknown_restriction: true,
            },
            Some(Cause::Object(15)),
            "\"PlayerA\" add {W}{G}{C} restriction ? by #15",
        ),
        (
            Statement::Pay {
                player: A.to_string(),
                mana: vec![Mana::C, Mana::U, Mana::B],
            },
            Some(Cause::Cost(2)),
            "\"PlayerA\" pay {U}{B}{C} by cost:#2",
        ),
        (
            Statement::LoseMana {
                player: B.to_string(),
                mana: vec![Mana::G],
            },
            Some(Cause::Rule("empty-pool".to_string())),
            "\"PlayerB\" lose-mana {G} by rule:empty-pool",
        ),
        (
            Statement::Untracked {
                object: hidden_obj(9),
            },
            None,
            "#9 untracked",
        ),
        (
            Statement::StackEntry {
                object: obj(2, "Thoughtseize"),
                face: None,
                controller: A.to_string(),
                choices: vec![
                    Choice::X(2),
                    Choice::Target(TargetRef::Player(B.to_string())),
                    Choice::Target(TargetRef::Object(bear.clone())),
                    Choice::Object(obj(8, "Ragavan, Nimble Pilferer")),
                ],
            },
            None,
            "#2 \"Thoughtseize\" controller \"PlayerA\" x 2 target \"PlayerB\" target #5 \"Grizzly Bears\" object #8 \"Ragavan, Nimble Pilferer\"",
        ),
        (
            Statement::StackAbility {
                id: 23,
                kind: AbilityKind::Triggered,
                of: bear.clone(),
                text: None,
                controller: B.to_string(),
                choices: vec![Choice::Target(TargetRef::Object(obj(7, "Plains")))],
            },
            None,
            "#23 triggered of #5 \"Grizzly Bears\" ? controller \"PlayerB\" target #7 \"Plains\"",
        ),
    ];
    for (statement, cause, expected) in cases {
        assert_eq!(statement_line(statement, cause), expected);
    }
}

/// Decision 0022: `face` on casts, entering permanents, stack entries and
/// keyframe entries; `transform`; day and night as events and in keyframes;
/// and an unknown power and toughness in a keyframe.
#[test]
fn writes_faces_transform_and_day_night() {
    let piper = obj(36, "Howlpack Piper // Wildsong Howler");
    let cases = vec![
        (
            Statement::Cast {
                object: obj(80, "Fire // Ice"),
                from: Zone::Hand,
                face: Some("Ice".to_string()),
            },
            None,
            "#80 \"Fire // Ice\" cast from hand face \"Ice\"",
        ),
        (
            Statement::Move {
                object: piper.clone(),
                from: Zone::Stack,
                to: Zone::Battlefield,
                action: None,
                entering: Some(Entering {
                    face: Some("Wildsong Howler".to_string()),
                    tapped: true,
                    types: vec![CardType::Creature],
                    pt: Some((5, 5)),
                }),
                position: None,
                with: None,
            },
            Some(Cause::Rule("resolve".to_string())),
            "#36 \"Howlpack Piper // Wildsong Howler\" move stack -> battlefield face \"Wildsong Howler\" tapped types creature pt 5/5 by rule:resolve",
        ),
        (
            Statement::Transform {
                object: piper.clone(),
            },
            Some(Cause::Object(36)),
            "#36 \"Howlpack Piper // Wildsong Howler\" transform by #36",
        ),
        (
            Statement::DayNight { night: true },
            Some(Cause::Rule("day-night".to_string())),
            "night by rule:day-night",
        ),
        (Statement::DayNight { night: false }, None, "day"),
        (
            Statement::StackEntry {
                object: obj(80, "Fire // Ice"),
                face: Some("Fire".to_string()),
                controller: A.to_string(),
                choices: vec![Choice::Target(TargetRef::Player(B.to_string()))],
            },
            None,
            "#80 \"Fire // Ice\" face \"Fire\" controller \"PlayerA\" target \"PlayerB\"",
        ),
        (
            Statement::KeyframeObject {
                object: piper,
                zone: Zone::Battlefield,
                player: Some(B.to_string()),
                face: Some("Wildsong Howler".to_string()),
                types: vec![CardType::Creature],
                pt: Some(PtValue::Unknown),
                tapped: false,
                attached: None,
                with: None,
                adventure: None,
                companion: false,
                counters: vec![],
                position: None,
            },
            None,
            "#36 \"Howlpack Piper // Wildsong Howler\" battlefield \"PlayerB\" face \"Wildsong Howler\" types creature pt ?",
        ),
    ];
    for (statement, cause, expected) in cases {
        assert_eq!(statement_line(statement, cause), expected);
    }
}

/// Decision 0023: a copy of a spell (CR 707.10) is `new copy`, with the
/// original and the copy's own name.
#[test]
fn writes_spell_copies() {
    let statement = Statement::NewCopy {
        id: 65,
        of: obj(63, "Consign to Memory"),
        name: Some("Consign to Memory".to_string()),
        owner: A.to_string(),
        zone: Zone::Stack,
        entering: None,
    };
    assert_eq!(
        statement_line(statement, Some(Cause::Object(64))),
        "new copy #65 of #63 \"Consign to Memory\" \"Consign to Memory\" owner \"PlayerA\" in stack by #64"
    );
}

/// Decisions 0043 to 0047: library positions on `new` lines, moves (`top`,
/// `top N`, `bottom`, `bottom ?`) and keyframe entries; `look`; a lasting effect with unknown text and its
/// keyframe entry.
#[test]
fn writes_library_positions_looks_and_effects() {
    let bauble = obj(6, "Mishra's Bauble");
    let cases = vec![
        (
            Statement::NewHidden {
                id: 38,
                owner: B.to_string(),
                zone: Zone::Library,
                position: Some(Position {
                    end: LibraryEnd::Top,
                    n: 2,
                }),
            },
            None,
            "new hidden #38 owner \"PlayerB\" in library top 2",
        ),
        (
            Statement::NewCard {
                object: obj(17, "Mountain"),
                owner: B.to_string(),
                zone: Zone::Library,
                position: Some(Position {
                    end: LibraryEnd::Top,
                    n: 1,
                }),
                entering: None,
                sources: vec![],
            },
            None,
            "new card #17 \"Mountain\" owner \"PlayerB\" in library top 1",
        ),
        (
            Statement::Look {
                object: obj(17, "Mountain"),
                player: A.to_string(),
            },
            Some(Cause::Object(16)),
            "#17 \"Mountain\" look \"PlayerA\" by #16",
        ),
        (
            Statement::Move {
                object: hidden_obj(39),
                from: Zone::Library,
                to: Zone::Library,
                action: None,
                position: Some(LibraryPlace::Bottom),
                entering: None,
                with: None,
            },
            Some(Cause::Object(18)),
            "#39 move library -> library bottom by #18",
        ),
        (
            Statement::Move {
                object: obj(4, "Island"),
                from: Zone::Hand,
                to: Zone::Library,
                action: None,
                position: Some(LibraryPlace::Top),
                entering: None,
                with: None,
            },
            Some(Cause::Rule("mulligan".to_string())),
            "#4 \"Island\" move hand -> library top by rule:mulligan",
        ),
        (
            Statement::Move {
                object: obj(30, "Llanowar Elves"),
                from: Zone::Battlefield,
                to: Zone::Library,
                action: None,
                position: Some(LibraryPlace::FromTop(3)),
                entering: None,
                with: None,
            },
            Some(Cause::Object(29)),
            "#30 \"Llanowar Elves\" move battlefield -> library top 3 by #29",
        ),
        (
            Statement::Move {
                object: obj(52, "Forest"),
                from: Zone::Exile,
                to: Zone::Library,
                action: None,
                position: Some(LibraryPlace::RandomBottom),
                entering: None,
                with: None,
            },
            Some(Cause::Object(51)),
            "#52 \"Forest\" move exile -> library bottom ? by #51",
        ),
        (
            Statement::NewEffect {
                id: 18,
                of: bauble.clone(),
                text: None,
                controller: A.to_string(),
            },
            Some(Cause::Object(16)),
            "new effect #18 of #6 \"Mishra's Bauble\" ? controller \"PlayerA\" by #16",
        ),
        (
            Statement::KeyframeEffect {
                id: 18,
                of: bauble,
                text: None,
                controller: A.to_string(),
            },
            None,
            "#18 effect of #6 \"Mishra's Bauble\" ? controller \"PlayerA\"",
        ),
        (
            Statement::KeyframeObject {
                object: obj(17, "Mountain"),
                zone: Zone::Library,
                player: Some(B.to_string()),
                face: None,
                types: Vec::new(),
                pt: None,
                tapped: false,
                attached: None,
                with: None,
                adventure: None,
                companion: false,
                counters: Vec::new(),
                position: Some(Position {
                    end: LibraryEnd::Bottom,
                    n: 3,
                }),
            },
            None,
            "#17 \"Mountain\" library \"PlayerB\" bottom 3",
        ),
    ];
    for (statement, cause, expected) in cases {
        assert_eq!(statement_line(statement, cause), expected);
    }
}

/// Spec 7.1 and 10.3: `with ID` ends the move line (after the entering
/// values) and follows `attached` in a keyframe entry.
#[test]
fn exile_links_are_written_on_moves_and_keyframes() {
    let mut linked = mv(obj(9, "Fatal Push"), Zone::Graveyard, Zone::Exile, None);
    if let Statement::Move { with, .. } = &mut linked {
        *with = Some(6);
    }
    assert_eq!(
        statement_line(linked, Some(Cause::Cost(6))),
        "#9 \"Fatal Push\" move graveyard -> exile with #6 by cost:#6"
    );
    let mut entry = zone_entry(
        obj(5, "Emrakul, the Exigent Doom"),
        Zone::Exile,
        "PlayerA",
        vec![],
        false,
    );
    if let Statement::KeyframeObject { with, player, .. } = &mut entry {
        *with = Some(7);
        *player = None;
    }
    assert_eq!(
        statement_line(entry, None),
        "#5 \"Emrakul, the Exigent Doom\" exile with #7"
    );
}

/// Spec 10.3: `adventure PLAYER` follows `with` on a card in exile on an
/// adventure (CR 715.3d).
#[test]
fn keyframe_adventure_names_who_may_cast_it() {
    let mut entry = zone_entry(
        obj(4, "Bonecrusher Giant // Stomp"),
        Zone::Exile,
        "PlayerA",
        vec![],
        false,
    );
    if let Statement::KeyframeObject {
        adventure, player, ..
    } = &mut entry
    {
        *adventure = Some(B.to_string());
        *player = None;
    }
    assert_eq!(
        statement_line(entry, None),
        "#4 \"Bonecrusher Giant // Stomp\" exile adventure \"PlayerB\""
    );
}

/// Spec 7.3: `control PLAYER`, with a required cause.
#[test]
fn control_names_the_new_controller() {
    let line = Statement::Control {
        object: obj(5, "Ragavan, Nimble Pilferer"),
        player: B.to_string(),
    };
    assert_eq!(
        statement_line(line, Some(Cause::Object(9))),
        "#5 \"Ragavan, Nimble Pilferer\" control \"PlayerB\" by #9"
    );
}

/// Spec 7.2: `choose x` on the spell, with no cause when its controller
/// chose.
#[test]
fn choose_x_is_written_on_the_spell() {
    let line = Statement::ChooseX {
        object: obj(3, "March of Otherworldly Light"),
        value: 2,
    };
    assert_eq!(
        statement_line(line, None),
        "#3 \"March of Otherworldly Light\" choose x 2"
    );
}

#[test]
fn writes_poison_counters() {
    let statement = Statement::Poison {
        player: A.to_string(),
        from: 2,
        to: 5,
    };
    assert_eq!(
        statement_line(statement, Some(Cause::Unknown)),
        "\"PlayerA\" poison 2 -> 5 by ?"
    );
}
