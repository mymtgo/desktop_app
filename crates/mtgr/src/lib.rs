//! Pure `.mtgr` 0.3.0 replay files (the mtgreplay standard).
//!
//! `model` holds the typed pieces of a file: a [`Record`] of [`Game`]s, each
//! with tags, body [`Line`]s and a result. `write` turns a record into the
//! canonical text. The writer owns everything canonical (tag order, `M:SS`
//! times, indentation, string quoting, LF endings), and statements are typed
//! variants, so a caller cannot produce a malformed line.
//!
//! The crate never touches the filesystem, network, database or clock. It does
//! not know where a game came from: callers convert their own data into the
//! model. New statement kinds are added as variants of [`Statement`], with a
//! matching arm in `write::statement_text`.

mod header;
mod model;
mod read;
mod write;

pub use header::{or_unknown, result_text, tag, utc_date};
pub use model::{
    AbilityKind, CardType, Cause, Choice, DeckCard, Entering, ExtArg, Game, GameResult, LibraryEnd,
    LibraryPlace, Line, Mana, MoveAction, ObjRef, Position, PtValue, Record, SpecialAction,
    Statement, Step, Tag, TargetRef, Zone,
};
pub use read::{HeaderError, HeaderGame, RecordingHeader, read_header};
pub use write::write;
