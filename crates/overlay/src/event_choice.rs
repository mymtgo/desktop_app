//! Which event the card shows, and which one the strip below it shows
//! (event overlay spec 6). Pure: the same candidates give the same choice
//! whether their facts arrived one by one or all at once.
//!
//! The rules, in order:
//! 1. An event with a live match beats one without.
//! 2. Two live matches: a tournament beats a league.
//! 3. and 4. Otherwise the most recent real transition wins (a finished
//!    event's finish is one, so it shows its result for the finished time
//!    unless another event has a live match: rule 5).
//!
//! Ties go to the lower key. The strip (rule 6) shows the next event in the
//! same order that has a live match, is a tournament still running, or
//! finished within the finished time; a league run between matches never
//! shows there.

/// An event that can be on the card. Leagues sort before tournaments, then
/// by id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum EventKey {
    /// A `leagues` row id.
    League(i64),
    /// A tournament's MTGO event id.
    Tournament(i64),
}

impl EventKey {
    fn is_tournament(self) -> bool {
        matches!(self, EventKey::Tournament(_))
    }
}

/// What the choice reads of one event. Only real transitions move
/// `last_transition_ms` (spec 6): matches-left counts never do, so they
/// never flip the card.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Candidate {
    pub key: EventKey,
    /// The account has a live match in it.
    pub live: bool,
    /// Its result is final (a finished event still inside its finished
    /// time; one past it is not a candidate).
    pub finished: bool,
    /// Its latest real transition, log time in UTC ms.
    pub last_transition_ms: i64,
}

/// The card's event and the strip's.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Choice {
    pub shown: Option<EventKey>,
    pub other: Option<EventKey>,
}

/// The event on the card and the one on the strip.
pub fn choose_event(candidates: &[Candidate]) -> Choice {
    let mut ordered: Vec<&Candidate> = candidates.iter().collect();
    ordered.sort_by(|a, b| {
        b.live
            .cmp(&a.live)
            .then_with(|| live_tournament_first(a, b))
            .then_with(|| b.last_transition_ms.cmp(&a.last_transition_ms))
            .then_with(|| a.key.cmp(&b.key))
    });
    let shown = ordered.first().map(|candidate| candidate.key);
    let other = ordered
        .iter()
        .skip(1)
        .find(|candidate| on_strip(candidate))
        .map(|candidate| candidate.key);
    Choice { shown, other }
}

/// Rule 2: of two live events, the tournament comes first.
fn live_tournament_first(a: &Candidate, b: &Candidate) -> std::cmp::Ordering {
    if a.live && b.live {
        b.key.is_tournament().cmp(&a.key.is_tournament())
    } else {
        std::cmp::Ordering::Equal
    }
}

/// Rule 6: whether an event that is not on the card shows on the strip.
fn on_strip(candidate: &Candidate) -> bool {
    candidate.live || candidate.finished || candidate.key.is_tournament()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn league(id: i64, live: bool, at: i64) -> Candidate {
        Candidate {
            key: EventKey::League(id),
            live,
            finished: false,
            last_transition_ms: at,
        }
    }

    fn tournament(id: i64, live: bool, at: i64) -> Candidate {
        Candidate {
            key: EventKey::Tournament(id),
            live,
            finished: false,
            last_transition_ms: at,
        }
    }

    fn finished(mut candidate: Candidate) -> Candidate {
        candidate.finished = true;
        candidate
    }

    #[test]
    fn nothing_to_choose_shows_nothing() {
        assert_eq!(choose_event(&[]), Choice::default());
    }

    #[test]
    fn live_beats_waiting() {
        let choice = choose_event(&[tournament(9, false, 500), league(1, true, 100)]);
        assert_eq!(choice.shown, Some(EventKey::League(1)));
        assert_eq!(
            choice.other,
            Some(EventKey::Tournament(9)),
            "a running tournament goes to the strip"
        );
    }

    #[test]
    fn a_tournament_beats_a_league_when_both_are_live() {
        let choice = choose_event(&[league(1, true, 900), tournament(9, true, 100)]);
        assert_eq!(choice.shown, Some(EventKey::Tournament(9)));
        assert_eq!(
            choice.other,
            Some(EventKey::League(1)),
            "a live league shows on the strip"
        );
    }

    #[test]
    fn of_two_tournaments_the_newest_transition_wins() {
        let both_waiting = choose_event(&[tournament(9, false, 100), tournament(8, false, 200)]);
        assert_eq!(both_waiting.shown, Some(EventKey::Tournament(8)));
        assert_eq!(both_waiting.other, Some(EventKey::Tournament(9)));
        let both_live = choose_event(&[tournament(9, true, 300), tournament(8, true, 200)]);
        assert_eq!(both_live.shown, Some(EventKey::Tournament(9)));
    }

    #[test]
    fn with_nothing_live_the_newest_transition_wins() {
        let choice = choose_event(&[league(1, false, 100), tournament(9, false, 200)]);
        assert_eq!(
            choice.shown,
            Some(EventKey::Tournament(9)),
            "a tournament waiting for its next round beats a quiet league"
        );
        assert_eq!(
            choice.other, None,
            "a league between matches never shows on the strip"
        );
    }

    #[test]
    fn a_finished_event_keeps_the_card_unless_another_is_live() {
        let quiet = choose_event(&[finished(tournament(9, false, 500)), league(1, false, 100)]);
        assert_eq!(quiet.shown, Some(EventKey::Tournament(9)));
        let live = choose_event(&[finished(tournament(9, false, 500)), league(1, true, 100)]);
        assert_eq!(live.shown, Some(EventKey::League(1)));
        assert_eq!(
            live.other,
            Some(EventKey::Tournament(9)),
            "the finished result goes to the strip"
        );
        let finished_league =
            choose_event(&[tournament(9, true, 100), finished(league(1, false, 500))]);
        assert_eq!(finished_league.other, Some(EventKey::League(1)));
    }

    #[test]
    fn the_strip_skips_a_waiting_league_for_the_next_event() {
        let choice = choose_event(&[
            tournament(9, true, 100),
            league(1, false, 900),
            tournament(8, false, 50),
        ]);
        assert_eq!(choice.shown, Some(EventKey::Tournament(9)));
        assert_eq!(choice.other, Some(EventKey::Tournament(8)));
    }

    #[test]
    fn ties_go_to_the_lower_key() {
        let choice = choose_event(&[tournament(9, false, 100), tournament(8, false, 100)]);
        assert_eq!(choice.shown, Some(EventKey::Tournament(8)));
        let mixed = choose_event(&[tournament(1, false, 100), league(5, false, 100)]);
        assert_eq!(
            mixed.shown,
            Some(EventKey::League(5)),
            "leagues sort before tournaments"
        );
    }

    #[test]
    fn the_order_of_the_candidates_does_not_matter() {
        let a = [
            league(1, false, 300),
            tournament(9, true, 100),
            tournament(8, false, 200),
        ];
        let mut b = a;
        b.reverse();
        assert_eq!(choose_event(&a), choose_event(&b));
    }
}
