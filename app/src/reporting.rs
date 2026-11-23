//! Error reports to Sentry (spec 9). Off unless the build sets
//! `MYMTGO_SENTRY_DSN` (CI release builds do; local builds do not). Every
//! report and breadcrumb is scrubbed of player names and the home folder,
//! and carries no user and no computer name. A report that cannot be
//! scrubbed right now is dropped, never sent as it is.

use std::sync::Arc;
use std::time::Duration;

use sentry::integrations::tracing::{EventFilter, default_event_filter};
use sentry::protocol::{Breadcrumb, Event};
use sentry::{ClientInitGuard, ClientOptions};
use serde::{Deserialize, Serialize};

const DSN: Option<&str> = option_env!("MYMTGO_SENTRY_DSN");
/// The panic hook logs under this target; the panic integration reports the
/// panic itself, so the tracing layer ignores these lines.
pub const PANIC_TARGET: &str = "mymtgo::panic";

/// Start reporting, when this build has a DSN. Keep the guard alive.
pub fn init(version: &str) -> Option<ClientInitGuard> {
    let dsn = DSN.map(str::trim).filter(|dsn| !dsn.is_empty())?;
    Some(sentry::init((dsn, options(version))))
}

pub fn options(version: &str) -> ClientOptions {
    let home = home_folder();
    let crumb_home = home.clone();
    // `ClientOptions` is non-exhaustive in 0.49, so set fields one by one.
    let mut options = ClientOptions::default();
    options.release = Some(format!("mymtgo-desktop@{version}").into());
    // `beta` or `production` by the version, not the name (spec 13.1).
    options.environment = Some(crate::identity::sentry_environment(version).into());
    options.send_default_pii = false;
    options.before_send = Some(Arc::new(move |event| scrub_event(event, home.as_deref())));
    options.before_breadcrumb = Some(Arc::new(move |crumb| {
        scrub_breadcrumb(crumb, crumb_home.as_deref())
    }));
    options
}

/// A report without names, home folder, user or computer name.
pub fn scrub_event(mut event: Event<'static>, home: Option<&str>) -> Option<Event<'static>> {
    if is_tray_panic(&event) {
        return None;
    }
    // The computer's name often holds a person's name.
    event.server_name = None;
    event.user = None;
    let known = crate::scrub::known()?;
    let mut value = serde_json::to_value(&event).ok()?;
    crate::scrub::scrub_json(&mut value, &known, home);
    serde_json::from_value(value).ok()
}

/// The tray library's panic on Linux without libayatana-appindicator. The app
/// catches it and starts without a tray (`window::build_tray_safely`), so it
/// is not a report: it would reach Sentry once per start.
fn is_tray_panic(event: &Event<'_>) -> bool {
    let mentions = |text: &str| text.to_ascii_lowercase().contains("appindicator");
    event.message.as_deref().is_some_and(mentions)
        || event
            .logentry
            .as_ref()
            .is_some_and(|entry| mentions(&entry.message))
        || event
            .exception
            .values
            .iter()
            .any(|exception| exception.value.as_deref().is_some_and(mentions))
}

pub fn scrub_breadcrumb(crumb: Breadcrumb, home: Option<&str>) -> Option<Breadcrumb> {
    let known = crate::scrub::known()?;
    let mut value = serde_json::to_value(&crumb).ok()?;
    crate::scrub::scrub_json(&mut value, &known, home);
    serde_json::from_value(value).ok()
}

/// Which log lines Sentry hears: errors become reports, warnings and info
/// become breadcrumbs (the default), except the panic hook's line and the
/// updater plugin's errors.
pub fn event_filter(metadata: &tracing::Metadata<'_>) -> EventFilter {
    let target = metadata.target();
    if is_ignored_target(target) {
        EventFilter::Ignore
    } else if is_breadcrumb_only_target(target) {
        EventFilter::Breadcrumb
    } else {
        default_event_filter(metadata)
    }
}

fn is_ignored_target(target: &str) -> bool {
    target == PANIC_TARGET
}

/// The updater plugin logs a failed check (most often no network) at error.
/// `updates.rs` logs the same failure as a warning and retries, so the
/// plugin's line is a breadcrumb, not a report.
fn is_breadcrumb_only_target(target: &str) -> bool {
    target.starts_with("tauri_plugin_updater")
}

/// The user's home folder, replaced with `~` in reports.
pub fn home_folder() -> Option<String> {
    let name = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    std::env::var(name).ok().filter(|home| !home.is_empty())
}

/// Send what is queued before the process ends.
pub fn flush() {
    if let Some(client) = sentry::Hub::current().client() {
        client.flush(Some(Duration::from_secs(2)));
    }
}

/// `MYMTGO_SENTRY_SELFTEST=1` sends one message at startup, so the owner can
/// check that a release build reports. Returns whether it sent one.
pub fn self_test(enabled: bool) -> bool {
    if enabled {
        sentry::capture_message("MyMTGO Sentry self test", sentry::Level::Info);
    }
    enabled
}

/// An error the UI caught (Vue, the window, an unhandled promise).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct UiError {
    /// Where it was caught, for example `vue: render function` or `promise`.
    pub source: String,
    pub message: String,
    pub stack: Option<String>,
    pub route: Option<String>,
}

/// Log a UI error. As a `tracing` error it is written to the log files and,
/// scrubbed, sent to Sentry: one path for every error.
pub fn ui_error(report: &UiError) {
    tracing::error!(
        target: "mymtgo::ui",
        source = %report.source,
        route = report.route.as_deref().unwrap_or(""),
        stack = report.stack.as_deref().unwrap_or(""),
        "{}",
        report.message
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use sentry::protocol::{Breadcrumb, Event, Level, User};

    #[test]
    fn reports_name_the_release_and_send_no_personal_data() {
        let options = options("1.0.0-beta.2");
        assert_eq!(
            options.release.as_deref(),
            Some("mymtgo-desktop@1.0.0-beta.2")
        );
        assert!(!options.send_default_pii);
        assert!(options.before_send.is_some());
        assert!(options.before_breadcrumb.is_some());
    }

    #[test]
    fn prerelease_builds_report_as_beta_and_stable_as_production() {
        assert_eq!(options("1.0.0-beta.2").environment.as_deref(), Some("beta"));
        assert_eq!(options("1.0.0").environment.as_deref(), Some("production"));
    }

    #[test]
    fn a_report_loses_names_the_home_folder_and_the_computer_name() {
        crate::scrub::remember(["Qreporter".to_string()]);
        let event = Event {
            message: Some(
                "failed for Qreporter in /home/qhome/logs, Username: Qstranger (55)".to_string(),
            ),
            server_name: Some("qhome-laptop".into()),
            user: Some(User {
                username: Some("Qreporter".to_string()),
                ..User::default()
            }),
            level: Level::Error,
            ..Event::default()
        };
        let scrubbed = scrub_event(event, Some("/home/qhome")).expect("kept");
        assert_eq!(
            scrubbed.message.as_deref(),
            Some("failed for <player> in ~/logs, Username: <player> (55)")
        );
        assert!(scrubbed.server_name.is_none());
        assert!(scrubbed.user.is_none());
        assert_eq!(scrubbed.level, Level::Error);
    }

    #[test]
    fn the_caught_tray_panic_is_not_reported() {
        crate::scrub::remember(["Qtray".to_string()]);
        let panic = Event {
            exception: vec![sentry::protocol::Exception {
                ty: "panic".to_string(),
                value: Some(
                    "Failed to load ayatana-appindicator3 or appindicator3 dynamic library"
                        .to_string(),
                ),
                ..sentry::protocol::Exception::default()
            }]
            .into(),
            level: Level::Fatal,
            ..Event::default()
        };
        assert!(scrub_event(panic, None).is_none());
        let other = Event {
            message: Some("failed to read the log".to_string()),
            ..Event::default()
        };
        assert!(scrub_event(other, None).is_some());
    }

    #[test]
    fn breadcrumbs_are_scrubbed_too() {
        crate::scrub::remember(["Qcrumbname".to_string()]);
        let crumb = Breadcrumb {
            message: Some("opened Qcrumbname".to_string()),
            ..Breadcrumb::default()
        };
        let scrubbed = scrub_breadcrumb(crumb, None).expect("kept");
        assert_eq!(scrubbed.message.as_deref(), Some("opened <player>"));
    }

    #[test]
    fn panic_lines_are_left_to_the_panic_integration() {
        assert!(is_ignored_target(PANIC_TARGET));
        assert!(!is_ignored_target("mymtgo::ui"));
        assert!(!is_ignored_target("tracker::supervisor"));
    }

    #[test]
    fn the_updater_plugin_errors_are_breadcrumbs_not_reports() {
        assert!(is_breadcrumb_only_target("tauri_plugin_updater::updater"));
        assert!(!is_breadcrumb_only_target("app::updates"));
        assert!(!is_breadcrumb_only_target("mymtgo::ui"));
    }

    #[test]
    fn the_installed_callbacks_scrub_what_they_are_given() {
        crate::scrub::remember(["Qwired".to_string()]);
        let options = options("0.1.0");

        let event = Event {
            message: Some("failed for Qwired".to_string()),
            server_name: Some("qhome-laptop".into()),
            ..Event::default()
        };
        let before_send = options.before_send.as_ref().expect("installed");
        let sent = before_send(event).expect("kept");
        assert_eq!(sent.message.as_deref(), Some("failed for <player>"));
        assert!(sent.server_name.is_none());

        let crumb = Breadcrumb {
            message: Some("opened Qwired".to_string()),
            ..Breadcrumb::default()
        };
        let before_breadcrumb = options.before_breadcrumb.as_ref().expect("installed");
        let kept = before_breadcrumb(crumb).expect("kept");
        assert_eq!(kept.message.as_deref(), Some("opened <player>"));
    }
}
