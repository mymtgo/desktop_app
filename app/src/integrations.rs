//! The Integrations switches (endstep spec 8.1): Arena and Endstep. Each is
//! one stored setting with its own command, so a general settings save never
//! undoes it (`Settings::keep_command_owned`).
//!
//! Arena off: the tracker discovers and reads no Arena log and the header
//! hides Arena. Endstep off: the helper is unregistered from the browsers and
//! the inbox is left alone; stored captures and matches stay.

use endstep_capture::{Connection, LastSeen, connection, parse_last_seen};
use serde::{Deserialize, Serialize};
use std::path::Path;
use store::platform::PlatformPresence;
use tauri::{AppHandle, Manager};
use tauri_specta::Event;

use crate::bridge::lock;
use crate::commands::{
    AppState, CommandError, Startup, off_async_workers, read_settings, ready, restart_tracker,
    storage,
};
use crate::events::SettingsChanged;
use crate::logging::Timer;
use crate::settings::{ARENA_ENABLED, ENDSTEP_ENABLED, Settings};

/// Whether Arena is on: the stored choice, else on for anyone who already
/// has Arena data and off for a fresh install (spec 8.1).
pub fn arena_effective(settings: &Settings, presence: &PlatformPresence) -> bool {
    store::platform::arena_on(
        store::platform::PlatformSwitches {
            arena: settings.arena_enabled,
            endstep: settings.endstep_enabled,
        },
        *presence,
    )
}

/// An integration the page can switch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "lowercase")]
pub enum IntegrationView {
    Arena,
    Endstep,
}

/// Whether the browser extension has been in touch (endstep spec 8.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ConnectionView {
    NeverSeen,
    #[serde(rename_all = "camelCase")]
    Recent {
        browser: String,
        extension: String,
        #[specta(type = specta_typescript::Number)]
        at_ms: i64,
    },
    #[serde(rename_all = "camelCase")]
    Stale {
        browser: String,
        extension: String,
        #[specta(type = specta_typescript::Number)]
        at_ms: i64,
    },
}

impl ConnectionView {
    pub fn from_connection(connection: Connection) -> ConnectionView {
        match connection {
            Connection::NeverSeen => ConnectionView::NeverSeen,
            Connection::Recent { seen } => ConnectionView::Recent {
                browser: seen.browser,
                extension: seen.extension,
                at_ms: seen.at,
            },
            Connection::Stale { seen } => ConnectionView::Stale {
                browser: seen.browser,
                extension: seen.extension,
                at_ms: seen.at,
            },
        }
    }
}

/// `last_seen.json` beside the inbox (`<data dir>/endstep/last_seen.json`);
/// `None` when it is missing or unreadable.
pub fn read_last_seen(data_dir: &Path) -> Option<LastSeen> {
    let text = std::fs::read_to_string(data_dir.join("endstep").join("last_seen.json")).ok()?;
    parse_last_seen(&text)
}

/// What the Endstep card says about the helper.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct EndstepStatusView {
    /// The last install found no helper beside MyMTGO (a dev build that did
    /// not build `endstep-host`).
    pub helper_missing: bool,
    pub connection: ConnectionView,
    /// An Endstep account exists, so a match has come in: the card stops
    /// saying matches appear after the first one.
    pub has_account: bool,
}

/// Both switches as the page shows them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct IntegrationsView {
    /// Arena is on (the stored choice, else whether Arena data exists).
    pub arena: bool,
    pub endstep: bool,
    pub endstep_status: EndstepStatusView,
}

/// What the command still has to do on the machine after the switch is
/// stored: the helper registration is files and the registry, kept out of
/// `apply` so tests never touch the browsers' real folders.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HostEffect {
    None,
    Install,
    Uninstall,
}

fn integrations_view(
    state: &AppState,
    settings: &Settings,
) -> Result<IntegrationsView, CommandError> {
    let (presence, endstep_accounts) = {
        let reader = lock(&state.reader);
        let presence = store::platform::platform_presence(&reader).map_err(storage)?;
        let accounts =
            store::accounts::accounts(&reader, store::Platform::Endstep).map_err(storage)?;
        (presence, accounts)
    };
    Ok(IntegrationsView {
        arena: arena_effective(settings, &presence),
        endstep: settings.endstep_enabled,
        endstep_status: EndstepStatusView {
            helper_missing: state
                .endstep_helper_missing
                .load(std::sync::atomic::Ordering::SeqCst),
            connection: ConnectionView::from_connection(connection(
                read_last_seen(&state.data_dir),
                crate::wall_clock_ms(),
            )),
            has_account: !endstep_accounts.is_empty(),
        },
    })
}

/// The row of `key` alone, so saving it never writes another setting.
fn only_row(settings: &Settings, key: &str) -> Vec<(String, String)> {
    settings
        .to_rows()
        .into_iter()
        .filter(|(name, _)| name == key)
        .collect()
}

/// Store one switch and switch the running tracker (Arena's log reading,
/// Endstep's capture sync). A switched-off platform stops being the current
/// one because `current_platform` hides it, so nothing moves the header.
pub(crate) fn apply(
    state: &AppState,
    integration: IntegrationView,
    on: bool,
) -> Result<(IntegrationsView, HostEffect), CommandError> {
    let (settings, effect) = {
        let _one_save = lock(&state.settings_lock);
        let mut settings = read_settings(state)?;
        if lock(&state.tracker).is_none() {
            restart_tracker(state, &settings)?;
        }
        match integration {
            IntegrationView::Arena => {
                settings.arena_enabled = Some(on);
                let row = only_row(&settings, ARENA_ENABLED);
                state
                    .tracker_part(tracker::TrackerHandle::settings_saver)?
                    .set_arena_enabled(on, row)
                    .map_err(CommandError::Tracker)?;
                (settings, HostEffect::None)
            }
            IntegrationView::Endstep => {
                settings.endstep_enabled = on;
                let row = only_row(&settings, ENDSTEP_ENABLED);
                state
                    .tracker_part(tracker::TrackerHandle::settings_saver)?
                    .set_endstep_enabled(on, row)
                    .map_err(CommandError::Tracker)?;
                let effect = if on {
                    HostEffect::Install
                } else {
                    HostEffect::Uninstall
                };
                (settings, effect)
            }
        }
    };
    Ok((integrations_view(state, &settings)?, effect))
}

fn read_view(state: &AppState) -> Result<IntegrationsView, CommandError> {
    integrations_view(state, &read_settings(state)?)
}

#[tauri::command]
#[specta::specta]
pub fn integrations(startup: tauri::State<'_, Startup>) -> Result<IntegrationsView, CommandError> {
    let _timer = Timer::start("integrations");
    read_view(ready(&startup)?)
}

/// Push the stored settings; the page refetches the integrations on it.
fn push_settings(app: &AppHandle) {
    let startup = app.state::<Startup>();
    let Ok(state) = ready(&startup) else { return };
    let Ok(settings) = read_settings(state) else {
        return;
    };
    if let Err(error) = (SettingsChanged { settings }).emit(app) {
        tracing::warn!(%error, "could not push the settings change");
    }
}

/// Async, and the wait for the tracker runs on a blocking thread.
#[tauri::command]
#[specta::specta]
pub async fn set_integration(
    app: AppHandle,
    integration: IntegrationView,
    on: bool,
) -> Result<IntegrationsView, CommandError> {
    let _timer = Timer::start("set_integration");
    let worker = app.clone();
    let (view, settings) = off_async_workers(move || {
        let startup = worker.state::<Startup>();
        let state = ready(&startup)?;
        let (view, effect) = apply(state, integration, on)?;
        match effect {
            HostEffect::Install | HostEffect::Uninstall => {
                // The view above was built before the registration thread
                // ran: push the settings again when it is done, so the page
                // refetches the final state.
                let finished = worker.clone();
                crate::endstep::host::spawn_sync(
                    state.data_dir.clone(),
                    state.db_path.clone(),
                    std::sync::Arc::clone(&state.endstep_helper_missing),
                    move || push_settings(&finished),
                );
            }
            HostEffect::None => {}
        }
        if effect != HostEffect::None
            && let Some(inbox) = worker.try_state::<crate::endstep::import::InboxWorker>()
        {
            inbox.wake();
        }
        Ok((view, read_settings(state)?))
    })
    .await?;
    if let Err(error) = (SettingsChanged { settings }).emit(&app) {
        tracing::warn!(%error, "could not push the settings change");
    }
    Ok(view)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::tests::ready_state;
    use crate::platform::tests::{seed, stop};
    use fixture_tools::TestDir;

    #[test]
    fn arena_defaults_to_whether_arena_data_exists() {
        let none = PlatformPresence {
            mtgo: true,
            arena: false,
            endstep: false,
        };
        let some = PlatformPresence {
            mtgo: true,
            arena: true,
            endstep: false,
        };
        let unchosen = Settings::default();
        assert!(!arena_effective(&unchosen, &none));
        assert!(arena_effective(&unchosen, &some));
        let off = Settings {
            arena_enabled: Some(false),
            ..Settings::default()
        };
        assert!(!arena_effective(&off, &some));
        let on = Settings {
            arena_enabled: Some(true),
            ..Settings::default()
        };
        assert!(arena_effective(&on, &none));
    }

    #[test]
    fn switching_endstep_off_unregisters_and_keeps_data() {
        let dir = TestDir::new("integrations-endstep-off");
        let (state, _events) = ready_state(&dir);
        {
            let conn = store::open_writer(&state.db_path).expect("writer");
            conn.execute(
                "INSERT INTO endstep_captures (unit_id, kind, started_at, counter, format,
                     extension, browser, developer, classification, frame_count, file_hash,
                     body, imported_at)
                 VALUES ('unit-1', 'match', 1, 1, 1, '0.1.0', 'firefox', 0, 'human', 1, 'h',
                     x'00', 1)",
                [],
            )
            .expect("capture");
        }
        let (on, effect) = apply(&state, IntegrationView::Endstep, true).expect("on");
        assert!(on.endstep);
        assert_eq!(effect, HostEffect::Install);
        assert!(read_settings(&state).expect("read").endstep_enabled);

        let (off, effect) = apply(&state, IntegrationView::Endstep, false).expect("off");

        assert!(!off.endstep);
        assert_eq!(effect, HostEffect::Uninstall);
        assert!(!read_settings(&state).expect("read").endstep_enabled);
        let count = store::endstep_captures::capture_count(&lock(&state.reader)).expect("count");
        assert_eq!(count, 1, "stored captures stay");
        stop(&state);
    }

    #[test]
    fn the_connection_comes_from_last_seen_json() {
        let dir = TestDir::new("integrations-last-seen");
        assert_eq!(read_last_seen(dir.path()), None);
        std::fs::create_dir_all(dir.join("endstep")).expect("folder");
        std::fs::write(
            dir.join("endstep/last_seen.json"),
            r#"{"at":1000,"browser":"chrome","extension":"0.2.0"}"#,
        )
        .expect("write");
        let seen = read_last_seen(dir.path());
        assert!(seen.is_some());
        let view = ConnectionView::from_connection(connection(seen, 2000));
        assert_eq!(
            view,
            ConnectionView::Recent {
                browser: "chrome".to_string(),
                extension: "0.2.0".to_string(),
                at_ms: 1000
            }
        );
    }

    #[test]
    fn the_arena_switch_is_stored_and_shown_as_chosen() {
        let dir = TestDir::new("integrations-arena-choice");
        let (state, _events) = ready_state(&dir);
        seed(&state, true, true);
        assert!(read_view(&state).expect("view").arena, "on with Arena data");

        let (off, effect) = apply(&state, IntegrationView::Arena, false).expect("off");
        assert!(!off.arena);
        assert_eq!(effect, HostEffect::None);
        assert_eq!(
            read_settings(&state).expect("read").arena_enabled,
            Some(false)
        );

        let (on, _) = apply(&state, IntegrationView::Arena, true).expect("on");
        assert!(on.arena);
        stop(&state);
    }
}
