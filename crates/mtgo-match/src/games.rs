//! Per-game analysis (port of 0.x `ExtractGameResults::analyzeGame`), trailing
//! forfeits, match score, and outcome rules. Games are keyed by MTGO Game ID.

use std::collections::{BTreeSet, HashMap};

use mtgo_protocol::GameEntry;

use crate::facts::{EventKey, MatchFacts};
use crate::model::{EndReason, GameResult, GameView, Outcome};

pub fn other_player(players: &[String], name: &str) -> Option<String> {
    players
        .iter()
        .find(|player| player.as_str() != name)
        .cloned()
}

/// One `GameView` per Game ID, numbered in order of first appearance.
pub fn build_games(facts: &MatchFacts, local: &str, players: &[String]) -> Vec<GameView> {
    let mut appearances: Vec<(EventKey, u64)> = facts
        .game_states
        .iter()
        .map(|state| (state.key, state.game_id))
        .chain(
            facts
                .entries
                .iter()
                .filter_map(|e| e.game_id.map(|id| (e.key, id))),
        )
        .collect();
    appearances.sort();

    let mut seen = BTreeSet::new();
    let mut order = Vec::new();
    for (_, game_id) in appearances {
        if seen.insert(game_id) {
            order.push(game_id);
        }
    }

    order
        .iter()
        .enumerate()
        .map(|(index, game_id)| analyse_game(facts, *game_id, index as u32 + 1, local, players))
        .collect()
}

fn analyse_game(
    facts: &MatchFacts,
    game_id: u64,
    game_number: u32,
    local: &str,
    players: &[String],
) -> GameView {
    let mut winner: Option<String> = None;
    let mut end_reason = EndReason::Unknown;
    let mut on_play_player: Option<String> = None;
    let mut hands: HashMap<String, u32> = HashMap::new();
    let mut rolls: HashMap<String, u32> = HashMap::new();
    let mut turn_count: Option<u32> = None;
    let state_times: Vec<i64> = facts
        .game_states
        .iter()
        .filter(|state| state.game_id == game_id)
        .map(|state| state.at_ms)
        .collect();
    let mut entry_times: Vec<i64> = Vec::new();

    for fact in facts.entries.iter().filter(|e| e.game_id == Some(game_id)) {
        entry_times.push(fact.at_ms);
        match &fact.entry {
            GameEntry::ChoosesToPlay { player, first } => {
                on_play_player = if *first {
                    Some(player.clone())
                } else {
                    other_player(players, player)
                };
            }
            GameEntry::BeginsGame { player, hand_size } => {
                hands.insert(player.clone(), *hand_size);
            }
            GameEntry::Rolled { player, value } => {
                rolls.insert(player.clone(), *value);
            }
            GameEntry::WinsGame { player } => {
                winner = Some(player.clone());
                end_reason = EndReason::Win;
            }
            GameEntry::ConcededGame { player } => {
                if winner.is_none()
                    && let Some(other) = other_player(players, player)
                {
                    winner = Some(other);
                    end_reason = EndReason::Concede;
                }
            }
            GameEntry::Turn { number } => {
                turn_count = Some(turn_count.map_or(*number, |current| current.max(*number)));
            }
            _ => {}
        }
    }

    // A game starts at its first sign of life. It ends at its last gameplay entry,
    // as in 0.x, because board snapshots keep arriving after "wins the game";
    // snapshots only date the end when the game has no entries at all.
    let started_at_ms = state_times.iter().chain(entry_times.iter()).min().copied();
    let ended_at_ms = entry_times
        .iter()
        .max()
        .or_else(|| state_times.iter().max())
        .copied();

    let opponent = other_player(players, local);
    let mulligans = |name: &str| hands.get(name).map(|kept| 7u32.saturating_sub(*kept));
    GameView {
        mtgo_game_id: Some(game_id),
        game_number,
        result: match &winner {
            Some(name) if name == local => GameResult::Win,
            Some(_) => GameResult::Loss,
            None => GameResult::Unknown,
        },
        end_reason,
        on_play: on_play_player.map(|name| name == local),
        local_mulligans: mulligans(local),
        opp_mulligans: opponent.as_deref().and_then(mulligans),
        local_die_roll: rolls.get(local).copied(),
        opp_die_roll: opponent.as_ref().and_then(|name| rolls.get(name)).copied(),
        turn_count,
        started_at_ms,
        ended_at_ms,
    }
}

pub fn count_results(games: &[GameView]) -> (u32, u32) {
    let wins = games.iter().filter(|g| g.result == GameResult::Win).count() as u32;
    let losses = games
        .iter()
        .filter(|g| g.result == GameResult::Loss)
        .count() as u32;
    (wins, losses)
}

/// Port of 0.x `detectTrailingForfeit`: a level, nonzero score followed by a concede
/// with no new game starting means the conceding player forfeited the decider.
pub fn trailing_forfeit(
    facts: &MatchFacts,
    games: &[GameView],
    local: &str,
    players: &[String],
) -> Option<GameView> {
    let last = games.last()?;
    if last.result == GameResult::Unknown {
        return None;
    }
    let (wins, losses) = count_results(games);
    if wins == 0 || wins != losses {
        return None;
    }

    let last_win = facts
        .entries
        .iter()
        .rposition(|e| matches!(e.entry, GameEntry::WinsGame { .. }))?;
    let mut leaver: Option<(String, i64)> = None;
    for fact in &facts.entries[last_win + 1..] {
        match &fact.entry {
            GameEntry::Rolled { .. } | GameEntry::Joined { .. } => return None,
            GameEntry::ConcededGame { player } => leaver = Some((player.clone(), fact.at_ms)),
            _ => {}
        }
    }

    let (leaver, at_ms) = leaver?;
    let winner = other_player(players, &leaver)?;
    Some(GameView {
        mtgo_game_id: None,
        game_number: games.len() as u32 + 1,
        result: if winner == local {
            GameResult::Win
        } else {
            GameResult::Loss
        },
        end_reason: EndReason::Forfeit,
        on_play: None,
        local_mulligans: None,
        opp_mulligans: None,
        local_die_roll: None,
        opp_die_roll: None,
        turn_count: None,
        started_at_ms: Some(at_ms),
        ended_at_ms: Some(at_ms),
    })
}

/// The player who left after the last game result: their "has lost
/// connection" line follows the last "wins the game" and no new game starts.
/// MTGO gives a match left at a level score to the player who stayed, but
/// logs only "Match Tied" (the "wins the match" line is not in the live log).
pub fn trailing_leaver(facts: &MatchFacts) -> Option<String> {
    let last_win = facts
        .entries
        .iter()
        .rposition(|e| matches!(e.entry, GameEntry::WinsGame { .. }))?;
    let mut leaver: Option<String> = None;
    for fact in &facts.entries[last_win + 1..] {
        match &fact.entry {
            GameEntry::Rolled { .. } | GameEntry::Joined { .. } => return None,
            GameEntry::LostConnection { player } => leaver = Some(player.clone()),
            _ => {}
        }
    }
    leaver
}

/// The last "leads/wins the match" or "Match Tied" line, oriented to the local
/// player; otherwise counted game results. A score only grows, so each side takes
/// the larger of the two: a line logged before the last game ended (or a game
/// result that never got logged) cannot understate the score.
pub fn score(facts: &MatchFacts, games: &[GameView], local: &str) -> (u32, u32) {
    let last_line = facts
        .entries
        .iter()
        .filter_map(|fact| match &fact.entry {
            GameEntry::MatchScore {
                player,
                wins,
                losses,
                ..
            } => Some(if player == local {
                (*wins, *losses)
            } else {
                (*losses, *wins)
            }),
            GameEntry::MatchTied { first, second } => Some((*first, *second)),
            _ => None,
        })
        .next_back();
    let (counted_wins, counted_losses) = count_results(games);
    match last_line {
        Some((wins, losses)) => (wins.max(counted_wins), losses.max(counted_losses)),
        None => (counted_wins, counted_losses),
    }
}

pub fn outcome_from(wins: u32, losses: u32) -> Outcome {
    if wins > losses {
        Outcome::Win
    } else if losses > wins {
        Outcome::Loss
    } else if wins > 0 {
        Outcome::Draw
    } else {
        Outcome::Unknown
    }
}

/// Live hint: a side has reached the win threshold (3 once either side has 3, else 2)
/// and is ahead. A level score such as 2-2 in a best of five decides nothing yet.
pub fn threshold_outcome(wins: u32, losses: u32) -> Option<Outcome> {
    let threshold = if wins >= 3 || losses >= 3 { 3 } else { 2 };
    if wins >= threshold && wins > losses {
        Some(Outcome::Win)
    } else if losses >= threshold && losses > wins {
        Some(Outcome::Loss)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::facts::{EntryFact, GameStateFact, MatchFacts};
    use mtgo_protocol::GameEntry;

    const ME: &str = "Local_Name";
    const OPP: &str = "Opp Name";

    fn players() -> Vec<String> {
        vec![ME.to_string(), OPP.to_string()]
    }

    struct Builder {
        facts: MatchFacts,
        offset: u64,
    }

    impl Builder {
        fn new() -> Self {
            Self {
                facts: MatchFacts::new("token"),
                offset: 0,
            }
        }
        fn state(&mut self, game_id: u64) -> &mut Self {
            self.offset += 1;
            self.facts.add_game_state(GameStateFact {
                key: (1, self.offset),
                at_ms: self.offset as i64 * 1000,
                game_id,
                players: Some(players()),
            });
            self
        }
        fn entry(&mut self, game_id: u64, entry: GameEntry) -> &mut Self {
            self.offset += 1;
            self.facts.add_entry(EntryFact {
                key: (1, self.offset),
                at_ms: self.offset as i64 * 1000,
                game_id: Some(game_id),
                entry,
            });
            self
        }
    }

    fn wins(player: &str) -> GameEntry {
        GameEntry::WinsGame {
            player: player.into(),
        }
    }

    #[test]
    fn games_follow_first_appearance_of_their_game_id() {
        let mut b = Builder::new();
        b.state(20)
            .state(10)
            .entry(10, wins(ME))
            .entry(20, wins(OPP));
        let games = build_games(&b.facts, ME, &players());
        assert_eq!(
            games.iter().map(|g| g.mtgo_game_id).collect::<Vec<_>>(),
            vec![Some(20), Some(10)]
        );
        assert_eq!(games[0].game_number, 1);
        assert_eq!(games[0].result, GameResult::Loss);
        assert_eq!(games[1].result, GameResult::Win);
    }

    #[test]
    fn per_game_details_come_from_entries() {
        let mut b = Builder::new();
        b.state(1)
            .entry(
                1,
                GameEntry::Rolled {
                    player: ME.into(),
                    value: 3,
                },
            )
            .entry(
                1,
                GameEntry::Rolled {
                    player: OPP.into(),
                    value: 5,
                },
            )
            .entry(
                1,
                GameEntry::ChoosesToPlay {
                    player: OPP.into(),
                    first: false,
                },
            )
            .entry(
                1,
                GameEntry::BeginsGame {
                    player: ME.into(),
                    hand_size: 6,
                },
            )
            .entry(
                1,
                GameEntry::BeginsGame {
                    player: OPP.into(),
                    hand_size: 7,
                },
            )
            .entry(1, GameEntry::Turn { number: 9 })
            .entry(1, GameEntry::ConcededGame { player: OPP.into() });
        let game = &build_games(&b.facts, ME, &players())[0];
        assert_eq!(game.on_play, Some(true));
        assert_eq!(game.local_mulligans, Some(1));
        assert_eq!(game.opp_mulligans, Some(0));
        assert_eq!(game.local_die_roll, Some(3));
        assert_eq!(game.opp_die_roll, Some(5));
        assert_eq!(game.turn_count, Some(9));
        assert_eq!(game.result, GameResult::Win);
        assert_eq!(game.end_reason, EndReason::Concede);
    }

    #[test]
    fn lost_connection_never_decides_a_game() {
        let mut b = Builder::new();
        b.state(1)
            .entry(1, GameEntry::LostConnection { player: OPP.into() });
        let game = &build_games(&b.facts, ME, &players())[0];
        assert_eq!(game.result, GameResult::Unknown);
    }

    #[test]
    fn trailing_concede_at_one_all_adds_a_forfeit_game() {
        let mut b = Builder::new();
        b.state(1).entry(1, wins(ME)).state(2).entry(2, wins(OPP));
        b.entry(2, GameEntry::ConcededGame { player: OPP.into() });
        let games = build_games(&b.facts, ME, &players());
        let forfeit = trailing_forfeit(&b.facts, &games, ME, &players()).expect("forfeit");
        assert_eq!(forfeit.mtgo_game_id, None);
        assert_eq!(forfeit.result, GameResult::Win);
        assert_eq!(forfeit.end_reason, EndReason::Forfeit);
        assert_eq!(forfeit.game_number, 3);
    }

    #[test]
    fn no_forfeit_when_someone_is_ahead_or_a_new_game_started() {
        let mut ahead = Builder::new();
        ahead
            .state(1)
            .entry(1, wins(ME))
            .entry(1, GameEntry::ConcededGame { player: OPP.into() });
        let games = build_games(&ahead.facts, ME, &players());
        assert_eq!(trailing_forfeit(&ahead.facts, &games, ME, &players()), None);

        let mut restarted = Builder::new();
        restarted
            .state(1)
            .entry(1, wins(ME))
            .state(2)
            .entry(2, wins(OPP));
        restarted.entry(
            2,
            GameEntry::Rolled {
                player: ME.into(),
                value: 1,
            },
        );
        restarted.entry(2, GameEntry::ConcededGame { player: OPP.into() });
        let games = build_games(&restarted.facts, ME, &players());
        assert_eq!(
            trailing_forfeit(&restarted.facts, &games, ME, &players()),
            None
        );
    }

    #[test]
    fn score_prefers_the_last_score_line_oriented_to_local() {
        let mut b = Builder::new();
        b.state(1).entry(1, wins(ME));
        b.entry(
            1,
            GameEntry::MatchScore {
                player: OPP.into(),
                wins: 2,
                losses: 1,
                is_final: true,
            },
        );
        let games = build_games(&b.facts, ME, &players());
        assert_eq!(score(&b.facts, &games, ME), (1, 2));
    }

    #[test]
    fn a_stale_score_line_never_hides_a_game_counted_after_it() {
        let mut b = Builder::new();
        b.state(1).entry(1, wins(ME));
        b.state(2).entry(2, wins(OPP));
        b.entry(
            2,
            GameEntry::MatchTied {
                first: 1,
                second: 1,
            },
        );
        // Game 3 is won, but the "wins the match" line never made it to the log.
        b.state(3).entry(3, wins(ME));
        let games = build_games(&b.facts, ME, &players());
        assert_eq!(score(&b.facts, &games, ME), (2, 1));
    }

    #[test]
    fn outcome_rules() {
        assert_eq!(outcome_from(2, 1), Outcome::Win);
        assert_eq!(outcome_from(0, 2), Outcome::Loss);
        assert_eq!(outcome_from(1, 1), Outcome::Draw);
        assert_eq!(outcome_from(0, 0), Outcome::Unknown);
        assert_eq!(threshold_outcome(2, 0), Some(Outcome::Win));
        assert_eq!(threshold_outcome(2, 2), None);
        assert_eq!(threshold_outcome(3, 2), Some(Outcome::Win));
        assert_eq!(threshold_outcome(1, 0), None);
    }
}
