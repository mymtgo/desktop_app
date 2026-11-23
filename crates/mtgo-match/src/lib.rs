//! Match projection: folds prepared MTGO events into match and game views.

pub mod derive;
pub mod facts;
pub mod games;
pub mod model;
pub mod projector;

pub use derive::{ABANDON_AFTER_MS, Derived, derive};
pub use model::{EndReason, GameResult, GameView, MatchState, MatchView, Outcome, Resolution};
pub use projector::{Footprint, Lookup, Memory, Projector, RETIRE_AFTER_MS, panic_message};

/// Bump whenever projection output can change, so stored matches get rebuilt.
/// Every fix for a derive panic bumps this, so stored failures are retried.
/// 2: quiet ended and decided matches complete (issue #26).
/// 3: the earliest settlement decides a match (issue #86).
/// 4: matches carry their league token and game structure (Plan I).
/// 5: a player who leaves at a level score loses the match.
pub const PROJECTION_VERSION: u32 = 5;
