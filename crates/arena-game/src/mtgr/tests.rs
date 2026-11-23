use std::path::PathBuf;

use super::{CardData, CardTable, GameMessages, MatchInput, MtgrError, match_record};
use crate::test_support::fixture_match_game;

/// The fixture card table, `fixtures/arena/cards.json` (grpId -> name, and
/// the face names of a multi-faced card when a test needs them).
fn fixture_cards() -> CardTable {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/arena/cards.json");
    let text = std::fs::read_to_string(path).expect("the fixture card table exists");
    let value: serde_json::Value = serde_json::from_str(&text).expect("the card table is JSON");
    let mut cards = CardTable::new();
    for (grp_id, card) in value.as_object().expect("the card table is an object") {
        let name = card["name"].as_str().expect("every card has a name");
        let faces = match card.get("faces") {
            Some(faces) => faces
                .as_array()
                .expect("faces is a list")
                .iter()
                .map(|face| face.as_str().expect("a face is a name").to_string())
                .collect(),
            None => Vec::new(),
        };
        cards.insert(
            grp_id.parse().expect("keys are grpIds"),
            CardData {
                name: name.to_string(),
                faces,
            },
        );
    }
    cards
}

/// Match `match_index` of fixture `name`, with its games 1 to 3 (empty games
/// included, as the store would give them).
fn fixture_match(name: &str, match_index: usize, opponent: &str) -> MatchInput {
    let mut games = Vec::new();
    for number in 1..=3 {
        let inputs = fixture_match_game(name, match_index, number).inputs;
        if !inputs.is_empty() {
            games.push(GameMessages {
                number,
                messages: inputs,
            });
        }
    }
    MatchInput {
        match_token: format!("{name}-{match_index}"),
        event: "Traditional_Explorer_Ladder".to_string(),
        format: "Explorer".to_string(),
        local_name: "Player".to_string(),
        opponent_name: opponent.to_string(),
        local_archetype: None,
        opponent_archetype: None,
        started_at: 0,
        games,
    }
}

fn three_games() -> MatchInput {
    fixture_match("bo3-three-games", 0, "Opponent")
}

/// The loss fixture's second match: game 1 conceded at the mulligan, then
/// the match conceded while sideboarding. The local player is seat 2.
fn match_concede() -> MatchInput {
    fixture_match("bo3-loss-and-match-concede", 1, "Opponent2")
}

/// The `.mtgr` goldens live in the crate's `src/snapshots`, where the UI's
/// goldens test validates them.
fn assert_golden(name: &str, record: &::mtgr::Record) {
    insta::with_settings!({ snapshot_path => "../snapshots" }, {
        insta::assert_snapshot!(name, ::mtgr::write(record));
    });
}

fn tag<'a>(game: &'a ::mtgr::Game, name: &str) -> Option<&'a str> {
    game.tags
        .iter()
        .find(|tag| tag.name == name)
        .map(|tag| tag.value.as_str())
}

#[test]
fn three_games_record() {
    let record = match_record(&three_games(), &fixture_cards()).expect("card data is complete");
    assert_golden("three_games_record", &record);
}

#[test]
fn concede_record() {
    let record = match_record(&match_concede(), &fixture_cards()).expect("card data is complete");
    assert_golden("concede_record", &record);
}

#[test]
fn loss_record() {
    let input = fixture_match("bo3-loss-and-match-concede", 0, "Opponent");
    let record = match_record(&input, &fixture_cards()).expect("card data is complete");
    assert_golden("loss_record", &record);
}

#[test]
fn tags_follow_the_roster() {
    let mut input = match_concede();
    input.local_archetype = Some("Fixture Local Deck".to_string());
    input.opponent_archetype = Some("Fixture Opponent Deck".to_string());
    let record = match_record(&input, &fixture_cards()).expect("card data is complete");
    let game = &record.games[0];
    assert_eq!(tag(game, "Mtgr"), Some("0.3.0"));
    assert_eq!(tag(game, "Site"), Some("Arena"));
    assert_eq!(tag(game, "Round"), Some("-"));
    assert_eq!(tag(game, "Event"), Some("Traditional_Explorer_Ladder"));
    assert_eq!(tag(game, "Format"), Some("Explorer"));
    assert_eq!(tag(game, "Player1"), Some("Opponent2"), "seat 1");
    assert_eq!(tag(game, "Player2"), Some("Player"), "seat 2 is local");
    assert_eq!(tag(game, "Perspective"), Some("Player"));
    assert_eq!(tag(game, "Player1Archetype"), Some("Fixture Opponent Deck"));
    assert_eq!(tag(game, "Player2Archetype"), Some("Fixture Local Deck"));
    assert_eq!(tag(game, "Game"), Some("1"));
    assert_eq!(
        tag(game, "ARENA_MatchId"),
        Some("bo3-loss-and-match-concede-1")
    );
    assert_eq!(tag(game, "Match"), Some("bo3-loss-and-match-concede-1"));

    let three = match_record(&three_games(), &fixture_cards()).expect("card data is complete");
    let results: Vec<(Option<&str>, Option<&str>, ::mtgr::GameResult)> = three
        .games
        .iter()
        .map(|game| (tag(game, "Game"), tag(game, "Result"), game.result))
        .collect();
    assert_eq!(
        results,
        vec![
            (Some("1"), Some("0-1"), ::mtgr::GameResult::Player2),
            (Some("2"), Some("1-0"), ::mtgr::GameResult::Player1),
            (Some("3"), Some("0-1"), ::mtgr::GameResult::Player2),
        ]
    );
    for game in &three.games {
        assert_eq!(tag(game, "Player1"), Some("Player"));
        assert_eq!(tag(game, "Player2"), Some("Opponent"));
        assert_eq!(tag(game, "Termination"), Some("concession"));
        assert_eq!(tag(game, "Player1Archetype"), None);
    }
}

/// Review Focus 4: the match conceded between games 1 and 2. Game 1 ends
/// with its own concede; game 2 never started, so the record holds one game.
#[test]
fn match_concede_between_games_writes_no_extra_game() {
    let input = match_concede();
    assert_eq!(input.games.len(), 2, "game 2 holds the match concede");
    let record = match_record(&input, &fixture_cards()).expect("card data is complete");
    assert_eq!(record.games.len(), 1);
    let game = &record.games[0];
    assert_eq!(tag(game, "Result"), Some("1-0"));
    assert_eq!(game.result, ::mtgr::GameResult::Player1);
    assert_eq!(tag(game, "Termination"), Some("concession"));
    let text = ::mtgr::write(&record);
    let concedes: Vec<&str> = text
        .lines()
        .filter(|line| line.ends_with(" concede"))
        .collect();
    assert_eq!(concedes.len(), 1);
    assert!(concedes[0].ends_with("  \"Player\" concede"));
    let last_body_line = text.lines().rev().nth(1).expect("a body line");
    assert_eq!(last_body_line, concedes[0], "nothing follows the concede");
}

/// Decision 0048: a card the card data cannot name is written `?` with its
/// Arena source id, on its `new card`, `identify` and deck lines, and is
/// referred to by id only. Nothing is refused. Youthful Valkyrie (75350) is
/// the local player's, introduced by name; Howlpack Piper (79017) is the
/// opponent's, hidden, identified as it is cast as a face, which a `?` card
/// does not name (no `face`).
#[test]
fn unknown_cards_are_written_with_unknown_names() {
    let mut cards = fixture_cards();
    cards.remove(&75350);
    cards.remove(&79017);
    let record = match_record(&three_games(), &cards).expect("unknown names are written");
    let games = raw_statements_by_game(&record);
    let statements: Vec<String> = games.iter().flatten().cloned().collect();
    let text = statements.join("\n");
    assert!(!text.contains("Youthful Valkyrie"), "no name for 75350");
    assert!(!text.contains("Howlpack Piper"), "no name for 79017");
    assert!(statements.contains(&"main 4 ? src arena:75350".to_string()));

    let new_cards: Vec<&String> = statements
        .iter()
        .filter(|s| s.starts_with("new card ") && s.ends_with(" src arena:75350"))
        .collect();
    assert!(!new_cards.is_empty());
    for statement in &new_cards {
        assert!(statement.contains(" ? owner \"Player\" in "), "{statement}");
    }
    let mut identified = 0;
    for game in &games {
        for statement in game {
            if !statement.contains(" identify ? src arena:79017") {
                continue;
            }
            identified += 1;
            let id = id_at(statement).expect("an id");
            for later in game.iter().filter(|s| mentions(s, id)) {
                assert!(!later.contains(&format!("#{id} \"")), "named: {later}");
                assert!(!later.contains(" face "), "a face on a ? card: {later}");
            }
        }
    }
    assert!(identified > 0);
}

/// Decision 0048 item 8: in each of main and side, unknown entries come
/// after the named ones, one per source id, in code point order of it.
#[test]
fn unknown_deck_entries_sort_after_named_ones() {
    let mut cards = fixture_cards();
    // Godless Shrine and Fatal Push (main), Thoughtseize (side).
    for grp_id in [75662, 69396, 73905] {
        cards.remove(&grp_id);
    }
    let record = match_record(&three_games(), &cards).expect("unknown names are written");
    let statements = raw_statements_by_game(&record).remove(0);
    let main: Vec<&String> = statements
        .iter()
        .filter(|s| s.starts_with("main "))
        .collect();
    let side: Vec<&String> = statements
        .iter()
        .filter(|s| s.starts_with("side "))
        .collect();
    let unknown_main: Vec<&str> = main
        .iter()
        .filter(|s| s.contains(" ? "))
        .map(|s| s.as_str())
        .collect();
    assert_eq!(
        unknown_main,
        vec!["main 4 ? src arena:69396", "main 4 ? src arena:75662"]
    );
    assert_eq!(main[main.len() - 2..], unknown_main[..]);
    assert_eq!(
        side.last().map(|s| s.as_str()),
        Some("side 3 ? src arena:73905")
    );
    let named: Vec<&str> = main
        .iter()
        .filter(|s| !s.contains(" ? "))
        .filter_map(|s| s.split_once(" \"").map(|(_, name)| name))
        .collect();
    let mut sorted = named.clone();
    sorted.sort();
    assert_eq!(named, sorted, "named entries stay in name order");
}

/// The concede match with three of the local player's cards unknown: a
/// golden the UI's goldens test validates (decision 0048 forms).
#[test]
fn concede_record_with_unknown_cards() {
    let mut cards = fixture_cards();
    // Spell Pierce, Steam Vents, Fable of the Mirror-Breaker.
    for grp_id in [66125, 68738, 79570] {
        cards.remove(&grp_id);
    }
    let record = match_record(&match_concede(), &cards).expect("unknown names are written");
    assert_golden("concede_record_with_unknown_cards", &record);
}

/// The lines of game `number` of a record's text, from its `pregame` line
/// to its first `turn` line (or its last body line).
fn pregame_lines(record: &::mtgr::Record, number: usize) -> Vec<String> {
    let game = ::mtgr::Record {
        games: vec![record.games[number - 1].clone()],
    };
    ::mtgr::write(&game)
        .lines()
        .skip_while(|line| !line.ends_with(" pregame"))
        .take_while(|line| !line.contains(" turn "))
        .filter(|line| line.contains(' '))
        .map(str::to_string)
        .collect()
}

/// A keep is written in the state where Arena stops waiting for the seat's
/// `MulliganResp` with its mulligan count unchanged, not guessed.
#[test]
fn keeps_follow_the_pending_mulligan_response() {
    // Match 2: Opponent2 (seat 1) keeps in gsId 3, before the local player
    // concedes at the mulligan.
    let record = match_record(&match_concede(), &fixture_cards()).expect("card data is complete");
    let lines = pregame_lines(&record, 1);
    let tail: Vec<&str> = lines
        .iter()
        .rev()
        .take(2)
        .rev()
        .map(String::as_str)
        .collect();
    assert_eq!(
        tail,
        vec!["0:26   \"Opponent2\" keep", "0:27   \"Player\" concede"]
    );

    // Three games, game 3: the opponent keeps in gsId 3, before the local
    // player's keep, not at the start of turn 1.
    let record = match_record(&three_games(), &fixture_cards()).expect("card data is complete");
    let lines = pregame_lines(&record, 3);
    let tail: Vec<&str> = lines
        .iter()
        .rev()
        .take(2)
        .rev()
        .map(String::as_str)
        .collect();
    assert_eq!(
        tail,
        vec!["0:24   \"Opponent\" keep", "0:25   \"Player\" keep"]
    );
}

/// The starting player's chooser is the seat Arena waits on for
/// `ChooseStartingPlayerResp`.
#[test]
fn starting_chooser_is_the_pending_seat() {
    let record = match_record(&three_games(), &fixture_cards()).expect("card data is complete");
    let chooses: Vec<String> = (1..=3)
        .flat_map(|number| pregame_lines(&record, number))
        .filter(|line| line.contains(" choose first "))
        .map(|line| {
            line.split_once("   ")
                .map(|(_, rest)| rest.to_string())
                .unwrap_or(line)
        })
        .collect();
    assert_eq!(
        chooses,
        vec![
            "\"Opponent\" choose first \"Opponent\"",
            "\"Player\" choose first \"Player\"",
            "\"Opponent\" choose first \"Opponent\"",
        ]
    );
}

/// Arena games have two seats. A state that waits on both seats for
/// `ChooseStartingPlayerResp` names no chooser; the chooser is the seat a
/// state waits on alone, not the lowest seat of the first state.
#[test]
fn starting_chooser_is_the_one_seat_waited_on() {
    let state = |id: u32, pending: &[u32]| {
        let players: Vec<String> = [1, 2]
            .iter()
            .map(|seat| {
                let message = if pending.contains(seat) {
                    r#","pendingMessageType":"ClientMessageType_ChooseStartingPlayerResp""#
                } else {
                    ""
                };
                format!(r#"{{"systemSeatNumber":{seat},"lifeTotal":20{message}}}"#)
            })
            .collect();
        format!(
            r#"{{"type":"GREMessageType_GameStateMessage","systemSeatIds":[1],
              "gameStateMessage":{{"type":"GameStateType_Full","gameStateId":{id},
                "players":[{players}]}}}}"#,
            players = players.join(",")
        )
    };
    let messages: Vec<crate::GameInput> = [state(1, &[1, 2]), state(2, &[2]), state(3, &[])]
        .iter()
        .map(|json| synthetic_state(json))
        .collect();
    let fold = crate::fold_game(&messages);
    assert_eq!(fold.states.len(), 3);
    assert_eq!(super::starting_choice(&fold, 2), Some((2, 2)));

    // Only ever both seats: no chooser to write.
    let messages: Vec<crate::GameInput> = [state(1, &[1, 2]), state(2, &[])]
        .iter()
        .map(|json| synthetic_state(json))
        .collect();
    let fold = crate::fold_game(&messages);
    assert_eq!(super::starting_choice(&fold, 1), None);
}

/// The id written after `#` at the start of `text`, if any.
fn id_at(text: &str) -> Option<u32> {
    let digits: String = text
        .strip_prefix('#')?
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    digits.parse().ok()
}

/// Each body line of a record's text without its time and indent, per game,
/// and without its source ids (`src arena:N`, metadata that
/// `named_cards_carry_their_arena_source_id` checks).
fn statements_by_game(record: &::mtgr::Record) -> Vec<Vec<String>> {
    raw_statements_by_game(record)
        .into_iter()
        .map(|statements| {
            statements
                .into_iter()
                .map(|statement| match statement.split_once(" src ") {
                    Some((before, _)) => before.to_string(),
                    None => statement,
                })
                .collect()
        })
        .collect()
}

/// Each body line of a record's text without its time and indent, per game.
fn raw_statements_by_game(record: &::mtgr::Record) -> Vec<Vec<String>> {
    record
        .games
        .iter()
        .map(|game| {
            let one = ::mtgr::Record {
                games: vec![game.clone()],
            };
            ::mtgr::write(&one)
                .lines()
                .filter(|line| line.contains(':') && !line.starts_with('['))
                .filter_map(|line| {
                    line.split_once(' ')
                        .map(|(_, rest)| rest.trim_start().to_string())
                })
                .collect()
        })
        .collect()
}

/// Whether `statement` refers to object `id` (as `#id` followed by a space
/// or the end of the line).
fn mentions(statement: &str, id: u32) -> bool {
    let needle = format!("#{id}");
    statement.match_indices(&needle).any(|(at, _)| {
        let rest = &statement[at + needle.len()..];
        !rest.starts_with(|c: char| c.is_ascii_digit())
    })
}

/// Review Focus 1: an opponent card is never named while it sits in a
/// library or hand. It is named as it is introduced only when that same move
/// takes it to a public zone (spec 6.5, decision 0019); a hidden card is
/// identified only by the line that makes it public (a reveal, cast, play
/// or move to a public zone).
#[test]
fn unrevealed_opponent_cards_stay_hidden() {
    let loss = fixture_match("bo3-loss-and-match-concede", 0, "Opponent");
    let mut games = Vec::new();
    for input in [three_games(), loss] {
        let record = match_record(&input, &fixture_cards()).expect("card data is complete");
        games.extend(statements_by_game(&record));
    }
    let mut named_as_they_leave = 0;
    for statements in games {
        for (at, statement) in statements.iter().enumerate() {
            let named_in_hidden_zone = statement.starts_with("new card ")
                && statement.contains(" owner \"Opponent\" in ")
                && (statement.ends_with(" in library") || statement.ends_with(" in hand"));
            if !named_in_hidden_zone {
                continue;
            }
            let id = id_at(statement.trim_start_matches("new card ")).expect("an id");
            let next = statements[at + 1..]
                .iter()
                .find(|s| mentions(s, id))
                .expect("an introduced card is used");
            assert!(
                makes_public(next),
                "named while hidden: {statement} / {next}"
            );
            named_as_they_leave += 1;
        }
        let hidden: Vec<u32> = statements
            .iter()
            .filter(|s| s.starts_with("new hidden ") && s.contains(" owner \"Opponent\" "))
            .filter_map(|s| id_at(s.trim_start_matches("new hidden ")))
            .collect();
        assert!(!hidden.is_empty());
        for id in hidden {
            let start = statements
                .iter()
                .position(|s| s.starts_with(&format!("new hidden #{id} ")))
                .expect("introduced");
            let named = format!("#{id} \"");
            let identify = format!("#{id} identify ");
            let mut rest = statements[start + 1..].iter();
            while let Some(statement) = rest.next() {
                if statement.starts_with(&identify) {
                    let next = rest
                        .find(|s| mentions(s, id))
                        .expect("an identified card is used");
                    assert!(makes_public(next), "#{id} identified for {next}");
                    break;
                }
                assert!(
                    !statement.contains(&named),
                    "#{id} named before it is identified: {statement}"
                );
            }
        }
    }
    assert!(
        named_as_they_leave > 0,
        "Collected Company's cards are named as they leave"
    );
}

/// Spec 6.4: a card introduced by name, or identified, carries the grpId of
/// the card that named it as `src arena:N` (a back face by its front card's).
/// A hidden card has none: its grpId would name it.
#[test]
fn named_cards_carry_their_arena_source_id() {
    let cards = fixture_cards();
    let loss = fixture_match("bo3-loss-and-match-concede", 0, "Opponent");
    let mut statements = Vec::new();
    for input in [three_games(), loss] {
        let record = match_record(&input, &cards).expect("card data is complete");
        statements.extend(raw_statements_by_game(&record).into_iter().flatten());
    }
    let mut named = 0;
    let mut identified = 0;
    for statement in &statements {
        if statement.starts_with("new hidden ") {
            assert!(
                !statement.contains(" src "),
                "hidden with a source: {statement}"
            );
            continue;
        }
        let is_new_card = statement.starts_with("new card ");
        let is_identify = statement.contains(" identify ");
        if !is_new_card && !is_identify {
            assert!(
                !statement.contains(" src "),
                "a source id elsewhere: {statement}"
            );
            continue;
        }
        let (before, source) = statement
            .split_once(" src arena:")
            .unwrap_or_else(|| panic!("no source id: {statement}"));
        let grp_id: u32 = source.parse().expect("one Arena source id");
        let card = cards
            .get(&grp_id)
            .expect("the source is a card in the data");
        let name = format!("\"{}\"", card.name);
        let quoted = if is_identify {
            before.split_once(" identify ").map(|(_, name)| name)
        } else {
            before
                .split_once(' ')
                .and_then(|(_, rest)| rest.split_once(' '))
                .map(|(_, rest)| rest)
        };
        assert!(
            quoted.is_some_and(|text| text.contains(&name)),
            "{statement} is not named by {grp_id}"
        );
        if is_identify {
            identified += 1;
        } else {
            named += 1;
        }
    }
    assert!(
        named > 0 && identified > 0,
        "{named} named, {identified} identified"
    );
    assert!(
        statements.iter().any(|s| s
            .contains(" identify \"Fixture Pathway 73476 // Fixture Pathway 73477\"")
            && s.ends_with(" src arena:73476")),
        "a back face's source is its front card"
    );
}

/// Whether a line makes its object public: a reveal, a cast, a play, or a
/// move to a public zone.
fn makes_public(statement: &str) -> bool {
    let verb = [" cast from ", " play from ", " reveal"]
        .iter()
        .any(|verb| statement.contains(verb));
    let public_move = statement.contains(" move ")
        && ["battlefield", "graveyard", "exile", "stack", "command"]
            .iter()
            .any(|zone| statement.contains(&format!("-> {zone}")));
    verb || public_move
}

/// The three-games token lines (the Samurai tokens of game 2).
fn token_lines(cards: &super::CardTable) -> Vec<String> {
    let record = match_record(&three_games(), cards).expect("card data is complete");
    statements_by_game(&record)
        .into_iter()
        .flatten()
        .filter(|statement| statement.starts_with("new token "))
        .collect()
}

/// Review Focus 5: a token with card data for its grpId takes that name.
#[test]
fn token_named_from_card_data() {
    let mut cards = fixture_cards();
    cards.insert(
        79745,
        CardData {
            name: "Fixture Samurai".to_string(),
            faces: Vec::new(),
        },
    );
    let lines = token_lines(&cards);
    assert!(!lines.is_empty());
    for line in &lines {
        assert!(
            line.contains(
                " \"Fixture Samurai\" owner \"Player\" in battlefield types creature pt 2/2"
            ),
            "{line}"
        );
    }
}

/// Review Focus 5: a token without card data is named by its subtypes and
/// "Token" (CR 111.4).
#[test]
fn token_named_by_subtypes_without_card_data() {
    let mut cards = fixture_cards();
    cards.remove(&79745);
    let lines = token_lines(&cards);
    assert!(!lines.is_empty());
    for line in &lines {
        assert!(
            line.contains(" \"Samurai Token\" owner \"Player\" "),
            "{line}"
        );
    }
}

/// Arena sends no rules text: every ability is written with text `?`
/// (spec section 6.5 forbids filling it from a card database).
#[test]
fn ability_text_is_unknown() {
    let record = match_record(&three_games(), &fixture_cards()).expect("card data is complete");
    let abilities: Vec<String> = statements_by_game(&record)
        .into_iter()
        .flatten()
        .filter(|statement| statement.starts_with("new ability "))
        .collect();
    assert!(!abilities.is_empty());
    for line in &abilities {
        let (_, after_of) = line.split_once(" of #").expect("an ability has a source");
        assert!(after_of.contains(" ? controller "), "{line}");
    }
}

/// Every `countered by #M` line is followed, on the same object, by the line
/// that takes it off the stack with the same cause.
fn countered_pairs(statements: &[String]) -> Vec<(String, String)> {
    let mut pairs = Vec::new();
    for (at, statement) in statements.iter().enumerate() {
        let Some((object, cause)) = statement.split_once(" countered by ") else {
            continue;
        };
        let id = id_at(object).expect("an id");
        let next = statements[at + 1..]
            .iter()
            .find(|s| mentions(s, id))
            .expect("a countered object leaves the stack");
        assert!(
            next.ends_with(&format!(" by {cause}")),
            "{statement} / {next}"
        );
        pairs.push((statement.clone(), next.clone()));
    }
    pairs
}

/// Arena's `Countered` transfers: the spell is countered by the resolving
/// counterspell, then moves off the stack with the same cause.
#[test]
fn countered_spells_are_written_countered() {
    let input = fixture_match("bo3-loss-and-match-concede", 0, "Opponent");
    let record = match_record(&input, &fixture_cards()).expect("card data is complete");
    let pairs: Vec<(String, String)> = statements_by_game(&record)
        .iter()
        .flat_map(|statements| countered_pairs(statements))
        .collect();
    assert_eq!(pairs.len(), 5);
    for (_, leave) in &pairs {
        assert!(leave.contains(" move stack -> "), "{leave}");
    }
}

/// An ability removed while another object resolves was countered by it:
/// the ward trigger counters Barrels of Blasting Jelly's ability.
#[test]
fn countered_abilities_cease_by_the_counter() {
    let record = match_record(&three_games(), &fixture_cards()).expect("card data is complete");
    let pairs: Vec<(String, String)> = statements_by_game(&record)
        .iter()
        .flat_map(|statements| countered_pairs(statements))
        .collect();
    assert_eq!(pairs.len(), 1);
    let (countered, cease) = &pairs[0];
    assert!(countered.starts_with("#82 countered by #84"), "{countered}");
    assert_eq!(cease, "#82 cease by #84");
}

/// `AttachmentCreated`: an Aura attaches as it resolves, and keyframes list
/// what it is attached to.
#[test]
fn auras_attach_as_they_resolve() {
    let record = match_record(&three_games(), &fixture_cards()).expect("card data is complete");
    let statements: Vec<String> = statements_by_game(&record).into_iter().flatten().collect();
    let attaches: Vec<&String> = statements
        .iter()
        .filter(|s| s.contains(" attach #"))
        .collect();
    assert_eq!(attaches.len(), 3);
    for attach in &attaches {
        assert!(attach.ends_with(" by rule:resolve"), "{attach}");
    }
    assert!(statements.iter().any(|s| s.contains(" attached #")));
}

/// `RevealedCardCreated`: a card revealed as it is put into a hand is named
/// in its library, revealed, then moved, each by the ability that did it.
#[test]
fn revealed_cards_are_revealed() {
    let record = match_record(&three_games(), &fixture_cards()).expect("card data is complete");
    let statements: Vec<String> = statements_by_game(&record).into_iter().flatten().collect();
    let reveals: Vec<usize> = statements
        .iter()
        .enumerate()
        .filter(|(_, s)| s.contains(" reveal by "))
        .map(|(at, _)| at)
        .collect();
    assert_eq!(reveals.len(), 3);
    for at in reveals {
        let id = id_at(&statements[at]).expect("an id");
        assert!(statements[at - 1].starts_with(&format!("new card #{id} ")));
        assert!(statements[at - 1].ends_with(" in library"));
        let cause = statements[at].split_once(" reveal by ").expect("a cause").1;
        let next = &statements[at + 1];
        assert!(
            next.contains(" move library -> hand ") && next.ends_with(&format!(" by {cause}")),
            "{next}"
        );
    }
}

/// Power and toughness changes from static and Aura effects name the
/// effect's source (`LayeredEffectCreated`), not `?`.
#[test]
fn effect_changes_name_their_source() {
    let record = match_record(&three_games(), &fixture_cards()).expect("card data is complete");
    let statements: Vec<String> = statements_by_game(&record).into_iter().flatten().collect();
    let speaker = statements
        .iter()
        .find(|s| s.starts_with("#45 \"Speaker of the Heavens\" pt 1/1 -> 2/1 "))
        .expect("the Aura's pump");
    assert!(!speaker.ends_with(" by ?"), "{speaker}");
    // A pt change is `#N "Name" pt A/B -> C/D by CAUSE`; a move's `pt`
    // (a permanent entering) is no change.
    let changes: Vec<&String> = statements
        .iter()
        .filter(|s| {
            s.split_once(" pt ")
                .is_some_and(|(_, rest)| rest.contains(" -> "))
        })
        .collect();
    assert_eq!(changes.len(), 37);
    let unknown: Vec<&String> = changes
        .iter()
        .copied()
        .filter(|s| s.ends_with(" by ?"))
        .collect();
    assert_eq!(unknown, Vec::<&String>::new());
}

/// Attacks and blocks are the first events of their declare step: nothing
/// in the step (an attacker tapping, a trigger) comes before them.
#[test]
fn declarations_open_their_step() {
    let loss = fixture_match("bo3-loss-and-match-concede", 0, "Opponent");
    let mut checked = 0;
    for input in [three_games(), loss] {
        let record = match_record(&input, &fixture_cards()).expect("card data is complete");
        for statements in statements_by_game(&record) {
            let mut step = "";
            let mut others_before = false;
            for statement in &statements {
                if !statement.contains(' ') || statement.starts_with("turn ") {
                    step = statement.as_str();
                    others_before = false;
                    continue;
                }
                let declaration = (step == "declare-attackers" && statement.contains(" attack "))
                    || (step == "declare-blockers" && statement.contains(" block #"));
                if declaration {
                    assert!(
                        !others_before,
                        "{statement} follows another event of {step}"
                    );
                    checked += 1;
                } else {
                    others_before = true;
                }
            }
        }
    }
    assert!(checked > 0);
}

/// The local player's two Youthful Valkyries (300, 301) and a Forest (302)
/// on the battlefield in turn 1, then the declare attackers step: states 1
/// to 4, the last with 300 declared but not submitted while the Forest
/// taps.
fn declaring_states() -> Vec<String> {
    let object = |id: u32, grp_id: u32, kind: &str, zone: u32, extra: &str| {
        format!(
            r#"{{"instanceId":{id},"grpId":{grp_id},"type":"GameObjectType_Card","zoneId":{zone},"visibility":"Visibility_Public","ownerSeatId":1,"controllerSeatId":1,"cardTypes":["CardType_{kind}"],"power":{{"value":2}},"toughness":{{"value":2}}{extra}}}"#
        )
    };
    let creature = |id: u32, zone: u32, extra: &str| object(id, 75350, "Creature", zone, extra);
    let land = |zone: u32, extra: &str| object(302, 95190, "Land", zone, extra);
    let state = |id: u32, body: &str| {
        format!(
            r#"{{"type":"GREMessageType_GameStateMessage","systemSeatIds":[1],
      "gameStateMessage":{{"type":"GameStateType_Diff","gameStateId":{id},"prevGameStateId":{prev},{body}}}}}"#,
            prev = id - 1
        )
    };
    let main =
        r#""turnInfo":{"turnNumber":1,"activePlayer":1,"phase":"Phase_Main1","priorityPlayer":1}"#;
    let declare = r#""turnInfo":{"turnNumber":1,"activePlayer":1,"phase":"Phase_Combat","step":"Step_DeclareAttack","decisionPlayer":1}"#;
    let full = format!(
        r#"{{"type":"GREMessageType_GameStateMessage","systemSeatIds":[1],
      "gameStateMessage":{{"type":"GameStateType_Full","gameStateId":1,
        "players":[{{"systemSeatNumber":1,"lifeTotal":20}},{{"systemSeatNumber":2,"lifeTotal":20}}],
        {main},
        "zones":[
          {{"zoneId":28,"type":"ZoneType_Battlefield","objectInstanceIds":[]}},
          {{"zoneId":31,"type":"ZoneType_Hand","ownerSeatId":1,"objectInstanceIds":[300,301,302]}}],
        "gameObjects":[{a},{b},{c}]}}}}"#,
        a = creature(300, 31, ""),
        b = creature(301, 31, ""),
        c = land(31, ""),
    );
    let put = state(
        2,
        &format!(
            r#"{main},
        "zones":[
          {{"zoneId":28,"type":"ZoneType_Battlefield","objectInstanceIds":[300,301,302]}},
          {{"zoneId":31,"type":"ZoneType_Hand","ownerSeatId":1,"objectInstanceIds":[]}}],
        "gameObjects":[{a},{b},{c}],
        "annotations":[
          {{"id":21,"affectedIds":[300,301,302],"type":["AnnotationType_ZoneTransfer"],
            "details":[{{"key":"zone_src","valueInt32":[31]}},{{"key":"zone_dest","valueInt32":[28]}},{{"key":"category","valueString":["Put"]}}]}}]"#,
            a = creature(300, 28, ""),
            b = creature(301, 28, ""),
            c = land(28, ""),
        ),
    );
    let combat = state(3, declare);
    let declared = r#","attackState":"AttackState_Declared","attackInfo":{"targetId":2}"#;
    let choosing = state(
        4,
        &format!(
            r#""gameObjects":[{a},{c}],
        "annotations":[
          {{"id":41,"affectorId":302,"affectedIds":[302],"type":["AnnotationType_TappedUntappedPermanent"],
            "details":[{{"key":"tapped","valueInt32":[1]}}]}}]"#,
            a = creature(300, 28, declared),
            c = land(28, r#","isTapped":true"#),
        ),
    );
    vec![full, put, combat, choosing]
}

/// A Youthful Valkyrie of the local player's, submitted as attacking.
fn submitted_attacker(id: u32) -> String {
    format!(
        r#"{{"instanceId":{id},"grpId":75350,"type":"GameObjectType_Card","zoneId":28,"visibility":"Visibility_Public","ownerSeatId":1,"controllerSeatId":1,"cardTypes":["CardType_Creature"],"power":{{"value":2}},"toughness":{{"value":2}},"attackState":"AttackState_Attacking","attackInfo":{{"targetId":2}},"isTapped":true}}"#
    )
}

/// Arena shows a declaration as it is made (`AttackState_Declared`) and
/// whole once submitted (`AttackState_Attacking`). An event while the
/// attacker is still choosing (here a land tapping, as for an attack tax)
/// must not write the attackers chosen so far: the declaration is written
/// whole, first in its step.
#[test]
fn a_declaration_is_written_whole() {
    let mut states = declaring_states();
    states.push(diff_state(
        5,
        r#"{"turnNumber":1,"activePlayer":1,"phase":"Phase_Combat","step":"Step_DeclareAttack","decisionPlayer":1}"#,
        &format!(
            r#","gameObjects":[{a},{b}],
        "annotations":[
          {{"id":51,"affectedIds":[300,301],"type":["AnnotationType_TappedUntappedPermanent"],
            "details":[{{"key":"tapped","valueInt32":[1]}}]}}]"#,
            a = submitted_attacker(300),
            b = submitted_attacker(301),
        ),
    ));
    let refs: Vec<&str> = states.iter().map(String::as_str).collect();
    let record =
        match_record(&synthetic_match(&refs), &fixture_cards()).expect("card data is complete");
    let statements = statements_by_game(&record).remove(0);
    let step = statements
        .iter()
        .position(|s| s == "declare-attackers")
        .unwrap_or_else(|| panic!("the step: {statements:?}"));
    let attacks: Vec<&String> = statements.iter().skip(step + 1).take(2).collect();
    assert!(
        attacks.len() == 2 && attacks.iter().all(|s| s.contains(" attack \"Opponent\"")),
        "{statements:#?}"
    );
}

/// A declaration Arena never shows submitted is never written: the game
/// conceded while the attacker still chooses writes no attack.
#[test]
fn a_declaration_never_submitted_is_not_written_at_a_concede() {
    let states = declaring_states();
    let refs: Vec<&str> = states.iter().map(String::as_str).collect();
    let mut input = synthetic_match(&refs);
    input.games[0].messages.push(crate::GameInput {
        source: crate::Source::Client,
        logged_at: 1_000,
        json: r#"{"type":"ClientMessageType_ConcedeReq","concedeReq":{"scope":"MatchScope_Game"}}"#
            .to_string(),
    });
    let record = match_record(&input, &fixture_cards()).expect("card data is complete");
    let statements = statements_by_game(&record).remove(0);
    assert!(
        statements.iter().any(|s| s.ends_with(" concede")),
        "{statements:#?}"
    );
    assert!(
        !statements.iter().any(|s| s.contains(" attack ")),
        "{statements:#?}"
    );
}

/// The state that submits a declaration is lost in a gap: the tentative
/// choice before it (300 alone, while the Forest taps) is not written. The
/// first state after the gap shows the attack submitted, so it is written
/// then, after the tap.
#[test]
fn a_declaration_submitted_in_a_gap_writes_no_tentative_choice() {
    let mut states = declaring_states();
    // State 5 is missing.
    let blockers = diff_state(
        6,
        r#"{"turnNumber":1,"activePlayer":1,"phase":"Phase_Combat","step":"Step_DeclareBlock","decisionPlayer":2}"#,
        &format!(r#","gameObjects":[{a}]"#, a = submitted_attacker(300)),
    );
    states.push(blockers);
    let refs: Vec<&str> = states.iter().map(String::as_str).collect();
    let record =
        match_record(&synthetic_match(&refs), &fixture_cards()).expect("card data is complete");
    let statements = statements_by_game(&record).remove(0);
    assert!(
        statements
            .iter()
            .any(|s| s.starts_with("incomplete \"Arena did not log")),
        "{statements:#?}"
    );
    let tap = statements
        .iter()
        .position(|s| s == "#3 \"Forest\" tap by #3")
        .unwrap_or_else(|| panic!("the tap: {statements:#?}"));
    assert!(
        !statements[..tap].iter().any(|s| s.contains(" attack ")),
        "{statements:#?}"
    );
}

/// CR 508.1h: Archangel of Tithes makes attacking the local player cost
/// {1} per attacker while it is untapped. Arena pays that mana toward a
/// seat, not an object, and names the Archangel only by the
/// `Qualification` it keeps on the protected player, so the payment is
/// `by cost:` the Archangel. Its block tax, while it attacks, has no such
/// annotation (Arena names nothing), so that payment stays `by ?`.
#[test]
fn attack_tax_is_paid_by_the_cost_of_its_permanent() {
    let record = match_record(&three_games(), &fixture_cards()).expect("card data is complete");
    let mut taxes = Vec::new();
    let mut unknown = Vec::new();
    for statements in statements_by_game(&record) {
        let mut step = "";
        for statement in &statements {
            if !statement.contains(' ') {
                step = statement.as_str();
                continue;
            }
            if !statement.contains("\" pay {") {
                continue;
            }
            let cause = statement.rsplit_once(" by ").expect("a cause").1;
            if !statement.starts_with("\"Opponent\" ") {
                // The local player casting the Archangel pays by its cost.
                continue;
            }
            if cause == "?" {
                unknown.push(format!("{step}: {statement}"));
                continue;
            }
            let id = cause.strip_prefix("cost:#").expect("a cost");
            let archangel = format!("#{id} \"Archangel of Tithes\"");
            if statements.iter().any(|s| s.contains(&archangel)) {
                taxes.push(format!("{step}: {statement}"));
            }
        }
    }
    assert_eq!(
        taxes
            .iter()
            .map(|tax| tax.split_once(" by ").expect("a cause").0)
            .collect::<Vec<_>>(),
        vec![
            "declare-attackers: \"Opponent\" pay {R}",
            "declare-attackers: \"Opponent\" pay {U}{R}",
        ]
    );
    assert_eq!(
        unknown,
        vec!["declare-blockers: \"Opponent\" pay {W} by ?".to_string()]
    );
}

/// The loss match's first game, as statements.
fn loss_game_one() -> Vec<String> {
    let input = fixture_match("bo3-loss-and-match-concede", 0, "Opponent");
    let record = match_record(&input, &fixture_cards()).expect("card data is complete");
    statements_by_game(&record).remove(0)
}

/// Decision 0029 and 0025: Arena shows the local companion, Kaheera, as a
/// public card in the sideboard from the first state. It is revealed in the
/// pregame, listed in keyframes as a companion while outside the game, and
/// put into the hand with the companion special action.
#[test]
fn companion_is_revealed_then_put_into_hand() {
    let statements = loss_game_one();
    let kaheera = "\"Kaheera, the Orphanguard\"";
    let new = statements
        .iter()
        .position(|s| {
            s.starts_with("new card #")
                && s.ends_with(&format!(
                    "{kaheera} owner \"Player\" in outside \"sideboard\""
                ))
        })
        .expect("the companion is introduced outside the game");
    let id = id_at(statements[new].trim_start_matches("new card ")).expect("an id");
    let object = format!("#{id} {kaheera}");
    assert_eq!(statements[new + 1], format!("{object} reveal by #{id}"));
    let choose = statements
        .iter()
        .position(|s| s.contains(" choose first "))
        .expect("a choose first line");
    let first_hand = statements
        .iter()
        .position(|s| s.ends_with(" by rule:opening-hand"))
        .expect("an opening hand");
    assert!(
        choose < new && new < first_hand,
        "after choose first, before the hands"
    );
    assert!(statements.contains(&format!(
        "{object} outside \"sideboard\" \"Player\" companion"
    )));
    let special = statements
        .iter()
        .position(|s| *s == format!("{object} special companion"))
        .expect("the special action");
    let put = statements
        .iter()
        .position(|s| s.starts_with(&format!("{object} move outside \"sideboard\" -> hand ")))
        .expect("the companion goes to the hand");
    assert!(special < put);
    assert!(
        statements[put].ends_with(&format!(" by #{id}")),
        "{}",
        statements[put]
    );
    // The standard's form (decision 0025, examples/more-situations.md): the
    // special action, the mana abilities and the payment of {3} by the
    // companion's cost, then the move. The card is already public, so no
    // second reveal.
    let between = &statements[special + 1..put];
    assert!(
        !between.iter().any(|s| s.contains(" reveal ")),
        "{between:?}"
    );
    assert_eq!(
        between.last().map(String::as_str),
        Some(format!("\"Player\" pay {{W}}{{C}}{{C}} by cost:#{id}").as_str()),
        "{between:?}"
    );
    assert!(
        between
            .iter()
            .any(|s| s.starts_with("\"Player\" add {C} by #"))
    );
    assert!(
        between
            .iter()
            .any(|s| s.starts_with("\"Player\" add {W} by #"))
    );
}

/// Spec 9.8: a move Arena reports between zones the record has no keyword
/// for is not dropped silently: the record says it is incomplete.
#[test]
fn unmapped_zone_transfer_writes_incomplete() {
    let mut input = fixture_match("bo3-loss-and-match-concede", 0, "Opponent");
    for game in &mut input.games {
        for message in &mut game.messages {
            message.json = message
                .json
                .replace("ZoneType_Sideboard", "ZoneType_Suppressed");
        }
    }
    let record = match_record(&input, &fixture_cards()).expect("card data is complete");
    let statements = statements_by_game(&record).remove(0);
    let incomplete: Vec<&String> = statements
        .iter()
        .filter(|s| s.starts_with("incomplete \""))
        .collect();
    assert!(!incomplete.is_empty());
    assert!(
        incomplete
            .iter()
            .any(|s| s.contains("ZoneType_Suppressed") && s.contains("ZoneType_Hand")),
        "{incomplete:?}"
    );
    // Only the deck block names the companion: it was never shown outside
    // the game, and its move was not invented.
    assert!(
        !statements.iter().any(|s| s.contains("Kaheera")
            && !s.starts_with("side ")
            && !s.contains(" hand \"Player\"")),
        "{:?}",
        statements
            .iter()
            .filter(|s| s.contains("Kaheera"))
            .collect::<Vec<_>>()
    );
}

/// Spec 4.7 and decision 0043, from the plan's evidence ledger: Arena never
/// reports cost changes (`Omits`); it reports the library family (no
/// searches) and lasting effects (no text, duration or delayed triggers)
/// only in part (`Partial`). Mana, priority and damage are recorded in
/// full. The format is baselined at 0.3.0 (spec 4.2).
#[test]
fn partial_tag_lists_library_and_effects() {
    let loss = fixture_match("bo3-loss-and-match-concede", 0, "Opponent");
    let mut games = 0;
    for input in [three_games(), loss, match_concede()] {
        let record = match_record(&input, &fixture_cards()).expect("card data is complete");
        for game in &record.games {
            assert_eq!(tag(game, "Mtgr"), Some("0.3.0"));
            assert_eq!(tag(game, "Omits"), Some("costs"));
            assert_eq!(tag(game, "Partial"), Some("library effects"));
            games += 1;
        }
    }
    assert_eq!(games, 7);
}

/// The mana symbols of a mana line (`"P" add {R}{G} ...`).
fn mana_count(statement: &str, verb: &str) -> usize {
    let Some((_, rest)) = statement.split_once(&format!("\" {verb} ")) else {
        return 0;
    };
    let symbols = rest.split(' ').next().unwrap_or("");
    symbols.matches('{').count()
}

/// Spec 9.4, decision 0006: each mana ability adds the mana Arena reports it
/// paid (`ManaPaid`), and the spell's payment follows its mana abilities,
/// `by cost:` the spell. Every mana added is spent or lost.
#[test]
fn mana_abilities_add_what_each_payment_spends() {
    let record = match_record(&three_games(), &fixture_cards()).expect("card data is complete");
    let statements = statements_by_game(&record).remove(0);
    // Turn 3: the opponent taps a Mountain and a Forest for a spell.
    let cast = statements
        .iter()
        .position(|s| s.contains(" cast from hand"))
        .expect("a cast");
    let spell = id_at(&statements[cast]).expect("an id");
    let pay = statements
        .iter()
        .position(|s| s.contains(" pay "))
        .expect("a payment");
    assert!(cast < pay);
    assert_eq!(
        statements[pay],
        format!("\"Opponent\" pay {{R}}{{G}} by cost:#{spell}")
    );
    let adds: Vec<&String> = statements[cast..pay]
        .iter()
        .filter(|s| s.contains(" add "))
        .collect();
    assert_eq!(adds.len(), 2, "{:?}", &statements[cast..pay]);
    assert!(adds[0].starts_with("\"Opponent\" add {R} by #"));
    assert!(adds[1].starts_with("\"Opponent\" add {G} by #"));
    for add in adds {
        let ability = add.rsplit('#').next().expect("a cause");
        assert!(
            statements[cast..pay]
                .iter()
                .any(|s| s.starts_with(&format!("new ability #{ability} mana of #"))),
            "{add} is caused by its mana ability"
        );
    }

    let loss = fixture_match("bo3-loss-and-match-concede", 0, "Opponent");
    for input in [three_games(), loss] {
        let record = match_record(&input, &fixture_cards()).expect("card data is complete");
        for statements in statements_by_game(&record) {
            let added: usize = statements.iter().map(|s| mana_count(s, "add")).sum();
            let paid: usize = statements.iter().map(|s| mana_count(s, "pay")).sum();
            let lost: usize = statements.iter().map(|s| mana_count(s, "lose-mana")).sum();
            assert!(added > 0);
            assert_eq!(added, paid + lost);
        }
    }
}

/// Carried note 1: Nissa, Who Shakes the World's "whenever you tap a Forest
/// for mana" is a triggered mana ability (CR 605.1b): never on the stack,
/// it adds {G} as the Forest's ability does (loss log lines 10714 and
/// 23809).
#[test]
fn triggered_mana_abilities_add_mana() {
    let statements = loss_game_one();
    let nissa = statements
        .iter()
        .find(|s| {
            s.starts_with("new ability #")
                && s.contains(" mana of #")
                && s.contains("\"Nissa, Who Shakes the World\"")
        })
        .expect("Nissa's mana ability");
    assert!(!nissa.ends_with(" in stack"), "{nissa}");
    let id = id_at(nissa.trim_start_matches("new ability ")).expect("an id");
    assert!(
        statements
            .iter()
            .any(|s| *s == format!("\"Opponent\" add {{G}} restriction ? by #{id}")),
        "Nissa's {{G}}"
    );
}

/// Mana a resolving ability adds floats in the pool (Outcaster
/// Trailblazer's "add one mana of any color", loss log line 5874): it is
/// added by the ability before it ceases, and spent from the pool later.
#[test]
fn floating_mana_is_added_by_its_ability() {
    let statements = loss_game_one();
    let trigger = statements
        .iter()
        .find(|s| {
            s.starts_with("new ability #")
                && s.contains(" triggered of #")
                && s.contains("\"Outcaster Trailblazer\"")
        })
        .expect("Outcaster Trailblazer's trigger");
    let id = id_at(trigger.trim_start_matches("new ability ")).expect("an id");
    let add = statements
        .iter()
        .position(|s| *s == format!("\"Opponent\" add {{G}} restriction ? by #{id}"))
        .expect("the trigger adds {G}");
    let cease = statements
        .iter()
        .position(|s| *s == format!("#{id} cease by rule:resolve"))
        .expect("the trigger ceases");
    assert!(add < cease);
    let pay = statements[cease..]
        .iter()
        .find(|s| s.starts_with("\"Opponent\" pay "))
        .expect("a later payment");
    assert!(pay.contains("{G}"), "{pay}");
}

/// Spec 9.3 and 9.9: Arena shows who holds priority in every state, so
/// each player who receives priority while the stack is not empty, and
/// each pass, is written, with the stack under each `priority` line.
#[test]
fn priority_and_passes_around_the_stack() {
    let record = match_record(&three_games(), &fixture_cards()).expect("card data is complete");
    let statements = statements_by_game(&record).remove(0);
    let pay = statements
        .iter()
        .position(|s| s.contains(" pay "))
        .expect("a payment");
    let spell = statements[pay]
        .rsplit('#')
        .next()
        .expect("a cause")
        .to_string();
    let entry = statements
        .iter()
        .find(|s| s.starts_with(&format!("#{spell} \"")) && s.contains(" cast from "))
        .map(|s| s.split(" cast from ").next().expect("a name").to_string())
        .expect("the cast");
    let expected = vec![
        "\"Opponent\" priority".to_string(),
        "stack".to_string(),
        format!("{entry} controller \"Opponent\""),
        "\"Opponent\" pass".to_string(),
        "\"Player\" priority".to_string(),
        "stack".to_string(),
        format!("{entry} controller \"Opponent\""),
        "\"Player\" pass".to_string(),
        format!("{entry} resolve"),
    ];
    assert_eq!(statements[pay + 1..pay + 10].to_vec(), expected);

    // Every resolution follows a pass: the last priority line before it is
    // closed by a pass.
    let loss = fixture_match("bo3-loss-and-match-concede", 0, "Opponent");
    for input in [three_games(), loss] {
        let record = match_record(&input, &fixture_cards()).expect("card data is complete");
        for statements in statements_by_game(&record) {
            let mut resolves = 0;
            for (at, statement) in statements.iter().enumerate() {
                if !statement.ends_with(" resolve") {
                    continue;
                }
                resolves += 1;
                let last = statements[..at]
                    .iter()
                    .rev()
                    .find(|s| s.ends_with(" priority") || s.ends_with(" pass"))
                    .expect("priority was given");
                assert!(last.ends_with(" pass"), "{statement} after {last}");
            }
            assert!(resolves > 0);
        }
    }
}

/// Arena reports "put the rest on the bottom in a random order" as a
/// `Shuffle` of only those cards (three-games log lines 12186, 23598, 23618
/// and 28539): that is not a shuffle of the library (CR 701.24a).
#[test]
fn partial_library_reorders_are_not_shuffles() {
    let record = match_record(&three_games(), &fixture_cards()).expect("card data is complete");
    for game in record.games {
        let text = ::mtgr::write(&::mtgr::Record { games: vec![game] });
        for line in text.lines().filter(|line| line.contains(" shuffle ")) {
            assert!(line.ends_with(" shuffle by rule:mulligan"), "{line}");
        }
    }
}

/// A synthetic state in turn 1's main phase where `seat` plays the Forest
/// `card` from its hand zone `hand` (Arena's `PlayLand`), leaving the
/// `battlefield` ids.
fn synthetic_play(state: u32, card: u32, hand: u32, seat: u32, battlefield: &str) -> String {
    format!(
        r#"{{"type":"GREMessageType_GameStateMessage","systemSeatIds":[1],
      "gameStateMessage":{{"type":"GameStateType_Diff","gameStateId":{state},"prevGameStateId":{prev},
        "turnInfo":{{"turnNumber":1,"activePlayer":1,"phase":"Phase_Main1","priorityPlayer":{seat}}},
        "zones":[
          {{"zoneId":28,"type":"ZoneType_Battlefield","objectInstanceIds":[{battlefield}]}},
          {{"zoneId":{hand},"type":"ZoneType_Hand","ownerSeatId":{seat},"objectInstanceIds":[]}}],
        "gameObjects":[
          {{"instanceId":{card},"grpId":95190,"type":"GameObjectType_Card","zoneId":28,"visibility":"Visibility_Public","ownerSeatId":{seat},"controllerSeatId":{seat},"cardTypes":["CardType_Land"]}}],
        "annotations":[
          {{"id":{state}01,"affectorId":{seat},"affectedIds":[{card}],"type":["AnnotationType_ZoneTransfer"],
            "details":[{{"key":"zone_src","valueInt32":[{hand}]}},{{"key":"zone_dest","valueInt32":[28]}},{{"key":"category","valueString":["PlayLand"]}}]}}]}}}}"#,
        prev = state - 1
    )
}

/// One GRE state message for a synthetic game.
fn synthetic_state(json: &str) -> crate::GameInput {
    crate::GameInput {
        source: crate::Source::Gre,
        logged_at: 1_000,
        json: json.to_string(),
    }
}

/// CR 106.4: mana a Forest adds that is not spent is lost as the main
/// phase ends, before the beginning of combat (`rule:empty-pool`).
#[test]
fn unspent_mana_is_lost_as_the_step_ends() {
    let full = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[1],
      "gameStateMessage":{"type":"GameStateType_Full","gameStateId":1,
        "players":[{"systemSeatNumber":1,"lifeTotal":20},{"systemSeatNumber":2,"lifeTotal":20}],
        "turnInfo":{"turnNumber":1,"activePlayer":1,"phase":"Phase_Main1","priorityPlayer":1},
        "zones":[
          {"zoneId":28,"type":"ZoneType_Battlefield","objectInstanceIds":[300]},
          {"zoneId":32,"type":"ZoneType_Library","ownerSeatId":1,"objectInstanceIds":[161]}],
        "gameObjects":[
          {"instanceId":300,"grpId":95190,"type":"GameObjectType_Card","zoneId":28,"visibility":"Visibility_Public","ownerSeatId":1,"controllerSeatId":1,"cardTypes":["CardType_Land"]}]}}"#;
    let tap = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[1],
      "gameStateMessage":{"type":"GameStateType_Diff","gameStateId":2,
        "players":[{"systemSeatNumber":1,"lifeTotal":20,
          "manaPool":[{"manaId":7,"color":"ManaColor_Green","srcInstanceId":300,"abilityGrpId":1005,"count":1}]}],
        "turnInfo":{"turnNumber":1,"activePlayer":1,"phase":"Phase_Main1","priorityPlayer":1},
        "gameObjects":[
          {"instanceId":300,"grpId":95190,"type":"GameObjectType_Card","zoneId":28,"visibility":"Visibility_Public","ownerSeatId":1,"controllerSeatId":1,"cardTypes":["CardType_Land"],"isTapped":true}],
        "annotations":[
          {"id":1,"affectorId":300,"affectedIds":[400],"type":["AnnotationType_AbilityInstanceCreated"]},
          {"id":2,"affectorId":400,"affectedIds":[300],"type":["AnnotationType_TappedUntappedPermanent"],
            "details":[{"key":"tapped","valueInt32":[1]}]},
          {"id":3,"affectorId":1,"affectedIds":[400],"type":["AnnotationType_UserActionTaken"],
            "details":[{"key":"actionType","valueInt32":[4]},{"key":"abilityGrpId","valueInt32":[1005]}]}]}}"#;
    let combat = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[1],
      "gameStateMessage":{"type":"GameStateType_Diff","gameStateId":3,
        "players":[{"systemSeatNumber":1,"lifeTotal":20}],
        "turnInfo":{"turnNumber":1,"activePlayer":1,"phase":"Phase_Combat","step":"Step_BeginCombat","priorityPlayer":1},
        "annotations":[
          {"id":4,"affectedIds":[1],"type":["AnnotationType_PhaseOrStepModified"],
            "details":[{"key":"phase","valueInt32":[3]},{"key":"step","valueInt32":[0]}]},
          {"id":5,"affectorId":300,"affectedIds":[400],"type":["AnnotationType_AbilityInstanceDeleted"]}]}}"#;
    let input = MatchInput {
        match_token: "synthetic".to_string(),
        event: String::new(),
        format: String::new(),
        local_name: "Player".to_string(),
        opponent_name: "Opponent".to_string(),
        local_archetype: None,
        opponent_archetype: None,
        started_at: 0,
        games: vec![GameMessages {
            number: 1,
            messages: vec![
                synthetic_state(full),
                synthetic_state(tap),
                synthetic_state(combat),
            ],
        }],
    };
    let record = match_record(&input, &fixture_cards()).expect("card data is complete");
    let statements = statements_by_game(&record).remove(0);
    let add = statements
        .iter()
        .position(|s| s.starts_with("\"Player\" add {G} by "))
        .unwrap_or_else(|| panic!("the Forest's {{G}}: {statements:?}"));
    assert_eq!(
        statements[add + 1..add + 3].to_vec(),
        vec![
            "\"Player\" lose-mana {G} by rule:empty-pool".to_string(),
            "beginning-of-combat".to_string(),
        ]
    );
}

/// Decision 0025: plot is a special action (CR 116.2k). Arena reports it as
/// `UserActionTaken` action 7 on an ability of the card, pays the mana into
/// that ability, and moves the card from hand to exile with the plotted
/// designation (loss log line 4436). The record writes `special plot`, the
/// mana, the payment by the card's cost, then the move (where the hidden
/// card is named).
#[test]
fn plot_is_a_special_action_paid_by_the_card() {
    let statements = loss_game_one();
    let special = statements
        .iter()
        .position(|s| s.ends_with(" special plot"))
        .expect("the plot");
    let id = id_at(&statements[special]).expect("an id");
    assert_eq!(statements[special], format!("#{id} special plot"));
    let put = statements
        .iter()
        .position(|s| {
            s.starts_with(&format!(
                "#{id} \"Outcaster Trailblazer\" move hand -> exile "
            ))
        })
        .expect("the card is exiled");
    assert_eq!(
        statements[special + 1..put].last().map(String::as_str),
        Some(format!("#{id} identify \"Outcaster Trailblazer\"").as_str())
    );
    assert_eq!(
        statements[put - 2],
        format!("\"Opponent\" pay {{G}}{{G}}{{G}} by cost:#{id}")
    );
    assert!(statements[put].ends_with(&format!(" by #{id}")));
    let adds = statements[special..put]
        .iter()
        .filter(|s| s.starts_with("\"Opponent\" add {G} by #"))
        .count();
    assert_eq!(adds, 3);
    // Every plot in the loss match (log lines 4436, 14048, 14778), and no
    // payment left `by ?`.
    let input = fixture_match("bo3-loss-and-match-concede", 0, "Opponent");
    let record = match_record(&input, &fixture_cards()).expect("card data is complete");
    let games = statements_by_game(&record);
    let plots: usize = games
        .iter()
        .map(|game| game.iter().filter(|s| s.ends_with(" special plot")).count())
        .sum();
    assert_eq!(plots, 3);
    assert!(
        !games
            .iter()
            .flatten()
            .any(|s| s.contains(" pay ") && s.ends_with(" by ?"))
    );
}

/// What a special action's ability pays for is forgotten once the ability
/// leaves the stack: after the loss match's first game, with its plot, the
/// writer holds no such entry.
#[test]
fn plot_payments_are_forgotten_as_the_ability_ends() {
    let input = fixture_match("bo3-loss-and-match-concede", 0, "Opponent");
    let cards = fixture_cards();
    let game = &input.games[0];
    let fold = crate::fold_game(&game.messages);
    let roster = super::Roster {
        local_seat: fold.local_seat.unwrap_or(1),
        local_name: input.local_name.clone(),
        opponent_name: input.opponent_name.clone(),
    };
    let mut writer = super::GameWriter::new(&fold, &cards, &roster, game);
    writer.write_body();
    assert!(
        writer
            .lines
            .iter()
            .any(|line| matches!(line.statement, ::mtgr::Statement::Special { .. }))
    );
    assert!(writer.paid_for.is_empty(), "{:?}", writer.paid_for);
}

/// CR 117.4: a resolution needs both players to pass with the stack as it
/// is. When the states never show the other player holding priority (here
/// three-games game 1, state 47, edited to leave priority with the caster),
/// the record says it is incomplete instead of implying their pass.
#[test]
fn resolution_without_both_passes_is_incomplete() {
    let mut input = three_games();
    let mut edited = 0;
    for message in &mut input.games[0].messages {
        let json = message.json.replace(": ", ":");
        if json.contains("\"gameStateId\":47,") {
            message.json = json.replace("\"priorityPlayer\":1", "\"priorityPlayer\":2");
            edited += 1;
        }
    }
    assert!(edited > 0);
    let record = match_record(&input, &fixture_cards()).expect("card data is complete");
    let statements = statements_by_game(&record).remove(0);
    let resolve = statements
        .iter()
        .position(|s| s.ends_with("\"Torgal, A Fine Hound\" resolve"))
        .expect("Torgal resolves");
    assert_eq!(
        statements[resolve - 2..resolve].to_vec(),
        vec![
            "\"Opponent\" pass".to_string(),
            "incomplete \"Arena resolved the top of the stack without showing both players pass\""
                .to_string(),
        ]
    );
}

/// Only a resolution the record writes needs both passes: an object the
/// record does not show on the stack resolving (here one injected into
/// three-games game 1, state 48, just before Torgal resolves) writes no
/// pass and no `incomplete`, and leaves Torgal's resolution as it was.
#[test]
fn an_object_off_the_record_stack_resolves_without_passes() {
    let plain = match_record(&three_games(), &fixture_cards()).expect("card data is complete");
    let mut input = three_games();
    let mut edited = 0;
    for message in &mut input.games[0].messages {
        let json = message.json.replace(": ", ":");
        if json.contains("\"gameStateId\":48,") {
            message.json = json.replacen(
                "\"annotations\":[",
                r#""annotations":[
                  {"id":999901,"affectorId":999999,"affectedIds":[999999],"type":["AnnotationType_ResolutionStart"]},
                  {"id":999902,"affectorId":999999,"affectedIds":[999999],"type":["AnnotationType_ResolutionComplete"]},"#,
                1,
            );
            edited += 1;
        }
    }
    assert_eq!(edited, 1);
    let record = match_record(&input, &fixture_cards()).expect("card data is complete");
    assert_eq!(
        statements_by_game(&record).remove(0),
        statements_by_game(&plain).remove(0)
    );
}

/// A synthetic one-game match from GRE state messages.
fn synthetic_match(states: &[&str]) -> MatchInput {
    MatchInput {
        match_token: "synthetic".to_string(),
        event: String::new(),
        format: String::new(),
        local_name: "Player".to_string(),
        opponent_name: "Opponent".to_string(),
        local_archetype: None,
        opponent_archetype: None,
        started_at: 0,
        games: vec![GameMessages {
            number: 1,
            messages: states.iter().map(|json| synthetic_state(json)).collect(),
        }],
    }
}

/// A card whose card data has face names.
fn with_faces(name: &str) -> CardData {
    CardData {
        name: name.to_string(),
        faces: name.split(" // ").map(str::to_string).collect(),
    }
}

/// Review Focus 2, decision 0022: the opponent's Pathway is shown by its
/// back face's grpId (73477), which has no card data. It is named by its
/// front card (73476) and played and listed with `face`.
#[test]
fn back_face_named_by_front_card() {
    let cards = fixture_cards();
    assert!(cards.contains_key(&73476) && !cards.contains_key(&73477));
    let record = match_record(&three_games(), &cards).expect("card data is complete");
    let statements = statements_by_game(&record).remove(0);
    let name = "\"Fixture Pathway 73476 // Fixture Pathway 73477\"";
    assert!(
        statements.iter().any(|s| s.ends_with(&format!(
            "{name} play from hand face \"Fixture Pathway 73477\" types land"
        ))),
        "the back face is played"
    );
    assert!(
        statements.iter().any(|s| s.contains(&format!(
            "{name} battlefield \"Opponent\" face \"Fixture Pathway 73477\" types land"
        ))),
        "keyframes show the back face"
    );
}

/// A double-faced card (Arena shows it with `othersideGrpId`) is listed by
/// its front grpId, so a reader knows each face has its own picture.
#[test]
fn double_faced_cards_are_listed_by_front() {
    let written =
        super::write_match(&three_games(), &fixture_cards()).expect("card data is complete");
    assert!(
        written.double_faced.contains(&73476),
        "{:?}",
        written.double_faced
    );
    assert!(
        !written.double_faced.contains(&73477),
        "the back face is not listed"
    );
}

/// The opponent casts the second half of a split card from hand. Arena
/// shows the card by the half's grpId (74015) with its halves as
/// `GameObjectType_SplitLeft` (74014) and `GameObjectType_SplitRight`
/// (74015) objects; the card (74013) is numbered just before its first
/// half. The record names the card and writes the half (decision 0022).
#[test]
fn split_half_on_stack_named_by_its_card() {
    let full = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[1],
      "gameStateMessage":{"type":"GameStateType_Full","gameStateId":1,
        "players":[{"systemSeatNumber":1,"lifeTotal":20},{"systemSeatNumber":2,"lifeTotal":20}],
        "turnInfo":{"turnNumber":1,"activePlayer":2,"phase":"Phase_Main1","priorityPlayer":2},
        "zones":[
          {"zoneId":27,"type":"ZoneType_Stack","objectInstanceIds":[]},
          {"zoneId":31,"type":"ZoneType_Hand","ownerSeatId":2,"objectInstanceIds":[200]}]}}"#;
    let cast = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[1],
      "gameStateMessage":{"type":"GameStateType_Diff","gameStateId":2,"prevGameStateId":1,
        "turnInfo":{"turnNumber":1,"activePlayer":2,"phase":"Phase_Main1","priorityPlayer":2},
        "zones":[
          {"zoneId":27,"type":"ZoneType_Stack","objectInstanceIds":[210]},
          {"zoneId":31,"type":"ZoneType_Hand","ownerSeatId":2,"objectInstanceIds":[]}],
        "gameObjects":[
          {"instanceId":210,"grpId":74015,"type":"GameObjectType_Card","zoneId":27,"visibility":"Visibility_Public","ownerSeatId":2,"controllerSeatId":2,"cardTypes":["CardType_Sorcery"]},
          {"instanceId":211,"grpId":74014,"type":"GameObjectType_SplitLeft","zoneId":27,"visibility":"Visibility_Public","ownerSeatId":2,"controllerSeatId":2,"cardTypes":["CardType_Sorcery"],"parentId":210},
          {"instanceId":212,"grpId":74015,"type":"GameObjectType_SplitRight","zoneId":27,"visibility":"Visibility_Public","ownerSeatId":2,"controllerSeatId":2,"cardTypes":["CardType_Sorcery"],"parentId":210}],
        "annotations":[
          {"id":1,"affectedIds":[200],"type":["AnnotationType_ObjectIdChanged"],
            "details":[{"key":"orig_id","valueInt32":[200]},{"key":"new_id","valueInt32":[210]}]},
          {"id":2,"affectedIds":[210],"type":["AnnotationType_ZoneTransfer"],
            "details":[{"key":"zone_src","valueInt32":[31]},{"key":"zone_dest","valueInt32":[27]},{"key":"category","valueString":["CastSpell"]}]},
          {"id":3,"affectorId":2,"affectedIds":[210],"type":["AnnotationType_UserActionTaken"],
            "details":[{"key":"actionType","valueInt32":[1]},{"key":"abilityGrpId","valueInt32":[0]}]}]}}"#;
    let mut cards = fixture_cards();
    cards.insert(74013, with_faces("Claim // Fame"));
    let written = super::write_match(&synthetic_match(&[full, cast]), &cards)
        .expect("the halves need no card data of their own");
    assert!(
        written.double_faced.is_empty(),
        "a split card's halves share one picture"
    );
    let statements = statements_by_game(&written.record).remove(0);
    let cast_at = statements
        .iter()
        .position(|s| s.ends_with(" cast from hand face \"Fame\""))
        .unwrap_or_else(|| panic!("the half is cast: {statements:?}"));
    let id = id_at(&statements[cast_at]).expect("an id");
    assert_eq!(
        statements[cast_at],
        format!("#{id} \"Claim // Fame\" cast from hand face \"Fame\"")
    );
    assert_eq!(
        statements[cast_at - 1],
        format!("#{id} identify \"Claim // Fame\"")
    );
    // The source id is the card's grpId, not the half's it was shown by.
    let raw = raw_statements_by_game(&written.record).remove(0);
    assert!(
        raw.contains(&format!("#{id} identify \"Claim // Fame\" src arena:74013")),
        "{raw:?}"
    );
    assert!(
        statements.contains(&format!(
            "#{id} \"Claim // Fame\" face \"Fame\" controller \"Opponent\""
        )),
        "the stack shows the half: {statements:?}"
    );
}

/// A door unlocked on a Room the record does not track (here an id no
/// state shows) is still a door the standard cannot write: `incomplete`,
/// with no id to name.
#[test]
fn door_unlocked_on_an_untracked_room_is_incomplete() {
    let full = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[1],
      "gameStateMessage":{"type":"GameStateType_Full","gameStateId":1,
        "players":[{"systemSeatNumber":1,"lifeTotal":20},{"systemSeatNumber":2,"lifeTotal":20}],
        "turnInfo":{"turnNumber":1,"activePlayer":2,"phase":"Phase_Main1","priorityPlayer":2}}}"#;
    let unlocked = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[1],
      "gameStateMessage":{"type":"GameStateType_Diff","gameStateId":2,"prevGameStateId":1,
        "turnInfo":{"turnNumber":1,"activePlayer":2,"phase":"Phase_Main1","priorityPlayer":2},
        "annotations":[
          {"id":1,"affectorId":2,"affectedIds":[999],"type":["AnnotationType_GainDesignation"],
            "details":[{"key":"DesignationType","valueInt32":[19]}]}]}}"#;
    let record = match_record(&synthetic_match(&[full, unlocked]), &fixture_cards())
        .expect("card data is complete");
    let statements = statements_by_game(&record).remove(0);
    assert!(
        statements.contains(
            &"incomplete \"a door of a Room was unlocked; the standard does not cover Rooms' doors yet\""
                .to_string()
        ),
        "{statements:?}"
    );
}

/// Arena's Room: cast as a door (the card shows the door's grpId, its doors
/// are `GameObjectType_RoomLeft` and `GameObjectType_RoomRight`), it enters
/// as the card, and the door is unlocked (`GainDesignation` 19). The
/// standard does not cover unlocked doors yet (spec section 15), so the
/// record says it is incomplete there.
#[test]
fn room_door_is_cast_as_a_face_and_its_unlock_is_incomplete() {
    let full = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[1],
      "gameStateMessage":{"type":"GameStateType_Full","gameStateId":1,
        "players":[{"systemSeatNumber":1,"lifeTotal":20},{"systemSeatNumber":2,"lifeTotal":20}],
        "turnInfo":{"turnNumber":1,"activePlayer":2,"phase":"Phase_Main1","priorityPlayer":2},
        "zones":[
          {"zoneId":27,"type":"ZoneType_Stack","objectInstanceIds":[]},
          {"zoneId":28,"type":"ZoneType_Battlefield","objectInstanceIds":[]},
          {"zoneId":31,"type":"ZoneType_Hand","ownerSeatId":2,"objectInstanceIds":[200]}]}}"#;
    let cast = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[1],
      "gameStateMessage":{"type":"GameStateType_Diff","gameStateId":2,"prevGameStateId":1,
        "turnInfo":{"turnNumber":1,"activePlayer":2,"phase":"Phase_Main1","priorityPlayer":2},
        "zones":[
          {"zoneId":27,"type":"ZoneType_Stack","objectInstanceIds":[230]},
          {"zoneId":31,"type":"ZoneType_Hand","ownerSeatId":2,"objectInstanceIds":[]}],
        "gameObjects":[
          {"instanceId":230,"grpId":92197,"type":"GameObjectType_Card","zoneId":27,"visibility":"Visibility_Public","ownerSeatId":2,"controllerSeatId":2,"cardTypes":["CardType_Enchantment"],"subtypes":["SubType_Room"]},
          {"instanceId":231,"grpId":92197,"type":"GameObjectType_RoomLeft","zoneId":27,"visibility":"Visibility_Public","ownerSeatId":2,"controllerSeatId":2,"cardTypes":["CardType_Enchantment"],"parentId":230},
          {"instanceId":232,"grpId":92198,"type":"GameObjectType_RoomRight","zoneId":27,"visibility":"Visibility_Public","ownerSeatId":2,"controllerSeatId":2,"cardTypes":["CardType_Enchantment"],"parentId":230}],
        "annotations":[
          {"id":1,"affectedIds":[200],"type":["AnnotationType_ObjectIdChanged"],
            "details":[{"key":"orig_id","valueInt32":[200]},{"key":"new_id","valueInt32":[230]}]},
          {"id":2,"affectedIds":[230],"type":["AnnotationType_ZoneTransfer"],
            "details":[{"key":"zone_src","valueInt32":[31]},{"key":"zone_dest","valueInt32":[27]},{"key":"category","valueString":["CastSpell"]}]}]}}"#;
    let resolve = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[1],
      "gameStateMessage":{"type":"GameStateType_Diff","gameStateId":3,"prevGameStateId":2,
        "turnInfo":{"turnNumber":1,"activePlayer":2,"phase":"Phase_Main1","priorityPlayer":2},
        "zones":[
          {"zoneId":27,"type":"ZoneType_Stack","objectInstanceIds":[]},
          {"zoneId":28,"type":"ZoneType_Battlefield","objectInstanceIds":[230]}],
        "gameObjects":[
          {"instanceId":230,"grpId":92196,"type":"GameObjectType_Card","zoneId":28,"visibility":"Visibility_Public","ownerSeatId":2,"controllerSeatId":2,"cardTypes":["CardType_Enchantment"],"subtypes":["SubType_Room"]},
          {"instanceId":231,"grpId":92197,"type":"GameObjectType_RoomLeft","zoneId":28,"visibility":"Visibility_Public","ownerSeatId":2,"controllerSeatId":2,"cardTypes":["CardType_Enchantment"],"parentId":230},
          {"instanceId":232,"grpId":92198,"type":"GameObjectType_RoomRight","zoneId":28,"visibility":"Visibility_Public","ownerSeatId":2,"controllerSeatId":2,"cardTypes":["CardType_Enchantment"],"parentId":230}],
        "annotations":[
          {"id":3,"affectorId":1,"affectedIds":[230],"type":["AnnotationType_ZoneTransfer"],
            "details":[{"key":"zone_src","valueInt32":[27]},{"key":"zone_dest","valueInt32":[28]},{"key":"category","valueString":["Resolve"]}]},
          {"id":4,"affectedIds":[230],"type":["AnnotationType_GainDesignation"],
            "details":[{"key":"DesignationType","valueInt32":[19]}]}]}}"#;
    let mut cards = fixture_cards();
    cards.insert(92196, with_faces("Fixture Door Left // Fixture Door Right"));
    let record = match_record(&synthetic_match(&[full, cast, resolve]), &cards)
        .expect("the doors need no card data of their own");
    let statements = statements_by_game(&record).remove(0);
    let name = "\"Fixture Door Left // Fixture Door Right\"";
    let cast_at = statements
        .iter()
        .position(|s| s.ends_with(&format!("{name} cast from hand face \"Fixture Door Left\"")))
        .unwrap_or_else(|| panic!("the door is cast: {statements:?}"));
    let id = id_at(&statements[cast_at]).expect("an id");
    let enters = statements
        .iter()
        .position(|s| {
            *s == format!(
                "#{id} {name} move stack -> battlefield types enchantment by rule:resolve"
            )
        })
        .unwrap_or_else(|| panic!("the Room enters as the card: {statements:?}"));
    assert!(
        statements[enters + 1].starts_with("incomplete \""),
        "{statements:?}"
    );
}

/// A double-faced card the card data cannot name is `?` by its front
/// face's grpId: Tovolar (78613) shows its back face (78614) too, which is
/// never its source id, and no face is written for it.
#[test]
fn unknown_double_faced_card_uses_its_front_face() {
    let mut cards = fixture_cards();
    cards.remove(&78613);
    let record = match_record(&three_games(), &cards).expect("unknown names are written");
    let text = ::mtgr::write(&record);
    assert!(
        text.contains(" ? owner \"Opponent\" in ") || text.contains(" identify ? src arena:78613")
    );
    assert!(text.contains("src arena:78613"));
    assert!(!text.contains("arena:78614"));
    assert!(!text.contains("Tovolar"));
}

/// `input` with game `number`'s GRE state messages `first..=last` removed
/// (a hole in the stored messages). Returns how many were removed.
fn drop_states(input: &mut MatchInput, number: u32, first: u32, last: u32) -> usize {
    let game = input
        .games
        .iter_mut()
        .find(|game| game.number == number)
        .expect("the game");
    let before = game.messages.len();
    game.messages.retain(|message| {
        if message.source != crate::Source::Gre {
            return true;
        }
        let value: serde_json::Value = serde_json::from_str(&message.json).unwrap_or_default();
        let id = value["gameStateMessage"]["gameStateId"].as_u64();
        !id.is_some_and(|id| id >= u64::from(first) && id <= u64::from(last))
    });
    before - game.messages.len()
}

/// Spec 4.3, standard 9.8 and 14.4: three-games game 1 without states 123
/// to 130 (turn 6). State 131 follows state 130, which is missing: the
/// record says it is incomplete there and writes nothing until the next
/// full state (135), then carries on, to the game's own end.
#[test]
fn gap_marks_incomplete_and_resumes_at_full_state() {
    let mut input = three_games();
    assert_eq!(drop_states(&mut input, 1, 123, 130), 8);
    let record = match_record(&input, &fixture_cards()).expect("card data is complete");
    let game = &record.games[0];
    assert_eq!(tag(game, "Result"), Some("0-1"));
    let statements = statements_by_game(&record).remove(0);
    let gaps: Vec<usize> = statements
        .iter()
        .enumerate()
        .filter(|(_, s)| s.starts_with("incomplete "))
        .map(|(at, _)| at)
        .collect();
    assert_eq!(gaps.len(), 1, "{statements:?}");
    assert_eq!(
        statements[gaps[0]],
        "incomplete \"Arena did not log game states 123 to 130 (message summarized)\""
    );
    // The record resumes: later turns, their keyframes and the concede.
    let rest = &statements[gaps[0]..];
    assert!(rest.iter().any(|s| s.starts_with("turn 7 ")));
    assert!(rest.iter().any(|s| s.ends_with(" concede")));
}

/// Controller ruling (supersedes spec 4.3's "the game ends at the gap"):
/// Arena summarizes a state message with more than 50 objects or
/// annotations and logs no JSON for it, so states go missing with no full
/// state after them. The record writes the gap and keeps folding the diffs
/// that follow, to the game's real end; the next keyframe resyncs.
#[test]
fn summarized_gap_continues_to_the_real_result() {
    // Game 3's last full state is 282.
    let mut input = three_games();
    assert_eq!(drop_states(&mut input, 3, 300, 302), 3);
    let record = match_record(&input, &fixture_cards()).expect("card data is complete");
    let game = &record.games[2];
    assert_eq!(tag(game, "Result"), Some("0-1"));
    assert_eq!(tag(game, "Termination"), Some("concession"));
    let statements = statements_by_game(&record).remove(2);
    let gap = statements
        .iter()
        .position(|s| {
            s == "incomplete \"Arena did not log game states 300 to 302 (message summarized)\""
        })
        .unwrap_or_else(|| panic!("the gap: {statements:?}"));
    let rest = &statements[gap + 1..];
    assert!(rest.iter().any(|s| s == "keyframe"), "a later keyframe");
    assert!(rest.iter().any(|s| s.ends_with(" concede")));
    assert!(
        !rest.iter().any(|s| s.contains(" without a move")),
        "ids changed in the gap are not reported as removed: {rest:?}"
    );
}

/// Carried note (a): in three-games game 3, two pumps on #92 end in the
/// cleanup step that ends turn 17, in the same message as the next turn
/// begins and day turns the permanents over. Arena never shows the value
/// in between, so the record writes `pt 7/5 -> ?` as the pumps end, a
/// keyframe with `pt ?`, then `day`, the transform and `pt ? -> 2/2`, and
/// no power and toughness change of a transforming permanent is `by ?`.
#[test]
fn pump_ending_as_day_and_night_turn_writes_an_unknown_pt() {
    let record = match_record(&three_games(), &fixture_cards()).expect("card data is complete");
    let statements = statements_by_game(&record).remove(2);
    let name = "#92 \"Outland Liberator // Frenzied Trapbreaker\"";
    let ends = statements
        .iter()
        .position(|s| *s == format!("{name} pt 7/5 -> ? by rule:cleanup"))
        .unwrap_or_else(|| panic!("the pumps end: {statements:?}"));
    let keyframe_entry = format!(
        "{name} battlefield \"Opponent\" face \"Frenzied Trapbreaker\" types creature pt ? tapped"
    );
    let listed = statements[ends..]
        .iter()
        .position(|s| *s == keyframe_entry)
        .expect("the keyframe shows the unknown value")
        + ends;
    let day = statements[listed..]
        .iter()
        .position(|s| s == "day by rule:day-night")
        .expect("day comes in the untap step")
        + listed;
    let transform = statements[day..]
        .iter()
        .position(|s| *s == format!("{name} transform by #92"))
        .expect("#92 turns over")
        + day;
    assert_eq!(
        statements[transform + 1],
        format!("{name} pt ? -> 2/2 by #92")
    );
    for game in statements_by_game(&record) {
        for statement in game {
            assert!(
                !(statement.contains(" pt ")
                    && statement.ends_with(" by ?")
                    && !statement.contains(" move ")),
                "{statement}"
            );
        }
    }
}

/// Carried note (b), spec 9.8: counters of a type the writer has no name
/// for are not dropped silently: the record says it is incomplete.
#[test]
fn unknown_counter_type_is_incomplete() {
    let mut input = fixture_match("bo3-loss-and-match-concede", 0, "Opponent");
    let mut edited = 0;
    for game in &mut input.games {
        for message in &mut game.messages {
            let mut value: serde_json::Value =
                serde_json::from_str(&message.json).unwrap_or_default();
            let mut changed = false;
            for list in ["annotations", "persistentAnnotations"] {
                let Some(annotations) = value
                    .get_mut("gameStateMessage")
                    .and_then(|state| state.get_mut(list))
                    .and_then(|list| list.as_array_mut())
                else {
                    continue;
                };
                for detail in annotations
                    .iter_mut()
                    .filter_map(|annotation| annotation.get_mut("details"))
                    .filter_map(|details| details.as_array_mut())
                    .flatten()
                {
                    if detail["key"] == "counter_type" && detail["valueInt32"][0] == 19 {
                        detail["valueInt32"][0] = 99.into();
                        changed = true;
                    }
                }
            }
            if changed {
                message.json = value.to_string();
                edited += 1;
            }
        }
    }
    assert!(edited > 0);
    let record = match_record(&input, &fixture_cards()).expect("card data is complete");
    let statements: Vec<String> = statements_by_game(&record).into_iter().flatten().collect();
    assert!(
        statements
            .iter()
            .any(|s| s == "incomplete \"Arena counter type 99 has no counter name here\""),
        "{statements:?}"
    );
    assert!(!statements.iter().any(|s| s.contains("\"charge\"")));
}

/// A card Arena removes from a public zone with no move (owner data: a
/// copy cast from exile that resolves to the graveyard and is deleted)
/// ends with `incomplete`: the record cannot say why it left.
#[test]
fn card_removed_without_a_move_is_incomplete() {
    let full = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[1],
      "gameStateMessage":{"type":"GameStateType_Full","gameStateId":1,
        "players":[{"systemSeatNumber":1,"lifeTotal":20},{"systemSeatNumber":2,"lifeTotal":20}],
        "turnInfo":{"turnNumber":1,"activePlayer":2,"phase":"Phase_Main1","priorityPlayer":2},
        "zones":[
          {"zoneId":27,"type":"ZoneType_Stack","objectInstanceIds":[]},
          {"zoneId":29,"type":"ZoneType_Exile","objectInstanceIds":[300]},
          {"zoneId":33,"type":"ZoneType_Graveyard","ownerSeatId":2,"objectInstanceIds":[]}],
        "gameObjects":[
          {"instanceId":300,"grpId":75350,"type":"GameObjectType_Card","zoneId":29,"visibility":"Visibility_Public","ownerSeatId":2,"controllerSeatId":2,"cardTypes":["CardType_Sorcery"]}]}}"#;
    let cast = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[1],
      "gameStateMessage":{"type":"GameStateType_Diff","gameStateId":2,"prevGameStateId":1,
        "turnInfo":{"turnNumber":1,"activePlayer":2,"phase":"Phase_Main1","priorityPlayer":2},
        "zones":[
          {"zoneId":27,"type":"ZoneType_Stack","objectInstanceIds":[301]},
          {"zoneId":29,"type":"ZoneType_Exile","objectInstanceIds":[]}],
        "gameObjects":[
          {"instanceId":301,"grpId":75350,"type":"GameObjectType_Card","zoneId":27,"visibility":"Visibility_Public","ownerSeatId":2,"controllerSeatId":2,"cardTypes":["CardType_Sorcery"]}],
        "annotations":[
          {"id":1,"affectedIds":[300],"type":["AnnotationType_ObjectIdChanged"],
            "details":[{"key":"orig_id","valueInt32":[300]},{"key":"new_id","valueInt32":[301]}]},
          {"id":2,"affectedIds":[301],"type":["AnnotationType_ZoneTransfer"],
            "details":[{"key":"zone_src","valueInt32":[29]},{"key":"zone_dest","valueInt32":[27]},{"key":"category","valueString":["CastSpell"]}]}]}}"#;
    let resolve = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[1],
      "gameStateMessage":{"type":"GameStateType_Diff","gameStateId":3,"prevGameStateId":2,
        "turnInfo":{"turnNumber":1,"activePlayer":2,"phase":"Phase_Main1","priorityPlayer":2},
        "zones":[
          {"zoneId":27,"type":"ZoneType_Stack","objectInstanceIds":[]},
          {"zoneId":33,"type":"ZoneType_Graveyard","ownerSeatId":2,"objectInstanceIds":[]}],
        "diffDeletedInstanceIds":[302],
        "annotations":[
          {"id":3,"affectedIds":[301],"type":["AnnotationType_ObjectIdChanged"],
            "details":[{"key":"orig_id","valueInt32":[301]},{"key":"new_id","valueInt32":[302]}]},
          {"id":4,"affectorId":1,"affectedIds":[302],"type":["AnnotationType_ZoneTransfer"],
            "details":[{"key":"zone_src","valueInt32":[27]},{"key":"zone_dest","valueInt32":[33]},{"key":"category","valueString":["Resolve"]}]}]}}"#;
    let record = match_record(&synthetic_match(&[full, cast, resolve]), &fixture_cards())
        .expect("card data is complete");
    let statements = statements_by_game(&record).remove(0);
    let moved = statements
        .iter()
        .position(|s| s.contains(" move stack -> graveyard "))
        .unwrap_or_else(|| panic!("the card resolves: {statements:?}"));
    let id = id_at(&statements[moved]).expect("an id");
    assert_eq!(
        statements[moved + 1],
        format!("incomplete \"Arena removed #{id} from the graveyard without a move\"")
    );
    assert!(!statements[moved + 2..].iter().any(|s| mentions(s, id)));
}

/// Item 1 of the review: the split card's own grpId comes from evidence (the
/// same card shown whole in an earlier state), not from numbering: here
/// the halves are not numbered after the card, and the grpId before the
/// left half belongs to another card.
#[test]
fn split_card_named_from_evidence_not_numbering() {
    let full = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[1],
      "gameStateMessage":{"type":"GameStateType_Full","gameStateId":1,
        "players":[{"systemSeatNumber":1,"lifeTotal":20},{"systemSeatNumber":2,"lifeTotal":20}],
        "turnInfo":{"turnNumber":1,"activePlayer":2,"phase":"Phase_Main1","priorityPlayer":2},
        "zones":[
          {"zoneId":27,"type":"ZoneType_Stack","objectInstanceIds":[]},
          {"zoneId":33,"type":"ZoneType_Graveyard","ownerSeatId":2,"objectInstanceIds":[200]}],
        "gameObjects":[
          {"instanceId":200,"grpId":90000,"type":"GameObjectType_Card","zoneId":33,"visibility":"Visibility_Public","ownerSeatId":2,"controllerSeatId":2,"cardTypes":["CardType_Sorcery"]},
          {"instanceId":201,"grpId":90005,"type":"GameObjectType_SplitLeft","zoneId":33,"visibility":"Visibility_Public","ownerSeatId":2,"controllerSeatId":2,"parentId":200},
          {"instanceId":202,"grpId":90006,"type":"GameObjectType_SplitRight","zoneId":33,"visibility":"Visibility_Public","ownerSeatId":2,"controllerSeatId":2,"parentId":200}]}}"#;
    let cast = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[1],
      "gameStateMessage":{"type":"GameStateType_Diff","gameStateId":2,"prevGameStateId":1,
        "turnInfo":{"turnNumber":1,"activePlayer":2,"phase":"Phase_Main1","priorityPlayer":2},
        "zones":[
          {"zoneId":27,"type":"ZoneType_Stack","objectInstanceIds":[210]},
          {"zoneId":33,"type":"ZoneType_Graveyard","ownerSeatId":2,"objectInstanceIds":[]}],
        "diffDeletedInstanceIds":[201,202],
        "gameObjects":[
          {"instanceId":210,"grpId":90006,"type":"GameObjectType_Card","zoneId":27,"visibility":"Visibility_Public","ownerSeatId":2,"controllerSeatId":2,"cardTypes":["CardType_Sorcery"]},
          {"instanceId":211,"grpId":90005,"type":"GameObjectType_SplitLeft","zoneId":27,"visibility":"Visibility_Public","ownerSeatId":2,"controllerSeatId":2,"parentId":210},
          {"instanceId":212,"grpId":90006,"type":"GameObjectType_SplitRight","zoneId":27,"visibility":"Visibility_Public","ownerSeatId":2,"controllerSeatId":2,"parentId":210}],
        "annotations":[
          {"id":1,"affectedIds":[200],"type":["AnnotationType_ObjectIdChanged"],
            "details":[{"key":"orig_id","valueInt32":[200]},{"key":"new_id","valueInt32":[210]}]},
          {"id":2,"affectedIds":[210],"type":["AnnotationType_ZoneTransfer"],
            "details":[{"key":"zone_src","valueInt32":[33]},{"key":"zone_dest","valueInt32":[27]},{"key":"category","valueString":["CastSpell"]}]}]}}"#;
    let mut cards = fixture_cards();
    cards.insert(90000, with_faces("Claim // Fame"));
    cards.insert(90004, with_faces("Fixture Neighbour // Fixture Other"));
    let record = match_record(&synthetic_match(&[full, cast]), &cards).expect("named by evidence");
    let statements = statements_by_game(&record).remove(0);
    assert!(
        statements
            .iter()
            .any(|s| s.ends_with("\"Claim // Fame\" cast from graveyard face \"Fame\"")),
        "{statements:?}"
    );
    assert!(!statements.iter().any(|s| s.contains("Fixture Neighbour")));
}

/// Item 1 of the review: with no evidence, the grpId before the left half
/// is used only when it is a two-faced card; an unrelated card there is not
/// used, and the card is `?` by the grpId Arena showed (decision 0048).
#[test]
fn split_card_not_named_by_an_unrelated_neighbour() {
    let full = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[1],
      "gameStateMessage":{"type":"GameStateType_Full","gameStateId":1,
        "players":[{"systemSeatNumber":1,"lifeTotal":20},{"systemSeatNumber":2,"lifeTotal":20}],
        "turnInfo":{"turnNumber":1,"activePlayer":2,"phase":"Phase_Main1","priorityPlayer":2},
        "zones":[
          {"zoneId":27,"type":"ZoneType_Stack","objectInstanceIds":[]},
          {"zoneId":31,"type":"ZoneType_Hand","ownerSeatId":2,"objectInstanceIds":[200]}]}}"#;
    let cast = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[1],
      "gameStateMessage":{"type":"GameStateType_Diff","gameStateId":2,"prevGameStateId":1,
        "turnInfo":{"turnNumber":1,"activePlayer":2,"phase":"Phase_Main1","priorityPlayer":2},
        "zones":[
          {"zoneId":27,"type":"ZoneType_Stack","objectInstanceIds":[210]},
          {"zoneId":31,"type":"ZoneType_Hand","ownerSeatId":2,"objectInstanceIds":[]}],
        "gameObjects":[
          {"instanceId":210,"grpId":74015,"type":"GameObjectType_Card","zoneId":27,"visibility":"Visibility_Public","ownerSeatId":2,"controllerSeatId":2,"cardTypes":["CardType_Sorcery"]},
          {"instanceId":211,"grpId":74014,"type":"GameObjectType_SplitLeft","zoneId":27,"visibility":"Visibility_Public","ownerSeatId":2,"controllerSeatId":2,"parentId":210},
          {"instanceId":212,"grpId":74015,"type":"GameObjectType_SplitRight","zoneId":27,"visibility":"Visibility_Public","ownerSeatId":2,"controllerSeatId":2,"parentId":210}],
        "annotations":[
          {"id":1,"affectedIds":[200],"type":["AnnotationType_ObjectIdChanged"],
            "details":[{"key":"orig_id","valueInt32":[200]},{"key":"new_id","valueInt32":[210]}]},
          {"id":2,"affectedIds":[210],"type":["AnnotationType_ZoneTransfer"],
            "details":[{"key":"zone_src","valueInt32":[31]},{"key":"zone_dest","valueInt32":[27]},{"key":"category","valueString":["CastSpell"]}]}]}}"#;
    let mut cards = fixture_cards();
    cards.insert(
        74013,
        CardData {
            name: "Fixture Unrelated".to_string(),
            faces: Vec::new(),
        },
    );
    let record =
        match_record(&synthetic_match(&[full, cast]), &cards).expect("unknown names are written");
    let text = ::mtgr::write(&record);
    assert!(text.contains(" identify ? src arena:74015"), "{text}");
    assert!(!text.contains("Fixture Unrelated"), "{text}");
}

/// Item 2 of the review (spec: `face` is always written on a cast of one
/// face): a double-faced card cast as its front face says so, on the cast
/// and in the stack snapshots.
#[test]
fn double_faced_front_cast_writes_its_face() {
    let record = match_record(&three_games(), &fixture_cards()).expect("card data is complete");
    let statements = statements_by_game(&record).remove(0);
    let name = "#36 \"Howlpack Piper // Wildsong Howler\"";
    assert!(statements.contains(&format!("{name} cast from hand face \"Howlpack Piper\"")));
    assert!(statements.contains(&format!(
        "{name} face \"Howlpack Piper\" controller \"Opponent\""
    )));
}

/// Item 5 of the review: a card whose face the record must name, with no
/// face names in its card data, is missing card data.
#[test]
fn face_with_no_name_is_missing_card_data() {
    let mut cards = fixture_cards();
    cards.insert(
        73476,
        CardData {
            name: "Fixture Pathway 73476".to_string(),
            faces: Vec::new(),
        },
    );
    let error = match_record(&three_games(), &cards).expect_err("the back face has no name");
    assert_eq!(error, super::MtgrError::MissingCardData(vec![73476]));
}

/// Item 6 of the review: the first day is credited to what was resolving
/// (Collected Company, which put two daybound creatures onto the
/// battlefield), not guessed from the permanents that entered.
#[test]
fn first_day_credits_the_resolving_object() {
    let record = match_record(&three_games(), &fixture_cards()).expect("card data is complete");
    let games = statements_by_game(&record);
    assert!(
        games[0].contains(&"day by #30".to_string()),
        "{:?}",
        games[0]
    );
    assert!(games[2].contains(&"day by #3".to_string()));
}

/// Item 4 of the review, decision 0023: Arena's `Copy` transfer (a copy of
/// a spell, `isCopy`, its parent the copied spell) is `new copy`; the copy
/// resolves to the graveyard and ceases there (CR 704.5e).
#[test]
fn spell_copies_are_new_copies() {
    let full = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[1],
      "gameStateMessage":{"type":"GameStateType_Full","gameStateId":1,
        "players":[{"systemSeatNumber":1,"lifeTotal":20},{"systemSeatNumber":2,"lifeTotal":20}],
        "turnInfo":{"turnNumber":1,"activePlayer":2,"phase":"Phase_Main1","priorityPlayer":2},
        "zones":[
          {"zoneId":27,"type":"ZoneType_Stack","objectInstanceIds":[]},
          {"zoneId":31,"type":"ZoneType_Hand","ownerSeatId":2,"objectInstanceIds":[200]},
          {"zoneId":33,"type":"ZoneType_Graveyard","ownerSeatId":2,"objectInstanceIds":[]}]}}"#;
    let cast = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[1],
      "gameStateMessage":{"type":"GameStateType_Diff","gameStateId":2,"prevGameStateId":1,
        "turnInfo":{"turnNumber":1,"activePlayer":2,"phase":"Phase_Main1","priorityPlayer":2},
        "zones":[
          {"zoneId":27,"type":"ZoneType_Stack","objectInstanceIds":[210]},
          {"zoneId":31,"type":"ZoneType_Hand","ownerSeatId":2,"objectInstanceIds":[]}],
        "gameObjects":[
          {"instanceId":210,"grpId":73905,"type":"GameObjectType_Card","zoneId":27,"visibility":"Visibility_Public","ownerSeatId":2,"controllerSeatId":2,"cardTypes":["CardType_Sorcery"]}],
        "annotations":[
          {"id":1,"affectedIds":[200],"type":["AnnotationType_ObjectIdChanged"],
            "details":[{"key":"orig_id","valueInt32":[200]},{"key":"new_id","valueInt32":[210]}]},
          {"id":2,"affectedIds":[210],"type":["AnnotationType_ZoneTransfer"],
            "details":[{"key":"zone_src","valueInt32":[31]},{"key":"zone_dest","valueInt32":[27]},{"key":"category","valueString":["CastSpell"]}]}]}}"#;
    let copy = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[1],
      "gameStateMessage":{"type":"GameStateType_Diff","gameStateId":3,"prevGameStateId":2,
        "turnInfo":{"turnNumber":1,"activePlayer":2,"phase":"Phase_Main1","priorityPlayer":2},
        "zones":[{"zoneId":27,"type":"ZoneType_Stack","objectInstanceIds":[220,210]}],
        "gameObjects":[
          {"instanceId":220,"grpId":73905,"type":"GameObjectType_Card","zoneId":27,"visibility":"Visibility_Public","ownerSeatId":2,"controllerSeatId":2,"cardTypes":["CardType_Sorcery"],"isCopy":true,"parentId":210}],
        "annotations":[
          {"id":3,"affectorId":210,"affectedIds":[220],"type":["AnnotationType_ZoneTransfer"],
            "details":[{"key":"zone_src","valueInt32":[27]},{"key":"zone_dest","valueInt32":[27]},{"key":"category","valueString":["Copy"]}]}]}}"#;
    let pass = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[1],
      "gameStateMessage":{"type":"GameStateType_Diff","gameStateId":4,"prevGameStateId":3,
        "turnInfo":{"turnNumber":1,"activePlayer":2,"phase":"Phase_Main1","priorityPlayer":1}}}"#;
    let resolve = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[1],
      "gameStateMessage":{"type":"GameStateType_Diff","gameStateId":5,"prevGameStateId":4,
        "turnInfo":{"turnNumber":1,"activePlayer":2,"phase":"Phase_Main1","priorityPlayer":2},
        "zones":[
          {"zoneId":27,"type":"ZoneType_Stack","objectInstanceIds":[210]},
          {"zoneId":33,"type":"ZoneType_Graveyard","ownerSeatId":2,"objectInstanceIds":[]}],
        "diffDeletedInstanceIds":[221],
        "annotations":[
          {"id":4,"affectorId":220,"affectedIds":[220],"type":["AnnotationType_ResolutionStart"],
            "details":[{"key":"grpid","valueInt32":[73905]}]},
          {"id":5,"affectorId":220,"affectedIds":[220],"type":["AnnotationType_ResolutionComplete"],
            "details":[{"key":"grpid","valueInt32":[73905]}]},
          {"id":6,"affectedIds":[220],"type":["AnnotationType_ObjectIdChanged"],
            "details":[{"key":"orig_id","valueInt32":[220]},{"key":"new_id","valueInt32":[221]}]},
          {"id":7,"affectorId":1,"affectedIds":[221],"type":["AnnotationType_ZoneTransfer"],
            "details":[{"key":"zone_src","valueInt32":[27]},{"key":"zone_dest","valueInt32":[33]},{"key":"category","valueString":["Resolve"]}]}]}}"#;
    let record = match_record(
        &synthetic_match(&[full, cast, copy, pass, resolve]),
        &fixture_cards(),
    )
    .expect("card data is complete");
    let statements = statements_by_game(&record).remove(0);
    let cast_at = statements
        .iter()
        .position(|s| s.ends_with("\"Thoughtseize\" cast from hand"))
        .unwrap_or_else(|| panic!("the cast: {statements:?}"));
    let original = id_at(&statements[cast_at]).expect("an id");
    let copy = original + 1;
    let name = "\"Thoughtseize\"";
    let new_copy = format!(
        "new copy #{copy} of #{original} {name} {name} owner \"Opponent\" in stack by #{original}"
    );
    let at = statements
        .iter()
        .position(|s| *s == new_copy)
        .unwrap_or_else(|| panic!("the copy: {statements:?}"));
    let moved = statements[at..]
        .iter()
        .position(|s| *s == format!("#{copy} {name} move stack -> graveyard by rule:resolve"))
        .expect("the copy resolves")
        + at;
    assert_eq!(
        statements[moved + 1],
        format!("#{copy} {name} cease by rule:sba")
    );
    assert!(
        !statements.iter().any(|s| s.starts_with("incomplete ")),
        "{statements:?}"
    );
    assert!(!statements.iter().any(|s| s.contains("stack -> stack")));
}

/// A diff state message for a synthetic game, following state `id - 1`.
fn diff_state(id: u32, turn: &str, body: &str) -> String {
    format!(
        r#"{{"type":"GREMessageType_GameStateMessage","systemSeatIds":[1],
      "gameStateMessage":{{"type":"GameStateType_Diff","gameStateId":{id},"prevGameStateId":{prev},
        "turnInfo":{turn}{body}}}}}"#,
        prev = id - 1
    )
}

/// The opponent's Youthful Valkyrie object `id` of `kind` in zone `zone`.
fn copy_test_valkyrie(id: u32, kind: &str, zone: u32, extra: &str) -> String {
    format!(
        r#"{{"instanceId":{id},"grpId":75350,"type":"GameObjectType_{kind}","zoneId":{zone},"visibility":"Visibility_Public","ownerSeatId":2,"controllerSeatId":2,"cardTypes":["CardType_Creature"],"power":{{"value":2}},"toughness":{{"value":2}}{extra}}}"#
    )
}

const COPY_TEST_TURN_TWO: &str =
    r#"{"turnNumber":2,"activePlayer":1,"phase":"Phase_Main1","priorityPlayer":1}"#;

/// The opponent casts Youthful Valkyrie, copies it, and both resolve onto
/// the battlefield in turn 1 (the copy as Arena object `resolved_kind`,
/// instance 221); states 1 to 8, the last one turn 2's first.
fn copied_valkyrie_states(resolved_kind: &str) -> Vec<String> {
    let state = diff_state;
    let main = |priority: u32| {
        format!(
            r#"{{"turnNumber":1,"activePlayer":2,"phase":"Phase_Main1","priorityPlayer":{priority}}}"#
        )
    };
    let valkyrie = copy_test_valkyrie;
    let full = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[1],
      "gameStateMessage":{"type":"GameStateType_Full","gameStateId":1,
        "players":[{"systemSeatNumber":1,"lifeTotal":20},{"systemSeatNumber":2,"lifeTotal":20}],
        "turnInfo":{"turnNumber":1,"activePlayer":2,"phase":"Phase_Main1","priorityPlayer":2},
        "zones":[
          {"zoneId":27,"type":"ZoneType_Stack","objectInstanceIds":[]},
          {"zoneId":28,"type":"ZoneType_Battlefield","objectInstanceIds":[]},
          {"zoneId":31,"type":"ZoneType_Hand","ownerSeatId":2,"objectInstanceIds":[200]},
          {"zoneId":33,"type":"ZoneType_Graveyard","ownerSeatId":2,"objectInstanceIds":[]}]}}"#
        .to_string();
    let cast = state(
        2,
        &main(2),
        &format!(
            r#","zones":[
          {{"zoneId":27,"type":"ZoneType_Stack","objectInstanceIds":[210]}},
          {{"zoneId":31,"type":"ZoneType_Hand","ownerSeatId":2,"objectInstanceIds":[]}}],
        "gameObjects":[{spell}],
        "annotations":[
          {{"id":1,"affectedIds":[200],"type":["AnnotationType_ObjectIdChanged"],
            "details":[{{"key":"orig_id","valueInt32":[200]}},{{"key":"new_id","valueInt32":[210]}}]}},
          {{"id":2,"affectedIds":[210],"type":["AnnotationType_ZoneTransfer"],
            "details":[{{"key":"zone_src","valueInt32":[31]}},{{"key":"zone_dest","valueInt32":[27]}},{{"key":"category","valueString":["CastSpell"]}}]}}]"#,
            spell = valkyrie(210, "Card", 27, ""),
        ),
    );
    let copy = state(
        3,
        &main(2),
        &format!(
            r#","zones":[{{"zoneId":27,"type":"ZoneType_Stack","objectInstanceIds":[220,210]}}],
        "gameObjects":[{copy}],
        "annotations":[
          {{"id":3,"affectorId":210,"affectedIds":[220],"type":["AnnotationType_ZoneTransfer"],
            "details":[{{"key":"zone_src","valueInt32":[27]}},{{"key":"zone_dest","valueInt32":[27]}},{{"key":"category","valueString":["Copy"]}}]}}]"#,
            copy = valkyrie(220, "Card", 27, r#","isCopy":true,"parentId":210"#),
        ),
    );
    let pass = state(4, &main(1), "");
    let copy_resolves = state(
        5,
        &main(2),
        &format!(
            r#","zones":[
          {{"zoneId":27,"type":"ZoneType_Stack","objectInstanceIds":[210]}},
          {{"zoneId":28,"type":"ZoneType_Battlefield","objectInstanceIds":[221]}}],
        "gameObjects":[{token}],
        "diffDeletedInstanceIds":[220],
        "annotations":[
          {{"id":4,"affectorId":220,"affectedIds":[220],"type":["AnnotationType_ResolutionStart"]}},
          {{"id":5,"affectorId":220,"affectedIds":[220],"type":["AnnotationType_ResolutionComplete"]}},
          {{"id":6,"affectedIds":[220],"type":["AnnotationType_ObjectIdChanged"],
            "details":[{{"key":"orig_id","valueInt32":[220]}},{{"key":"new_id","valueInt32":[221]}}]}},
          {{"id":7,"affectorId":1,"affectedIds":[221],"type":["AnnotationType_ZoneTransfer"],
            "details":[{{"key":"zone_src","valueInt32":[27]}},{{"key":"zone_dest","valueInt32":[28]}},{{"key":"category","valueString":["Resolve"]}}]}}]"#,
            token = valkyrie(221, resolved_kind, 28, r#","isCopy":true"#),
        ),
    );
    let pass_again = state(6, &main(1), "");
    let original_resolves = state(
        7,
        &main(2),
        &format!(
            r#","zones":[
          {{"zoneId":27,"type":"ZoneType_Stack","objectInstanceIds":[]}},
          {{"zoneId":28,"type":"ZoneType_Battlefield","objectInstanceIds":[221,211]}}],
        "gameObjects":[{card}],
        "diffDeletedInstanceIds":[210],
        "annotations":[
          {{"id":8,"affectorId":210,"affectedIds":[210],"type":["AnnotationType_ResolutionStart"]}},
          {{"id":9,"affectorId":210,"affectedIds":[210],"type":["AnnotationType_ResolutionComplete"]}},
          {{"id":10,"affectedIds":[210],"type":["AnnotationType_ObjectIdChanged"],
            "details":[{{"key":"orig_id","valueInt32":[210]}},{{"key":"new_id","valueInt32":[211]}}]}},
          {{"id":11,"affectorId":1,"affectedIds":[211],"type":["AnnotationType_ZoneTransfer"],
            "details":[{{"key":"zone_src","valueInt32":[27]}},{{"key":"zone_dest","valueInt32":[28]}},{{"key":"category","valueString":["Resolve"]}}]}}]"#,
            card = valkyrie(211, "Card", 28, ""),
        ),
    );
    let next_turn = state(8, COPY_TEST_TURN_TWO, "");
    vec![
        full,
        cast,
        copy,
        pass,
        copy_resolves,
        pass_again,
        original_resolves,
        next_turn,
    ]
}

/// Decision 0023 item 4, spec 7.3 (CR 608.3f): a copy of a permanent spell
/// becomes a token as it resolves and keeps its id. It lives across the
/// turn (the next keyframe lists it), and once it is a token it ceases
/// when Arena deletes it outside the battlefield (CR 111.7), here after a
/// bounce to its owner's hand.
#[test]
fn copied_permanent_spell_resolves_into_a_token() {
    let state = diff_state;
    let valkyrie = copy_test_valkyrie;
    let turn_two = COPY_TEST_TURN_TWO;
    let bounce = state(
        9,
        turn_two,
        &format!(
            r#","zones":[
          {{"zoneId":28,"type":"ZoneType_Battlefield","objectInstanceIds":[211]}},
          {{"zoneId":31,"type":"ZoneType_Hand","ownerSeatId":2,"objectInstanceIds":[222]}}],
        "gameObjects":[{token}],
        "diffDeletedInstanceIds":[221],
        "annotations":[
          {{"id":12,"affectedIds":[221],"type":["AnnotationType_ObjectIdChanged"],
            "details":[{{"key":"orig_id","valueInt32":[221]}},{{"key":"new_id","valueInt32":[222]}}]}},
          {{"id":13,"affectedIds":[222],"type":["AnnotationType_ZoneTransfer"],
            "details":[{{"key":"zone_src","valueInt32":[28]}},{{"key":"zone_dest","valueInt32":[31]}},{{"key":"category","valueString":["Return"]}}]}}]"#,
            token = valkyrie(222, "Token", 31, r#","isCopy":true"#),
        ),
    );
    let deleted = state(
        10,
        turn_two,
        r#","zones":[{"zoneId":31,"type":"ZoneType_Hand","ownerSeatId":2,"objectInstanceIds":[]}],
        "diffDeletedInstanceIds":[222],
        "annotations":[
          {"id":14,"affectedIds":[222],"type":["AnnotationType_TokenDeleted"]}]"#,
    );
    let mut states = copied_valkyrie_states("Token");
    states.push(bounce);
    states.push(deleted);
    let refs: Vec<&str> = states.iter().map(String::as_str).collect();
    let record =
        match_record(&synthetic_match(&refs), &fixture_cards()).expect("card data is complete");
    let statements = statements_by_game(&record).remove(0);
    let name = "\"Youthful Valkyrie\"";
    let cast_at = statements
        .iter()
        .position(|s| s.ends_with(&format!("{name} cast from hand")))
        .unwrap_or_else(|| panic!("the cast: {statements:#?}"));
    let original = id_at(&statements[cast_at]).expect("an id");
    let copy = original + 1;
    let expected = [
        format!(
            "new copy #{copy} of #{original} {name} {name} owner \"Opponent\" in stack by #{original}"
        ),
        format!("#{copy} {name} move stack -> battlefield types creature pt 2/2 by rule:resolve"),
        format!("#{copy} {name} battlefield \"Opponent\" types creature pt 2/2"),
        format!("#{copy} {name} move battlefield -> hand by ?"),
        format!("#{copy} {name} cease by rule:sba"),
    ];
    let mut from = 0;
    for line in &expected {
        let at = statements[from..]
            .iter()
            .position(|s| s == line)
            .unwrap_or_else(|| panic!("{line} after line {from}: {statements:#?}"));
        from += at + 1;
    }
    let turn = statements
        .iter()
        .position(|s| s.starts_with("turn 2 "))
        .expect("turn 2");
    assert!(
        statements[turn..].iter().any(|s| *s == expected[2]),
        "the copy lives across the turn: {statements:#?}"
    );
    assert!(
        !statements.iter().any(|s| s.starts_with("incomplete ")),
        "{statements:#?}"
    );
}

/// A resolved copy Arena still shows as a card (`isCopy`, not a token
/// object) stays a copy: it gets no `TokenDeleted`, and it ceases once it
/// is in a graveyard and Arena shows it nowhere (CR 704.5e).
#[test]
fn copied_permanent_spell_shown_as_a_card_stays_a_copy() {
    let mut states = copied_valkyrie_states("Card");
    states.push(diff_state(
        9,
        COPY_TEST_TURN_TWO,
        &format!(
            r#","zones":[
          {{"zoneId":28,"type":"ZoneType_Battlefield","objectInstanceIds":[211]}},
          {{"zoneId":33,"type":"ZoneType_Graveyard","ownerSeatId":2,"objectInstanceIds":[222]}}],
        "gameObjects":[{copy}],
        "diffDeletedInstanceIds":[221],
        "annotations":[
          {{"id":12,"affectedIds":[221],"type":["AnnotationType_ObjectIdChanged"],
            "details":[{{"key":"orig_id","valueInt32":[221]}},{{"key":"new_id","valueInt32":[222]}}]}},
          {{"id":13,"affectedIds":[222],"type":["AnnotationType_ZoneTransfer"],
            "details":[{{"key":"zone_src","valueInt32":[28]}},{{"key":"zone_dest","valueInt32":[33]}},{{"key":"category","valueString":["Destroy"]}}]}}]"#,
            copy = copy_test_valkyrie(222, "Card", 33, r#","isCopy":true"#),
        ),
    ));
    states.push(diff_state(
        10,
        COPY_TEST_TURN_TWO,
        r#","zones":[{"zoneId":33,"type":"ZoneType_Graveyard","ownerSeatId":2,"objectInstanceIds":[]}],
        "diffDeletedInstanceIds":[222]"#,
    ));
    let refs: Vec<&str> = states.iter().map(String::as_str).collect();
    let record =
        match_record(&synthetic_match(&refs), &fixture_cards()).expect("card data is complete");
    let statements = statements_by_game(&record).remove(0);
    let name = "\"Youthful Valkyrie\"";
    let moved = statements
        .iter()
        .position(|s| s.ends_with(" move battlefield -> graveyard as destroy by ?"))
        .unwrap_or_else(|| panic!("the copy dies: {statements:#?}"));
    let copy = id_at(&statements[moved]).expect("an id");
    assert!(
        statements[moved + 1..].contains(&format!("#{copy} {name} cease by rule:sba")),
        "{statements:#?}"
    );
}

/// A copy the record first sees as it leaves the battlefield (no transfer
/// made it) is introduced there as a permanent: `ENTERING` with its types
/// (spec 6.5).
#[test]
fn copy_first_seen_on_the_battlefield_is_introduced_as_a_permanent() {
    let full = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[1],
      "gameStateMessage":{"type":"GameStateType_Full","gameStateId":1,
        "players":[{"systemSeatNumber":1,"lifeTotal":20},{"systemSeatNumber":2,"lifeTotal":20}],
        "turnInfo":{"turnNumber":1,"activePlayer":2,"phase":"Phase_Main1","priorityPlayer":2},
        "zones":[
          {"zoneId":28,"type":"ZoneType_Battlefield","objectInstanceIds":[]},
          {"zoneId":31,"type":"ZoneType_Hand","ownerSeatId":2,"objectInstanceIds":[200]},
          {"zoneId":33,"type":"ZoneType_Graveyard","ownerSeatId":2,"objectInstanceIds":[]}]}}"#;
    let played = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[1],
      "gameStateMessage":{"type":"GameStateType_Diff","gameStateId":2,"prevGameStateId":1,
        "turnInfo":{"turnNumber":1,"activePlayer":2,"phase":"Phase_Main1","priorityPlayer":2},
        "zones":[
          {"zoneId":28,"type":"ZoneType_Battlefield","objectInstanceIds":[201,300]},
          {"zoneId":31,"type":"ZoneType_Hand","ownerSeatId":2,"objectInstanceIds":[]}],
        "gameObjects":[
          {"instanceId":201,"grpId":82390,"type":"GameObjectType_Card","zoneId":28,"visibility":"Visibility_Public","ownerSeatId":2,"controllerSeatId":2,"cardTypes":["CardType_Land"]},
          {"instanceId":300,"grpId":82390,"type":"GameObjectType_Card","zoneId":28,"visibility":"Visibility_Public","ownerSeatId":2,"controllerSeatId":2,"cardTypes":["CardType_Land"],"isCopy":true,"parentId":201}],
        "annotations":[
          {"id":1,"affectedIds":[200],"type":["AnnotationType_ObjectIdChanged"],
            "details":[{"key":"orig_id","valueInt32":[200]},{"key":"new_id","valueInt32":[201]}]},
          {"id":2,"affectedIds":[201],"type":["AnnotationType_ZoneTransfer"],
            "details":[{"key":"zone_src","valueInt32":[31]},{"key":"zone_dest","valueInt32":[28]},{"key":"category","valueString":["PlayLand"]}]}]}}"#;
    let dies = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[1],
      "gameStateMessage":{"type":"GameStateType_Diff","gameStateId":3,"prevGameStateId":2,
        "turnInfo":{"turnNumber":1,"activePlayer":2,"phase":"Phase_Main1","priorityPlayer":2},
        "zones":[
          {"zoneId":28,"type":"ZoneType_Battlefield","objectInstanceIds":[201]},
          {"zoneId":33,"type":"ZoneType_Graveyard","ownerSeatId":2,"objectInstanceIds":[301]}],
        "gameObjects":[
          {"instanceId":301,"grpId":82390,"type":"GameObjectType_Card","zoneId":33,"visibility":"Visibility_Public","ownerSeatId":2,"controllerSeatId":2,"cardTypes":["CardType_Land"],"isCopy":true}],
        "diffDeletedInstanceIds":[300],
        "annotations":[
          {"id":3,"affectedIds":[300],"type":["AnnotationType_ObjectIdChanged"],
            "details":[{"key":"orig_id","valueInt32":[300]},{"key":"new_id","valueInt32":[301]}]},
          {"id":4,"affectedIds":[301],"type":["AnnotationType_ZoneTransfer"],
            "details":[{"key":"zone_src","valueInt32":[28]},{"key":"zone_dest","valueInt32":[33]},{"key":"category","valueString":["Destroy"]}]}]}}"#;
    let record = match_record(&synthetic_match(&[full, played, dies]), &fixture_cards())
        .expect("card data is complete");
    let statements = statements_by_game(&record).remove(0);
    let copy = statements
        .iter()
        .find(|s| s.starts_with("new copy "))
        .unwrap_or_else(|| panic!("the copy: {statements:#?}"));
    assert!(
        copy.ends_with(" \"Plains\" owner \"Opponent\" in battlefield types land by ?"),
        "{copy}"
    );
}

/// Decision 0044, spec 9.10: an effect a resolving ability creates (Arena's
/// `LayeredEffectCreated` with the ability as affector) that outlives the
/// cleanup step is a lasting effect, written with text `?`. Loss game 1:
/// Nissa, Who Shakes the World's +1 makes a Forest a creature with no
/// duration (log line 10614, ability 605, six layered effect ids, one
/// effect, CR 613.6). Keyframes list it while it lasts.
#[test]
fn lasting_effects_are_written_with_unknown_text() {
    let statements = loss_game_one();
    let created: Vec<&String> = statements
        .iter()
        .filter(|s| s.starts_with("new effect "))
        .collect();
    assert_eq!(created.len(), 1, "{created:?}");
    let line = created[0];
    let id = line
        .strip_prefix("new effect #")
        .and_then(|rest| rest.split(' ').next())
        .expect("an effect id");
    let (_, rest) = line.split_once(" of ").expect("a source");
    let (source, cause) = rest
        .split_once(" ? controller \"Opponent\" by ")
        .expect("unknown text, the controller and a cause");
    assert!(
        source.ends_with(" \"Nissa, Who Shakes the World\""),
        "{line}"
    );
    let ability = id_at(cause).expect("caused by an ability");
    let at = statements.iter().position(|s| s == line).expect("the line");
    assert_eq!(statements[at - 1], format!("#{ability} resolve"), "{line}");
    assert!(
        statements[..at].iter().any(|s| s.starts_with(&format!(
            "new ability #{ability} loyalty of {source} ? controller \"Opponent\""
        ))),
        "{line}"
    );
    let entry = format!("#{id} effect of {source} ? controller \"Opponent\"");
    let keyframe = statements[at..]
        .iter()
        .position(|s| s == "keyframe")
        .expect("a later turn");
    assert!(statements[at + keyframe..].contains(&entry), "{entry}");

    // Three-games: Reckless Stormseeker's trigger (log line 3422) and The
    // Wandering Emperor's +1 (21446) end in the cleanup step ("until end of
    // turn"), and an Aura's effects are its static ability (CR 611.3; lines
    // 9625, 18369, 20346): none is a lasting effect.
    let record = match_record(&three_games(), &fixture_cards()).expect("card data is complete");
    for statements in statements_by_game(&record) {
        assert!(!statements.iter().any(|s| s.starts_with("new effect ")));
    }
}

/// A Forest's ability making it a creature, which
/// Arena ends in the next turn: `new effect` as it resolves, listed by the
/// next keyframe, `cease by ?` when Arena destroys it (Arena does not say
/// why it ended). The `new effect` comes straight after `resolve`, before
/// the lines its resolution causes.
#[test]
fn lasting_effect_ceases_when_arena_destroys_it() {
    let full = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[1],
      "gameStateMessage":{"type":"GameStateType_Full","gameStateId":1,
        "players":[{"systemSeatNumber":1,"lifeTotal":20},{"systemSeatNumber":2,"lifeTotal":20}],
        "turnInfo":{"turnNumber":1,"activePlayer":1,"phase":"Phase_Main1","priorityPlayer":1},
        "zones":[
          {"zoneId":27,"type":"ZoneType_Stack","objectInstanceIds":[]},
          {"zoneId":28,"type":"ZoneType_Battlefield","objectInstanceIds":[]},
          {"zoneId":31,"type":"ZoneType_Hand","ownerSeatId":1,"objectInstanceIds":[300]}],
        "gameObjects":[
          {"instanceId":300,"grpId":95190,"type":"GameObjectType_Card","zoneId":31,"visibility":"Visibility_Private","ownerSeatId":1,"controllerSeatId":1,"cardTypes":["CardType_Land"]}]}}"#;
    let play = synthetic_play(2, 300, 31, 1, "300");
    let trigger = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[1],
      "gameStateMessage":{"type":"GameStateType_Diff","gameStateId":3,"prevGameStateId":2,
        "turnInfo":{"turnNumber":1,"activePlayer":1,"phase":"Phase_Main1","priorityPlayer":1},
        "zones":[{"zoneId":27,"type":"ZoneType_Stack","objectInstanceIds":[400]}],
        "gameObjects":[
          {"instanceId":400,"grpId":192752,"type":"GameObjectType_Ability","zoneId":27,"visibility":"Visibility_Public","ownerSeatId":1,"controllerSeatId":1,"objectSourceGrpId":95190,"parentId":300}],
        "annotations":[
          {"id":1,"affectorId":300,"affectedIds":[400],"type":["AnnotationType_AbilityInstanceCreated"]}]}}"#;
    let other_priority = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[1],
      "gameStateMessage":{"type":"GameStateType_Diff","gameStateId":4,"prevGameStateId":3,
        "turnInfo":{"turnNumber":1,"activePlayer":1,"phase":"Phase_Main1","priorityPlayer":2}}}"#;
    let resolve = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[1],
      "gameStateMessage":{"type":"GameStateType_Diff","gameStateId":5,"prevGameStateId":4,
        "turnInfo":{"turnNumber":1,"activePlayer":1,"phase":"Phase_Main1","priorityPlayer":1},
        "zones":[{"zoneId":27,"type":"ZoneType_Stack","objectInstanceIds":[]}],
        "diffDeletedInstanceIds":[400],
        "annotations":[
          {"id":2,"affectorId":400,"affectedIds":[400],"type":["AnnotationType_ResolutionStart"],
            "details":[{"key":"grpid","valueInt32":[192752]}]},
          {"id":3,"affectorId":400,"affectedIds":[7002],"type":["AnnotationType_LayeredEffectCreated"]},
          {"id":4,"affectorId":400,"affectedIds":[7003],"type":["AnnotationType_LayeredEffectCreated"]},
          {"id":5,"affectorId":400,"affectedIds":[400],"type":["AnnotationType_ResolutionComplete"],
            "details":[{"key":"grpid","valueInt32":[192752]}]},
          {"id":6,"affectorId":300,"affectedIds":[400],"type":["AnnotationType_AbilityInstanceDeleted"]}],
        "persistentAnnotations":[
          {"id":50,"affectorId":300,"affectedIds":[300],"type":["AnnotationType_ModifiedType","AnnotationType_LayeredEffect"],
            "details":[{"key":"effect_id","valueInt32":[7002]}]},
          {"id":51,"affectorId":300,"affectedIds":[300],"type":["AnnotationType_AddAbility","AnnotationType_LayeredEffect"],
            "details":[{"key":"effect_id","valueInt32":[7003]}]}]}}"#;
    let next_turn = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[1],
      "gameStateMessage":{"type":"GameStateType_Diff","gameStateId":6,"prevGameStateId":5,
        "turnInfo":{"turnNumber":2,"activePlayer":2,"phase":"Phase_Beginning","step":"Step_Upkeep","priorityPlayer":2},
        "annotations":[
          {"id":7,"affectorId":2,"affectedIds":[2],"type":["AnnotationType_NewTurnStarted"]},
          {"id":8,"affectedIds":[2],"type":["AnnotationType_PhaseOrStepModified"],
            "details":[{"key":"phase","valueInt32":[1]},{"key":"step","valueInt32":[1]}]}]}}"#;
    let ended = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[1],
      "gameStateMessage":{"type":"GameStateType_Diff","gameStateId":7,"prevGameStateId":6,
        "turnInfo":{"turnNumber":2,"activePlayer":2,"phase":"Phase_Main1","priorityPlayer":2},
        "diffDeletedPersistentAnnotationIds":[50,51],
        "annotations":[
          {"id":9,"affectedIds":[2],"type":["AnnotationType_PhaseOrStepModified"],
            "details":[{"key":"phase","valueInt32":[2]},{"key":"step","valueInt32":[0]}]},
          {"id":10,"affectorId":300,"affectedIds":[7002,7003],"type":["AnnotationType_LayeredEffectDestroyed"]}]}}"#;
    let input = synthetic_match(&[
        full,
        &play,
        trigger,
        other_priority,
        resolve,
        next_turn,
        ended,
    ]);
    let record = match_record(&input, &fixture_cards()).expect("card data is complete");
    let statements = statements_by_game(&record).remove(0);
    let created: Vec<&String> = statements
        .iter()
        .filter(|s| s.starts_with("new effect "))
        .collect();
    assert_eq!(created.len(), 1, "{statements:?}");
    let id = id_at(
        created[0]
            .strip_prefix("new effect ")
            .expect("a new effect"),
    )
    .expect("an id");
    let resolve_at = statements
        .iter()
        .position(|s| s.ends_with(" resolve"))
        .expect("the ability resolves");
    let ability = id_at(&statements[resolve_at]).expect("an ability id");
    let at = statements
        .iter()
        .position(|s| s == created[0])
        .expect("the line");
    assert_eq!(at, resolve_at + 1, "{statements:?}");
    let source = statements
        .iter()
        .find_map(|s| {
            s.strip_prefix(&format!("new ability #{ability} triggered of "))
                .and_then(|rest| rest.strip_suffix(" ? controller \"Player\" in stack"))
        })
        .expect("the ability's source")
        .to_string();
    assert_eq!(
        *created[0],
        format!("new effect #{id} of {source} ? controller \"Player\" by #{ability}")
    );
    let keyframe = statements
        .iter()
        .rposition(|s| s == "keyframe")
        .expect("turn 2's keyframe");
    assert!(keyframe > at);
    assert!(
        statements[keyframe..]
            .contains(&format!("#{id} effect of {source} ? controller \"Player\"")),
        "{statements:?}"
    );
    let ceased: Vec<&String> = statements.iter().filter(|s| mentions(s, id)).collect();
    assert_eq!(
        ceased.last().map(|s| s.as_str()),
        Some(format!("#{id} cease by ?").as_str()),
        "{statements:?}"
    );
}

/// CR 103.5 and decision 0043: the cards a mulligan puts on the bottom are
/// written `bottom`, in the order Arena lists them (loss log line 3657: the
/// last one listed is the bottom card), and keyframes give their positions.
#[test]
fn mulligan_bottoms_are_written_on_the_bottom() {
    let record = match_record(&three_games(), &fixture_cards()).expect("card data is complete");
    let statements = statements_by_game(&record).remove(0);
    assert!(statements.contains(&"#15 move hand -> library bottom by rule:mulligan".to_string()));
    assert!(statements.contains(&"#15 library \"Opponent\" bottom 1".to_string()));

    let statements = loss_game_one();
    let bottoms: Vec<u32> = statements
        .iter()
        .filter(|s| s.ends_with(" move hand -> library bottom by rule:mulligan"))
        .filter_map(|s| id_at(s))
        .collect();
    assert_eq!(bottoms.len(), 4, "{statements:?}");
    // Two players bottom two cards each: the later line is the bottom card.
    let keyframe = statements
        .iter()
        .position(|s| s.starts_with("turn 1 "))
        .expect("turn 1");
    for (id, position) in [
        (bottoms[0], 2),
        (bottoms[1], 1),
        (bottoms[2], 2),
        (bottoms[3], 1),
    ] {
        assert!(
            statements[keyframe..]
                .iter()
                .any(|s| mentions(s, id) && s.ends_with(&format!(" bottom {position}"))),
            "#{id} bottom {position}: {statements:?}"
        );
    }
    // No other move into a library has a position the states do not show:
    // the hands shuffled back by a mulligan have none.
    assert!(
        statements
            .iter()
            .filter(|s| s.contains(" -> library ") && s.ends_with(" by rule:mulligan"))
            .all(|s| s.contains(" -> library bottom ") || s.contains(" -> library by "))
    );
}

/// CR 701.22a, decision 0043: Arena reports a scry's results (`Scry`
/// annotation, `topIds` and `bottomIds`; owner data: seven scries). The
/// scried cards are the library's top cards: introduced there with their
/// position, looked at, and moved to the bottom when put there. The local
/// player's card is named; the opponent's stays hidden.
#[test]
fn scry_results_are_written() {
    let full = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[1],
      "gameStateMessage":{"type":"GameStateType_Full","gameStateId":1,
        "players":[{"systemSeatNumber":1,"lifeTotal":20},{"systemSeatNumber":2,"lifeTotal":20}],
        "turnInfo":{"turnNumber":1,"activePlayer":1,"phase":"Phase_Main1","priorityPlayer":1},
        "zones":[
          {"zoneId":27,"type":"ZoneType_Stack","objectInstanceIds":[]},
          {"zoneId":28,"type":"ZoneType_Battlefield","objectInstanceIds":[]},
          {"zoneId":31,"type":"ZoneType_Hand","ownerSeatId":1,"objectInstanceIds":[300]},
          {"zoneId":35,"type":"ZoneType_Hand","ownerSeatId":2,"objectInstanceIds":[310]},
          {"zoneId":32,"type":"ZoneType_Library","ownerSeatId":1,"objectInstanceIds":[161,162,163]},
          {"zoneId":36,"type":"ZoneType_Library","ownerSeatId":2,"objectInstanceIds":[171,172]}],
        "gameObjects":[
          {"instanceId":300,"grpId":95190,"type":"GameObjectType_Card","zoneId":31,"visibility":"Visibility_Private","ownerSeatId":1,"controllerSeatId":1,"cardTypes":["CardType_Land"]}]}}"#;
    let trigger = |state: u32, ability: u32, source: u32, seat: u32| {
        format!(
            r#"{{"type":"GREMessageType_GameStateMessage","systemSeatIds":[1],
      "gameStateMessage":{{"type":"GameStateType_Diff","gameStateId":{state},"prevGameStateId":{prev},
        "turnInfo":{{"turnNumber":1,"activePlayer":1,"phase":"Phase_Main1","priorityPlayer":{seat}}},
        "zones":[{{"zoneId":27,"type":"ZoneType_Stack","objectInstanceIds":[{ability}]}}],
        "gameObjects":[
          {{"instanceId":{ability},"grpId":192752,"type":"GameObjectType_Ability","zoneId":27,"visibility":"Visibility_Public","ownerSeatId":{seat},"controllerSeatId":{seat},"objectSourceGrpId":95190,"parentId":{source}}}],
        "annotations":[
          {{"id":{state}01,"affectorId":{source},"affectedIds":[{ability}],"type":["AnnotationType_AbilityInstanceCreated"]}}]}}}}"#,
            prev = state - 1
        )
    };
    let priority = |state: u32, seat: u32| {
        format!(
            r#"{{"type":"GREMessageType_GameStateMessage","systemSeatIds":[1],
      "gameStateMessage":{{"type":"GameStateType_Diff","gameStateId":{state},"prevGameStateId":{prev},
        "turnInfo":{{"turnNumber":1,"activePlayer":1,"phase":"Phase_Main1","priorityPlayer":{seat}}}}}}}"#,
            prev = state - 1
        )
    };
    let scry = |state: u32, ability: u32, source: u32, scry: &str, zone: &str, objects: &str| {
        format!(
            r#"{{"type":"GREMessageType_GameStateMessage","systemSeatIds":[1],
      "gameStateMessage":{{"type":"GameStateType_Diff","gameStateId":{state},"prevGameStateId":{prev},
        "turnInfo":{{"turnNumber":1,"activePlayer":1,"phase":"Phase_Main1","priorityPlayer":1}},
        "zones":[{{"zoneId":27,"type":"ZoneType_Stack","objectInstanceIds":[]}}{zone}],
        "gameObjects":[{objects}],
        "diffDeletedInstanceIds":[{ability}],
        "annotations":[
          {{"id":{state}01,"affectorId":{ability},"affectedIds":[{ability}],"type":["AnnotationType_ResolutionStart"],
            "details":[{{"key":"grpid","valueInt32":[192752]}}]}},
          {scry},
          {{"id":{state}03,"affectorId":{ability},"affectedIds":[{ability}],"type":["AnnotationType_ResolutionComplete"],
            "details":[{{"key":"grpid","valueInt32":[192752]}}]}},
          {{"id":{state}04,"affectorId":{source},"affectedIds":[{ability}],"type":["AnnotationType_AbilityInstanceDeleted"]}}]}}}}"#,
            prev = state - 1
        )
    };
    let local_scry = scry(
        6,
        400,
        300,
        r#"{"id":602,"affectorId":400,"affectedIds":[161],"type":["AnnotationType_Scry"],
            "details":[{"key":"topIds"},{"key":"bottomIds","type":"KeyValuePairValueType_int32","valueInt32":[161]}]}"#,
        r#",{"zoneId":32,"type":"ZoneType_Library","ownerSeatId":1,"objectInstanceIds":[162,163,161]}"#,
        r#"{"instanceId":161,"grpId":95190,"type":"GameObjectType_Card","zoneId":32,"visibility":"Visibility_Private","ownerSeatId":1,"controllerSeatId":1,"cardTypes":["CardType_Land"]}"#,
    );
    let opponent_scry = scry(
        9,
        401,
        310,
        r#"{"id":902,"affectorId":401,"affectedIds":[171],"type":["AnnotationType_Scry"],
            "details":[{"key":"topIds","type":"KeyValuePairValueType_int32","valueInt32":[171]},{"key":"bottomIds"}]}"#,
        "",
        "",
    );
    let next_turn = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[1],
      "gameStateMessage":{"type":"GameStateType_Diff","gameStateId":10,"prevGameStateId":9,
        "turnInfo":{"turnNumber":2,"activePlayer":2,"phase":"Phase_Beginning","step":"Step_Upkeep","priorityPlayer":2},
        "annotations":[
          {"id":1001,"affectorId":2,"affectedIds":[2],"type":["AnnotationType_NewTurnStarted"]},
          {"id":1002,"affectedIds":[2],"type":["AnnotationType_PhaseOrStepModified"],
            "details":[{"key":"phase","valueInt32":[1]},{"key":"step","valueInt32":[1]}]}]}}"#;
    let states = [
        full.to_string(),
        synthetic_play(2, 300, 31, 1, "300"),
        synthetic_play(3, 310, 35, 2, "300,310"),
        trigger(4, 400, 300, 1),
        priority(5, 2),
        local_scry,
        trigger(7, 401, 310, 2),
        priority(8, 1),
        opponent_scry,
        next_turn.to_string(),
    ];
    let states: Vec<&str> = states.iter().map(String::as_str).collect();
    let record =
        match_record(&synthetic_match(&states), &fixture_cards()).expect("card data is complete");
    let statements = statements_by_game(&record).remove(0);
    let ability_of = |n: usize| {
        statements
            .iter()
            .filter(|s| s.starts_with("new ability #"))
            .nth(n)
            .and_then(|s| id_at(s.strip_prefix("new ability ")?))
            .unwrap_or_else(|| panic!("the scry abilities: {statements:?}"))
    };
    let (mine, theirs) = (ability_of(0), ability_of(1));
    let introduced = statements
        .iter()
        .enumerate()
        .filter(|(_, s)| {
            s.starts_with("new card #")
                && s.ends_with(" \"Forest\" owner \"Player\" in library top 1")
        })
        .map(|(at, _)| at)
        .find(|at| statements[at + 1].contains(" look "))
        .unwrap_or_else(|| panic!("the scried Forest: {statements:?}"));
    let forest = id_at(
        statements[introduced]
            .strip_prefix("new card ")
            .expect("new card"),
    )
    .expect("an id");
    assert_eq!(
        statements[introduced + 1..introduced + 3].to_vec(),
        vec![
            format!("#{forest} \"Forest\" look \"Player\" by #{mine}"),
            format!("#{forest} \"Forest\" move library -> library bottom by #{mine}"),
        ]
    );
    let hidden = statements
        .iter()
        .enumerate()
        .filter(|(_, s)| {
            s.starts_with("new hidden #") && s.ends_with(" owner \"Opponent\" in library top 1")
        })
        .map(|(at, _)| at)
        .find(|at| statements[at + 1].contains(" look "))
        .unwrap_or_else(|| panic!("the opponent's scried card: {statements:?}"));
    let card = id_at(
        statements[hidden]
            .strip_prefix("new hidden ")
            .expect("new hidden"),
    )
    .expect("an id");
    assert_eq!(
        statements[hidden + 1],
        format!("#{card} look \"Opponent\" by #{theirs}")
    );
    assert!(
        !statements
            .iter()
            .any(|s| s.starts_with(&format!("#{card} move")))
    );
    let keyframe = statements
        .iter()
        .rposition(|s| s == "keyframe")
        .expect("turn 2's keyframe");
    let rest = &statements[keyframe..];
    assert!(
        rest.contains(&format!("#{forest} \"Forest\" library \"Player\" bottom 1")),
        "{rest:?}"
    );
    assert!(
        rest.contains(&format!("#{card} library \"Opponent\" top 1")),
        "{rest:?}"
    );
}

/// Spec 6.5, decision 0019 item 3: a card introduced in a library as it
/// leaves carries its position when the perspective player knows it: a
/// draw takes the top card (CR 121.2), and Cavalier of Thorns reveals the
/// top five cards (loss log, game 1 at 2:27), each introduced where it was
/// after the ones taken before it. A searched card has none (three-games:
/// the opponent's tutors reveal a card from the library, then shuffle).
#[test]
fn new_library_cards_take_public_positions() {
    let loss = fixture_match("bo3-loss-and-match-concede", 0, "Opponent");
    let mut games = Vec::new();
    for input in [three_games(), loss] {
        let record = match_record(&input, &fixture_cards()).expect("card data is complete");
        games.extend(statements_by_game(&record));
    }
    let mut draws = 0;
    for statements in &games {
        for pair in statements.windows(2) {
            if pair[0].starts_with("new ")
                && pair[0].contains(" in library")
                && pair[1].contains(" as draw ")
            {
                assert!(pair[0].ends_with(" in library top 1"), "{pair:?}");
                draws += 1;
            }
        }
    }
    assert!(draws > 100, "{draws}");

    let revealed: Vec<&String> = games[3]
        .iter()
        .skip_while(|s| !s.contains("triggered of") || !s.contains("\"Cavalier of Thorns\""))
        .filter(|s| s.starts_with("new card ") && s.contains(" owner \"Opponent\" in library"))
        .take(5)
        .collect();
    // Each line without `new card #N `.
    let tails: Vec<&str> = revealed
        .iter()
        .filter_map(|s| s.strip_prefix("new card #"))
        .filter_map(|s| s.split_once(' ').map(|(_, rest)| rest))
        .collect();
    assert_eq!(
        tails,
        vec![
            "\"Evendo, Waking Haven\" owner \"Opponent\" in library top 4",
            "\"Old-Growth Troll\" owner \"Opponent\" in library top 3",
            "\"Storm the Festival\" owner \"Opponent\" in library top 3",
            "\"Forest\" owner \"Opponent\" in library top 1",
            "\"Elvish Mystic\" owner \"Opponent\" in library top 1",
        ]
    );

    let three_games: Vec<&String> = games[..3].iter().flatten().collect();
    let searched: Vec<&String> = three_games
        .windows(3)
        .filter(|w| {
            w[0].starts_with("new card ")
                && w[1].contains(" reveal by #")
                && w[2].contains(" move library -> hand by #")
        })
        .map(|w| w[0])
        .collect();
    assert!(!searched.is_empty());
    for line in searched {
        assert!(line.ends_with(" in library"), "{line}");
    }
}

/// Spec 6.5: the opponent's two top cards, one taken into their hand and
/// one put on the bottom (a Sleight of Hand), get no position: which hidden
/// card was which is not the perspective player's to know.
#[test]
fn hidden_library_orders_get_no_position() {
    let full = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[1],
      "gameStateMessage":{"type":"GameStateType_Full","gameStateId":1,
        "players":[{"systemSeatNumber":1,"lifeTotal":20},{"systemSeatNumber":2,"lifeTotal":20}],
        "turnInfo":{"turnNumber":1,"activePlayer":1,"phase":"Phase_Main1","priorityPlayer":1},
        "zones":[
          {"zoneId":29,"type":"ZoneType_Exile","objectInstanceIds":[310,311]},
          {"zoneId":32,"type":"ZoneType_Library","ownerSeatId":1,"objectInstanceIds":[161,162,163,164]},
          {"zoneId":35,"type":"ZoneType_Hand","ownerSeatId":2,"objectInstanceIds":[]},
          {"zoneId":36,"type":"ZoneType_Library","ownerSeatId":2,"objectInstanceIds":[171,172,173]}],
        "gameObjects":[
          {"instanceId":310,"grpId":95190,"type":"GameObjectType_Card","zoneId":29,"visibility":"Visibility_Public","ownerSeatId":1,"controllerSeatId":1,"cardTypes":["CardType_Land"]},
          {"instanceId":311,"grpId":95190,"type":"GameObjectType_Card","zoneId":29,"visibility":"Visibility_Public","ownerSeatId":1,"controllerSeatId":1,"cardTypes":["CardType_Land"]},
          {"instanceId":171,"type":"GameObjectType_Card","zoneId":36,"visibility":"Visibility_Hidden","ownerSeatId":2,"controllerSeatId":2},
          {"instanceId":172,"type":"GameObjectType_Card","zoneId":36,"visibility":"Visibility_Hidden","ownerSeatId":2,"controllerSeatId":2}]}}"#;
    let sleight = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[1],
      "gameStateMessage":{"type":"GameStateType_Diff","gameStateId":2,"prevGameStateId":1,
        "turnInfo":{"turnNumber":1,"activePlayer":1,"phase":"Phase_Main1","priorityPlayer":1},
        "zones":[
          {"zoneId":35,"type":"ZoneType_Hand","ownerSeatId":2,"objectInstanceIds":[171]},
          {"zoneId":36,"type":"ZoneType_Library","ownerSeatId":2,"objectInstanceIds":[173,172]}],
        "annotations":[
          {"id":301,"affectedIds":[171],"type":["AnnotationType_ZoneTransfer"],
            "details":[{"key":"zone_src","valueInt32":[36]},{"key":"zone_dest","valueInt32":[35]},{"key":"category","valueString":["Put"]}]},
          {"id":302,"affectedIds":[172],"type":["AnnotationType_ZoneTransfer"],
            "details":[{"key":"zone_src","valueInt32":[36]},{"key":"zone_dest","valueInt32":[36]},{"key":"category","valueString":["Put"]}]}]}}"#;
    let next_turn = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[1],
      "gameStateMessage":{"type":"GameStateType_Diff","gameStateId":3,"prevGameStateId":2,
        "turnInfo":{"turnNumber":2,"activePlayer":2,"phase":"Phase_Beginning","step":"Step_Upkeep","priorityPlayer":2},
        "annotations":[
          {"id":401,"affectorId":2,"affectedIds":[2],"type":["AnnotationType_NewTurnStarted"]},
          {"id":402,"affectedIds":[2],"type":["AnnotationType_PhaseOrStepModified"],
            "details":[{"key":"phase","valueInt32":[1]},{"key":"step","valueInt32":[1]}]}]}}"#;
    let record = match_record(
        &synthetic_match(&[full, sleight, next_turn]),
        &fixture_cards(),
    )
    .expect("card data is complete");
    let statements = statements_by_game(&record).remove(0);
    let hidden: Vec<&String> = statements
        .iter()
        .filter(|s| s.starts_with("new hidden ") && s.contains(" owner \"Opponent\" in library"))
        .filter(|s| {
            !statements.iter().any(|o| {
                o.ends_with("as draw by rule:opening-hand")
                    && id_at(o) == id_at(s.strip_prefix("new hidden ").unwrap_or(""))
            })
        })
        .collect();
    assert_eq!(hidden.len(), 2, "{statements:?}");
    for line in hidden {
        assert!(line.ends_with(" in library"), "{line}");
    }
    assert!(
        statements
            .iter()
            .any(|s| s.ends_with(" move library -> library bottom by ?")),
        "{statements:?}"
    );
}

/// Where a card was before it left a library is known only when the
/// perspective player saw it (spec 6.5): the opponent's card taken from a
/// private look, into their hand or their graveyard (surveil), has no
/// origin position, even when Arena moves just that one card; and a search
/// that happens to take the top two cards (the rest of the library then
/// shuffled) gives none either, for any player.
#[test]
fn private_looks_and_searches_give_no_origin() {
    let full = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[1],
      "gameStateMessage":{"type":"GameStateType_Full","gameStateId":1,
        "players":[{"systemSeatNumber":1,"lifeTotal":20},{"systemSeatNumber":2,"lifeTotal":20}],
        "turnInfo":{"turnNumber":1,"activePlayer":1,"phase":"Phase_Main1","priorityPlayer":1},
        "zones":[
          {"zoneId":31,"type":"ZoneType_Hand","ownerSeatId":1,"objectInstanceIds":[]},
          {"zoneId":32,"type":"ZoneType_Library","ownerSeatId":1,"objectInstanceIds":[161,162,163,164,165,166]},
          {"zoneId":35,"type":"ZoneType_Hand","ownerSeatId":2,"objectInstanceIds":[]},
          {"zoneId":36,"type":"ZoneType_Library","ownerSeatId":2,"objectInstanceIds":[171,172,173,174]},
          {"zoneId":37,"type":"ZoneType_Graveyard","ownerSeatId":2,"objectInstanceIds":[]}],
        "gameObjects":[
          {"instanceId":161,"grpId":95190,"type":"GameObjectType_Card","zoneId":32,"visibility":"Visibility_Private","ownerSeatId":1,"controllerSeatId":1,"cardTypes":["CardType_Land"]},
          {"instanceId":162,"grpId":95190,"type":"GameObjectType_Card","zoneId":32,"visibility":"Visibility_Private","ownerSeatId":1,"controllerSeatId":1,"cardTypes":["CardType_Land"]},
          {"instanceId":171,"type":"GameObjectType_Card","zoneId":36,"visibility":"Visibility_Hidden","ownerSeatId":2,"controllerSeatId":2},
          {"instanceId":172,"grpId":95190,"type":"GameObjectType_Card","zoneId":36,"visibility":"Visibility_Hidden","ownerSeatId":2,"controllerSeatId":2,"cardTypes":["CardType_Land"]}]}}"#;
    // The opponent looks at the top two and takes one into their hand.
    let to_hand = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[1],
      "gameStateMessage":{"type":"GameStateType_Diff","gameStateId":2,"prevGameStateId":1,
        "turnInfo":{"turnNumber":1,"activePlayer":1,"phase":"Phase_Main1","priorityPlayer":1},
        "zones":[
          {"zoneId":35,"type":"ZoneType_Hand","ownerSeatId":2,"objectInstanceIds":[171]},
          {"zoneId":36,"type":"ZoneType_Library","ownerSeatId":2,"objectInstanceIds":[172,173,174]}],
        "annotations":[
          {"id":201,"affectedIds":[171],"type":["AnnotationType_ZoneTransfer"],
            "details":[{"key":"zone_src","valueInt32":[36]},{"key":"zone_dest","valueInt32":[35]},{"key":"category","valueString":["Put"]}]}]}}"#;
    // Then surveils one into their graveyard.
    let surveil = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[1],
      "gameStateMessage":{"type":"GameStateType_Diff","gameStateId":3,"prevGameStateId":2,
        "turnInfo":{"turnNumber":1,"activePlayer":1,"phase":"Phase_Main1","priorityPlayer":1},
        "zones":[
          {"zoneId":36,"type":"ZoneType_Library","ownerSeatId":2,"objectInstanceIds":[173,174]},
          {"zoneId":37,"type":"ZoneType_Graveyard","ownerSeatId":2,"objectInstanceIds":[172]}],
        "gameObjects":[
          {"instanceId":172,"grpId":95190,"type":"GameObjectType_Card","zoneId":37,"visibility":"Visibility_Public","ownerSeatId":2,"controllerSeatId":2,"cardTypes":["CardType_Land"]}],
        "annotations":[
          {"id":301,"affectedIds":[172],"type":["AnnotationType_ZoneTransfer"],
            "details":[{"key":"zone_src","valueInt32":[36]},{"key":"zone_dest","valueInt32":[37]},{"key":"category","valueString":["Surveil"]}]}]}}"#;
    // The local player searches for two cards that were on top, then shuffles.
    let search = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[1],
      "gameStateMessage":{"type":"GameStateType_Diff","gameStateId":4,"prevGameStateId":3,
        "turnInfo":{"turnNumber":1,"activePlayer":1,"phase":"Phase_Main1","priorityPlayer":1},
        "zones":[
          {"zoneId":31,"type":"ZoneType_Hand","ownerSeatId":1,"objectInstanceIds":[161,162]},
          {"zoneId":32,"type":"ZoneType_Library","ownerSeatId":1,"objectInstanceIds":[190,191,192,193]}],
        "annotations":[
          {"id":401,"affectedIds":[161],"type":["AnnotationType_ZoneTransfer"],
            "details":[{"key":"zone_src","valueInt32":[32]},{"key":"zone_dest","valueInt32":[31]},{"key":"category","valueString":["Put"]}]},
          {"id":402,"affectedIds":[162],"type":["AnnotationType_ZoneTransfer"],
            "details":[{"key":"zone_src","valueInt32":[32]},{"key":"zone_dest","valueInt32":[31]},{"key":"category","valueString":["Put"]}]},
          {"id":403,"affectedIds":[1],"type":["AnnotationType_Shuffle"],
            "details":[{"key":"OldIds","valueInt32":[163,164,165,166]},{"key":"NewIds","valueInt32":[190,191,192,193]}]}]}}"#;
    let record = match_record(
        &synthetic_match(&[full, to_hand, surveil, search]),
        &fixture_cards(),
    )
    .expect("card data is complete");
    let statements = statements_by_game(&record).remove(0);
    let introduced: Vec<&String> = statements
        .iter()
        .filter(|s| s.starts_with("new ") && s.contains(" in library"))
        .collect();
    assert_eq!(
        introduced
            .iter()
            .map(|s| s.split_once(" owner ").map(|(_, rest)| rest).unwrap_or(""))
            .collect::<Vec<_>>(),
        vec![
            "\"Opponent\" in library",
            "\"Opponent\" in library",
            "\"Player\" in library",
            "\"Player\" in library",
        ],
        "{statements:?}"
    );
}

/// Issue #843: Arena reports a search's `ZoneTransfer` one message before
/// the `Shuffle` of the rest of the library (`bo1-wins-and-concede` gsids
/// 259 and 260), or later still when each player searches (gsids 322, then
/// 324 and 325). A searched card that happened to be on top (or at the
/// bottom) gets no origin position: where it was is a coincidence.
#[test]
fn searched_cards_shuffled_in_a_later_message_give_no_origin() {
    let full = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[1],
      "gameStateMessage":{"type":"GameStateType_Full","gameStateId":1,
        "players":[{"systemSeatNumber":1,"lifeTotal":20},{"systemSeatNumber":2,"lifeTotal":20}],
        "turnInfo":{"turnNumber":1,"activePlayer":1,"phase":"Phase_Main1","priorityPlayer":1},
        "zones":[
          {"zoneId":31,"type":"ZoneType_Hand","ownerSeatId":1,"objectInstanceIds":[]},
          {"zoneId":32,"type":"ZoneType_Library","ownerSeatId":1,"objectInstanceIds":[161,162,163,164,165,166]},
          {"zoneId":35,"type":"ZoneType_Hand","ownerSeatId":2,"objectInstanceIds":[]},
          {"zoneId":36,"type":"ZoneType_Library","ownerSeatId":2,"objectInstanceIds":[171,172,173,174]}],
        "gameObjects":[
          {"instanceId":161,"grpId":95190,"type":"GameObjectType_Card","zoneId":32,"visibility":"Visibility_Private","ownerSeatId":1,"controllerSeatId":1,"cardTypes":["CardType_Land"]},
          {"instanceId":166,"grpId":95190,"type":"GameObjectType_Card","zoneId":32,"visibility":"Visibility_Private","ownerSeatId":1,"controllerSeatId":1,"cardTypes":["CardType_Land"]}]}}"#;
    // The local player searches for their top card.
    let search_top = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[1],
      "gameStateMessage":{"type":"GameStateType_Diff","gameStateId":2,"prevGameStateId":1,
        "turnInfo":{"turnNumber":1,"activePlayer":1,"phase":"Phase_Main1","priorityPlayer":1},
        "zones":[
          {"zoneId":31,"type":"ZoneType_Hand","ownerSeatId":1,"objectInstanceIds":[161]},
          {"zoneId":32,"type":"ZoneType_Library","ownerSeatId":1,"objectInstanceIds":[162,163,164,165,166]}],
        "annotations":[
          {"id":201,"affectedIds":[161],"type":["AnnotationType_ZoneTransfer"],
            "details":[{"key":"zone_src","valueInt32":[32]},{"key":"zone_dest","valueInt32":[31]},{"key":"category","valueString":["Put"]}]}]}}"#;
    // Then shuffles, in the next message.
    let shuffle = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[1],
      "gameStateMessage":{"type":"GameStateType_Diff","gameStateId":3,"prevGameStateId":2,
        "turnInfo":{"turnNumber":1,"activePlayer":1,"phase":"Phase_Main1","priorityPlayer":1},
        "zones":[
          {"zoneId":32,"type":"ZoneType_Library","ownerSeatId":1,"objectInstanceIds":[190,191,192,193,194]}],
        "annotations":[
          {"id":301,"affectedIds":[1],"type":["AnnotationType_Shuffle"],
            "details":[{"key":"OldIds","valueInt32":[162,163,164,165,166]},{"key":"NewIds","valueInt32":[190,191,192,193,194]}]}]}}"#;
    // The local player searches for their bottom card.
    let search_bottom = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[1],
      "gameStateMessage":{"type":"GameStateType_Diff","gameStateId":4,"prevGameStateId":3,
        "turnInfo":{"turnNumber":1,"activePlayer":1,"phase":"Phase_Main1","priorityPlayer":1},
        "zones":[
          {"zoneId":31,"type":"ZoneType_Hand","ownerSeatId":1,"objectInstanceIds":[161,194]},
          {"zoneId":32,"type":"ZoneType_Library","ownerSeatId":1,"objectInstanceIds":[190,191,192,193]}],
        "gameObjects":[
          {"instanceId":194,"grpId":95190,"type":"GameObjectType_Card","zoneId":31,"visibility":"Visibility_Private","ownerSeatId":1,"controllerSeatId":1,"cardTypes":["CardType_Land"]}],
        "annotations":[
          {"id":401,"affectedIds":[194],"type":["AnnotationType_ZoneTransfer"],
            "details":[{"key":"zone_src","valueInt32":[32]},{"key":"zone_dest","valueInt32":[31]},{"key":"category","valueString":["Put"]}]}]}}"#;
    // A message with no annotations, then the shuffle, as when each player
    // searches and the shuffles follow one by one.
    let quiet = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[1],
      "gameStateMessage":{"type":"GameStateType_Diff","gameStateId":5,"prevGameStateId":4,
        "turnInfo":{"turnNumber":1,"activePlayer":1,"phase":"Phase_Main1","priorityPlayer":1}}}"#;
    let shuffle_later = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[1],
      "gameStateMessage":{"type":"GameStateType_Diff","gameStateId":6,"prevGameStateId":5,
        "turnInfo":{"turnNumber":1,"activePlayer":1,"phase":"Phase_Main1","priorityPlayer":1},
        "zones":[
          {"zoneId":32,"type":"ZoneType_Library","ownerSeatId":1,"objectInstanceIds":[195,196,197,198]}],
        "annotations":[
          {"id":601,"affectedIds":[1],"type":["AnnotationType_Shuffle"],
            "details":[{"key":"OldIds","valueInt32":[190,191,192,193]},{"key":"NewIds","valueInt32":[195,196,197,198]}]}]}}"#;
    let record = match_record(
        &synthetic_match(&[
            full,
            search_top,
            shuffle,
            search_bottom,
            quiet,
            shuffle_later,
        ]),
        &fixture_cards(),
    )
    .expect("card data is complete");
    let statements = statements_by_game(&record).remove(0);
    let introduced: Vec<&String> = statements
        .iter()
        .filter(|s| s.starts_with("new ") && s.contains(" in library"))
        .collect();
    assert_eq!(introduced.len(), 2, "{statements:?}");
    assert!(
        introduced.iter().all(|s| s.ends_with(" in library")),
        "{statements:?}"
    );
}

/// Issue #599: the opponent looks at their top three cards, puts two into
/// their graveyard and keeps the third on top. Arena reports no look, and
/// the cards were never revealed in the library, so where the two public
/// cards were (together at the top, in that order) is not the perspective
/// player's to know: no origin positions. A card revealed on top of the
/// library (explore) and then put into a graveyard on its own keeps its
/// `top 1` (#602, Task 8b).
#[test]
fn opponent_cards_from_a_hidden_look_get_no_origin() {
    let full = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[1],
      "gameStateMessage":{"type":"GameStateType_Full","gameStateId":1,
        "players":[{"systemSeatNumber":1,"lifeTotal":20},{"systemSeatNumber":2,"lifeTotal":20}],
        "turnInfo":{"turnNumber":1,"activePlayer":1,"phase":"Phase_Main1","priorityPlayer":1},
        "zones":[
          {"zoneId":32,"type":"ZoneType_Library","ownerSeatId":1,"objectInstanceIds":[161,162]},
          {"zoneId":36,"type":"ZoneType_Library","ownerSeatId":2,"objectInstanceIds":[171,172,173,174,175]},
          {"zoneId":37,"type":"ZoneType_Graveyard","ownerSeatId":2,"objectInstanceIds":[]}],
        "gameObjects":[
          {"instanceId":171,"type":"GameObjectType_Card","zoneId":36,"visibility":"Visibility_Hidden","ownerSeatId":2,"controllerSeatId":2},
          {"instanceId":172,"type":"GameObjectType_Card","zoneId":36,"visibility":"Visibility_Hidden","ownerSeatId":2,"controllerSeatId":2},
          {"instanceId":173,"type":"GameObjectType_Card","zoneId":36,"visibility":"Visibility_Hidden","ownerSeatId":2,"controllerSeatId":2}]}}"#;
    // The look: the top two into the graveyard, the third stays on top.
    let look = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[1],
      "gameStateMessage":{"type":"GameStateType_Diff","gameStateId":2,"prevGameStateId":1,
        "turnInfo":{"turnNumber":1,"activePlayer":1,"phase":"Phase_Main1","priorityPlayer":1},
        "zones":[
          {"zoneId":36,"type":"ZoneType_Library","ownerSeatId":2,"objectInstanceIds":[173,174,175]},
          {"zoneId":37,"type":"ZoneType_Graveyard","ownerSeatId":2,"objectInstanceIds":[171,172]}],
        "gameObjects":[
          {"instanceId":171,"grpId":95190,"type":"GameObjectType_Card","zoneId":37,"visibility":"Visibility_Public","ownerSeatId":2,"controllerSeatId":2,"cardTypes":["CardType_Land"]},
          {"instanceId":172,"grpId":95190,"type":"GameObjectType_Card","zoneId":37,"visibility":"Visibility_Public","ownerSeatId":2,"controllerSeatId":2,"cardTypes":["CardType_Land"]}],
        "annotations":[
          {"id":201,"affectedIds":[171],"type":["AnnotationType_ZoneTransfer"],
            "details":[{"key":"zone_src","valueInt32":[36]},{"key":"zone_dest","valueInt32":[37]},{"key":"category","valueString":["Put"]}]},
          {"id":202,"affectedIds":[172],"type":["AnnotationType_ZoneTransfer"],
            "details":[{"key":"zone_src","valueInt32":[36]},{"key":"zone_dest","valueInt32":[37]},{"key":"category","valueString":["Put"]}]}]}}"#;
    // Explore: the top card is revealed in the library, then put into the
    // graveyard.
    let reveal = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[1],
      "gameStateMessage":{"type":"GameStateType_Diff","gameStateId":3,"prevGameStateId":2,
        "turnInfo":{"turnNumber":1,"activePlayer":1,"phase":"Phase_Main1","priorityPlayer":1},
        "gameObjects":[
          {"instanceId":173,"grpId":95190,"type":"GameObjectType_Card","zoneId":36,"visibility":"Visibility_Public","ownerSeatId":2,"controllerSeatId":2,"cardTypes":["CardType_Land"]}]}}"#;
    let explore = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[1],
      "gameStateMessage":{"type":"GameStateType_Diff","gameStateId":4,"prevGameStateId":3,
        "turnInfo":{"turnNumber":1,"activePlayer":1,"phase":"Phase_Main1","priorityPlayer":1},
        "zones":[
          {"zoneId":36,"type":"ZoneType_Library","ownerSeatId":2,"objectInstanceIds":[174,175]},
          {"zoneId":37,"type":"ZoneType_Graveyard","ownerSeatId":2,"objectInstanceIds":[171,172,173]}],
        "gameObjects":[
          {"instanceId":173,"grpId":95190,"type":"GameObjectType_Card","zoneId":37,"visibility":"Visibility_Public","ownerSeatId":2,"controllerSeatId":2,"cardTypes":["CardType_Land"]}],
        "annotations":[
          {"id":401,"affectedIds":[173],"type":["AnnotationType_ZoneTransfer"],
            "details":[{"key":"zone_src","valueInt32":[36]},{"key":"zone_dest","valueInt32":[37]},{"key":"category","valueString":["Put"]}]}]}}"#;
    let record = match_record(
        &synthetic_match(&[full, look, reveal, explore]),
        &fixture_cards(),
    )
    .expect("card data is complete");
    let statements = statements_by_game(&record).remove(0);
    let introduced: Vec<&str> = statements
        .iter()
        .filter(|s| s.starts_with("new card ") && s.contains(" in library"))
        .filter_map(|s| s.split_once(" owner ").map(|(_, rest)| rest))
        .collect();
    assert_eq!(
        introduced,
        vec![
            "\"Opponent\" in library",
            "\"Opponent\" in library",
            "\"Opponent\" in library top 1",
        ],
        "{statements:?}"
    );
}

/// Decision 0045: a card put into a library at a known place other than
/// an end (Teferi, Hero of Dominaria's "third from the top"; Arena's list
/// shows the place) is `top N`, and keyframes give it that position. With
/// fewer than N other cards, Arena puts it on the bottom (CR 401.7), and
/// the move is `bottom`.
#[test]
fn cards_put_nth_from_the_top_take_top_n() {
    let full = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[1],
      "gameStateMessage":{"type":"GameStateType_Full","gameStateId":1,
        "players":[{"systemSeatNumber":1,"lifeTotal":20},{"systemSeatNumber":2,"lifeTotal":20}],
        "turnInfo":{"turnNumber":1,"activePlayer":1,"phase":"Phase_Main1","priorityPlayer":1},
        "zones":[
          {"zoneId":31,"type":"ZoneType_Hand","ownerSeatId":1,"objectInstanceIds":[150]},
          {"zoneId":32,"type":"ZoneType_Library","ownerSeatId":1,"objectInstanceIds":[161,162,163,164]},
          {"zoneId":35,"type":"ZoneType_Hand","ownerSeatId":2,"objectInstanceIds":[175]},
          {"zoneId":36,"type":"ZoneType_Library","ownerSeatId":2,"objectInstanceIds":[171]}],
        "gameObjects":[
          {"instanceId":150,"grpId":95190,"type":"GameObjectType_Card","zoneId":31,"visibility":"Visibility_Private","ownerSeatId":1,"controllerSeatId":1,"cardTypes":["CardType_Land"]},
          {"instanceId":175,"type":"GameObjectType_Card","zoneId":35,"visibility":"Visibility_Hidden","ownerSeatId":2,"controllerSeatId":2}]}}"#;
    let third = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[1],
      "gameStateMessage":{"type":"GameStateType_Diff","gameStateId":2,"prevGameStateId":1,
        "turnInfo":{"turnNumber":1,"activePlayer":1,"phase":"Phase_Main1","priorityPlayer":1},
        "zones":[
          {"zoneId":31,"type":"ZoneType_Hand","ownerSeatId":1,"objectInstanceIds":[]},
          {"zoneId":32,"type":"ZoneType_Library","ownerSeatId":1,"objectInstanceIds":[161,162,150,163,164]}],
        "gameObjects":[
          {"instanceId":150,"grpId":95190,"type":"GameObjectType_Card","zoneId":32,"visibility":"Visibility_Private","ownerSeatId":1,"controllerSeatId":1,"cardTypes":["CardType_Land"]}],
        "annotations":[
          {"id":201,"affectedIds":[150],"type":["AnnotationType_ZoneTransfer"],
            "details":[{"key":"zone_src","valueInt32":[31]},{"key":"zone_dest","valueInt32":[32]},{"key":"category","valueString":["Put"]}]}]}}"#;
    // The opponent's library has one card: "third from the top" is the bottom.
    let too_few = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[1],
      "gameStateMessage":{"type":"GameStateType_Diff","gameStateId":3,"prevGameStateId":2,
        "turnInfo":{"turnNumber":1,"activePlayer":1,"phase":"Phase_Main1","priorityPlayer":1},
        "zones":[
          {"zoneId":35,"type":"ZoneType_Hand","ownerSeatId":2,"objectInstanceIds":[]},
          {"zoneId":36,"type":"ZoneType_Library","ownerSeatId":2,"objectInstanceIds":[171,175]}],
        "annotations":[
          {"id":301,"affectedIds":[175],"type":["AnnotationType_ZoneTransfer"],
            "details":[{"key":"zone_src","valueInt32":[35]},{"key":"zone_dest","valueInt32":[36]},{"key":"category","valueString":["Put"]}]}]}}"#;
    let next_turn = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[1],
      "gameStateMessage":{"type":"GameStateType_Diff","gameStateId":4,"prevGameStateId":3,
        "turnInfo":{"turnNumber":2,"activePlayer":2,"phase":"Phase_Beginning","step":"Step_Upkeep","priorityPlayer":2},
        "annotations":[
          {"id":401,"affectorId":2,"affectedIds":[2],"type":["AnnotationType_NewTurnStarted"]},
          {"id":402,"affectedIds":[2],"type":["AnnotationType_PhaseOrStepModified"],
            "details":[{"key":"phase","valueInt32":[1]},{"key":"step","valueInt32":[1]}]}]}}"#;
    let record = match_record(
        &synthetic_match(&[full, third, too_few, next_turn]),
        &fixture_cards(),
    )
    .expect("card data is complete");
    let statements = statements_by_game(&record).remove(0);
    let put = statements
        .iter()
        .find(|s| s.contains(" \"Forest\" move hand -> library"))
        .expect("the Forest is put into the library");
    assert!(put.ends_with(" move hand -> library top 3 by ?"), "{put}");
    let forest = id_at(put).expect("an id");
    let keyframe = statements
        .iter()
        .rposition(|s| s == "keyframe")
        .expect("turn 2's keyframe");
    assert!(
        statements[keyframe..].contains(&format!("#{forest} \"Forest\" library \"Player\" top 3")),
        "{statements:?}"
    );
    assert!(
        statements
            .iter()
            .any(|s| s.ends_with(" move hand -> library bottom by ?")),
        "{statements:?}"
    );
}

/// Decision 0047: two or more cards put on the bottom together in a random
/// order (Arena moves them, then renumbers just them with a `Shuffle`) are
/// each written `bottom ?`, and no keyframe gives them a position. A single
/// card put there in a random order is plain `bottom`.
#[test]
fn random_bottom_is_written_bottom_unknown() {
    let full = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[1],
      "gameStateMessage":{"type":"GameStateType_Full","gameStateId":1,
        "players":[{"systemSeatNumber":1,"lifeTotal":20},{"systemSeatNumber":2,"lifeTotal":20}],
        "turnInfo":{"turnNumber":1,"activePlayer":1,"phase":"Phase_Main1","priorityPlayer":1},
        "zones":[
          {"zoneId":29,"type":"ZoneType_Exile","objectInstanceIds":[310,311,312]},
          {"zoneId":32,"type":"ZoneType_Library","ownerSeatId":1,"objectInstanceIds":[161,162,163,164]}],
        "gameObjects":[
          {"instanceId":310,"grpId":95190,"type":"GameObjectType_Card","zoneId":29,"visibility":"Visibility_Public","ownerSeatId":1,"controllerSeatId":1,"cardTypes":["CardType_Land"]},
          {"instanceId":311,"grpId":95190,"type":"GameObjectType_Card","zoneId":29,"visibility":"Visibility_Public","ownerSeatId":1,"controllerSeatId":1,"cardTypes":["CardType_Land"]},
          {"instanceId":312,"grpId":95190,"type":"GameObjectType_Card","zoneId":29,"visibility":"Visibility_Public","ownerSeatId":1,"controllerSeatId":1,"cardTypes":["CardType_Land"]}]}}"#;
    let two = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[1],
      "gameStateMessage":{"type":"GameStateType_Diff","gameStateId":2,"prevGameStateId":1,
        "turnInfo":{"turnNumber":1,"activePlayer":1,"phase":"Phase_Main1","priorityPlayer":1},
        "zones":[
          {"zoneId":29,"type":"ZoneType_Exile","objectInstanceIds":[312]},
          {"zoneId":32,"type":"ZoneType_Library","ownerSeatId":1,"objectInstanceIds":[161,162,163,164,320,321]}],
        "gameObjects":[
          {"instanceId":310,"grpId":95190,"type":"GameObjectType_Card","zoneId":32,"visibility":"Visibility_Private","ownerSeatId":1,"controllerSeatId":1,"cardTypes":["CardType_Land"]},
          {"instanceId":311,"grpId":95190,"type":"GameObjectType_Card","zoneId":32,"visibility":"Visibility_Private","ownerSeatId":1,"controllerSeatId":1,"cardTypes":["CardType_Land"]}],
        "annotations":[
          {"id":201,"affectedIds":[310],"type":["AnnotationType_ZoneTransfer"],
            "details":[{"key":"zone_src","valueInt32":[29]},{"key":"zone_dest","valueInt32":[32]},{"key":"category","valueString":["Put"]}]},
          {"id":202,"affectedIds":[311],"type":["AnnotationType_ZoneTransfer"],
            "details":[{"key":"zone_src","valueInt32":[29]},{"key":"zone_dest","valueInt32":[32]},{"key":"category","valueString":["Put"]}]},
          {"id":203,"affectedIds":[1],"type":["AnnotationType_Shuffle"],
            "details":[{"key":"OldIds","valueInt32":[310,311]},{"key":"NewIds","valueInt32":[320,321]}]}]}}"#;
    let one = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[1],
      "gameStateMessage":{"type":"GameStateType_Diff","gameStateId":3,"prevGameStateId":2,
        "turnInfo":{"turnNumber":1,"activePlayer":1,"phase":"Phase_Main1","priorityPlayer":1},
        "zones":[
          {"zoneId":29,"type":"ZoneType_Exile","objectInstanceIds":[]},
          {"zoneId":32,"type":"ZoneType_Library","ownerSeatId":1,"objectInstanceIds":[161,162,163,164,320,321,322]}],
        "gameObjects":[
          {"instanceId":312,"grpId":95190,"type":"GameObjectType_Card","zoneId":32,"visibility":"Visibility_Private","ownerSeatId":1,"controllerSeatId":1,"cardTypes":["CardType_Land"]}],
        "annotations":[
          {"id":301,"affectedIds":[312],"type":["AnnotationType_ZoneTransfer"],
            "details":[{"key":"zone_src","valueInt32":[29]},{"key":"zone_dest","valueInt32":[32]},{"key":"category","valueString":["Put"]}]},
          {"id":302,"affectedIds":[1],"type":["AnnotationType_Shuffle"],
            "details":[{"key":"OldIds","valueInt32":[312]},{"key":"NewIds","valueInt32":[322]}]}]}}"#;
    let next_turn = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[1],
      "gameStateMessage":{"type":"GameStateType_Diff","gameStateId":4,"prevGameStateId":3,
        "turnInfo":{"turnNumber":2,"activePlayer":2,"phase":"Phase_Beginning","step":"Step_Upkeep","priorityPlayer":2},
        "annotations":[
          {"id":401,"affectorId":2,"affectedIds":[2],"type":["AnnotationType_NewTurnStarted"]},
          {"id":402,"affectedIds":[2],"type":["AnnotationType_PhaseOrStepModified"],
            "details":[{"key":"phase","valueInt32":[1]},{"key":"step","valueInt32":[1]}]}]}}"#;
    let record = match_record(
        &synthetic_match(&[full, two, one, next_turn]),
        &fixture_cards(),
    )
    .expect("card data is complete");
    let statements = statements_by_game(&record).remove(0);
    let moves: Vec<&String> = statements
        .iter()
        .filter(|s| s.contains(" \"Forest\" move exile -> library"))
        .collect();
    assert_eq!(moves.len(), 3, "{statements:?}");
    assert!(moves[0].ends_with(" library bottom ? by ?"), "{}", moves[0]);
    assert!(moves[1].ends_with(" library bottom ? by ?"), "{}", moves[1]);
    assert!(moves[2].ends_with(" library bottom by ?"), "{}", moves[2]);
    let keyframe = statements
        .iter()
        .rposition(|s| s == "keyframe")
        .expect("turn 2's keyframe");
    for line in &moves[..2] {
        let id = id_at(line).expect("an id");
        assert!(
            !statements[keyframe..]
                .iter()
                .any(|s| mentions(s, id) && (s.contains(" top ") || s.contains(" bottom "))),
            "{statements:?}"
        );
    }
    // The single card keeps its known place, and its identity (#602, Task
    // 8c).
    let single = id_at(moves[2]).expect("an id");
    assert!(
        !statements
            .iter()
            .any(|s| mentions(s, single) && s.ends_with(" untracked")),
        "{statements:?}"
    );
    assert!(
        statements[keyframe..]
            .iter()
            .any(|s| mentions(s, single) && s.ends_with(" library \"Player\" bottom 1")),
        "{statements:?}"
    );
}

/// #602, Task 7: cards put on the bottom in a random order are no shuffle
/// even when they outnumber the library they join (one card here).
#[test]
fn random_bottom_of_a_tiny_library_is_no_shuffle() {
    let full = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[1],
      "gameStateMessage":{"type":"GameStateType_Full","gameStateId":1,
        "players":[{"systemSeatNumber":1,"lifeTotal":20},{"systemSeatNumber":2,"lifeTotal":20}],
        "turnInfo":{"turnNumber":1,"activePlayer":1,"phase":"Phase_Main1","priorityPlayer":1},
        "zones":[
          {"zoneId":29,"type":"ZoneType_Exile","objectInstanceIds":[310,311]},
          {"zoneId":32,"type":"ZoneType_Library","ownerSeatId":1,"objectInstanceIds":[161]}],
        "gameObjects":[
          {"instanceId":310,"grpId":95190,"type":"GameObjectType_Card","zoneId":29,"visibility":"Visibility_Public","ownerSeatId":1,"controllerSeatId":1,"cardTypes":["CardType_Land"]},
          {"instanceId":311,"grpId":95190,"type":"GameObjectType_Card","zoneId":29,"visibility":"Visibility_Public","ownerSeatId":1,"controllerSeatId":1,"cardTypes":["CardType_Land"]}]}}"#;
    // Two cards on the bottom of a one-card library.
    let two = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[1],
      "gameStateMessage":{"type":"GameStateType_Diff","gameStateId":2,"prevGameStateId":1,
        "turnInfo":{"turnNumber":1,"activePlayer":1,"phase":"Phase_Main1","priorityPlayer":1},
        "zones":[
          {"zoneId":29,"type":"ZoneType_Exile","objectInstanceIds":[]},
          {"zoneId":32,"type":"ZoneType_Library","ownerSeatId":1,"objectInstanceIds":[161,320,321]}],
        "gameObjects":[
          {"instanceId":310,"grpId":95190,"type":"GameObjectType_Card","zoneId":32,"visibility":"Visibility_Private","ownerSeatId":1,"controllerSeatId":1,"cardTypes":["CardType_Land"]},
          {"instanceId":311,"grpId":95190,"type":"GameObjectType_Card","zoneId":32,"visibility":"Visibility_Private","ownerSeatId":1,"controllerSeatId":1,"cardTypes":["CardType_Land"]}],
        "annotations":[
          {"id":201,"affectedIds":[310],"type":["AnnotationType_ZoneTransfer"],
            "details":[{"key":"zone_src","valueInt32":[29]},{"key":"zone_dest","valueInt32":[32]},{"key":"category","valueString":["Put"]}]},
          {"id":202,"affectedIds":[311],"type":["AnnotationType_ZoneTransfer"],
            "details":[{"key":"zone_src","valueInt32":[29]},{"key":"zone_dest","valueInt32":[32]},{"key":"category","valueString":["Put"]}]},
          {"id":203,"affectedIds":[1],"type":["AnnotationType_Shuffle"],
            "details":[{"key":"OldIds","valueInt32":[310,311]},{"key":"NewIds","valueInt32":[320,321]}]}]}}"#;
    let record = match_record(&synthetic_match(&[full, two]), &fixture_cards())
        .expect("card data is complete");
    let statements = statements_by_game(&record).remove(0);
    assert!(
        !statements.iter().any(|s| s.contains(" shuffle")),
        "{statements:?}"
    );
    let moves: Vec<&String> = statements
        .iter()
        .filter(|s| s.contains(" move exile -> library"))
        .collect();
    assert_eq!(moves.len(), 2, "{statements:?}");
    for line in moves {
        assert!(line.ends_with(" library bottom ? by ?"), "{line}");
    }
}

/// Review of #602, Task 8c: the opponent's hidden card scried on top and
/// then renumbered on the bottom without a transfer ("look at the top two,
/// one into your hand, the other on the bottom") is not moved there: which
/// hidden card went to the bottom is not the perspective player's to know,
/// so it is `untracked`.
#[test]
fn opponent_hidden_card_renumbered_on_the_bottom_is_untracked() {
    let full = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[1],
      "gameStateMessage":{"type":"GameStateType_Full","gameStateId":1,
        "players":[{"systemSeatNumber":1,"lifeTotal":20},{"systemSeatNumber":2,"lifeTotal":20}],
        "turnInfo":{"turnNumber":1,"activePlayer":1,"phase":"Phase_Main1","priorityPlayer":1},
        "zones":[
          {"zoneId":35,"type":"ZoneType_Hand","ownerSeatId":2,"objectInstanceIds":[]},
          {"zoneId":36,"type":"ZoneType_Library","ownerSeatId":2,"objectInstanceIds":[171,172,173,174]}],
        "gameObjects":[
          {"instanceId":171,"type":"GameObjectType_Card","zoneId":36,"visibility":"Visibility_Hidden","ownerSeatId":2,"controllerSeatId":2},
          {"instanceId":172,"type":"GameObjectType_Card","zoneId":36,"visibility":"Visibility_Hidden","ownerSeatId":2,"controllerSeatId":2}]}}"#;
    let scry = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[1],
      "gameStateMessage":{"type":"GameStateType_Diff","gameStateId":2,"prevGameStateId":1,
        "turnInfo":{"turnNumber":1,"activePlayer":1,"phase":"Phase_Main1","priorityPlayer":1},
        "annotations":[
          {"id":201,"affectedIds":[171],"type":["AnnotationType_Scry"],
            "details":[{"key":"topIds","type":"KeyValuePairValueType_int32","valueInt32":[171]},{"key":"bottomIds"}]}]}}"#;
    let chosen = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[1],
      "gameStateMessage":{"type":"GameStateType_Diff","gameStateId":3,"prevGameStateId":2,
        "turnInfo":{"turnNumber":1,"activePlayer":1,"phase":"Phase_Main1","priorityPlayer":1},
        "zones":[
          {"zoneId":35,"type":"ZoneType_Hand","ownerSeatId":2,"objectInstanceIds":[172]},
          {"zoneId":36,"type":"ZoneType_Library","ownerSeatId":2,"objectInstanceIds":[173,174,180]}],
        "gameObjects":[
          {"instanceId":180,"type":"GameObjectType_Card","zoneId":36,"visibility":"Visibility_Hidden","ownerSeatId":2,"controllerSeatId":2}],
        "annotations":[
          {"id":301,"affectedIds":[172],"type":["AnnotationType_ZoneTransfer"],
            "details":[{"key":"zone_src","valueInt32":[36]},{"key":"zone_dest","valueInt32":[35]},{"key":"category","valueString":["Put"]}]},
          {"id":302,"affectedIds":[2],"type":["AnnotationType_Shuffle"],
            "details":[{"key":"OldIds","valueInt32":[171]},{"key":"NewIds","valueInt32":[180]}]}]}}"#;
    let record = match_record(&synthetic_match(&[full, scry, chosen]), &fixture_cards())
        .expect("card data is complete");
    let statements = statements_by_game(&record).remove(0);
    let scried = statements
        .iter()
        .find(|s| s.starts_with("new hidden ") && s.ends_with(" in library top 1"))
        .unwrap_or_else(|| panic!("{statements:?}"));
    let id = id_at(scried.strip_prefix("new hidden ").unwrap_or("")).expect("an id");
    assert!(
        !statements
            .iter()
            .any(|s| s.contains(" move library -> library")),
        "{statements:?}"
    );
    assert!(
        statements.contains(&format!("#{id} untracked")),
        "{statements:?}"
    );
}

/// #599: an opponent's card revealed and put into a public zone in the
/// same message (a `RevealedCardCreated` whose proxy is where the card
/// went) keeps its origin position; a proxy that matches no move gives
/// none.
#[test]
fn revealed_card_created_in_the_same_message_gives_a_position() {
    let full = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[1],
      "gameStateMessage":{"type":"GameStateType_Full","gameStateId":1,
        "players":[{"systemSeatNumber":1,"lifeTotal":20},{"systemSeatNumber":2,"lifeTotal":20}],
        "turnInfo":{"turnNumber":1,"activePlayer":1,"phase":"Phase_Main1","priorityPlayer":1},
        "zones":[
          {"zoneId":36,"type":"ZoneType_Library","ownerSeatId":2,"objectInstanceIds":[171,172,173]},
          {"zoneId":37,"type":"ZoneType_Graveyard","ownerSeatId":2,"objectInstanceIds":[]},
          {"zoneId":38,"type":"ZoneType_Exile","objectInstanceIds":[]}],
        "gameObjects":[
          {"instanceId":171,"type":"GameObjectType_Card","zoneId":36,"visibility":"Visibility_Hidden","ownerSeatId":2,"controllerSeatId":2},
          {"instanceId":172,"type":"GameObjectType_Card","zoneId":36,"visibility":"Visibility_Hidden","ownerSeatId":2,"controllerSeatId":2}]}}"#;
    // The top card revealed and put into the graveyard; the proxy is in
    // the graveyard with it.
    let revealed = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[1],
      "gameStateMessage":{"type":"GameStateType_Diff","gameStateId":2,"prevGameStateId":1,
        "turnInfo":{"turnNumber":1,"activePlayer":1,"phase":"Phase_Main1","priorityPlayer":1},
        "zones":[
          {"zoneId":36,"type":"ZoneType_Library","ownerSeatId":2,"objectInstanceIds":[172,173]},
          {"zoneId":37,"type":"ZoneType_Graveyard","ownerSeatId":2,"objectInstanceIds":[171,500]}],
        "gameObjects":[
          {"instanceId":171,"grpId":95190,"type":"GameObjectType_Card","zoneId":37,"visibility":"Visibility_Public","ownerSeatId":2,"controllerSeatId":2,"cardTypes":["CardType_Land"]},
          {"instanceId":500,"grpId":95190,"type":"GameObjectType_RevealedCard","zoneId":37,"visibility":"Visibility_Public","ownerSeatId":2,"controllerSeatId":2,"cardTypes":["CardType_Land"]}],
        "annotations":[
          {"id":201,"affectedIds":[171],"type":["AnnotationType_ZoneTransfer"],
            "details":[{"key":"zone_src","valueInt32":[36]},{"key":"zone_dest","valueInt32":[37]},{"key":"category","valueString":["Put"]}]},
          {"id":202,"affectorId":500,"affectedIds":[500],"type":["AnnotationType_RevealedCardCreated"]}]}}"#;
    // The next top card put into exile, while the proxy shown is in the
    // graveyard: it pairs with no move, so no position.
    let unpaired = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[1],
      "gameStateMessage":{"type":"GameStateType_Diff","gameStateId":3,"prevGameStateId":2,
        "turnInfo":{"turnNumber":1,"activePlayer":1,"phase":"Phase_Main1","priorityPlayer":1},
        "zones":[
          {"zoneId":36,"type":"ZoneType_Library","ownerSeatId":2,"objectInstanceIds":[173]},
          {"zoneId":37,"type":"ZoneType_Graveyard","ownerSeatId":2,"objectInstanceIds":[171,500,501]},
          {"zoneId":38,"type":"ZoneType_Exile","objectInstanceIds":[172]}],
        "gameObjects":[
          {"instanceId":172,"grpId":95190,"type":"GameObjectType_Card","zoneId":38,"visibility":"Visibility_Public","ownerSeatId":2,"controllerSeatId":2,"cardTypes":["CardType_Land"]},
          {"instanceId":501,"grpId":95190,"type":"GameObjectType_RevealedCard","zoneId":37,"visibility":"Visibility_Public","ownerSeatId":2,"controllerSeatId":2,"cardTypes":["CardType_Land"]}],
        "annotations":[
          {"id":301,"affectedIds":[172],"type":["AnnotationType_ZoneTransfer"],
            "details":[{"key":"zone_src","valueInt32":[36]},{"key":"zone_dest","valueInt32":[38]},{"key":"category","valueString":["Put"]}]},
          {"id":302,"affectorId":501,"affectedIds":[501],"type":["AnnotationType_RevealedCardCreated"]}]}}"#;
    let record = match_record(
        &synthetic_match(&[full, revealed, unpaired]),
        &fixture_cards(),
    )
    .expect("card data is complete");
    let statements = statements_by_game(&record).remove(0);
    let introduced: Vec<&str> = statements
        .iter()
        .filter(|s| s.starts_with("new card ") && s.contains(" in library"))
        .filter_map(|s| s.split_once(" owner ").map(|(_, rest)| rest))
        .collect();
    assert_eq!(
        introduced,
        vec!["\"Opponent\" in library top 1", "\"Opponent\" in library"],
        "{statements:?}"
    );
    // The reveal pairs with its move though the move comes first in the
    // message; only the unpaired proxy is incomplete.
    let reveals: Vec<&String> = statements
        .iter()
        .filter(|s| s.contains(" reveal ") || s.starts_with("incomplete "))
        .collect();
    assert_eq!(
        reveals,
        vec![
            "#1 \"Forest\" reveal by ?",
            "incomplete \"Arena revealed a card the record cannot tell\"",
        ],
        "{statements:?}"
    );
}

/// A reveal proxy in the opponent's hand (Arena reveals hand cards so:
/// seven proxies in `brawl/brawl_queue.log` line 3294) beside a hidden draw
/// in the same message, written before it: the hidden card is not tied to
/// the reveal by its move, so pairing them would tell which hidden object
/// the revealed card is. The record says it cannot tell instead.
#[test]
fn a_hand_reveal_does_not_name_a_hidden_draw() {
    let full = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[1],
      "gameStateMessage":{"type":"GameStateType_Full","gameStateId":1,
        "players":[{"systemSeatNumber":1,"lifeTotal":20},{"systemSeatNumber":2,"lifeTotal":20}],
        "turnInfo":{"turnNumber":1,"activePlayer":2,"phase":"Phase_Main1","priorityPlayer":2},
        "zones":[
          {"zoneId":35,"type":"ZoneType_Hand","ownerSeatId":2,"objectInstanceIds":[]},
          {"zoneId":36,"type":"ZoneType_Library","ownerSeatId":2,"objectInstanceIds":[171,172]}],
        "gameObjects":[
          {"instanceId":171,"type":"GameObjectType_Card","zoneId":36,"visibility":"Visibility_Hidden","ownerSeatId":2,"controllerSeatId":2},
          {"instanceId":172,"type":"GameObjectType_Card","zoneId":36,"visibility":"Visibility_Hidden","ownerSeatId":2,"controllerSeatId":2}]}}"#;
    let drawn_and_revealed = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[1],
      "gameStateMessage":{"type":"GameStateType_Diff","gameStateId":2,"prevGameStateId":1,
        "turnInfo":{"turnNumber":1,"activePlayer":2,"phase":"Phase_Main1","priorityPlayer":2},
        "zones":[
          {"zoneId":35,"type":"ZoneType_Hand","ownerSeatId":2,"objectInstanceIds":[171,500]},
          {"zoneId":36,"type":"ZoneType_Library","ownerSeatId":2,"objectInstanceIds":[172]}],
        "gameObjects":[
          {"instanceId":171,"type":"GameObjectType_Card","zoneId":35,"visibility":"Visibility_Hidden","ownerSeatId":2,"controllerSeatId":2},
          {"instanceId":500,"grpId":95190,"type":"GameObjectType_RevealedCard","zoneId":35,"visibility":"Visibility_Public","ownerSeatId":2,"controllerSeatId":2,"cardTypes":["CardType_Land"]}],
        "annotations":[
          {"id":201,"affectedIds":[171],"type":["AnnotationType_ZoneTransfer"],
            "details":[{"key":"zone_src","valueInt32":[36]},{"key":"zone_dest","valueInt32":[35]},{"key":"category","valueString":["Draw"]}]},
          {"id":202,"affectorId":500,"affectedIds":[500],"type":["AnnotationType_RevealedCardCreated"]}]}}"#;
    let record = match_record(
        &synthetic_match(&[full, drawn_and_revealed]),
        &fixture_cards(),
    )
    .expect("card data is complete");
    let statements = statements_by_game(&record).remove(0);
    assert!(
        !statements
            .iter()
            .any(|s| s.contains(" identify ") || s.contains("\"Forest\"")),
        "{statements:#?}"
    );
    assert!(
        statements
            .contains(&"incomplete \"Arena revealed a card the record cannot tell\"".to_string()),
        "{statements:#?}"
    );
}

/// The three-games fixture's `Shuffle`s (lines 12186, 23598, 23618 and
/// 28539: "the rest on the bottom in a random order") are all random ends
/// of the opponent's library, not shuffles (#602, Task 8b).
#[test]
fn fixture_partial_shuffles_are_random_bottoms() {
    let mut shuffles = Vec::new();
    for number in 1..=3 {
        let inputs = fixture_match_game("bo3-three-games", 0, number).inputs;
        let fold = crate::fold_game(&inputs);
        for step in &fold.steps {
            for annotation in &step.annotations {
                if !annotation.is("AnnotationType_Shuffle") {
                    continue;
                }
                shuffles.push((
                    fold.states[step.state_index].state_id,
                    annotation.affected_ids.clone(),
                    super::library::random_end(&fold, step.state_index, annotation),
                ));
            }
        }
    }
    let bottom = ::mtgr::LibraryEnd::Bottom;
    assert_eq!(
        shuffles,
        vec![
            (121, vec![2], Some((bottom, 6))),
            (421, vec![2], Some((bottom, 5))),
            (424, vec![2], Some((bottom, 5))),
            (519, vec![2], Some((bottom, 5))),
        ]
    );
}

/// #602, Task 8c: "put the other on the bottom" moves no card in Arena's
/// report; it renumbers the one card left on the bottom with a `Shuffle`
/// (`bo1-wins-and-concede` line 10654). Its place is known: the record
/// moves it to the bottom and keeps tracking it.
#[test]
fn one_card_renumbered_on_the_bottom_keeps_its_place() {
    let full = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[1],
      "gameStateMessage":{"type":"GameStateType_Full","gameStateId":1,
        "players":[{"systemSeatNumber":1,"lifeTotal":20},{"systemSeatNumber":2,"lifeTotal":20}],
        "turnInfo":{"turnNumber":1,"activePlayer":1,"phase":"Phase_Main1","priorityPlayer":1},
        "zones":[
          {"zoneId":31,"type":"ZoneType_Hand","ownerSeatId":1,"objectInstanceIds":[]},
          {"zoneId":32,"type":"ZoneType_Library","ownerSeatId":1,"objectInstanceIds":[161,162,163,164]}],
        "gameObjects":[
          {"instanceId":161,"grpId":95190,"type":"GameObjectType_Card","zoneId":32,"visibility":"Visibility_Private","ownerSeatId":1,"controllerSeatId":1,"cardTypes":["CardType_Land"]},
          {"instanceId":162,"grpId":95190,"type":"GameObjectType_Card","zoneId":32,"visibility":"Visibility_Private","ownerSeatId":1,"controllerSeatId":1,"cardTypes":["CardType_Land"]}]}}"#;
    let select = r#"{"type":"GREMessageType_SelectNReq","systemSeatIds":[1],"gameStateId":1,
      "selectNReq":{"minSel":1,"maxSel":1,"context":"SelectionContext_Resolution","idType":"IdType_InstanceId",
        "ids":[161,162],"unfilteredIds":[161,162],"sourceId":0}}"#;
    let chosen = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[1],
      "gameStateMessage":{"type":"GameStateType_Diff","gameStateId":2,"prevGameStateId":1,
        "turnInfo":{"turnNumber":1,"activePlayer":1,"phase":"Phase_Main1","priorityPlayer":1},
        "zones":[
          {"zoneId":31,"type":"ZoneType_Hand","ownerSeatId":1,"objectInstanceIds":[161]},
          {"zoneId":32,"type":"ZoneType_Library","ownerSeatId":1,"objectInstanceIds":[163,164,170]}],
        "gameObjects":[
          {"instanceId":161,"grpId":95190,"type":"GameObjectType_Card","zoneId":31,"visibility":"Visibility_Private","ownerSeatId":1,"controllerSeatId":1,"cardTypes":["CardType_Land"]},
          {"instanceId":170,"grpId":95190,"type":"GameObjectType_Card","zoneId":32,"visibility":"Visibility_Private","ownerSeatId":1,"controllerSeatId":1,"cardTypes":["CardType_Land"]}],
        "annotations":[
          {"id":201,"affectedIds":[161],"type":["AnnotationType_ZoneTransfer"],
            "details":[{"key":"zone_src","valueInt32":[32]},{"key":"zone_dest","valueInt32":[31]},{"key":"category","valueString":["Put"]}]},
          {"id":202,"affectedIds":[1],"type":["AnnotationType_Shuffle"],
            "details":[{"key":"OldIds","valueInt32":[162]},{"key":"NewIds","valueInt32":[170]}]}]}}"#;
    let next_turn = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[1],
      "gameStateMessage":{"type":"GameStateType_Diff","gameStateId":3,"prevGameStateId":2,
        "turnInfo":{"turnNumber":2,"activePlayer":2,"phase":"Phase_Beginning","step":"Step_Upkeep","priorityPlayer":2},
        "annotations":[
          {"id":301,"affectorId":2,"affectedIds":[2],"type":["AnnotationType_NewTurnStarted"]},
          {"id":302,"affectedIds":[2],"type":["AnnotationType_PhaseOrStepModified"],
            "details":[{"key":"phase","valueInt32":[1]},{"key":"step","valueInt32":[1]}]}]}}"#;
    let record = match_record(
        &synthetic_match(&[full, select, chosen, next_turn]),
        &fixture_cards(),
    )
    .expect("card data is complete");
    let statements = statements_by_game(&record).remove(0);
    let bottom = statements
        .iter()
        .find(|s| s.ends_with(" move library -> library bottom by ?"))
        .unwrap_or_else(|| panic!("{statements:?}"));
    let id = id_at(bottom).expect("an id");
    assert!(
        !statements
            .iter()
            .any(|s| s.contains(" shuffle") || s.ends_with(" untracked")),
        "{statements:?}"
    );
    let keyframe = statements
        .iter()
        .rposition(|s| s == "keyframe")
        .expect("turn 2's keyframe");
    assert!(
        statements[keyframe..]
            .iter()
            .any(|s| mentions(s, id) && s.ends_with(" library \"Player\" bottom 1")),
        "{statements:?}"
    );
}

/// Spec 14.2, decision 0045: `top N` pushes the positions counted from the
/// top that are N or more one further away, and a position counted from
/// the bottom only when that card is above the new one.
#[test]
fn top_n_moves_the_positions_beyond_it() {
    use ::mtgr::{LibraryEnd, LibraryPlace, Line, ObjRef, Position, Statement, Zone};
    let line = |statement: Statement| Line {
        time_ms: 0,
        level: 2,
        statement,
        cause: None,
    };
    let card = |id: u32, position: Option<Position>| {
        line(Statement::NewCard {
            object: ObjRef {
                id,
                name: Some("Forest".to_string()),
            },
            owner: "Player".to_string(),
            zone: Zone::Library,
            position,
            entering: None,
            sources: Vec::new(),
        })
    };
    let at = |end: LibraryEnd, n: u32| Some(Position { end, n });
    let lines = vec![
        line(Statement::KeyframePlayer {
            player: "Player".to_string(),
            life: 20,
            poison: 0,
            library: 5,
        }),
        card(1, at(LibraryEnd::Top, 1)),
        card(2, at(LibraryEnd::Top, 4)),
        card(3, at(LibraryEnd::Bottom, 1)),
        // Second from the top, counted from the bottom: above the new card.
        card(4, at(LibraryEnd::Bottom, 4)),
        line(Statement::NewCard {
            object: ObjRef {
                id: 5,
                name: Some("Forest".to_string()),
            },
            owner: "Player".to_string(),
            zone: Zone::Hand,
            position: None,
            entering: None,
            sources: Vec::new(),
        }),
        line(Statement::Move {
            object: ObjRef {
                id: 5,
                name: Some("Forest".to_string()),
            },
            from: Zone::Hand,
            to: Zone::Library,
            action: None,
            position: Some(LibraryPlace::FromTop(3)),
            entering: None,
            with: None,
        }),
    ];
    let mut view = super::library::PositionView::default();
    view.catch_up(&lines);
    assert_eq!(view.position(1), at(LibraryEnd::Top, 1));
    assert_eq!(view.position(2), at(LibraryEnd::Top, 5));
    assert_eq!(view.position(3), at(LibraryEnd::Bottom, 1));
    assert_eq!(view.position(4), at(LibraryEnd::Bottom, 5));
    assert_eq!(view.position(5), at(LibraryEnd::Top, 3));
}

/// Decision 0046: Arena reports the local player's looks at their library
/// in the prompts it sends them: a surveil's `GroupReq` (loss log line
/// 23096, game 3: Meticulous Archive) and a `SelectNReq` choosing from the
/// top cards (`unfilteredIds` lists every card looked at). Each card is
/// introduced where it is, then looked at by the resolving object. A
/// mulligan's `GroupReq` (cards in a hand) and the opponent's library give
/// no look.
#[test]
fn local_looks_at_library_cards_are_written() {
    let loss = fixture_match("bo3-loss-and-match-concede", 0, "Opponent");
    let record = match_record(&loss, &fixture_cards()).expect("card data is complete");
    let game_three = statements_by_game(&record).remove(2);
    let resolve = game_three
        .iter()
        .position(|s| s == "#24 resolve")
        .expect("the surveil trigger resolves");
    let introduced = &game_three[resolve + 1];
    assert!(
        introduced.starts_with("new card #")
            && introduced.ends_with(" owner \"Player\" in library top 1"),
        "{introduced}"
    );
    let card = id_at(introduced.strip_prefix("new card ").unwrap_or("")).expect("an id");
    let look = &game_three[resolve + 2];
    assert!(look.starts_with(&format!("#{card} \"")), "{look}");
    assert!(look.ends_with(" look \"Player\" by #24"), "{look}");
    let mut looks = 0;
    for input in [three_games(), match_concede()] {
        let record = match_record(&input, &fixture_cards()).expect("card data is complete");
        for statements in statements_by_game(&record) {
            looks += statements.iter().filter(|s| s.contains(" look ")).count();
        }
    }
    assert_eq!(looks, 0, "mulligan prompts are not looks at a library");

    let full = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[1],
      "gameStateMessage":{"type":"GameStateType_Full","gameStateId":1,
        "players":[{"systemSeatNumber":1,"lifeTotal":20},{"systemSeatNumber":2,"lifeTotal":20}],
        "turnInfo":{"turnNumber":1,"activePlayer":1,"phase":"Phase_Main1","priorityPlayer":1},
        "zones":[
          {"zoneId":32,"type":"ZoneType_Library","ownerSeatId":1,"objectInstanceIds":[161,162,163,164,165]},
          {"zoneId":36,"type":"ZoneType_Library","ownerSeatId":2,"objectInstanceIds":[171,172,173]}]}}"#;
    let shown = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[1],
      "gameStateMessage":{"type":"GameStateType_Diff","gameStateId":2,"prevGameStateId":1,
        "turnInfo":{"turnNumber":1,"activePlayer":1,"phase":"Phase_Main1","priorityPlayer":1},
        "gameObjects":[
          {"instanceId":161,"grpId":95190,"type":"GameObjectType_Card","zoneId":32,"visibility":"Visibility_Private","ownerSeatId":1,"controllerSeatId":1,"cardTypes":["CardType_Land"]},
          {"instanceId":162,"grpId":95190,"type":"GameObjectType_Card","zoneId":32,"visibility":"Visibility_Private","ownerSeatId":1,"controllerSeatId":1,"cardTypes":["CardType_Land"]},
          {"instanceId":163,"grpId":95190,"type":"GameObjectType_Card","zoneId":32,"visibility":"Visibility_Private","ownerSeatId":1,"controllerSeatId":1,"cardTypes":["CardType_Land"]}]}}"#;
    let select = r#"{"type":"GREMessageType_SelectNReq","systemSeatIds":[1],"gameStateId":2,
      "selectNReq":{"minSel":1,"maxSel":1,"context":"SelectionContext_Resolution","idType":"IdType_InstanceId",
        "ids":[162],"unfilteredIds":[161,162,163],"sourceId":0}}"#;
    let theirs = r#"{"type":"GREMessageType_SelectNReq","systemSeatIds":[1],"gameStateId":2,
      "selectNReq":{"minSel":1,"maxSel":1,"context":"SelectionContext_Resolution","idType":"IdType_InstanceId",
        "ids":[171,172],"sourceId":0}}"#;
    let record = match_record(
        &synthetic_match(&[full, shown, select, theirs]),
        &fixture_cards(),
    )
    .expect("card data is complete");
    let statements = statements_by_game(&record).remove(0);
    let in_library: Vec<String> = statements
        .iter()
        .filter(|s| s.starts_with("new ") && s.contains(" in library"))
        .filter_map(|s| s.split_once(" owner ").map(|(_, rest)| rest.to_string()))
        .collect();
    assert_eq!(
        in_library,
        vec![
            "\"Player\" in library top 1",
            "\"Player\" in library top 2",
            "\"Player\" in library top 3",
        ],
        "{statements:?}"
    );
    let looks: Vec<&String> = statements.iter().filter(|s| s.contains(" look ")).collect();
    assert_eq!(looks.len(), 3, "{statements:?}");
    for look in looks {
        assert!(look.ends_with(" \"Forest\" look \"Player\" by ?"), "{look}");
    }
}

/// #602, Task 8c: a prompt sent again in the same state looks at nothing
/// new, but a later prompt from the same source is a new look.
#[test]
fn a_later_prompt_from_the_same_source_is_a_new_look() {
    let full = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[1],
      "gameStateMessage":{"type":"GameStateType_Full","gameStateId":1,
        "players":[{"systemSeatNumber":1,"lifeTotal":20},{"systemSeatNumber":2,"lifeTotal":20}],
        "turnInfo":{"turnNumber":1,"activePlayer":1,"phase":"Phase_Main1","priorityPlayer":1},
        "zones":[
          {"zoneId":32,"type":"ZoneType_Library","ownerSeatId":1,"objectInstanceIds":[161,162,163]}],
        "gameObjects":[
          {"instanceId":161,"grpId":95190,"type":"GameObjectType_Card","zoneId":32,"visibility":"Visibility_Private","ownerSeatId":1,"controllerSeatId":1,"cardTypes":["CardType_Land"]},
          {"instanceId":162,"grpId":95190,"type":"GameObjectType_Card","zoneId":32,"visibility":"Visibility_Private","ownerSeatId":1,"controllerSeatId":1,"cardTypes":["CardType_Land"]}]}}"#;
    let select = |state: u32, ids: &str| {
        format!(
            r#"{{"type":"GREMessageType_SelectNReq","systemSeatIds":[1],"gameStateId":{state},
      "selectNReq":{{"minSel":1,"maxSel":1,"context":"SelectionContext_Resolution","idType":"IdType_InstanceId",
        "ids":[{ids}],"unfilteredIds":[{ids}],"sourceId":0}}}}"#
        )
    };
    let next = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[1],
      "gameStateMessage":{"type":"GameStateType_Diff","gameStateId":2,"prevGameStateId":1,
        "turnInfo":{"turnNumber":1,"activePlayer":1,"phase":"Phase_Main1","priorityPlayer":1}}}"#;
    let first = select(1, "161,162");
    let again = select(2, "161");
    let record = match_record(
        &synthetic_match(&[full, first.as_str(), first.as_str(), next, again.as_str()]),
        &fixture_cards(),
    )
    .expect("card data is complete");
    let statements = statements_by_game(&record).remove(0);
    let looks = statements.iter().filter(|s| s.contains(" look ")).count();
    assert_eq!(looks, 3, "{statements:?}");
}

/// #602, Task 8c: only a prompt sent to the local seat alone shows the
/// local player cards.
#[test]
fn prompts_to_another_seat_are_no_looks() {
    let full = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[1],
      "gameStateMessage":{"type":"GameStateType_Full","gameStateId":1,
        "players":[{"systemSeatNumber":1,"lifeTotal":20},{"systemSeatNumber":2,"lifeTotal":20}],
        "turnInfo":{"turnNumber":1,"activePlayer":1,"phase":"Phase_Main1","priorityPlayer":1},
        "zones":[
          {"zoneId":32,"type":"ZoneType_Library","ownerSeatId":1,"objectInstanceIds":[161,162,163]}],
        "gameObjects":[
          {"instanceId":161,"grpId":95190,"type":"GameObjectType_Card","zoneId":32,"visibility":"Visibility_Private","ownerSeatId":1,"controllerSeatId":1,"cardTypes":["CardType_Land"]}]}}"#;
    let theirs = r#"{"type":"GREMessageType_SelectNReq","systemSeatIds":[2],"gameStateId":1,
      "selectNReq":{"minSel":1,"maxSel":1,"context":"SelectionContext_Resolution","idType":"IdType_InstanceId",
        "ids":[161],"sourceId":0}}"#;
    let both = r#"{"type":"GREMessageType_SelectNReq","systemSeatIds":[1,2],"gameStateId":1,
      "selectNReq":{"minSel":1,"maxSel":1,"context":"SelectionContext_Resolution","idType":"IdType_InstanceId",
        "ids":[161],"sourceId":0}}"#;
    let record = match_record(&synthetic_match(&[full, theirs, both]), &fixture_cards())
        .expect("card data is complete");
    let statements = statements_by_game(&record).remove(0);
    assert!(
        !statements.iter().any(|s| s.contains(" look ")),
        "{statements:?}"
    );
}

/// Tokens named by their subtypes (no card data by grpId) are listed with
/// their grpId and subtypes, so a reader can find their art by the
/// Scryfall name (the subtypes). The record keeps "Map Token" (CR 111.4).
#[test]
fn subtype_named_tokens_are_listed() {
    let input = fixture_match("bo3-loss-and-match-concede", 0, "Opponent");
    let written = super::write_match(&input, &fixture_cards()).expect("card data is complete");
    let maps: Vec<&super::SubtypeToken> = written
        .tokens
        .iter()
        .filter(|token| token.name == "Map Token")
        .collect();
    assert_eq!(maps.len(), 1, "one entry per grpId: {:?}", written.tokens);
    assert_eq!(maps[0].subtypes, vec!["Map".to_string()]);
    assert!(maps[0].grp_id > 0);
    assert!(::mtgr::write(&written.record).contains("\"Map Token\""));
    assert_eq!(
        written.record,
        match_record(&input, &fixture_cards()).expect("the same record")
    );
}

/// Arena shows an exiled card under the card it is exiled with (a persistent
/// `DisplayCardUnderCard`, source to card). Brutal Cathar's trigger exiles
/// the Valkyrie "until this creature leaves the battlefield or transforms"
/// (three-games game 1): the move and the keyframes link it (spec 7.1,
/// 10.3).
#[test]
fn exiled_cards_are_linked_to_their_source() {
    let record = match_record(&three_games(), &fixture_cards()).expect("card data is complete");
    let statements = statements_by_game(&record).remove(0);
    let exiled = statements
        .iter()
        .position(|s| s == "#29 \"Youthful Valkyrie\" move battlefield -> exile with #42 by #43")
        .expect("the exile names its source");
    let linked: Vec<&String> = statements[exiled..]
        .iter()
        .filter(|s| s.starts_with("#29 \"Youthful Valkyrie\" exile"))
        .collect();
    assert_eq!(
        linked,
        vec![
            "#29 \"Youthful Valkyrie\" exile with #42",
            "#29 \"Youthful Valkyrie\" exile with #42"
        ]
    );
}

/// CR 704.5n: Equipment attached to a permanent that left the battlefield
/// becomes unattached, and Arena ends its `Attachment` while the Equipment
/// stays (sealed fixture, first match: a Sweep exiles #14 under #10). The
/// record says so, or its next keyframe would still show `attached #14`.
#[test]
fn equipment_is_unattached_when_its_creature_leaves() {
    let input = fixture_match("sealed-direct-fast-login", 0, "Opponent");
    let record = match_record(&input, &fixture_cards()).expect("card data is complete");
    let statements = statements_by_game(&record).remove(0);
    let exiled = statements
        .iter()
        .position(|s| s == "#14 move battlefield -> exile by #36")
        .expect("the creature is exiled");
    let unattach = statements
        .iter()
        .position(|s| s == "#10 unattach by rule:sba")
        .expect("the Equipment is unattached");
    assert!(unattach > exiled);
    let keyframe = statements[unattach..]
        .iter()
        .find(|s| s.starts_with("#10 ") && s.contains(" battlefield "))
        .expect("a later keyframe lists the Equipment");
    assert!(!keyframe.contains(" attached "), "{keyframe}");
}

/// CR 715.3: the Bo1 fixture's opponent casts Merfolk Secretkeeper (70200)
/// as its Adventure, Venture Deeper. Arena shows the card by the
/// Adventure's grpId (70481) under a `GameObjectType_Adventure` part, so the
/// card is named by the grpId it shows elsewhere, cast with `face`, goes
/// on an adventure as it resolves (spec 14.3), and is cast from exile as
/// the creature.
#[test]
fn adventure_is_cast_as_its_face_and_goes_on_an_adventure() {
    let mut cards = fixture_cards();
    cards.insert(70200, with_faces("Merfolk Secretkeeper // Venture Deeper"));
    let input = fixture_match("bo1-wins-and-concede", 1, "Opponent");
    let record = match_record(&input, &cards).expect("card data is complete");
    let statements = statements_by_game(&record).remove(0);
    let name = "\"Merfolk Secretkeeper // Venture Deeper\"";
    let cast_at = statements
        .iter()
        .position(|s| s.ends_with(&format!("{name} cast from hand face \"Venture Deeper\"")))
        .unwrap_or_else(|| panic!("the Adventure is cast: {statements:?}"));
    let id = id_at(&statements[cast_at]).expect("an id");
    let card = format!("#{id} {name}");
    let rest = &statements[cast_at..];
    let exiled = rest
        .iter()
        .position(|s| *s == format!("{card} move stack -> exile by rule:resolve"))
        .unwrap_or_else(|| panic!("it goes on an adventure: {rest:?}"));
    assert_eq!(
        rest[exiled + 1],
        format!("{card} cast from exile"),
        "cast as the creature"
    );
    assert!(
        !raw_statements_by_game(&record)[0]
            .iter()
            .any(|s| s.contains("arena:70481")),
        "the Adventure's grpId never names the card"
    );
}

/// Spec 10.3: a card on an adventure at a keyframe is marked with the
/// player who may cast it (CR 715.3d), the Adventure spell's controller.
#[test]
fn keyframes_mark_a_card_on_an_adventure() {
    let full = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[1],
      "gameStateMessage":{"type":"GameStateType_Full","gameStateId":1,
        "players":[{"systemSeatNumber":1,"lifeTotal":20},{"systemSeatNumber":2,"lifeTotal":20}],
        "turnInfo":{"turnNumber":1,"activePlayer":2,"phase":"Phase_Main1","priorityPlayer":2},
        "zones":[
          {"zoneId":27,"type":"ZoneType_Stack","objectInstanceIds":[]},
          {"zoneId":29,"type":"ZoneType_Exile","objectInstanceIds":[]},
          {"zoneId":31,"type":"ZoneType_Hand","ownerSeatId":2,"objectInstanceIds":[200]}]}}"#;
    let cast = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[1],
      "gameStateMessage":{"type":"GameStateType_Diff","gameStateId":2,"prevGameStateId":1,
        "turnInfo":{"turnNumber":1,"activePlayer":2,"phase":"Phase_Main1","priorityPlayer":2},
        "zones":[
          {"zoneId":27,"type":"ZoneType_Stack","objectInstanceIds":[210]},
          {"zoneId":31,"type":"ZoneType_Hand","ownerSeatId":2,"objectInstanceIds":[]}],
        "gameObjects":[
          {"instanceId":210,"grpId":70481,"type":"GameObjectType_Card","zoneId":27,"visibility":"Visibility_Public","ownerSeatId":2,"controllerSeatId":2,"cardTypes":["CardType_Sorcery"]},
          {"instanceId":211,"grpId":70481,"type":"GameObjectType_Adventure","zoneId":27,"visibility":"Visibility_Public","ownerSeatId":2,"controllerSeatId":2,"cardTypes":["CardType_Sorcery"],"parentId":210}],
        "annotations":[
          {"id":1,"affectedIds":[200],"type":["AnnotationType_ObjectIdChanged"],
            "details":[{"key":"orig_id","valueInt32":[200]},{"key":"new_id","valueInt32":[210]}]},
          {"id":2,"affectedIds":[210],"type":["AnnotationType_ZoneTransfer"],
            "details":[{"key":"zone_src","valueInt32":[31]},{"key":"zone_dest","valueInt32":[27]},{"key":"category","valueString":["CastSpell"]}]}]}}"#;
    let resolve = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[1],
      "gameStateMessage":{"type":"GameStateType_Diff","gameStateId":3,"prevGameStateId":2,
        "turnInfo":{"turnNumber":1,"activePlayer":2,"phase":"Phase_Main1","priorityPlayer":2},
        "zones":[
          {"zoneId":27,"type":"ZoneType_Stack","objectInstanceIds":[]},
          {"zoneId":29,"type":"ZoneType_Exile","objectInstanceIds":[220]}],
        "gameObjects":[
          {"instanceId":220,"grpId":70200,"type":"GameObjectType_Card","zoneId":29,"visibility":"Visibility_Public","ownerSeatId":2,"controllerSeatId":2,"cardTypes":["CardType_Creature"],"toughness":{"value":4}},
          {"instanceId":221,"grpId":70481,"type":"GameObjectType_Adventure","zoneId":29,"visibility":"Visibility_Public","ownerSeatId":2,"controllerSeatId":2,"cardTypes":["CardType_Sorcery"],"parentId":220}],
        "annotations":[
          {"id":3,"affectorId":210,"affectedIds":[210],"type":["AnnotationType_ResolutionStart"],
            "details":[{"key":"grpid","valueInt32":[70481]}]},
          {"id":4,"affectorId":210,"affectedIds":[210],"type":["AnnotationType_ResolutionComplete"],
            "details":[{"key":"grpid","valueInt32":[70481]}]},
          {"id":5,"affectedIds":[210],"type":["AnnotationType_ObjectIdChanged"],
            "details":[{"key":"orig_id","valueInt32":[210]},{"key":"new_id","valueInt32":[220]}]},
          {"id":6,"affectorId":2,"affectedIds":[220],"type":["AnnotationType_ZoneTransfer"],
            "details":[{"key":"zone_src","valueInt32":[27]},{"key":"zone_dest","valueInt32":[29]},{"key":"category","valueString":["Resolve"]}]}]}}"#;
    let next_turn = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[1],
      "gameStateMessage":{"type":"GameStateType_Diff","gameStateId":4,"prevGameStateId":3,
        "turnInfo":{"turnNumber":2,"activePlayer":1,"phase":"Phase_Beginning","step":"Step_Upkeep","priorityPlayer":1}}}"#;
    let mut cards = fixture_cards();
    cards.insert(70200, with_faces("Merfolk Secretkeeper // Venture Deeper"));
    let record = match_record(&synthetic_match(&[full, cast, resolve, next_turn]), &cards)
        .expect("card data is complete");
    let statements = statements_by_game(&record).remove(0);
    let name = "\"Merfolk Secretkeeper // Venture Deeper\"";
    let cast_at = statements
        .iter()
        .position(|s| s.ends_with(&format!("{name} cast from hand face \"Venture Deeper\"")))
        .unwrap_or_else(|| panic!("the Adventure is cast: {statements:?}"));
    let id = id_at(&statements[cast_at]).expect("an id");
    let turn = statements
        .iter()
        .position(|s| s.starts_with("turn 2 "))
        .unwrap_or_else(|| panic!("turn 2 begins: {statements:?}"));
    assert!(
        statements[turn..].contains(&format!("#{id} {name} exile adventure \"Opponent\"")),
        "{statements:?}"
    );
}

/// Spec 7.3: Arena gives each object its `controllerSeatId`, and no
/// annotation when it changes. A permanent whose controller changes is
/// written `control`, and keyframes list it under its controller.
#[test]
fn control_changes_are_written() {
    let full = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[1],
      "gameStateMessage":{"type":"GameStateType_Full","gameStateId":1,
        "players":[{"systemSeatNumber":1,"lifeTotal":20},{"systemSeatNumber":2,"lifeTotal":20}],
        "turnInfo":{"turnNumber":1,"activePlayer":2,"phase":"Phase_Main1","priorityPlayer":2},
        "zones":[
          {"zoneId":28,"type":"ZoneType_Battlefield","objectInstanceIds":[]},
          {"zoneId":31,"type":"ZoneType_Hand","ownerSeatId":2,"objectInstanceIds":[200]}]}}"#;
    let played = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[1],
      "gameStateMessage":{"type":"GameStateType_Diff","gameStateId":2,"prevGameStateId":1,
        "turnInfo":{"turnNumber":1,"activePlayer":2,"phase":"Phase_Main1","priorityPlayer":2},
        "zones":[
          {"zoneId":28,"type":"ZoneType_Battlefield","objectInstanceIds":[300]},
          {"zoneId":31,"type":"ZoneType_Hand","ownerSeatId":2,"objectInstanceIds":[]}],
        "gameObjects":[
          {"instanceId":300,"grpId":82390,"type":"GameObjectType_Card","zoneId":28,"visibility":"Visibility_Public","ownerSeatId":2,"controllerSeatId":2,"cardTypes":["CardType_Land"]}],
        "annotations":[
          {"id":1,"affectedIds":[200],"type":["AnnotationType_ObjectIdChanged"],
            "details":[{"key":"orig_id","valueInt32":[200]},{"key":"new_id","valueInt32":[300]}]},
          {"id":2,"affectedIds":[300],"type":["AnnotationType_ZoneTransfer"],
            "details":[{"key":"zone_src","valueInt32":[31]},{"key":"zone_dest","valueInt32":[28]},{"key":"category","valueString":["PlayLand"]}]}]}}"#;
    let stolen = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[1],
      "gameStateMessage":{"type":"GameStateType_Diff","gameStateId":3,"prevGameStateId":2,
        "turnInfo":{"turnNumber":1,"activePlayer":2,"phase":"Phase_Main1","priorityPlayer":2},
        "gameObjects":[
          {"instanceId":300,"grpId":82390,"type":"GameObjectType_Card","zoneId":28,"visibility":"Visibility_Public","ownerSeatId":2,"controllerSeatId":1,"cardTypes":["CardType_Land"]}]}}"#;
    let next_turn = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[1],
      "gameStateMessage":{"type":"GameStateType_Diff","gameStateId":4,"prevGameStateId":3,
        "turnInfo":{"turnNumber":2,"activePlayer":1,"phase":"Phase_Beginning","step":"Step_Upkeep","priorityPlayer":1}}}"#;
    let record = match_record(
        &synthetic_match(&[full, played, stolen, next_turn]),
        &fixture_cards(),
    )
    .expect("card data is complete");
    let statements = statements_by_game(&record).remove(0);
    let control = statements
        .iter()
        .position(|s| s.ends_with(" control \"Player\" by ?"))
        .unwrap_or_else(|| panic!("the control change is written: {statements:?}"));
    let id = id_at(&statements[control]).expect("an id");
    let turn = statements
        .iter()
        .position(|s| s.starts_with("turn 2 "))
        .unwrap_or_else(|| panic!("turn 2 begins: {statements:?}"));
    assert!(control < turn);
    let entry = statements[turn..]
        .iter()
        .find(|s| s.starts_with(&format!("#{id} ")))
        .unwrap_or_else(|| panic!("the keyframe lists it: {statements:?}"));
    assert!(entry.contains(" battlefield \"Player\" "), "{entry}");
    assert_eq!(
        statements
            .iter()
            .filter(|s| s.contains(" control "))
            .count(),
        1,
        "{statements:?}"
    );
}

/// CR 608.2b, judged as the MTGO recorder judges it: the sealed fixture's
/// opponent targets their own creature with an instant, the local player
/// kills the creature in response, and Arena moves the instant from the
/// stack to the graveyard with category `nil` and no `ResolutionStart`.
/// Every target left the zone it was targeted in, so it fizzles.
#[test]
fn spell_whose_targets_left_fizzles() {
    let input = fixture_match("sealed-direct-fast-login", 3, "Opponent");
    let record = match_record(&input, &fixture_cards()).expect("card data is complete");
    let games = statements_by_game(&record);
    let statements = games
        .iter()
        .find(|statements| statements.iter().any(|s| s.ends_with(" fizzle")))
        .unwrap_or_else(|| panic!("a spell fizzles: {games:?}"));
    let at = statements
        .iter()
        .position(|s| s.ends_with(" fizzle"))
        .expect("the fizzle");
    let id = id_at(&statements[at]).expect("an id");
    assert!(
        statements[at - 1].ends_with(" pass"),
        "the last player to pass passes first: {:?}",
        &statements[at - 3..=at]
    );
    let next = &statements[at + 1];
    assert!(
        mentions(next, id) && next.ends_with(" move stack -> graveyard by rule:fizzle"),
        "{next}"
    );
    let all: usize = games
        .iter()
        .flatten()
        .filter(|s| s.ends_with(" fizzle"))
        .count();
    assert_eq!(all, 1, "one spell fizzles in the match");
}

/// CR 608.2b: a target that becomes illegal where it stands also fizzles
/// the spell. The sealed fixture's first match has an instant whose target
/// gained protection or hexproof from an Aura in response: Arena moves it
/// from the stack with category `nil`, no affector and no
/// `ResolutionStart`, while the target is still on the battlefield.
#[test]
fn spell_whose_target_became_illegal_fizzles() {
    let input = fixture_match("sealed-direct-fast-login", 0, "Opponent");
    let record = match_record(&input, &fixture_cards()).expect("card data is complete");
    let games = statements_by_game(&record);
    let statements = games
        .iter()
        .find(|statements| statements.iter().any(|s| s.ends_with(" fizzle")))
        .unwrap_or_else(|| panic!("a spell fizzles: {games:?}"));
    let at = statements
        .iter()
        .position(|s| s.ends_with(" fizzle"))
        .expect("the fizzle");
    let id = id_at(&statements[at]).expect("an id");
    let next = &statements[at + 1];
    assert!(
        mentions(next, id) && next.ends_with(" move stack -> graveyard by rule:fizzle"),
        "{next}"
    );
}

/// Spec 9.5 and 9.8: a counter Arena puts on a player (the affected id is a
/// seat) is never dropped silently. No Arena counter type is known to name
/// a player counter, so the record says it is incomplete there.
#[test]
fn player_counters_are_incomplete() {
    let full = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[1],
      "gameStateMessage":{"type":"GameStateType_Full","gameStateId":1,
        "players":[{"systemSeatNumber":1,"lifeTotal":20},{"systemSeatNumber":2,"lifeTotal":20}],
        "turnInfo":{"turnNumber":1,"activePlayer":2,"phase":"Phase_Main1","priorityPlayer":2}}}"#;
    let poisoned = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[1],
      "gameStateMessage":{"type":"GameStateType_Diff","gameStateId":2,"prevGameStateId":1,
        "turnInfo":{"turnNumber":1,"activePlayer":2,"phase":"Phase_Main1","priorityPlayer":2},
        "annotations":[
          {"id":1,"affectorId":300,"affectedIds":[1],"type":["AnnotationType_CounterAdded"],
            "details":[{"key":"counter_type","valueInt32":[3]},{"key":"transaction_amount","valueInt32":[2]}]}]}}"#;
    let record = match_record(&synthetic_match(&[full, poisoned]), &fixture_cards())
        .expect("card data is complete");
    let statements = statements_by_game(&record).remove(0);
    assert!(
        statements.contains(
            &"incomplete \"Arena counter type 3 on player Player has no counter name here\""
                .to_string()
        ),
        "{statements:?}"
    );
}

/// Spec 7.2: Arena reports X as a `CastingTimeOption` of type 2 (the
/// client's `ChooseX` answer). The Bo1 fixture's March of Otherworldly Light
/// is cast with X 1: `choose x 1` comes before its target, and stack
/// snapshots repeat it.
#[test]
fn x_is_chosen_before_targets() {
    let input = fixture_match("bo1-wins-and-concede", 1, "Opponent");
    let record = match_record(&input, &fixture_cards()).expect("card data is complete");
    let statements = statements_by_game(&record).remove(0);
    let name = "\"March of Otherworldly Light\"";
    let chose = statements
        .iter()
        .position(|s| s.ends_with(&format!("{name} choose x 1")))
        .unwrap_or_else(|| panic!("X is chosen: {statements:?}"));
    let id = id_at(&statements[chose]).expect("an id");
    let target = statements
        .iter()
        .position(|s| s.starts_with(&format!("#{id} {name} target ")))
        .unwrap_or_else(|| panic!("the spell targets: {statements:?}"));
    assert!(chose < target);
    assert!(
        statements[target..]
            .iter()
            .any(|s| s.starts_with(&format!("#{id} {name} controller \"Player\" x 1 target "))),
        "a snapshot repeats X: {statements:?}"
    );
}

/// Spec 7.2: a `ChoiceResult` with no domain, while an ability of its
/// affector resolves, chose the object its value names (sealed fixture,
/// second match: a land's ability chooses an artifact, then taps it).
#[test]
fn choice_results_choose_objects() {
    let input = fixture_match("sealed-direct-fast-login", 1, "Opponent");
    let record = match_record(&input, &fixture_cards()).expect("card data is complete");
    let statements: Vec<String> = statements_by_game(&record).into_iter().flatten().collect();
    let chose = statements
        .iter()
        .position(|s| s.contains(" choose object "))
        .unwrap_or_else(|| panic!("an object is chosen: {statements:?}"));
    let ability = id_at(&statements[chose]).expect("the ability");
    let chosen = statements[chose]
        .split(" choose object #")
        .nth(1)
        .and_then(|rest| rest.split(' ').next())
        .and_then(|id| id.parse::<u32>().ok())
        .expect("the chosen id");
    assert!(
        statements[chose + 1].starts_with(&format!("#{chosen} "))
            && statements[chose + 1].ends_with(&format!(" tap by #{ability}")),
        "{:?}",
        &statements[chose..chose + 2]
    );
}

#[test]
fn a_brawl_record_lists_the_commander_as_side() {
    let games = vec![GameMessages {
        number: 1,
        messages: fixture_match_game("brawl/brawl_queue", 0, 1).inputs,
    }];
    let input = MatchInput {
        match_token: "brawl".to_string(),
        event: "Play_Brawl_Historic".to_string(),
        format: "Historic Brawl".to_string(),
        local_name: "Player".to_string(),
        opponent_name: "Opponent".to_string(),
        local_archetype: None,
        opponent_archetype: None,
        started_at: 0,
        games,
    };
    // The fixture card table has no Brawl cards: name every missing one.
    let mut cards = fixture_cards();
    let record = loop {
        match match_record(&input, &cards) {
            Ok(record) => break record,
            Err(MtgrError::MissingCardData(ids)) => {
                for id in ids {
                    cards.insert(
                        id,
                        CardData {
                            name: format!("Card {id}"),
                            faces: Vec::new(),
                        },
                    );
                }
            }
        }
    };
    let statements = raw_statements_by_game(&record).remove(0);
    let side: Vec<&String> = statements
        .iter()
        .filter(|s| s.starts_with("side "))
        .collect();
    // The fixture card table has no name for the commander, so it is an
    // unknown entry by its source id; with a name it reads `side 1 "Name"`.
    assert_eq!(side, vec!["side 1 ? src arena:83902"]);
    assert!(::mtgr::write(&record).contains("0.3.0"));
}
