//! The data console window: one decorated window, `console`, opened from
//! the main window. Opening it while it is open brings it to the front.
//! Like the replay window it is built off the IPC thread and placed by the
//! window manager from its saved bounds (its size only on native Wayland).

use std::sync::Mutex;

use tauri::{AppHandle, Manager, WindowEvent};

use super::off_ipc;
use crate::bridge::lock;
use crate::overlay::windows::{TauriWindows, WindowManager, WindowSpec, WindowStyle};
use crate::window::WindowSupport;

/// The console window's label.
pub const CONSOLE: &str = "console";

/// The console window: "Data console", 1100 x 720, at least 800 x 500,
/// decorated, its size remembered; its saved position is restored except
/// on native Wayland, where the compositor places windows.
pub fn console_spec(native_wayland: bool) -> WindowSpec {
    WindowSpec {
        label: CONSOLE.to_string(),
        url: "console.html".to_string(),
        title: "Data console".to_string(),
        width: 1100.0,
        height: 720.0,
        min_width: 800.0,
        min_height: 500.0,
        max_width: None,
        remember_size: true,
        restore_position: !native_wayland,
        style: WindowStyle::Normal,
        shadow: true,
        transparent: false,
    }
}

/// Opens the console window, or focuses it when it is open already.
pub fn open_console(windows: &mut impl WindowManager, native_wayland: bool) -> Result<(), String> {
    if !windows.open(&console_spec(native_wayland))? {
        windows.focus(CONSOLE)?;
    }
    Ok(())
}

/// Whether `event` on the window `label` closes the console window, so its
/// running query or export is stopped (as `console_cancel` does). Only the
/// destroy counts: a close request can still be refused.
pub fn stops_console_work(label: &str, event: &WindowEvent) -> bool {
    label == CONSOLE && matches!(event, WindowEvent::Destroyed)
}

/// One console window open at a time: two quick clicks never build two.
static OPENING: Mutex<()> = Mutex::new(());

/// Open the data console window. Async: the window is built on a worker
/// thread.
#[tauri::command]
#[specta::specta]
pub async fn open_console_window(app: AppHandle) -> Result<(), String> {
    let native_wayland = app
        .try_state::<WindowSupport>()
        .is_some_and(|support| support.native_wayland());
    off_ipc(move || {
        let _one_at_a_time = lock(&OPENING);
        let mut windows = TauriWindows::new(app);
        open_console(&mut windows, native_wayland)
    })
    .await?
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::overlay::windows::testing::FakeWindows;

    #[test]
    fn console_spec_values() {
        let spec = console_spec(false);
        assert_eq!(spec.label, "console");
        assert_eq!(spec.url, "console.html");
        assert_eq!(spec.title, "Data console");
        assert_eq!((spec.width, spec.height), (1100.0, 720.0));
        assert_eq!((spec.min_width, spec.min_height), (800.0, 500.0));
        assert_eq!(spec.max_width, None);
        assert_eq!(spec.style, WindowStyle::Normal);
        assert!(spec.remember_size);
        assert!(spec.restore_position);
        assert!(
            !console_spec(true).restore_position,
            "native Wayland places it"
        );
        assert!(spec.shadow);
    }

    #[test]
    fn closing_the_console_window_stops_its_work() {
        assert!(stops_console_work(CONSOLE, &WindowEvent::Destroyed));
        assert!(!stops_console_work("main", &WindowEvent::Destroyed));
        assert!(!stops_console_work(CONSOLE, &WindowEvent::Focused(false)));
    }

    #[test]
    fn opening_it_again_focuses_the_open_window() {
        let mut windows = FakeWindows::default();
        open_console(&mut windows, false).expect("open");
        open_console(&mut windows, false).expect("open again");
        assert_eq!(windows.opened_specs().len(), 1, "one window");
        assert_eq!(windows.focused(), vec![CONSOLE.to_string()]);
    }

    #[test]
    fn the_capability_names_the_console_window_and_only_its_permissions() {
        let capability: serde_json::Value =
            serde_json::from_str(include_str!("../../capabilities/console.json"))
                .expect("console.json parses");
        assert_eq!(capability["identifier"], "console");
        assert_eq!(capability["windows"], serde_json::json!([CONSOLE]));
        assert_eq!(
            capability["permissions"],
            serde_json::json!([
                "core:default",
                "allow-report-ui-error",
                "allow-console-run",
                "allow-console-cancel",
                "allow-console-schema",
                "allow-console-parameters",
                "allow-console-export",
                "allow-console-saved-list",
                "allow-console-saved-put",
                "allow-console-saved-delete",
                "allow-open-database-folder"
            ])
        );
    }
}
