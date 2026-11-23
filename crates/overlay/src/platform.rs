//! Whether overlay windows are available (overlay spec 5.3, ruling 2).
//!
//! On a native Wayland backend a client can neither keep a window above
//! others nor place it, so an overlay would open where the compositor puts it
//! and fall behind MTGO. v1 does not switch the GDK backend itself; under
//! native Wayland the three overlay windows are simply unavailable. Starting
//! the app with `GDK_BACKEND=x11` (XWayland) makes them available.
//!
//! Decided once at start-up: the app reads the four variables and passes
//! their values in.

/// The operating system the app runs on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    Windows,
    MacOs,
    Linux,
    Other,
}

impl Os {
    /// The OS from Rust's name for it (`std::env::consts::OS`).
    pub fn from_name(name: &str) -> Os {
        match name {
            "windows" => Os::Windows,
            "macos" => Os::MacOs,
            "linux" => Os::Linux,
            _ => Os::Other,
        }
    }
}

/// The session's display variables, `None` when unset.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionEnv {
    pub wayland_display: Option<String>,
    pub wayland_socket: Option<String>,
    pub gdk_backend: Option<String>,
    pub display: Option<String>,
}

/// Whether the overlay windows can open. Always on every OS but Linux. On
/// Linux, not when GTK picks its Wayland backend: `WAYLAND_DISPLAY` or
/// `WAYLAND_SOCKET` is non-empty and NOT (the first entry of `GDK_BACKEND` is
/// `x11` and `DISPLAY` is non-empty). An unset `GDK_BACKEND`, `*`, or a list
/// starting with `wayland` is native Wayland on a Wayland session.
pub fn overlay_windows_available(os: Os, env: &SessionEnv) -> bool {
    if os != Os::Linux {
        return true;
    }
    let wayland_session = is_set(&env.wayland_display) || is_set(&env.wayland_socket);
    if !wayland_session {
        return true;
    }
    first_backend(&env.gdk_backend) == "x11" && is_set(&env.display)
}

fn is_set(value: &Option<String>) -> bool {
    value.as_deref().is_some_and(|value| !value.is_empty())
}

/// The first entry of a `GDK_BACKEND` list (`x11,wayland` gives `x11`).
fn first_backend(value: &Option<String>) -> String {
    let value = value.as_deref().unwrap_or("");
    value.split(',').next().unwrap_or("").trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(
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
    fn windows_and_macos_always_have_overlays() {
        let wayland = env(Some("wayland-0"), None, None, None);

        assert!(overlay_windows_available(Os::Windows, &wayland));
        assert!(overlay_windows_available(Os::MacOs, &wayland));
        assert!(overlay_windows_available(
            Os::Windows,
            &SessionEnv::default()
        ));
    }

    #[test]
    fn a_plain_x11_session_has_overlays() {
        assert!(overlay_windows_available(
            Os::Linux,
            &env(None, None, None, Some(":0"))
        ));
        // An empty variable counts as unset.
        assert!(overlay_windows_available(
            Os::Linux,
            &env(Some(""), Some(""), None, Some(":0"))
        ));
    }

    #[test]
    fn a_wayland_session_without_gdk_backend_has_no_overlays() {
        assert!(!overlay_windows_available(
            Os::Linux,
            &env(Some("wayland-0"), None, None, Some(":0"))
        ));
    }

    #[test]
    fn gdk_backend_x11_with_display_has_overlays() {
        assert!(overlay_windows_available(
            Os::Linux,
            &env(Some("wayland-0"), None, Some("x11"), Some(":0"))
        ));
        assert!(overlay_windows_available(
            Os::Linux,
            &env(Some("wayland-0"), None, Some("x11,wayland"), Some(":1"))
        ));
    }

    #[test]
    fn gdk_backend_x11_without_display_has_no_overlays() {
        assert!(!overlay_windows_available(
            Os::Linux,
            &env(Some("wayland-0"), None, Some("x11"), None)
        ));
        assert!(!overlay_windows_available(
            Os::Linux,
            &env(Some("wayland-0"), None, Some("x11"), Some(""))
        ));
    }

    #[test]
    fn gdk_backend_wayland_first_or_star_has_no_overlays() {
        for backend in ["wayland", "wayland,x11", "*", ""] {
            assert!(
                !overlay_windows_available(
                    Os::Linux,
                    &env(Some("wayland-0"), None, Some(backend), Some(":0"))
                ),
                "{backend}"
            );
        }
    }

    #[test]
    fn wayland_socket_alone_counts_as_wayland() {
        assert!(!overlay_windows_available(
            Os::Linux,
            &env(None, Some("3"), None, Some(":0"))
        ));
    }

    #[test]
    fn the_os_comes_from_rust_s_name_for_it() {
        assert_eq!(Os::from_name("windows"), Os::Windows);
        assert_eq!(Os::from_name("macos"), Os::MacOs);
        assert_eq!(Os::from_name("linux"), Os::Linux);
        assert_eq!(Os::from_name("freebsd"), Os::Other);
    }
}
