//! The Replays page's commands (recorded replays spec 4 and 5). Only these
//! change `recorder_enabled` and `recorder_acceptance`: a general settings
//! save keeps the stored values (`Settings::keep_command_owned`).

use recorder::{PIN, download_blocked};
use serde::Serialize;
use tauri::{AppHandle, Manager, State};
use tauri_plugin_opener::OpenerExt;

use crate::bridge::lock;
use crate::commands::{
    AppState, CommandError, Startup, off_async_workers, read_settings, ready, restart_tracker,
};
use crate::events::SettingsChanged;
use crate::logging::Timer;
use crate::recorder::import::recorder_folder;
use crate::recorder::supervisor::{
    NOTICE_VERSION, RecorderSignal, RecorderStatus, RecorderWorker, SUPPORTED, needs_acceptance,
    view_status,
};
use crate::settings::{self, RecorderAcceptance, Settings};
use tauri_specta::Event;

/// What the Replays page shows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct RecorderStatusView {
    pub status: RecorderStatus,
    /// The recorder's output folder.
    pub folder: String,
    /// Recordings imported so far.
    #[specta(type = specta_typescript::Number)]
    pub imported: i64,
    /// The notice must be shown before the switch can turn on.
    pub needs_acceptance: bool,
    /// The pinned recorder version.
    pub version: String,
}

/// Store the recorder keys (and only those) after `change`.
fn store_recorder(
    state: &AppState,
    change: impl FnOnce(&mut Settings) -> Result<(), CommandError>,
) -> Result<Settings, CommandError> {
    let _one_save = lock(&state.settings_lock);
    let mut settings = read_settings(state)?;
    if lock(&state.tracker).is_none() {
        restart_tracker(state, &settings)?;
    }
    change(&mut settings)?;
    let rows = settings
        .to_rows()
        .into_iter()
        .filter(|(key, _)| {
            key == settings::RECORDER_ENABLED || key == settings::RECORDER_ACCEPTANCE
        })
        .collect();
    state
        .tracker_part(tracker::TrackerHandle::settings_saver)?
        .save(rows)
        .map_err(CommandError::Tracker)?;
    Ok(settings)
}

/// Store the acceptance at `now_ms` and turn the switch on.
pub fn accept_notice(state: &AppState, now_ms: i64) -> Result<Settings, CommandError> {
    store_recorder(state, |settings| {
        settings.recorder_acceptance = Some(RecorderAcceptance {
            notice_version: NOTICE_VERSION,
            consent_version: PIN.consent_version,
            accepted_at_ms: now_ms,
        });
        settings.recorder_enabled = true;
        Ok(())
    })
}

/// Turn the switch on or off. On is refused while the notice is due.
pub fn set_enabled(state: &AppState, on: bool) -> Result<Settings, CommandError> {
    store_recorder(state, |settings| {
        if on && needs_acceptance(settings.recorder_acceptance.as_ref(), &PIN) {
            return Err(CommandError::Refused(
                "Read and accept the notice first.".to_string(),
            ));
        }
        settings.recorder_enabled = on;
        Ok(())
    })
}

fn view(app: &AppHandle, state: &AppState, settings: &Settings) -> RecorderStatusView {
    let worker_status = app
        .try_state::<RecorderWorker>()
        .map_or(RecorderStatus::Off, |worker| worker.status());
    let enabled =
        settings.recorder_enabled && !needs_acceptance(settings.recorder_acceptance.as_ref(), &PIN);
    let folder = app
        .path()
        .document_dir()
        .map(|documents| recorder_folder(&documents).to_string_lossy().into_owned())
        .unwrap_or_default();
    let imported = store::recorded_replays::recording_count(&lock(&state.reader)).unwrap_or(0);
    RecorderStatusView {
        status: view_status(download_blocked(&PIN), SUPPORTED, enabled, &worker_status),
        folder,
        imported,
        needs_acceptance: needs_acceptance(settings.recorder_acceptance.as_ref(), &PIN),
        version: PIN.version.to_string(),
    }
}

fn signal(app: &AppHandle, signal: RecorderSignal) {
    if let Some(worker) = app.try_state::<RecorderWorker>() {
        worker.send(signal);
    }
}

fn push_settings(app: &AppHandle, settings: Settings) {
    if let Err(error) = (SettingsChanged { settings }).emit(app) {
        tracing::warn!(%error, "could not push the settings change");
    }
}

/// The page's current status.
#[tauri::command]
#[specta::specta]
pub async fn recorder_status(app: AppHandle) -> Result<RecorderStatusView, CommandError> {
    let _timer = Timer::start("recorder_status");
    off_async_workers(move || {
        let startup = app.state::<Startup>();
        let state = ready(&startup)?;
        Ok(view(&app, state, &read_settings(state)?))
    })
    .await
}

/// "Download and enable": stores the acceptance, turns the switch on and
/// starts the download.
#[tauri::command]
#[specta::specta]
pub async fn accept_recorder_notice(app: AppHandle) -> Result<RecorderStatusView, CommandError> {
    let _timer = Timer::start("accept_recorder_notice");
    let worker = app.clone();
    let (settings, shown) = off_async_workers(move || {
        let startup = worker.state::<Startup>();
        let state = ready(&startup)?;
        let settings = accept_notice(state, crate::wall_clock_ms())?;
        signal(&worker, RecorderSignal::Retry);
        let shown = view(&worker, state, &settings);
        Ok((settings, shown))
    })
    .await?;
    push_settings(&app, settings);
    Ok(shown)
}

/// The switch. On is refused while the notice is due.
#[tauri::command]
#[specta::specta]
pub async fn set_recorder_enabled(
    app: AppHandle,
    on: bool,
) -> Result<RecorderStatusView, CommandError> {
    let _timer = Timer::start("set_recorder_enabled");
    let worker = app.clone();
    let (settings, shown) = off_async_workers(move || {
        let startup = worker.state::<Startup>();
        let state = ready(&startup)?;
        let settings = set_enabled(state, on)?;
        signal(&worker, RecorderSignal::Wake);
        let shown = view(&worker, state, &settings);
        Ok((settings, shown))
    })
    .await?;
    push_settings(&app, settings);
    Ok(shown)
}

/// Retry after a failure, a block or a stop.
#[tauri::command]
#[specta::specta]
pub async fn retry_recorder(app: AppHandle) -> Result<RecorderStatusView, CommandError> {
    let _timer = Timer::start("retry_recorder");
    off_async_workers(move || {
        let startup = app.state::<Startup>();
        let state = ready(&startup)?;
        signal(&app, RecorderSignal::Retry);
        Ok(view(&app, state, &read_settings(state)?))
    })
    .await
}

/// Open the recorder's folder in the file manager.
#[tauri::command]
#[specta::specta]
pub fn open_recorder_folder(
    app: AppHandle,
    startup: State<'_, Startup>,
) -> Result<(), CommandError> {
    let _timer = Timer::start("open_recorder_folder");
    ready(&startup)?;
    let documents = app
        .path()
        .document_dir()
        .map_err(|_| CommandError::Refused("The Documents folder was not found.".to_string()))?;
    let folder = recorder_folder(&documents);
    if let Err(error) = std::fs::create_dir_all(&folder) {
        tracing::warn!(%error, "the recorder folder could not be made");
    }
    app.opener()
        .open_path(folder.to_string_lossy().into_owned(), None::<&str>)
        .map_err(|error| {
            tracing::warn!(%error, "the recorder folder could not be opened");
            CommandError::Refused("The folder could not be opened.".to_string())
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::apply_settings;
    use crate::commands::tests::ready_state;
    use fixture_tools::TestDir;

    /// Accepting stores the acceptance and the switch; a general save from a
    /// stale page cannot undo either.
    #[test]
    fn accepting_turns_it_on_and_a_general_save_keeps_it() {
        let dir = TestDir::new("recorder-accept");
        let (state, _receiver) = ready_state(&dir);
        assert!(!read_settings(&state).expect("read").recorder_enabled);
        accept_notice(&state, 7).expect("accepted");
        let stored = read_settings(&state).expect("read");
        assert!(stored.recorder_enabled);
        assert_eq!(
            stored
                .recorder_acceptance
                .expect("acceptance")
                .accepted_at_ms,
            7
        );
        apply_settings(&state, Settings::default()).expect("saved");
        let after = read_settings(&state).expect("read");
        assert!(after.recorder_enabled);
        assert!(after.recorder_acceptance.is_some());
        lock(&state.tracker).take().expect("tracker").stop();
    }

    #[test]
    fn on_is_refused_until_the_notice_is_accepted() {
        let dir = TestDir::new("recorder-refuse");
        let (state, _receiver) = ready_state(&dir);
        assert!(matches!(
            set_enabled(&state, true),
            Err(CommandError::Refused(_))
        ));
        assert!(!read_settings(&state).expect("read").recorder_enabled);
        accept_notice(&state, 1).expect("accepted");
        set_enabled(&state, false).expect("off");
        assert!(!read_settings(&state).expect("read").recorder_enabled);
        set_enabled(&state, true).expect("on again");
        assert!(read_settings(&state).expect("read").recorder_enabled);
        lock(&state.tracker).take().expect("tracker").stop();
    }
}
