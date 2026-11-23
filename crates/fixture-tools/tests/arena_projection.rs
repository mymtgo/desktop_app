//! The Arena projector over the Arena fixture: what each match becomes, the
//! deck it used, and live equals rebuild.

use arena_match::{ArenaChanges, ArenaInput, ArenaProjector, RankPoint};
use arena_protocol::{ArenaEvent, HeaderClock, decode_all, header_time, split_complete};
use fixture_tools::arena_fixture;
use mtgo_match::{GameResult, MatchView, Outcome};
use serde_json::json;

const FIXTURE: &str = "bo3-loss-and-match-concede";
const BO1_FIXTURE: &str = "bo1-wins-and-concede";
const SEALED_FIXTURE: &str = "sealed-direct-fast-login";
const MATCH_ONE: &str = "bf62b620-6924-4a6b-901a-275c36778f67";
const MATCH_TWO: &str = "85d265dc-f113-429f-a8c4-89e89c258b66";

/// Every event of the Bo3 fixture as the tracker feeds it.
fn inputs() -> Vec<ArenaInput> {
    fixture_inputs(FIXTURE)
}

/// Every event of a fixture as the tracker feeds it (file id 1): each entry
/// takes its header time, or the last one before it. Nothing is added: a
/// fast-login session (the Bo1 one) has no login line and finds its account
/// from the `authenticateResponse` screen name.
fn fixture_inputs(name: &str) -> Vec<ArenaInput> {
    let fixture = arena_fixture(name);
    let clock = HeaderClock {
        day_first: Some(fixture.meta.day_first),
        utc_offset_minutes: fixture.meta.utc_offset_minutes,
    };
    let mut at_ms = 0;
    let mut inputs = Vec::new();
    for entry in split_complete(&fixture.bytes) {
        // The order is known, so the reference never decides.
        if let Some(time) = header_time(&clock, &entry.head, at_ms) {
            at_ms = time;
        }
        for (seq, event) in decode_all(&entry).into_iter().enumerate() {
            inputs.push(ArenaInput {
                file_id: 1,
                offset: entry.offset,
                seq: seq as u32,
                at_ms,
                event,
            });
        }
    }
    inputs
}

fn fold(inputs: &[ArenaInput]) -> (ArenaProjector, Vec<ArenaChanges>) {
    let mut projector = ArenaProjector::new();
    let changes = inputs.iter().map(|input| projector.apply(input)).collect();
    (projector, changes)
}

fn view(projector: &ArenaProjector, token: &str) -> MatchView {
    projector.view(token).expect("the match has a view").clone()
}

fn rank_points(changes: &[ArenaChanges]) -> Vec<RankPoint> {
    changes.iter().flat_map(|c| c.rank.clone()).collect()
}

/// Tokens of the matches that got a view, in the order they first changed.
fn tokens_in_order(changes: &[ArenaChanges]) -> Vec<String> {
    let mut tokens: Vec<String> = Vec::new();
    for token in changes.iter().flat_map(|c| c.matches.iter()) {
        if !tokens.contains(token) {
            tokens.push(token.clone());
        }
    }
    tokens
}

#[test]
fn fixture_match_one_is_a_one_two_loss() {
    let (projector, _) = fold(&inputs());
    let view = view(&projector, MATCH_ONE);
    assert_eq!(view.format.as_deref(), Some("CPIONEER"));
    assert_eq!(view.match_type.as_deref(), Some("ranked"));
    assert_eq!((view.wins, view.losses), (1, 2));
    assert_eq!(view.outcome, Outcome::Loss);
    assert_eq!(view.local_username, "Player#00001");
    let results: Vec<GameResult> = view.games.iter().map(|game| game.result).collect();
    assert_eq!(
        results,
        vec![GameResult::Loss, GameResult::Win, GameResult::Loss]
    );
}

#[test]
fn seat_two_local_player_gets_the_right_outcome() {
    let (projector, _) = fold(&inputs());
    let view = view(&projector, MATCH_TWO);
    assert_eq!(view.outcome, Outcome::Loss);
    assert_eq!((view.wins, view.losses), (0, 1));
    assert_eq!(view.games.len(), 1);
    assert_eq!(view.games[0].result, GameResult::Loss);
}

#[test]
fn fixture_deck_use_is_the_set_deck() {
    let inputs = inputs();
    let (_, changes) = fold(&inputs);
    let set_deck_id = inputs
        .iter()
        .find_map(|input| match &input.event {
            ArenaEvent::DeckSet { deck_id, .. } => Some(deck_id.clone()),
            _ => None,
        })
        .expect("the fixture sets a deck");
    let deck_use = changes
        .iter()
        .flat_map(|c| c.deck_uses.iter())
        .find(|deck| deck.match_token == MATCH_ONE)
        .expect("match 1 has a deck use");
    assert_eq!(deck_use.deck_id, set_deck_id);
    assert_eq!(deck_use.format_code, "CPIONEER");
    // The fixture's match 1 deck is a 61 card list (its game 1 deck message
    // holds 61 cards as well).
    let main_cards: u32 = deck_use.main.iter().map(|(_, copies)| copies).sum();
    assert_eq!(main_cards, 61);
}

#[test]
fn live_equals_rebuild_for_arena() {
    live_equals_rebuild(FIXTURE);
}

#[test]
fn live_equals_rebuild_for_arena_bo1() {
    live_equals_rebuild(BO1_FIXTURE);
}

fn live_equals_rebuild(name: &str) {
    let inputs = fixture_inputs(name);

    // Live: one event at a time, reading every changed view as it changes.
    let mut live = ArenaProjector::new();
    let mut live_changes = Vec::new();
    for input in &inputs {
        let changes = live.apply(input);
        for token in &changes.matches {
            assert!(live.view(token).is_some(), "a changed match has a view");
        }
        live_changes.push(changes);
    }

    // Rebuild: every event read back from its stored JSON, folded at once.
    let stored: Vec<ArenaInput> = inputs
        .iter()
        .map(|input| {
            let raw = serde_json::to_string(&input.event).expect("serialise");
            ArenaInput {
                event: serde_json::from_str(&raw).expect("deserialise"),
                ..input.clone()
            }
        })
        .collect();
    let (rebuilt, rebuilt_changes) = fold(&stored);

    assert_eq!(live_changes, rebuilt_changes);
    let tokens = tokens_in_order(&rebuilt_changes);
    assert!(!tokens.is_empty(), "{name} has matches");
    for token in &tokens {
        assert_eq!(live.view(token), rebuilt.view(token));
    }
    assert_eq!(live.account(), rebuilt.account());
}

#[test]
fn arena_projection_snapshot() {
    let (projector, changes) = fold(&inputs());
    let tokens = [MATCH_ONE.to_string(), MATCH_TWO.to_string()];
    let summary = projection_summary(&projector, &changes, &tokens);
    insta::assert_json_snapshot!("arena_projection_bo3-loss-and-match-concede", summary);
}

#[test]
fn bo1_fixture_has_four_bo1_matches() {
    let inputs = fixture_inputs(BO1_FIXTURE);
    let has_login = inputs
        .iter()
        .any(|input| matches!(input.event, ArenaEvent::Login { .. }));
    assert!(!has_login, "a fast-login session writes no login line");
    let (projector, changes) = fold(&inputs);
    let tokens = tokens_in_order(&changes);
    let views: Vec<MatchView> = tokens.iter().map(|t| view(&projector, t)).collect();
    let summary: Vec<(&str, Outcome, &str, usize)> = views
        .iter()
        .map(|view| {
            (
                view.game_structure.as_deref().unwrap_or(""),
                view.outcome,
                view.match_type.as_deref().unwrap_or(""),
                view.games.len(),
            )
        })
        .collect();
    assert_eq!(
        summary,
        vec![
            ("bo1", Outcome::Win, "unranked", 1),
            ("bo1", Outcome::Win, "unranked", 1),
            ("bo1", Outcome::Loss, "unranked", 1),
            ("bo1", Outcome::Win, "ranked", 1),
        ]
    );
    let summary = projection_summary(&projector, &changes, &tokens);
    insta::assert_json_snapshot!("arena_projection_bo1-wins-and-concede", summary);
}

#[test]
fn sealed_direct_fixture_is_a_limited_run_of_an_untagged_account() {
    let inputs = fixture_inputs(SEALED_FIXTURE);
    let (projector, changes) = fold(&inputs);
    let account = projector
        .account()
        .expect("the untagged fast login finds the account");
    assert_eq!(account.username, "arena:PLAYER0000000000000000000A");
    assert_eq!(account.display_name.as_deref(), Some("Player"));
    let tokens = tokens_in_order(&changes);
    let views: Vec<MatchView> = tokens.iter().map(|t| view(&projector, t)).collect();
    let summary: Vec<(&str, &str, &str, Outcome)> = views
        .iter()
        .map(|view| {
            (
                view.format.as_deref().unwrap_or(""),
                view.match_type.as_deref().unwrap_or(""),
                view.game_structure.as_deref().unwrap_or(""),
                view.outcome,
            )
        })
        .collect();
    let won = ("S6EOE", "limited", "bo1", Outcome::Win);
    let playing = ("S6EOE", "limited", "bo1", Outcome::Unknown);
    assert_eq!(summary, vec![won, won, won, won, won, playing]);
    assert!(
        changes.iter().all(|c| c.deck_uses.is_empty()),
        "a sealed deck is never a constructed deck"
    );
    let summary = projection_summary(&projector, &changes, &tokens);
    insta::assert_json_snapshot!("arena_projection_sealed-direct-fast-login", summary);
}

/// The account, views of `tokens`, opponents, deck uses and rank points.
fn projection_summary(
    projector: &ArenaProjector,
    changes: &[ArenaChanges],
    tokens: &[String],
) -> serde_json::Value {
    let views: Vec<MatchView> = tokens.iter().map(|token| view(projector, token)).collect();
    let opponents: Vec<(String, String)> =
        changes.iter().flat_map(|c| c.opponents.clone()).collect();
    let deck_uses: Vec<serde_json::Value> = changes
        .iter()
        .flat_map(|c| c.deck_uses.iter())
        .map(|deck| {
            json!({
                "match_token": deck.match_token,
                "deck_id": deck.deck_id,
                "deck_name": deck.deck_name,
                "format_code": deck.format_code,
                "main_lines": deck.main.len(),
                "main_cards": deck.main.iter().map(|(_, n)| n).sum::<u32>(),
                "side_cards": deck.side.iter().map(|(_, n)| n).sum::<u32>(),
            })
        })
        .collect();
    json!({
        "account": projector.account(),
        "views": views,
        "opponents": opponents,
        "deck_uses": deck_uses,
        "rank": rank_points(changes),
    })
}
