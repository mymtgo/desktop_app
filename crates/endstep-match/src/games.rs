//! Per-game records: what the fold learns about each game number, by seat.
//!
//! Seats are kept as Endstep sends them (indexes) and turned into the user's
//! side only at the end, so a match body that arrives after the game frames
//! still gives the same games.

use endstep_protocol::{GameDelta, GameEvent, GameState};
use mtgo_match::{EndReason, GameResult, GameView};

use crate::MAX_GAMES;
use crate::cards::CardTally;
use crate::hands::HandTally;
use crate::log::EndstepLogLine;
use crate::sideboard::SideboardTally;

/// One game of the match. Forge reuses one game id for every game of a Bo3,
/// so a game is its `game_number` (trap 7).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GameRecord {
    pub number: u32,
    /// The `at` of the first frame of this game number.
    pub started_at_ms: i64,
    /// Set by `GAME_OUTCOME`.
    pub decided: bool,
    pub winner_seat: Option<u32>,
    pub end_reason: Option<String>,
    pub turn_count: Option<u32>,
    pub ended_at_ms: Option<i64>,
    /// The active seat of the first state on turn 1.
    pub play_seat: Option<u32>,
    /// The seat of each `MULLIGAN` event with an `amount`.
    pub mulligan_seats: Vec<u32>,
    /// The cards this game's states showed and its events counted.
    pub cards: CardTally,
    /// The hands the states and events showed, by seat.
    pub hands: HandTally,
    /// The sideboard prompt that opened this game and the answer to it.
    pub sideboard: SideboardTally,
    /// This game's log lines, in event order.
    pub log: Vec<EndstepLogLine>,
    /// This game's frames in walk order, kept only for the replay.
    pub frames: Vec<KeptFrame>,
}

/// One frame of a game as the walk read it: its arrival time and what it
/// holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct KeptFrame {
    pub at_ms: i64,
    pub kind: KeptKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum KeptKind {
    State(GameState),
    Delta(GameDelta),
    Event(GameEvent),
}

/// The record for game `number`, created at `at_ms` when it is new. The list
/// stays sorted by game number.
pub(crate) fn game_mut(games: &mut Vec<GameRecord>, number: u32, at_ms: i64) -> &mut GameRecord {
    let mut index = games.len();
    for (position, game) in games.iter().enumerate() {
        if game.number == number {
            return &mut games[position];
        }
        if game.number > number {
            index = position;
            break;
        }
    }
    games.insert(
        index,
        GameRecord {
            number,
            started_at_ms: at_ms,
            decided: false,
            winner_seat: None,
            end_reason: None,
            turn_count: None,
            ended_at_ms: None,
            play_seat: None,
            mulligan_seats: Vec::new(),
            cards: CardTally::default(),
            hands: HandTally::default(),
            sideboard: SideboardTally::default(),
            log: Vec::new(),
            frames: Vec::new(),
        },
    );
    &mut games[index]
}

/// The most games this match can have: `gamesPerMatch` clamped to 1 to
/// `MAX_GAMES` when known, else `MAX_GAMES`.
pub(crate) fn game_limit(games_per_match: Option<u32>) -> u32 {
    match games_per_match {
        Some(games) => games.clamp(1, MAX_GAMES),
        None => MAX_GAMES,
    }
}

/// The views of games 1 to n, one per number with no gaps: n is the highest
/// game number recorded or `games_played`, whichever is larger, and never
/// above `limit`. A game known only by number (its frames were lost) is a
/// placeholder. Game number 0 and repeated numbers never reach the view (the
/// store expects games 1 to n).
pub(crate) fn game_views(
    games: &[GameRecord],
    games_played: u32,
    limit: u32,
    seat: u32,
) -> Vec<GameView> {
    let mut highest = games_played;
    for game in games {
        highest = highest.max(game.number);
    }
    let highest = highest.min(limit);
    let mut views = Vec::new();
    for number in 1..=highest {
        match games.iter().find(|game| game.number == number) {
            Some(game) => views.push(game_view(game, seat)),
            None => views.push(placeholder_view(number)),
        }
    }
    views
}

/// A game known only by its number: nothing about it is known.
fn placeholder_view(number: u32) -> GameView {
    GameView {
        mtgo_game_id: None,
        game_number: number,
        result: GameResult::Unknown,
        end_reason: EndReason::Unknown,
        on_play: None,
        local_mulligans: None,
        opp_mulligans: None,
        local_die_roll: None,
        opp_die_roll: None,
        turn_count: None,
        started_at_ms: None,
        ended_at_ms: None,
    }
}

/// The shared view of one game, seen from the user's `seat`.
fn game_view(game: &GameRecord, seat: u32) -> GameView {
    let result = match (game.decided, game.winner_seat) {
        (true, Some(winner)) if winner == seat => GameResult::Win,
        (true, Some(_)) => GameResult::Loss,
        _ => GameResult::Unknown,
    };
    // Each source can miss a mulligan (a lost event, a lost state), none
    // counts one too many: the larger count is the one the page and Game
    // Stats both show (#707).
    let mut local_mulligans: u32 = 0;
    let mut opp_mulligans: u32 = 0;
    for mulligan_seat in &game.mulligan_seats {
        if *mulligan_seat == seat {
            local_mulligans = local_mulligans.saturating_add(1);
        } else {
            opp_mulligans = opp_mulligans.saturating_add(1);
        }
    }
    let local_mulligans = local_mulligans.max(game.hands.local_mulligans(seat));
    let opp_mulligans = opp_mulligans.max(game.hands.opponent_mulligans(seat));
    GameView {
        mtgo_game_id: None,
        game_number: game.number,
        result,
        end_reason: end_reason(game),
        on_play: game.play_seat.map(|play_seat| play_seat == seat),
        local_mulligans: Some(local_mulligans),
        opp_mulligans: Some(opp_mulligans),
        local_die_roll: None,
        opp_die_roll: None,
        turn_count: game.turn_count,
        started_at_ms: Some(game.started_at_ms),
        ended_at_ms: game.ended_at_ms,
    }
}

/// Only `concede` has been seen in a capture; Forge's other end reasons map
/// to `Unknown` until a capture shows them.
fn end_reason(game: &GameRecord) -> EndReason {
    if !game.decided {
        return EndReason::Unknown;
    }
    match game.end_reason.as_deref() {
        Some("concede") => EndReason::Concede,
        _ => EndReason::Unknown,
    }
}

/// Wins and losses counted from the decided games up to `limit`, for when no
/// state gave a score.
pub(crate) fn counted_score(games: &[GameRecord], limit: u32, seat: u32) -> (u32, u32) {
    let mut wins = 0;
    let mut losses = 0;
    for game in games {
        if game.number == 0 || game.number > limit {
            continue;
        }
        match game_view(game, seat).result {
            GameResult::Win => wins += 1,
            GameResult::Loss => losses += 1,
            GameResult::Unknown => {}
        }
    }
    (wins, losses)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decided(number: u32, winner_seat: u32) -> GameRecord {
        GameRecord {
            number,
            started_at_ms: 0,
            decided: true,
            winner_seat: Some(winner_seat),
            end_reason: Some("concede".to_string()),
            turn_count: None,
            ended_at_ms: None,
            play_seat: None,
            mulligan_seats: Vec::new(),
            cards: CardTally::default(),
            hands: HandTally::default(),
            sideboard: SideboardTally::default(),
            log: Vec::new(),
            frames: Vec::new(),
        }
    }

    fn numbers(views: &[GameView]) -> Vec<u32> {
        views.iter().map(|view| view.game_number).collect()
    }

    #[test]
    fn game_zero_and_repeats_never_reach_the_view() {
        let games = vec![decided(0, 0), decided(1, 0), decided(1, 1), decided(3, 1)];
        let views = game_views(&games, 0, MAX_GAMES, 0);
        assert_eq!(numbers(&views), vec![1, 2, 3]);
        // The first record of a repeated number wins.
        assert_eq!(views[0].result, GameResult::Win);
        assert_eq!(views[1], placeholder_view(2));
        assert_eq!(views[2].result, GameResult::Loss);
    }

    #[test]
    fn games_played_shows_games_whose_frames_were_lost() {
        let views = game_views(&[decided(1, 0)], 2, MAX_GAMES, 0);
        assert_eq!(numbers(&views), vec![1, 2]);
        assert_eq!(views[1].result, GameResult::Unknown);
        assert!(game_views(&[], 0, MAX_GAMES, 0).is_empty());
    }

    #[test]
    fn games_never_go_above_the_limit() {
        let views = game_views(&[decided(1, 0), decided(5, 1)], 1000, 3, 0);
        assert_eq!(numbers(&views), vec![1, 2, 3]);
        assert_eq!(counted_score(&[decided(1, 0), decided(5, 1)], 3, 0), (1, 0));
        assert_eq!(game_limit(None), MAX_GAMES);
        assert_eq!(game_limit(Some(0)), 1);
        assert_eq!(game_limit(Some(3)), 3);
        assert_eq!(game_limit(Some(100_000)), MAX_GAMES);
    }
}
