//! Reading ahead: the events and the full state after the frame being
//! read, once per frame.

use std::cell::OnceCell;

use ::mtgr::Zone;
use endstep_protocol::{GameEvent, GameState};

use super::super::GameWriter;
use super::super::objects::Permanent;
use super::super::shown::shown_cards;
use crate::fold::ReplayGame;
use crate::games::KeptKind;

/// The most frames an event looks ahead through for the next full state,
/// and the most events it reads on the way (hostile input: a capture with
/// no full states must not make every event scan the whole game).
const LOOK_AHEAD: usize = 1_000;

/// What the frames after the one being read show: read on first use, and
/// dropped when the next frame is read.
#[derive(Debug, Default)]
pub(in crate::mtgr) struct Ahead<'a> {
    events: OnceCell<Vec<&'a GameEvent>>,
    state: OnceCell<Option<&'a GameState>>,
}

impl<'a> GameWriter<'a> {
    /// The events after the frame being read, up to the next full state or
    /// the next turn. Read once per frame, then kept.
    pub(in crate::mtgr) fn events_ahead(&self) -> &[&'a GameEvent] {
        self.ahead.events.get_or_init(|| {
            let game: &'a ReplayGame = self.game;
            let mut events = Vec::new();
            let frames = game.record.frames.iter().skip(self.index + 1);
            for frame in frames.take(LOOK_AHEAD) {
                match &frame.kind {
                    KeptKind::State(state) if !state.sideboarding => break,
                    KeptKind::Event(event) if event.kind == "TURN_BEGAN" => break,
                    KeptKind::Event(event) => events.push(event),
                    _ => {}
                }
            }
            events
        })
    }

    /// The next full state after the frame being read. Read once per frame,
    /// then kept.
    pub(in crate::mtgr) fn next_state(&self) -> Option<&'a GameState> {
        *self.ahead.state.get_or_init(|| {
            let game: &'a ReplayGame = self.game;
            game.record
                .frames
                .iter()
                .skip(self.index + 1)
                .take(LOOK_AHEAD)
                .find_map(|frame| match &frame.kind {
                    KeptKind::State(state) if !state.sideboarding => Some(state),
                    _ => None,
                })
        })
    }

    /// What the permanent `forge_id` has on the battlefield in the next full
    /// state.
    pub(super) fn permanent_ahead(&self, forge_id: u64) -> Option<Permanent> {
        shown_cards(self.next_state()?)
            .into_iter()
            .find(|card| card.forge_id == forge_id && card.zone == Zone::Battlefield)
            .map(|card| card.permanent)
    }

    /// Whether a `LAND_PLAYED` for this card follows.
    pub(super) fn is_land_play(&self, forge_id: u64) -> bool {
        self.events_ahead()
            .iter()
            .any(|event| event.kind == "LAND_PLAYED" && event.card_id == Some(forge_id))
    }

    /// Whether a reveal into a hand that names this card follows.
    pub(super) fn revealed_ahead(&self, forge_id: u64) -> bool {
        self.events_ahead().iter().any(|event| {
            event.kind == "CARD_REVEALED"
                && event.to_zone.as_deref() == Some("Hand")
                && event.card_ids.contains(&forge_id)
        })
    }
}
