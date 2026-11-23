//! The replay snapshot contract v2 (package `ReplaySnapshot` and the shape
//! `BuildReplaySnapshot` writes): one match, every game in it, as shared from
//! the desktop app and served by mymtgo.com.
//!
//! Frame content stays a JSON value so every key MTGO (or 0.x's dense
//! recorder) wrote reaches the viewer untouched.

use serde::{Deserialize, Serialize};

/// The snapshot version this crate writes and accepts (package `VERSION`).
pub const VERSION: u32 = 2;

/// The name every redacted player is shown under (package `REDACTED_NAME`).
pub const REDACTED_NAME: &str = "Opponent";

/// One shared match.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub version: u32,
    pub meta: Meta,
    pub games: Vec<SnapshotGame>,
}

/// The head metadata the website shows.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Meta {
    pub format: Option<String>,
    pub played_at: String,
    pub games_in_match: u32,
    pub local_archetype: Option<String>,
    pub opponent_archetype: Option<String>,
}

/// One game with frames. `sideboard` is left out of the JSON when `None`,
/// since an empty list means it went unrecorded.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SnapshotGame {
    pub game_number: u32,
    pub won: Option<bool>,
    pub frames: Vec<ReplayFrame>,
    pub log: Vec<LogEntry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sideboard: Option<Vec<SideboardEntry>>,
}

/// One game state: the frame's time text and its JSON content (`Players`,
/// `Cards` and whatever else was recorded).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReplayFrame {
    pub timestamp: String,
    pub content: serde_json::Value,
}

/// One game log line.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LogEntry {
    pub timestamp: String,
    pub message: String,
}

/// One card of your sideboard as the game began.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SideboardEntry {
    pub catalog_id: u32,
    pub quantity: u32,
    pub name: Option<String>,
    #[serde(rename = "type")]
    pub type_line: Option<String>,
    pub image: Option<String>,
}

/// What the card catalog knows about one catalog id.
///
/// `image` is the remote URL and `local_image` the stored file name (Plan S
/// `cards` rows). `other_image` is the other face's remote image; 0.x never
/// set it, so v1 callers pass `None`, but the builder honours it as the
/// package does.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CardInfo {
    pub name: Option<String>,
    pub type_line: Option<String>,
    pub image: Option<String>,
    pub local_image: Option<String>,
    pub other_image: Option<String>,
}

/// Where decorated frames are shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageMode {
    /// The app's own replay window: the remote image as stored plus
    /// `local_image`, which the window turns into a `card` scheme URL.
    App,
    /// Frames that leave this machine: remote `https://` images only.
    Portable,
}
