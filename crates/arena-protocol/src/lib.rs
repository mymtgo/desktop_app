//! Pure parsing of MTG Arena's `Player.log`: splitting the log into entries,
//! decoding the entries the tracker reads, header clocks and event names.
//! No IO and no clock access.

pub mod clock;
pub mod decode;
pub mod event_name;
pub mod game_message;
pub mod split;

pub use clock::{HeaderClock, detect_day_first, dotnet_ticks_to_ms, header_time};
pub use decode::{
    ArenaEvent, CardCount, Course, RankReading, ResultEntry, Scope, Seat, decode_all,
};
pub use event_name::{EventName, commander_format_code, parse_event_name};
pub use game_message::{
    GameMessage, INTERMISSION_TYPE, MessageSource, game_messages, is_intermission_json,
};
pub use split::{ArenaSplitter, MAX_ENTRY_BYTES, RawEntry, split_complete};
