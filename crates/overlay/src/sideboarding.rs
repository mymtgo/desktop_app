//! Sideboarding detection (overlay spec 7.7, 0.x `DetectSideboarding` and
//! `DecideMatchPhase`).
//!
//! MTGO enters a `*SideboardingState` only once a game has ended, so a
//! transition into one that comes after the newest game's first frame means
//! that game is over and the next has not started. 0.x compared timestamps
//! rebuilt from `HH:MM:SS`; v1 compares log positions (CLAUDE.md: event order
//! is `(file_id, byte_offset)`, never timestamp), which also settles the
//! same-second cases.

use std::sync::LazyLock;

use regex::Regex;

/// A log position: `(log_file_id, byte_offset)`.
pub type Position = (i64, i64);

/// A transition whose destination is a sideboarding state, across the
/// casual, league and tournament prefixes. Exits (`from
/// ...SideboardingState to ...DeckSubmittedState`) do not match.
static ENTERING: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\bto \w*SideboardingState\b").expect("the sideboarding pattern is valid")
});

/// Whether the player is sideboarding: a `match_state_changed` event of the
/// live match entering a sideboarding state lies after the newest game's
/// first frame. `state_events` are the match's state events (position,
/// text); `newest_game_start` is the position of the newest game's first
/// `game_state_update`, `None` before any game (then never sideboarding).
pub fn is_sideboarding(
    state_events: &[(Position, String)],
    newest_game_start: Option<Position>,
) -> bool {
    let Some(start) = newest_game_start else {
        return false;
    };
    state_events
        .iter()
        .any(|(position, text)| *position > start && ENTERING.is_match(text))
}

/// The phase of an active league match (0.x `DecideMatchPhase`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchPhase {
    InGame,
    Sideboarding,
}

/// The phase of an active match, by the same rule as `is_sideboarding`.
pub fn match_phase(
    state_events: &[(Position, String)],
    newest_game_start: Option<Position>,
) -> MatchPhase {
    if is_sideboarding(state_events, newest_game_start) {
        MatchPhase::Sideboarding
    } else {
        MatchPhase::InGame
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ENTER: &str =
        "Match State Changed from MatchJoinedGameStartedState to MatchJoinedSideboardingState";

    fn event(file: i64, offset: i64, text: &str) -> (Position, String) {
        ((file, offset), text.to_string())
    }

    #[test]
    fn is_false_before_any_game_has_ended() {
        let events = [event(
            1,
            100,
            "Match State Changed from MatchJoinedEventUnderwayState to MatchJoinedSideboardingState",
        )];

        assert!(!is_sideboarding(&events, None));
    }

    #[test]
    fn is_true_when_sideboarding_follows_a_finished_game() {
        let events = [event(1, 900, ENTER)];

        assert!(is_sideboarding(&events, Some((1, 200))));
    }

    #[test]
    fn recognises_league_and_tournament_state_prefixes() {
        for text in [
            "Match State Changed to LeagueMatchJoinedSideboardingState",
            "Match State Changed to TournamentMatchJoinedSideboardingState",
        ] {
            let events = [event(1, 900, text)];

            assert!(is_sideboarding(&events, Some((1, 200))), "{text}");
        }
    }

    #[test]
    fn is_false_again_once_the_next_game_has_a_snapshot() {
        let events = [event(
            1,
            900,
            "Match State Changed to MatchJoinedSideboardingState",
        )];

        // Game 2's first frame comes after the transition.
        assert!(!is_sideboarding(&events, Some((1, 1200))));
    }

    #[test]
    fn is_false_when_no_sideboarding_event_exists() {
        assert!(!is_sideboarding(&[], Some((1, 200))));
    }

    // 0.x: the transition in the same second the game ended. By position the
    // transition still follows the game's first frame.
    #[test]
    fn is_true_when_the_sideboarding_transition_lands_in_the_same_second_the_game_ended() {
        let events = [event(
            1,
            900,
            "Match State Changed from LeagueMatchJoinedEventUnderwayState to LeagueMatchJoinedSideboardingState",
        )];

        assert!(is_sideboarding(&events, Some((1, 200))));
    }

    // 0.x: `ended_at` trails the transition. The game's end plays no part in
    // the position rule, only its first frame.
    #[test]
    fn is_true_when_ended_at_trails_the_sideboarding_transition_by_several_seconds() {
        let events = [event(
            1,
            900,
            "Match State Changed from MatchJoinedEventUnderwayState to MatchJoinedSideboardingState",
        )];

        assert!(is_sideboarding(&events, Some((1, 200))));
    }

    #[test]
    fn stays_false_during_a_game_whose_sideboarding_transition_preceded_its_start() {
        let events = [event(
            1,
            900,
            "Match State Changed from MatchJoinedEventUnderwayState to MatchJoinedSideboardingState",
        )];

        assert!(!is_sideboarding(&events, Some((1, 1000))));
    }

    #[test]
    fn ignores_the_exit_transition_when_it_shares_a_second_with_the_next_game_start() {
        let enter = event(
            1,
            900,
            "Match State Changed from LeagueMatchJoinedEventUnderwayState to LeagueMatchJoinedSideboardingState",
        );
        let exit_text = "Match State Changed from LeagueMatchJoinedSideboardingState to LeagueMatchSideboardingDeckSubmittedState";

        // The exit just before and just after game 2's first frame.
        let before = [enter.clone(), event(1, 1190, exit_text)];
        let after = [enter, event(1, 1210, exit_text)];

        assert!(!is_sideboarding(&before, Some((1, 1200))));
        assert!(!is_sideboarding(&after, Some((1, 1200))));
    }

    #[test]
    fn a_same_second_exit_never_reads_as_sideboarding() {
        // Game 1 ends, the player sideboards, submits, and game 2 starts, all
        // logged in one second: only the order of the lines decides.
        let events = [
            event(1, 900, ENTER),
            event(
                1,
                950,
                "Match State Changed from MatchJoinedSideboardingState to MatchSideboardingDeckSubmittedState",
            ),
        ];

        assert!(!is_sideboarding(&events, Some((1, 1000))));
        // Before game 2's first frame the entry still counts.
        assert!(is_sideboarding(&events, Some((1, 200))));
    }

    #[test]
    fn log_position_orders_by_file_before_offset() {
        let events = [event(2, 10, ENTER)];

        assert!(is_sideboarding(&events, Some((1, 5000))));
        assert!(!is_sideboarding(&events, Some((3, 0))));
    }

    #[test]
    fn is_sideboarding_when_the_log_shows_the_sideboarding_state_after_a_game_ended() {
        let events = [event(1, 900, ENTER)];

        assert_eq!(
            match_phase(&events, Some((1, 200))),
            MatchPhase::Sideboarding
        );
    }

    #[test]
    fn is_in_game_when_a_game_has_ended_at_but_no_sideboarding_state_was_logged() {
        assert_eq!(match_phase(&[], Some((1, 200))), MatchPhase::InGame);
    }

    #[test]
    fn is_in_game_before_any_game_has_been_logged() {
        assert_eq!(match_phase(&[], None), MatchPhase::InGame);
    }
}
