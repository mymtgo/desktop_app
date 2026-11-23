//! Rank observations: one point per ladder of each rank response, kept only
//! when it belongs to the current season (trap 5) and an account is logged
//! in to own it (trap 6).

use arena_protocol::RankReading;
use serde::Serialize;

/// Which rank a reading is about: constructed (every ranked constructed
/// queue) or limited.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Ladder {
    Constructed,
    Limited,
}

/// One ladder of a rank response, at the rank event's place in the log.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RankPoint {
    pub ladder: Ladder,
    pub reading: RankReading,
    pub at_ms: i64,
    pub file_id: u64,
    pub offset: u64,
    /// The event's index within its log entry.
    pub seq: u32,
}

/// Rank points waiting for what they need before they can be kept: a login
/// to own them and the current season to check them against.
#[derive(Debug, Clone, Default)]
pub(crate) struct RankGate {
    /// The newest season a season response named.
    season: Option<u32>,
    /// Points not yet kept or dropped, in log order.
    held: Vec<RankPoint>,
}

impl RankGate {
    /// Takes a rank response's points; returns every point that can now be
    /// kept.
    pub(crate) fn observe(&mut self, points: Vec<RankPoint>, logged_in: bool) -> Vec<RankPoint> {
        self.held.extend(points);
        self.release(logged_in)
    }

    /// Takes a season response; returns every held point that can now be
    /// kept.
    pub(crate) fn season(&mut self, ordinal: u32, logged_in: bool) -> Vec<RankPoint> {
        let newest = self.season.map_or(ordinal, |season| season.max(ordinal));
        self.season = Some(newest);
        self.release(logged_in)
    }

    /// Drops held points from an older season than the current one, and
    /// returns the current season's points once someone is logged in. A point
    /// from a newer season than any season response so far waits for one.
    pub(crate) fn release(&mut self, logged_in: bool) -> Vec<RankPoint> {
        let Some(current) = self.season else {
            return Vec::new();
        };
        let mut kept = Vec::new();
        let mut waiting = Vec::new();
        for point in self.held.drain(..) {
            if point.reading.season < current {
                continue;
            }
            if point.reading.season == current && logged_in {
                kept.push(point);
            } else {
                waiting.push(point);
            }
        }
        self.held = waiting;
        kept
    }
}
