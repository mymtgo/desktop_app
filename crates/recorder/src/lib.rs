//! Pure rules for the opt-in mtgr-record recorder: the pinned build, exit
//! codes, the crash window and its status file. No IO: callers pass in the
//! clock and file contents.

mod crashes;
mod exit;
mod pin;
mod status;

pub use crashes::CrashWindow;
pub use exit::{ExitKind, exit_kind, respawns};
pub use pin::{PIN, Pin, PinnedFile, download_blocked, file_url};
pub use status::{RecorderState, parse_status};
