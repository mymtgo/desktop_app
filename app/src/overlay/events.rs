//! Events the overlay worker pushes to one window each (overlay spec 6 step
//! 3, `emit_to`, never to every window). Each is sent only when its view
//! changed.

use serde::Serialize;
use tauri_specta::Event;

use crate::limited::DraftNotesView;
use crate::overlay::game::GameOverlayView;

/// The game overlay's view changed. `revision` grows with every push, so a
/// page applies only views newer than the one it holds.
#[derive(Debug, Clone, PartialEq, Serialize, specta::Type, Event)]
#[serde(rename_all = "camelCase")]
pub struct GameOverlayChanged {
    pub revision: u32,
    pub view: GameOverlayView,
}

/// The draft notes window's data changed (a pick, a note, the draft's
/// state); `None` when no draft is live.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type, Event)]
#[serde(rename_all = "camelCase")]
pub struct DraftNotesChanged {
    pub view: Option<DraftNotesView>,
}

/// The league card changed (overlay spec 6 step 3): pushed to the league
/// window and to the main window (the Overlays settings preview's real
/// deck).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type, Event)]
#[serde(rename_all = "camelCase")]
pub struct LeagueOverlayChanged {
    pub state: overlay::LeagueOverlayState,
}

/// The hosted overlay's publish state changed (overlay spec 10): pushed to
/// the main window for the Overlays settings card, instead of 0.x's 15 s
/// poll. Both `None` once publishing is switched off.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, specta::Type, Event)]
#[serde(rename_all = "camelCase")]
pub struct OverlayPublishChanged {
    /// When the last push was accepted (UTC ms).
    #[specta(type = Option<specta_typescript::Number>)]
    pub last_published_at_ms: Option<i64>,
    /// Why the last push failed; cleared by the next accepted one.
    pub error: Option<overlay::PublishError>,
}
