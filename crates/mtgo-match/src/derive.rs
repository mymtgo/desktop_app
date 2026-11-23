//! The projection rule table (spec section 5.5). `derive` is a pure function of a
//! match's facts, the known local accounts, and `now`, so live updates and rebuilds
//! always agree.

use std::sync::LazyLock;

use mtgo_protocol::GameEntry;
use regex::Regex;

use crate::facts::{EventKey, JoinFact, MatchFacts};
use crate::games::{
    build_games, other_player, outcome_from, score, threshold_outcome, trailing_forfeit,
    trailing_leaver,
};
use crate::model::{GameView, MatchState, MatchView, Outcome, Resolution};

/// Quiet time after which a match without an end signal is resolved post-mortem.
pub const ABANDON_AFTER_MS: i64 = 60 * 60 * 1000;

const COMPLETION_STATES: [&str; 3] = [
    "MatchJoinedCompletedState",
    "MatchClosedState",
    "MatchCompletedState",
];
const END_STATES: [&str; 5] = [
    "TournamentMatchClosedState",
    "MatchCompletedState",
    "MatchEndedState",
    "MatchClosedState",
    "JoinedCompletedState",
];

static TOURNAMENT_ROUND: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"Tournament:(\d+)\s+Round:(\d+)").expect("valid regex"));

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Derived {
    /// No join yet, or joined but no game state yet (`pending`). Never shown.
    Hidden,
    /// Visible otherwise, but the local player cannot be identified yet.
    AwaitingIdentity,
    /// Not the local player's match (or fewer than two players). Discarded.
    Phantom,
    /// Projection panicked or errored for this match.
    Failed(String),
    Visible(Box<MatchView>),
}

pub fn is_completion(to: &str) -> bool {
    COMPLETION_STATES.iter().any(|state| to.contains(state))
}

pub fn is_end(to: &str) -> bool {
    END_STATES.iter().any(|state| to.contains(state))
}

/// The local player's concede-and-leave: `...ConcedeReqState` to `...NotJoined...`.
pub fn is_local_concede(from: &str, to: &str) -> bool {
    from.ends_with("ConcedeReqState") && to.contains("NotJoined")
}

pub fn derive(facts: &MatchFacts, known_accounts: &[String], now_ms: Option<i64>) -> Derived {
    let Some(join) = facts.preferred_join() else {
        return Derived::Hidden;
    };
    let Some((_, players)) = facts.first_players() else {
        return Derived::Hidden;
    };
    let players = players.clone();
    let Some(local) = resolve_local(facts, &players, known_accounts) else {
        return Derived::AwaitingIdentity;
    };
    if players.len() < 2 || !players.contains(&local) {
        return Derived::Phantom;
    }

    let Projected {
        games,
        wins,
        losses,
        last_game_event_ms,
    } = project_games(facts, &local, &players);
    let last_activity_ms = facts.last_activity_ms();

    let signal_key = facts
        .state_changes
        .iter()
        .filter(|change| is_completion(&change.to))
        .map(|change| change.key)
        .min();
    let ended = facts
        .state_changes
        .iter()
        .any(|c| is_end(&c.to) || is_local_concede(&c.from, &c.to));

    let disconnect = settled_disconnect(facts, &players, now_ms);
    // Issue #26: a match whose end or result is known does not wait forever for
    // a completion signal MTGO never logged. Both rules settle on the facts
    // before the gap.
    let quiet = earliest_quiet(
        settled_quiet(facts, &local, &players, now_ms),
        settled_decided(facts, &local, &players, now_ms),
    );

    let (state, resolution, ended_at_ms, outcome) =
        match first_settlement(disconnect, signal_key, quiet) {
            Some(Settlement::Disconnect { survivor, at_ms }) => {
                let outcome = if survivor == local {
                    Outcome::Win
                } else {
                    Outcome::Loss
                };
                (
                    MatchState::Complete,
                    Some(Resolution::Disconnect),
                    Some(at_ms),
                    outcome,
                )
            }
            Some(Settlement::Signal) => (
                MatchState::Complete,
                Some(Resolution::Signal),
                Some(last_game_event_ms),
                signal_outcome(facts, signal_key, &local, &players, wins, losses),
            ),
            Some(Settlement::Quiet { outcome, at_ms }) => (
                MatchState::Complete,
                Some(Resolution::Quiet),
                Some(at_ms),
                outcome,
            ),
            None if ended => (
                MatchState::Ended,
                Some(Resolution::Signal),
                Some(last_game_event_ms),
                Outcome::Unknown,
            ),
            None if is_quiet(now_ms, last_activity_ms) => (
                MatchState::Abandoned,
                Some(Resolution::Abandoned),
                Some(last_activity_ms),
                Outcome::Unknown,
            ),
            None => (MatchState::InProgress, None, None, Outcome::Unknown),
        };

    let tentative_outcome = if state == MatchState::InProgress {
        threshold_outcome(wins, losses)
    } else {
        None
    };
    let (tournament_event_id, tournament_round) = tournament_from(join);

    Derived::Visible(Box::new(MatchView {
        token: facts.token.clone(),
        mtgo_match_id: facts.match_id,
        opponent: other_player(&players, &local),
        local_username: local,
        format: join.kv.get("PlayFormatCd").cloned(),
        match_type: join.kv.get("GameStructureCd").cloned(),
        tournament_event_id,
        tournament_round,
        league_token: kv_text(join, "League Token"),
        game_structure: kv_text(join, "GameStructureCd"),
        state,
        resolution,
        outcome,
        tentative_outcome,
        wins,
        losses,
        started_at_ms: join.at_ms,
        ended_at_ms,
        last_activity_ms,
        games,
    }))
}

fn is_quiet(now_ms: Option<i64>, last_activity_ms: i64) -> bool {
    match now_ms {
        Some(now) => now - last_activity_ms >= ABANDON_AFTER_MS,
        None => false,
    }
}

/// The local player `derive` picks: `None` until a game state names the
/// players, or while the local player cannot be identified.
pub fn local_player(facts: &MatchFacts, known_accounts: &[String]) -> Option<String> {
    let (_, players) = facts.first_players()?;
    resolve_local(facts, players, known_accounts)
}

/// Local identity in priority order: the log file's Login name, the deck-used
/// line, then a single known account among the players.
fn resolve_local(
    facts: &MatchFacts,
    players: &[String],
    known_accounts: &[String],
) -> Option<String> {
    if let Some((_, name)) = &facts.file_username {
        return Some(name.clone());
    }
    if let Some((_, name)) = &facts.deck_used_username {
        return Some(name.clone());
    }
    let mut known = players
        .iter()
        .filter(|player| known_accounts.contains(player));
    let first = known.next()?;
    if known.next().is_some() {
        return None;
    }
    Some(first.clone())
}

/// Every fact in event order, with its gameplay entry when it has one.
fn timeline(facts: &MatchFacts) -> Vec<(EventKey, i64, Option<&GameEntry>)> {
    let mut timeline: Vec<(EventKey, i64, Option<&GameEntry>)> = Vec::new();
    for join in facts.gm_join.iter().chain(facts.state_join.iter()) {
        timeline.push((join.key, join.at_ms, None));
    }
    for change in &facts.state_changes {
        timeline.push((change.key, change.at_ms, None));
    }
    for fact in &facts.entries {
        timeline.push((fact.key, fact.at_ms, Some(&fact.entry)));
    }
    for state in &facts.game_states {
        timeline.push((state.key, state.at_ms, None));
    }
    timeline.sort_by_key(|(key, _, _)| *key);
    timeline
}

/// Every fact that the match then sat quiet after for `ABANDON_AFTER_MS`, as
/// the key of the last fact before the quiet: a gap that long before the next
/// fact in event order, or (when `now` is that long past the last activity)
/// the last fact itself. A gap between facts stays in the facts, so later
/// facts never remove a quiet point.
fn quiet_points(facts: &MatchFacts, now_ms: Option<i64>) -> Vec<EventKey> {
    let timeline = timeline(facts);
    let mut points: Vec<EventKey> = Vec::new();
    for pair in timeline.windows(2) {
        if pair[1].1 - pair[0].1 >= ABANDON_AFTER_MS {
            points.push(pair[0].0);
        }
    }
    if let Some((key, _, _)) = timeline.last()
        && is_quiet(now_ms, facts.last_activity_ms())
    {
        points.push(*key);
    }
    points
}

/// The key of the first end signal or local concede, in event order.
fn first_end_key(facts: &MatchFacts) -> Option<EventKey> {
    facts
        .state_changes
        .iter()
        .filter(|change| is_end(&change.to) || is_local_concede(&change.from, &change.to))
        .map(|change| change.key)
        .min()
}

/// The games, score and last game event time of a set of facts.
struct Projected {
    games: Vec<GameView>,
    wins: u32,
    losses: u32,
    last_game_event_ms: i64,
}

fn project_games(facts: &MatchFacts, local: &str, players: &[String]) -> Projected {
    let mut games = build_games(facts, local, players);
    if let Some(forfeit) = trailing_forfeit(facts, &games, local, players) {
        games.push(forfeit);
    }
    let (wins, losses) = score(facts, &games, local);
    let last_activity_ms = facts.last_activity_ms();
    let last_entry_ms = facts
        .entries
        .iter()
        .map(|e| e.at_ms)
        .max()
        .or_else(|| facts.game_states.iter().map(|g| g.at_ms).max());
    // A match never ends before one of its games.
    let last_game_end_ms = games.iter().filter_map(|g| g.ended_at_ms).max();
    let last_game_event_ms = last_entry_ms
        .max(last_game_end_ms)
        .unwrap_or(last_activity_ms);
    Projected {
        games,
        wins,
        losses,
        last_game_event_ms,
    }
}

/// The match settled by silence, from the facts before the gap: an ended match
/// that went quiet, or a score at the win threshold that went quiet. Returns
/// the quiet point, the outcome and when the match ended.
fn settled_quiet(
    facts: &MatchFacts,
    local: &str,
    players: &[String],
    now_ms: Option<i64>,
) -> Option<(EventKey, Outcome, i64)> {
    let points = quiet_points(facts, now_ms);
    if let Some(end_key) = first_end_key(facts)
        && let Some(point) = points.iter().find(|point| **point >= end_key)
    {
        let prefix = facts.up_to(*point);
        let projected = project_games(&prefix, local, players);
        let outcome = quiet_end_outcome(&prefix, local, players, projected.wins, projected.losses);
        return Some((*point, outcome, projected.last_game_event_ms));
    }
    None
}

/// When both quiet rows apply, the one whose quiet point comes first wins
/// (the ended row on a tie), so a late end signal never undoes an earlier gap.
/// Returns the quiet point, the outcome and when the match ended.
fn earliest_quiet(
    ended: Option<(EventKey, Outcome, i64)>,
    decided: Option<(EventKey, Outcome, i64)>,
) -> Option<(EventKey, Outcome, i64)> {
    match (ended, decided) {
        (Some(end), Some(dec)) if dec.0 < end.0 => Some(dec),
        (Some(end), _) => Some(end),
        (None, dec) => dec,
    }
}

/// How a match completed, from the rule that settled it first.
enum Settlement {
    Disconnect { survivor: String, at_ms: i64 },
    Signal,
    Quiet { outcome: Outcome, at_ms: i64 },
}

/// The rule that settled the match first, in event order (spec 5.5, owner
/// decision 2026-10-07): once complete, a match never changes because of a
/// later fact. On a tie the order is signal, disconnect, quiet: a signal
/// settles at once, while a disconnect needs an hour of quiet after it.
fn first_settlement(
    disconnect: Option<(String, i64, EventKey)>,
    signal: Option<EventKey>,
    quiet: Option<(EventKey, Outcome, i64)>,
) -> Option<Settlement> {
    let mut candidates: Vec<(EventKey, Settlement)> = Vec::new();
    if let Some(key) = signal {
        candidates.push((key, Settlement::Signal));
    }
    if let Some((survivor, at_ms, key)) = disconnect {
        candidates.push((key, Settlement::Disconnect { survivor, at_ms }));
    }
    if let Some((key, outcome, at_ms)) = quiet {
        candidates.push((key, Settlement::Quiet { outcome, at_ms }));
    }
    // `min_by_key` keeps the first of equal keys: the order above
    // (signal, disconnect, quiet).
    candidates
        .into_iter()
        .min_by_key(|(key, _)| *key)
        .map(|(_, settlement)| settlement)
}

/// Like `settled_quiet` for a match with no end signal: the first quiet point
/// whose facts already have a score at the win threshold.
fn settled_decided(
    facts: &MatchFacts,
    local: &str,
    players: &[String],
    now_ms: Option<i64>,
) -> Option<(EventKey, Outcome, i64)> {
    for point in quiet_points(facts, now_ms) {
        let prefix = facts.up_to(point);
        let projected = project_games(&prefix, local, players);
        if let Some(outcome) = threshold_outcome(projected.wins, projected.losses) {
            return Some((point, outcome, projected.last_game_event_ms));
        }
    }
    None
}

/// The outcome of a match completed by a signal, from the facts up to it.
fn signal_outcome(
    facts: &MatchFacts,
    signal_key: Option<EventKey>,
    local: &str,
    players: &[String],
    wins: u32,
    losses: u32,
) -> Outcome {
    match (outcome_from(wins, losses), signal_key) {
        (Outcome::Draw, Some(key)) => level_score_outcome(&facts.up_to(key), local, players),
        (outcome, _) => outcome,
    }
}

/// The outcome of an ended match that went quiet: the score, or a loss when the
/// score decides nothing and the local player conceded.
fn quiet_end_outcome(
    facts: &MatchFacts,
    local: &str,
    players: &[String],
    wins: u32,
    losses: u32,
) -> Outcome {
    match outcome_from(wins, losses) {
        Outcome::Unknown if local_conceded(facts) => Outcome::Loss,
        Outcome::Draw => level_score_outcome(facts, local, players),
        outcome => outcome,
    }
}

/// A level score is a draw only when nobody left. MTGO's game history has no
/// two-game 1-1 match without a winner: a player who drops after the last game
/// result loses the match, though the live log only says "Match Tied".
fn level_score_outcome(facts: &MatchFacts, local: &str, players: &[String]) -> Outcome {
    if let Some(leaver) = trailing_leaver(facts) {
        return if other_player(players, &leaver).as_deref() == Some(local) {
            Outcome::Win
        } else {
            Outcome::Loss
        };
    }
    if local_conceded(facts) {
        return Outcome::Loss;
    }
    Outcome::Draw
}

/// Whether the local player conceded and left the match.
fn local_conceded(facts: &MatchFacts) -> bool {
    facts
        .state_changes
        .iter()
        .any(|change| is_local_concede(&change.from, &change.to))
}

/// The disconnect that settled the match: the last terminal action was a disconnect
/// and the match then went quiet for `ABANDON_AFTER_MS`, either before a later fact
/// or before `now`. Returns the player who stayed, when the disconnect happened,
/// and the key of the last fact before the quiet (where it settled).
fn settled_disconnect(
    facts: &MatchFacts,
    players: &[String],
    now_ms: Option<i64>,
) -> Option<(String, i64, EventKey)> {
    let mut disconnect: Option<(&str, i64)> = None;
    let mut previous: Option<(EventKey, i64)> = None;
    for (key, at_ms, entry) in timeline(facts) {
        if let (Some((leaver, disconnect_ms)), Some((previous_key, previous_ms))) =
            (disconnect, previous)
            && at_ms - previous_ms >= ABANDON_AFTER_MS
        {
            return Some((other_player(players, leaver)?, disconnect_ms, previous_key));
        }
        match entry {
            Some(GameEntry::LostConnection { player }) => disconnect = Some((player, at_ms)),
            Some(
                GameEntry::WinsGame { .. }
                | GameEntry::ConcededGame { .. }
                | GameEntry::MatchScore { is_final: true, .. },
            ) => disconnect = None,
            _ => {}
        }
        previous = Some((key, at_ms));
    }

    let (leaver, disconnect_ms) = disconnect?;
    if !is_quiet(now_ms, facts.last_activity_ms()) {
        return None;
    }
    let (last_key, _) = previous?;
    Some((other_player(players, leaver)?, disconnect_ms, last_key))
}

/// A join block value, trimmed; an empty or missing value is `None`.
fn kv_text(join: &JoinFact, key: &str) -> Option<String> {
    let value = join.kv.get(key)?.trim();
    if value.is_empty() {
        return None;
    }
    Some(value.to_string())
}

fn tournament_from(join: &JoinFact) -> (Option<u64>, Option<u32>) {
    let source = join
        .kv
        .get("Description")
        .map(String::as_str)
        .unwrap_or(&join.raw_text);
    match TOURNAMENT_ROUND.captures(source) {
        Some(caps) => (caps[1].parse().ok(), caps[2].parse().ok()),
        None => (None, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::facts::{EntryFact, GameStateFact, JoinFact, StateChangeFact};
    use mtgo_protocol::GameEntry;
    use std::collections::BTreeMap;

    const ME: &str = "Local_Name";
    const OPP: &str = "Opp Name";
    const HOUR: i64 = 60 * 60 * 1000;

    fn base() -> MatchFacts {
        let mut facts = MatchFacts::new("token");
        facts.match_id = Some(99);
        let mut kv = BTreeMap::new();
        kv.insert("PlayFormatCd".to_string(), "CMODERN".to_string());
        kv.insert("GameStructureCd".to_string(), "Modern".to_string());
        kv.insert(
            "Description".to_string(),
            "Tournament:123 Round:2".to_string(),
        );
        facts.add_gm_join(JoinFact {
            key: (1, 1),
            at_ms: 1_000,
            kv,
            raw_text: String::new(),
        });
        facts.note_file_username((1, 1), ME);
        facts.add_game_state(GameStateFact {
            key: (1, 2),
            at_ms: 2_000,
            game_id: 7,
            players: Some(vec![ME.to_string(), OPP.to_string()]),
        });
        facts
    }

    fn entry(facts: &mut MatchFacts, offset: u64, entry: GameEntry) {
        facts.add_entry(EntryFact {
            key: (1, offset),
            at_ms: offset as i64 * 1_000,
            game_id: Some(7),
            entry,
        });
    }

    fn change(facts: &mut MatchFacts, offset: u64, from: &str, to: &str) {
        facts.add_state_change(StateChangeFact {
            key: (1, offset),
            at_ms: offset as i64 * 1_000,
            from: from.to_string(),
            to: to.to_string(),
        });
    }

    fn visible(derived: Derived) -> MatchView {
        match derived {
            Derived::Visible(view) => *view,
            other => panic!("expected visible, got {other:?}"),
        }
    }

    #[test]
    fn no_join_or_no_game_state_is_hidden() {
        assert_eq!(derive(&MatchFacts::new("t"), &[], None), Derived::Hidden);
        let mut facts = base();
        facts.game_states.clear();
        assert_eq!(derive(&facts, &[], None), Derived::Hidden);
    }

    #[test]
    fn phantom_gate_rejects_matches_without_the_local_player() {
        let mut facts = base();
        facts.game_states[0].players = Some(vec!["Other A".into(), "Other B".into()]);
        assert_eq!(derive(&facts, &[], None), Derived::Phantom);
        facts.game_states[0].players = Some(vec![ME.into()]);
        assert_eq!(derive(&facts, &[], None), Derived::Phantom);
    }

    #[test]
    fn players_come_from_the_first_game_state_that_names_them() {
        let mut facts = base();
        facts.game_states[0].players = None;
        assert_eq!(derive(&facts, &[], None), Derived::Hidden);
        facts.add_game_state(GameStateFact {
            key: (1, 3),
            at_ms: 3_000,
            game_id: 7,
            players: Some(vec![]),
        });
        assert_eq!(derive(&facts, &[], None), Derived::Hidden);
        facts.add_game_state(GameStateFact {
            key: (1, 4),
            at_ms: 4_000,
            game_id: 7,
            players: Some(vec![ME.into(), OPP.into()]),
        });
        assert_eq!(
            visible(derive(&facts, &[], None)).opponent.as_deref(),
            Some(OPP)
        );
    }

    #[test]
    fn identity_falls_back_to_deck_used_then_known_accounts() {
        let mut facts = base();
        facts.file_username = None;
        assert_eq!(derive(&facts, &[], None), Derived::AwaitingIdentity);
        assert_eq!(
            visible(derive(&facts, &[ME.to_string()], None)).local_username,
            ME
        );
        facts.note_deck_used_username((1, 3), ME);
        assert_eq!(visible(derive(&facts, &[], None)).local_username, ME);
    }

    #[test]
    fn join_metadata_fills_format_type_and_tournament() {
        let view = visible(derive(&base(), &[], None));
        assert_eq!(view.state, MatchState::InProgress);
        assert_eq!(view.format.as_deref(), Some("CMODERN"));
        assert_eq!(view.match_type.as_deref(), Some("Modern"));
        assert_eq!(view.tournament_event_id, Some(123));
        assert_eq!(view.tournament_round, Some(2));
        assert_eq!(view.opponent.as_deref(), Some(OPP));
        assert_eq!(view.started_at_ms, 1_000);
    }

    #[test]
    fn join_metadata_fills_the_league_token_and_game_structure_trimmed() {
        let mut facts = base();
        let join = facts.gm_join.as_mut().expect("join");
        join.kv.insert(
            "League Token".to_string(),
            " d2050286-53fd-4072-804f-190d6a3c030a ".to_string(),
        );
        join.kv
            .insert("GameStructureCd".to_string(), " Modern".to_string());
        let view = visible(derive(&facts, &[], None));
        assert_eq!(
            view.league_token.as_deref(),
            Some("d2050286-53fd-4072-804f-190d6a3c030a")
        );
        assert_eq!(view.game_structure.as_deref(), Some("Modern"));
    }

    #[test]
    fn an_empty_or_missing_league_token_or_game_structure_is_none() {
        let mut facts = base();
        let join = facts.gm_join.as_mut().expect("join");
        join.kv.insert("League Token".to_string(), "  ".to_string());
        join.kv.insert("GameStructureCd".to_string(), String::new());
        let view = visible(derive(&facts, &[], None));
        assert_eq!(view.league_token, None);
        assert_eq!(view.game_structure, None);

        let mut facts = base();
        facts.gm_join.as_mut().expect("join").kv.clear();
        let view = visible(derive(&facts, &[], None));
        assert_eq!(view.league_token, None);
        assert_eq!(view.game_structure, None);
    }

    #[test]
    fn completion_signal_completes_with_outcome() {
        let mut facts = base();
        entry(&mut facts, 3, GameEntry::WinsGame { player: ME.into() });
        change(
            &mut facts,
            4,
            "MatchJoinedEventUnderwayState",
            "MatchJoinedCompletedState",
        );
        let view = visible(derive(&facts, &[], None));
        assert_eq!(view.state, MatchState::Complete);
        assert_eq!(view.resolution, Some(Resolution::Signal));
        assert_eq!(view.outcome, Outcome::Win);
        assert_eq!(view.ended_at_ms, Some(3_000));
    }

    #[test]
    fn a_game_ends_at_its_last_entry_and_the_match_never_before_its_games() {
        let mut facts = base();
        entry(&mut facts, 3, GameEntry::WinsGame { player: ME.into() });
        // The final board snapshot is logged after the "wins the game" line.
        facts.add_game_state(GameStateFact {
            key: (1, 4),
            at_ms: 4_000,
            game_id: 7,
            players: None,
        });
        // A second game shows only board snapshots before the match completes.
        facts.add_game_state(GameStateFact {
            key: (1, 5),
            at_ms: 5_000,
            game_id: 8,
            players: None,
        });
        change(
            &mut facts,
            6,
            "MatchJoinedEventUnderwayState",
            "MatchJoinedCompletedState",
        );
        let view = visible(derive(&facts, &[], None));
        assert_eq!(view.games[0].ended_at_ms, Some(3_000));
        assert_eq!(view.games[1].ended_at_ms, Some(5_000));
        assert_eq!(view.ended_at_ms, Some(5_000));
    }

    #[test]
    fn end_signal_without_completion_is_ended_with_unknown_outcome() {
        let mut facts = base();
        change(
            &mut facts,
            3,
            "LeagueMatchConcedeReqState",
            "LeagueMatchNotJoinedCatchAllState",
        );
        let view = visible(derive(&facts, &[], None));
        assert_eq!(view.state, MatchState::Ended);
        assert_eq!(view.outcome, Outcome::Unknown);
    }

    #[test]
    fn signals_match_the_to_state_only() {
        let mut facts = base();
        change(
            &mut facts,
            3,
            "MatchClosedState",
            "MatchNotJoinedEventUnderwayState",
        );
        assert_eq!(
            visible(derive(&facts, &[], None)).state,
            MatchState::InProgress
        );
    }

    #[test]
    fn disconnects_only_decide_after_an_hour_of_silence() {
        let mut facts = base();
        entry(
            &mut facts,
            3,
            GameEntry::LostConnection { player: OPP.into() },
        );
        let quiet = facts.last_activity_ms();
        assert_eq!(
            visible(derive(&facts, &[], Some(quiet + HOUR - 1))).state,
            MatchState::InProgress
        );
        let view = visible(derive(&facts, &[], Some(quiet + HOUR)));
        assert_eq!(view.state, MatchState::Complete);
        assert_eq!(view.resolution, Some(Resolution::Disconnect));
        assert_eq!(view.outcome, Outcome::Win);
    }

    fn disconnected_then_silent() -> MatchFacts {
        let mut facts = base();
        entry(
            &mut facts,
            3,
            GameEntry::LostConnection { player: OPP.into() },
        );
        facts
    }

    fn assert_disconnect_win(view: MatchView) {
        assert_eq!(view.state, MatchState::Complete);
        assert_eq!(view.resolution, Some(Resolution::Disconnect));
        assert_eq!(view.outcome, Outcome::Win);
        assert_eq!(view.ended_at_ms, Some(3_000));
    }

    #[test]
    fn disconnect_completion_never_regresses_on_a_later_signal() {
        let mut facts = disconnected_then_silent();
        let late = 3_000 + HOUR + 60_000;
        facts.add_state_change(StateChangeFact {
            key: (1, 10),
            at_ms: late,
            from: "MatchJoinedEventUnderwayState".to_string(),
            to: "MatchClosedState".to_string(),
        });
        assert_disconnect_win(visible(derive(&facts, &[], Some(late))));
        assert_disconnect_win(visible(derive(&facts, &[], None)));
    }

    #[test]
    fn disconnect_completion_never_regresses_on_later_activity() {
        let mut facts = disconnected_then_silent();
        let late = 3_000 + HOUR + 60_000;
        facts.add_game_state(GameStateFact {
            key: (1, 10),
            at_ms: late,
            game_id: 7,
            players: None,
        });
        assert_disconnect_win(visible(derive(&facts, &[], Some(late))));
    }

    #[test]
    fn silence_without_a_disconnect_abandons_and_new_events_revive() {
        let mut facts = base();
        let quiet = facts.last_activity_ms();
        let view = visible(derive(&facts, &[], Some(quiet + HOUR)));
        assert_eq!(view.state, MatchState::Abandoned);
        assert_eq!(view.outcome, Outcome::Unknown);

        facts.add_game_state(GameStateFact {
            key: (1, 9),
            at_ms: quiet + HOUR,
            game_id: 7,
            players: None,
        });
        assert_eq!(
            visible(derive(&facts, &[], Some(quiet + HOUR))).state,
            MatchState::InProgress
        );
    }

    #[test]
    fn tentative_outcome_shows_before_completion() {
        let view = visible(derive(&two_local_wins(), &[], None));
        assert_eq!(view.state, MatchState::InProgress);
        assert_eq!(view.outcome, Outcome::Unknown);
        assert_eq!(view.tentative_outcome, Some(Outcome::Win));
    }

    fn two_local_wins() -> MatchFacts {
        let mut facts = base();
        entry(&mut facts, 3, GameEntry::WinsGame { player: ME.into() });
        facts.add_game_state(GameStateFact {
            key: (1, 4),
            at_ms: 4_000,
            game_id: 8,
            players: None,
        });
        facts.add_entry(EntryFact {
            key: (1, 5),
            at_ms: 5_000,
            game_id: Some(8),
            entry: GameEntry::WinsGame { player: ME.into() },
        });
        facts
    }

    fn one_local_win() -> MatchFacts {
        let mut facts = base();
        entry(&mut facts, 3, GameEntry::WinsGame { player: ME.into() });
        facts
    }

    #[test]
    fn an_ended_match_quiet_for_an_hour_completes_from_its_score() {
        let mut facts = one_local_win();
        change(
            &mut facts,
            4,
            "MatchJoinedEventUnderwayState",
            "MatchEndedState",
        );
        let quiet = facts.last_activity_ms();
        assert_eq!(
            visible(derive(&facts, &[], Some(quiet + HOUR - 1))).state,
            MatchState::Ended
        );
        let view = visible(derive(&facts, &[], Some(quiet + HOUR)));
        assert_eq!(view.state, MatchState::Complete);
        assert_eq!(view.resolution, Some(Resolution::Quiet));
        assert_eq!(view.outcome, Outcome::Win);
        assert_eq!(view.ended_at_ms, Some(3_000));
    }

    #[test]
    fn a_local_concede_quiet_for_an_hour_is_a_loss_when_the_score_decides_nothing() {
        let mut facts = base();
        change(
            &mut facts,
            3,
            "LeagueMatchConcedeReqState",
            "LeagueMatchNotJoinedCatchAllState",
        );
        let quiet = facts.last_activity_ms();
        let view = visible(derive(&facts, &[], Some(quiet + HOUR)));
        assert_eq!(view.state, MatchState::Complete);
        assert_eq!(view.resolution, Some(Resolution::Quiet));
        assert_eq!(view.outcome, Outcome::Loss);
    }

    #[test]
    fn a_decided_score_quiet_for_an_hour_completes_instead_of_abandoning() {
        let facts = two_local_wins();
        let quiet = facts.last_activity_ms();
        let view = visible(derive(&facts, &[], Some(quiet + HOUR)));
        assert_eq!(view.state, MatchState::Complete);
        assert_eq!(view.resolution, Some(Resolution::Quiet));
        assert_eq!(view.outcome, Outcome::Win);
        assert_eq!(view.tentative_outcome, None);
    }

    #[test]
    fn a_quiet_completion_never_regresses_on_later_activity() {
        let mut facts = two_local_wins();
        let late = facts.last_activity_ms() + HOUR + 60_000;
        // A stray state change an hour later (logs lie about order and timing).
        facts.add_state_change(StateChangeFact {
            key: (1, 10),
            at_ms: late,
            from: "MatchJoinedEventUnderwayState".to_string(),
            to: "MatchJoinedEventUnderwayState".to_string(),
        });
        let view = visible(derive(&facts, &[], Some(late)));
        assert_eq!(view.state, MatchState::Complete);
        assert_eq!(view.resolution, Some(Resolution::Quiet));
        assert_eq!(view.outcome, Outcome::Win);
    }

    #[test]
    fn a_late_final_score_line_does_not_undo_a_quiet_completion() {
        let mut facts = two_local_wins();
        let late = facts.last_activity_ms() + HOUR + 60_000;
        facts.add_entry(EntryFact {
            key: (1, 10),
            at_ms: late,
            game_id: Some(8),
            entry: GameEntry::MatchScore {
                player: ME.into(),
                wins: 2,
                losses: 0,
                is_final: true,
            },
        });
        let view = visible(derive(&facts, &[], Some(late)));
        assert_eq!(view.state, MatchState::Complete);
        assert_eq!(view.resolution, Some(Resolution::Quiet));
        assert_eq!(view.outcome, Outcome::Win);
    }

    #[test]
    fn a_late_game_for_the_opponent_does_not_change_a_quiet_outcome() {
        let mut facts = two_local_wins();
        let late = facts.last_activity_ms() + HOUR + 60_000;
        facts.add_game_state(GameStateFact {
            key: (1, 10),
            at_ms: late,
            game_id: 9,
            players: None,
        });
        facts.add_entry(EntryFact {
            key: (1, 11),
            at_ms: late + 1_000,
            game_id: Some(9),
            entry: GameEntry::WinsGame { player: OPP.into() },
        });
        let view = visible(derive(&facts, &[], Some(late + 1_000)));
        assert_eq!(view.state, MatchState::Complete);
        assert_eq!(view.resolution, Some(Resolution::Quiet));
        assert_eq!(view.outcome, Outcome::Win);
    }

    #[test]
    fn an_ended_match_quiet_between_facts_completes_when_a_stray_fact_follows() {
        let mut facts = one_local_win();
        change(
            &mut facts,
            4,
            "MatchJoinedEventUnderwayState",
            "MatchEndedState",
        );
        let late = facts.last_activity_ms() + HOUR + 60_000;
        facts.add_state_change(StateChangeFact {
            key: (1, 10),
            at_ms: late,
            from: "MatchEndedState".to_string(),
            to: "MatchEndedState".to_string(),
        });
        let view = visible(derive(&facts, &[], Some(late)));
        assert_eq!(view.state, MatchState::Complete);
        assert_eq!(view.resolution, Some(Resolution::Quiet));
        assert_eq!(view.outcome, Outcome::Win);
    }

    fn two_wins_then_late_change(from: &str, to: &str) -> MatchFacts {
        let mut facts = two_local_wins();
        let late = facts.last_activity_ms() + HOUR + 60_000;
        facts.add_state_change(StateChangeFact {
            key: (1, 10),
            at_ms: late,
            from: from.to_string(),
            to: to.to_string(),
        });
        facts
    }

    fn assert_quiet_win(facts: &MatchFacts) {
        let late = facts.last_activity_ms();
        for now in [Some(late), None] {
            let view = visible(derive(facts, &[], now));
            assert_eq!(view.state, MatchState::Complete);
            assert_eq!(view.resolution, Some(Resolution::Quiet));
            assert_eq!(view.outcome, Outcome::Win);
        }
    }

    #[test]
    fn a_late_end_signal_does_not_undo_an_earlier_decided_quiet_gap() {
        let facts = two_wins_then_late_change("MatchJoinedEventUnderwayState", "MatchEndedState");
        assert_quiet_win(&facts);
    }

    #[test]
    fn a_late_local_concede_does_not_undo_an_earlier_decided_quiet_gap() {
        let facts = two_wins_then_late_change(
            "LeagueMatchConcedeReqState",
            "LeagueMatchNotJoinedCatchAllState",
        );
        assert_quiet_win(&facts);
    }

    #[test]
    fn tentative_outcome_is_hidden_once_the_match_ended_or_was_abandoned() {
        // 1-0 decides nothing, so silence still abandons it.
        let undecided = one_local_win();
        let quiet = undecided.last_activity_ms();
        let abandoned = visible(derive(&undecided, &[], Some(quiet + HOUR)));
        assert_eq!(abandoned.state, MatchState::Abandoned);
        assert_eq!(abandoned.tentative_outcome, None);

        let mut facts = two_local_wins();
        change(
            &mut facts,
            6,
            "LeagueMatchConcedeReqState",
            "LeagueMatchNotJoinedCatchAllState",
        );
        let ended = visible(derive(&facts, &[], None));
        assert_eq!(ended.state, MatchState::Ended);
        assert_eq!(ended.tentative_outcome, None);
    }

    #[test]
    fn a_late_completion_signal_does_not_change_a_quiet_outcome() {
        // A local concede at 0-0 settles as a loss after an hour of quiet.
        let mut facts = base();
        change(
            &mut facts,
            3,
            "LeagueMatchConcedeReqState",
            "LeagueMatchNotJoinedCatchAllState",
        );
        let late = facts.last_activity_ms() + HOUR + 60_000;
        facts.add_state_change(StateChangeFact {
            key: (1, 10),
            at_ms: late,
            from: "LeagueMatchNotJoinedCatchAllState".to_string(),
            to: "MatchCompletedState".to_string(),
        });
        for now in [Some(late), None] {
            let view = visible(derive(&facts, &[], now));
            assert_eq!(view.state, MatchState::Complete);
            assert_eq!(view.resolution, Some(Resolution::Quiet));
            assert_eq!(view.outcome, Outcome::Loss);
        }
    }

    #[test]
    fn a_late_disconnect_does_not_change_a_quiet_outcome() {
        let mut facts = two_local_wins();
        let late = facts.last_activity_ms() + HOUR + 60_000;
        facts.add_entry(EntryFact {
            key: (1, 10),
            at_ms: late,
            game_id: Some(8),
            entry: GameEntry::LostConnection { player: ME.into() },
        });
        let view = visible(derive(&facts, &[], Some(late + HOUR)));
        assert_eq!(view.state, MatchState::Complete);
        assert_eq!(view.resolution, Some(Resolution::Quiet));
        assert_eq!(view.outcome, Outcome::Win);
    }

    #[test]
    fn a_disconnect_after_a_completion_signal_does_not_change_its_outcome() {
        let mut facts = one_local_win();
        change(
            &mut facts,
            4,
            "MatchJoinedEventUnderwayState",
            "MatchJoinedCompletedState",
        );
        entry(
            &mut facts,
            5,
            GameEntry::LostConnection { player: ME.into() },
        );
        let view = visible(derive(&facts, &[], Some(5_000 + HOUR)));
        assert_eq!(view.state, MatchState::Complete);
        assert_eq!(view.resolution, Some(Resolution::Signal));
        assert_eq!(view.outcome, Outcome::Win);
        // A signal's end time uses all the match's facts (spec 5.5).
        assert_eq!(view.ended_at_ms, Some(5_000));
    }

    #[test]
    fn a_completion_signal_after_a_lost_connection_never_turns_into_a_disconnect() {
        let mut facts = base();
        entry(
            &mut facts,
            3,
            GameEntry::LostConnection { player: OPP.into() },
        );
        change(
            &mut facts,
            4,
            "MatchJoinedEventUnderwayState",
            "MatchJoinedCompletedState",
        );
        let views: Vec<MatchView> = [Some(4_000 + 1), Some(4_000 + HOUR), None]
            .into_iter()
            .map(|now| visible(derive(&facts, &[], now)))
            .collect();
        for view in &views {
            assert_eq!(view.state, MatchState::Complete);
            assert_eq!(view.resolution, Some(Resolution::Signal));
        }
        assert_eq!(views[0], views[1]);
        assert_eq!(views[0], views[2]);
    }

    fn entry_in(facts: &mut MatchFacts, offset: u64, game_id: u64, entry: GameEntry) {
        facts.add_entry(EntryFact {
            key: (1, offset),
            at_ms: offset as i64 * 1_000,
            game_id: Some(game_id),
            entry,
        });
    }

    /// The opponent wins game 1, concedes game 2, and the score is level.
    fn level_after_two_games() -> MatchFacts {
        let mut facts = base();
        entry_in(&mut facts, 3, 7, GameEntry::WinsGame { player: OPP.into() });
        entry_in(
            &mut facts,
            4,
            8,
            GameEntry::ConcededGame { player: OPP.into() },
        );
        entry_in(&mut facts, 5, 8, GameEntry::WinsGame { player: ME.into() });
        facts
    }

    fn completed_at(facts: &mut MatchFacts, offset: u64) {
        change(
            facts,
            offset,
            "LeagueMatchJoinedSideboardingState",
            "LeagueMatchJoinedCompletedState",
        );
    }

    #[test]
    fn a_player_who_drops_at_a_level_score_loses_the_match() {
        // MTGO's own log: "has lost connection" after the level score, then
        // "Match Tied 1-1", and MTGO gives the match to the player who stayed.
        let mut facts = level_after_two_games();
        entry_in(
            &mut facts,
            6,
            8,
            GameEntry::LostConnection { player: OPP.into() },
        );
        entry_in(
            &mut facts,
            7,
            8,
            GameEntry::MatchTied {
                first: 1,
                second: 1,
            },
        );
        completed_at(&mut facts, 8);
        let view = visible(derive(&facts, &[], None));
        assert_eq!(view.resolution, Some(Resolution::Signal));
        assert_eq!((view.wins, view.losses), (1, 1));
        assert_eq!(view.games.len(), 2);
        assert_eq!(view.outcome, Outcome::Win);

        let mut facts = level_after_two_games();
        entry_in(
            &mut facts,
            6,
            8,
            GameEntry::LostConnection { player: ME.into() },
        );
        completed_at(&mut facts, 7);
        assert_eq!(visible(derive(&facts, &[], None)).outcome, Outcome::Loss);
    }

    #[test]
    fn a_level_score_without_a_drop_is_still_a_draw() {
        let mut facts = level_after_two_games();
        completed_at(&mut facts, 6);
        assert_eq!(visible(derive(&facts, &[], None)).outcome, Outcome::Draw);
    }

    #[test]
    fn a_drop_before_the_last_game_or_after_the_signal_decides_nothing() {
        // Lost connection during game 2, which then ended normally.
        let mut facts = base();
        entry_in(&mut facts, 3, 7, GameEntry::WinsGame { player: OPP.into() });
        entry_in(
            &mut facts,
            4,
            8,
            GameEntry::LostConnection { player: OPP.into() },
        );
        entry_in(&mut facts, 5, 8, GameEntry::WinsGame { player: ME.into() });
        completed_at(&mut facts, 6);
        assert_eq!(visible(derive(&facts, &[], None)).outcome, Outcome::Draw);

        // Reconnected and a new game started.
        let mut facts = level_after_two_games();
        entry_in(
            &mut facts,
            6,
            8,
            GameEntry::LostConnection { player: OPP.into() },
        );
        entry_in(
            &mut facts,
            7,
            9,
            GameEntry::Rolled {
                player: OPP.into(),
                value: 4,
            },
        );
        completed_at(&mut facts, 8);
        assert_eq!(visible(derive(&facts, &[], None)).outcome, Outcome::Draw);

        // The lost connection comes after the match already completed.
        let mut facts = level_after_two_games();
        completed_at(&mut facts, 6);
        entry_in(
            &mut facts,
            7,
            8,
            GameEntry::LostConnection { player: OPP.into() },
        );
        assert_eq!(visible(derive(&facts, &[], None)).outcome, Outcome::Draw);
    }

    #[test]
    fn a_local_concede_at_a_level_score_is_a_loss() {
        let mut facts = level_after_two_games();
        change(
            &mut facts,
            6,
            "LeagueMatchConcedeReqState",
            "LeagueMatchNotJoinedEventUnderwayState",
        );
        let quiet = facts.last_activity_ms();
        let view = visible(derive(&facts, &[], Some(quiet + HOUR)));
        assert_eq!(view.resolution, Some(Resolution::Quiet));
        assert_eq!(view.outcome, Outcome::Loss);

        completed_at(&mut facts, 7);
        assert_eq!(visible(derive(&facts, &[], None)).outcome, Outcome::Loss);
    }
}
