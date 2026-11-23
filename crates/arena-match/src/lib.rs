//! Pure projection of decoded MTG Arena events into the app's match views
//! (`mtgo_match::MatchView`, so the store's match write path is reused), the
//! deck each match used and rank observations. No IO and no clock access:
//! every time comes in with the events.
//!
//! The projector is a fold over events in log order (file id, byte offset,
//! index within the entry), so applying events one at a time and folding
//! them all at once give the same result (live equals rebuild).

pub mod projector;
pub mod rank;
pub mod rank_band;
pub mod rank_line;

pub use projector::{
    ArenaChanges, ArenaIdentity, ArenaInput, ArenaProjector, DeckUse, opponent_key,
};
pub use rank::{Ladder, RankPoint};
pub use rank_band::{BandMatch, BandReading, RankBand, match_bands};
