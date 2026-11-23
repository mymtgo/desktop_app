//! Game detail over the real (anonymised) fixture logs, and the 1,000-frame
//! speed check.

use std::collections::HashSet;
use std::time::Instant;

use decks::parse_deck_used;
use fixture_tools::{load_fixture, prepared_events};
use game_detail::{
    DeckCard, Frame, HandCard, OpeningHand, estimate_turns, local_deck, opening_hand,
    opponent_cards, parse_frame, parse_frames,
};
use mtgo_protocol::EventKind;

/// One game of a fixture: its frame event texts in log order and its first
/// `deck_used` list.
struct FixtureGame {
    game_id: u64,
    frame_texts: Vec<String>,
    deck_used: Vec<DeckCard>,
}

fn fixture_games(name: &str) -> (String, Vec<FixtureGame>) {
    let fixture = load_fixture(name);
    let mut games: Vec<FixtureGame> = Vec::new();
    for event in prepared_events(&fixture) {
        let Some(game_id) = event.subjects.game_id else {
            continue;
        };
        let at = match games.iter().position(|game| game.game_id == game_id) {
            Some(at) => at,
            None => {
                games.push(FixtureGame {
                    game_id,
                    frame_texts: Vec::new(),
                    deck_used: Vec::new(),
                });
                games.len() - 1
            }
        };
        let game = &mut games[at];
        if event.kind == EventKind::GameStateUpdate {
            game.frame_texts.push(event.text.clone());
        } else if event.kind == EventKind::DeckUsed
            && game.deck_used.is_empty()
            && let Some((_, cards)) = parse_deck_used(&event.text)
        {
            game.deck_used = cards;
        }
    }
    games.retain(|game| !game.frame_texts.is_empty());
    (fixture.meta.local_username.clone(), games)
}

fn game(games: &[FixtureGame], game_id: u64) -> &FixtureGame {
    games
        .iter()
        .find(|game| game.game_id == game_id)
        .expect("fixture game")
}

fn frames_of(game: &FixtureGame) -> Vec<Frame> {
    let texts: Vec<&str> = game.frame_texts.iter().map(String::as_str).collect();
    parse_frames(&texts)
}

/// The local and opponent seat ids, by the local player's name.
fn seats(frame: &Frame, local_name: &str) -> (i64, i64) {
    let local = frame
        .players
        .iter()
        .find(|player| player.name == local_name)
        .expect("local player");
    let opponent = frame
        .players
        .iter()
        .find(|player| player.name != local_name)
        .expect("opponent");
    (local.id, opponent.id)
}

fn hand(cards: &[(i64, u32)]) -> Vec<HandCard> {
    cards
        .iter()
        .map(|&(instance_id, catalog_id)| HandCard {
            instance_id,
            catalog_id,
        })
        .collect()
}

fn sums(deck: &[DeckCard]) -> (u32, u32) {
    let main = deck
        .iter()
        .filter(|row| !row.sideboard)
        .map(|row| row.quantity);
    let side = deck
        .iter()
        .filter(|row| row.sideboard)
        .map(|row| row.quantity);
    (main.sum(), side.sum())
}

#[test]
fn every_fixture_frame_parses() {
    for name in ["league_join_drop", "draft_hobbit"] {
        let (_, games) = fixture_games(name);
        let mut count = 0;
        for game in &games {
            for text in &game.frame_texts {
                let frame = parse_frame(text).expect("fixture frame parses");
                assert_eq!(frame.players.len(), 2, "{name}");
                count += 1;
            }
        }
        assert!(count > 0, "{name} has frames");
    }
}

#[test]
fn skipping_repeated_frames_changes_no_result() {
    for name in ["league_join_drop", "draft_hobbit"] {
        let (local_name, games) = fixture_games(name);
        for game in &games {
            let all: Vec<Frame> = game
                .frame_texts
                .iter()
                .filter_map(|text| parse_frame(text))
                .collect();
            let distinct = frames_of(game);
            assert!(distinct.len() <= all.len());
            let (local, opponent) = seats(&all[0], &local_name);
            let from_all = opening_hand(&all, local, opponent);
            let from_distinct = opening_hand(&distinct, local, opponent);
            assert_eq!(
                all[from_all.kept_from], distinct[from_distinct.kept_from],
                "{name} game {}",
                game.game_id
            );
            assert_eq!(
                OpeningHand {
                    kept_from: 0,
                    ..from_all
                },
                OpeningHand {
                    kept_from: 0,
                    ..from_distinct
                },
                "{name} game {}",
                game.game_id
            );
            assert_eq!(all.first(), distinct.first());
            assert_eq!(all.last(), distinct.last());
        }
    }
}

#[test]
fn league_game_one_keeps_seven_and_reveals_two_opponent_cards() {
    let (local_name, games) = fixture_games("league_join_drop");
    let game = game(&games, 945500844);
    let frames = frames_of(game);
    let (local, opponent) = seats(&frames[0], &local_name);
    assert_eq!((local, opponent), (0, 1));

    let hand_result = opening_hand(&frames, local, opponent);
    assert_eq!(
        hand_result.starting,
        hand(&[
            (416, 125847),
            (415, 140887),
            (414, 134458),
            (413, 139635),
            (412, 77979),
            (411, 82696),
            (410, 82692),
        ])
    );
    assert!(hand_result.mulliganed_hands.is_empty());
    assert!(hand_result.bottomed.is_empty());
    assert_eq!(hand_result.local_mulligans, 0);
    assert_eq!(hand_result.opponent_mulligans, 0);

    let last = frames.last().expect("last frame");
    assert_eq!(
        opponent_cards(last, opponent),
        vec![(136111, 1), (34832, 1)]
    );

    let deck = local_deck(&game.deck_used, &frames[0], local);
    assert_eq!(sums(&deck), (60, 15));
    assert_eq!(deck.len(), 35);
}

#[test]
fn league_game_two_ended_before_any_hand() {
    let (local_name, games) = fixture_games("league_join_drop");
    let game = game(&games, 945500946);
    let frames = frames_of(game);
    let (local, opponent) = seats(&frames[0], &local_name);

    assert_eq!(
        opening_hand(&frames, local, opponent),
        game_detail::OpeningHand::default()
    );
    assert!(opponent_cards(frames.last().expect("last"), opponent).is_empty());
    let lands: HashSet<u32> = HashSet::new();
    assert_eq!(estimate_turns(frames.last().expect("last"), &lands), None);
}

#[test]
fn draft_game_with_a_mulligan_and_a_bottomed_card() {
    let (local_name, games) = fixture_games("draft_hobbit");
    let game = game(&games, 959763634);
    let frames = frames_of(game);
    let (local, opponent) = seats(&frames[0], &local_name);
    // The local player is seat 1 in this match.
    assert_eq!((local, opponent), (1, 0));

    let result = opening_hand(&frames, local, opponent);

    assert_eq!(result.local_mulligans, 1);
    assert_eq!(
        result.mulliganed_hands,
        vec![hand(&[
            (402, 153892),
            (401, 154020),
            (400, 154192),
            (399, 153894),
            (398, 153900),
            (397, 153914),
            (396, 153892),
        ])]
    );
    assert_eq!(result.bottomed, vec![410]);
    // On the draw: the first draw lands in the hand before the local player
    // acts, so it is part of the starting hand (0.x behaviour; the stored T1
    // draw pill accounts for it).
    assert_eq!(
        result.starting,
        hand(&[
            (416, 153892),
            (415, 154021),
            (414, 153892),
            (413, 153998),
            (412, 153950),
            (411, 153894),
            (419, 154182),
        ])
    );
    assert_eq!(
        result.opening,
        hand(&[
            (416, 153892),
            (415, 154021),
            (414, 153892),
            (413, 153998),
            (412, 153950),
            (411, 153894),
            (410, 154222),
            (419, 154182),
        ])
    );
    assert_eq!(result.opponent_mulligans, 0);

    let deck = local_deck(&game.deck_used, &frames[0], local);
    assert_eq!(sums(&deck), (40, 20));
    assert_eq!(deck.len(), 33);

    let last = frames.last().expect("last frame");
    let seen = opponent_cards(last, opponent);
    assert_eq!(seen.len(), 11);
    assert_eq!(seen[0], (153897, 6));
    assert_eq!(seen[1], (153898, 3));

    // With every catalog id on the battlefield counted as a land, the
    // estimate is the battlefield size.
    let battlefield: Vec<u32> = last
        .cards
        .iter()
        .filter(|card| card.zone == "Battlefield")
        .map(|card| card.catalog_id)
        .collect();
    let lands: HashSet<u32> = battlefield.iter().copied().collect();
    assert_eq!(estimate_turns(last, &lands), Some(battlefield.len() as u32));
}

#[test]
fn draft_game_on_the_draw_keeps_its_first_draw() {
    let (local_name, games) = fixture_games("draft_hobbit");
    let game = game(&games, 959762288);
    let frames = frames_of(game);
    let (local, opponent) = seats(&frames[0], &local_name);

    let result = opening_hand(&frames, local, opponent);

    assert_eq!(result.starting.len(), 8);
    assert_eq!(
        result.starting.last(),
        Some(&HandCard {
            instance_id: 426,
            catalog_id: 125769
        })
    );
    assert!(result.bottomed.is_empty());
    assert_eq!(result.local_mulligans, 0);
}

/// A synthetic constructed frame: 15 sideboard cards, a 7-card hand, and a
/// battlefield (both players) that grows with `step`.
fn synthetic_frame(step: usize) -> String {
    let mut cards: Vec<String> = Vec::new();
    for i in 0..15 {
        cards.push(format!(
            r#"{{"Id":{},"CatalogID":{},"Zone":"Sideboard","ActualZone":"Sideboard","Owner":0,"Controller":0}}"#,
            300 + i,
            60000 + i
        ));
    }
    for i in 0..7 {
        cards.push(format!(
            r#"{{"Id":{},"CatalogID":{},"Zone":"Hand","ActualZone":"Hand","Owner":0,"Controller":0}}"#,
            400 + i,
            70000 + i
        ));
    }
    let on_battlefield = (step / 20).min(60);
    for i in 0..on_battlefield {
        let owner = i % 2;
        cards.push(format!(
            r#"{{"Id":{},"CatalogID":{},"Zone":"Battlefield","ActualZone":"Battlefield","Owner":{owner},"Controller":{owner}}}"#,
            500 + i,
            80000 + (i % 12)
        ));
    }
    format!(
        r#"12:26:09 [INF] (Twitch Info|Game Play Status Update for Game ID: 1, Match ID: 2, Event ID: 2) {{"Players":[{{"Id":0,"Name":"Qaaaaa","LibraryCount":53,"HandCount":7,"Life":20}},{{"Id":1,"Name":"Qbbbbb","LibraryCount":{},"HandCount":7,"Life":{}}}],"Cards":[{}]}}"#,
        53 - (step / 40) as i64,
        // Changes with every step, so each step is a new frame.
        1000 - step as i64,
        cards.join(",")
    )
}

/// The read budget for a big game: 1,000 frames (about 60% repeats, as in
/// real logs) through every frame-based result. Run in release:
/// `cargo test -p game-detail --release -- --ignored`. Ignored by default
/// because debug builds are about ten times slower.
#[test]
#[ignore = "timing check, run in release"]
fn a_thousand_frame_game_is_read_within_a_few_milliseconds() {
    let mut texts: Vec<String> = Vec::new();
    for step in 0..1000 {
        // Four of every ten frames are new; the rest repeat the previous one.
        let distinct_step = step - step % 10 + (step % 10).min(3);
        texts.push(synthetic_frame(distinct_step));
    }
    let raws: Vec<&str> = texts.iter().map(String::as_str).collect();
    let bytes: usize = texts.iter().map(String::len).sum();
    let deck_used: Vec<DeckCard> = (0..15)
        .map(|i| DeckCard {
            catalog_id: 60000 + i,
            quantity: 4,
            sideboard: false,
        })
        .collect();
    let lands: HashSet<u32> = (80000..80006).collect();

    let mut best = f64::MAX;
    for _ in 0..5 {
        let started = Instant::now();
        let frames = parse_frames(&raws);
        let first = frames.first().expect("first");
        let last = frames.last().expect("last");
        let hand_result = opening_hand(&frames, 0, 1);
        let deck = local_deck(&deck_used, first, 0);
        let seen = opponent_cards(last, 1);
        let turns = estimate_turns(last, &lands);
        let elapsed = started.elapsed().as_secs_f64() * 1000.0;
        best = best.min(elapsed);

        assert_eq!(frames.len(), 400);
        assert_eq!(hand_result.starting.len(), 7);
        assert!(!deck.is_empty());
        assert!(!seen.is_empty());
        assert!(turns.is_some());
    }

    println!(
        "1000 frames ({} KB): best of 5 = {best:.2} ms",
        bytes / 1024
    );
    assert!(best < 20.0, "1000 frames took {best:.2} ms");
}
