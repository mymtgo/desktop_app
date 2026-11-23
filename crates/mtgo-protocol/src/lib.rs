//! Pure parsing of MTGO's `mtgo.log`: event splitting, classification, MetaMessage
//! decoding and timestamp reconstruction. No IO and no clock access.

pub mod anonymise;
pub mod classify;
pub mod entries;
pub mod game_management;
pub mod game_state;
pub mod header;
pub mod json_repair;
pub mod login;
pub mod prepare;
pub mod splitter;
pub mod state_change;
pub mod time;
pub mod tournament;

pub use classify::{Classified, EventKind, Subjects, classify};
pub use entries::{GameEntry, decode_meta_message, parse_entry};
pub use game_management::{GameManagement, parse_game_management, parse_kv_block};
pub use game_state::parse_players;
pub use header::{Header, header_time, is_header_line, parse_header};
pub use login::{Login, deck_used_username, parse_login};
pub use prepare::{FileContext, PreparedEvent, prepare_at};
pub use splitter::{EventSplitter, RawEvent, split_complete};
pub use state_change::{StateChange, parse_state_change};
pub use time::{ClockState, DayClock, day_anchor_from_last_write, local_to_utc_ms};
