//! Replays, ported from the `mymtgo/replay` package (at `1559239`) and the
//! 0.x replay actions (replays spec section 5).
//!
//! - `snapshot`: the snapshot contract v2 types and constants.
//! - `frames`: decorating frames for the app viewer (0.x `BuildReplayFrames`)
//!   and dropping repeated frames.
//! - `repair`: the midnight wrap and active player repairs.
//! - `build`: the snapshot builder (package `BuildReplaySnapshot`).
//! - `format`: the archetype format key of an MTGO format code.
//! - `link`: a share link opening on one game.
//! - `redact`: replacing an opponent's name (package `RedactPlayer`).
//! - `validate`: the snapshot rules the website applies (package
//!   `ValidateReplaySnapshot`).
//! - `share`: the prepared snapshot (redacted and validated, the only thing
//!   an upload is built from) and the share and switch off outcomes.
//!
//! Pure: no filesystem, clock, network or database. Callers pass in the
//! frames, cards and times read elsewhere.

pub mod build;
pub mod format;
pub mod frames;
pub mod link;
pub mod redact;
pub mod repair;
pub mod share;
pub mod snapshot;
pub mod validate;

pub use build::{GameInput, SnapshotInput, build_snapshot, iso8601_utc};
pub use format::format_key;
pub use frames::{FrameText, decorate_frames, drop_repeats, normalise_time};
pub use link::link_to_game;
pub use redact::{RedactError, assert_gone, redact_player};
pub use repair::{repair_active_player, repair_midnight_wrap};
pub use share::{
    PrepareFailure, PreparedSnapshot, RevokeFailure, ShareFailure, prepare_shared_snapshot,
};
pub use snapshot::{
    CardInfo, ImageMode, LogEntry, Meta, REDACTED_NAME, ReplayFrame, SideboardEntry, Snapshot,
    SnapshotGame, VERSION,
};
pub use validate::{ValidationErrors, validate_snapshot};
