//! MTGO's `mtgo_game_history` files: a .NET BinaryFormatter (MS-NRBF) stream
//! holding a `List<HistoricalItem>` of past matches. Ported from 0.x
//! `ParseGameHistory`.
//!
//! [`binary`] reads the record grammar into objects keyed by id; [`records`]
//! maps the list's `HistoricalMatch` items to [`HistoryRecord`]s as 0.x's
//! `cleanMatch` did. Pure: callers pass the file bytes and the timezone the
//! file was written in. The input is hostile: every read is bounds-checked
//! and corrupt input returns a [`HistoryError`], never a panic.

pub mod binary;
pub mod records;
pub mod verdict;

pub use records::{HistoryRecord, merge_records, parse_history};
pub use verdict::{Verdict, local_verdict};

use std::fmt;

/// Why a history file could not be read. `at` is the byte offset where the
/// problem was found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HistoryError {
    /// The file has no bytes.
    Empty,
    /// The first record is not a serialization header.
    BadHeader,
    /// The file ends inside a record, or before the message end record.
    Truncated { at: usize },
    /// A record type this reader does not know.
    UnknownRecord { record_type: u8, at: usize },
    /// A primitive type this reader does not know.
    UnknownPrimitive { primitive_type: u8, at: usize },
    /// A `BinaryArray` type outside the format's six kinds.
    UnknownArrayType { array_type: u8, at: usize },
    /// A member type outside the format's eight kinds.
    UnknownMemberType { member_type: u8, at: usize },
    /// A `ClassWithId` record names a class no earlier record defined.
    UnknownClass { metadata_id: i32, at: usize },
    /// A negative count, or a count larger than the bytes left can hold.
    BadLength { at: usize },
    /// A null run where no array element is expected, or one that overruns
    /// its array.
    BadNullRun { at: usize },
    /// Records nested deeper than [`binary::MAX_DEPTH`].
    TooDeep { at: usize },
    /// The header's root object never appeared.
    MissingRoot,
}

impl fmt::Display for HistoryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HistoryError::Empty => write!(f, "the game history file is empty"),
            HistoryError::BadHeader => {
                write!(f, "the game history file has no serialization header")
            }
            HistoryError::Truncated { at } => write!(f, "the game history ends early at byte {at}"),
            HistoryError::UnknownRecord { record_type, at } => {
                write!(f, "unknown record type {record_type} at byte {at}")
            }
            HistoryError::UnknownPrimitive { primitive_type, at } => {
                write!(f, "unknown primitive type {primitive_type} at byte {at}")
            }
            HistoryError::UnknownArrayType { array_type, at } => {
                write!(f, "unknown array type {array_type} at byte {at}")
            }
            HistoryError::UnknownMemberType { member_type, at } => {
                write!(f, "unknown member type {member_type} at byte {at}")
            }
            HistoryError::UnknownClass { metadata_id, at } => {
                write!(f, "unknown class {metadata_id} at byte {at}")
            }
            HistoryError::BadLength { at } => write!(f, "bad length at byte {at}"),
            HistoryError::BadNullRun { at } => write!(f, "bad null run at byte {at}"),
            HistoryError::TooDeep { at } => write!(f, "records nest too deep at byte {at}"),
            HistoryError::MissingRoot => write!(f, "the game history has no root object"),
        }
    }
}

impl std::error::Error for HistoryError {}
