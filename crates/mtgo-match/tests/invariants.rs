//! Properties that must hold on every real fixture.

use std::collections::BTreeSet;

use fixture_tools::{fixture_names, load_fixture, prepared_events};
use mtgo_match::{Lookup, MatchView, Memory, Projector};
use mtgo_protocol::PreparedEvent;

fn rebuild(events: &[PreparedEvent], now_ms: i64) -> Vec<MatchView> {
    let mut projector = Projector::new(vec![]);
    for event in events {
        projector.apply(event);
    }
    projector.finalize(now_ms);
    projector.views().into_iter().cloned().collect()
}

#[test]
fn live_equals_rebuild() {
    for name in fixture_names() {
        let fixture = load_fixture(&name);
        let events = prepared_events(&fixture);
        let now_ms = fixture.meta.now_ms();

        let mut live = Projector::new(vec![]);
        for event in &events {
            live.apply(event);
            live.finalize(event.at_ms);
        }
        live.finalize(now_ms);
        let live_views: Vec<MatchView> = live.views().into_iter().cloned().collect();

        assert_eq!(live_views, rebuild(&events, now_ms), "fixture {name}");
    }
}

#[test]
fn processing_everything_twice_is_idempotent() {
    for name in fixture_names() {
        let fixture = load_fixture(&name);
        let events = prepared_events(&fixture);
        let doubled: Vec<PreparedEvent> = events.iter().chain(events.iter()).cloned().collect();
        let now_ms = fixture.meta.now_ms();
        assert_eq!(
            rebuild(&doubled, now_ms),
            rebuild(&events, now_ms),
            "fixture {name}"
        );
    }
}

#[test]
fn truncating_a_log_never_panics_and_leaves_no_complete_match_without_a_signal() {
    for name in fixture_names() {
        let fixture = load_fixture(&name);
        let events = prepared_events(&fixture);
        for cut in [events.len() / 4, events.len() / 2, events.len() * 3 / 4] {
            let mut projector = Projector::new(vec![]);
            for event in &events[..cut] {
                projector.apply(event);
            }
            for view in projector.views() {
                if view.state == mtgo_match::MatchState::Complete {
                    // A quiet completion (issue #26) is the only other way to
                    // complete without a signal.
                    assert!(
                        matches!(
                            view.resolution,
                            Some(mtgo_match::Resolution::Signal | mtgo_match::Resolution::Quiet)
                        ),
                        "fixture {name}: {:?}",
                        view.resolution
                    );
                }
            }
        }
    }
}

#[test]
fn a_signalled_match_never_ends_before_its_last_game() {
    for name in fixture_names() {
        let fixture = load_fixture(&name);
        let events = prepared_events(&fixture);
        for view in rebuild(&events, fixture.meta.now_ms()) {
            if view.resolution != Some(mtgo_match::Resolution::Signal) {
                continue;
            }
            let ended = view.ended_at_ms.expect("a signalled match has an end");
            for game in &view.games {
                assert!(
                    game.ended_at_ms.is_none_or(|game_end| game_end <= ended),
                    "fixture {name}, match {}, game {}",
                    view.token,
                    game.game_number
                );
            }
        }
    }
}

/// What the store's `events_for_match` returns: every stored event naming the
/// token, one of its Match IDs, or one of those matches' Game IDs.
fn history(token: &str, stored: &[PreparedEvent]) -> Vec<PreparedEvent> {
    let match_ids: BTreeSet<u64> = stored
        .iter()
        .filter(|e| e.subjects.match_token.as_deref() == Some(token))
        .filter_map(|e| e.subjects.match_id)
        .collect();
    let game_ids: BTreeSet<u64> = stored
        .iter()
        .filter(|e| {
            e.subjects
                .match_id
                .is_some_and(|id| match_ids.contains(&id))
        })
        .filter_map(|e| e.subjects.game_id)
        .collect();
    stored
        .iter()
        .filter(|e| {
            e.subjects.match_token.as_deref() == Some(token)
                || e.subjects
                    .match_id
                    .is_some_and(|id| match_ids.contains(&id))
                || e.subjects.game_id.is_some_and(|id| game_ids.contains(&id))
        })
        .cloned()
        .collect()
}

fn token_for_match_id(match_id: u64, stored: &[PreparedEvent]) -> Option<String> {
    stored
        .iter()
        .find(|e| e.subjects.match_id == Some(match_id) && e.subjects.match_token.is_some())
        .and_then(|e| e.subjects.match_token.clone())
}

fn match_id_for_game_id(game_id: u64, stored: &[PreparedEvent]) -> Option<u64> {
    stored
        .iter()
        .find(|e| e.subjects.game_id == Some(game_id) && e.subjects.match_id.is_some())
        .and_then(|e| e.subjects.match_id)
}

/// Answer the projector's lookups the way the tracker does (Task 10).
fn answer_lookups(projector: &mut Projector, stored: &[PreparedEvent]) {
    for _ in 0..100 {
        let lookups = projector.take_lookups();
        if lookups.is_empty() {
            return;
        }
        for lookup in lookups {
            let token = match lookup {
                Lookup::Reload(token) => Some(token),
                Lookup::MatchId(id) => token_for_match_id(id, stored),
                Lookup::GameId(id) => match_id_for_game_id(id, stored)
                    .and_then(|match_id| token_for_match_id(match_id, stored)),
            };
            if let Some(token) = token {
                projector.restore(&token, &history(&token, stored));
            }
        }
    }
    panic!("lookups never settled");
}

#[test]
fn retiring_and_reloading_matches_equals_rebuild_on_every_fixture() {
    for name in fixture_names() {
        let fixture = load_fixture(&name);
        let events = prepared_events(&fixture);
        let now_ms = fixture.meta.now_ms();

        let mut live = Projector::new(vec![]);
        for (index, event) in events.iter().enumerate() {
            live.apply(event);
            live.finalize(event.at_ms);
            live.retire_quiet(event.at_ms);
            answer_lookups(&mut live, &events[..=index]);
        }
        live.finalize(now_ms);
        live.retire_quiet(now_ms);
        answer_lookups(&mut live, &events);

        let live_views: Vec<MatchView> = live.views().into_iter().cloned().collect();
        assert_eq!(live_views, rebuild(&events, now_ms), "fixture {name}");
        assert_eq!(
            live.memory(),
            Memory {
                matches: 0,
                held_events: 0
            },
            "fixture {name}: everything finished, so nothing stays in memory"
        );
    }
}
