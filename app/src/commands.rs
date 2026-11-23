//! Tauri commands (spec 6: "controller props become commands"). Each command
//! is a thin shell over a plain function that the tests call directly.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

use chrono_tz::Tz;
use serde::Serialize;
use store::Connection;
use tauri::{AppHandle, Manager, State};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_opener::OpenerExt;
use tauri_specta::Event;
use tracker::{TrackerConfig, TrackerEvent, TrackerHandle};

use crate::bridge::{Shared, lock};
use crate::events::{ImportProgressChanged, SettingsChanged};
use crate::live::{PER_PAGE, read_page};
use crate::logging::Timer;
use crate::matches::log_cache::GameLogCache;
use crate::matches::{MatchListQuery, match_filter};
use crate::settings::{Settings, TrackedFolders};
use crate::summary::{MatchPage, StatusView};
use decks::stats::Range;

/// Everything the commands need once startup succeeded.
pub struct AppState {
    pub data_dir: PathBuf,
    pub db_path: PathBuf,
    pub timezone: Tz,
    /// The commands' read-only connection (spec 3: readers never block the writer).
    pub reader: Mutex<Connection>,
    pub shared: Arc<Shared>,
    /// `None` only while tracking restarts on a new log folder.
    pub tracker: Mutex<Option<TrackerHandle>>,
    /// Kept so a restarted tracker reports to the same bridge.
    pub tracker_events: Sender<TrackerEvent>,
    /// The folders the running tracker was started with.
    pub tracking_folders: Mutex<TrackedFolders>,
    /// Held for the whole of `apply_settings`, so two saves never interleave
    /// their store and restart steps. Taken before any other lock.
    pub settings_lock: Mutex<()>,
    /// A 0.x import is running (one at a time, importer spec section 7).
    pub import_running: AtomicBool,
    /// Where each match's MTGO game log file is, and its parsed entries.
    pub game_logs: Mutex<GameLogCache>,
    /// The latest history scan and the game log index scans share.
    pub history: crate::history_import::scan::HistoryScans,
    /// A fresh install (`startup::Prepared`: the database was created by this
    /// start, or by one that ended before its first-run decision): the
    /// first-run and What's new decisions treat the user as new, even when
    /// the tracker's catch-up has stored matches already.
    pub fresh_install: bool,
    /// Whether the last Endstep helper install found no helper beside
    /// MyMTGO (a dev build without it): the Integrations page says so.
    pub endstep_helper_missing: Arc<AtomicBool>,
}

/// Managed state: startup either worked or the fatal screen explains why.
pub enum Startup {
    /// Boxed: `AppState` is far larger than `Failed` (clippy `large_enum_variant`).
    Ready(Box<AppState>),
    Failed {
        data_dir: String,
        error: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(tag = "kind", content = "message", rename_all = "camelCase")]
pub enum CommandError {
    /// Startup failed; the message says why.
    NotReady(String),
    Storage(String),
    /// Tracking is not running or did not answer, or a command's blocking
    /// work stopped unexpectedly (`off_async_workers`).
    Tracker(String),
    /// The update could not be installed.
    Update(String),
    /// The request was refused and nothing changed: invalid input, or an
    /// edit of an archived deck. The message is for the user.
    Refused(String),
}

impl CommandError {
    /// The message, whatever the kind: for callers that answer with text.
    pub fn message(&self) -> &str {
        match self {
            CommandError::NotReady(message)
            | CommandError::Storage(message)
            | CommandError::Tracker(message)
            | CommandError::Update(message)
            | CommandError::Refused(message) => message,
        }
    }
}

impl AppState {
    /// A part of the running tracker (an editor, the importer, the settings
    /// saver), or a `Tracker` error while tracking is not running. The
    /// tracker lock is held only while `part` clones the handle, never while
    /// waiting for an answer, so stopping or restarting tracking never waits
    /// for a slow command. A stop or restart that comes between this and the
    /// send makes the command answer that the tracker stopped.
    pub fn tracker_part<T>(
        &self,
        part: impl FnOnce(&TrackerHandle) -> T,
    ) -> Result<T, CommandError> {
        match lock(&self.tracker).as_ref() {
            Some(tracker) => Ok(part(tracker)),
            None => Err(CommandError::Tracker("tracking is not running".to_string())),
        }
    }

    /// Store settings rows through the tracker's writer and wait for the
    /// commit, without holding the tracker lock. The error is text to show.
    pub fn save_setting_rows(&self, rows: Vec<(String, String)>) -> Result<(), String> {
        self.tracker_part(TrackerHandle::settings_saver)
            .map_err(|error| error.message().to_string())?
            .save(rows)
    }
}

/// Run `work`, which may wait (for the tracker, the network, a dialog or
/// the disk), on a blocking thread, so the wait never holds one of the async
/// runtime's workers. A panic in `work` answers as a `Tracker` error.
pub async fn off_async_workers<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, CommandError> + Send + 'static,
) -> Result<T, CommandError> {
    match tauri::async_runtime::spawn_blocking(work).await {
        Ok(result) => result,
        Err(error) => Err(CommandError::Tracker(format!(
            "the command stopped unexpectedly: {error}"
        ))),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    pub version: String,
    pub data_dir: String,
    /// Why startup failed, for the fatal screen.
    pub fatal: Option<String>,
    /// Whether the tray icon was built: the Background card's closing text
    /// (spec section 4, ruling 3) and its close to tray switch follow it.
    pub tray_available: bool,
    /// The product name, read at runtime (`package_info().name`; release
    /// spec 13.1): the beta name up to the rename, "MyMTGO" from 1.0.
    pub product_name: String,
    /// A prerelease build: the header's "Beta" tag follows the version, not
    /// the name.
    pub prerelease: bool,
    /// The operating system (`std::env::consts::OS`: "windows", "macos",
    /// "linux"): the Replays page is Windows only.
    pub os: String,
}

pub fn ready(startup: &Startup) -> Result<&AppState, CommandError> {
    match startup {
        Startup::Ready(state) => Ok(state.as_ref()),
        Startup::Failed { error, .. } => Err(CommandError::NotReady(error.clone())),
    }
}

pub(crate) fn storage(error: store::StoreError) -> CommandError {
    CommandError::Storage(error.to_string())
}

/// Arena logs are found on their own (no override) in the app. Unit tests
/// name a log that is never there, so a test never reads the machine's real
/// Arena log.
#[cfg(not(test))]
fn arena_log_overrides() -> Vec<PathBuf> {
    Vec::new()
}

#[cfg(test)]
fn arena_log_overrides() -> Vec<PathBuf> {
    vec![std::env::temp_dir().join("mymtgo-tests-no-arena-log/Player.log")]
}

/// The MTGO data folder the tracker reads: the chosen one, else found on its
/// own in the app.
#[cfg(not(test))]
fn mtgo_data_folder(settings: &Settings) -> Option<PathBuf> {
    settings.mtgo_data_folder_path()
}

/// Unit tests without a chosen folder name one that is never there, so a
/// test never reads the machine's real MTGO decks, history or game logs.
#[cfg(test)]
fn mtgo_data_folder(settings: &Settings) -> Option<PathBuf> {
    settings
        .mtgo_data_folder_path()
        .or_else(|| Some(std::env::temp_dir().join("mymtgo-tests-no-mtgo-data")))
}

/// The MTGO log folders the tracker searches: the chosen one, else none, so
/// it finds them on its own in the app.
#[cfg(not(test))]
fn log_overrides(settings: &Settings) -> Vec<PathBuf> {
    settings.log_overrides()
}

/// Unit tests without a chosen folder name one that is never there, so a
/// test never searches the machine's real MTGO log folders (#787).
#[cfg(test)]
fn log_overrides(settings: &Settings) -> Vec<PathBuf> {
    let chosen = settings.log_overrides();
    if chosen.is_empty() {
        vec![std::env::temp_dir().join("mymtgo-tests-no-mtgo-logs")]
    } else {
        chosen
    }
}

/// The tracker's start-up config. Arena is on by the stored choice, else
/// when the database already has Arena data (`integrations::arena_effective`).
/// Endstep is on by its stored switch (`endstep_enabled`, off by default).
pub fn tracker_config(
    db_path: PathBuf,
    timezone: Tz,
    settings: &Settings,
    presence: &store::platform::PlatformPresence,
) -> TrackerConfig {
    TrackerConfig {
        db_path,
        timezone,
        os_timezone: Some(crate::timezone::system_timezone),
        log_overrides: log_overrides(settings),
        arena_log_overrides: arena_log_overrides(),
        mtgo_data_folder: mtgo_data_folder(settings),
        replay_day_anchor: None,
        watching: settings.watcher_active,
        arena_enabled: crate::integrations::arena_effective(settings, presence),
        endstep_enabled: settings.endstep_enabled,
    }
}

/// Stop the tracker so its pending writes commit. Afterwards there is none.
pub fn stop_tracker(state: &AppState) {
    if let Some(tracker) = lock(&state.tracker).take() {
        tracker.stop();
    }
}

pub fn read_settings(state: &AppState) -> Result<Settings, CommandError> {
    let rows = store::read::settings(&lock(&state.reader)).map_err(storage)?;
    Ok(Settings::from_rows(&rows))
}

/// One page of the matches list through the list's filters and sort:
/// stored rows with live views laid over them. Live matches storage has not
/// written yet join the first page only when nothing is filtered and the
/// list is newest first.
pub fn matches_page(state: &AppState, query: &MatchListQuery) -> Result<MatchPage, CommandError> {
    let page = query.page.max(1);
    let filter = match_filter(query, Range::all_time(), state.timezone)?;
    // Hold the live views while reading storage, so the bridge cannot forget
    // one that this read is still too old to show.
    let live = lock(&state.shared.live);
    // The scope is read once, with the page, so a platform switch between
    // two reads cannot mix platforms.
    let reader = lock(&state.reader);
    let platform = crate::platform::page_scope(&reader)
        .map_err(storage)?
        .platform;
    let (stored, total) = read_page(&reader, platform, &filter, page).map_err(storage)?;
    drop(reader);
    let with_live_only = page == 1 && query.is_unfiltered();
    let rows = live.merge(stored, with_live_only, PER_PAGE, platform);
    drop(live);
    let page = MatchPage {
        rows,
        total,
        page,
        per_page: PER_PAGE,
        unknown_archetype_count: 0,
        archetypes: Vec::new(),
    };
    crate::matches::with_list_archetypes(
        &lock(&state.reader),
        page,
        store::match_list::Scope::All(platform),
        Range::all_time(),
        None,
    )
}

/// What a save did. The settings are stored; `restart_error` is set when a
/// tracked folder changed and tracking could not be restarted on it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Applied {
    pub settings: Settings,
    pub restart_error: Option<CommandError>,
}

/// How a tracker is started. Tests swap in one that fails.
type SpawnTracker = fn(TrackerConfig, Sender<TrackerEvent>) -> std::io::Result<TrackerHandle>;

/// Store settings through the tracker's writer; a new log or data folder restarts
/// tracking on it. An `Err` means nothing was stored. A failed restart after
/// the store is reported in `Applied`, so the caller can still tell the window
/// and the UI what is now stored.
pub fn apply_settings(state: &AppState, settings: Settings) -> Result<Applied, CommandError> {
    apply_settings_with(state, settings, tracker::spawn)
}

fn apply_settings_with(
    state: &AppState,
    settings: Settings,
    spawn: SpawnTracker,
) -> Result<Applied, CommandError> {
    let mut settings = settings.cleaned();
    // Network and device folders are never opened (an NTLM hash leak).
    for folder in [&settings.log_folder, &settings.mtgo_data_folder]
        .into_iter()
        .flatten()
    {
        crate::paths::refuse_remote_path(folder)?;
    }
    // One save at a time, so another save cannot restart tracking on a
    // different folder between our store and our restart.
    let _one_save = lock(&state.settings_lock);
    // Fields with their own command keep their stored values (under this
    // lock, as `accounts::switch` and `save_trust` take it too).
    settings.keep_command_owned(&read_settings(state)?);
    // An earlier restart that failed left no tracker: saves need its writer,
    // so start one first.
    if lock(&state.tracker).is_none() {
        restart_tracker_with(state, &settings, spawn)?;
    }
    state
        .tracker_part(TrackerHandle::settings_saver)?
        .save(settings.to_rows())
        .map_err(CommandError::Tracker)?;
    // Compare with the folders tracking really uses, not the stored ones: a
    // save that timed out in the UI may have been stored without a restart.
    let restart_error = if *lock(&state.tracking_folders) != settings.tracked_folders() {
        restart_tracker_with(state, &settings, spawn).err()
    } else {
        None
    };
    Ok(Applied {
        settings,
        restart_error,
    })
}

/// What the beta switch did: the stored settings, and whether the update
/// channel changed (the caller then starts a check on the new feed, release
/// spec 6.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BetaSaved {
    pub settings: Settings,
    pub channel_changed: bool,
}

/// The Updates card's "Enable beta updates" switch (release spec 6.1):
/// stores only `beta_updates`, so a settings save from a stale page can
/// never undo it (`apply_settings` keeps the stored value).
pub fn set_beta(state: &AppState, beta_updates: bool) -> Result<BetaSaved, CommandError> {
    let _one_save = lock(&state.settings_lock);
    let mut settings = read_settings(state)?;
    if lock(&state.tracker).is_none() {
        restart_tracker_with(state, &settings, tracker::spawn)?;
    }
    let version = env!("CARGO_PKG_VERSION");
    let before = crate::updates::channel(settings.beta_updates, version);
    settings.beta_updates = Some(beta_updates);
    let row = settings
        .to_rows()
        .into_iter()
        .filter(|(key, _)| key == crate::settings::BETA_UPDATES)
        .collect();
    state
        .tracker_part(TrackerHandle::settings_saver)?
        .save(row)
        .map_err(CommandError::Tracker)?;
    let channel_changed = before != crate::updates::channel(settings.beta_updates, version);
    Ok(BetaSaved {
        settings,
        channel_changed,
    })
}

/// The Watcher card's Start and Stop (release spec 4.1; 0.x
/// `UpdateWatcherController`): stores `watcher_active` and switches the
/// tracker in one command. Start is refused, changing nothing, while no
/// `mtgo.log` is found (0.x refused it silently while a path was invalid).
/// Returns the stored settings.
pub fn set_watcher(state: &AppState, active: bool) -> Result<Settings, CommandError> {
    let _one_save = lock(&state.settings_lock);
    let mut settings = read_settings(state)?;
    if active && lock(&state.shared.status).files.is_empty() {
        return Ok(settings);
    }
    if lock(&state.tracker).is_none() {
        restart_tracker_with(state, &settings, tracker::spawn)?;
    }
    settings.watcher_active = active;
    let row = settings
        .to_rows()
        .into_iter()
        .filter(|(key, _)| key == crate::settings::WATCHER_ACTIVE)
        .collect();
    state
        .tracker_part(TrackerHandle::settings_saver)?
        .set_watching(active, row)
        .map_err(CommandError::Tracker)?;
    Ok(settings)
}

/// Stop the tracker (its pending writes commit) and start it with `settings`.
pub fn restart_tracker(state: &AppState, settings: &Settings) -> Result<(), CommandError> {
    restart_tracker_with(state, settings, tracker::spawn)
}

fn restart_tracker_with(
    state: &AppState,
    settings: &Settings,
    spawn: SpawnTracker,
) -> Result<(), CommandError> {
    let mut slot = lock(&state.tracker);
    if let Some(old) = slot.take() {
        old.stop();
        // The old tracker is joined, so all its events are already sent. Tell the
        // bridge to reset the live views, as an error restart does. If the spawn
        // below fails, the status keeps saying tracking is paused.
        let reason = "the tracked folders changed".to_string();
        if state
            .tracker_events
            .send(TrackerEvent::Restarting { reason })
            .is_err()
        {
            tracing::warn!("the bridge is gone; could not announce the restart");
        }
    }
    let presence = store::platform::platform_presence(&lock(&state.reader)).map_err(storage)?;
    let config = tracker_config(state.db_path.clone(), state.timezone, settings, &presence);
    let handle = spawn(config, state.tracker_events.clone())
        .map_err(|error| CommandError::Tracker(format!("could not restart tracking: {error}")))?;
    *slot = Some(handle);
    *lock(&state.tracking_folders) = settings.tracked_folders();
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn app_info(app: AppHandle, startup: State<'_, Startup>) -> AppInfo {
    let _timer = Timer::start("app_info");
    let version = app.package_info().version.to_string();
    let product_name = app.package_info().name.clone();
    let tray_available = app
        .try_state::<crate::window::WindowPrefs>()
        .is_some_and(|prefs| prefs.tray_available.load(Ordering::SeqCst));
    app_info_for(startup.inner(), version, product_name, tray_available)
}

/// The app's facts for the window, from the startup outcome.
pub fn app_info_for(
    startup: &Startup,
    version: String,
    product_name: String,
    tray_available: bool,
) -> AppInfo {
    let prerelease = crate::identity::show_beta_tag(&version);
    let (data_dir, fatal) = match startup {
        Startup::Ready(state) => (state.data_dir.to_string_lossy().into_owned(), None),
        Startup::Failed { data_dir, error } => (data_dir.clone(), Some(error.clone())),
    };
    AppInfo {
        version,
        data_dir,
        fatal,
        tray_available,
        product_name,
        prerelease,
        os: std::env::consts::OS.to_string(),
    }
}

/// The app's own log folder inside its data folder (`logging::LOG_FOLDER`).
pub fn app_log_folder(data_dir: &Path) -> PathBuf {
    data_dir.join(crate::logging::LOG_FOLDER)
}

/// Open the app's log folder in the file manager (the Advanced page's Logs
/// card), through the opener plugin's Rust side: its JS commands get no
/// permission. The folder is made first, so a fresh install opens too.
#[tauri::command]
#[specta::specta]
pub fn open_log_folder(app: AppHandle, startup: State<'_, Startup>) -> Result<(), CommandError> {
    let _timer = Timer::start("open_log_folder");
    let data_dir = match startup.inner() {
        Startup::Ready(state) => state.data_dir.clone(),
        Startup::Failed { data_dir, .. } => PathBuf::from(data_dir),
    };
    let folder = app_log_folder(&data_dir);
    if let Err(error) = std::fs::create_dir_all(&folder) {
        tracing::warn!(%error, "the log folder could not be made");
    }
    app.opener()
        .open_path(folder.to_string_lossy().into_owned(), None::<&str>)
        .map_err(|error| {
            tracing::warn!(%error, "the log folder could not be opened");
            CommandError::Refused("The folder could not be opened.".to_string())
        })
}

#[tauri::command]
#[specta::specta]
pub fn list_matches(
    query: MatchListQuery,
    startup: State<'_, Startup>,
) -> Result<MatchPage, CommandError> {
    let _timer = Timer::start("list_matches");
    matches_page(ready(&startup)?, &query)
}

#[tauri::command]
#[specta::specta]
pub fn get_status(startup: State<'_, Startup>) -> Result<StatusView, CommandError> {
    let _timer = Timer::start("get_status");
    Ok(lock(&ready(&startup)?.shared.status).clone())
}

#[tauri::command]
#[specta::specta]
pub fn get_settings(startup: State<'_, Startup>) -> Result<Settings, CommandError> {
    let _timer = Timer::start("get_settings");
    read_settings(ready(&startup)?)
}

/// Async so the wait for the tracker's commit never blocks the window.
#[tauri::command]
#[specta::specta]
pub async fn save_settings(app: AppHandle, settings: Settings) -> Result<Settings, CommandError> {
    let _timer = Timer::start("save_settings");
    let startup = app.state::<Startup>();
    let applied = apply_settings(ready(&startup)?, settings)?;
    // The settings are stored even if tracking could not restart on a new
    // folder, so the window and the UI follow what is stored.
    crate::window::set_close_to_tray(&app, applied.settings.close_to_tray);
    crate::window::apply_interface_scale(&app, applied.settings.interface_scale);
    if let Err(error) = (SettingsChanged {
        settings: applied.settings.clone(),
    })
    .emit(&app)
    {
        tracing::warn!(%error, "could not push the settings change");
    }
    // Overlay windows follow at once (overlay spec 11, ruling C7): a window
    // whose own setting was toggled is reapplied. Opened and closed on the
    // overlay thread, never here.
    crate::overlay::signal(
        &app,
        crate::overlay::worker::OverlayMsg::SettingsChanged {
            settings: Box::new(applied.settings.clone()),
        },
    );
    match applied.restart_error {
        Some(error) => Err(error),
        None => Ok(applied.settings),
    }
}

/// Start or stop the watcher. Async, and the wait for the tracker runs on
/// a blocking thread.
#[tauri::command]
#[specta::specta]
pub async fn set_watcher_active(app: AppHandle, active: bool) -> Result<Settings, CommandError> {
    let _timer = Timer::start("set_watcher_active");
    let worker = app.clone();
    let settings = off_async_workers(move || {
        let startup = worker.state::<Startup>();
        set_watcher(ready(&startup)?, active)
    })
    .await?;
    if let Err(error) = (SettingsChanged {
        settings: settings.clone(),
    })
    .emit(&app)
    {
        tracing::warn!(%error, "could not push the settings change");
    }
    Ok(settings)
}

/// Choose the update channel. Async, and the wait for the tracker runs on a
/// blocking thread.
#[tauri::command]
#[specta::specta]
pub async fn set_beta_updates(app: AppHandle, on: bool) -> Result<Settings, CommandError> {
    let _timer = Timer::start("set_beta_updates");
    let worker = app.clone();
    let saved = off_async_workers(move || {
        let startup = worker.state::<Startup>();
        set_beta(ready(&startup)?, on)
    })
    .await?;
    if saved.channel_changed {
        crate::updates::channel_changed(&app, saved.settings.beta_updates);
    }
    if let Err(error) = (SettingsChanged {
        settings: saved.settings.clone(),
    })
    .emit(&app)
    {
        tracing::warn!(%error, "could not push the settings change");
    }
    Ok(saved.settings)
}

/// Async: a blocking dialog must not run on the main thread.
#[tauri::command]
#[specta::specta]
pub async fn choose_log_folder(app: AppHandle) -> Option<String> {
    let _timer = Timer::start("choose_log_folder");
    let mut dialog = app
        .dialog()
        .file()
        .set_title("Choose the folder that holds mtgo.log");
    // Modal: the window cannot be used (and settings cannot change) while it is open.
    if let Some(window) = app.get_webview_window("main") {
        dialog = dialog.set_parent(&window);
    }
    let picked = dialog.blocking_pick_folder()?;
    let path = picked.into_path().ok()?;
    Some(path.to_string_lossy().into_owned())
}

/// Async: a blocking dialog must not run on the main thread.
#[tauri::command]
#[specta::specta]
pub async fn pick_legacy_db(app: AppHandle) -> Option<String> {
    let _timer = Timer::start("pick_legacy_db");
    let mut dialog = app
        .dialog()
        .file()
        .set_title("Choose the MyMTGO 0.x database")
        .add_filter("SQLite database", &["sqlite"])
        .add_filter("All files", &["*"]);
    if let Some(window) = app.get_webview_window("main") {
        dialog = dialog.set_parent(&window);
    }
    let picked = dialog.blocking_pick_file()?;
    let path = picked.into_path().ok()?;
    Some(path.to_string_lossy().into_owned())
}

#[tauri::command]
#[specta::specta]
pub fn get_update_status(app: AppHandle) -> crate::updates::UpdateStatus {
    let _timer = Timer::start("get_update_status");
    app.state::<crate::updates::Updates>().snapshot()
}

/// Check the feed now (the Settings button). Waits for the check.
#[tauri::command]
#[specta::specta]
pub async fn check_for_updates(app: AppHandle) -> crate::updates::UpdateStatus {
    let _timer = Timer::start("check_for_updates");
    crate::updates::check_now(&app).await;
    app.state::<crate::updates::Updates>().snapshot()
}

/// Install the downloaded update and restart. Async: stopping the tracker can
/// take a moment and must not block the window.
#[tauri::command]
#[specta::specta]
pub async fn install_update(app: AppHandle) -> Result<(), CommandError> {
    crate::updates::install(&app).map_err(CommandError::Update)
}

/// The 0.x import card's state (importer spec section 7).
#[tauri::command]
#[specta::specta]
pub fn get_legacy_import(
    startup: State<'_, Startup>,
) -> Result<crate::legacy::LegacyImportInfo, CommandError> {
    let _timer = Timer::start("get_legacy_import");
    crate::legacy::info(ready(&startup)?, crate::legacy::detected_path())
}

/// Store the 0.x database the user chose (`None` or blank: use the detected
/// one). Async so the wait for the tracker's commit never blocks the window.
#[tauri::command]
#[specta::specta]
pub async fn choose_legacy_db(
    app: AppHandle,
    path: Option<String>,
) -> Result<crate::legacy::LegacyImportInfo, CommandError> {
    let _timer = Timer::start("choose_legacy_db");
    let startup = app.state::<Startup>();
    crate::legacy::choose(ready(&startup)?, path, crate::legacy::detected_path())
}

/// Start importing the 0.x database on a background thread. Progress arrives
/// as `ImportProgressChanged` events; only one import runs at a time.
#[tauri::command]
#[specta::specta]
pub fn start_legacy_import(
    app: AppHandle,
    startup: State<'_, Startup>,
) -> Result<(), CommandError> {
    let _timer = Timer::start("start_legacy_import");
    let state = ready(&startup)?;
    spawn_legacy_import(&app, state)
}

/// Claim the import slot and run the 0.x import on its own thread (the
/// Import page, and the first run's migration: release spec section 13.2).
pub(crate) fn spawn_legacy_import(app: &AppHandle, state: &AppState) -> Result<(), CommandError> {
    let path = crate::legacy::begin(state, crate::legacy::detected_path())?;
    let app = app.clone();
    let spawned = std::thread::Builder::new()
        .name("legacy-import".to_string())
        .spawn(move || {
            let startup = app.state::<Startup>();
            // Startup was ready when the import began, and never changes.
            if let Ok(state) = ready(&startup) {
                let launch = AutostartLaunchAtLogin { app: &app };
                let legacy_images =
                    crate::legacy::run_import(state, &path, &launch, &mut |progress| {
                        // The first run hears the end before any window does,
                        // so a window that refetches sees it settled.
                        if matches!(
                            progress,
                            crate::legacy::ImportProgress::Finished { .. }
                                | crate::legacy::ImportProgress::Failed { .. }
                        ) && let Some(slot) = app.try_state::<crate::first_run::FirstRunSlot>()
                        {
                            slot.import_ended(&progress);
                        }
                        if let Err(error) = (ImportProgressChanged { progress }).emit(&app) {
                            tracing::warn!(%error, "could not push the import progress");
                        }
                    });
                // 0.x had local images on: its files are copied before any
                // download (cards spec section 13).
                if let Some(folder) = legacy_images {
                    crate::cards::worker::hand_legacy_images(&app, folder);
                }
                // An offline 0.x user stays offline from here on.
                crate::api::offline::follow_import(&app);
                // 0.x's custom background and overlay settings may have
                // come in (overlay spec section 12): the overlays follow.
                for message in crate::legacy::overlay_messages_after_import(state) {
                    crate::overlay::signal(&app, message);
                }
                // Imported matches without archetype rows wait for detection,
                // and imported archetypes known only by uuid for their
                // definitions (archetypes spec section 15).
                crate::archetypes::worker::signal(
                    &app,
                    crate::archetypes::worker::ArchetypeSignal::GateChanged,
                );
            }
        });
    if let Err(error) = spawned {
        state
            .import_running
            .store(false, std::sync::atomic::Ordering::SeqCst);
        return Err(CommandError::Tracker(format!(
            "could not start the import: {error}"
        )));
    }
    Ok(())
}

/// Launch at login through the autostart plugin, for the 0.x import.
struct AutostartLaunchAtLogin<'a> {
    app: &'a AppHandle,
}

impl crate::legacy::LaunchAtLogin for AutostartLaunchAtLogin<'_> {
    fn is_enabled(&self) -> Result<bool, String> {
        use tauri_plugin_autostart::ManagerExt;
        self.app
            .autolaunch()
            .is_enabled()
            .map_err(|error| error.to_string())
    }

    fn enable(&self) -> Result<(), String> {
        use tauri_plugin_autostart::ManagerExt;
        self.app
            .autolaunch()
            .enable()
            .map_err(|error| error.to_string())
    }
}

/// The user changed launch at login (the switch calls the autostart plugin
/// itself): the 0.x import never replaces that choice (release spec section
/// 13 item 4). Async so the wait for the tracker's commit never blocks the
/// window.
#[tauri::command]
#[specta::specta]
pub async fn launch_at_login_changed(app: AppHandle) -> Result<(), CommandError> {
    let _timer = Timer::start("launch_at_login_changed");
    let startup = app.state::<Startup>();
    crate::legacy::launch_at_login_changed(ready(&startup)?)
}

/// The UI reports an error it caught (spec 9). Works on the fatal screen too.
#[tauri::command]
#[specta::specta]
pub fn report_ui_error(report: crate::reporting::UiError) {
    let _timer = Timer::start("report_ui_error");
    crate::reporting::ui_error(&report);
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use chrono_tz::Tz;
    use fixture_tools::TestDir;
    use std::sync::mpsc::{self, Receiver};
    use std::time::{Duration, Instant};

    /// A ready app state over an empty log folder, as `lib.rs` builds it.
    pub(crate) fn ready_state(dir: &TestDir) -> (AppState, Receiver<TrackerEvent>) {
        dir.write("logs-a/.keep", b"");
        dir.write("logs-b/.keep", b"");
        let prepared =
            crate::startup::prepare(&dir.join("data"), (Tz::UTC, None)).expect("prepare");
        let settings = Settings {
            log_folder: Some(dir.join("logs-a").to_string_lossy().into_owned()),
            close_to_tray: true,
            mtgo_data_folder: None,
            ..Settings::default()
        };
        let (sender, receiver) = mpsc::channel();
        let presence =
            store::platform::platform_presence(&prepared.reader).expect("platform presence");
        let config = tracker_config(
            prepared.db_path.clone(),
            prepared.timezone,
            &settings,
            &presence,
        );
        let tracker = tracker::spawn(config, sender.clone()).expect("spawn");
        let state = AppState {
            data_dir: dir.join("data"),
            db_path: prepared.db_path,
            timezone: prepared.timezone,
            reader: Mutex::new(prepared.reader),
            shared: Arc::new(Shared::new(None)),
            tracker: Mutex::new(Some(tracker)),
            tracker_events: sender,
            tracking_folders: Mutex::new(settings.tracked_folders()),
            settings_lock: Mutex::new(()),
            import_running: AtomicBool::new(false),
            game_logs: Mutex::new(GameLogCache::new()),
            history: crate::history_import::scan::HistoryScans::new(),
            fresh_install: prepared.fresh_install,
            endstep_helper_missing: Arc::new(AtomicBool::new(false)),
        };
        (state, receiver)
    }

    pub(crate) fn store_a_match(state: &AppState) {
        let mut writer = store::open_writer(&state.db_path).expect("writer");
        let batch = store::write::WriteBatch {
            matches: vec![store::write::MatchWrite::Upsert {
                view: Box::new(crate::summary::testing::view(
                    "Qmatch-1",
                    1_000,
                    mtgo_match::MatchState::Complete,
                )),
                revision: 1,
            }],
            ..store::write::WriteBatch::default()
        };
        store::write::commit(&mut writer, &batch).expect("match");
    }

    /// The league sync the tracker runs after a change, for every owner, on
    /// a writer connection: tests that store matches and league rows
    /// directly run it as the tracker would (UTC, no draft abandoned). The
    /// write lock is taken at the start (`BEGIN IMMEDIATE`), as a running
    /// tracker may commit meanwhile: the busy timeout then waits for it,
    /// where a deferred transaction could fail on a stale snapshot.
    pub(crate) fn sync_leagues(conn: &store::Connection) {
        let tx = store::Transaction::new_unchecked(conn, store::TransactionBehavior::Immediate)
            .expect("transaction");
        store::league_sync::sync_every_owner(&tx, Tz::UTC, 0).expect("league sync");
        tx.commit().expect("commit");
    }

    /// Wait for a status whose searched roots include `root`.
    fn saw_roots(receiver: &Receiver<TrackerEvent>, root: &std::path::Path) -> bool {
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline {
            if let Ok(TrackerEvent::Status(status)) =
                receiver.recv_timeout(Duration::from_millis(100))
                && status.roots.iter().any(|searched| searched == root)
            {
                return true;
            }
        }
        false
    }

    /// The last status the tracker reported, waiting until `done` holds.
    fn status_until(
        receiver: &Receiver<TrackerEvent>,
        done: impl Fn(&tracker::Status) -> bool,
    ) -> Option<tracker::Status> {
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline {
            if let Ok(TrackerEvent::Status(status)) =
                receiver.recv_timeout(Duration::from_millis(100))
                && done(&status)
            {
                return Some(status);
            }
        }
        None
    }

    #[test]
    fn commands_wait_off_the_async_workers() {
        let caller = std::thread::current().id();
        let ran_on =
            tauri::async_runtime::block_on(off_async_workers(|| Ok(std::thread::current().id())));
        assert_ne!(ran_on, Ok(caller), "a blocking thread, not the caller's");
        let refused: Result<(), CommandError> =
            tauri::async_runtime::block_on(off_async_workers(|| {
                Err(CommandError::Refused("Q no".to_string()))
            }));
        assert_eq!(refused, Err(CommandError::Refused("Q no".to_string())));
        let crashed: Result<(), CommandError> =
            tauri::async_runtime::block_on(off_async_workers(|| panic!("Q crash")));
        assert!(
            matches!(&crashed, Err(CommandError::Tracker(message)) if message.starts_with("the command stopped unexpectedly")),
            "a panic is an internal failure, not a refusal: {crashed:?}"
        );
    }

    /// A save holds the tracker lock only to take the saver: a stop that
    /// comes before the send is never blocked, and the save answers with an
    /// error instead of waiting.
    #[test]
    fn a_save_racing_a_stop_answers_an_error() {
        let dir = TestDir::new("commands-save-stop");
        let (state, _receiver) = ready_state(&dir);
        let saver = state
            .tracker_part(TrackerHandle::settings_saver)
            .expect("running");
        stop_tracker(&state);
        assert!(saver.save(Settings::default().to_rows()).is_err());
        assert_eq!(
            state.save_setting_rows(Settings::default().to_rows()),
            Err("tracking is not running".to_string())
        );
    }

    /// Changing the beta setting starts an update check at once (release
    /// spec 6.1); a choice that keeps the channel does not.
    #[test]
    fn changing_the_channel_starts_a_check() {
        let dir = TestDir::new("commands-channel-check");
        let (state, _receiver) = ready_state(&dir);
        let build = crate::updates::channel(None, env!("CARGO_PKG_VERSION"));
        let build_is_beta = build == crate::updates::UpdateChannel::Edge;
        let choose = |on: bool| set_beta(&state, on).expect("saved");
        assert!(
            !choose(build_is_beta).channel_changed,
            "choosing the build's own channel"
        );
        let switched = choose(!build_is_beta);
        assert!(switched.channel_changed);
        assert_eq!(switched.settings.beta_updates, Some(!build_is_beta));
        assert_eq!(
            read_settings(&state).expect("read").beta_updates,
            Some(!build_is_beta)
        );
        assert!(!choose(!build_is_beta).channel_changed, "saved again");
        assert!(choose(build_is_beta).channel_changed, "and back");
        lock(&state.tracker).take().expect("tracker").stop();
    }

    /// Only the Updates card's switch (`set_beta_updates`) changes the
    /// channel: a page that read the settings before the switch saves them
    /// again and keeps the choice.
    #[test]
    fn a_settings_save_keeps_the_beta_choice_as_stored() {
        let dir = TestDir::new("commands-save-keeps-beta");
        let (state, _receiver) = ready_state(&dir);
        set_beta(&state, true).expect("chosen");
        let saved = apply_settings(
            &state,
            Settings {
                log_folder: Some(dir.join("logs-a").to_string_lossy().into_owned()),
                beta_updates: Some(false),
                ..Settings::default()
            },
        )
        .expect("saved");
        assert_eq!(saved.settings.beta_updates, Some(true));
        assert_eq!(
            read_settings(&state).expect("read").beta_updates,
            Some(true)
        );
        lock(&state.tracker).take().expect("tracker").stop();
    }

    /// Only the trust slider (`set_card_stats_trust`) changes the trust
    /// threshold: a page that read the settings before the slider moved
    /// saves them again and keeps the new level.
    #[test]
    fn a_settings_save_keeps_the_trust_as_stored() {
        let dir = TestDir::new("commands-save-keeps-trust");
        let (state, _receiver) = ready_state(&dir);
        crate::cards::stats::save_trust(&state, 80).expect("trusted");
        let saved = apply_settings(
            &state,
            Settings {
                log_folder: Some(dir.join("logs-a").to_string_lossy().into_owned()),
                card_stats_trust: 10,
                ..Settings::default()
            },
        )
        .expect("saved");
        assert_eq!(saved.settings.card_stats_trust, 80);
        assert_eq!(read_settings(&state).expect("read").card_stats_trust, 80);
        lock(&state.tracker).take().expect("tracker").stop();
    }

    /// The interface scale is an ordinary General setting: a save stores a
    /// listed value and reads anything else back as 100.
    #[test]
    fn a_settings_save_stores_the_interface_scale() {
        let dir = TestDir::new("commands-save-interface-scale");
        let (state, _receiver) = ready_state(&dir);
        let saved = apply_settings(
            &state,
            Settings {
                interface_scale: 125,
                ..Settings::default()
            },
        )
        .expect("saved");
        assert_eq!(saved.settings.interface_scale, 125);
        assert_eq!(read_settings(&state).expect("read").interface_scale, 125);
        let odd = apply_settings(
            &state,
            Settings {
                interface_scale: 333,
                ..Settings::default()
            },
        )
        .expect("saved");
        assert_eq!(odd.settings.interface_scale, 100);
        lock(&state.tracker).take().expect("tracker").stop();
    }

    /// v1 never switches the watcher off by itself (release spec ruling 6):
    /// 0.x stopped it for good on a mistyped path.
    #[test]
    fn a_saved_path_never_stops_the_watcher() {
        let dir = TestDir::new("commands-path-keeps-watcher");
        let (state, receiver) = ready_state(&dir);
        let missing = dir.join("no-such-folder").to_string_lossy().into_owned();
        let saved = apply_settings(
            &state,
            Settings {
                log_folder: Some(missing.clone()),
                mtgo_data_folder: Some(missing.clone()),
                ..Settings::default()
            },
        )
        .expect("saved");
        assert_eq!(saved.restart_error, None);
        assert!(saved.settings.watcher_active);
        assert!(read_settings(&state).expect("read").watcher_active);
        let status = status_until(&receiver, |status| {
            status
                .roots
                .iter()
                .any(|root| root.to_string_lossy() == missing)
        })
        .expect("the restarted tracker reported");
        assert!(!status.paused, "still watching, and searching");
        lock(&state.tracker).take().expect("tracker").stop();
    }

    const NO_PLATFORMS: store::platform::PlatformPresence = store::platform::PlatformPresence {
        mtgo: false,
        arena: false,
        endstep: false,
    };

    /// #787: a test tracker with no chosen log folder searches one that is
    /// never there, never the machine's real MTGO log folders.
    #[test]
    fn a_test_tracker_without_a_log_folder_never_searches_the_real_ones() {
        let config = tracker_config(
            PathBuf::from("unused.sqlite"),
            Tz::UTC,
            &Settings::default(),
            &NO_PLATFORMS,
        );
        assert_eq!(config.log_overrides.len(), 1);
        assert!(!config.log_overrides[0].exists());
        let chosen = Settings {
            log_folder: Some("chosen-logs".to_string()),
            ..Settings::default()
        };
        let config = tracker_config(
            PathBuf::from("unused.sqlite"),
            Tz::UTC,
            &chosen,
            &NO_PLATFORMS,
        );
        assert_eq!(config.log_overrides, vec![PathBuf::from("chosen-logs")]);
    }

    #[test]
    fn a_settings_save_keeps_the_watcher_as_stored() {
        let dir = TestDir::new("commands-save-keeps-watcher");
        dir.write("logs-a/mtgo.log", b"");
        let (state, receiver) = ready_state(&dir);
        assert!(status_until(&receiver, |status| !status.files.is_empty()).is_some());
        set_watcher(&state, false).expect("stopped");
        // A page that read the settings before the switch saves them again.
        let saved = apply_settings(
            &state,
            Settings {
                log_folder: Some(dir.join("logs-b").to_string_lossy().into_owned()),
                watcher_active: true,
                ..Settings::default()
            },
        )
        .expect("saved");
        assert!(
            !saved.settings.watcher_active,
            "only the Watcher card switches it"
        );
        let status = status_until(&receiver, |status| {
            status.roots.contains(&dir.join("logs-b"))
        })
        .expect("the restarted tracker reported");
        assert!(status.paused, "a restart on a new folder stays stopped");
        lock(&state.tracker).take().expect("tracker").stop();
    }

    #[test]
    fn the_watcher_starts_only_while_a_log_is_found() {
        let dir = TestDir::new("commands-watcher");
        let (state, receiver) = ready_state(&dir);
        assert!(status_until(&receiver, |status| !status.paused).is_some());
        let stopped = set_watcher(&state, false).expect("stopped");
        assert!(!stopped.watcher_active);
        assert!(!read_settings(&state).expect("read").watcher_active);
        assert!(status_until(&receiver, |status| status.paused).is_some());

        // No mtgo.log found: Start is refused and nothing changes (0.x).
        lock(&state.shared.status).files.clear();
        let refused = set_watcher(&state, true).expect("answered");
        assert!(!refused.watcher_active);
        assert!(!read_settings(&state).expect("read").watcher_active);

        dir.write("logs-a/mtgo.log", b"");
        assert!(status_until(&receiver, |status| !status.files.is_empty()).is_some());
        lock(&state.shared.status).files = vec![crate::summary::FileView {
            path: dir.join("logs-a/mtgo.log").to_string_lossy().into_owned(),
            size: 0,
            cursor: 0,
            last_live_growth_ms: None,
            bad_timezone: false,
        }];
        let started = set_watcher(&state, true).expect("started");
        assert!(started.watcher_active);
        assert!(read_settings(&state).expect("read").watcher_active);
        assert!(status_until(&receiver, |status| !status.paused).is_some());
        lock(&state.tracker).take().expect("tracker").stop();
    }

    #[test]
    fn saving_settings_stores_them_cleaned() {
        let dir = TestDir::new("commands-save");
        let (state, _receiver) = ready_state(&dir);
        let saved = apply_settings(
            &state,
            Settings {
                log_folder: Some(format!("  {}  ", dir.join("logs-a").display())),
                close_to_tray: false,
                mtgo_data_folder: None,
                ..Settings::default()
            },
        )
        .expect("saved")
        .settings;
        assert!(!saved.close_to_tray);
        assert_eq!(
            saved.log_folder,
            Some(dir.join("logs-a").to_string_lossy().into_owned())
        );
        assert_eq!(read_settings(&state).expect("read"), saved);
        lock(&state.tracker).take().expect("tracker").stop();
    }

    #[test]
    fn a_new_log_folder_restarts_tracking_on_it() {
        let dir = TestDir::new("commands-folder");
        let (state, receiver) = ready_state(&dir);
        assert!(
            saw_roots(&receiver, &dir.join("logs-a")),
            "first tracker searched logs-a"
        );
        apply_settings(
            &state,
            Settings {
                log_folder: Some(dir.join("logs-b").to_string_lossy().into_owned()),
                close_to_tray: true,
                mtgo_data_folder: None,
                ..Settings::default()
            },
        )
        .expect("saved");
        assert!(
            saw_roots(&receiver, &dir.join("logs-b")),
            "the restarted tracker searches logs-b"
        );
        lock(&state.tracker).take().expect("tracker").stop();
    }

    #[test]
    fn a_new_mtgo_data_folder_restarts_tracking_and_an_unchanged_save_does_not() {
        let dir = TestDir::new("commands-data-folder");
        let (state, receiver) = ready_state(&dir);
        assert!(saw_roots(&receiver, &dir.join("logs-a")));
        let same = Settings {
            log_folder: Some(dir.join("logs-a").to_string_lossy().into_owned()),
            close_to_tray: false,
            mtgo_data_folder: None,
            ..Settings::default()
        };
        apply_settings(&state, same.clone()).expect("saved");
        let with_data = Settings {
            mtgo_data_folder: Some(dir.join("mtgo-data").to_string_lossy().into_owned()),
            ..same
        };
        apply_settings(&state, with_data.clone()).expect("saved");
        let mut restarts = 0;
        while let Ok(event) = receiver.recv_timeout(Duration::from_millis(500)) {
            if matches!(event, TrackerEvent::Restarting { .. }) {
                restarts += 1;
            }
        }
        assert_eq!(restarts, 1, "only the data folder change restarts");
        assert_eq!(*lock(&state.tracking_folders), with_data.tracked_folders());
        lock(&state.tracker).take().expect("tracker").stop();
    }

    #[test]
    fn a_retried_save_restarts_on_a_folder_stored_without_a_restart() {
        let dir = TestDir::new("commands-retry");
        let (state, receiver) = ready_state(&dir);
        let new = Settings {
            log_folder: Some(dir.join("logs-b").to_string_lossy().into_owned()),
            close_to_tray: true,
            mtgo_data_folder: None,
            ..Settings::default()
        };
        // A save that timed out in the UI but still committed: stored, no restart.
        state
            .tracker_part(TrackerHandle::settings_saver)
            .expect("tracker")
            .save(new.to_rows())
            .expect("stored");
        // The retry sees the same stored folder but tracking still uses logs-a.
        apply_settings(&state, new).expect("saved");
        assert!(
            saw_roots(&receiver, &dir.join("logs-b")),
            "the retry restarted tracking"
        );
        lock(&state.tracker).take().expect("tracker").stop();
    }

    #[test]
    fn a_save_after_a_failed_restart_starts_tracking_again() {
        let dir = TestDir::new("commands-respawn");
        let (state, receiver) = ready_state(&dir);
        // As if an earlier restart could not spawn the tracker.
        lock(&state.tracker).take().expect("tracker").stop();
        let new = Settings {
            log_folder: Some(dir.join("logs-b").to_string_lossy().into_owned()),
            close_to_tray: false,
            mtgo_data_folder: None,
            ..Settings::default()
        };
        let saved = apply_settings(&state, new.clone()).expect("saved");
        assert_eq!(saved.settings, new);
        assert_eq!(saved.restart_error, None);
        assert_eq!(read_settings(&state).expect("read"), new);
        assert!(saw_roots(&receiver, &dir.join("logs-b")));
        lock(&state.tracker).take().expect("tracker").stop();
    }

    #[test]
    fn a_folder_change_resets_the_live_views_before_the_new_tracker_reports() {
        let dir = TestDir::new("commands-reset");
        let (state, receiver) = ready_state(&dir);
        assert!(saw_roots(&receiver, &dir.join("logs-a")));
        apply_settings(
            &state,
            Settings {
                log_folder: Some(dir.join("logs-b").to_string_lossy().into_owned()),
                close_to_tray: true,
                mtgo_data_folder: None,
                ..Settings::default()
            },
        )
        .expect("saved");
        let new_root = dir.join("logs-b");
        let mut reset_seen = false;
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline {
            match receiver.recv_timeout(Duration::from_millis(100)) {
                Ok(TrackerEvent::Restarting { .. }) => reset_seen = true,
                Ok(TrackerEvent::Status(status)) if status.roots.contains(&new_root) => {
                    assert!(reset_seen, "Restarting must arrive before the new status");
                    lock(&state.tracker).take().expect("tracker").stop();
                    return;
                }
                _ => {}
            }
        }
        panic!("the restarted tracker never reported logs-b");
    }

    #[test]
    fn a_failed_restart_after_a_stored_save_still_reports_what_was_stored() {
        let dir = TestDir::new("commands-restart-fails");
        let (state, _receiver) = ready_state(&dir);
        let new = Settings {
            log_folder: Some(dir.join("logs-b").to_string_lossy().into_owned()),
            close_to_tray: false,
            mtgo_data_folder: None,
            ..Settings::default()
        };
        fn cannot_spawn(
            _config: TrackerConfig,
            _sink: Sender<TrackerEvent>,
        ) -> std::io::Result<TrackerHandle> {
            Err(std::io::Error::other("no threads left"))
        }
        let applied = apply_settings_with(&state, new.clone(), cannot_spawn).expect("stored");
        assert_eq!(applied.settings, new);
        assert_eq!(read_settings(&state).expect("read"), new);
        let error = applied.restart_error.expect("the restart failed");
        assert!(matches!(error, CommandError::Tracker(_)), "{error:?}");
        assert!(lock(&state.tracker).is_none());
    }

    #[test]
    fn a_save_with_a_network_folder_is_refused_and_stores_nothing() {
        let dir = TestDir::new("commands-remote-folder");
        let (state, _receiver) = ready_state(&dir);
        let before = read_settings(&state).expect("read");
        for settings in [
            Settings {
                log_folder: Some(r"\\host\share\logs".to_string()),
                ..Settings::default()
            },
            Settings {
                mtgo_data_folder: Some(" //host/share".to_string()),
                ..Settings::default()
            },
        ] {
            let error = apply_settings(&state, settings).expect_err("refused");
            assert_eq!(
                error,
                CommandError::Refused(crate::paths::REMOTE_PATH_REFUSED.to_string())
            );
        }
        assert_eq!(read_settings(&state).expect("read"), before);
    }

    #[test]
    fn a_save_waits_for_a_save_in_progress() {
        let dir = TestDir::new("commands-lock");
        let (state, _receiver) = ready_state(&dir);
        let settings = Settings {
            log_folder: Some(dir.join("logs-b").to_string_lossy().into_owned()),
            close_to_tray: true,
            mtgo_data_folder: None,
            ..Settings::default()
        };
        let (done_sender, done) = mpsc::channel();
        let held = lock(&state.settings_lock);
        std::thread::scope(|scope| {
            scope.spawn(|| {
                let result = apply_settings(&state, settings.clone());
                done_sender.send(result.is_ok()).expect("send");
            });
            // Generous wait: a save that ignores the lock finishes in far less.
            let waited = done.recv_timeout(Duration::from_millis(500)).is_err();
            // Release before asserting: a failed assert must not leave the
            // spawned save blocked on the lock, or the scope never ends.
            drop(held);
            assert!(waited, "the second save must wait for the first");
            assert!(
                done.recv_timeout(Duration::from_secs(30))
                    .expect("finished")
            );
        });
        assert_eq!(
            read_settings(&state).expect("read").tracked_folders(),
            *lock(&state.tracking_folders)
        );
        lock(&state.tracker).take().expect("tracker").stop();
    }

    #[test]
    fn a_page_lays_live_views_over_storage() {
        let dir = TestDir::new("commands-page");
        let (state, _receiver) = ready_state(&dir);
        lock(&state.shared.live).update(crate::summary::testing::summary(
            "live",
            5_000,
            1,
            crate::summary::SummaryState::InProgress,
        ));
        let page = matches_page(&state, &MatchListQuery::newest_first(1)).expect("page");
        assert_eq!(page.total, 0, "nothing stored yet");
        assert_eq!(
            page.rows.len(),
            1,
            "the live match shows before it is stored"
        );
        assert_eq!((page.page, page.per_page), (1, crate::live::PER_PAGE));
        // A filtered or re-sorted list cannot place a match storage has not
        // written: it shows stored rows only.
        let losses = MatchListQuery {
            result: Some(crate::matches::MatchResultFilter::Loss),
            ..MatchListQuery::newest_first(1)
        };
        assert!(matches_page(&state, &losses).expect("page").rows.is_empty());
        lock(&state.tracker).take().expect("tracker").stop();
    }

    #[test]
    fn errors_reach_the_ui_as_kind_and_message() {
        let json = serde_json::to_value(CommandError::Tracker("busy".to_string())).expect("json");
        assert_eq!(
            json,
            serde_json::json!({ "kind": "tracker", "message": "busy" })
        );
        let failed = Startup::Failed {
            data_dir: "D".to_string(),
            error: "E".to_string(),
        };
        assert_eq!(
            ready(&failed).err(),
            Some(CommandError::NotReady("E".to_string()))
        );
    }

    #[test]
    fn stopping_the_tracker_releases_it_and_keeps_saved_settings() {
        let dir = TestDir::new("commands-stop-for-install");
        let (state, _events) = ready_state(&dir);
        let saved = Settings {
            log_folder: lock(&state.tracking_folders).log_folder.clone(),
            close_to_tray: false,
            mtgo_data_folder: None,
            ..Settings::default()
        };
        apply_settings(&state, saved.clone()).expect("save");
        stop_tracker(&state);
        assert!(lock(&state.tracker).is_none());
        stop_tracker(&state); // a second stop does nothing
        assert_eq!(read_settings(&state).expect("read"), saved);
    }

    #[test]
    fn app_info_reports_whether_a_tray_exists() {
        let startup = Startup::Failed {
            data_dir: "C:/Users/Q/AppData/Roaming/com.mymtgo.desktop".to_string(),
            error: "boom".to_string(),
        };
        let with_tray = app_info_for(&startup, "1.0.0".to_string(), "MyMTGO".to_string(), true);
        assert!(with_tray.tray_available);
        assert_eq!(with_tray.version, "1.0.0");
        assert_eq!(with_tray.fatal.as_deref(), Some("boom"));
        assert!(
            !app_info_for(&startup, "1.0.0".to_string(), "MyMTGO".to_string(), false)
                .tray_available
        );
    }

    #[test]
    fn app_info_names_the_product_and_whether_the_build_is_a_prerelease() {
        // Release spec 13.1: the name is read at runtime, the beta tag
        // follows the version.
        let startup = Startup::Failed {
            data_dir: "D".to_string(),
            error: "E".to_string(),
        };
        let beta = app_info_for(
            &startup,
            "1.0.0-beta.2".to_string(),
            "Qapp".to_string(),
            true,
        );
        assert_eq!(beta.product_name, "Qapp");
        assert!(beta.prerelease);
        let stable = app_info_for(&startup, "1.0.0".to_string(), "Qapp".to_string(), true);
        assert!(!stable.prerelease);
        assert_eq!(stable.os, std::env::consts::OS);
    }

    #[test]
    fn the_log_folder_is_inside_the_data_folder() {
        let data_dir = std::path::Path::new("data");
        assert_eq!(app_log_folder(data_dir), data_dir.join("logs"));
    }
}
