//! Decoding the committed ranked Modern Bo3 capture.

use endstep_protocol::{
    DeckBody, FrameKind, GameAction, GameDelta, GameEvent, GameState, LobbyBody, MAX_PROMPT_CARDS,
    MatchBody, RankedResult, StateCard, Unit, Zone, decode_unit, unit_refs,
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
const DB_ID: &str = "00000000-0000-4000-8000-e0000000003d";
const PLAYER_ID: &str = "00000000-0000-4000-8000-00000000000a";
const DECK_ID: &str = "00000000-0000-4000-8000-e00000000031";
const LOBBY_ID: &str = "00000000-0000-4000-8000-e00000000038";
const MATCH_ID: &str = "00000000-0000-4000-8000-e0000000003a";

fn unit(text: &str) -> Unit {
    decode_unit(text).expect("unit decodes")
}

fn match_bodies(unit: &Unit) -> Vec<MatchBody> {
    let mut out = Vec::new();
    for frame in &unit.frames {
        if let FrameKind::MatchBody(body) = &frame.kind {
            out.push(body.clone());
        }
    }
    out
}

fn states(unit: &Unit) -> Vec<GameState> {
    let mut out = Vec::new();
    for frame in &unit.frames {
        if let FrameKind::GameState(state) = &frame.kind {
            out.push(state.clone());
        }
    }
    out
}

fn events(unit: &Unit) -> Vec<GameEvent> {
    let mut out = Vec::new();
    for frame in &unit.frames {
        if let FrameKind::GameEvent(event) = &frame.kind {
            out.push(event.clone());
        }
    }
    out
}

#[test]
fn the_header_fields_are_kept() {
    let unit = unit(UNIT_01);
    assert_eq!(unit.unit_id, "a7a2056a-63ea-47e9-a198-028e1ebf8532");
    assert_eq!(unit.started_at, 1_794_310_841_419);
    assert_eq!(unit.counter, 1);
    assert!(!unit.frames.is_empty());
}

#[test]
fn the_session_unit_has_the_match_body() {
    let unit = unit(UNIT_01);
    let bodies = match_bodies(&unit);
    assert!(!bodies.is_empty());
    let body = &bodies[0];
    assert_eq!(body.id, "00000000-0000-4000-8000-e0000000003a");
    assert_eq!(body.forge_game_id, FORGE_GAME);
    assert_eq!(body.db_id.as_deref(), Some(DB_ID));
    assert_eq!(
        body.lobby_id.as_deref(),
        Some("00000000-0000-4000-8000-e00000000038")
    );
    assert_eq!(body.format_id.as_deref(), Some("Modern"));
    assert_eq!(body.my_player_index, Some(0));
    assert_eq!(body.my_deck.len(), 60);
    assert_eq!(body.my_sideboard.len(), 15);
    assert!(body.ranked);
    assert_eq!(body.games_per_match, Some(3));
    // 2026-11-10T11:41:49.543Z
    assert_eq!(body.started_at_ms, Some(1_794_310_909_543));
    assert!(body.my_deck_art.contains(&(
        "sowing mycospawn".to_string(),
        "mh3".to_string(),
        "340".to_string()
    )));
    assert_eq!(body.participants.len(), 2);
    assert_eq!(body.participants[0].user_id, PLAYER_ID);
    assert_eq!(body.participants[0].username, "Player");
    assert!(!body.participants[0].bot);
    assert_eq!(body.participants[1].username, "Opponent");
}

#[test]
fn the_lobby_and_deck_bodies_decode() {
    let unit = unit(UNIT_01);
    let mut lobby: Option<LobbyBody> = None;
    let mut deck: Option<DeckBody> = None;
    for frame in &unit.frames {
        match &frame.kind {
            FrameKind::LobbyBody(body) => lobby = Some(body.clone()),
            FrameKind::DeckBody(body) => deck = Some(body.clone()),
            _ => {}
        }
    }
    let lobby = lobby.expect("a lobby body");
    assert_eq!(lobby.id, "00000000-0000-4000-8000-e00000000038");
    assert!(
        lobby
            .seat_decks
            .contains(&(PLAYER_ID.to_string(), DECK_ID.to_string()))
    );

    let deck = deck.expect("a deck body");
    assert_eq!(deck.id, DECK_ID);
    assert_eq!(deck.name, "Tron");
    assert_eq!(deck.format_id.as_deref(), Some("Modern"));
    assert_eq!(deck.main.len(), 19);
    assert_eq!(deck.side.len(), 15);
    let mycospawn = deck
        .main
        .iter()
        .find(|line| line.name == "Sowing Mycospawn")
        .expect("Sowing Mycospawn");
    assert_eq!(mycospawn.set_code.as_deref(), Some("mh3"));
    assert_eq!(mycospawn.collector_number.as_deref(), Some("340"));
    assert_eq!(mycospawn.quantity, 4);
    let total: u32 = deck.main.iter().map(|line| line.quantity).sum();
    assert_eq!(total, 60);
}

#[test]
fn the_second_match_unit_has_three_outcomes() {
    let unit = unit(UNIT_04);
    let outcomes: Vec<GameEvent> = events(&unit)
        .into_iter()
        .filter(|event| event.kind == "GAME_OUTCOME")
        .collect();
    assert_eq!(outcomes.len(), 3);
    let winners: Vec<Option<u32>> = outcomes.iter().map(|e| e.player_index).collect();
    assert_eq!(winners, vec![Some(0), Some(1), Some(1)]);
    for outcome in &outcomes {
        assert_eq!(outcome.end_reason.as_deref(), Some("concede"));
        assert_eq!(outcome.game_id, FORGE_GAME);
    }
    assert_eq!(outcomes[0].turn_number, Some(12));

    let mut result: Option<RankedResult> = None;
    for frame in &unit.frames {
        if let FrameKind::RankedResult(body) = &frame.kind {
            result = Some(body.clone());
        }
    }
    let result = result.expect("a ranked result");
    assert_eq!(result.db_id, DB_ID);
    assert_eq!(result.status.as_deref(), Some("rated"));
    assert_eq!(result.format_id.as_deref(), Some("Modern"));
    assert_eq!(result.rating_before, Some(1371));
    assert_eq!(result.rating_after, Some(1336));
    assert_eq!(result.score_before, Some(1089));
    assert_eq!(result.score_after, Some(1071));
    assert_eq!(result.delta, Some(-18));
    assert_eq!(result.rd_before, Some(141));
    assert_eq!(result.rd_after, Some(132));
    assert_eq!(result.rank, None);
}

#[test]
fn game_numbers_come_from_states() {
    let unit = unit(UNIT_04);
    let mut numbers: Vec<u32> = states(&unit)
        .iter()
        .filter_map(|state| state.game_number)
        .collect();
    numbers.sort();
    numbers.dedup();
    assert_eq!(numbers, vec![1, 2, 3]);

    let mut over: Option<GameState> = None;
    for frame in &unit.frames {
        if let FrameKind::GameOver(state) = &frame.kind {
            over = Some(state.clone());
        }
    }
    let over = over.expect("a game over");
    assert!(over.is_match_over);
    assert_eq!(over.wins_by_seat, vec![1, 2]);
    assert_eq!(over.winner, Some(1));
    assert_eq!(over.active_player, Some(0));
    assert_eq!(over.turn_number, Some(7));
    assert_eq!(over.game_number, Some(3));
    assert_eq!(over.game_id, FORGE_GAME);
}

#[test]
fn the_first_state_reads_the_envelope_and_players() {
    let unit = unit(UNIT_02);
    let first = states(&unit).into_iter().next().expect("a state");
    assert_eq!(first.seq, Some(1_794_310_912_448_018));
    assert_eq!(
        first.match_id.as_deref(),
        Some("00000000-0000-4000-8000-e0000000003a")
    );
    assert_eq!(first.game_id, FORGE_GAME);
    assert_eq!(first.game_number, Some(1));
    assert_eq!(first.games_played, Some(0));
    assert_eq!(first.turn_number, Some(0));
    assert_eq!(first.active_player, None);
    assert_eq!(first.winner, None);
    assert_eq!(
        first.players,
        vec![(0, "Player".to_string()), (1, "Opponent".to_string())]
    );

    let mulligan = events(&unit)
        .into_iter()
        .find(|event| event.kind == "MULLIGAN")
        .expect("a mulligan");
    assert_eq!(mulligan.player_index, Some(1));
    assert_eq!(mulligan.sequence_number, Some(19));
    assert_eq!(mulligan.seq, Some(1_794_310_912_448_020));

    let mut deltas = 0;
    for frame in &unit.frames {
        if let FrameKind::GameDelta(delta) = &frame.kind {
            assert!(delta.seq.is_some());
            deltas += 1;
        }
    }
    assert_eq!(deltas, 172);
}

#[test]
fn refs_link_session_and_match_units() {
    let game_ref = format!("game:{FORGE_GAME}");
    let db_ref = format!("db:{DB_ID}");
    for text in [UNIT_01, UNIT_02, UNIT_03, UNIT_04] {
        let refs = unit_refs(&unit(text));
        assert!(refs.contains(&game_ref), "{refs:?}");
    }
    assert_eq!(
        unit_refs(&unit(UNIT_01)),
        vec![
            db_ref.clone(),
            format!("deck:{DECK_ID}"),
            format!("deckbody:{DECK_ID}"),
            game_ref,
            format!("lobby:{LOBBY_ID}"),
            format!("match:{MATCH_ID}"),
        ]
    );
    assert!(unit_refs(&unit(UNIT_04)).contains(&db_ref));
    assert!(!unit_refs(&unit(UNIT_02)).contains(&db_ref));
    assert!(unit_refs(&unit(UNIT_05)).is_empty());
}

#[test]
fn the_gateway_owner_body_is_not_a_match_body() {
    let unit = unit(UNIT_03);
    // Unit 03 has five bodies at the match URL; four are `{"owner":"gw-4"}`.
    assert_eq!(match_bodies(&unit).len(), 1);
}

const HEADER: &str = r#"{"mymtgo_endstep_capture":1,"unit_id":"0f8fad5b-d9cb-469f-a165-70867728950e","kind":"match","started_at":1700000000000,"counter":3,"extension":"0.1.0","browser":"firefox","developer":false,"classification":"unknown"}"#;

#[test]
fn a_bad_line_is_other() {
    let text = format!(
        "{HEADER}\n{}\n{}\nnot json\n{}\n",
        r#"{"at":5,"src":"rest","dir":"in","url":"/api/ranked/results/abc","body":{"status":"rated","you":{"delta":3,"rank":"gold"}}}"#,
        r#"{"at":1,"src":"rest","dir":"in","url":"/api/matches/x","body":[1]}"#,
        r#"{"at":7,"src":"game_ws","dir":"in","url":"/ws","body":{"type":"GAME_STATE","payload":{"turnNumber":"3"}}}"#,
    );
    let unit = unit(&text);
    assert_eq!(unit.counter, 3);
    assert_eq!(unit.frames.len(), 4);
    match &unit.frames[0].kind {
        FrameKind::RankedResult(result) => {
            assert_eq!(result.db_id, "abc");
            assert_eq!(result.delta, Some(3));
            assert_eq!(result.rank.as_deref(), Some("gold"));
        }
        other => panic!("expected a ranked result, got {other:?}"),
    }
    assert_eq!(unit.frames[1].at, 1);
    assert_eq!(unit.frames[1].kind, FrameKind::Other);
    assert_eq!(unit.frames[2].kind, FrameKind::Other);
    // A state without a game id is not a state.
    assert_eq!(unit.frames[3].kind, FrameKind::Other);
}

#[test]
fn a_bad_header_is_an_error() {
    assert!(decode_unit("").is_err());
    assert!(decode_unit("not json\n").is_err());
    let wrong_format = HEADER.replace(
        "\"mymtgo_endstep_capture\":1",
        "\"mymtgo_endstep_capture\":9",
    );
    assert!(decode_unit(&wrong_format).is_err());
}

#[test]
fn a_bot_participant_is_flagged() {
    let text = format!(
        "{HEADER}\n{}\n",
        r#"{"at":1,"src":"rest","dir":"in","url":"/api/matches/m1","body":{"id":"m1","forgeGameId":"g1","participants":[{"userId":"u1","username":"Player"},{"userId":"u2","username":"Forge AI","isAi":true},{"userId":"u3","username":"Bot","isBot":true}]}}"#,
    );
    let bodies = match_bodies(&unit(&text));
    let bots: Vec<bool> = bodies[0].participants.iter().map(|p| p.bot).collect();
    assert_eq!(bots, vec![false, true, true]);
    assert_eq!(bodies[0].db_id, None);
    assert_eq!(bodies[0].started_at_ms, None);
    assert!(!bodies[0].ranked);
    assert_eq!(
        unit_refs(&unit(&text)),
        vec!["game:g1".to_string(), "match:m1".to_string()]
    );
}

#[test]
fn lobby_and_deck_bodies_are_refs() {
    let refs = unit_refs(&unit(UNIT_01));
    assert!(refs.contains(&format!("lobby:{LOBBY_ID}")), "{refs:?}");
    // The lobby's seat deck and the deck body each name the deck by role.
    assert!(refs.contains(&format!("deck:{DECK_ID}")), "{refs:?}");
    assert!(refs.contains(&format!("deckbody:{DECK_ID}")), "{refs:?}");
    // The opponent's hidden seat deck is never a ref.
    assert!(
        !refs.iter().any(|r| r.starts_with("deck:hidden")),
        "{refs:?}"
    );

    // A session unit closed before the match body still links by lobby and deck.
    let text = format!(
        "{HEADER}\n{}\n{}\n",
        r#"{"at":1,"src":"rest","dir":"in","url":"/api/lobbies/l1","body":{"id":"l1","config":{"seatDecks":{"u1":"d1","hidden-u2":"hidden-deck"}}}}"#,
        r#"{"at":2,"src":"rest","dir":"in","url":"/api/decks/d2","body":{"id":"d2","name":"Tron","cards":[]}}"#,
    );
    assert_eq!(
        unit_refs(&unit(&text)),
        vec![
            "deck:d1".to_string(),
            "deckbody:d2".to_string(),
            "lobby:l1".to_string()
        ]
    );
}

#[test]
fn a_ranked_error_body_is_other() {
    let text = format!(
        "{HEADER}\n{}\n{}\n",
        r#"{"at":1,"src":"rest","dir":"in","url":"/api/ranked/results/abc","body":{"error":"not found"}}"#,
        r#"{"at":2,"src":"rest","dir":"in","url":"/api/ranked/results/abc","body":{"status":"rated","you":null}}"#,
    );
    let unit = unit(&text);
    assert_eq!(unit.frames[0].kind, FrameKind::Other);
    assert_eq!(unit.frames[1].kind, FrameKind::Other);
    assert!(unit_refs(&unit).is_empty());
}

#[test]
fn a_float_rating_is_rounded() {
    let text = format!(
        "{HEADER}\n{}\n",
        r#"{"at":1,"src":"rest","dir":"in","url":"/api/ranked/results/abc","body":{"status":"rated","you":{"rdBefore":141.5,"rdAfter":132.4,"delta":-17.6}}}"#,
    );
    match &unit(&text).frames[0].kind {
        FrameKind::RankedResult(result) => {
            assert_eq!(result.rd_before, Some(142));
            assert_eq!(result.rd_after, Some(132));
            assert_eq!(result.delta, Some(-18));
        }
        other => panic!("expected a ranked result, got {other:?}"),
    }
}

#[test]
fn states_carry_both_seats_cards() {
    let unit = unit(UNIT_02);
    let state = states(&unit)
        .into_iter()
        .find(|state| {
            state
                .cards
                .iter()
                .any(|card| card.zone == Zone::Battlefield)
        })
        .expect("a state with a battlefield");

    let mine: Vec<&StateCard> = state
        .cards
        .iter()
        .filter(|card| card.owner_seat == Some(0))
        .collect();
    let map = mine
        .iter()
        .find(|card| card.name == "Expedition Map")
        .expect("seat 0 holds Expedition Map");
    assert_eq!(map.id, 38);
    assert_eq!(map.zone, Zone::Hand);
    assert_eq!(map.set_code, "2xm");
    assert_eq!(map.collector_number, "359");
    assert!(!map.is_token);
    assert!(!map.face_down);
    assert!(map.has_types);
    // A card without a chosen printing keeps empty strings.
    let boulder = mine
        .iter()
        .find(|card| card.name == "Giant's Boulder")
        .expect("seat 0 holds Giant's Boulder");
    assert_eq!(boulder.set_code, "");
    assert_eq!(boulder.collector_number, "");

    // The opponent's permanent is there, with no printing.
    let maze = state
        .cards
        .iter()
        .find(|card| card.owner_seat == Some(1) && card.zone == Zone::Battlefield)
        .expect("seat 1 has a permanent");
    assert_eq!(maze.name, "Hedge Maze");
    assert_eq!(maze.id, 78);
    assert_eq!(maze.set_code, "");
    assert_eq!(maze.collector_number, "");

    // Its enter trigger on the stack is an ability: the source card is
    // listed once, on the battlefield, never again as a stack card.
    let mazes: Vec<&StateCard> = state.cards.iter().filter(|card| card.id == 78).collect();
    assert_eq!(mazes.len(), 1, "{:?}", state.cards);
    assert_eq!(mazes[0].zone, Zone::Battlefield);
}

#[test]
fn a_spell_on_the_stack_is_a_stack_card() {
    let unit = unit(UNIT_02);
    let on_stack = states(&unit).into_iter().any(|state| {
        state
            .cards
            .iter()
            .any(|card| card.zone == Zone::Stack && card.name == "Expedition Map")
    });
    assert!(
        on_stack,
        "Expedition Map is cast, so a state has it on the stack"
    );
}

#[test]
fn command_and_library_cards_decode() {
    let unit = unit(UNIT_04);
    let mut command = None;
    let mut library = None;
    for state in states(&unit) {
        for card in state.cards {
            match card.zone {
                Zone::Command if command.is_none() => command = Some(card),
                Zone::Library if library.is_none() => library = Some(card),
                _ => {}
            }
        }
    }
    let command = command.expect("a commandZone card");
    assert_eq!(command.name, "Devourer of Destiny's Effect");
    assert_eq!(command.owner_seat, Some(0));
    assert!(!command.has_types, "a Forge effect object has no types");
    let library = library.expect("a libraryTop card");
    assert_eq!(library.name, "Urza's Power Plant");
    assert_eq!(library.id, 23);
    assert_eq!(library.owner_seat, Some(0));
    assert!(library.has_types);
}

#[test]
fn hidden_cards_are_dropped() {
    let unit = unit(UNIT_02);
    let mut total = 0;
    for state in states(&unit) {
        for card in &state.cards {
            total += 1;
            assert_ne!(card.name, "Hidden card");
        }
    }
    assert!(total > 0);
}

#[test]
fn a_game_start_says_who_chose_and_who_goes_first() {
    let unit = unit(UNIT_04);
    let starts: Vec<(Option<u32>, Option<u32>)> = events(&unit)
        .into_iter()
        .filter(|event| event.kind == "GAME_STARTED")
        .map(|event| (event.toss_player, event.player_index))
        .collect();
    // Games 2 and 3: the loser of the game before chose to go first.
    assert_eq!(starts, vec![(Some(1), Some(1)), (Some(0), Some(0))]);
    let other = events(&unit)
        .into_iter()
        .find(|event| event.kind == "TURN_BEGAN")
        .expect("a turn");
    assert_eq!(other.toss_player, None);
}

#[test]
fn events_carry_text_and_cards() {
    let unit = unit(UNIT_02);
    let cast = events(&unit)
        .into_iter()
        .find(|event| {
            event.kind == "SPELL_CAST" && event.card_name.as_deref() == Some("Expedition Map")
        })
        .expect("Expedition Map is cast");
    assert_eq!(cast.message.as_deref(), Some("Player cast Expedition Map"));
    assert_eq!(cast.card_id, Some(38));

    // An event without a card keeps its text only.
    let shuffle = events(&unit)
        .into_iter()
        .find(|event| event.kind == "SHUFFLE")
        .expect("a shuffle");
    assert!(shuffle.message.is_some());
    assert_eq!(shuffle.card_id, None);
    assert_eq!(shuffle.card_name, None);
}

#[test]
fn a_discard_log_names_the_card_and_its_player() {
    let all = events(&unit(UNIT_04));
    let mut discards = 0;
    for (index, event) in all.iter().enumerate() {
        if event.kind != "DISCARD_LOG" {
            continue;
        }
        discards += 1;
        assert!(event.card_id.is_some());
        assert!(event.card_name.is_some());
        assert_eq!(event.player_index, Some(1), "the opponent cycles");
        // Forge sends the named hand to graveyard move just before it.
        let change = &all[index - 1];
        assert_eq!(change.kind, "CARD_ZONE_CHANGE");
        assert_eq!(
            (change.from_zone.as_deref(), change.to_zone.as_deref()),
            (Some("Hand"), Some("Graveyard"))
        );
        assert_eq!(change.card_id, event.card_id);
        assert_eq!(change.zone_owner, event.player_index);
    }
    assert_eq!(discards, 4);
    let first = all
        .iter()
        .find(|event| event.kind == "DISCARD_LOG")
        .expect("a discard");
    assert_eq!(first.card_name.as_deref(), Some("Generous Ent"));
    assert_eq!(first.card_id, Some(117));
}

#[test]
fn hostile_card_fields_decode_to_none() {
    let text = format!(
        "{HEADER}\n{}\n",
        r#"{"at":1,"src":"game_ws","dir":"in","url":"/ws","body":{"type":"GAME_STATE","seq":1,"payload":{"gameId":"g1","players":[{"id":"0","name":"Player","hand":[{"id":"x","name":"Bad Id"},{"id":7,"name":"Island","ownerId":-1,"selectedSetCode":5},{"id":8},"junk"],"battlefield":"junk","graveyard":[{"id":9,"name":"Shock","ownerId":"0","isToken":"yes","faceDown":1}]}],"stack":[{"sourceCard":null},{"sourceCard":{"id":10,"name":"Bolt","ownerId":"1"}},{"isAbility":true,"sourceCard":{"id":11,"name":"Trigger Source","ownerId":"1"}},"junk"]}}}"#,
    );
    let unit = unit(&text);
    let state = match &unit.frames[0].kind {
        FrameKind::GameState(state) => state.clone(),
        other => panic!("expected a state, got {other:?}"),
    };
    assert_eq!(
        state.cards,
        vec![
            StateCard {
                id: 7,
                name: "Island".to_string(),
                zone: Zone::Hand,
                owner_seat: None,
                is_token: false,
                face_down: false,
                has_types: false,
                set_code: String::new(),
                collector_number: String::new(),
                controller_seat: None,
                tapped: false,
                power: None,
                toughness: None,
                types: Vec::new(),
                counters: Vec::new(),
                attached_to: None,
            },
            StateCard {
                id: 9,
                name: "Shock".to_string(),
                zone: Zone::Graveyard,
                owner_seat: Some(0),
                // "yes" is no flag; the number 1 is (trap 57).
                is_token: false,
                face_down: true,
                has_types: false,
                set_code: String::new(),
                collector_number: String::new(),
                controller_seat: None,
                tapped: false,
                power: None,
                toughness: None,
                types: Vec::new(),
                counters: Vec::new(),
                attached_to: None,
            },
            StateCard {
                id: 10,
                name: "Bolt".to_string(),
                zone: Zone::Stack,
                owner_seat: Some(1),
                is_token: false,
                face_down: false,
                has_types: false,
                set_code: String::new(),
                collector_number: String::new(),
                controller_seat: None,
                tapped: false,
                power: None,
                toughness: None,
                types: Vec::new(),
                counters: Vec::new(),
                attached_to: None,
            },
        ]
    );
}

fn actions(unit: &Unit) -> Vec<GameAction> {
    let mut out = Vec::new();
    for frame in &unit.frames {
        if let FrameKind::Action(action) = &frame.kind {
            out.push(action.clone());
        }
    }
    out
}

#[test]
fn states_carry_player_sizes() {
    let unit = unit(UNIT_02);
    let first = &states(&unit)[0];
    assert_eq!(first.player_states.len(), 2);
    let user = &first.player_states[0];
    assert_eq!(user.seat, 0);
    assert_eq!(user.hand_size, Some(7));
    assert_eq!(user.life, Some(20));
}

#[test]
fn the_sideboard_prompt_lists_the_75() {
    let unit = unit(UNIT_04);
    let found = states(&unit)
        .into_iter()
        .find(|state| state.sideboarding)
        .expect("a sideboarding state");
    let prompt = found.prompt.expect("a prompt");
    assert_eq!(prompt.kind, "CHOOSE_CARDS");
    assert_eq!(prompt.context, "sideboard");
    assert_eq!(prompt.main_count, Some(60));
    assert_eq!(prompt.options.len(), 75);
    assert!(
        prompt
            .options
            .iter()
            .any(|card| card.name == "Giant's Boulder")
    );
}

#[test]
fn the_sideboard_submit_is_an_action() {
    let unit = unit(UNIT_04);
    let submit = actions(&unit)
        .into_iter()
        .find(|action| action.kind == "SIDEBOARD_SUBMIT")
        .expect("a submit");
    assert_eq!(submit.ordered_cards.len(), 60);
    assert_eq!(submit.prompt_version, Some(268));
    assert_eq!(submit.match_id.as_deref(), Some(MATCH_ID));
}

#[test]
fn the_bottom_choice_is_an_action() {
    let unit = unit(UNIT_04);
    assert!(
        actions(&unit)
            .iter()
            .any(|action| action.kind == "CHOOSE_CARDS" && action.ordered_cards == vec![20])
    );
}

#[test]
fn reveals_list_their_cards() {
    let unit = unit(UNIT_02);
    let reveals: Vec<GameEvent> = events(&unit)
        .into_iter()
        .filter(|event| event.kind == "CARD_REVEALED")
        .collect();
    assert!(
        reveals
            .iter()
            .any(|e| e.card_ids.len() == 21 && e.card_names.len() == 21 && !e.owner_echo)
    );
    assert!(reveals.iter().any(|e| e.owner_echo));
}

#[test]
fn mulligan_events_carry_their_fields() {
    let unit = unit(UNIT_04);
    let all = events(&unit);
    assert!(
        all.iter().any(|e| e.kind == "HAND_SETTLED"
            && e.mulligans == Some(1)
            && e.player_index == Some(0))
    );
    assert!(
        all.iter()
            .any(|e| e.kind == "CARD_ZONE_CHANGE" && e.during_mulligan)
    );
}

#[test]
fn hostile_prompts_and_actions_decode_safely() {
    let ids: Vec<String> = (0..100_000).map(|n| n.to_string()).collect();
    let big_action = format!(
        r#"{{"at":1,"src":"game_ws","dir":"out","url":"/ws","body":{{"type":"GAME_ACTION","payload":{{"type":"CHOOSE_CARDS","orderedCards":[{}]}}}}}}"#,
        ids.join(",")
    );
    let text = format!(
        "{HEADER}\n{}\n{}\n{}\n{}\n{}\n{}\n",
        r#"{"at":1,"src":"game_ws","dir":"in","url":"/ws","body":{"type":"GAME_STATE","seq":1,"payload":{"gameId":"g1","players":[{"id":"0","name":"P","handSize":"x","life":"y"}],"pendingAction":{"type":"CHOOSE_CARDS","cardOptions":"junk","sideboardState":{"mainCount":-1}}}}}"#,
        r#"{"at":1,"src":"game_ws","dir":"in","url":"/ws","body":{"type":"GAME_STATE","seq":2,"payload":{"gameId":"g1","pendingAction":{"contextType":"sideboard"}}}}"#,
        r#"{"at":1,"src":"game_ws","dir":"in","url":"/ws","body":{"type":"GAME_EVENT","payload":{"gameId":"g1","type":"CARD_REVEALED","cardIds":[1,2,3],"cardNames":["A"]}}}"#,
        big_action,
        r#"{"at":1,"src":"game_ws","dir":"out","url":"/ws","body":{"type":"GAME_ACTION","payload":{"type":"PASS_PRIORITY"}}}"#,
        r#"{"at":1,"src":"lobby_ws","dir":"out","url":"/ws","body":"junk"}"#,
    );
    let unit = unit(&text);
    let first = match &unit.frames[0].kind {
        FrameKind::GameState(state) => state.clone(),
        other => panic!("expected a state, got {other:?}"),
    };
    assert_eq!(first.player_states[0].hand_size, None);
    assert_eq!(first.player_states[0].life, None);
    let prompt = first.prompt.expect("a typed prompt");
    assert!(prompt.options.is_empty());
    assert_eq!(prompt.main_count, None);
    match &unit.frames[1].kind {
        FrameKind::GameState(state) => assert_eq!(state.prompt, None),
        other => panic!("expected a state, got {other:?}"),
    }
    match &unit.frames[2].kind {
        FrameKind::GameEvent(event) => {
            assert!(event.card_ids.is_empty());
            assert!(event.card_names.is_empty());
        }
        other => panic!("expected an event, got {other:?}"),
    }
    match &unit.frames[3].kind {
        FrameKind::Action(action) => assert_eq!(action.ordered_cards.len(), MAX_PROMPT_CARDS),
        other => panic!("expected an action, got {other:?}"),
    }
    assert_eq!(unit.frames[4].kind, FrameKind::Other);
    assert_eq!(unit.frames[5].kind, FrameKind::Other);
}

fn deltas(unit: &Unit) -> Vec<GameDelta> {
    let mut out = Vec::new();
    for frame in &unit.frames {
        if let FrameKind::GameDelta(delta) = &frame.kind {
            out.push(delta.clone());
        }
    }
    out
}

fn hand_built(lines: &[&str]) -> Unit {
    let mut text = format!("{HEADER}\n");
    for line in lines {
        text.push_str(line);
        text.push('\n');
    }
    unit(&text)
}

#[test]
fn battlefield_cards_carry_their_state() {
    let unit = unit(UNIT_02);
    let mut tapped_land = false;
    let mut creature = false;
    for state in states(&unit) {
        for card in state.cards.iter().filter(|c| c.zone == Zone::Battlefield) {
            if card.tapped
                && card.types == vec!["Land".to_string()]
                && card.controller_seat.is_some()
            {
                tapped_land = true;
            }
            if card.types.iter().any(|t| t == "Creature")
                && card.power.is_some()
                && card.toughness.is_some()
            {
                creature = true;
            }
        }
    }
    assert!(tapped_land, "a tapped land with a controller");
    assert!(creature, "a creature with power and toughness");
}

#[test]
fn states_carry_phase_and_clock() {
    let unit = unit(UNIT_02);
    let found = states(&unit).into_iter().any(|state| {
        state.turn_number.is_some_and(|turn| turn >= 1)
            && state.phase.is_some()
            && state.step.is_some()
            && state.priority_player.is_some()
            && state.server_now_ms.is_some()
    });
    assert!(
        found,
        "a state after turn 1 has phase, step, priority, clock"
    );
}

#[test]
fn stack_entries_name_their_source() {
    let unit = unit(UNIT_02);
    let all: Vec<_> = states(&unit).into_iter().flat_map(|s| s.stack).collect();
    let spell = all
        .iter()
        .find(|entry| !entry.is_ability && entry.source_name == "Expedition Map")
        .expect("a spell entry");
    assert_eq!(spell.source_id, 38);
    assert_eq!(spell.controller_seat, Some(0));
    let ability = all
        .iter()
        .find(|entry| entry.is_ability && !entry.text.is_empty())
        .expect("an ability entry with text");
    assert!(ability.is_trigger);
    assert!(
        all.iter().any(|entry| !entry.target_ids.is_empty()),
        "an entry with a target"
    );
}

#[test]
fn ability_entries_stay_out_of_the_stack_cards() {
    let unit = unit(UNIT_02);
    for state in states(&unit) {
        let abilities = state.stack.iter().filter(|e| e.is_ability).count();
        let spells = state.stack.len() - abilities;
        let stack_cards = state.cards.iter().filter(|c| c.zone == Zone::Stack).count();
        assert!(stack_cards <= spells, "abilities are not stack cards");
    }
}

#[test]
fn combat_events_list_attackers_and_blockers() {
    let attackers = events(&unit(UNIT_02))
        .into_iter()
        .find(|e| e.kind == "ATTACKERS_DECLARED" && !e.attacker_ids.is_empty())
        .expect("an attack");
    assert_eq!(attackers.attacker_ids, vec![82]);
    assert_eq!(attackers.player_name.as_deref(), Some("Opponent"));
    let blockers = events(&unit(UNIT_04))
        .into_iter()
        .find(|e| e.kind == "BLOCKERS_DECLARED" && !e.blockers.is_empty())
        .expect("a block");
    assert_eq!(blockers.blockers, vec![(16, 82)]);
}

#[test]
fn life_changes_carry_old_and_new() {
    let change = events(&unit(UNIT_02))
        .into_iter()
        .find(|e| e.kind == "PLAYER_LIFE_CHANGED")
        .expect("a life change");
    assert_eq!(change.old_value, Some(20));
    assert_eq!(change.new_value, Some(18));
}

#[test]
fn deltas_carry_life() {
    let found = [UNIT_02, UNIT_04]
        .into_iter()
        .flat_map(|text| deltas(&unit(text)))
        .any(|delta| delta.life.contains(&(0, 18)));
    assert!(found, "a delta sets seat 0 to 18");
    let phase = deltas(&unit(UNIT_02))
        .into_iter()
        .any(|delta| delta.phase.is_some() && delta.step.is_some());
    assert!(phase, "a delta carries phase and step");
}

#[test]
fn hostile_board_fields_decode_safely() {
    let entries = vec![r#"{"i":0,"life":5}"#; MAX_PROMPT_CARDS + 10].join(",");
    let many_life = format!(
        r#"{{"at":1,"src":"game_ws","dir":"in","url":"/ws","body":{{"type":"GAME_DELTA","payload":{{"patch":{{"players":[{entries}]}}}}}}}}"#
    );
    let unit = hand_built(&[
        r#"{"at":1,"src":"game_ws","dir":"in","url":"/ws","body":{"type":"GAME_STATE","seq":1,"payload":{"gameId":"g1","players":[{"id":"0","name":"P","battlefield":[{"id":4,"name":"Bear","power":"x","toughness":[],"counters":7,"attachedCards":{},"types":5,"tapped":"yes","controllerId":-3}]}],"stack":[{"isAbility":false,"sourceCard":{"id":9,"name":"Bolt"},"targets":[{"id":"y"},{"zone":"Player"},"junk"]}]}}}"#,
        r#"{"at":1,"src":"game_ws","dir":"in","url":"/ws","body":{"type":"GAME_EVENT","payload":{"gameId":"g1","type":"BLOCKERS_DECLARED","blockerAssignments":[],"attackerIds":"x","oldValue":"z"}}}"#,
        r#"{"at":1,"src":"game_ws","dir":"in","url":"/ws","body":{"type":"GAME_DELTA","payload":{"patch":{"players":[{"i":"x","life":3},{"i":1,"life":"y"},"junk"],"state":{"phase":4}}}}}"#,
        &many_life,
        r#"{"at":1,"src":"game_ws","dir":"in","url":"/ws","body":{"type":"GAME_DELTA","payload":{"patch":{"players":{}}}}}"#,
    ]);
    let state = match &unit.frames[0].kind {
        FrameKind::GameState(state) => state.clone(),
        other => panic!("expected a state, got {other:?}"),
    };
    let bear = &state.cards[0];
    assert_eq!((bear.power, bear.toughness), (None, None));
    assert!(bear.counters.is_empty());
    assert_eq!(bear.attached_to, None);
    assert!(bear.types.is_empty());
    assert!(!bear.tapped);
    assert_eq!(bear.controller_seat, None);
    assert_eq!(state.stack.len(), 1);
    assert!(state.stack[0].target_ids.is_empty());
    match &unit.frames[1].kind {
        FrameKind::GameEvent(event) => {
            assert!(event.blockers.is_empty());
            assert!(event.attacker_ids.is_empty());
            assert_eq!(event.old_value, None);
        }
        other => panic!("expected an event, got {other:?}"),
    }
    match &unit.frames[2].kind {
        FrameKind::GameDelta(delta) => {
            assert!(delta.life.is_empty());
            assert_eq!(delta.phase, None);
        }
        other => panic!("expected a delta, got {other:?}"),
    }
    match &unit.frames[3].kind {
        FrameKind::GameDelta(delta) => assert_eq!(delta.life.len(), MAX_PROMPT_CARDS),
        other => panic!("expected a delta, got {other:?}"),
    }
}

#[test]
fn the_stack_lists_its_top_first() {
    let unit = unit(UNIT_04);
    let stack = states(&unit)
        .into_iter()
        .map(|state| state.stack)
        .find(|stack| stack.iter().any(|e| e.source_name == "Force of Negation"))
        .expect("a state with Force of Negation on the stack");
    assert_eq!(stack[0].source_name, "Force of Negation");
    assert_eq!(stack[1].source_name, "Kozilek's Command");
}

#[test]
fn deltas_carry_priority_and_clock() {
    let all = deltas(&unit(UNIT_02));
    assert!(all.iter().any(|d| d.priority_player.is_some()));
    assert!(all.iter().any(|d| d.server_now_ms.is_some()));
}

#[test]
fn counters_attachments_and_player_targets_decode() {
    let unit = hand_built(&[
        r#"{"at":1,"src":"game_ws","dir":"in","url":"/ws","body":{"type":"GAME_STATE","seq":1,"payload":{"gameId":"g1","players":[{"id":"0","name":"P","poisonCounters":"2","battlefield":[{"id":4,"name":"Bear","counters":{"P1P1":2,"bad":"x"},"attachedCards":[5,"x"],"types":["Creature"]},{"id":5,"name":"Aura","types":["Enchantment"]}]}],"stack":[{"isAbility":true,"isTrigger":false,"abilityDescription":"Zap","controllerId":"1","sourceCard":{"id":4,"name":"Bear"},"targetDescription":"[P], [Aura (5)]","targets":[{"id":-1,"zone":"Player"},{"id":5}]}]}}}"#,
    ]);
    let state = match &unit.frames[0].kind {
        FrameKind::GameState(state) => state.clone(),
        other => panic!("expected a state, got {other:?}"),
    };
    assert_eq!(state.player_states[0].poison, Some(2));
    assert_eq!(state.cards[0].counters, vec![("P1P1".to_string(), 2)]);
    assert_eq!(state.cards[0].attached_to, None);
    assert_eq!(state.cards[1].attached_to, Some(4));
    assert_eq!(
        state.cards.len(),
        2,
        "the ability source is not a stack card"
    );
    assert_eq!(state.stack[0].target_ids, vec![-1, 5]);
    assert_eq!(state.stack[0].text, "Zap");
    assert_eq!(state.stack[0].controller_seat, Some(1));
    assert_eq!(state.stack[0].target_description, "[P], [Aura (5)]");
}

#[test]
fn the_last_host_naming_a_card_is_its_host() {
    // Hosts in two players' zones name the same card (hostile: a card has
    // one host); the last one read wins, and a card no host names has none.
    let unit = hand_built(&[
        r#"{"at":1,"src":"game_ws","dir":"in","url":"/ws","body":{"type":"GAME_STATE","seq":1,"payload":{"gameId":"g1","players":[{"id":"0","name":"P","battlefield":[{"id":4,"name":"Bear","attachedCards":[5,7],"types":["Creature"]},{"id":5,"name":"Aura","types":["Enchantment"]},{"id":8,"name":"Rock","types":["Artifact"]}]},{"id":"1","name":"Q","battlefield":[{"id":6,"name":"Wall","attachedCards":[5],"types":["Creature"]},{"id":7,"name":"Sword","types":["Artifact"]}]}]}}}"#,
    ]);
    let state = match &unit.frames[0].kind {
        FrameKind::GameState(state) => state.clone(),
        other => panic!("expected a state, got {other:?}"),
    };
    let host_of = |id: u64| {
        state
            .cards
            .iter()
            .find(|card| card.id == id)
            .and_then(|card| card.attached_to)
    };
    assert_eq!(host_of(5), Some(6));
    assert_eq!(host_of(7), Some(4));
    assert_eq!(host_of(8), None);
    assert_eq!(host_of(4), None);
}

#[test]
fn a_stack_entry_without_a_target_description_has_an_empty_one() {
    let unit = unit(UNIT_02);
    let all: Vec<_> = states(&unit).into_iter().flat_map(|s| s.stack).collect();
    assert!(all.iter().any(|entry| entry.target_description.is_empty()));
    // Kozilek's Command names the player it targets only here: the target
    // itself is id -1 with no owner.
    assert!(
        all.iter()
            .any(|entry| entry.target_description.starts_with("[Player], "))
    );
}

#[test]
fn reveal_lists_stay_aligned_past_the_cap() {
    let ids: Vec<String> = (0..600)
        .map(|n| {
            if n == 5 {
                "\"x\"".to_string()
            } else {
                n.to_string()
            }
        })
        .collect();
    let names: Vec<String> = (0..600).map(|n| format!("\"C{n}\"")).collect();
    let line = |ids: &[String]| {
        format!(
            r#"{{"at":1,"src":"game_ws","dir":"in","url":"/ws","body":{{"type":"GAME_EVENT","payload":{{"gameId":"g1","type":"CARD_REVEALED","cardIds":[{}],"cardNames":[{}]}}}}}}"#,
            ids.join(","),
            names.join(",")
        )
    };
    let good: Vec<String> = (0..600).map(|n| n.to_string()).collect();
    let unit = hand_built(&[&line(&ids), &line(&good)]);
    match &unit.frames[0].kind {
        FrameKind::GameEvent(event) => {
            assert!(event.card_ids.is_empty(), "one bad id empties both lists");
            assert!(event.card_names.is_empty());
        }
        other => panic!("expected an event, got {other:?}"),
    }
    match &unit.frames[1].kind {
        FrameKind::GameEvent(event) => {
            assert_eq!(event.card_ids.len(), MAX_PROMPT_CARDS);
            assert_eq!(event.card_names.len(), MAX_PROMPT_CARDS);
            assert_eq!(event.card_names[499], "C499");
        }
        other => panic!("expected an event, got {other:?}"),
    }
}

#[test]
fn a_mulligan_prompt_arrives_in_a_delta() {
    let unit = unit(UNIT_04);
    let found = deltas(&unit).into_iter().any(|delta| {
        delta.prompt.is_some_and(|prompt| {
            prompt.context == "mulligan" && prompt.options.len() == 7 && prompt.version == Some(415)
        })
    });
    assert!(found, "a delta holds the 7 card mulligan prompt");
}

#[test]
fn a_sideboard_option_keeps_its_printing() {
    let unit = unit(UNIT_04);
    let prompt = states(&unit)
        .into_iter()
        .find(|state| state.sideboarding)
        .and_then(|state| state.prompt)
        .expect("a sideboard prompt");
    assert!(prompt.options.iter().any(|card| !card.set_code.is_empty()));
}

#[test]
fn an_action_without_a_payload_reads_the_frame_body() {
    let unit = hand_built(&[
        r#"{"at":1,"src":"lobby_ws","dir":"out","url":"/ws","body":{"type":"CONCEDE","matchId":"m1"}}"#,
        r#"{"at":1,"src":"game_ws","dir":"out","url":"/ws","body":{"type":"SIDEBOARD_SUBMIT","payload":"junk","orderedCards":[3,4],"promptVersion":9}}"#,
    ]);
    match &unit.frames[0].kind {
        FrameKind::Action(action) => {
            assert_eq!(action.kind, "CONCEDE");
            assert_eq!(action.match_id.as_deref(), Some("m1"));
        }
        other => panic!("expected an action, got {other:?}"),
    }
    match &unit.frames[1].kind {
        FrameKind::Action(action) => {
            assert_eq!(action.ordered_cards, vec![3, 4]);
            assert_eq!(action.prompt_version, Some(9));
        }
        other => panic!("expected an action, got {other:?}"),
    }
}
