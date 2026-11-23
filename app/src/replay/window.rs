//! The replay window (replays spec section 6): one decorated window,
//! `game-replay`, that shows one game at a time.
//!
//! - Opening a game while the window is closed builds it at
//!   `replay.html?game={id}`, where it was left (Plan U's window manager:
//!   saved size and maximized state, and the saved position when it is on a
//!   monitor that still exists, else centred on the primary monitor).
//! - Opening a game while it is open pushes `ReplayGameChanged` to it and
//!   focuses it: the page switches games in place (0.x closed and reopened
//!   the window; ruling 7).
//! - On native Wayland (section 6.1) the compositor places it: its size and
//!   maximized state come back, its position is never applied. It opens on
//!   every platform and session, unlike the overlays.
//!
//! The window is built off the IPC thread (`off_ipc`), as every extra
//! window: Tauri deadlocks on Windows when a synchronous command builds one.
//! The window-state plugin's saved state is read through the manager, whose
//! saves run on the main thread (Plan U); its own `restore_state` is never
//! called here.

use std::sync::Mutex;

use serde::Serialize;
use tauri::{AppHandle, Manager};
use tauri_specta::Event;

use crate::bridge::lock;
use crate::overlay::windows::{TauriWindows, WindowEvent, WindowManager, WindowSpec, WindowStyle};
use crate::window::WindowSupport;

/// The replay window's label (0.x `game-replay-{gameId}`; v1 has one).
pub const GAME_REPLAY: &str = "game-replay";

/// Pushed to the open replay window when another game is opened: the page
/// shows that game instead.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type, Event)]
#[serde(rename_all = "camelCase")]
pub struct ReplayGameChanged {
    pub game_id: u32,
}

/// The replay window's page for one game.
pub fn replay_url(game_id: u32) -> String {
    format!("replay.html?game={game_id}")
}

/// The replay window (0.x `OpenReplayController`): "Game Replay", 1200 x
/// 800, at least 800 x 600, decorated, its size remembered. Its saved
/// position is restored except on native Wayland, where the compositor
/// places windows (spec section 6.1).
pub fn replay_spec(game_id: u32, native_wayland: bool) -> WindowSpec {
    WindowSpec {
        label: GAME_REPLAY.to_string(),
        url: replay_url(game_id),
        title: "Game Replay".to_string(),
        width: 1200.0,
        height: 800.0,
        min_width: 800.0,
        min_height: 600.0,
        max_width: None,
        remember_size: true,
        restore_position: !native_wayland,
        style: WindowStyle::Normal,
        shadow: true,
        transparent: false,
    }
}

/// What `open_replay` did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplayOpened {
    /// The window was built for the game.
    Built,
    /// The open window was told to show the game and focused.
    Navigated,
}

/// Open game row `game_id` in the replay window: build it when closed,
/// else switch the open one to the game and bring it to the front. The
/// same game again switches to the game it already shows, which the page
/// ignores.
pub fn open_replay(
    windows: &mut impl WindowManager,
    game_id: u32,
    native_wayland: bool,
) -> Result<ReplayOpened, String> {
    // `open` builds only a closed window.
    if windows.open(&replay_spec(game_id, native_wayland))? {
        return Ok(ReplayOpened::Built);
    }
    windows.emit(
        GAME_REPLAY,
        &WindowEvent::new(ReplayGameChanged { game_id }),
    )?;
    windows.focus(GAME_REPLAY)?;
    Ok(ReplayOpened::Navigated)
}

/// Run `job` on a blocking worker thread, never on the IPC thread or an
/// async task (Plan U's window rule).
pub async fn off_ipc<T: Send + 'static>(
    job: impl FnOnce() -> T + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(job)
        .await
        .map_err(|error| format!("the replay window could not be opened: {error}"))
}

/// One replay window open at a time: two quick clicks never build two.
static OPENING: Mutex<()> = Mutex::new(());

/// The game the last Replay click asked for. A click that lands while the
/// window's page is still loading emits `replay-game-changed` before the page
/// listens, so the page asks for this once its listener is registered and
/// shows the newest request.
#[derive(Debug, Default)]
pub struct RequestedGame {
    game_id: Mutex<Option<u32>>,
}

impl RequestedGame {
    pub const fn new() -> Self {
        Self {
            game_id: Mutex::new(None),
        }
    }

    /// Remember `game_id` as the newest request.
    pub fn set(&self, game_id: u32) {
        *lock(&self.game_id) = Some(game_id);
    }

    /// Put back what `get` returned before a request that failed.
    fn restore(&self, previous: Option<u32>) {
        *lock(&self.game_id) = previous;
    }

    /// The newest request, `None` before any click.
    pub fn get(&self) -> Option<u32> {
        *lock(&self.game_id)
    }
}

static REQUESTED: RequestedGame = RequestedGame::new();

/// `open_replay` for a click: the request is remembered before the window is
/// built or told (a page that asks the moment it listens must find it), and
/// forgotten again when the open fails, so a failed click is not shown later.
pub fn open_requested(
    windows: &mut impl WindowManager,
    requested: &RequestedGame,
    game_id: u32,
    native_wayland: bool,
) -> Result<ReplayOpened, String> {
    let previous = requested.get();
    requested.set(game_id);
    let opened = open_replay(windows, game_id, native_wayland);
    if opened.is_err() {
        requested.restore(previous);
    }
    opened
}

/// The game the newest Replay click asked for (the page asks once it is
/// listening, so a click during its load is not lost).
#[tauri::command]
#[specta::specta]
pub fn replay_requested_game() -> Option<u32> {
    REQUESTED.get()
}

/// Open a game's replay (the match page's "Replay" button). Async: the
/// window is built on a worker thread.
#[tauri::command]
#[specta::specta]
pub async fn open_replay_window(app: AppHandle, game_id: u32) -> Result<(), String> {
    let native_wayland = app
        .try_state::<WindowSupport>()
        .is_some_and(|support| support.native_wayland());
    off_ipc(move || {
        let _one_at_a_time = lock(&OPENING);
        let mut windows = TauriWindows::new(app);
        open_requested(&mut windows, &REQUESTED, game_id, native_wayland)
    })
    .await??;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::overlay::windows::testing::{FakeWindows, primary};
    use crate::overlay::windows::{Monitors, Opening, SavedWindow};
    use ::overlay::{Os, Placement, Rect, SessionEnv};

    fn rect(x: i32, y: i32, width: u32, height: u32) -> Rect {
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    #[test]
    fn the_window_is_0x_s_game_replay_window() {
        let spec = replay_spec(42, false);
        assert_eq!(spec.label, "game-replay");
        assert_eq!(spec.url, "replay.html?game=42");
        assert_eq!(spec.title, "Game Replay");
        assert_eq!((spec.width, spec.height), (1200.0, 800.0));
        assert_eq!((spec.min_width, spec.min_height), (800.0, 600.0));
        assert_eq!(spec.max_width, None);
        assert_eq!(spec.style, WindowStyle::Normal);
        assert!(spec.remember_size);
        assert!(spec.restore_position);
        assert!(spec.shadow);
    }

    #[test]
    fn opening_another_game_navigates_the_open_window() {
        let mut windows = FakeWindows::default();
        assert_eq!(open_replay(&mut windows, 1, false), Ok(ReplayOpened::Built));
        assert_eq!(
            open_replay(&mut windows, 2, false),
            Ok(ReplayOpened::Navigated)
        );

        let specs = windows.opened_specs();
        assert_eq!(specs.len(), 1, "one window");
        assert_eq!(specs[0].url, "replay.html?game=1");
        let pushed = windows.emitted();
        assert_eq!(pushed.len(), 1);
        assert_eq!(pushed[0].0, GAME_REPLAY);
        assert_eq!(pushed[0].1.name, "replay-game-changed");
        assert_eq!(pushed[0].1.payload, serde_json::json!({ "gameId": 2 }));
        assert_eq!(windows.focused(), vec![GAME_REPLAY.to_string()]);
        assert!(windows.closed().is_empty(), "never closed and reopened");
    }

    #[test]
    fn the_app_keeps_the_newest_requested_game() {
        let requested = RequestedGame::new();
        assert_eq!(requested.get(), None, "nothing before a click");
        requested.set(4);
        // A second click while the page loads: the page asks and gets this one.
        requested.set(9);
        assert_eq!(requested.get(), Some(9));
    }

    /// A manager that answers, at the moment the window is told or built,
    /// what a page asking the app right then would get.
    struct AskingPage<'a> {
        inner: FakeWindows,
        requested: &'a RequestedGame,
        asked_at_emit: Vec<Option<u32>>,
        asked_at_build: Vec<Option<u32>>,
    }

    impl WindowManager for AskingPage<'_> {
        fn is_open(&self, label: &str) -> bool {
            self.inner.is_open(label)
        }
        fn saved_window(&mut self, label: &str) -> Option<SavedWindow> {
            self.inner.saved_window(label)
        }
        fn monitors(&self) -> Monitors {
            self.inner.monitors()
        }
        fn build(&mut self, spec: &WindowSpec, opening: &Opening) -> Result<(), String> {
            self.asked_at_build.push(self.requested.get());
            self.inner.build(spec, opening)
        }
        fn save_positions(&mut self) -> Result<(), String> {
            self.inner.save_positions()
        }
        fn destroy(&mut self, label: &str) -> Result<(), String> {
            self.inner.destroy(label)
        }
        fn resize(&mut self, label: &str, width: f64, height: f64) -> Result<(), String> {
            self.inner.resize(label, width, height)
        }
        fn size(&self, label: &str) -> Option<(f64, f64)> {
            self.inner.size(label)
        }
        fn focus(&mut self, label: &str) -> Result<(), String> {
            self.inner.focus(label)
        }
        fn emit(&mut self, label: &str, event: &WindowEvent) -> Result<(), String> {
            self.asked_at_emit.push(self.requested.get());
            self.inner.emit(label, event)
        }
    }

    #[test]
    fn a_page_asking_during_the_open_already_finds_the_click() {
        // The race of #307 F3: the page loads from the first click's url,
        // the second click emits before it listens. Whenever the page asks
        // after that emit, the app must already hold the second game.
        let requested = RequestedGame::new();
        let mut windows = AskingPage {
            inner: FakeWindows::default(),
            requested: &requested,
            asked_at_emit: Vec::new(),
            asked_at_build: Vec::new(),
        };
        open_requested(&mut windows, &requested, 1, false).expect("first click");
        open_requested(&mut windows, &requested, 2, false).expect("second click");
        assert_eq!(
            windows.asked_at_build,
            vec![Some(1)],
            "set before the build"
        );
        assert_eq!(windows.asked_at_emit, vec![Some(2)], "set before the push");
        assert_eq!(requested.get(), Some(2));
    }

    #[test]
    fn a_failed_open_is_not_remembered() {
        let requested = RequestedGame::new();
        let mut windows = FakeWindows::default();
        open_requested(&mut windows, &requested, 1, false).expect("open");
        let mut failing = FakeWindows {
            fail: true,
            ..FakeWindows::default()
        };
        assert!(open_requested(&mut failing, &requested, 5, false).is_err());
        assert_eq!(requested.get(), Some(1), "the failed click is forgotten");
    }

    #[test]
    fn opening_the_same_game_twice_does_nothing_new() {
        let mut windows = FakeWindows::default();
        open_replay(&mut windows, 7, false).expect("open");
        assert_eq!(
            open_replay(&mut windows, 7, false),
            Ok(ReplayOpened::Navigated)
        );
        assert_eq!(windows.opened_specs().len(), 1, "no second window");
        assert!(windows.closed().is_empty());
        // The page already shows game 7 and ignores the push.
        let pushed = windows.emitted();
        assert_eq!(pushed.len(), 1);
        assert_eq!(pushed[0].1.payload, serde_json::json!({ "gameId": 7 }));
    }

    /// A manager that remembers which thread built each window.
    struct ThreadRecording {
        inner: FakeWindows,
        built_on: Vec<std::thread::ThreadId>,
    }

    impl WindowManager for ThreadRecording {
        fn is_open(&self, label: &str) -> bool {
            self.inner.is_open(label)
        }
        fn saved_window(&mut self, label: &str) -> Option<SavedWindow> {
            self.inner.saved_window(label)
        }
        fn monitors(&self) -> Monitors {
            self.inner.monitors()
        }
        fn build(&mut self, spec: &WindowSpec, opening: &Opening) -> Result<(), String> {
            self.built_on.push(std::thread::current().id());
            self.inner.build(spec, opening)
        }
        fn save_positions(&mut self) -> Result<(), String> {
            self.inner.save_positions()
        }
        fn destroy(&mut self, label: &str) -> Result<(), String> {
            self.inner.destroy(label)
        }
        fn resize(&mut self, label: &str, width: f64, height: f64) -> Result<(), String> {
            self.inner.resize(label, width, height)
        }
        fn size(&self, label: &str) -> Option<(f64, f64)> {
            self.inner.size(label)
        }
        fn focus(&mut self, label: &str) -> Result<(), String> {
            self.inner.focus(label)
        }
        fn emit(&mut self, label: &str, event: &WindowEvent) -> Result<(), String> {
            self.inner.emit(label, event)
        }
    }

    #[test]
    fn the_window_is_never_built_on_the_ipc_thread() {
        // The command's path: `off_ipc` around `open_replay`. The caller
        // here stands for the thread the command was called on.
        let caller = std::thread::current().id();
        let windows = tauri::async_runtime::block_on(off_ipc(move || {
            let mut windows = ThreadRecording {
                inner: FakeWindows::default(),
                built_on: Vec::new(),
            };
            let opened = open_replay(&mut windows, 3, false);
            (windows, opened)
        }));
        let (windows, opened) = windows.expect("the worker ran");
        assert_eq!(opened, Ok(ReplayOpened::Built));
        assert_eq!(windows.built_on.len(), 1);
        assert_ne!(windows.built_on[0], caller, "built on a worker thread");
    }

    #[test]
    fn the_window_restores_its_saved_bounds() {
        let mut windows = FakeWindows::default();
        open_replay(&mut windows, 1, false).expect("open");
        assert_eq!(
            windows.openings()[0].1,
            Opening {
                position: Some(Placement::Centre(Some(primary()))),
                size: None,
                maximized: false
            },
            "first open: centred at its spec size"
        );
        windows.user_moves(GAME_REPLAY, rect(300, 200, 1300, 850));
        windows.user_closes(GAME_REPLAY);

        open_replay(&mut windows, 2, false).expect("reopen");
        assert_eq!(
            windows.openings()[1].1,
            Opening {
                position: Some(Placement::Restore(rect(300, 200, 1300, 850))),
                size: Some((1300, 850)),
                maximized: false
            }
        );
        assert_eq!(windows.opened_specs()[1].url, "replay.html?game=2");

        windows.user_maximizes(GAME_REPLAY);
        windows.user_closes(GAME_REPLAY);
        open_replay(&mut windows, 2, false).expect("reopen maximized");
        assert!(windows.openings()[2].1.maximized);
    }

    #[test]
    fn the_window_restores_size_but_not_position_on_native_wayland() {
        let mut windows = FakeWindows::default();
        // Placed during an X11 session.
        open_replay(&mut windows, 1, false).expect("open");
        windows.user_moves(GAME_REPLAY, rect(300, 200, 1300, 850));
        windows.user_maximizes(GAME_REPLAY);
        windows.user_closes(GAME_REPLAY);

        open_replay(&mut windows, 1, true).expect("open on Wayland");
        assert_eq!(
            windows.openings()[1].1,
            Opening {
                position: None,
                size: Some((1300, 850)),
                maximized: true
            }
        );
        assert!(!replay_spec(1, true).restore_position);
    }

    fn session(
        wayland_display: Option<&str>,
        wayland_socket: Option<&str>,
        gdk_backend: Option<&str>,
        display: Option<&str>,
    ) -> SessionEnv {
        SessionEnv {
            wayland_display: wayland_display.map(str::to_string),
            wayland_socket: wayland_socket.map(str::to_string),
            gdk_backend: gdk_backend.map(str::to_string),
            display: display.map(str::to_string),
        }
    }

    #[test]
    fn the_replay_window_is_available_on_native_wayland() {
        // Plan U's native Wayland sessions: no overlays there, the replay
        // window still opens, placed by the compositor.
        let wayland = [
            session(Some("wayland-0"), None, None, None),
            session(Some("wayland-0"), None, None, Some(":0")),
            session(Some("wayland-0"), None, Some("wayland"), Some(":0")),
            session(Some("wayland-0"), None, Some("*"), Some(":0")),
            session(Some("wayland-0"), None, Some("wayland,x11"), Some(":0")),
            session(None, Some("3"), None, None),
            session(Some("wayland-0"), None, Some("x11"), None),
        ];
        for env in &wayland {
            let support = WindowSupport::from_env(Os::Linux, env);
            assert!(support.native_wayland(), "{env:?}");
            let mut windows = FakeWindows::default();
            assert_eq!(
                open_replay(&mut windows, 5, support.native_wayland()),
                Ok(ReplayOpened::Built),
                "{env:?}"
            );
            assert_eq!(windows.opened(), vec![(GAME_REPLAY.to_string(), None)]);
        }
        // X11 and XWayland: placed as on Windows.
        let placed = [
            session(None, None, None, Some(":0")),
            session(Some("wayland-0"), None, Some("x11"), Some(":0")),
        ];
        for env in &placed {
            let support = WindowSupport::from_env(Os::Linux, env);
            assert!(!support.native_wayland(), "{env:?}");
            let mut windows = FakeWindows::default();
            open_replay(&mut windows, 5, support.native_wayland()).expect("open");
            assert_eq!(
                windows.opened(),
                vec![(
                    GAME_REPLAY.to_string(),
                    Some(Placement::Centre(Some(primary())))
                )]
            );
        }
    }

    #[test]
    fn a_saved_position_on_a_missing_monitor_is_centred() {
        let second = rect(1920, 0, 2560, 1440);
        let mut windows = FakeWindows {
            monitors: Monitors {
                all: vec![primary(), second],
                primary: Some(primary()),
            },
            ..FakeWindows::default()
        };
        open_replay(&mut windows, 1, false).expect("open");
        windows.user_moves(GAME_REPLAY, rect(2200, 300, 1200, 800));
        windows.user_closes(GAME_REPLAY);
        // The second monitor is unplugged.
        windows.monitors = Monitors {
            all: vec![primary()],
            primary: Some(primary()),
        };
        open_replay(&mut windows, 1, false).expect("reopen");
        let opening = windows.openings()[1].1;
        assert_eq!(opening.position, Some(Placement::Centre(Some(primary()))));
        assert_eq!(opening.size, Some((1200, 800)), "its size still comes back");
    }

    #[test]
    fn a_failing_window_api_is_an_error() {
        let mut windows = FakeWindows {
            fail: true,
            ..FakeWindows::default()
        };
        assert!(open_replay(&mut windows, 1, false).is_err());
    }

    #[test]
    fn the_capability_names_the_replay_window_and_only_its_permissions() {
        let capability: serde_json::Value =
            serde_json::from_str(include_str!("../../capabilities/replay.json"))
                .expect("replay.json parses");
        assert_eq!(capability["identifier"], "game-replay");
        assert_eq!(capability["windows"], serde_json::json!([GAME_REPLAY]));
        assert_eq!(
            capability["permissions"],
            serde_json::json!([
                "core:default",
                "core:window:allow-set-fullscreen",
                "core:window:allow-is-fullscreen",
                // Only the app commands the replay page calls (#389).
                "allow-report-ui-error",
                "allow-replay-view",
                "allow-replay-requested-game",
                "allow-replay-share-state",
                "allow-share-replay",
                "allow-revoke-replay"
            ])
        );
    }
}
