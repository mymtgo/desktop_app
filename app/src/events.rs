//! Typed events pushed to every window (spec 6). TypeScript bindings are
//! generated from these (Task 7).

use serde::Serialize;
use tauri_specta::Event;

use crate::settings::Settings;
use crate::summary::{MatchSummary, StatusView};

/// The update status changed (checking, downloading, ready, failed).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type, Event)]
#[serde(rename_all = "camelCase")]
pub struct UpdateStatusChanged {
    pub status: crate::updates::UpdateStatus,
}

/// A match's committed result changed. Patch the row in place.
#[derive(Debug, Clone, PartialEq, Serialize, specta::Type, Event)]
#[serde(rename_all = "camelCase")]
pub struct MatchUpdated {
    pub summary: MatchSummary,
}

/// What a match will look like once the event being written commits. Any later
/// update or removal of the match supersedes it (issue #50).
#[derive(Debug, Clone, PartialEq, Serialize, specta::Type, Event)]
#[serde(rename_all = "camelCase")]
pub struct MatchPreviewed {
    pub summary: MatchSummary,
}

/// A visible match was discarded (it failed the phantom gate), or the player
/// removed it from stats.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type, Event)]
#[serde(rename_all = "camelCase")]
pub struct MatchRemoved {
    pub token: String,
}

/// Projection failed for a match; it is stored as needing attention.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type, Event)]
#[serde(rename_all = "camelCase")]
pub struct MatchAttention {
    pub token: String,
    pub reason: String,
}

/// What the tracker is doing changed (searched folders, warnings, paused or
/// running). Replace the cached status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type, Event)]
#[serde(rename_all = "camelCase")]
pub struct StatusChanged {
    pub status: StatusView,
}

/// The tracker hit an error and is restarting. Revisions may repeat after a
/// restart: drop cached pages and refetch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type, Event)]
#[serde(rename_all = "camelCase")]
pub struct TrackerRestarting {
    pub reason: String,
}

/// Settings were stored (by any window). Replace the cached settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type, Event)]
#[serde(rename_all = "camelCase")]
pub struct SettingsChanged {
    pub settings: Settings,
}

/// A 0.x import moved to its next phase. On `finished` or `failed`, refetch
/// match pages: the import added rows no other event announced (a failed
/// import may have committed earlier batches).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type, Event)]
#[serde(rename_all = "camelCase")]
pub struct ImportProgressChanged {
    pub progress: crate::legacy::ImportProgress,
}

/// The history scan moved on (a stage, progress, its end) or was cancelled
/// or forgotten (`None`). Replace the page's scan status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type, Event)]
#[serde(rename_all = "camelCase")]
pub struct HistoryScanChanged {
    pub status: Option<crate::history_import::scan::HistoryScanStatus>,
}

/// History imports were stored or deleted and are committed. Refetch the
/// match pages and the dashboard (the tracker tells the deck and league
/// pages itself), and show `imported_count` as the import page's count.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type, Event)]
#[serde(rename_all = "camelCase")]
pub struct HistoryImportsChanged {
    pub imported_count: u32,
}

/// Stored decks changed (a deck sync, an edit, an import or a new match
/// link) and are committed. Refetch the deck pages.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type, Event)]
#[serde(rename_all = "camelCase")]
pub struct DecksChanged {}

/// The current account may have changed and the change is committed (a
/// Login line, a switch, a tracking change; release spec 5). Refetch every
/// page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type, Event)]
#[serde(rename_all = "camelCase")]
pub struct AccountChanged {}

/// Arena rank readings were stored and committed (Arena spec 15). Refetch the
/// rank views: the dashboard's rank card and an Arena deck's climb.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type, Event)]
#[serde(rename_all = "camelCase")]
pub struct RankChanged {}

/// League runs or tournaments may have changed (a league or tournament match
/// was derived or removed, a league event was stored, a league edit, an
/// import, or a round adopting its deck) and the change is committed.
/// Refetch the leagues and deck pages.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type, Event)]
#[serde(rename_all = "camelCase")]
pub struct LeaguesChanged {}

/// Limited data changed (a draft event was stored, a pick note was saved,
/// or 0.x Limited data was imported) and the change is committed. Refetch
/// the Limited pages.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type, Event)]
#[serde(rename_all = "camelCase")]
pub struct LimitedChanged {}

/// Card data was filled (names, details and images from the API or MTGO's
/// token XML) and is committed. Refetch pages that show cards and the
/// Settings "Card data" card.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type, Event)]
#[serde(rename_all = "camelCase")]
pub struct CardsChanged {}

/// Archetypes changed as a whole: a definitions check or download (names,
/// formats, moves, retirements, new rows) or a 0.x import's archetypes.
/// Refetch every page that shows archetype names or counts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type, Event)]
#[serde(rename_all = "camelCase")]
pub struct ArchetypesChanged {}

/// Detection or a hand edit changed these matches' archetype rows or
/// detection marker, and the change is committed. Refetch the match pages
/// that show them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type, Event)]
#[serde(rename_all = "camelCase")]
pub struct MatchArchetypesChanged {
    #[specta(type = Vec<specta_typescript::Number>)]
    pub match_ids: Vec<i64>,
}

/// "Detect" the player asked for wrote this match's archetypes, committed,
/// while the rest of the run is still asked: the page refetches and stops
/// the match's spinner. `MatchArchetypesChanged` follows on the tracker's
/// next tick as for any detection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type, Event)]
#[serde(rename_all = "camelCase")]
pub struct MatchDetected {
    pub token: String,
}

/// A sideboard guide or matchup note of this deck and opponent archetype
/// was created, saved or deleted, and the change is committed. Refetch the
/// deck's guide pages, the archetype page's guides block and the overlay's
/// guide pane.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type, Event)]
#[serde(rename_all = "camelCase")]
pub struct GuidesChanged {
    #[specta(type = specta_typescript::Number)]
    pub deck_id: i64,
    #[specta(type = specta_typescript::Number)]
    pub archetype_id: i64,
}

/// The website's metagame shares of a format slug over `days` were fetched
/// (or failed) on the archetypes worker. Refetch the Metagame page; its
/// global column reads the cache again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type, Event)]
#[serde(rename_all = "camelCase")]
pub struct MetagameSharesReady {
    pub format: String,
    pub days: u32,
}

/// The MyMTGO account changed: a sign-in started or ended, or the device
/// signed out. Refetch the account status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type, Event)]
#[serde(rename_all = "camelCase")]
pub struct ApiChanged {}

/// Offline mode was switched (by the user or a 0.x import). Replace the
/// offline mode card's state; the status bar follows `StatusChanged`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type, Event)]
#[serde(rename_all = "camelCase")]
pub struct OfflineModeChanged {
    pub state: crate::api::offline::OfflineModeState,
}

/// An in-app toast raised by the app (0.x `AppNotification`): the sync run's
/// "Synced with MyMTGO" when matches moved, and "Match recorded 2-1" when a
/// live match completes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type, Event)]
#[serde(rename_all = "camelCase")]
pub struct AppNotification {
    /// What raised it (`sync`), or a match's result (`match_win`,
    /// `match_loss`, `match_voided` for a draw, `match_incomplete`).
    pub kind: String,
    pub title: String,
    pub message: String,
    /// Where the toast leads.
    pub route: Option<String>,
}
