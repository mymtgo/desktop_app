//! The window manager (overlay spec 5.2): opens, closes, resizes and pushes
//! events to the app's extra windows. Generic over the window: each window
//! is a `WindowSpec` (label, page, title, sizes, style) passed to `open`, so
//! another window (Plan V's replay window) is one more spec, not a second
//! manager.
//!
//! `WindowManager` has a few primitives (is a window open, its saved bounds,
//! the monitors, build, save, destroy, resize, emit) and two provided
//! methods built on them, `open` and `close`, which hold the rules:
//! - opening an open window does nothing; a window reopens where it was left
//!   (`overlay::placement`): centred only when it was never opened or its
//!   saved monitor is gone;
//! - closing a closed window does nothing; every close the manager makes
//!   saves the positions first, so a programmatic close remembers the window
//!   as a user close does.
//!
//! `TauriWindows` is the real one (Tauri 2 `WebviewWindowBuilder` and
//! `tauri-plugin-window-state`); `testing::FakeWindows` records what it was
//! asked, for tests without a webview. Like a Laravel facade with a fake:
//! the worker only sees the trait.
//!
//! Windows are built from a worker thread, never inside a synchronous
//! command (Tauri deadlocks on Windows).

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;

use overlay::{Placement, Rect, placement};
use serde::Serialize;
use tauri::{AppHandle, EventTarget, Manager, PhysicalPosition, PhysicalSize, WebviewUrl};
use tauri_plugin_window_state::{AppHandleExt, StateFlags};

/// How a window looks and behaves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowStyle {
    /// An overlay (spec 5.2): no decorations, always on top, opens without
    /// taking focus (MTGO keeps the keyboard), cannot be maximized or
    /// minimized, still shown in the taskbar.
    Overlay,
    /// A normal app window: decorations, takes focus when opened.
    Normal,
}

/// Everything needed to build one window. Sizes are logical pixels.
#[derive(Debug, Clone, PartialEq)]
pub struct WindowSpec {
    pub label: String,
    /// The page, relative to the app (`overlay.html?view=game`).
    pub url: String,
    pub title: String,
    pub width: f64,
    pub height: f64,
    pub min_width: f64,
    pub min_height: f64,
    pub max_width: Option<f64>,
    /// Restore the saved size and maximized state.
    pub remember_size: bool,
    /// Restore the saved position (centred when it is off every monitor).
    /// False where the compositor places windows itself (native Wayland):
    /// the window then opens where the compositor puts it.
    pub restore_position: bool,
    pub style: WindowStyle,
    /// A drop shadow (the league window has none, 0.x `hasShadow(false)`).
    pub shadow: bool,
    /// A see-through window, so a page with rounded corners shows only
    /// itself (the league card).
    pub transparent: bool,
}

/// The connected monitors in physical pixels, and the primary one.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Monitors {
    pub all: Vec<Rect>,
    pub primary: Option<Rect>,
}

/// What the window-state plugin saved for a window: its bounds (physical
/// pixels; before it was maximized, when it is) and whether it was
/// maximized.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SavedWindow {
    pub bounds: Rect,
    pub maximized: bool,
}

/// How a window opens, decided from its spec and what was saved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Opening {
    /// `None`: the compositor places it (`restore_position` off).
    pub position: Option<Placement>,
    /// The saved size in physical pixels; `None`: the spec's size.
    pub size: Option<(u32, u32)>,
    pub maximized: bool,
}

/// The opening of `spec`: the saved position on a monitor that still
/// exists (else centred on the primary monitor), the saved size and
/// maximized state when the spec remembers them.
pub fn opening(spec: &WindowSpec, saved: Option<SavedWindow>, monitors: &Monitors) -> Opening {
    let position = spec.restore_position.then(|| {
        placement(
            saved.map(|saved| saved.bounds),
            &monitors.all,
            monitors.primary,
        )
    });
    let remembered = saved.filter(|_| spec.remember_size);
    Opening {
        position,
        size: remembered.map(|saved| (saved.bounds.width, saved.bounds.height)),
        maximized: remembered.is_some_and(|saved| saved.maximized),
    }
}

/// Emits a typed event to a target window.
type SendEvent = dyn Fn(&AppHandle, EventTarget) -> tauri::Result<()> + Send + Sync;

/// One typed event pushed to one window: any `tauri_specta` event of any
/// window (an overlay's view, a replay window's game). Tests read its
/// `name` and JSON `payload`; the real manager emits the typed event.
#[derive(Clone)]
pub struct WindowEvent {
    pub name: &'static str,
    pub payload: serde_json::Value,
    send: Arc<SendEvent>,
}

impl WindowEvent {
    pub fn new<E>(event: E) -> WindowEvent
    where
        E: tauri_specta::Event + Serialize + Clone + Send + Sync + 'static,
    {
        let payload = serde_json::to_value(&event).unwrap_or(serde_json::Value::Null);
        WindowEvent {
            name: E::NAME,
            payload,
            send: Arc::new(move |app, target| event.emit_to(app, target)),
        }
    }
}

impl std::fmt::Debug for WindowEvent {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WindowEvent")
            .field("name", &self.name)
            .field("payload", &self.payload)
            .finish()
    }
}

impl PartialEq for WindowEvent {
    fn eq(&self, other: &WindowEvent) -> bool {
        self.name == other.name && self.payload == other.payload
    }
}

/// Opens, closes, resizes and talks to the app's extra windows. Errors are
/// plain messages: the caller logs them and retries at its next wake.
pub trait WindowManager {
    /// Whether a window with this label exists.
    fn is_open(&self, label: &str) -> bool;
    /// What was saved for a window when it was last closed or moved;
    /// `None` when it was never opened.
    fn saved_window(&mut self, label: &str) -> Option<SavedWindow>;
    fn monitors(&self) -> Monitors;
    /// Build the window as `opening` says.
    fn build(&mut self, spec: &WindowSpec, opening: &Opening) -> Result<(), String>;
    /// Save every open window's bounds (as on app exit).
    fn save_positions(&mut self) -> Result<(), String>;
    /// Close an open window.
    fn destroy(&mut self, label: &str) -> Result<(), String>;
    /// Resize an open window (logical pixels).
    fn resize(&mut self, label: &str, width: f64, height: f64) -> Result<(), String>;
    /// An open window's inner size (logical pixels); `None` when closed.
    fn size(&self, label: &str) -> Option<(f64, f64)>;
    /// Bring an open window to the front with the keyboard.
    fn focus(&mut self, label: &str) -> Result<(), String>;
    /// Push an event to one window only.
    fn emit(&mut self, label: &str, event: &WindowEvent) -> Result<(), String>;

    /// Open a window where it was left. `Ok(false)` when it was already open.
    fn open(&mut self, spec: &WindowSpec) -> Result<bool, String> {
        if self.is_open(&spec.label) {
            return Ok(false);
        }
        let saved = self.saved_window(&spec.label);
        let monitors = self.monitors();
        self.build(spec, &opening(spec, saved, &monitors))?;
        Ok(true)
    }

    /// Close a window, saving its position first. `Ok(false)` when it was
    /// not open.
    fn close(&mut self, label: &str) -> Result<bool, String> {
        if !self.is_open(label) {
            return Ok(false);
        }
        if let Err(error) = self.save_positions() {
            tracing::warn!(%error, label, "could not save the window position before closing");
        }
        self.destroy(label)?;
        Ok(true)
    }
}

/// What the window-state plugin saved for `label` in its file's text
/// (`{label: {x, y, width, height, maximized, ..}}`, physical pixels).
/// `None` when the label has no entry or only the plugin's empty default
/// (never placed).
pub fn saved_window_in(text: &str, label: &str) -> Option<SavedWindow> {
    let states: serde_json::Value = serde_json::from_str(text).ok()?;
    let state = states.get(label)?;
    let number = |key: &str| state.get(key).and_then(serde_json::Value::as_i64);
    let width = u32::try_from(number("width")?).ok()?;
    let height = u32::try_from(number("height")?).ok()?;
    if width == 0 || height == 0 {
        return None;
    }
    let maximized = state
        .get("maximized")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    // A maximized window's x and y are the monitor's corner; the plugin
    // keeps where it was before in `prev_x` and `prev_y`.
    let (x_key, y_key) = if maximized {
        ("prev_x", "prev_y")
    } else {
        ("x", "y")
    };
    Some(SavedWindow {
        bounds: Rect {
            x: i32::try_from(number(x_key)?).ok()?,
            y: i32::try_from(number(y_key)?).ok()?,
            width,
            height,
        },
        maximized,
    })
}

/// The flags the window-state plugin saves with (its default: all).
const SAVED_STATE: StateFlags = StateFlags::all();

/// How long a worker waits for the main thread to save the window states.
pub const SAVE_WAIT: Duration = Duration::from_secs(5);

/// Run `job` on the thread `post` hands it to and wait up to `timeout` for
/// its answer.
///
/// Why the window states are saved on the main thread: the window-state
/// plugin's save holds its cache lock while it reads every window's size
/// and position. From another thread each read waits for the main thread,
/// while the main thread's move, resize and close handlers wait for that
/// same lock: a drag during an overlay open or close would hang the app.
/// On the main thread the reads run inline, so nothing waits on anything.
pub fn run_on_owner<T: Send + 'static>(
    post: impl FnOnce(Box<dyn FnOnce() + Send>) -> Result<(), String>,
    job: impl FnOnce() -> T + Send + 'static,
    timeout: Duration,
) -> Result<T, String> {
    let (answer, answered) = mpsc::channel();
    post(Box::new(move || {
        let _ = answer.send(job());
    }))?;
    answered
        .recv_timeout(timeout)
        .map_err(|_| "the main thread did not answer".to_string())
}

/// The real window manager.
pub struct TauriWindows {
    app: AppHandle,
}

impl TauriWindows {
    pub fn new(app: AppHandle) -> TauriWindows {
        TauriWindows { app }
    }

    fn state_file(&self) -> Option<PathBuf> {
        let dir = self.app.path().app_config_dir().ok()?;
        Some(dir.join(self.app.filename()))
    }

    /// The plugin's save, on the main thread (see `run_on_owner`).
    fn save_on_main_thread(&self) -> Result<(), String> {
        let app = self.app.clone();
        let poster = self.app.clone();
        run_on_owner(
            move |task| {
                poster
                    .run_on_main_thread(task)
                    .map_err(|error| error.to_string())
            },
            move || {
                app.save_window_state(SAVED_STATE)
                    .map_err(|error| error.to_string())
            },
            SAVE_WAIT,
        )?
    }

    /// The scale factor of the monitor holding `rect` (1 when none does).
    fn scale_at(&self, rect: Rect) -> f64 {
        let monitors = self.app.available_monitors().unwrap_or_default();
        for monitor in monitors {
            if overlay::on_any_monitor(rect, &[monitor_rect(&monitor)]) {
                return monitor.scale_factor();
            }
        }
        1.0
    }
}

fn monitor_rect(monitor: &tauri::Monitor) -> Rect {
    Rect {
        x: monitor.position().x,
        y: monitor.position().y,
        width: monitor.size().width,
        height: monitor.size().height,
    }
}

impl WindowManager for TauriWindows {
    fn is_open(&self, label: &str) -> bool {
        self.app.get_webview_window(label).is_some()
    }

    fn saved_window(&mut self, label: &str) -> Option<SavedWindow> {
        // The plugin keeps moves and user closes in memory; write them out
        // so the file holds this session's last bounds.
        if let Err(error) = self.save_on_main_thread() {
            tracing::warn!(%error, "could not save the window positions");
        }
        let text = std::fs::read_to_string(self.state_file()?).ok()?;
        saved_window_in(&text, label)
    }

    fn monitors(&self) -> Monitors {
        let all = self
            .app
            .available_monitors()
            .unwrap_or_default()
            .iter()
            .map(monitor_rect)
            .collect();
        let primary = self.app.primary_monitor().ok().flatten();
        Monitors {
            all,
            primary: primary.as_ref().map(monitor_rect),
        }
    }

    fn build(&mut self, spec: &WindowSpec, opening: &Opening) -> Result<(), String> {
        let url = WebviewUrl::App(PathBuf::from(&spec.url));
        let mut builder = tauri::WebviewWindowBuilder::new(&self.app, &spec.label, url)
            .title(&spec.title)
            .inner_size(spec.width, spec.height)
            .min_inner_size(spec.min_width, spec.min_height)
            .resizable(true)
            .shadow(spec.shadow)
            .transparent(spec.transparent)
            // Windows: no white flash before a transparent page paints.
            .no_redirection_bitmap(spec.transparent);
        if let Some(max_width) = spec.max_width {
            // Tauri has no width-only maximum.
            builder = builder.max_inner_size(max_width, 100_000.0);
        }
        builder = match spec.style {
            WindowStyle::Overlay => builder
                .decorations(false)
                .always_on_top(true)
                .maximizable(false)
                // Minimised from the taskbar or the right-click menu, as in
                // 0.x (#510).
                .minimizable(true)
                .focused(false)
                .skip_taskbar(false)
                // macOS: follow the user to every Space. Whether that keeps
                // an overlay over a Wine window in native full screen is not
                // yet checked on a Mac (macOS spec section 8).
                .visible_on_all_workspaces(cfg!(target_os = "macos")),
            WindowStyle::Normal => builder.focused(true),
        };
        // The scale of the monitor the window lands on, for logical sizes.
        let scale = match opening.position {
            Some(Placement::Restore(rect)) => self.scale_at(rect),
            Some(Placement::Centre(Some(primary))) => self.scale_at(primary),
            _ => self
                .app
                .primary_monitor()
                .ok()
                .flatten()
                .map_or(1.0, |monitor| monitor.scale_factor()),
        };
        let (width, height) = match opening.size {
            Some((width, height)) => (f64::from(width), f64::from(height)),
            None => (spec.width * scale, spec.height * scale),
        };
        if opening.size.is_some() {
            builder = builder.inner_size(width / scale, height / scale);
        }
        // The physical position to set once built, exactly.
        let mut exact: Option<Rect> = None;
        builder = match opening.position {
            Some(Placement::Restore(rect)) => {
                exact = Some(rect);
                builder.position(f64::from(rect.x) / scale, f64::from(rect.y) / scale)
            }
            Some(Placement::Centre(Some(primary))) => {
                let x = f64::from(primary.x) + (f64::from(primary.width) - width) / 2.0;
                let y = f64::from(primary.y) + (f64::from(primary.height) - height) / 2.0;
                builder.position(x / scale, y / scale)
            }
            Some(Placement::Centre(None)) => builder.center(),
            // The compositor places it.
            None => builder,
        };
        if opening.maximized {
            builder = builder.maximized(true);
        }
        let window = builder.build().map_err(|error| error.to_string())?;
        // Built at the logical bounds already; these only correct the
        // rounding of a mixed-DPI setup. Neither call takes focus.
        if !opening.maximized {
            if let Some(rect) = exact {
                let _ = window.set_position(PhysicalPosition::new(rect.x, rect.y));
            }
            if let Some((width, height)) = opening.size {
                let _ = window.set_size(PhysicalSize::new(width, height));
            }
        }
        Ok(())
    }

    fn save_positions(&mut self) -> Result<(), String> {
        self.save_on_main_thread()
    }

    fn destroy(&mut self, label: &str) -> Result<(), String> {
        // `destroy`, not `close`: gone at once (no close request to answer),
        // so the next wake never sees it half closed. Its bounds were saved
        // just before.
        match self.app.get_webview_window(label) {
            Some(window) => window.destroy().map_err(|error| error.to_string()),
            None => Ok(()),
        }
    }

    fn resize(&mut self, label: &str, width: f64, height: f64) -> Result<(), String> {
        match self.app.get_webview_window(label) {
            Some(window) => window
                .set_size(tauri::LogicalSize::new(width, height))
                .map_err(|error| error.to_string()),
            None => Ok(()),
        }
    }

    fn size(&self, label: &str) -> Option<(f64, f64)> {
        let window = self.app.get_webview_window(label)?;
        let scale = window.scale_factor().ok()?;
        let size = window.inner_size().ok()?.to_logical::<f64>(scale);
        Some((size.width, size.height))
    }

    fn focus(&mut self, label: &str) -> Result<(), String> {
        match self.app.get_webview_window(label) {
            Some(window) => window.set_focus().map_err(|error| error.to_string()),
            None => Ok(()),
        }
    }

    fn emit(&mut self, label: &str, event: &WindowEvent) -> Result<(), String> {
        (event.send)(&self.app, EventTarget::webview_window(label))
            .map_err(|error| error.to_string())
    }
}

/// A recording window manager for tests (the worker's, and any other
/// window's: Plan V's replay window uses it too).
#[cfg(test)]
pub mod testing {
    use std::collections::{BTreeMap, BTreeSet};

    use super::*;

    /// What a test saw the manager do.
    #[derive(Debug, Clone, PartialEq)]
    pub enum Call {
        /// A window was built with this spec, opening this way.
        Opened(WindowSpec, Opening),
        /// The positions were saved (before a close the manager made).
        Saved,
        /// An open window was closed.
        Closed(String),
        Resized(String, f64, f64),
        Focused(String),
        Emitted(String, WindowEvent),
    }

    /// Windows in memory: which are open and where, what was saved, the
    /// monitors, and every call. `fail` makes build, destroy and resize
    /// fail, like a window API that does not answer.
    pub struct FakeWindows {
        pub open: BTreeMap<String, Rect>,
        pub saved: BTreeMap<String, SavedWindow>,
        /// Open windows the player maximized.
        pub maximized: BTreeSet<String>,
        pub monitors: Monitors,
        pub calls: Vec<Call>,
        pub fail: bool,
    }

    /// A 1920 x 1080 primary monitor at the origin.
    pub fn primary() -> Rect {
        Rect {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
        }
    }

    impl Default for FakeWindows {
        fn default() -> FakeWindows {
            FakeWindows {
                open: BTreeMap::new(),
                saved: BTreeMap::new(),
                maximized: BTreeSet::new(),
                monitors: Monitors {
                    all: vec![primary()],
                    primary: Some(primary()),
                },
                calls: Vec::new(),
                fail: false,
            }
        }
    }

    impl FakeWindows {
        /// The player drags a window somewhere (the plugin tracks it).
        pub fn user_moves(&mut self, label: &str, to: Rect) {
            self.open.insert(label.to_string(), to);
        }

        /// The player maximizes a window (the plugin keeps the bounds it
        /// had before).
        pub fn user_maximizes(&mut self, label: &str) {
            self.maximized.insert(label.to_string());
        }

        /// The player closes a window (Alt+F4): the plugin keeps its bounds.
        pub fn user_closes(&mut self, label: &str) {
            self.save_one(label);
            self.open.remove(label);
            self.maximized.remove(label);
        }

        fn save_one(&mut self, label: &str) {
            if let Some(bounds) = self.open.get(label) {
                let saved = SavedWindow {
                    bounds: *bounds,
                    maximized: self.maximized.contains(label),
                };
                self.saved.insert(label.to_string(), saved);
            }
        }

        /// Every open: the label and how it opened.
        pub fn openings(&self) -> Vec<(String, Opening)> {
            self.calls
                .iter()
                .filter_map(|call| match call {
                    Call::Opened(spec, opening) => Some((spec.label.clone(), *opening)),
                    _ => None,
                })
                .collect()
        }

        /// Every open: the label and its position (`None`: the compositor's).
        pub fn opened(&self) -> Vec<(String, Option<Placement>)> {
            self.openings()
                .into_iter()
                .map(|(label, opening)| (label, opening.position))
                .collect()
        }

        pub fn focused(&self) -> Vec<String> {
            self.calls
                .iter()
                .filter_map(|call| match call {
                    Call::Focused(label) => Some(label.clone()),
                    _ => None,
                })
                .collect()
        }

        pub fn opened_specs(&self) -> Vec<WindowSpec> {
            self.calls
                .iter()
                .filter_map(|call| match call {
                    Call::Opened(spec, _) => Some(spec.clone()),
                    _ => None,
                })
                .collect()
        }

        pub fn opened_labels(&self) -> Vec<String> {
            self.opened().into_iter().map(|(label, _)| label).collect()
        }

        pub fn closed(&self) -> Vec<String> {
            self.calls
                .iter()
                .filter_map(|call| match call {
                    Call::Closed(label) => Some(label.clone()),
                    _ => None,
                })
                .collect()
        }

        pub fn emitted(&self) -> Vec<(String, WindowEvent)> {
            self.calls
                .iter()
                .filter_map(|call| match call {
                    Call::Emitted(label, event) => Some((label.clone(), event.clone())),
                    _ => None,
                })
                .collect()
        }
    }

    impl WindowManager for FakeWindows {
        fn is_open(&self, label: &str) -> bool {
            self.open.contains_key(label)
        }

        fn saved_window(&mut self, label: &str) -> Option<SavedWindow> {
            self.saved.get(label).copied()
        }

        fn monitors(&self) -> Monitors {
            self.monitors.clone()
        }

        fn build(&mut self, spec: &WindowSpec, opening: &Opening) -> Result<(), String> {
            if self.fail {
                return Err("the window api is down".to_string());
            }
            let (width, height) = opening
                .size
                .unwrap_or((spec.width as u32, spec.height as u32));
            let area = super::testing::primary();
            let centred = |area: Rect| Rect {
                x: area.x + (area.width.saturating_sub(width) / 2) as i32,
                y: area.y + (area.height.saturating_sub(height) / 2) as i32,
                width,
                height,
            };
            let bounds = match opening.position {
                Some(Placement::Restore(rect)) => Rect {
                    width,
                    height,
                    ..rect
                },
                Some(Placement::Centre(primary)) => centred(primary.unwrap_or(area)),
                // Where a compositor would put it: the top left corner.
                None => Rect {
                    x: area.x,
                    y: area.y,
                    width,
                    height,
                },
            };
            self.open.insert(spec.label.clone(), bounds);
            if opening.maximized {
                self.maximized.insert(spec.label.clone());
            }
            self.calls.push(Call::Opened(spec.clone(), *opening));
            Ok(())
        }

        fn save_positions(&mut self) -> Result<(), String> {
            let labels: Vec<String> = self.open.keys().cloned().collect();
            for label in labels {
                self.save_one(&label);
            }
            self.calls.push(Call::Saved);
            Ok(())
        }

        fn destroy(&mut self, label: &str) -> Result<(), String> {
            if self.fail {
                return Err("the window api is down".to_string());
            }
            self.open.remove(label);
            self.maximized.remove(label);
            self.calls.push(Call::Closed(label.to_string()));
            Ok(())
        }

        fn resize(&mut self, label: &str, width: f64, height: f64) -> Result<(), String> {
            if self.fail {
                return Err("the window api is down".to_string());
            }
            if let Some(bounds) = self.open.get_mut(label) {
                bounds.width = width as u32;
                bounds.height = height as u32;
            }
            self.calls
                .push(Call::Resized(label.to_string(), width, height));
            Ok(())
        }

        fn size(&self, label: &str) -> Option<(f64, f64)> {
            let bounds = self.open.get(label)?;
            Some((f64::from(bounds.width), f64::from(bounds.height)))
        }

        fn focus(&mut self, label: &str) -> Result<(), String> {
            if self.fail {
                return Err("the window api is down".to_string());
            }
            if self.open.contains_key(label) {
                self.calls.push(Call::Focused(label.to_string()));
            }
            Ok(())
        }

        fn emit(&mut self, label: &str, event: &WindowEvent) -> Result<(), String> {
            self.calls
                .push(Call::Emitted(label.to_string(), event.clone()));
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::*;
    use super::*;

    fn rect(x: i32, y: i32, width: u32, height: u32) -> Rect {
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    fn spec(label: &str) -> WindowSpec {
        WindowSpec {
            label: label.to_string(),
            url: format!("page.html?view={label}"),
            title: label.to_string(),
            width: 320.0,
            height: 220.0,
            min_width: 280.0,
            min_height: 180.0,
            max_width: None,
            remember_size: true,
            restore_position: true,
            style: WindowStyle::Overlay,
            shadow: true,
            transparent: false,
        }
    }

    #[test]
    fn the_plugins_saved_bounds_are_read_and_its_empty_default_is_not() {
        let text = r#"{
            "main": {"width": 1600, "height": 900, "x": 10, "y": 20},
            "draft-notes": {"width": 320, "height": 220, "x": -1800, "y": 200,
                "prev_x": 0, "prev_y": 0, "maximized": false, "visible": true,
                "decorated": false, "fullscreen": false},
            "game-replay": {"width": 1200, "height": 800, "x": 0, "y": 0,
                "prev_x": 300, "prev_y": 150, "maximized": true},
            "game-overlay": {"width": 0, "height": 0, "x": 0, "y": 0}
        }"#;
        assert_eq!(
            saved_window_in(text, "draft-notes"),
            Some(SavedWindow {
                bounds: rect(-1800, 200, 320, 220),
                maximized: false
            })
        );
        assert_eq!(
            saved_window_in(text, "game-replay"),
            Some(SavedWindow {
                bounds: rect(300, 150, 1200, 800),
                maximized: true
            }),
            "a maximized window keeps where it was before"
        );
        assert_eq!(saved_window_in(text, "game-overlay"), None, "never placed");
        assert_eq!(saved_window_in(text, "league-overlay"), None);
        assert_eq!(saved_window_in("not json", "main"), None);
    }

    #[test]
    fn without_position_restore_the_size_and_maximized_state_still_come_back() {
        let mut windows = FakeWindows::default();
        let mut normal = spec("a");
        normal.style = WindowStyle::Normal;
        normal.restore_position = false;
        windows.open(&normal).expect("open");
        assert_eq!(windows.opened(), vec![("a".to_string(), None)]);
        windows.user_moves("a", rect(400, 300, 1000, 700));
        windows.user_maximizes("a");
        windows.close("a").expect("close");

        windows.open(&normal).expect("reopen");
        let (_, opening) = windows.openings()[1];
        assert_eq!(
            opening,
            Opening {
                position: None,
                size: Some((1000, 700)),
                maximized: true
            }
        );
        // With position restore on, a saved window comes back where it was.
        windows.user_moves("a", rect(400, 300, 1000, 700));
        windows.close("a").expect("close");
        normal.restore_position = true;
        windows.open(&normal).expect("reopen");
        assert_eq!(
            windows.openings()[2].1.position,
            Some(Placement::Restore(rect(400, 300, 1000, 700)))
        );
    }

    #[test]
    fn a_window_that_does_not_remember_its_size_opens_at_its_spec_size() {
        let mut windows = FakeWindows::default();
        let mut fixed = spec("a");
        fixed.remember_size = false;
        windows.open(&fixed).expect("open");
        windows.user_moves("a", rect(10, 10, 900, 900));
        windows.close("a").expect("close");
        windows.open(&fixed).expect("reopen");
        let (_, opening) = windows.openings()[1];
        assert_eq!(opening.size, None);
        assert!(!opening.maximized);
        assert_eq!(
            opening.position,
            Some(Placement::Restore(rect(10, 10, 900, 900)))
        );
    }

    #[test]
    fn only_an_open_window_is_focused() {
        let mut windows = FakeWindows::default();
        windows.focus("a").expect("focus");
        windows.open(&spec("a")).expect("open");
        windows.focus("a").expect("focus");
        assert_eq!(windows.focused(), vec!["a".to_string()]);
    }

    /// A model of the main thread (an event loop running posted tasks) and
    /// the window-state plugin's cache lock: the main thread's move handler
    /// takes the lock; the plugin's save holds it while it reads windows,
    /// and a read from another thread waits for the main thread.
    struct MainThread {
        tasks: mpsc::Sender<Box<dyn FnOnce() + Send>>,
        thread: std::thread::JoinHandle<()>,
    }

    impl MainThread {
        fn start() -> MainThread {
            let (tasks, inbox) = mpsc::channel::<Box<dyn FnOnce() + Send>>();
            let thread = std::thread::spawn(move || {
                for task in inbox {
                    task();
                }
            });
            MainThread { tasks, thread }
        }

        fn post(&self) -> impl Fn(Box<dyn FnOnce() + Send>) -> Result<(), String> + use<> {
            let tasks = self.tasks.clone();
            move |task| tasks.send(task).map_err(|error| error.to_string())
        }
    }

    #[test]
    fn the_window_states_are_saved_on_the_main_thread_so_a_drag_cannot_hang_it() {
        let cache = Arc::new(std::sync::Mutex::new(()));
        let main = MainThread::start();
        let held = Arc::new(std::sync::Barrier::new(2));

        // The old order, saving from the worker: it holds the lock, then a
        // window read waits for the main thread, which is in a move
        // handler waiting for the lock. Bounded here; it hung for good.
        let guard = cache.lock().expect("lock");
        let handler_cache = Arc::clone(&cache);
        main.post()(Box::new(move || {
            let _moved = handler_cache.lock().expect("lock");
        }))
        .expect("post");
        let read = run_on_owner(main.post(), || "size", Duration::from_millis(300));
        assert!(read.is_err(), "the old order deadlocks");
        drop(guard);

        // The new order: the whole save runs on the main thread, its reads
        // inline, after or before a move handler, never against it.
        let handler_cache = Arc::clone(&cache);
        let in_handler = Arc::clone(&held);
        main.post()(Box::new(move || {
            in_handler.wait();
            let _moved = handler_cache.lock().expect("lock");
        }))
        .expect("post");
        held.wait();
        let save_cache = Arc::clone(&cache);
        let saved = run_on_owner(
            main.post(),
            move || {
                let _saving = save_cache.lock().expect("lock");
                "saved"
            },
            Duration::from_secs(5),
        );
        assert_eq!(saved, Ok("saved"));
        drop(main.tasks);
        main.thread.join().expect("main thread");
    }

    #[test]
    fn opening_an_open_window_and_closing_a_closed_one_do_nothing() {
        let mut windows = FakeWindows::default();
        assert_eq!(windows.open(&spec("a")), Ok(true));
        assert_eq!(windows.open(&spec("a")), Ok(false));
        assert_eq!(windows.opened_labels(), vec!["a".to_string()]);

        assert_eq!(windows.close("b"), Ok(false));
        assert!(windows.calls.iter().all(|call| *call != Call::Saved));
        assert_eq!(windows.close("a"), Ok(true));
        assert_eq!(windows.closed(), vec!["a".to_string()]);
    }

    #[test]
    fn a_close_the_manager_makes_saves_the_positions_first() {
        let mut windows = FakeWindows::default();
        windows.open(&spec("a")).expect("open");
        windows.user_moves("a", rect(100, 200, 320, 220));
        windows.close("a").expect("close");
        let tail: Vec<&Call> = windows.calls.iter().rev().take(2).collect();
        assert_eq!(tail, vec![&Call::Closed("a".to_string()), &Call::Saved]);
        assert_eq!(
            windows.saved.get("a").map(|saved| saved.bounds),
            Some(rect(100, 200, 320, 220))
        );
    }

    #[test]
    fn a_failing_window_api_is_an_error_not_a_panic() {
        let mut windows = FakeWindows {
            fail: true,
            ..FakeWindows::default()
        };
        assert!(windows.open(&spec("a")).is_err());
        assert!(!windows.is_open("a"));
    }
}
