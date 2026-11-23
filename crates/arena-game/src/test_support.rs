//! Test helpers: one game's messages out of an Arena fixture, assigned to
//! matches and games the way the tracker does.

use std::collections::BTreeMap;

use arena_protocol::{
    ArenaEvent, GameMessage, MessageSource, decode_all, game_messages, split_complete,
};
use fixture_tools::arena_fixture;

use crate::{GameInput, Source};

/// One game of a fixture's match.
pub(crate) struct FixtureGame {
    pub inputs: Vec<GameInput>,
    /// The local player's seat in the match room (found by user id).
    pub room_local_seat: Option<u32>,
}

/// A message with the match and game the tracker would give it.
struct Assigned {
    match_id: Option<String>,
    game: Option<u32>,
    input: GameInput,
}

/// The tracker's current match and game while walking a log.
#[derive(Default)]
struct Capture {
    current_match: Option<String>,
    current_game: Option<u32>,
    info_game: Option<u32>,
    /// Match ids in the order they were entered.
    matches: Vec<String>,
    assigned: Vec<Assigned>,
}

impl Capture {
    /// A new current match starts at game 1 and takes the messages read
    /// while no match was current, as game 1.
    fn enter_match(&mut self, id: &str) {
        if id.is_empty() || self.current_match.as_deref() == Some(id) {
            return;
        }
        self.current_match = Some(id.to_string());
        self.current_game = Some(1);
        self.info_game = None;
        if !self.matches.iter().any(|m| m == id) {
            self.matches.push(id.to_string());
        }
        for message in &mut self.assigned {
            if message.match_id.is_none() {
                message.match_id = Some(id.to_string());
                message.game = Some(1);
            }
        }
    }

    fn leave_match(&mut self, id: &str) {
        if self.current_match.as_deref() == Some(id) {
            self.current_match = None;
            self.current_game = None;
            self.info_game = None;
        }
    }

    fn capture(&mut self, message: GameMessage) {
        if let Some(id) = &message.match_id {
            self.enter_match(id);
        }
        if let Some(number) = message.game_number {
            self.current_game = Some(number);
            self.info_game = Some(number);
        }
        let intermission = message.is_intermission();
        self.assigned.push(Assigned {
            match_id: self.current_match.clone(),
            game: self.current_game,
            input: GameInput {
                source: match message.source {
                    MessageSource::Gre => Source::Gre,
                    MessageSource::Client => Source::Client,
                },
                logged_at: message.at_ms.unwrap_or(0),
                json: message.json,
            },
        });
        if intermission && let Some(ended) = self.info_game.or(self.current_game) {
            self.current_game = Some(ended + 1);
        }
    }
}

/// Game `game` of the first match in fixture `name`.
pub(crate) fn fixture_game(name: &str, game: u32) -> FixtureGame {
    fixture_match_game(name, 0, game)
}

/// Game `game` of match `match_index` (0 for the first) in fixture `name`.
pub(crate) fn fixture_match_game(name: &str, match_index: usize, game: u32) -> FixtureGame {
    let fixture = arena_fixture(name);
    let mut capture = Capture::default();
    let mut local_seats: BTreeMap<String, u32> = BTreeMap::new();
    for entry in &split_complete(&fixture.bytes) {
        for event in decode_all(entry) {
            if let ArenaEvent::RoomState {
                match_id,
                completed,
                players,
                ..
            } = event
            {
                for seat in &players {
                    if seat.user_id == fixture.meta.local_user_id {
                        local_seats.insert(match_id.clone(), seat.seat);
                    }
                }
                if completed {
                    capture.leave_match(&match_id);
                } else {
                    capture.enter_match(&match_id);
                }
            }
        }
        for message in game_messages(entry) {
            capture.capture(message);
        }
    }
    let match_id = capture
        .matches
        .get(match_index)
        .cloned()
        .expect("the fixture has that match");
    let inputs = capture
        .assigned
        .into_iter()
        .filter(|m| m.match_id.as_ref() == Some(&match_id) && m.game == Some(game))
        .map(|m| m.input)
        .collect();
    FixtureGame {
        inputs,
        room_local_seat: local_seats.get(&match_id).copied(),
    }
}
