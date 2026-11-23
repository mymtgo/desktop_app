//! Folding the committed ranked Modern Bo3 capture into one match.

use std::collections::BTreeSet;

use endstep_match::{
    DropReason, EndstepGameCards, EndstepMatch, Folded, QUIET_MS, fold_match, game_log,
};
use endstep_protocol::{
    EndstepCardKey, Frame, FrameKind, GameAction, GameDelta, GameEvent, GameState, PlayerState,
    Prompt, PromptCard, StateCard, Unit, Zone, decode_unit,
};
use game_detail::{GameCardRow, Side};
use mtgo_match::{EndReason, GameResult, MatchState, Outcome, Resolution};

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
const HOUR_MS: i64 = 60 * 60 * 1000;
const MINUTE_MS: i64 = 60 * 1000;

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

fn last_at(units: &[Unit]) -> i64 {
    let mut last = 0;
    for unit in units {
        for frame in &unit.frames {
            last = last.max(frame.at);
        }
    }
    last
}

fn folded_match(units: &[Unit], now_ms: i64) -> EndstepMatch {
    match fold_match(FORGE_GAME, units, now_ms) {
        Folded::Match(found) => *found,
        Folded::Dropped(reason) => panic!("expected a match, dropped: {reason:?}"),
    }
}

fn full_match() -> EndstepMatch {
    let units = all_units();
    let now_ms = last_at(&units) + HOUR_MS;
    folded_match(&units, now_ms)
}

fn card_total(lines: &[(EndstepCardKey, u32)]) -> u32 {
    lines.iter().map(|(_, count)| count).sum()
}

#[test]
fn the_ranked_bo3_folds() {
    let found = full_match();
    let view = &found.view;
    assert_eq!(view.token, "00000000-0000-4000-8000-e0000000003a");
    assert_eq!(view.mtgo_match_id, None);
    assert_eq!(view.format.as_deref(), Some("Modern"));
    assert_eq!(view.match_type.as_deref(), Some("ranked"));
    assert_eq!(view.game_structure.as_deref(), Some("bo3"));
    let numbers: Vec<u32> = view.games.iter().map(|game| game.game_number).collect();
    assert_eq!(numbers, vec![1, 2, 3]);
    let results: Vec<GameResult> = view.games.iter().map(|game| game.result).collect();
    assert_eq!(
        results,
        vec![GameResult::Win, GameResult::Loss, GameResult::Loss]
    );
    for game in &view.games {
        assert_eq!(game.end_reason, EndReason::Concede);
        assert_eq!(game.mtgo_game_id, None);
    }
    assert_eq!(view.wins, 1);
    assert_eq!(view.losses, 2);
    assert_eq!(view.outcome, Outcome::Loss);
    assert_eq!(view.state, MatchState::Complete);
    assert_eq!(view.resolution, Some(Resolution::Signal));
    assert_eq!(view.tentative_outcome, None);

    assert_eq!(
        found.account.user_id,
        "00000000-0000-4000-8000-00000000000a"
    );
    assert_eq!(found.account.username, "Player");
    assert_eq!(
        view.local_username,
        "endstep:00000000-0000-4000-8000-00000000000a"
    );
    let opponent = found.opponent.as_ref().expect("an opponent");
    assert_eq!(opponent.user_id, "00000000-0000-4000-8000-00000000000b");
    assert_eq!(opponent.username, "Opponent");
    assert_eq!(
        view.opponent.as_deref(),
        Some("endstep:00000000-0000-4000-8000-00000000000b")
    );

    let deck = found.deck.as_ref().expect("a deck");
    assert_eq!(deck.key, "endstep:00000000-0000-4000-8000-e00000000031");
    assert_eq!(deck.name, "Tron");
    assert_eq!(deck.format, "Modern");
    assert_eq!(card_total(&deck.main), 60);
    assert_eq!(card_total(&deck.side), 15);

    let ranked = found.ranked.as_ref().expect("a ranked result");
    assert_eq!(ranked.rating_before, Some(1371));
    assert_eq!(ranked.rating_after, Some(1336));
    assert_eq!(ranked.delta, Some(-18));
    assert_eq!(found.forge_game_id, FORGE_GAME);
}

#[test]
fn split_units_fold_to_one_match() {
    let mut units = all_units();
    let now_ms = last_at(&units) + HOUR_MS;
    let forward = folded_match(&units, now_ms);
    units.reverse();
    let reversed = folded_match(&units, now_ms);
    assert_eq!(forward, reversed);

    // Any other order too: the session units first, then the match units.
    let shuffled = vec![
        unit(UNIT_03),
        unit(UNIT_05),
        unit(UNIT_01),
        unit(UNIT_04),
        unit(UNIT_02),
    ];
    assert_eq!(forward, folded_match(&shuffled, now_ms));
}

#[test]
fn a_prefix_folds_to_an_unfinished_match() {
    let units = vec![unit(UNIT_01), unit(UNIT_02)];
    let now_ms = last_at(&units) + MINUTE_MS;
    let found = folded_match(&units, now_ms);
    let view = &found.view;
    assert_eq!(view.state, MatchState::InProgress);
    assert_eq!(view.resolution, None);
    assert_eq!(view.outcome, Outcome::Unknown);
    assert_eq!(view.tentative_outcome, None);
    assert_eq!(view.ended_at_ms, None);
    assert_eq!(view.wins, 0);
    assert_eq!(view.losses, 0);
    for game in &view.games {
        assert_eq!(game.result, GameResult::Unknown);
    }
}

#[test]
fn an_unfinished_match_is_never_decided() {
    let units = vec![unit(UNIT_01), unit(UNIT_02)];
    let now_ms = last_at(&units) + QUIET_MS;
    let found = folded_match(&units, now_ms);
    let view = &found.view;
    assert_eq!(view.state, MatchState::Abandoned);
    assert_eq!(view.resolution, Some(Resolution::Abandoned));
    assert_eq!(view.outcome, Outcome::Unknown);
    assert_eq!(view.tentative_outcome, None);
}

#[test]
fn a_repeated_frame_counts_once() {
    let once = full_match();
    let units = vec![
        unit(UNIT_01),
        unit(UNIT_02),
        unit(UNIT_03),
        unit(UNIT_04),
        unit(UNIT_04),
        unit(UNIT_05),
    ];
    let now_ms = last_at(&units) + HOUR_MS;
    assert_eq!(folded_match(&units, now_ms), once);
}

#[test]
fn mulligans_and_play_order_come_from_the_games() {
    let found = full_match();
    let games = &found.view.games;
    // Game 1: the opponent went to six and went first.
    assert_eq!(games[0].local_mulligans, Some(0));
    assert_eq!(games[0].opp_mulligans, Some(1));
    assert_eq!(games[0].on_play, Some(false));
    assert_eq!(games[0].turn_count, Some(12));
    // Game 3: the user went to six and went first.
    assert_eq!(games[2].local_mulligans, Some(1));
    assert_eq!(games[2].opp_mulligans, Some(0));
    assert_eq!(games[2].on_play, Some(true));
    assert_eq!(games[2].turn_count, Some(7));
    // A game starts at its first frame and ends at its outcome.
    assert_eq!(games[1].started_at_ms, games[0].ended_at_ms);
    for game in games {
        assert!(game.started_at_ms <= game.ended_at_ms);
    }
}

fn set_match_bodies(unit: &mut Unit, change: impl Fn(&mut endstep_protocol::MatchBody)) {
    for frame in &mut unit.frames {
        if let FrameKind::MatchBody(body) = &mut frame.kind {
            change(body);
        }
    }
}

#[test]
fn bot_and_spectator_matches_are_dropped() {
    let mut with_bot = unit(UNIT_01);
    set_match_bodies(&mut with_bot, |body| body.participants[1].bot = true);
    let units = vec![with_bot, unit(UNIT_02), unit(UNIT_04)];
    assert_eq!(
        fold_match(FORGE_GAME, &units, last_at(&units)),
        Folded::Dropped(DropReason::Bot)
    );

    let mut spectating = unit(UNIT_01);
    set_match_bodies(&mut spectating, |body| body.my_player_index = None);
    let units = vec![spectating, unit(UNIT_02), unit(UNIT_04)];
    assert_eq!(
        fold_match(FORGE_GAME, &units, last_at(&units)),
        Folded::Dropped(DropReason::Spectator)
    );

    let units = vec![unit(UNIT_02), unit(UNIT_04)];
    assert_eq!(
        fold_match(FORGE_GAME, &units, last_at(&units)),
        Folded::Dropped(DropReason::NoMatchBody)
    );
}

#[test]
fn another_games_match_body_is_not_read() {
    let units = all_units();
    assert_eq!(
        fold_match("another-forge-game", &units, last_at(&units)),
        Folded::Dropped(DropReason::NoMatchBody)
    );
}

#[test]
fn deck_from_names_when_no_deck_body() {
    let mut session = unit(UNIT_01);
    session
        .frames
        .retain(|frame| !matches!(frame.kind, FrameKind::DeckBody(_) | FrameKind::LobbyBody(_)));
    let units = vec![
        session,
        unit(UNIT_02),
        unit(UNIT_03),
        unit(UNIT_04),
        unit(UNIT_05),
    ];
    let found = folded_match(&units, last_at(&units) + HOUR_MS);
    let deck = found.deck.expect("a deck from the match body");
    assert_eq!(
        deck.key,
        "endstep:match:00000000-0000-4000-8000-e0000000003a"
    );
    assert_eq!(deck.name, "Endstep deck");
    assert_eq!(deck.format, "Modern");
    assert_eq!(card_total(&deck.main), 60);
    assert_eq!(card_total(&deck.side), 15);
    let mycospawn = EndstepCardKey {
        name: "Sowing Mycospawn".to_string(),
        set_code: "mh3".to_string(),
        collector_number: "340".to_string(),
    };
    assert!(deck.main.contains(&(mycospawn, 4)), "{:?}", deck.main);
}

/// A unit after every fixture unit holding the fixture's deck body edited
/// (same deck id, one card line left in each section).
fn later_edited_deck_unit(units: &[Unit]) -> Unit {
    let mut deck = None;
    for unit in units {
        for frame in &unit.frames {
            if let FrameKind::DeckBody(body) = &frame.kind {
                deck = Some(body.clone());
            }
        }
    }
    let mut deck = deck.expect("the fixture has a deck body");
    deck.main.truncate(1);
    deck.side.truncate(1);
    let at = last_at(units) + MINUTE_MS;
    Unit {
        unit_id: "00000000-0000-4000-8000-0000000000ed".to_string(),
        started_at: at,
        counter: 99,
        frames: vec![endstep_protocol::Frame {
            at,
            kind: FrameKind::DeckBody(deck),
        }],
    }
}

#[test]
fn a_deck_edited_after_the_match_keeps_the_match_deck() {
    let mut units = all_units();
    let edited = later_edited_deck_unit(&units);
    units.push(edited);
    let now_ms = last_at(&units) + HOUR_MS;
    let found = folded_match(&units, now_ms);
    let deck = found.deck.as_ref().expect("a deck");
    assert_eq!(card_total(&deck.main), 60);
    assert_eq!(card_total(&deck.side), 15);
    assert_eq!(found.deck, full_match().deck);
}

#[test]
fn a_deck_body_only_after_the_match_body_is_used() {
    let mut units = all_units();
    let edited = later_edited_deck_unit(&units);
    for unit in &mut units {
        unit.frames
            .retain(|frame| !matches!(frame.kind, FrameKind::DeckBody(_)));
    }
    units.push(edited);
    let found = folded_match(&units, last_at(&units) + HOUR_MS);
    let deck = found.deck.as_ref().expect("a deck");
    assert_eq!(deck.key, "endstep:00000000-0000-4000-8000-e00000000031");
    assert!(card_total(&deck.main) < 60, "the first deck body after it");
}

fn card_lines(lines: &[(EndstepCardKey, u32)]) -> Vec<serde_json::Value> {
    let mut out = Vec::new();
    for (key, count) in lines {
        out.push(serde_json::json!([
            key.name,
            key.set_code,
            key.collector_number,
            count
        ]));
    }
    out
}

/// Each game's rows as one compact line per row: side, name, set, number,
/// main, side, seen, cast, played, activated, opening hand, bottomed, starting
/// hand, sided in, sided out, discarded, hand seen, graveyard seen, exile
/// seen, battlefield seen, has zone data.
fn game_card_lines(games: &[EndstepGameCards]) -> Vec<serde_json::Value> {
    let mut out = Vec::new();
    for game in games {
        let mut rows = Vec::new();
        for (key, row) in &game.rows {
            rows.push(serde_json::json!([
                row.side.as_str(),
                key.name,
                key.set_code,
                key.collector_number,
                row.main_qty,
                row.side_qty,
                row.seen,
                row.cast,
                row.played,
                row.activated,
                row.opening_hand,
                row.bottomed,
                row.starting_hand,
                row.sided_in,
                row.sided_out,
                row.discarded,
                row.hand_seen,
                row.graveyard_seen,
                row.exile_seen,
                row.battlefield_seen,
                row.has_zone_data,
            ]));
        }
        let mulligans: Vec<serde_json::Value> = game
            .mulligans
            .iter()
            .map(|(number, key, count)| serde_json::json!([number, key.name, count]))
            .collect();
        out.push(serde_json::json!({
            "game": game.game_number,
            "opening_hand_size": game.opening_hand_size,
            "starting_hand_size": game.starting_hand_size,
            "opp_opening_hand_size": game.opp_opening_hand_size,
            "opp_starting_hand_size": game.opp_starting_hand_size,
            "mulligans": mulligans,
            "rows": rows,
        }));
    }
    out
}

#[test]
fn the_ranked_bo3_snapshot() {
    let found = full_match();
    let deck = found.deck.as_ref().expect("a deck");
    let ranked = found.ranked.as_ref().expect("a ranked result");
    let summary = serde_json::json!({
        "view": found.view,
        "account": [found.account.user_id, found.account.username],
        "opponent": found.opponent.as_ref().map(|p| [p.user_id.clone(), p.username.clone()]),
        "deck": {
            "key": deck.key,
            "name": deck.name,
            "format": deck.format,
            "main": card_lines(&deck.main),
            "side": card_lines(&deck.side),
        },
        "ranked": {
            "db_id": ranked.db_id,
            "status": ranked.status,
            "rating_before": ranked.rating_before,
            "rating_after": ranked.rating_after,
            "delta": ranked.delta,
        },
        "forge_game_id": found.forge_game_id,
        "game_cards": game_card_lines(&found.game_cards),
    });
    insta::assert_json_snapshot!("the_ranked_bo3_snapshot", summary);
}

#[test]
fn an_empty_printing_takes_the_deck_art() {
    let mut session = unit(UNIT_01);
    for frame in &mut session.frames {
        if let FrameKind::DeckBody(deck) = &mut frame.kind {
            for line in &mut deck.main {
                if line.name == "Sowing Mycospawn" {
                    line.set_code = Some(String::new());
                    line.collector_number = Some(String::new());
                }
            }
        }
    }
    let units = vec![session, unit(UNIT_02), unit(UNIT_04)];
    let found = folded_match(&units, last_at(&units) + HOUR_MS);
    let deck = found.deck.expect("a deck");
    let mycospawn = EndstepCardKey {
        name: "Sowing Mycospawn".to_string(),
        set_code: "mh3".to_string(),
        collector_number: "340".to_string(),
    };
    assert!(deck.main.contains(&(mycospawn, 4)), "{:?}", deck.main);
}

#[test]
fn a_hostile_clock_does_not_overflow() {
    let units = vec![unit(UNIT_01), unit(UNIT_02)];
    let found = folded_match(&units, i64::MIN);
    assert_eq!(found.view.state, MatchState::InProgress);
}

/// Unit 04 without game 2's game socket frames: everything after game 1's
/// `GAME_OUTCOME` up to and including game 2's.
fn unit_04_without_game_2() -> Unit {
    let mut unit = unit(UNIT_04);
    let mut outcomes = 0;
    unit.frames.retain(|frame| {
        let is_game_frame = matches!(
            frame.kind,
            FrameKind::GameState(_) | FrameKind::GameDelta(_) | FrameKind::GameEvent(_)
        );
        let is_outcome =
            matches!(&frame.kind, FrameKind::GameEvent(event) if event.kind == "GAME_OUTCOME");
        let in_game_2 = outcomes == 1;
        if is_outcome {
            outcomes += 1;
        }
        !(is_game_frame && in_game_2)
    });
    unit
}

#[test]
fn a_missing_middle_game_gets_a_placeholder() {
    let units = vec![
        unit(UNIT_01),
        unit(UNIT_02),
        unit(UNIT_03),
        unit_04_without_game_2(),
        unit(UNIT_05),
    ];
    let found = folded_match(&units, last_at(&units) + HOUR_MS);
    let games = &found.view.games;
    let numbers: Vec<u32> = games.iter().map(|game| game.game_number).collect();
    assert_eq!(numbers, vec![1, 2, 3]);
    let results: Vec<GameResult> = games.iter().map(|game| game.result).collect();
    assert_eq!(
        results,
        vec![GameResult::Win, GameResult::Unknown, GameResult::Loss]
    );
    assert_eq!(games[1].end_reason, EndReason::Unknown);
    assert_eq!(games[1].started_at_ms, None);
    assert_eq!(games[1].local_mulligans, None);
    // The final score still says 1-2.
    assert_eq!(found.view.wins, 1);
    assert_eq!(found.view.losses, 2);
}

fn set_first_state(unit: &mut Unit, change: impl Fn(&mut endstep_protocol::GameState)) {
    for frame in &mut unit.frames {
        if let FrameKind::GameState(state) = &mut frame.kind {
            change(state);
            return;
        }
    }
}

#[test]
fn a_hostile_game_number_adds_no_game() {
    let mut first = unit(UNIT_02);
    set_first_state(&mut first, |state| state.game_number = Some(100_000));
    let units = vec![
        unit(UNIT_01),
        first,
        unit(UNIT_03),
        unit(UNIT_04),
        unit(UNIT_05),
    ];
    let found = folded_match(&units, last_at(&units) + HOUR_MS);
    let numbers: Vec<u32> = found.view.games.iter().map(|g| g.game_number).collect();
    assert_eq!(numbers, vec![1, 2, 3]);
}

#[test]
fn a_hostile_games_played_makes_at_most_seven_games() {
    let mut session = unit(UNIT_01);
    set_match_bodies(&mut session, |body| body.games_per_match = None);
    let mut first = unit(UNIT_02);
    set_first_state(&mut first, |state| state.games_played = Some(100_000));
    let units = vec![session, first];
    let found = folded_match(&units, last_at(&units) + MINUTE_MS);
    assert_eq!(found.view.games.len(), 7);
    assert_eq!(found.view.game_structure, None);
}

fn game_rows(found: &EndstepMatch, number: u32) -> &[(EndstepCardKey, GameCardRow)] {
    let game = found
        .game_cards
        .iter()
        .find(|game| game.game_number == number)
        .expect("the game has card rows");
    &game.rows
}

fn row<'a>(
    rows: &'a [(EndstepCardKey, GameCardRow)],
    side: Side,
    name: &str,
) -> Option<&'a (EndstepCardKey, GameCardRow)> {
    rows.iter()
        .find(|(key, row)| row.side == side && key.name == name)
}

#[test]
fn each_game_has_card_rows() {
    let found = full_match();
    let deck = found.deck.as_ref().expect("a deck");
    let numbers: Vec<u32> = found.game_cards.iter().map(|g| g.game_number).collect();
    assert_eq!(numbers, vec![1, 2, 3]);
    for game in &found.game_cards {
        let mut main = 0;
        let mut side = 0;
        for (_, row) in &game.rows {
            assert_eq!(row.catalog_id, 0, "the store fills catalog ids");
            if row.side == Side::Local {
                main += row.main_qty;
                side += row.side_qty;
            }
        }
        assert_eq!(main, card_total(&deck.main), "game {}", game.game_number);
        assert_eq!(side, card_total(&deck.side), "game {}", game.game_number);
    }

    let rows = game_rows(&found, 1);
    let (key, maze) = row(rows, Side::Opponent, "Hedge Maze").expect("Hedge Maze row");
    assert!(maze.seen >= 1);
    assert_eq!(maze.played, Some(1));
    assert_eq!(maze.main_qty, 0);
    assert_eq!(key.set_code, "", "opponent rows are name only");
    assert_eq!(key.collector_number, "");

    // Rows are sorted by side, then key.
    for game in &found.game_cards {
        let mut sorted = game.rows.clone();
        sorted.sort_by(|a, b| (a.1.side, &a.0).cmp(&(b.1.side, &b.0)));
        assert_eq!(sorted, game.rows);
    }
}

#[test]
fn local_casts_are_counted() {
    let found = full_match();
    let rows = game_rows(&found, 1);
    let (key, map) = row(rows, Side::Local, "Expedition Map").expect("Expedition Map row");
    assert_eq!(map.cast, 1);
    assert!(map.seen >= 1);
    assert_eq!(map.played, Some(0));
    assert_eq!(map.activated.map(|count| count >= 1), Some(true));
    // The deck's printing.
    assert_eq!(key.set_code, "2xm");
    assert_eq!(key.collector_number, "359");
}

fn state_card(id: u64, name: &str, zone: Zone, owner_seat: u32) -> StateCard {
    StateCard {
        id,
        name: name.to_string(),
        zone,
        owner_seat: Some(owner_seat),
        is_token: false,
        face_down: false,
        has_types: true,
        set_code: String::new(),
        collector_number: String::new(),
        controller_seat: None,
        tapped: false,
        power: None,
        toughness: None,
        types: Vec::new(),
        counters: Vec::new(),
        attached_to: None,
    }
}

/// A match unit holding one game 1 state with `cards`.
fn state_unit(cards: Vec<StateCard>) -> Unit {
    let state = GameState {
        seq: Some(1),
        match_id: None,
        game_id: FORGE_GAME.to_string(),
        game_number: Some(1),
        games_played: Some(0),
        wins_by_seat: vec![0, 0],
        is_match_over: false,
        turn_number: Some(1),
        active_player: Some(0),
        winner: None,
        players: Vec::new(),
        player_states: Vec::new(),
        sideboarding: false,
        prompt: None,
        cards,
        phase: None,
        step: None,
        priority_player: None,
        server_now_ms: None,
        stack: Vec::new(),
    };
    Unit {
        unit_id: "00000000-0000-4000-8000-0000000000aa".to_string(),
        started_at: 1_900_000_000_000,
        counter: 1,
        frames: vec![Frame {
            at: 1_900_000_000_000,
            kind: FrameKind::GameState(state),
        }],
    }
}

#[test]
fn hidden_face_down_and_tokens_are_not_rows() {
    let found = full_match();
    for game in &found.game_cards {
        for (key, _) in &game.rows {
            assert_ne!(key.name, "Hidden card");
        }
    }

    let mut token = state_card(1, "Eldrazi Spawn", Zone::Battlefield, 0);
    token.is_token = true;
    let mut opp_token = state_card(2, "Opp Token", Zone::Battlefield, 1);
    opp_token.is_token = true;
    let mut face_down = state_card(3, "Face Down Card", Zone::Battlefield, 1);
    face_down.face_down = true;
    let library = state_card(4, "Library Card", Zone::Library, 1);
    let shown = state_card(5, "Shown Card", Zone::Battlefield, 1);
    let units = vec![
        unit(UNIT_01),
        state_unit(vec![token, opp_token, face_down, library, shown]),
    ];
    let found = folded_match(&units, 1_900_000_000_000);
    let rows = game_rows(&found, 1);
    for name in [
        "Eldrazi Spawn",
        "Opp Token",
        "Face Down Card",
        "Library Card",
    ] {
        assert!(
            rows.iter().all(|(key, _)| key.name != name),
            "{name} has a row"
        );
    }
    let (_, shown) = row(rows, Side::Opponent, "Shown Card").expect("a shown card row");
    assert_eq!(shown.seen, 1);
}

#[test]
fn seen_counts_distinct_cards_and_sides_by_owner() {
    // Card 7 shows in hand, then on the battlefield: one card. A local card
    // outside the deck keeps its state printing. A local command card makes
    // no row; an opponent's does.
    let mut extra = state_card(8, "Not In Deck", Zone::Hand, 0);
    extra.set_code = "MH3".to_string();
    extra.collector_number = "12".to_string();
    let units = vec![
        unit(UNIT_01),
        state_unit(vec![
            state_card(7, "Opp Card", Zone::Hand, 1),
            state_card(7, "Opp Card", Zone::Battlefield, 1),
            extra,
            state_card(9, "Local Emblem", Zone::Command, 0),
            state_card(10, "Opp Emblem", Zone::Command, 1),
        ]),
    ];
    let found = folded_match(&units, 1_900_000_000_000);
    let rows = game_rows(&found, 1);
    let (_, opp) = row(rows, Side::Opponent, "Opp Card").expect("opponent row");
    assert_eq!(opp.seen, 1);
    let (key, extra) = row(rows, Side::Local, "Not In Deck").expect("local extra row");
    assert_eq!(extra.main_qty, 0);
    assert_eq!(extra.seen, 1);
    assert_eq!(key.set_code, "mh3");
    assert_eq!(key.collector_number, "12");
    assert!(row(rows, Side::Local, "Local Emblem").is_none());
    assert_eq!(
        row(rows, Side::Opponent, "Opp Emblem").map(|r| r.1.seen),
        Some(1)
    );
}

/// Every message of the kinds the game log leaves out, across all units.
fn left_out_messages(units: &[Unit]) -> Vec<String> {
    let mut out = Vec::new();
    for unit in units {
        for frame in &unit.frames {
            if let FrameKind::GameEvent(event) = &frame.kind {
                let left_out = matches!(
                    event.kind.as_str(),
                    "TURN_PHASE" | "CARD_ZONE_CHANGE" | "MANA_LOG"
                );
                if let (true, Some(message)) = (left_out, &event.message) {
                    out.push(message.clone());
                }
            }
        }
    }
    out
}

#[test]
fn log_lines_follow_game_numbers() {
    let units = all_units();
    let found = full_match();
    let game_2 = &found.view.games[1];
    let start = game_2.started_at_ms.expect("game 2 start");
    let end = game_2.ended_at_ms.expect("game 2 end");
    let lines = game_log(FORGE_GAME, &units, 2);
    assert!(!lines.is_empty());
    let left_out = left_out_messages(&units);
    assert!(!left_out.is_empty());
    for line in &lines {
        assert!(
            start <= line.at_ms && line.at_ms <= end,
            "{} at {}",
            line.message,
            line.at_ms
        );
        assert!(!line.message.is_empty());
        assert!(!left_out.contains(&line.message), "{}", line.message);
    }
    let game_1 = game_log(FORGE_GAME, &units, 1);
    assert!(
        game_1
            .iter()
            .any(|line| line.message == "Player cast Expedition Map")
    );
    assert!(game_log(FORGE_GAME, &units, 4).is_empty());
}

#[test]
fn rows_do_not_depend_on_unit_order() {
    let mut units = all_units();
    let now_ms = last_at(&units) + HOUR_MS;
    let forward = folded_match(&units, now_ms);
    units.reverse();
    let reversed = folded_match(&units, now_ms);
    assert_eq!(forward.game_cards, reversed.game_cards);

    let reversed_log = game_log(FORGE_GAME, &units, 3);
    units.reverse();
    let forward_log = game_log(FORGE_GAME, &units, 3);
    assert_eq!(forward_log, reversed_log);
}

#[test]
fn effect_objects_are_not_rows() {
    // A Forge effect has no types; a real command card has some.
    let mut effect = state_card(11, "Opp Card's Effect", Zone::Command, 1);
    effect.has_types = false;
    let mut local_effect = state_card(12, "Local Card's Effect", Zone::Battlefield, 0);
    local_effect.has_types = false;
    let units = vec![
        unit(UNIT_01),
        state_unit(vec![
            effect,
            local_effect,
            state_card(13, "Opp Commander", Zone::Command, 1),
        ]),
    ];
    let found = folded_match(&units, 1_900_000_000_000);
    let rows = game_rows(&found, 1);
    assert!(row(rows, Side::Opponent, "Opp Card's Effect").is_none());
    assert!(row(rows, Side::Local, "Local Card's Effect").is_none());
    assert_eq!(
        row(rows, Side::Opponent, "Opp Commander").map(|r| r.1.seen),
        Some(1)
    );
}

fn game_cards_of(found: &EndstepMatch, number: u32) -> &EndstepGameCards {
    found
        .game_cards
        .iter()
        .find(|game| game.game_number == number)
        .expect("the game has card rows")
}

fn local_total(game: &EndstepGameCards, pick: impl Fn(&GameCardRow) -> u32) -> u32 {
    game.rows
        .iter()
        .filter(|(_, row)| row.side == Side::Local)
        .map(|(_, row)| pick(row))
        .sum()
}

#[test]
fn kept_sevens_have_no_bottomed_cards() {
    let found = full_match();
    for number in [1, 2] {
        let game = game_cards_of(&found, number);
        assert_eq!(game.opening_hand_size, Some(7), "game {number}");
        assert_eq!(game.starting_hand_size, Some(7), "game {number}");
        assert_eq!(local_total(game, |row| row.opening_hand), 7);
        assert_eq!(local_total(game, |row| row.bottomed), 0);
        assert!(game.mulligans.is_empty());
    }
}

#[test]
fn game_three_has_a_mulligan_hand_and_a_bottomed_card() {
    let found = full_match();
    let game = game_cards_of(&found, 3);
    let hand_one: Vec<&(u32, EndstepCardKey, u32)> =
        game.mulligans.iter().filter(|line| line.0 == 1).collect();
    assert_eq!(game.mulligans.len(), hand_one.len(), "one mulligan hand");
    assert_eq!(hand_one.iter().map(|line| line.2).sum::<u32>(), 7);
    for name in ["Trinisphere", "Forest"] {
        assert!(
            hand_one.iter().any(|line| line.1.name == name),
            "{name} was sent back"
        );
    }
    assert_eq!(local_total(game, |row| row.opening_hand), 7);
    let (_, temple) = row(&game.rows, Side::Local, "Eldrazi Temple").expect("temple row");
    assert_eq!(temple.opening_hand, 2);
    assert_eq!(local_total(game, |row| row.bottomed), 1);
    let bottomed: Vec<&(EndstepCardKey, GameCardRow)> = game
        .rows
        .iter()
        .filter(|(_, row)| row.side == Side::Local && row.bottomed > 0)
        .collect();
    assert!(
        bottomed[0].0.name.starts_with("Karn"),
        "{:?}",
        bottomed[0].0
    );
    assert_eq!(game.opening_hand_size, Some(7));
    assert_eq!(game.starting_hand_size, Some(6));
    assert_eq!(local_total(game, |row| row.starting_hand), 6);
}

#[test]
fn opponent_sizes_come_from_hand_settled() {
    let found = full_match();
    let one = game_cards_of(&found, 1);
    assert_eq!(one.opp_opening_hand_size, Some(7));
    assert_eq!(one.opp_starting_hand_size, Some(6));
    let two = game_cards_of(&found, 2);
    assert_eq!(two.opp_opening_hand_size, Some(7));
    assert_eq!(two.opp_starting_hand_size, Some(7));
}

#[test]
fn starting_is_opening_minus_bottomed() {
    let found = full_match();
    for game in &found.game_cards {
        for (key, row) in &game.rows {
            assert_eq!(
                row.starting_hand,
                row.opening_hand - row.bottomed,
                "game {} {}",
                game.game_number,
                key.name
            );
        }
    }
}

/// Unit 04 without the game frames of game 2 before its first `HAND_SETTLED`:
/// the game is joined after the keep.
fn unit_04_joined_game_2_late() -> Unit {
    let mut unit = unit(UNIT_04);
    let mut outcomes = 0;
    let mut settled = false;
    unit.frames.retain(|frame| {
        let is_game_frame = matches!(
            frame.kind,
            FrameKind::GameState(_) | FrameKind::GameDelta(_) | FrameKind::GameEvent(_)
        );
        match &frame.kind {
            FrameKind::GameEvent(event) if event.kind == "GAME_OUTCOME" => outcomes += 1,
            FrameKind::GameEvent(event) if event.kind == "HAND_SETTLED" && outcomes == 1 => {
                settled = true;
            }
            _ => {}
        }
        !(is_game_frame && outcomes == 1 && !settled)
    });
    unit
}

#[test]
fn a_game_joined_after_the_keep_has_no_hand() {
    let units = vec![
        unit(UNIT_01),
        unit(UNIT_02),
        unit(UNIT_03),
        unit_04_joined_game_2_late(),
        unit(UNIT_05),
    ];
    let found = folded_match(&units, last_at(&units) + HOUR_MS);
    let game = game_cards_of(&found, 2);
    assert_eq!(game.opening_hand_size, None);
    assert_eq!(game.starting_hand_size, None);
    assert!(game.mulligans.is_empty());
    assert_eq!(local_total(game, |row| row.opening_hand), 0);
    assert_eq!(local_total(game, |row| row.bottomed), 0);
    assert!(!game.rows.is_empty(), "the card rows stay");
    // The other games keep their hands.
    assert_eq!(game_cards_of(&found, 1).opening_hand_size, Some(7));
    // The first HAND_SETTLED of a seat is the one that counts.
    assert_eq!(game_cards_of(&found, 1).opp_starting_hand_size, Some(6));
    assert_eq!(game_cards_of(&found, 3).starting_hand_size, Some(6));
}

fn hand_state(seq: u64, hand: &[(u64, &str)], elsewhere: &[(u64, &str)]) -> Frame {
    let mut cards = Vec::new();
    for (id, name) in hand {
        cards.push(state_card(*id, name, Zone::Hand, 0));
    }
    for (id, name) in elsewhere {
        cards.push(state_card(*id, name, Zone::Battlefield, 0));
    }
    let player = |seat: u32| PlayerState {
        seat,
        life: Some(20),
        hand_size: Some(if seat == 0 { hand.len() as u32 } else { 7 }),
        library_size: Some(53),
        poison: None,
    };
    let state = GameState {
        seq: Some(seq),
        match_id: None,
        game_id: FORGE_GAME.to_string(),
        game_number: Some(1),
        games_played: Some(0),
        wins_by_seat: vec![0, 0],
        is_match_over: false,
        turn_number: Some(0),
        active_player: None,
        winner: None,
        players: Vec::new(),
        player_states: vec![player(0), player(1)],
        sideboarding: false,
        prompt: None,
        cards,
        phase: None,
        step: None,
        priority_player: None,
        server_now_ms: None,
        stack: Vec::new(),
    };
    Frame {
        at: 1_900_000_000_000 + seq as i64,
        kind: FrameKind::GameState(state),
    }
}

fn hand_event(
    seq: u64,
    kind: &str,
    seat: u32,
    amount: Option<u32>,
    mulligans: Option<u32>,
) -> Frame {
    let event = GameEvent {
        seq: Some(seq),
        game_id: FORGE_GAME.to_string(),
        kind: kind.to_string(),
        sequence_number: None,
        player_index: Some(seat),
        amount,
        turn_number: None,
        end_reason: None,
        message: None,
        card_id: None,
        card_name: None,
        from_zone: None,
        to_zone: None,
        zone_owner: None,
        during_mulligan: false,
        mulligans,
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
    };
    Frame {
        at: 1_900_000_000_000 + seq as i64,
        kind: FrameKind::GameEvent(event),
    }
}

fn hand_game_unit(frames: Vec<Frame>) -> Unit {
    Unit {
        unit_id: "00000000-0000-4000-8000-0000000000bb".to_string(),
        started_at: 1_900_000_000_000,
        counter: 1,
        frames,
    }
}

const SEVEN: [(u64, &str); 7] = [
    (1, "Card A"),
    (2, "Card B"),
    (3, "Card C"),
    (4, "Card D"),
    (5, "Card E"),
    (6, "Card F"),
    (7, "Card G"),
];

#[test]
fn a_card_that_left_the_hand_another_way_is_not_bottomed() {
    let frames = vec![
        hand_state(1, &SEVEN, &[]),
        hand_event(2, "HAND_KEPT", 0, Some(7), None),
        hand_event(3, "HAND_SETTLED", 0, Some(7), Some(0)),
        hand_state(4, &SEVEN[1..], &[]),
    ];
    let units = vec![unit(UNIT_01), hand_game_unit(frames)];
    let found = folded_match(&units, 1_900_000_000_000);
    let game = game_cards_of(&found, 1);
    assert_eq!(game.opening_hand_size, Some(7));
    assert_eq!(game.starting_hand_size, Some(7));
    assert_eq!(local_total(game, |row| row.opening_hand), 7);
    assert_eq!(local_total(game, |row| row.bottomed), 0);
}

#[test]
fn a_sideboarding_state_does_not_fix_or_reset_a_hand() {
    let mut sideboarding = hand_state(3, &[], &[]);
    if let FrameKind::GameState(state) = &mut sideboarding.kind {
        state.sideboarding = true;
    }
    let frames = vec![
        hand_state(1, &SEVEN, &[]),
        sideboarding,
        hand_event(4, "HAND_KEPT", 0, Some(7), None),
    ];
    let units = vec![unit(UNIT_01), hand_game_unit(frames)];
    let found = folded_match(&units, 1_900_000_000_000);
    assert_eq!(game_cards_of(&found, 1).opening_hand_size, Some(7));
}

#[test]
fn hands_do_not_depend_on_unit_order() {
    let mut units = all_units();
    let now_ms = last_at(&units) + HOUR_MS;
    let forward = folded_match(&units, now_ms);
    units.reverse();
    let reversed = folded_match(&units, now_ms);
    assert_eq!(forward.game_cards, reversed.game_cards);
    assert_eq!(game_cards_of(&reversed, 3).starting_hand_size, Some(6));
}

fn with_turn(mut frame: Frame, turn: Option<u32>) -> Frame {
    if let FrameKind::GameState(state) = &mut frame.kind {
        state.turn_number = turn;
    }
    frame
}

#[test]
fn a_settled_without_amount_leaves_the_sizes_unknown() {
    let frames = vec![
        hand_state(1, &SEVEN, &[]),
        hand_event(2, "HAND_SETTLED", 1, None, Some(1)),
        hand_event(3, "HAND_SETTLED", 1, Some(7), Some(0)),
    ];
    let units = vec![unit(UNIT_01), hand_game_unit(frames)];
    let found = folded_match(&units, 1_900_000_000_000);
    let game = game_cards_of(&found, 1);
    assert_eq!(game.opp_opening_hand_size, None);
    assert_eq!(game.opp_starting_hand_size, None);
}

#[test]
fn a_keep_fixes_only_a_hand_from_turn_zero() {
    let late = vec![
        with_turn(hand_state(1, &SEVEN, &[]), Some(5)),
        hand_event(2, "HAND_KEPT", 0, Some(7), None),
    ];
    let units = vec![unit(UNIT_01), hand_game_unit(late)];
    let found = folded_match(&units, 1_900_000_000_000);
    assert_eq!(game_cards_of(&found, 1).opening_hand_size, None);
    assert_eq!(
        local_total(game_cards_of(&found, 1), |row| row.opening_hand),
        0
    );

    for turn in [Some(0), None] {
        let early = vec![
            with_turn(hand_state(1, &SEVEN, &[]), turn),
            hand_event(2, "HAND_KEPT", 0, Some(7), None),
        ];
        let units = vec![unit(UNIT_01), hand_game_unit(early)];
        let found = folded_match(&units, 1_900_000_000_000);
        assert_eq!(game_cards_of(&found, 1).opening_hand_size, Some(7));
    }
}

/// One local row per card name in the deck: name, main, side, sided in,
/// sided out. Cards a state showed outside the deck are left out.
fn deck_lines(game: &EndstepGameCards) -> Vec<(String, u32, u32, u32, u32)> {
    let mut lines = Vec::new();
    for (key, row) in &game.rows {
        let in_deck = row.main_qty + row.side_qty + row.sided_in + row.sided_out > 0;
        if row.side == Side::Local && in_deck {
            lines.push((
                key.name.clone(),
                row.main_qty,
                row.side_qty,
                row.sided_in,
                row.sided_out,
            ));
        }
    }
    lines.sort();
    lines
}

fn local_row<'a>(game: &'a EndstepGameCards, name: &str) -> &'a GameCardRow {
    &row(&game.rows, Side::Local, name)
        .unwrap_or_else(|| panic!("no local row for {name}"))
        .1
}

#[test]
fn game_one_plays_the_match_deck() {
    let found = full_match();
    let game = game_cards_of(&found, 1);
    assert_eq!(local_total(game, |row| row.main_qty), 60);
    assert_eq!(local_total(game, |row| row.side_qty), 15);
    assert_eq!(local_total(game, |row| row.sided_in), 0);
    assert_eq!(local_total(game, |row| row.sided_out), 0);
}

#[test]
fn game_two_sides_two_boulders_out() {
    let found = full_match();
    let one = game_cards_of(&found, 1);
    let two = game_cards_of(&found, 2);
    let boulder_one = local_row(one, "Giant's Boulder");
    let boulder_two = local_row(two, "Giant's Boulder");
    assert_eq!(boulder_two.sided_out, 2);
    assert_eq!(boulder_two.sided_in, 0);
    assert_eq!(boulder_two.main_qty, boulder_one.main_qty - 2);
    assert_eq!(boulder_two.side_qty, boulder_one.side_qty + 2);
    for name in ["Chalice of the Void", "Disruptor Flute"] {
        let card = local_row(two, name);
        assert_eq!(card.sided_in, 1, "{name}");
        assert_eq!(card.sided_out, 0, "{name}");
        assert_eq!(card.main_qty, local_row(one, name).main_qty + 1, "{name}");
    }
    assert_eq!(local_total(two, |row| row.main_qty), 60);
    assert_eq!(local_total(two, |row| row.side_qty), 15);
    assert_eq!(local_total(two, |row| row.sided_in), 2);
    assert_eq!(local_total(two, |row| row.sided_out), 2);
}

#[test]
fn game_three_keeps_the_game_two_deck() {
    let found = full_match();
    assert_eq!(
        deck_lines(game_cards_of(&found, 3)),
        deck_lines(game_cards_of(&found, 2))
    );
}

fn without_submits(unit: &mut Unit, versions: &[u64]) {
    unit.frames.retain(|frame| match &frame.kind {
        FrameKind::Action(action) => {
            !(action.kind == "SIDEBOARD_SUBMIT"
                && action
                    .prompt_version
                    .is_some_and(|version| versions.contains(&version)))
        }
        _ => true,
    });
}

fn match_with_unit_04(unit_04: Unit) -> EndstepMatch {
    let units = vec![
        unit(UNIT_01),
        unit(UNIT_02),
        unit(UNIT_03),
        unit_04,
        unit(UNIT_05),
    ];
    folded_match(&units, last_at(&units) + HOUR_MS)
}

/// Unit 04 up to and including game 2's `GAME_OUTCOME`: the match as if it
/// had ended after game 2, so no game 3 prompt shows game 2's deck.
fn unit_04_cut_after_game_2() -> Unit {
    let mut unit = unit(UNIT_04);
    let mut outcomes = 0;
    unit.frames.retain(|frame| {
        let kept = outcomes < 2;
        if matches!(&frame.kind, FrameKind::GameEvent(event) if event.kind == "GAME_OUTCOME") {
            outcomes += 1;
        }
        kept
    });
    unit
}

#[test]
fn a_submit_sets_the_deck_without_a_next_game() {
    let units = vec![
        unit(UNIT_01),
        unit(UNIT_02),
        unit(UNIT_03),
        unit_04_cut_after_game_2(),
    ];
    let found = folded_match(&units, last_at(&units) + HOUR_MS);
    let two = game_cards_of(&found, 2);
    assert_eq!(local_row(two, "Giant's Boulder").sided_out, 2);
    assert_eq!(local_row(two, "Chalice of the Void").sided_in, 1);
    assert_eq!(local_row(two, "Disruptor Flute").sided_in, 1);
}

#[test]
fn a_missing_submit_uses_the_next_prompt() {
    let full = full_match();
    let mut unit_04 = unit(UNIT_04);
    without_submits(&mut unit_04, &[268]);
    let found = match_with_unit_04(unit_04);
    // Game 3's prompt shows the deck game 2 was played with.
    assert_eq!(
        local_row(game_cards_of(&found, 2), "Giant's Boulder").sided_out,
        2
    );
    assert_eq!(
        deck_lines(game_cards_of(&found, 2)),
        deck_lines(game_cards_of(&full, 2))
    );
    assert_eq!(game_cards_of(&found, 2), game_cards_of(&full, 2));

    let mut unit_04 = unit(UNIT_04);
    without_submits(&mut unit_04, &[268, 401]);
    let found = match_with_unit_04(unit_04);
    assert_eq!(
        deck_lines(game_cards_of(&found, 3)),
        deck_lines(game_cards_of(&found, 2))
    );
    assert_eq!(
        deck_lines(game_cards_of(&found, 2)),
        deck_lines(game_cards_of(&full, 2))
    );
}

fn option_names() -> Vec<PromptCard> {
    (0..75)
        .map(|id| PromptCard {
            id,
            name: format!("Option {id}"),
            set_code: String::new(),
            collector_number: String::new(),
        })
        .collect()
}

fn sideboard_prompt(main_count: Option<u32>) -> Prompt {
    Prompt {
        kind: "CHOOSE_CARDS".to_string(),
        context: "sideboard".to_string(),
        version: Some(7),
        main_count,
        options: option_names(),
    }
}

fn game_two_sideboard_state(seq: u64, prompt: Prompt) -> Frame {
    let mut frame = hand_state(seq, &[], &[]);
    if let FrameKind::GameState(state) = &mut frame.kind {
        state.game_number = Some(2);
        state.sideboarding = true;
        state.prompt = Some(prompt);
    }
    frame
}

fn submit(ids: Vec<u64>, version: Option<u64>, match_id: Option<&str>) -> Frame {
    Frame {
        at: 1_900_000_000_500,
        kind: FrameKind::Action(GameAction {
            kind: "SIDEBOARD_SUBMIT".to_string(),
            ordered_cards: ids,
            prompt_version: version,
            match_id: match_id.map(str::to_string),
        }),
    }
}

/// Game 1 with a hand, then game 2's sideboard prompt followed by `after`.
fn two_game_match(main_count: Option<u32>, after: Vec<Frame>) -> EndstepMatch {
    let mut frames = vec![
        hand_state(1, &SEVEN, &[]),
        game_two_sideboard_state(2, sideboard_prompt(main_count)),
    ];
    frames.extend(after);
    let units = vec![unit(UNIT_01), hand_game_unit(frames)];
    folded_match(&units, 1_900_000_000_000)
}

fn has_option_rows(game: &EndstepGameCards) -> bool {
    game.rows
        .iter()
        .any(|(key, _)| key.name.starts_with("Option "))
}

#[test]
fn a_usable_submit_sets_the_game_deck() {
    let ids: Vec<u64> = (15..75).collect();
    let found = two_game_match(Some(60), vec![submit(ids, Some(7), None)]);
    let two = game_cards_of(&found, 2);
    assert_eq!(local_row(two, "Option 15").main_qty, 1);
    assert_eq!(local_row(two, "Option 0").main_qty, 0);
    assert_eq!(local_row(two, "Option 0").side_qty, 1);
    assert_eq!(local_row(two, "Option 15").sided_in, 1);
    assert_eq!(local_total(two, |row| row.main_qty), 60);
    // The match deck's cards are all sided out against game 1.
    assert_eq!(
        local_total(two, |row| row.sided_out),
        local_total(game_cards_of(&found, 1), |row| row.main_qty)
    );
}

fn versioned(prompt: Prompt, version: u64) -> Prompt {
    Prompt {
        version: Some(version),
        ..prompt
    }
}

/// A `GAME_DELTA` that re-sends a prompt, as Forge does after a submit.
fn prompt_delta(seq: u64, prompt: Prompt) -> Frame {
    Frame {
        at: 1_900_000_000_000 + seq as i64,
        kind: FrameKind::GameDelta(GameDelta {
            seq: Some(seq),
            match_score: None,
            prompt: Some(prompt),
            life: Vec::new(),
            phase: None,
            step: None,
            priority_player: None,
            server_now_ms: None,
        }),
    }
}

#[test]
fn a_prompt_sent_again_after_the_submit_keeps_the_answer() {
    let ids: Vec<u64> = (15..75).collect();
    let found = two_game_match(
        Some(60),
        vec![
            submit(ids, Some(7), None),
            prompt_delta(3, versioned(sideboard_prompt(Some(60)), 9)),
            prompt_delta(4, versioned(sideboard_prompt(Some(60)), 10)),
        ],
    );
    let two = game_cards_of(&found, 2);
    assert_eq!(local_row(two, "Option 15").main_qty, 1);
    assert_eq!(local_row(two, "Option 0").side_qty, 1);
    assert_eq!(local_total(two, |row| row.main_qty), 60);
}

#[test]
fn each_game_keeps_its_own_submitted_deck() {
    let prompt = sideboard_prompt(Some(60));
    let mut game_three = game_two_sideboard_state(5, versioned(prompt.clone(), 20));
    if let FrameKind::GameState(state) = &mut game_three.kind {
        state.game_number = Some(3);
    }
    let frames = vec![
        hand_state(1, &SEVEN, &[]),
        game_two_sideboard_state(2, prompt.clone()),
        submit((15..75).collect(), Some(7), None),
        prompt_delta(3, versioned(prompt.clone(), 9)),
        prompt_delta(4, versioned(prompt.clone(), 10)),
        game_three,
        submit((10..70).collect(), Some(20), None),
        prompt_delta(6, versioned(prompt.clone(), 22)),
        prompt_delta(7, versioned(prompt, 23)),
    ];
    let units = vec![unit(UNIT_01), hand_game_unit(frames)];
    let found = folded_match(&units, 1_900_000_000_000);
    let two = game_cards_of(&found, 2);
    let three = game_cards_of(&found, 3);
    assert_eq!(local_row(two, "Option 10").main_qty, 0);
    assert_eq!(local_row(two, "Option 74").main_qty, 1);
    assert_eq!(local_row(three, "Option 10").main_qty, 1);
    assert_eq!(local_row(three, "Option 74").main_qty, 0);
    assert_eq!(local_total(three, |row| row.main_qty), 60);
}

#[test]
fn a_hostile_submit_is_ignored() {
    let mut big: Vec<u64> = (0..60).collect();
    big.extend(std::iter::repeat_n(0, 100_000));
    let hostile: Vec<(&str, Vec<u64>)> = vec![
        ("ids outside the prompt", (900..960).collect()),
        ("one id sixty times", vec![0; 60]),
        ("too many ids", big),
    ];
    for (label, ids) in hostile {
        let found = two_game_match(Some(60), vec![submit(ids, Some(7), None)]);
        let one = game_cards_of(&found, 1);
        let two = game_cards_of(&found, 2);
        assert!(!has_option_rows(two), "{label}");
        assert_eq!(deck_lines(two), deck_lines(one), "{label}");
    }

    // A prompt that cannot give a main deck.
    for main_count in [Some(500), None, Some(0)] {
        let ids: Vec<u64> = (0..60).collect();
        let found = two_game_match(main_count, vec![submit(ids, Some(7), None)]);
        let two = game_cards_of(&found, 2);
        assert!(!has_option_rows(two), "main_count {main_count:?}");
        assert_eq!(deck_lines(two), deck_lines(game_cards_of(&found, 1)));
    }
}

#[test]
fn a_submit_for_another_prompt_or_match_is_ignored() {
    let ids: Vec<u64> = (15..75).collect();
    let other = [
        submit(ids.clone(), Some(8), None),
        submit(ids.clone(), None, None),
        submit(ids.clone(), Some(7), Some("another-match")),
    ];
    for frame in other {
        let found = two_game_match(Some(60), vec![frame]);
        assert!(!has_option_rows(game_cards_of(&found, 2)));
    }
    // A submit before any state of the game is not tied to it.
    let mut early = hand_game_unit(vec![
        submit(ids, Some(7), None),
        hand_state(1, &SEVEN, &[]),
        game_two_sideboard_state(2, sideboard_prompt(Some(60))),
    ]);
    early.counter = 2;
    let units = vec![unit(UNIT_01), early];
    let found = folded_match(&units, 1_900_000_000_000);
    assert!(!has_option_rows(game_cards_of(&found, 2)));
}

#[test]
fn sideboarding_does_not_depend_on_unit_order() {
    let mut units = all_units();
    let now_ms = last_at(&units) + HOUR_MS;
    let forward = folded_match(&units, now_ms);
    units.reverse();
    let reversed = folded_match(&units, now_ms);
    assert_eq!(forward.game_cards, reversed.game_cards);
    assert_eq!(
        local_row(game_cards_of(&reversed, 2), "Chalice of the Void").sided_in,
        1
    );
}

fn sorted_units(units: &[Unit]) -> Vec<&Unit> {
    let mut ordered: Vec<&Unit> = units.iter().collect();
    ordered.sort_by(|a, b| {
        (a.started_at, a.counter, &a.unit_id).cmp(&(b.started_at, b.counter, &b.unit_id))
    });
    ordered
}

/// The game states and events of game `number`, in fold order. The game
/// changes on a state that names its game number, as in the fold.
fn game_frames(units: &[Unit], number: u32) -> Vec<&FrameKind> {
    let mut current = 1;
    let mut out = Vec::new();
    for unit in sorted_units(units) {
        for frame in &unit.frames {
            match &frame.kind {
                FrameKind::GameState(state) | FrameKind::GameOver(state) => {
                    if let Some(game_number) = state.game_number {
                        current = game_number;
                    }
                    if current == number {
                        out.push(&frame.kind);
                    }
                }
                FrameKind::GameEvent(_) if current == number => out.push(&frame.kind),
                _ => {}
            }
        }
    }
    out
}

fn is_named(name: &str) -> bool {
    !name.is_empty() && name != "Hidden card"
}

/// The distinct ids of the opponent's (seat 1) cards that game `number`'s
/// states, reveals and casts showed.
fn opponent_card_ids(units: &[Unit], number: u32) -> BTreeSet<u64> {
    let mut ids = BTreeSet::new();
    let mut tokens = BTreeSet::new();
    for kind in game_frames(units, number) {
        match kind {
            FrameKind::GameState(state) | FrameKind::GameOver(state) => {
                for card in &state.cards {
                    if card.is_token {
                        tokens.insert(card.id);
                    }
                    let shown = !card.is_token
                        && !card.face_down
                        && card.has_types
                        && card.zone != Zone::Library;
                    if shown && card.owner_seat == Some(1) {
                        ids.insert(card.id);
                    }
                }
            }
            FrameKind::GameEvent(event) => {
                if event.kind == "CARD_REVEALED" && !event.owner_echo {
                    for (id, name) in event.card_ids.iter().zip(&event.card_names) {
                        if is_named(name) {
                            ids.insert(*id);
                        }
                    }
                }
                let left_hand = event.kind == "CARD_ZONE_CHANGE"
                    && event.from_zone.as_deref() == Some("Hand")
                    && event.zone_owner == Some(1);
                let cast = (event.kind == "SPELL_CAST" || event.kind == "LAND_PLAYED")
                    && event.player_index == Some(1);
                if let (true, Some(id), Some(name)) =
                    (left_hand || cast, event.card_id, &event.card_name)
                    && is_named(name)
                {
                    ids.insert(id);
                }
            }
            _ => {}
        }
    }
    ids.retain(|id| !tokens.contains(id));
    ids
}

fn opponent_rows_of(found: &EndstepMatch, number: u32) -> Vec<&(EndstepCardKey, GameCardRow)> {
    game_rows(found, number)
        .iter()
        .filter(|(_, row)| row.side == Side::Opponent)
        .collect()
}

#[test]
fn revealed_cards_are_opponent_rows() {
    let found = full_match();
    let rows = game_rows(&found, 1);
    for name in ["Street Wraith", "Thundering Falls"] {
        let (_, card) =
            row(rows, Side::Opponent, name).unwrap_or_else(|| panic!("no opponent row for {name}"));
        assert!(card.seen >= 1, "{name}");
    }
}

#[test]
fn a_library_look_counts_each_card_once() {
    let units = all_units();
    let found = folded_match(&units, last_at(&units) + HOUR_MS);
    let expected = opponent_card_ids(&units, 1).len();
    let total: usize = opponent_rows_of(&found, 1)
        .iter()
        .map(|(_, row)| row.seen as usize)
        .sum();
    assert_eq!(total, expected);
    // Counted by hand: a separate python3 script over the fixture's game 1
    // states (every owner "1" card with types, not a token, face down or in a
    // library), plus named reveals, casts and hand exits (none add a card).
    assert_eq!(total, 33);
    assert!(expected >= 21, "the 21 card library look is counted");
    for (key, row) in opponent_rows_of(&found, 1) {
        let basic = key.name.starts_with("Snow-Covered ")
            || ["Plains", "Island", "Swamp", "Mountain", "Forest", "Wastes"]
                .contains(&key.name.as_str());
        if !basic {
            assert!(row.seen <= 4, "{} seen {}", key.name, row.seen);
        }
    }
}

#[test]
fn a_card_only_an_event_named_is_a_row() {
    // Force of Negation: no state lists it, only a named Hand to Stack move.
    let found = full_match();
    let rows = game_rows(&found, 3);
    let (key, card) = row(rows, Side::Opponent, "Force of Negation").expect("Force of Negation");
    assert_eq!(card.seen, 1);
    assert_eq!(key.set_code, "");
}

#[test]
fn own_reveals_are_not_opponent_cards() {
    let found = full_match();
    for game in &found.game_cards {
        for (key, row) in &game.rows {
            if row.side == Side::Opponent {
                assert_ne!(key.name, "Urza's Tower");
                assert_ne!(key.name, "Devourer of Destiny");
            }
        }
    }
    let one = game_cards_of(&found, 1);
    assert!(local_row(one, "Urza's Tower").seen >= 1);
    let two = game_cards_of(&found, 2);
    assert!(local_row(two, "Devourer of Destiny").seen >= 1);
}

#[test]
fn local_rows_have_zone_counters() {
    let found = full_match();
    let one = game_cards_of(&found, 1);
    let map = local_row(one, "Expedition Map");
    assert!(map.has_zone_data);
    assert!(map.hand_seen >= 1);
    assert!(map.battlefield_seen >= 1);
    for game in &found.game_cards {
        for (key, row) in &game.rows {
            if row.side == Side::Opponent {
                let counters = (
                    row.has_zone_data,
                    row.discarded,
                    row.hand_seen,
                    row.graveyard_seen,
                    row.exile_seen,
                    row.battlefield_seen,
                );
                assert_eq!(counters, (false, 0, 0, 0, 0, 0), "{}", key.name);
            }
        }
    }
}

#[test]
fn mulligan_hands_are_not_zone_sightings() {
    let units = all_units();
    let found = folded_match(&units, last_at(&units) + HOUR_MS);
    let mut kept = false;
    let mut after_keep = BTreeSet::new();
    let mut anywhere = BTreeSet::new();
    for kind in game_frames(&units, 3) {
        match kind {
            FrameKind::GameEvent(event)
                if event.kind == "HAND_KEPT" && event.player_index == Some(0) =>
            {
                kept = true;
            }
            FrameKind::GameState(state) if !state.sideboarding => {
                for card in &state.cards {
                    let mine = card.owner_seat == Some(0) && card.zone == Zone::Hand;
                    if mine && card.name == "Trinisphere" {
                        anywhere.insert(card.id);
                        if kept {
                            after_keep.insert(card.id);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    assert!(kept, "game 3 has the user's keep");
    assert!(
        anywhere.len() > after_keep.len(),
        "the sent back Trinisphere shows only before the keep"
    );
    let game = game_cards_of(&found, 3);
    let card = local_row(game, "Trinisphere");
    assert_eq!(card.hand_seen as usize, after_keep.len());
    // Counted by hand with a separate python3 script over the fixture: the
    // sent back Trinisphere (id 54) shows only in states before the kept hand.
    assert_eq!((card.seen, card.hand_seen), (0, 0));
    // The bottomed Karn: ids 20 (bottomed) and 21 are in the kept hand state,
    // id 19 only in the hand sent back.
    let karn = local_row(game, "Karn, the Great Creator");
    assert_eq!((karn.seen, karn.hand_seen), (2, 2));
}

fn event(seq: u64, kind: &str) -> GameEvent {
    GameEvent {
        seq: Some(seq),
        game_id: FORGE_GAME.to_string(),
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

fn event_frame(event: GameEvent) -> Frame {
    Frame {
        at: 1_900_000_000_000 + event.seq.unwrap_or(0) as i64,
        kind: FrameKind::GameEvent(event),
    }
}

fn reveal(seq: u64, cards: &[(u64, &str)], owner_echo: bool) -> Frame {
    let mut reveal = event(seq, "CARD_REVEALED");
    reveal.player_index = Some(0);
    reveal.owner_echo = owner_echo;
    reveal.card_ids = cards.iter().map(|(id, _)| *id).collect();
    reveal.card_names = cards.iter().map(|(_, name)| name.to_string()).collect();
    event_frame(reveal)
}

fn cast(seq: u64, kind: &str, seat: u32, id: u64, name: &str) -> Frame {
    let mut cast = event(seq, kind);
    cast.player_index = Some(seat);
    cast.card_id = Some(id);
    cast.card_name = Some(name.to_string());
    event_frame(cast)
}

fn match_of_frames(frames: Vec<Frame>) -> EndstepMatch {
    let units = vec![unit(UNIT_01), hand_game_unit(frames)];
    folded_match(&units, 1_900_000_000_000)
}

#[test]
fn tokens_and_hidden_names_are_not_sightings() {
    let mut created = event(1, "TOKEN_CREATED");
    created.card_id = Some(9);
    created.card_name = Some("Treasure".to_string());
    let mut token = state_card(30, "State Token", Zone::Battlefield, 1);
    token.is_token = true;
    let mut state = hand_state(2, &[], &[]);
    if let FrameKind::GameState(state) = &mut state.kind {
        state.cards.push(token);
    }
    let frames = vec![
        event_frame(created),
        state,
        reveal(
            3,
            &[(5, "Hidden card"), (6, ""), (7, "Real Opp Card")],
            false,
        ),
        reveal(4, &[(9, "Treasure"), (30, "State Token")], false),
        cast(5, "SPELL_CAST", 1, 9, "Treasure"),
        cast(6, "SPELL_CAST", 1, 30, "State Token"),
        cast(7, "LAND_PLAYED", 1, 31, "Hidden card"),
    ];
    let found = match_of_frames(frames);
    let rows = game_rows(&found, 1);
    let opponent: Vec<&str> = rows
        .iter()
        .filter(|(_, row)| row.side == Side::Opponent)
        .map(|(key, _)| key.name.as_str())
        .collect();
    assert_eq!(opponent, vec!["Real Opp Card"]);
    assert!(row(rows, Side::Local, "Treasure").is_none());
}

#[test]
fn event_sightings_join_the_states_by_card_id() {
    let mut opp_state = hand_state(1, &[], &[]);
    if let FrameKind::GameState(state) = &mut opp_state.kind {
        state
            .cards
            .push(state_card(40, "Opp Card", Zone::Battlefield, 1));
    }
    let frames = vec![
        reveal(2, &[(40, "Opp Card"), (41, "Opp Card")], false),
        opp_state,
        cast(3, "SPELL_CAST", 1, 41, "Opp Card"),
        cast(4, "SPELL_CAST", 0, 50, "My Spell"),
        reveal(5, &[(51, "My Echo")], true),
        reveal(6, &[(50, "My Spell")], true),
    ];
    let found = match_of_frames(frames);
    let rows = game_rows(&found, 1);
    let (_, opp) = row(rows, Side::Opponent, "Opp Card").expect("opponent row");
    assert_eq!(opp.seen, 2);
    assert_eq!(opp.cast, 1);
    let (_, spell) = row(rows, Side::Local, "My Spell").expect("local cast row");
    assert_eq!(spell.seen, 1);
    assert_eq!(spell.cast, 1);
    let (_, echo) = row(rows, Side::Local, "My Echo").expect("local echo row");
    assert_eq!(echo.seen, 1);
    assert!(row(rows, Side::Opponent, "My Echo").is_none());
    assert!(row(rows, Side::Opponent, "My Spell").is_none());
}

#[test]
fn reveals_do_not_depend_on_unit_order() {
    let mut state = hand_state(1, &[], &[]);
    if let FrameKind::GameState(state) = &mut state.kind {
        state
            .cards
            .push(state_card(40, "Opp Card", Zone::Battlefield, 1));
    }
    let first = Unit {
        unit_id: "00000000-0000-4000-8000-0000000000c1".to_string(),
        started_at: 1_900_000_000_000,
        counter: 1,
        frames: vec![
            reveal(2, &[(40, "Opp Card"), (41, "Opp Card")], false),
            reveal(3, &[(51, "My Echo")], true),
        ],
    };
    let second = Unit {
        unit_id: "00000000-0000-4000-8000-0000000000c2".to_string(),
        started_at: 1_900_000_000_100,
        counter: 2,
        frames: vec![state, cast(4, "SPELL_CAST", 1, 42, "Opp Card")],
    };
    let forward = folded_match(
        &[unit(UNIT_01), first.clone(), second.clone()],
        1_900_000_000_000,
    );
    let reversed = folded_match(&[second, first, unit(UNIT_01)], 1_900_000_000_000);
    assert_eq!(forward.game_cards, reversed.game_cards);
    let rows = game_rows(&forward, 1);
    assert_eq!(
        row(rows, Side::Opponent, "Opp Card").map(|found| found.1.seen),
        Some(3)
    );
}

#[test]
fn zone_counters_start_at_the_kept_hand() {
    let frames = vec![
        hand_state(1, &SEVEN, &[]),
        hand_event(2, "HAND_KEPT", 0, Some(7), None),
        hand_event(3, "HAND_SETTLED", 0, Some(7), Some(0)),
        hand_state(4, &SEVEN[1..], &SEVEN[..1]),
    ];
    let found = match_of_frames(frames);
    let game = game_cards_of(&found, 1);
    // The state holding the kept hand counts, so the opening hand is seen.
    let a = local_row(game, "Card A");
    assert_eq!((a.hand_seen, a.battlefield_seen, a.seen), (1, 1, 1));
    let b = local_row(game, "Card B");
    assert_eq!((b.hand_seen, b.battlefield_seen), (1, 0));
    assert!(a.has_zone_data && b.has_zone_data);

    // A keep with no state after it still has the kept hand's state.
    let frames = vec![
        hand_state(1, &SEVEN, &[]),
        hand_event(2, "HAND_KEPT", 0, Some(7), None),
    ];
    let found = match_of_frames(frames);
    let a = local_row(game_cards_of(&found, 1), "Card A");
    assert!(a.has_zone_data);
    assert_eq!((a.hand_seen, a.seen), (1, 1));

    // A keep with no state at all knows nothing.
    let found = match_of_frames(vec![hand_event(1, "HAND_KEPT", 0, Some(7), None)]);
    assert!(
        game_cards_of(&found, 1)
            .rows
            .iter()
            .all(|(_, row)| !row.has_zone_data)
    );

    // No keep at all: every state counts.
    let found = match_of_frames(vec![hand_state(1, &SEVEN, &[])]);
    let a = local_row(game_cards_of(&found, 1), "Card A");
    assert!(a.has_zone_data);
    assert_eq!(a.hand_seen, 1);
}

#[test]
fn a_hand_sent_back_is_neither_seen_nor_a_zone_sighting() {
    let sent_back = [(11, "Card X"), (12, "Card Y")];
    let kept = [(21, "Card Y"), (22, "Card Z")];
    let frames = vec![
        hand_state(1, &sent_back, &[]),
        hand_state(2, &kept, &[]),
        hand_event(3, "HAND_KEPT", 0, Some(2), None),
        hand_state(4, &kept[..1], &kept[1..]),
    ];
    let found = match_of_frames(frames);
    let game = game_cards_of(&found, 1);
    // Card X has no row: nothing but the hand sent back showed it.
    assert!(row(&game.rows, Side::Local, "Card X").is_none());
    // Card Y: id 12 only in the hand sent back, id 21 in the kept hand.
    let y = local_row(game, "Card Y");
    assert_eq!((y.seen, y.hand_seen), (1, 1));
    let z = local_row(game, "Card Z");
    assert_eq!((z.seen, z.hand_seen, z.battlefield_seen), (1, 1, 1));
}

#[test]
fn a_state_owner_survives_the_filters() {
    // Id 70 only shows in the user's library top: no seen, no counters, but
    // its owner is known, so a reveal that names it is the user's card.
    let mut state = hand_state(1, &[], &[]);
    if let FrameKind::GameState(state) = &mut state.kind {
        state
            .cards
            .push(state_card(70, "Top Card", Zone::Library, 0));
    }
    let frames = vec![state, reveal(2, &[(70, "Top Card")], false)];
    let found = match_of_frames(frames);
    let game = game_cards_of(&found, 1);
    assert!(row(&game.rows, Side::Opponent, "Top Card").is_none());
    assert_eq!(local_row(game, "Top Card").seen, 1);
}

#[test]
fn a_state_owner_wins_over_an_event_seat() {
    // Ids 70 and 71 only show in the user's library, then an event of seat 1
    // names each: a cast and a named move out of a hand.
    let mut state = hand_state(1, &[], &[]);
    if let FrameKind::GameState(state) = &mut state.kind {
        state
            .cards
            .push(state_card(70, "Top Card", Zone::Library, 0));
        state
            .cards
            .push(state_card(71, "Next Card", Zone::Library, 0));
    }
    let mut left_hand = event(3, "CARD_ZONE_CHANGE");
    left_hand.card_id = Some(71);
    left_hand.card_name = Some("Next Card".to_string());
    left_hand.from_zone = Some("Hand".to_string());
    left_hand.to_zone = Some("Stack".to_string());
    left_hand.zone_owner = Some(1);
    let frames = vec![
        state,
        cast(2, "SPELL_CAST", 1, 70, "Top Card"),
        event_frame(left_hand),
    ];
    let found = match_of_frames(frames);
    let game = game_cards_of(&found, 1);
    for name in ["Top Card", "Next Card"] {
        assert!(row(&game.rows, Side::Opponent, name).is_none(), "{name}");
        assert_eq!(local_row(game, name).seen, 1, "{name}");
    }
}

#[test]
fn the_next_games_mulligan_does_not_reach_a_finished_game() {
    // Game 2's sideboarding state was lost: its `MULLIGAN` choice and
    // `HAND_KEPT` arrive while game 1, last seen in turn 6, is current.
    let frames = vec![
        hand_state(1, &SEVEN, &[]),
        hand_event(2, "HAND_KEPT", 0, Some(7), None),
        with_turn(hand_state(3, &SEVEN[1..], &SEVEN[..1]), Some(6)),
        hand_event(4, "MULLIGAN", 0, None, None),
        hand_event(5, "HAND_KEPT", 0, Some(6), None),
    ];
    let found = match_of_frames(frames);
    let game = game_cards_of(&found, 1);
    assert!(game.mulligans.is_empty());
    assert_eq!(game.opening_hand_size, Some(7));
}

#[test]
fn the_next_games_keep_does_not_cut_a_finished_game() {
    // Game 1 joined late (no keep of its own); game 2's `HAND_KEPT` arrives
    // after game 1's last state, which no longer holds Card A.
    let frames = vec![
        with_turn(hand_state(1, &SEVEN, &[]), Some(3)),
        with_turn(hand_state(2, &SEVEN[1..], &[]), Some(4)),
        hand_event(3, "HAND_KEPT", 0, Some(7), None),
    ];
    let found = match_of_frames(frames);
    let a = local_row(game_cards_of(&found, 1), "Card A");
    assert!(a.has_zone_data);
    assert_eq!((a.seen, a.hand_seen), (1, 1));
}

#[test]
fn a_discard_counts_from_the_keep_on() {
    let discard = |seq: u64, id: u64, name: &str| {
        let mut change = event(seq, "CARD_ZONE_CHANGE");
        change.card_id = Some(id);
        change.card_name = Some(name.to_string());
        change.from_zone = Some("Hand".to_string());
        change.to_zone = Some("Graveyard".to_string());
        change.zone_owner = Some(0);
        event_frame(change)
    };
    let frames = vec![
        hand_state(1, &SEVEN, &[]),
        discard(2, 1, "Card A"),
        hand_event(3, "HAND_KEPT", 0, Some(7), None),
        discard(4, 2, "Card B"),
    ];
    let found = match_of_frames(frames);
    let game = game_cards_of(&found, 1);
    assert_eq!(local_row(game, "Card A").discarded, 0);
    assert_eq!(local_row(game, "Card B").discarded, 1);

    // No state at all: the keep is not kept (no turn 0 state before it), so
    // every named discard counts. The events name it, no state is needed.
    let frames = vec![
        hand_event(1, "HAND_KEPT", 0, Some(7), None),
        discard(2, 2, "Card B"),
    ];
    let found = match_of_frames(frames);
    let b = local_row(game_cards_of(&found, 1), "Card B");
    assert!(!b.has_zone_data);
    assert_eq!((b.seen, b.discarded), (1, 1));
}

fn discard_log(seq: u64, seat: u32, id: u64, name: &str) -> Frame {
    let mut log = event(seq, "DISCARD_LOG");
    log.player_index = Some(seat);
    log.card_id = Some(id);
    log.card_name = Some(name.to_string());
    event_frame(log)
}

#[test]
fn a_discard_log_is_a_discard() {
    let mut change = event(3, "CARD_ZONE_CHANGE");
    change.card_id = Some(1);
    change.card_name = Some("Card A".to_string());
    change.from_zone = Some("Hand".to_string());
    change.to_zone = Some("Graveyard".to_string());
    change.zone_owner = Some(0);
    let frames = vec![
        hand_state(1, &SEVEN, &[]),
        hand_event(2, "HAND_KEPT", 0, Some(7), None),
        // Forge sends the zone change, then the DISCARD_LOG of the same card.
        event_frame(change),
        discard_log(4, 0, 1, "Card A"),
        // A DISCARD_LOG alone counts too.
        discard_log(5, 0, 2, "Card B"),
        discard_log(6, 0, 70, "Not In Hand"),
        // The opponent's shows the card, but is no local counter.
        discard_log(7, 1, 60, "Opp Discard"),
        // A hidden name is no discard.
        discard_log(8, 0, 71, "Hidden card"),
    ];
    let found = match_of_frames(frames);
    let game = game_cards_of(&found, 1);
    assert_eq!(local_row(game, "Card A").discarded, 1);
    assert_eq!(local_row(game, "Card B").discarded, 1);
    assert_eq!(local_row(game, "Not In Hand").discarded, 1);
    assert_eq!(local_total(game, |row| row.discarded), 3);
    let (_, opponent) = row(&game.rows, Side::Opponent, "Opp Discard").expect("opponent row");
    assert_eq!((opponent.seen, opponent.discarded), (1, 0));
    assert!(row(&game.rows, Side::Local, "Hidden card").is_none());
}

#[test]
fn an_echo_reveal_of_a_card_only_in_a_hand_sent_back_is_seen() {
    let sent_back = [(11, "Card X"), (12, "Card Y")];
    let kept = [(21, "Card Z")];
    let frames = vec![
        hand_state(1, &sent_back, &[]),
        hand_event(2, "MULLIGAN", 0, None, None),
        hand_state(3, &kept, &[]),
        hand_event(4, "HAND_KEPT", 0, Some(1), None),
        hand_state(5, &kept, &[]),
        // Card X went back into the library with the mulligan; a later
        // effect reveals it.
        reveal(6, &[(11, "Card X")], true),
    ];
    let found = match_of_frames(frames);
    let game = game_cards_of(&found, 1);
    assert_eq!(local_row(game, "Card X").seen, 1);
    assert!(row(&game.rows, Side::Opponent, "Card X").is_none());
    // Card Y was only in the hand sent back: still not seen.
    assert_eq!(local_row(game, "Card Y").seen, 0);
}

#[test]
fn the_settled_amount_is_the_starting_hand_size() {
    // The user went to six, but no full state after the settle names the
    // bottomed card: the size still comes from `HAND_SETTLED`.
    let frames = vec![
        hand_state(1, &SEVEN, &[]),
        hand_event(2, "HAND_KEPT", 0, Some(7), None),
        hand_event(3, "HAND_SETTLED", 0, Some(6), Some(1)),
    ];
    let found = match_of_frames(frames);
    let game = game_cards_of(&found, 1);
    assert_eq!(game.opening_hand_size, Some(7));
    assert_eq!(game.starting_hand_size, Some(6));
    // Which card went to the bottom is not known.
    assert_eq!(local_total(game, |row| row.bottomed), 0);

    // A hostile amount above the opening hand is cut to it.
    let frames = vec![
        hand_state(1, &SEVEN, &[]),
        hand_event(2, "HAND_KEPT", 0, Some(7), None),
        hand_event(3, "HAND_SETTLED", 0, Some(40), Some(0)),
    ];
    let found = match_of_frames(frames);
    assert_eq!(game_cards_of(&found, 1).starting_hand_size, Some(7));
}

#[test]
fn mulligan_counts_take_the_hands_and_the_settle() {
    // No `MULLIGAN` with an amount came (the redraw event was lost): the
    // hand sent back and both `HAND_SETTLED` events still count.
    let frames = vec![
        hand_state(1, &SEVEN, &[]),
        hand_event(2, "MULLIGAN", 0, None, None),
        hand_state(3, &SEVEN[..6], &[]),
        hand_event(4, "HAND_KEPT", 0, Some(6), None),
        hand_event(5, "HAND_SETTLED", 0, Some(6), Some(1)),
        hand_event(6, "HAND_SETTLED", 1, Some(5), Some(2)),
    ];
    let found = match_of_frames(frames);
    let game = &found.view.games[0];
    assert_eq!(game.local_mulligans, Some(1));
    assert_eq!(game.opp_mulligans, Some(2));
    assert_eq!(game_cards_of(&found, 1).mulligans.len(), 7);

    // A settle that says fewer than the hands sent back does not lower it.
    let frames = vec![
        hand_state(1, &SEVEN, &[]),
        hand_event(2, "MULLIGAN", 0, None, None),
        hand_state(3, &SEVEN, &[]),
        hand_event(4, "MULLIGAN", 0, None, None),
        hand_state(5, &SEVEN[..5], &[]),
        hand_event(6, "HAND_KEPT", 0, Some(5), None),
        hand_event(7, "HAND_SETTLED", 0, Some(5), Some(0)),
    ];
    let found = match_of_frames(frames);
    assert_eq!(found.view.games[0].local_mulligans, Some(2));
}

#[test]
fn a_named_discard_counts_on_the_local_row() {
    let discard = |seq: u64, seat: u32, id: u64, name: &str| {
        let mut change = event(seq, "CARD_ZONE_CHANGE");
        change.card_id = Some(id);
        change.card_name = Some(name.to_string());
        change.from_zone = Some("Hand".to_string());
        change.to_zone = Some("Graveyard".to_string());
        change.zone_owner = Some(seat);
        event_frame(change)
    };
    let mut anonymous = event(9, "CARD_ZONE_CHANGE");
    anonymous.from_zone = Some("Hand".to_string());
    anonymous.to_zone = Some("Library".to_string());
    anonymous.zone_owner = Some(0);
    anonymous.during_mulligan = true;
    let frames = vec![
        hand_state(1, &SEVEN, &[]),
        discard(2, 0, 3, "Card C"),
        discard(3, 0, 3, "Card C"),
        discard(4, 1, 60, "Opp Discard"),
        event_frame(anonymous),
    ];
    let found = match_of_frames(frames);
    let game = game_cards_of(&found, 1);
    assert_eq!(local_row(game, "Card C").discarded, 1);
    assert_eq!(local_total(game, |row| row.discarded), 1);
    // The opponent's discard shows the card but is no local counter.
    let (_, opponent) = row(&game.rows, Side::Opponent, "Opp Discard").expect("opponent row");
    assert_eq!((opponent.seen, opponent.discarded), (1, 0));
}

/// Unit 04 without game 1's game socket frames: everything before and
/// including game 1's `GAME_OUTCOME`.
fn unit_04_without_game_1() -> Unit {
    let mut unit = unit(UNIT_04);
    let mut outcomes = 0;
    unit.frames.retain(|frame| {
        let is_game_frame = matches!(
            frame.kind,
            FrameKind::GameState(_) | FrameKind::GameDelta(_) | FrameKind::GameEvent(_)
        );
        let is_outcome =
            matches!(&frame.kind, FrameKind::GameEvent(event) if event.kind == "GAME_OUTCOME");
        let in_game_1 = outcomes == 0;
        if is_outcome {
            outcomes += 1;
        }
        !(is_game_frame && in_game_1)
    });
    unit
}

#[test]
fn game_two_still_sides_out_when_game_one_was_lost() {
    let units = vec![unit(UNIT_01), unit_04_without_game_1(), unit(UNIT_05)];
    let found = folded_match(&units, last_at(&units) + HOUR_MS);
    assert!(game_cards_of(&found, 1).rows.is_empty());
    let two = game_cards_of(&found, 2);
    let boulder = local_row(two, "Giant's Boulder");
    assert_eq!(boulder.sided_out, 2);
    assert_eq!(local_total(two, |row| row.sided_in), 2);
    assert_eq!(local_total(two, |row| row.sided_out), 2);
}

/// The match body of unit 01, fetched again `later_ms` after the last frame
/// of the match, in a session unit of its own.
fn refetched_body_unit(units: &[Unit], later_ms: i64) -> Unit {
    let session = unit(UNIT_01);
    let mut body = session
        .frames
        .iter()
        .find(|frame| matches!(frame.kind, FrameKind::MatchBody(_)))
        .expect("a match body")
        .clone();
    body.at = last_at(units) + later_ms;
    Unit {
        unit_id: "00000000-0000-4000-8000-0000000000d1".to_string(),
        started_at: body.at,
        counter: 99,
        frames: vec![body],
    }
}

#[test]
fn a_refetched_match_body_is_not_activity() {
    let units = all_units();
    let now_ms = last_at(&units) + 2 * HOUR_MS;
    let once = folded_match(&units, now_ms);
    let mut refetched = all_units();
    refetched.push(refetched_body_unit(&units, HOUR_MS));
    let found = folded_match(&refetched, now_ms);
    assert_eq!(found.view.last_activity_ms, once.view.last_activity_ms);
    assert_eq!(found, once);

    // An unfinished match whose page only fetched its body again stays
    // as quiet as it was.
    let prefix = vec![unit(UNIT_01), unit(UNIT_02)];
    let quiet_ms = last_at(&prefix) + QUIET_MS;
    let mut with_body = prefix.clone();
    with_body.push(refetched_body_unit(&prefix, MINUTE_MS));
    let without = folded_match(&prefix, quiet_ms).view;
    let view = folded_match(&with_body, quiet_ms).view;
    assert_eq!(view.state, MatchState::Abandoned);
    assert_eq!(view.last_activity_ms, without.last_activity_ms);
}

/// Unit 04 with a second `GAME_OUTCOME` after game 1's, naming the other
/// seat (the user won game 1).
fn unit_04_with_second_outcome(seq: Option<u64>) -> Unit {
    let mut unit = unit(UNIT_04);
    let position = unit
        .frames
        .iter()
        .position(|frame| {
            matches!(&frame.kind, FrameKind::GameEvent(event) if event.kind == "GAME_OUTCOME")
        })
        .expect("game 1's outcome");
    let mut second = unit.frames[position].clone();
    if let FrameKind::GameEvent(event) = &mut second.kind {
        event.seq = seq;
        event.sequence_number = Some(527);
        event.player_index = Some(1);
        event.turn_number = Some(13);
        event.end_reason = Some("timeout".to_string());
    }
    unit.frames.insert(position + 1, second);
    unit
}

#[test]
fn a_second_outcome_does_not_rewrite_a_decided_game() {
    let once = full_match();
    for seq in [None, Some(1)] {
        let units = vec![
            unit(UNIT_01),
            unit(UNIT_02),
            unit(UNIT_03),
            unit_04_with_second_outcome(seq),
            unit(UNIT_05),
        ];
        let found = folded_match(&units, last_at(&units) + HOUR_MS);
        let games = &found.view.games;
        let results: Vec<GameResult> = games.iter().map(|game| game.result).collect();
        assert_eq!(
            results,
            vec![GameResult::Win, GameResult::Loss, GameResult::Loss]
        );
        assert_eq!(games[0].end_reason, EndReason::Concede);
        assert_eq!(games[0].turn_count, once.view.games[0].turn_count);
        assert_eq!(games[0].ended_at_ms, once.view.games[0].ended_at_ms);
        assert_eq!((found.view.wins, found.view.losses), (1, 2));
    }
}

#[test]
fn frames_of_other_kinds_with_one_seq_are_all_read() {
    // A state and an event that share an envelope `seq`, and two events
    // that share it but not their `sequenceNumber`: none is a repeat.
    let mut opp_state = hand_state(1, &[], &[]);
    if let FrameKind::GameState(state) = &mut opp_state.kind {
        state
            .cards
            .push(state_card(40, "Opp Card", Zone::Battlefield, 1));
    }
    let numbered = |number: u64, id: u64, name: &str| {
        let mut frame = cast(2, "SPELL_CAST", 1, id, name);
        if let FrameKind::GameEvent(event) = &mut frame.kind {
            event.sequence_number = Some(number);
        }
        frame
    };
    let frames = vec![
        opp_state,
        cast(1, "SPELL_CAST", 1, 41, "Seq Card"),
        numbered(10, 42, "First Card"),
        numbered(11, 43, "Second Card"),
        // A real repeat: same kind, `seq` and `sequenceNumber`.
        numbered(11, 43, "Second Card"),
    ];
    let found = match_of_frames(frames);
    let rows = game_rows(&found, 1);
    for name in ["Opp Card", "Seq Card", "First Card", "Second Card"] {
        let (_, card) = row(rows, Side::Opponent, name).expect(name);
        assert_eq!(card.seen, 1, "{name}");
    }
    let (_, second) = row(rows, Side::Opponent, "Second Card").expect("second");
    assert_eq!(second.cast, 1, "the repeat counts once");
}

#[test]
fn a_bo1_with_a_hostile_score_counts_its_games() {
    // The body says Bo1, but the states' `winsBySeat` add up to three games.
    let mut session = unit(UNIT_01);
    set_match_bodies(&mut session, |body| body.games_per_match = Some(1));
    let mut again = unit(UNIT_03);
    set_match_bodies(&mut again, |body| body.games_per_match = Some(1));
    let units = vec![session, unit(UNIT_02), again, unit(UNIT_04), unit(UNIT_05)];
    let found = folded_match(&units, last_at(&units) + HOUR_MS);
    assert_eq!(found.view.games.len(), 1);
    assert_eq!(found.view.games[0].result, GameResult::Win);
    assert_eq!((found.view.wins, found.view.losses), (1, 0));
    assert_eq!(found.view.outcome, Outcome::Win);
}

#[test]
fn an_activated_card_seen_nowhere_else_is_a_row() {
    let frames = vec![
        hand_state(1, &[], &[]),
        cast(2, "ABILITY_ACTIVATED", 1, 70, "Opp Land"),
        cast(3, "ABILITY_ACTIVATED", 0, 71, "My Artifact"),
    ];
    let found = match_of_frames(frames);
    let rows = game_rows(&found, 1);
    let (_, opp) = row(rows, Side::Opponent, "Opp Land").expect("opponent row");
    assert_eq!((opp.seen, opp.activated), (1, Some(1)));
    let (_, mine) = row(rows, Side::Local, "My Artifact").expect("local row");
    assert_eq!((mine.seen, mine.activated), (1, Some(1)));
}

#[test]
fn a_double_faced_card_counts_once_under_its_first_name() {
    let state_with = |seq: u64, cards: Vec<StateCard>| {
        let mut frame = hand_state(seq, &[], &[]);
        if let FrameKind::GameState(state) = &mut frame.kind {
            state.cards.extend(cards);
        }
        frame
    };
    let frames = vec![
        // A state card that transforms between two states.
        state_with(1, vec![state_card(80, "Opp Front", Zone::Battlefield, 1)]),
        state_with(2, vec![state_card(80, "Opp Back", Zone::Battlefield, 1)]),
        // An event card that only events name, under both faces.
        reveal(3, &[(81, "Event Front")], false),
        cast(4, "SPELL_CAST", 1, 81, "Event Back"),
        // The user's card, its back face played as a land.
        state_with(5, vec![state_card(82, "My Front", Zone::Hand, 0)]),
        cast(6, "LAND_PLAYED", 0, 82, "My Back"),
        state_with(7, vec![state_card(82, "My Back", Zone::Battlefield, 0)]),
    ];
    let found = match_of_frames(frames);
    let rows = game_rows(&found, 1);
    let (_, front) = row(rows, Side::Opponent, "Opp Front").expect("first name");
    assert_eq!(front.seen, 1);
    assert!(row(rows, Side::Opponent, "Opp Back").is_none());
    let (_, event) = row(rows, Side::Opponent, "Event Front").expect("event card");
    assert_eq!((event.seen, event.cast), (1, 1));
    assert!(row(rows, Side::Opponent, "Event Back").is_none());
    let (_, mine) = row(rows, Side::Local, "My Front").expect("local card");
    assert_eq!((mine.seen, mine.played), (1, Some(1)));
    assert_eq!((mine.hand_seen, mine.battlefield_seen), (1, 1));
    assert!(row(rows, Side::Local, "My Back").is_none());
}
