//! Reading MTGO's log files from disk: finding them, telling whether a file is
//! still the same file, and tailing new bytes into whole events. All MTGO
//! knowledge about the bytes themselves lives in `mtgo-protocol`.

pub mod deck_files;
pub mod discover;
pub mod game_logs;
pub mod hash;
pub mod history_files;
pub mod identity;
pub mod tail;
mod walk;

pub use discover::{
    Detection, Env, MAC_DEV_FOLDER, MAX_DEPTH, Platform, candidate_roots, detect, find_logs,
    prefix_of, wine_user_homes,
};
pub use game_logs::{find_game_log, game_log_token, list_game_logs};
pub use hash::fnv1a64;
pub use history_files::find_history_files;
pub use tail::{LIVE_WINDOW_MS, MAX_READ, QUIET_COMMIT_MS, TailRead, Tailer};
