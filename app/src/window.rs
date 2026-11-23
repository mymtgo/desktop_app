//! The main window and the tray (spec 6): close to tray when a tray exists,
//! otherwise closing quits. The app stays usable without a tray (GNOME has none
//! by default).

use std::sync::atomic::{AtomicBool, Ordering};

use tauri::image::Image;
use tauri::menu::{IsMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{App, AppHandle, Manager, Window, WindowEvent};

use crate::show_main;
use crate::updates::{UpdatePhase, UpdateStatus};

/// What this session's windowing allows, decided once at start-up (the
/// GDK backend cannot change while the app runs). Managed app state, read by
/// the overlay worker and the settings page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowSupport {
    /// Overlay windows can stay on top and be placed (overlay spec 5.3):
    /// false on a native Wayland backend.
    pub overlay_windows_available: bool,
}

impl WindowSupport {
    /// The app runs on a native Wayland backend, where windows cannot be
    /// placed: the only case overlay windows are unavailable (Linux).
    /// Windows that still open there (the replay window) skip their saved
    /// position (`WindowSpec::restore_position`).
    pub fn native_wayland(&self) -> bool {
        !self.overlay_windows_available
    }

    /// What `os` with these display variables allows.
    pub fn from_env(os: overlay::Os, env: &overlay::SessionEnv) -> WindowSupport {
        WindowSupport {
            overlay_windows_available: overlay::overlay_windows_available(os, env),
        }
    }

    /// Read the session's display variables (reading is safe; nothing is
    /// written) and decide.
    pub fn detect() -> WindowSupport {
        let read = |name: &str| std::env::var(name).ok();
        let env = overlay::SessionEnv {
            wayland_display: read("WAYLAND_DISPLAY"),
            wayland_socket: read("WAYLAND_SOCKET"),
            gdk_backend: read("GDK_BACKEND"),
            display: read("DISPLAY"),
        };
        WindowSupport::from_env(overlay::Os::from_name(std::env::consts::OS), &env)
    }
}

pub struct WindowPrefs {
    pub close_to_tray: AtomicBool,
    pub tray_available: AtomicBool,
}

impl Default for WindowPrefs {
    fn default() -> Self {
        Self {
            close_to_tray: AtomicBool::new(true),
            tray_available: AtomicBool::new(false),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseAction {
    Hide,
    Close,
}

pub fn close_action(close_to_tray: bool, tray_available: bool) -> CloseAction {
    if close_to_tray && tray_available {
        CloseAction::Hide
    } else {
        CloseAction::Close
    }
}

pub fn set_close_to_tray(app: &AppHandle, on: bool) {
    if let Some(prefs) = app.try_state::<WindowPrefs>() {
        prefs.close_to_tray.store(on, Ordering::SeqCst);
    }
}

/// Hide the main window instead of closing it when close to tray applies.
/// Closing the data console window stops its running query or export.
pub fn on_window_event(window: &Window, event: &WindowEvent) {
    if crate::console::window::stops_console_work(window.label(), event)
        && let Some(console) = window
            .app_handle()
            .try_state::<crate::console::ConsoleState>()
    {
        console.cancel();
    }
    let WindowEvent::CloseRequested { api, .. } = event else {
        return;
    };
    if window.label() != "main" {
        return;
    }
    let Some(prefs) = window.app_handle().try_state::<WindowPrefs>() else {
        return;
    };
    let action = close_action(
        prefs.close_to_tray.load(Ordering::SeqCst),
        prefs.tray_available.load(Ordering::SeqCst),
    );
    if action == CloseAction::Hide {
        api.prevent_close();
        let _ = window.hide();
    }
}

pub const TRAY_ID: &str = "main-tray";

/// The tray icon: the 0.x book, with a badge while an update is ready.
pub fn tray_icon(update_ready: bool) -> Image<'static> {
    if update_ready {
        tauri::include_image!("icons/tray/tray-update.png")
    } else {
        tauri::include_image!("icons/tray/tray.png")
    }
}

/// The tray tooltip: the product name, and the version while an update is
/// ready (0.x `CreateTrayMenuBar::tooltip`).
pub fn tray_tooltip(product: &str, ready_version: Option<&str>) -> String {
    match ready_version {
        Some(version) => format!("{product}: update ready (v{version})"),
        None => product.to_string(),
    }
}

/// What the tray shows for an update: the badge icon and the tooltip. The
/// menu is never part of it (0.x `SyncTrayUpdateState` leaves the context
/// menu alone).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrayUpdateState {
    pub badge: bool,
    pub tooltip: String,
}

/// The tray's update state for `status`: badged only once an update is ready.
pub fn tray_update_state(product: &str, status: &UpdateStatus) -> TrayUpdateState {
    let ready = if status.phase == UpdatePhase::Ready {
        status.available.as_deref()
    } else {
        None
    };
    TrayUpdateState {
        badge: ready.is_some(),
        tooltip: tray_tooltip(product, ready),
    }
}

/// What a tray click or menu item does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayAction {
    OpenMain,
    Quit,
}

/// One entry of the tray menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrayMenuItem {
    Item { id: &'static str, label: String },
    Separator,
}

/// The tray menu: "Open <product>", a separator, "Quit <product>" (0.x's
/// labels with the product name, release spec 6.3).
pub fn tray_menu_items(product: &str) -> Vec<TrayMenuItem> {
    vec![
        TrayMenuItem::Item {
            id: "open",
            label: format!("Open {product}"),
        },
        TrayMenuItem::Separator,
        TrayMenuItem::Item {
            id: "quit",
            label: format!("Quit {product}"),
        },
    ]
}

/// The action of a tray menu item.
pub fn menu_action(id: &str) -> Option<TrayAction> {
    match id {
        "open" => Some(TrayAction::OpenMain),
        "quit" => Some(TrayAction::Quit),
        _ => None,
    }
}

/// The action of a click on the tray icon: a left click (on release) opens
/// and focuses the main window (0.x `HandleTrayClick`); a right click shows
/// the menu, which the tray does by itself.
pub fn click_action(button: MouseButton, state: MouseButtonState) -> Option<TrayAction> {
    if button == MouseButton::Left && state == MouseButtonState::Up {
        Some(TrayAction::OpenMain)
    } else {
        None
    }
}

/// Whether a left click opens the menu. Only on Linux, where appindicator
/// trays send no click events, so the menu is the only way in (ruling 9).
pub fn menu_on_left_click(os: &str) -> bool {
    os == "linux"
}

fn run_tray_action(app: &AppHandle, action: Option<TrayAction>) {
    match action {
        Some(TrayAction::OpenMain) => show_main(app),
        Some(TrayAction::Quit) => app.exit(0),
        None => {}
    }
}

/// Show or clear "update ready" on the tray and, on Windows, as an overlay on
/// the taskbar button, which shows it even when there is no tray (spec 9).
pub fn show_update_ready(app: &AppHandle, status: &UpdateStatus) {
    let product = app.package_info().name.clone();
    let state = tray_update_state(&product, status);
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        let _ = tray.set_icon(Some(tray_icon(state.badge)));
        let _ = tray.set_tooltip(Some(state.tooltip));
    }
    #[cfg(windows)]
    if let Some(window) = app.get_webview_window("main") {
        let overlay = state
            .badge
            .then(|| tauri::include_image!("icons/tray/overlay-update.png"));
        let _ = window.set_overlay_icon(overlay);
    }
}

/// Zoom the main window's page to the interface scale. Other windows keep
/// 100%: overlays are sized in pixels for OBS.
pub fn apply_interface_scale(app: &AppHandle, percent: u32) {
    if let Some(window) = app.get_webview_window("main")
        && let Err(error) = window.set_zoom(crate::settings::zoom_factor(percent))
    {
        tracing::warn!(%error, percent, "could not set the interface scale");
    }
}

/// Build the tray, turning a panic into an error. On Linux without
/// libayatana-appindicator the tray library panics instead of failing, and
/// the app must still start (closing then quits).
pub fn build_tray_safely<F: FnOnce() -> tauri::Result<()>>(build: F) -> Result<(), String> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(build)) {
        Ok(Ok(())) => Ok(()),
        Ok(Err(error)) => Err(error.to_string()),
        Err(_) => Err("the tray library panicked (no system tray support)".to_string()),
    }
}

/// The tray icon with Open and Quit (release spec 6.3). A left click opens
/// the main window, a right click shows the menu (on Linux any click shows
/// it). Fails on desktops without a tray.
pub fn build_tray(app: &App) -> tauri::Result<()> {
    let product = app.package_info().name.clone();
    // Built from `tray_menu_items`, so the tested list is the menu.
    let mut items: Vec<Box<dyn IsMenuItem<tauri::Wry>>> = Vec::new();
    for item in tray_menu_items(&product) {
        match item {
            TrayMenuItem::Item { id, label } => {
                items.push(Box::new(MenuItem::with_id(
                    app,
                    id,
                    label,
                    true,
                    None::<&str>,
                )?));
            }
            TrayMenuItem::Separator => items.push(Box::new(PredefinedMenuItem::separator(app)?)),
        }
    }
    let item_refs: Vec<&dyn IsMenuItem<tauri::Wry>> =
        items.iter().map(|item| item.as_ref()).collect();
    let menu = Menu::with_items(app, &item_refs)?;
    TrayIconBuilder::with_id(TRAY_ID)
        .tooltip(tray_tooltip(&product, None))
        .icon(tray_icon(false))
        .menu(&menu)
        .show_menu_on_left_click(menu_on_left_click(std::env::consts::OS))
        .on_menu_event(|app, event| run_tray_action(app, menu_action(event.id.as_ref())))
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button,
                button_state,
                ..
            } = event
            {
                run_tray_action(tray.app_handle(), click_action(button, button_state));
            }
        })
        .build(app)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::updates::UpdateStatus;
    use tauri::tray::{MouseButton, MouseButtonState};

    #[test]
    fn native_wayland_is_where_overlay_windows_are_unavailable() {
        let wayland = WindowSupport {
            overlay_windows_available: false,
        };
        let x11 = WindowSupport {
            overlay_windows_available: true,
        };
        assert!(wayland.native_wayland());
        assert!(!x11.native_wayland());
    }

    #[test]
    fn closing_hides_only_when_asked_and_a_tray_exists() {
        assert_eq!(close_action(true, true), CloseAction::Hide);
        assert_eq!(
            close_action(true, false),
            CloseAction::Close,
            "no tray: never strand the app"
        );
        assert_eq!(close_action(false, true), CloseAction::Close);
        assert_eq!(close_action(false, false), CloseAction::Close);
    }

    #[test]
    fn preferences_start_with_close_to_tray_on_and_no_tray() {
        let prefs = WindowPrefs::default();
        assert!(prefs.close_to_tray.load(Ordering::SeqCst));
        assert!(!prefs.tray_available.load(Ordering::SeqCst));
    }

    #[test]
    fn a_tray_that_panics_is_an_error_not_a_crash() {
        let failed = build_tray_safely(|| panic!("libayatana-appindicator not found"));
        assert!(failed.is_err());
        assert!(build_tray_safely(|| Ok(())).is_ok());
    }

    // 0.x CreateTrayMenuBarTest: "gives the tray Open item an event so it
    // works while the main window is hidden".
    #[test]
    fn the_tray_menu_is_open_a_separator_and_quit_with_the_product_name() {
        assert_eq!(
            tray_menu_items("MyMTGO"),
            vec![
                TrayMenuItem::Item {
                    id: "open",
                    label: "Open MyMTGO".to_string()
                },
                TrayMenuItem::Separator,
                TrayMenuItem::Item {
                    id: "quit",
                    label: "Quit MyMTGO".to_string()
                },
            ]
        );
        assert_eq!(menu_action("open"), Some(TrayAction::OpenMain));
        assert_eq!(menu_action("quit"), Some(TrayAction::Quit));
        assert_eq!(menu_action("other"), None);
    }

    // 0.x HandleTrayClickTest: "focuses or opens the main window on tray click".
    #[test]
    fn a_left_click_on_the_tray_opens_the_main_window() {
        assert_eq!(
            click_action(MouseButton::Left, MouseButtonState::Up),
            Some(TrayAction::OpenMain)
        );
        assert_eq!(
            click_action(MouseButton::Left, MouseButtonState::Down),
            None
        );
        assert_eq!(
            click_action(MouseButton::Right, MouseButtonState::Up),
            None,
            "a right click shows the menu"
        );
    }

    // 0.x HandleTrayClickTest: "focuses or opens the main window when the
    // tray Open item is clicked".
    #[test]
    fn the_open_item_opens_the_main_window() {
        let open = tray_menu_items("Qapp")
            .into_iter()
            .find_map(|item| match item {
                TrayMenuItem::Item { id, label } if label == "Open Qapp" => Some(id),
                _ => None,
            })
            .expect("an Open item");
        assert_eq!(menu_action(open), Some(TrayAction::OpenMain));
    }

    #[test]
    fn the_menu_opens_on_left_click_only_on_linux() {
        // Linux appindicator trays send no click events (ruling 9).
        assert!(menu_on_left_click("linux"));
        assert!(!menu_on_left_click("windows"));
        assert!(!menu_on_left_click("macos"));
    }

    fn ready_status(version: &str) -> UpdateStatus {
        let mut status = UpdateStatus::new("1.0.0-beta.2", true);
        status.begin_check(1);
        status.found(version);
        status.downloaded();
        status
    }

    // 0.x TrayUpdateStateTest: "pushes the badge icon and tooltip to a
    // running tray".
    #[test]
    fn a_ready_update_badges_the_tray_and_names_the_version() {
        assert_eq!(
            tray_update_state("MyMTGO", &ready_status("1.0.0")),
            TrayUpdateState {
                badge: true,
                tooltip: "MyMTGO: update ready (v1.0.0)".to_string()
            }
        );
    }

    // 0.x TrayUpdateStateTest: "restores the normal icon and tooltip when
    // nothing is ready".
    #[test]
    fn nothing_ready_restores_the_plain_icon_and_tooltip() {
        let mut status = UpdateStatus::new("1.0.0-beta.2", true);
        assert_eq!(
            tray_update_state("Qapp", &status),
            TrayUpdateState {
                badge: false,
                tooltip: "Qapp".to_string()
            }
        );
        status.begin_check(1);
        status.found("1.0.0");
        assert!(
            !tray_update_state("MyMTGO", &status).badge,
            "downloading is not ready"
        );
        assert_eq!(tray_tooltip("MyMTGO", None), "MyMTGO");
    }

    // 0.x TrayUpdateStateTest: "syncs the tray icon when an update finishes
    // downloading": the state follows the status `publish` sends after the
    // download.
    #[test]
    fn the_tray_follows_a_finished_download() {
        let mut status = UpdateStatus::new("1.0.0-beta.2", true);
        status.begin_check(1);
        status.found("1.0.0");
        assert!(!tray_update_state("MyMTGO", &status).badge);
        status.downloaded();
        assert!(tray_update_state("MyMTGO", &status).badge);
    }

    // 0.x TrayUpdateStateTest: "never replaces the context menu of a running
    // tray". The update state is the icon and tooltip only (this pattern
    // names every field), and the menu depends on the product name alone.
    #[test]
    fn an_update_never_replaces_the_tray_menu() {
        let TrayUpdateState { badge, tooltip } =
            tray_update_state("MyMTGO", &ready_status("1.0.0"));
        assert!(badge);
        assert!(tooltip.contains("update ready"));
        assert_eq!(tray_menu_items("MyMTGO").len(), 3);
    }

    #[test]
    fn the_tray_icons_are_small_and_the_update_one_differs() {
        let plain = tray_icon(false);
        let ready = tray_icon(true);
        assert_eq!((plain.width(), plain.height()), (32, 32));
        assert_eq!((ready.width(), ready.height()), (32, 32));
        assert_ne!(plain.rgba(), ready.rgba());
    }
}
