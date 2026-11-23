//! Reading MTG Arena's `Player.log` from disk: where it is on each platform,
//! and tailing new bytes into whole entries. All knowledge about the bytes
//! themselves lives in `arena-protocol`; whether a file is still the same file
//! reuses `mtgo_log::identity`.

pub mod discover;
pub mod tail;

pub use discover::{PLAYER_LOG, PLAYER_PREV_LOG, arena_log_paths, prev_log_path};
pub use tail::{ArenaRead, ArenaTailer, DAY_ORDER_BYTES, MAX_READ, read_start};
