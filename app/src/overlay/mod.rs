//! The overlays (overlay spec, sub-project 8): the game overlay, the draft
//! notes window and the league overlay window, all always on top, opened and
//! fed by the `overlay` thread, with network calls on the `overlay-net`
//! thread.
//!
//! - `windows`: the window manager (Tauri and a recording fake);
//! - `worker`: the `overlay` thread (compute, push, open and close);
//! - `net`: the `overlay-net` thread (one request at a time);
//! - `game`: the game overlay's view;
//! - `game_net`: what the game overlay asks the network, and its answers;
//! - `draft`: the draft notes window (Plan N's view, pushed);
//! - `league`: the league overlay window's card and its commands;
//! - `background`: the card's custom background and its `overlay` scheme;
//! - `publish`: the hosted OBS overlay (publish, heartbeat, clear);
//! - `commands`: the Overlays settings page's read and the game overlay's
//!   commands;
//! - `events`: what is pushed to each window.

pub mod background;
pub mod commands;
pub mod draft;
pub mod events;
pub mod game;
pub mod game_net;
pub mod league;
pub mod net;
pub mod publish;
pub mod windows;
pub mod worker;

use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::mpsc::{Receiver, Sender};
use std::thread::JoinHandle;

use overlay::{LeagueTheme, OverlaySize};
use tauri::{AppHandle, Manager};

use crate::bridge::lock;
use crate::overlay::game::KnownOpponent;
use crate::overlay::game_net::{AppGamePort, GameNet};
use crate::overlay::net::NetQueue;
use crate::overlay::publish::AppPort;
use crate::overlay::windows::{TauriWindows, WindowSpec, WindowStyle};
use crate::overlay::worker::{GameSlot, OverlayMsg, SharedGameSlot, Worker, run_overlay_worker};
use crate::settings::Settings;

/// The window labels (spec 5.2).
pub const GAME_OVERLAY: &str = "game-overlay";
pub const DRAFT_NOTES: &str = "draft-notes";
pub const LEAGUE_OVERLAY: &str = "league-overlay";
/// The main window (the league card's settings preview listens there).
pub const MAIN_WINDOW: &str = "main";

/// Every overlay window: the window-state plugin skips their initial restore
/// (the manager places them itself) and the overlay capability names them.
pub const OVERLAY_LABELS: [&str; 3] = [GAME_OVERLAY, DRAFT_NOTES, LEAGUE_OVERLAY];

/// The game overlay (0.x `OpenGameOverlayWindow`): 320 wide, as tall as the
/// sections need before the page measures itself, low enough to collapse.
pub fn game_overlay_spec(settings: &Settings) -> WindowSpec {
    let sections = game::sections(settings);
    WindowSpec {
        label: GAME_OVERLAY.to_string(),
        url: "overlay.html?view=game".to_string(),
        title: "Game overlay".to_string(),
        width: f64::from(overlay::height::DEFAULT_WIDTH),
        height: f64::from(overlay::height::from_settings(&sections, None)),
        min_width: 300.0,
        min_height: f64::from(overlay::height::COLLAPSED_MIN_HEIGHT),
        max_width: Some(400.0),
        remember_size: true,
        restore_position: true,
        style: WindowStyle::Overlay,
        shadow: true,
        transparent: false,
    }
}

/// The draft notes window (0.x `OpenDraftNotesWindow`).
pub fn draft_notes_spec() -> WindowSpec {
    WindowSpec {
        label: DRAFT_NOTES.to_string(),
        url: "overlay.html?view=draft-notes".to_string(),
        title: "Draft notes".to_string(),
        width: 320.0,
        height: 220.0,
        min_width: 280.0,
        min_height: 180.0,
        max_width: Some(480.0),
        remember_size: true,
        restore_position: true,
        style: WindowStyle::Overlay,
        shadow: true,
        transparent: false,
    }
}

/// The league overlay window: exactly the card (and the other event strip
/// while one shows); its size always follows the theme and size settings.
pub fn league_overlay_spec(theme: LeagueTheme, size: OverlaySize, strip: bool) -> WindowSpec {
    let (width, height) = league_window_size(theme, size, strip);
    WindowSpec {
        label: LEAGUE_OVERLAY.to_string(),
        url: "overlay.html?view=league".to_string(),
        title: "League Overlay".to_string(),
        width,
        height,
        // One floor below every theme's window: an open window keeps the
        // minimum it opened with, so a theme or size change must still fit
        // (Classic compact is 58 high).
        min_width: 200.0,
        min_height: 50.0,
        max_width: None,
        remember_size: false,
        restore_position: true,
        style: WindowStyle::Overlay,
        shadow: false,
        transparent: true,
    }
}

/// The league window's size for a theme and card size: the card exactly,
/// on both themes, as the settings preview and the OBS source show it (the
/// hosted page pads the card itself). Any transparent margin would block
/// clicks to MTGO underneath, so the window is taller by the other event
/// strip only while one shows (event overlay spec 8.4).
pub fn league_window_size(theme: LeagueTheme, size: OverlaySize, strip: bool) -> (f64, f64) {
    let (width, height) = overlay::card_and_strip_size(theme, size, strip);
    (f64::from(width), f64::from(height))
}

/// The running `overlay` and `overlay-net` threads.
pub struct OverlayWorker {
    sender: Mutex<Option<Sender<OverlayMsg>>>,
    /// The game overlay's latest push and live match, for its commands.
    game_slot: SharedGameSlot,
    thread: Mutex<Option<JoinHandle<()>>>,
    net: Mutex<Option<NetQueue>>,
}

impl OverlayWorker {
    /// Start both threads. The worker reads with its own read-only
    /// connection; `inbox` was made before the bridge started, so nothing
    /// the tracker said meanwhile is lost.
    pub fn start(
        app: AppHandle,
        sender: Sender<OverlayMsg>,
        inbox: Receiver<OverlayMsg>,
        db_path: PathBuf,
        overlay_folder: PathBuf,
        available: bool,
    ) -> OverlayWorker {
        let net = NetQueue::start(sender.clone());
        let game_slot: SharedGameSlot = std::sync::Arc::new(Mutex::new(GameSlot::default()));
        let worker_slot = std::sync::Arc::clone(&game_slot);
        let game_port = net
            .as_ref()
            .map(|net| AppGamePort::new(app.clone(), net.jobs()));
        let save_app = app.clone();
        let publish = net.as_ref().map(|net| {
            AppPort::new(
                app.clone(),
                net.jobs(),
                db_path.clone(),
                overlay_folder.clone(),
            )
        });
        let spawned = std::thread::Builder::new()
            .name("overlay".to_string())
            .spawn(move || {
                let conn = match store::open_reader(&db_path) {
                    Ok(conn) => conn,
                    Err(error) => {
                        tracing::error!(%error, "the overlay worker could not open the database; no overlays");
                        return;
                    }
                };
                let rows = store::read::settings(&conn).unwrap_or_default();
                let mut worker = Worker::new(
                    Settings::from_rows(&rows),
                    crate::settings::overlay_collapsed(&rows),
                    available,
                )
                .with_overlay_folder(overlay_folder)
                .with_expanded_height(crate::settings::overlay_expanded_height(&rows))
                .with_game_slot(worker_slot)
                .with_save_rows(Box::new(move |rows| {
                    crate::overlay::publish::save_through_tracker(&save_app, rows)
                }));
                if let Some(port) = game_port {
                    worker = worker.with_game_net(GameNet::new(Some(Box::new(port))));
                }
                if let Some(port) = publish {
                    worker = worker.with_publish(Box::new(port));
                }
                let mut windows = TauriWindows::new(app);
                run_overlay_worker(
                    &inbox,
                    &mut worker,
                    &conn,
                    &mut windows,
                    &crate::wall_clock_ms,
                );
            });
        let thread = match spawned {
            Ok(thread) => Some(thread),
            Err(error) => {
                tracing::error!(%error, "could not start the overlay worker; no overlays");
                None
            }
        };
        OverlayWorker {
            sender: Mutex::new(Some(sender)),
            game_slot,
            thread: Mutex::new(thread),
            net: Mutex::new(net),
        }
    }

    /// The game overlay's latest push and live match.
    pub fn game_slot(&self) -> GameSlot {
        lock(&self.game_slot).clone()
    }

    /// The archetype the game overlay named for its live match.
    pub fn known_opponent(&self) -> Option<KnownOpponent> {
        lock(&self.game_slot).opponent.clone()
    }

    /// Wake the worker. Returns false once stopped.
    pub fn send(&self, message: OverlayMsg) -> bool {
        match lock(&self.sender).as_ref() {
            Some(sender) => sender.send(message).is_ok(),
            None => false,
        }
    }

    /// Stop both threads without waiting on a window call or a request.
    pub fn stop(&self) {
        if let Some(sender) = lock(&self.sender).take() {
            let _ = sender.send(OverlayMsg::Stop);
        }
        if let Some(net) = lock(&self.net).take() {
            net.stop();
        }
        if let Some(thread) = lock(&self.thread).take()
            && thread.is_finished()
        {
            let _ = thread.join();
        }
    }
}

/// Send a message to the app's overlay worker, if it runs.
pub fn signal(app: &AppHandle, message: OverlayMsg) {
    if let Some(worker) = app.try_state::<OverlayWorker>() {
        worker.send(message);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_capability_names_every_overlay_window_and_only_its_permissions() {
        let capability: serde_json::Value =
            serde_json::from_str(include_str!("../../capabilities/overlay.json"))
                .expect("overlay.json parses");
        assert_eq!(capability["windows"], serde_json::json!(OVERLAY_LABELS));
        // Dragging, minimising from the right-click menu (#510), and only
        // the app commands the overlay pages call (#389).
        assert_eq!(
            capability["permissions"],
            serde_json::json!([
                "core:default",
                "core:window:allow-start-dragging",
                "core:window:allow-minimize",
                "allow-report-ui-error",
                "allow-game-overlay",
                "allow-fit-game-overlay",
                "allow-set-game-overlay-collapsed",
                "allow-overlay-archetype-options",
                "allow-set-overlay-opponent-archetype",
                "allow-add-overlay-note",
                "allow-delete-overlay-note",
                "allow-draft-notes",
                "allow-set-pick-note",
                "allow-league-overlay",
                "allow-set-overlay-deck-label"
            ])
        );
    }

    // The window is transparent (0.x's final card was opaque in an opaque
    // window, so its rounded corners sat in a square box; owner,
    // 2026-10-12).
    #[test]
    fn the_classic_window_is_its_card_at_both_sizes() {
        let full = league_overlay_spec(LeagueTheme::Classic, OverlaySize::Full, false);
        assert_eq!((full.width, full.height), (300.0, 80.0));
        let compact = league_overlay_spec(LeagueTheme::Classic, OverlaySize::Compact, false);
        assert_eq!((compact.width, compact.height), (240.0, 58.0));
        // 200 x 50 sits below every theme and size, so a window that opened
        // at one still fits any other.
        assert_eq!((compact.min_width, compact.min_height), (200.0, 50.0));
        assert!(!compact.shadow);
        assert!(compact.transparent, "only the rounded card shows");
        assert!(!game_overlay_spec(&Settings::default()).transparent);
        assert!(!compact.remember_size, "its size follows the setting");
        assert_eq!(compact.title, "League Overlay");
    }

    /// The overlay deviations of spec section 17, as notes (fixture
    /// `overlay`) in `fixtures/parity/allowlist.json`.
    const DEVIATIONS: [&str; 14] = [
        "pushed events, no polls",
        "one entry page per window",
        "sideboarding by log position",
        "no community rates",
        "unmet archetypes in the picker",
        "league window position remembered",
        "deck labels in a column",
        "unlinked draft notes",
        "scouting failures cached",
        "windows open without focus",
        "windows reopen at their saved position",
        "overlay windows unavailable on native Wayland",
        "game overlay only for matches announced this session",
        "debug simulator dropped",
    ];

    // Spec section 17: every deviation from 0.x is recorded in the
    // allowlist, with its reason.
    #[test]
    fn the_overlay_deviations_are_noted_in_the_allowlist() {
        let list: Vec<serde_json::Value> =
            serde_json::from_str(include_str!("../../../fixtures/parity/allowlist.json"))
                .expect("the allowlist is JSON");
        let notes: Vec<(String, String)> = list
            .iter()
            .filter(|entry| entry["fixture"] == "overlay")
            .map(|entry| {
                (
                    entry["field"].as_str().expect("field").to_string(),
                    entry["reason"].as_str().expect("reason").to_string(),
                )
            })
            .collect();
        for field in DEVIATIONS {
            let note = notes.iter().find(|(name, _)| name == field);
            assert!(
                note.is_some_and(|(_, reason)| !reason.trim().is_empty()),
                "{field}"
            );
        }
        assert_eq!(notes.len(), DEVIATIONS.len(), "{notes:?}");
    }

    #[test]
    fn every_window_is_its_card_exactly() {
        // No padding on either theme: a transparent margin would block
        // clicks to MTGO, and the card does not stretch to fill one.
        assert_eq!(
            league_window_size(LeagueTheme::Versus, OverlaySize::Full, false),
            (580.0, 104.0)
        );
        assert_eq!(
            league_window_size(LeagueTheme::Versus, OverlaySize::Compact, false),
            (380.0, 64.0)
        );
        assert_eq!(
            league_window_size(LeagueTheme::Classic, OverlaySize::Full, false),
            (300.0, 80.0)
        );
        assert_eq!(
            league_window_size(LeagueTheme::Classic, OverlaySize::Compact, false),
            (240.0, 58.0)
        );
        // Every window size fits the minimum any window opened with.
        for opened in [LeagueTheme::Classic, LeagueTheme::Versus] {
            let spec = league_overlay_spec(opened, OverlaySize::Full, false);
            for theme in [LeagueTheme::Classic, LeagueTheme::Versus] {
                for size in [OverlaySize::Full, OverlaySize::Compact] {
                    let (width, height) = league_window_size(theme, size, false);
                    assert!(spec.min_width <= width && spec.min_height <= height);
                }
            }
        }
    }

    #[test]
    fn the_league_window_grows_down_by_the_strip_on_every_theme_and_size() {
        // Event overlay spec 8.2: card and strip exactly on both themes.
        let rows = [
            (LeagueTheme::Classic, OverlaySize::Full, (300.0, 96.0)),
            (LeagueTheme::Classic, OverlaySize::Compact, (240.0, 74.0)),
            (LeagueTheme::Versus, OverlaySize::Full, (580.0, 124.0)),
            (LeagueTheme::Versus, OverlaySize::Compact, (380.0, 80.0)),
        ];
        for (theme, size, grown) in rows {
            assert_eq!(league_window_size(theme, size, true), grown);
            let (width, height) = league_window_size(theme, size, false);
            assert_eq!(width, grown.0, "the width never changes");
            assert!(height < grown.1);
            let spec = league_overlay_spec(theme, size, true);
            assert_eq!((spec.width, spec.height), grown);
        }
    }

    #[test]
    fn each_window_opens_its_page_of_the_overlay_entry() {
        let settings = Settings::default();
        assert_eq!(game_overlay_spec(&settings).url, "overlay.html?view=game");
        assert_eq!(draft_notes_spec().url, "overlay.html?view=draft-notes");
        assert_eq!(
            league_overlay_spec(LeagueTheme::Classic, OverlaySize::Full, false).url,
            "overlay.html?view=league"
        );
        let draft = draft_notes_spec();
        assert_eq!((draft.width, draft.height), (320.0, 220.0));
        assert_eq!((draft.min_width, draft.min_height), (280.0, 180.0));
        assert_eq!(draft.max_width, Some(480.0));
    }
}
