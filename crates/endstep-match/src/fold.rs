//! Folding one match: one pass over the sorted frames of its units into a
//! small record, then the shared `MatchView` from that record.

use std::collections::BTreeSet;

use endstep_protocol::{
    DeckBody, FrameKind, GameEvent, GameState, LobbyBody, MatchBody, RankedResult, Unit,
};
use mtgo_match::{MatchState, MatchView, Outcome, Resolution};

use crate::cards::{GameDecks, game_card_rows};
use crate::deck::{deck_from_body, deck_from_names};
use crate::games::{
    GameRecord, KeptFrame, KeptKind, counted_score, game_limit, game_mut, game_views,
};
use crate::log::log_line;
use crate::sideboard::{GameDeck, decks_by_game};
use crate::{
    DropReason, EndstepDeck, EndstepGameCards, EndstepMatch, Folded, MAX_GAMES, Person, QUIET_MS,
    account_key,
};

/// What one pass over the frames collects. Lobby, deck and ranked bodies are
/// kept whole and matched to the match body after the pass, because they can
/// arrive before it.
#[derive(Default)]
struct Pass {
    body: Option<MatchBody>,
    lobbies: Vec<LobbyBody>,
    decks: Vec<DeckBody>,
    /// How many deck bodies were read before this game's first match body:
    /// the deck the match was played with is among them, or else the first
    /// one read after it (a deck edited later never rewrites the match).
    decks_at_body: Option<usize>,
    ranked: Vec<RankedResult>,
    /// The game frames read, by kind, envelope `seq` and `sequenceNumber`.
    seen_frames: BTreeSet<FrameKey>,
    first_at: Option<i64>,
    last_at: Option<i64>,
    current_game: Option<u32>,
    games: Vec<GameRecord>,
    /// The highest `matchScore.gamesPlayed` seen: games 1 to this many were
    /// reached even when their frames were lost.
    games_played: u32,
    wins_by_seat: Vec<u32>,
    match_over_at: Option<i64>,
    /// Keep each game's frames in its record (for the replay).
    keep_frames: bool,
}

/// What makes a game frame a repeat: its kind, its envelope `seq` and an
/// event's `sequenceNumber`. A refresh can send a frame again; two frames
/// that differ in any of these are both read (trap 54).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct FrameKey {
    kind: KeyKind,
    seq: u64,
    sequence_number: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum KeyKind {
    State,
    Delta,
    Event,
}

/// A match the user is seated in, with what the walk read.
struct Seated {
    pass: Pass,
    body: MatchBody,
    seat: u32,
    account: Person,
    opponent: Option<Person>,
}

/// One game of a match for the replay: its record with the frames kept, and
/// the deck it was played with.
pub(crate) struct ReplayGame {
    pub record: GameRecord,
    pub deck: Option<GameDeck>,
}

/// What the replay is written from: the same walk, drop rules, game
/// numbering and decks as `fold_match`.
pub(crate) struct ReplaySource {
    pub body: MatchBody,
    /// The user's seat.
    pub seat: u32,
    pub started_at_ms: i64,
    /// The games of the match view that have a record, in game order.
    pub games: Vec<ReplayGame>,
}

/// Folds the units of the Forge game `forge_game_id` into one match. Units
/// may come in any order: they are sorted by `(started_at, counter)` here.
pub fn fold_match(forge_game_id: &str, units: &[Unit], now_ms: i64) -> Folded {
    let seated = match seated(forge_game_id, units, false) {
        Ok(seated) => seated,
        Err(reason) => return Folded::Dropped(reason),
    };
    let Seated {
        pass,
        body,
        seat,
        account,
        opponent,
    } = seated;
    let view = match_view(&pass, &body, seat, &account, opponent.as_ref(), now_ms);
    let deck = find_deck(&pass, &body, &account);
    let ranked = find_ranked(&pass, &body);
    let game_cards = game_cards(&pass, &view, seat, deck.as_ref());
    Folded::Match(Box::new(EndstepMatch {
        view,
        account,
        opponent,
        deck,
        ranked,
        game_cards,
        forge_game_id: forge_game_id.to_string(),
    }))
}

/// The walk and the match's people, or why the match is dropped.
fn seated(forge_game_id: &str, units: &[Unit], keep_frames: bool) -> Result<Seated, DropReason> {
    let mut pass = read_frames(forge_game_id, units, keep_frames);
    let Some(body) = pass.body.clone() else {
        return Err(DropReason::NoMatchBody);
    };
    if body.participants.iter().any(|person| person.bot) {
        return Err(DropReason::Bot);
    }
    let Some(seat) = body.my_player_index else {
        return Err(DropReason::Spectator);
    };
    let Some(user) = body.participants.get(seat as usize) else {
        return Err(DropReason::Spectator);
    };
    let account = Person {
        user_id: user.user_id.clone(),
        username: user.username.clone(),
    };
    let opponent = body
        .participants
        .iter()
        .find(|person| person.user_id != account.user_id)
        .map(|person| Person {
            user_id: person.user_id.clone(),
            username: person.username.clone(),
        });
    for game in &mut pass.games {
        game.sideboard.keep_match(&body.id);
    }
    Ok(Seated {
        pass,
        body,
        seat,
        account,
        opponent,
    })
}

/// The match for the replay, `None` when `fold_match` would drop it.
pub(crate) fn replay_source(forge_game_id: &str, units: &[Unit]) -> Option<ReplaySource> {
    let Seated {
        pass,
        body,
        seat,
        account,
        ..
    } = seated(forge_game_id, units, true).ok()?;
    let limit = game_limit(body.games_per_match);
    let views = game_views(&pass.games, pass.games_played.min(limit), limit, seat);
    let deck = find_deck(&pass, &body, &account);
    let decks = decks_by_game(&pass.games, deck.as_ref());
    let started_at_ms = body.started_at_ms.or(pass.first_at).unwrap_or(0);
    // The records are moved out of the walk: each holds its game's frames.
    let mut records = pass.games;
    let mut games = Vec::new();
    for view in &views {
        let found = records
            .iter()
            .position(|record| record.number == view.game_number);
        let Some(index) = found else {
            continue;
        };
        let deck = decks
            .iter()
            .find(|(number, _)| *number == view.game_number)
            .and_then(|(_, deck)| deck.clone());
        games.push(ReplayGame {
            record: records.remove(index),
            deck,
        });
    }
    Some(ReplaySource {
        started_at_ms,
        body,
        seat,
        games,
    })
}

/// Every game record of the Forge game `forge_game_id`, from the same walk
/// `fold_match` reads (for the game log).
pub(crate) fn read_games(forge_game_id: &str, units: &[Unit]) -> Vec<GameRecord> {
    read_frames(forge_game_id, units, false).games
}

/// The card rows of each game in the view; a game known only by number has
/// none.
fn game_cards(
    pass: &Pass,
    view: &MatchView,
    seat: u32,
    deck: Option<&EndstepDeck>,
) -> Vec<EndstepGameCards> {
    let decks = decks_by_game(&pass.games, deck);
    // Game 1 has no record when its frames were lost: the match deck is then
    // the deck to measure the sideboarding against.
    let game_one = decks
        .iter()
        .find(|(number, _)| *number == 1)
        .and_then(|(_, deck)| deck.clone())
        .or_else(|| deck.map(GameDeck::from_match));
    let mut out = Vec::new();
    for game in &view.games {
        let record = pass
            .games
            .iter()
            .find(|record| record.number == game.game_number);
        let Some(record) = record else {
            out.push(EndstepGameCards {
                game_number: game.game_number,
                rows: Vec::new(),
                opening_hand_size: None,
                starting_hand_size: None,
                opp_opening_hand_size: None,
                opp_starting_hand_size: None,
                mulligans: Vec::new(),
            });
            continue;
        };
        let hands = record.hands.hands_of(seat);
        let game_deck = decks
            .iter()
            .find(|(number, _)| *number == game.game_number)
            .and_then(|(_, played)| played.as_ref());
        let against = if game.game_number == 1 {
            None
        } else {
            game_one.as_ref()
        };
        let decks_used = GameDecks {
            played: game_deck,
            game_one: against,
        };
        let built = game_card_rows(&record.cards, seat, deck, decks_used, &hands);
        out.push(EndstepGameCards {
            game_number: game.game_number,
            rows: built.rows,
            opening_hand_size: built.opening_hand_size,
            starting_hand_size: built.starting_hand_size,
            opp_opening_hand_size: hands.opp_opening_size,
            opp_starting_hand_size: hands.opp_starting_size,
            mulligans: built.mulligans,
        });
    }
    out
}

/// The one pass: units by `(started_at, counter)` (then unit id, so equal
/// headers still sort the same way), frames in line order.
fn read_frames(forge_game_id: &str, units: &[Unit], keep_frames: bool) -> Pass {
    let mut ordered: Vec<&Unit> = units.iter().collect();
    ordered.sort_by(|a, b| {
        (a.started_at, a.counter, &a.unit_id).cmp(&(b.started_at, b.counter, &b.unit_id))
    });

    let mut pass = Pass {
        keep_frames,
        ..Pass::default()
    };
    for unit in ordered {
        // A delta names no game: it belongs to the game of the last state or
        // event on the same socket, so only deltas after one of ours count.
        // Deltas before the first state or event of this game in a unit are
        // ignored by design: nothing ties them to this game.
        let mut in_our_game = false;
        for frame in &unit.frames {
            match &frame.kind {
                FrameKind::MatchBody(body) => {
                    if body.forge_game_id == forge_game_id {
                        // The page fetches the body again (a reload, another
                        // tab): only the first one is activity.
                        if pass.body.is_none() {
                            pass.touch(frame.at);
                        }
                        if pass.decks_at_body.is_none() {
                            pass.decks_at_body = Some(pass.decks.len());
                        }
                        pass.body = Some(body.clone());
                    }
                }
                FrameKind::LobbyBody(lobby) => pass.lobbies.push(lobby.clone()),
                FrameKind::DeckBody(deck) => pass.decks.push(deck.clone()),
                FrameKind::RankedResult(result) => pass.ranked.push(result.clone()),
                FrameKind::GameState(state) | FrameKind::GameOver(state) => {
                    in_our_game = state.game_id == forge_game_id;
                    if in_our_game && pass.first_time(KeyKind::State, state.seq, None) {
                        pass.read_state(state, frame.at);
                        // The lobby's `GAME_OVER` repeats the last state
                        // after the outcome: the replay has no use for it.
                        if matches!(frame.kind, FrameKind::GameState(_)) {
                            pass.keep(frame.at, || KeptKind::State(state.clone()));
                        }
                    }
                }
                FrameKind::GameEvent(event) => {
                    in_our_game = event.game_id == forge_game_id;
                    if in_our_game
                        && pass.first_time(KeyKind::Event, event.seq, event.sequence_number)
                    {
                        pass.read_event(event, frame.at);
                        pass.keep(frame.at, || KeptKind::Event(event.clone()));
                    }
                }
                FrameKind::GameDelta(delta) => {
                    if in_our_game && pass.first_time(KeyKind::Delta, delta.seq, None) {
                        pass.touch(frame.at);
                        if let Some((number, wins, over)) = &delta.match_score {
                            pass.read_score(*number, wins, *over, frame.at);
                        }
                        if let Some(prompt) = &delta.prompt {
                            let game = pass.current_game_mut(frame.at);
                            game.sideboard.read_prompt(prompt);
                        }
                        pass.keep(frame.at, || KeptKind::Delta(delta.clone()));
                    }
                }
                FrameKind::Action(action) => {
                    // An action names no game either: only one after a state
                    // or event of ours counts, and it belongs to the current
                    // game.
                    if in_our_game {
                        pass.current_game_mut(frame.at)
                            .sideboard
                            .read_action(action);
                    }
                }
                FrameKind::Other => {}
            }
        }
    }
    pass
}

impl Pass {
    /// False when a frame of this kind, envelope `seq` and `sequenceNumber`
    /// was already read (a refresh can repeat a frame). Frames without a
    /// `seq` are always read.
    fn first_time(
        &mut self,
        kind: KeyKind,
        seq: Option<u64>,
        sequence_number: Option<u64>,
    ) -> bool {
        match seq {
            Some(seq) => self.seen_frames.insert(FrameKey {
                kind,
                seq,
                sequence_number,
            }),
            None => true,
        }
    }

    /// Notes a frame of this match for the start and last activity times.
    fn touch(&mut self, at_ms: i64) {
        if self.first_at.is_none() {
            self.first_at = Some(at_ms);
        }
        self.last_at = Some(at_ms);
    }

    /// Keeps a frame in the current game's record when the walk keeps
    /// frames. Called after the frame was read, so the game is the one the
    /// stored rows give it to.
    fn keep(&mut self, at_ms: i64, kind: impl FnOnce() -> KeptKind) {
        if self.keep_frames {
            let frame = KeptFrame {
                at_ms,
                kind: kind(),
            };
            self.current_game_mut(at_ms).frames.push(frame);
        }
    }

    fn current_game_mut(&mut self, at_ms: i64) -> &mut GameRecord {
        let number = self.current_game.unwrap_or(1);
        game_mut(&mut self.games, number, at_ms)
    }

    /// Moves to `number`, keeps the score and notes when the match ended.
    /// Games count from 1 to `MAX_GAMES`: any other game number is hostile
    /// input and is ignored. The match's own limit (`gamesPerMatch`) is
    /// applied after the pass, since its body can come after the games.
    fn read_score(&mut self, number: u32, wins_by_seat: &[u32], over: bool, at_ms: i64) {
        if (1..=MAX_GAMES).contains(&number) {
            self.current_game = Some(number);
            game_mut(&mut self.games, number, at_ms);
        }
        if !wins_by_seat.is_empty() {
            self.wins_by_seat = wins_by_seat.to_vec();
        }
        if over && self.match_over_at.is_none() {
            self.match_over_at = Some(at_ms);
        }
    }

    fn read_state(&mut self, state: &GameState, at_ms: i64) {
        self.touch(at_ms);
        if let Some(played) = state.games_played {
            self.games_played = self.games_played.max(played.min(MAX_GAMES));
        }
        if let Some(number) = state.game_number {
            self.read_score(number, &state.wins_by_seat, state.is_match_over, at_ms);
        } else if state.is_match_over && self.match_over_at.is_none() {
            self.match_over_at = Some(at_ms);
        }
        let game = self.current_game_mut(at_ms);
        if state.turn_number == Some(1) && game.play_seat.is_none() {
            game.play_seat = state.active_player;
        }
        game.cards.read_state(state);
        game.hands.read_state(state);
        if let Some(prompt) = &state.prompt {
            game.sideboard.read_prompt(prompt);
        }
    }

    fn read_event(&mut self, event: &GameEvent, at_ms: i64) {
        self.touch(at_ms);
        let game = self.current_game_mut(at_ms);
        game.cards.read_event(event);
        game.hands.read_event(event);
        if let Some(line) = log_line(event, at_ms) {
            game.log.push(line);
        }
        match event.kind.as_str() {
            // A game is decided once: a later outcome for the same game number
            // (a repeat Forge sent with new numbers, or hostile input) never
            // rewrites it, nor starts a game the score never named.
            "GAME_OUTCOME" if !game.decided => {
                game.decided = true;
                game.winner_seat = event.player_index;
                game.end_reason = event.end_reason.clone();
                game.turn_count = event.turn_number;
                game.ended_at_ms = Some(at_ms);
            }
            "MULLIGAN" => {
                // Endstep sends one without `amount` when the choice opens;
                // only the one with the new hand size is a mulligan taken.
                if let (Some(_), Some(seat)) = (event.amount, event.player_index) {
                    game.mulligan_seats.push(seat);
                }
            }
            _ => {}
        }
    }
}

fn match_view(
    pass: &Pass,
    body: &MatchBody,
    seat: u32,
    account: &Person,
    opponent: Option<&Person>,
    now_ms: i64,
) -> MatchView {
    let last_activity_ms = pass.last_at.unwrap_or(0);
    let started_at_ms = body.started_at_ms.or(pass.first_at).unwrap_or(0);
    let limit = game_limit(body.games_per_match);
    let (wins, losses) = score(pass, limit, seat);

    let (state, resolution, outcome, ended_at_ms) = match pass.match_over_at {
        Some(over_at) => (
            MatchState::Complete,
            Some(Resolution::Signal),
            outcome_by_wins(wins, losses),
            Some(over_at),
        ),
        None if now_ms.saturating_sub(last_activity_ms) < QUIET_MS => {
            (MatchState::InProgress, None, Outcome::Unknown, None)
        }
        None => (
            MatchState::Abandoned,
            Some(Resolution::Abandoned),
            Outcome::Unknown,
            Some(last_activity_ms),
        ),
    };
    let tentative_outcome = if state == MatchState::InProgress {
        tentative(body.games_per_match, wins, losses)
    } else {
        None
    };

    // Never more games than the match can have.
    let games = game_views(&pass.games, pass.games_played.min(limit), limit, seat);

    MatchView {
        token: body.id.clone(),
        mtgo_match_id: None,
        local_username: account_key(&account.user_id),
        opponent: opponent.map(|person| account_key(&person.user_id)),
        format: body.format_id.clone(),
        match_type: Some(if body.ranked { "ranked" } else { "unranked" }.to_string()),
        tournament_event_id: None,
        tournament_round: None,
        league_token: None,
        game_structure: game_structure(body.games_per_match),
        state,
        resolution,
        outcome,
        tentative_outcome,
        wins,
        losses,
        started_at_ms,
        ended_at_ms,
        last_activity_ms,
        games,
    }
}

/// The last `winsBySeat` (the user's seat against the others), or the
/// decided games counted when no score names the user's seat or the score
/// has more games than the match can have (hostile input: a Bo1 whose score
/// says three games).
fn score(pass: &Pass, limit: u32, seat: u32) -> (u32, u32) {
    let Some(wins) = pass.wins_by_seat.get(seat as usize) else {
        return counted_score(&pass.games, limit, seat);
    };
    let mut losses = 0;
    for (index, seat_wins) in pass.wins_by_seat.iter().enumerate() {
        if index != seat as usize {
            losses = u32::saturating_add(losses, *seat_wins);
        }
    }
    if wins.saturating_add(losses) > limit {
        return counted_score(&pass.games, limit, seat);
    }
    (*wins, losses)
}

fn outcome_by_wins(wins: u32, losses: u32) -> Outcome {
    if wins > losses {
        Outcome::Win
    } else if losses > wins {
        Outcome::Loss
    } else {
        Outcome::Draw
    }
}

/// The live hint once a side has won `ceil(games_per_match / 2)` games.
fn tentative(games_per_match: Option<u32>, wins: u32, losses: u32) -> Option<Outcome> {
    let games_per_match = games_per_match.filter(|games| *games > 0)?;
    let needed = games_per_match.div_ceil(2);
    if wins >= needed {
        Some(Outcome::Win)
    } else if losses >= needed {
        Some(Outcome::Loss)
    } else {
        None
    }
}

fn game_structure(games_per_match: Option<u32>) -> Option<String> {
    match games_per_match {
        Some(1) => Some("bo1".to_string()),
        Some(3) => Some("bo3".to_string()),
        _ => None,
    }
}

/// The deck body of the user's seat in this match's lobby, else the deck
/// from the match body's card names (none when it lists no cards).
fn find_deck(pass: &Pass, body: &MatchBody, account: &Person) -> Option<EndstepDeck> {
    let lobby = pass
        .lobbies
        .iter()
        .rev()
        .find(|lobby| body.lobby_id.as_deref() == Some(lobby.id.as_str()));
    let deck_id = lobby.and_then(|lobby| {
        lobby
            .seat_decks
            .iter()
            .find(|(user_id, _)| *user_id == account.user_id)
            .map(|(_, deck_id)| deck_id.clone())
    });
    let deck = deck_id.and_then(|deck_id| deck_at_match_time(pass, &deck_id));
    match deck {
        Some(deck) => Some(deck_from_body(deck, body)),
        None if body.my_deck.is_empty() && body.my_sideboard.is_empty() => None,
        None => Some(deck_from_names(body)),
    }
}

/// The deck body `deck_id` as the match was played: the last one read
/// before the match body, else the first one read after it.
fn deck_at_match_time<'a>(pass: &'a Pass, deck_id: &str) -> Option<&'a DeckBody> {
    let split = pass.decks_at_body.unwrap_or(pass.decks.len());
    let (before, after) = pass.decks.split_at(split);
    before
        .iter()
        .rev()
        .find(|deck| deck.id == deck_id)
        .or_else(|| after.iter().find(|deck| deck.id == deck_id))
}

/// The last ranked result for this match's db id.
fn find_ranked(pass: &Pass, body: &MatchBody) -> Option<RankedResult> {
    let db_id = body.db_id.as_deref()?;
    pass.ranked
        .iter()
        .rev()
        .find(|result| result.db_id == db_id)
        .cloned()
}
