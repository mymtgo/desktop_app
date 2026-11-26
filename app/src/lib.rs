//! The desktop app (spec 3): Tauri wiring around the tracker. Logic lives in
//! small modules that are tested without a Tauri runtime.

pub mod accounts;
pub mod api;
pub mod archetypes;
pub mod bindings;
pub mod bridge;
pub mod cards;
pub mod commands;
pub mod console;
pub mod dashboard;
pub mod decks;
pub mod endstep;
#[cfg(test)]
mod endstep_seed;
pub mod events;
pub mod external;
pub mod first_run;
pub mod guides;
pub mod hash;
pub mod history_import;
pub mod identity;
pub mod integrations;
pub mod leagues;
pub mod legacy;
pub mod limited;
pub mod live;
pub mod logging;
pub mod manual;
pub mod matches;
pub mod opponents;
pub mod overlay;
pub mod paths;
pub mod platform;
pub mod rank;
pub mod recorder;
pub mod replay;
pub mod reporting;
pub mod scrub;
pub mod settings;
pub mod startup;
pub mod summary;
pub mod support;
pub mod timezone;
pub mod updates;
pub mod usage;
pub mod usage_marks;
pub mod whats_new;
pub mod window;
pub mod worker_thread;
pub mod wrapped;

use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, mpsc};

use tauri::{App, AppHandle, Manager, RunEvent};

use crate::bridge::{Shared, lock};
use crate::commands::{AppState, Startup};
use crate::window::WindowPrefs;

/// The windows whose initial restore the window-state plugin skips: the
/// overlays, the replay window and the data console. Their manager places them itself from the
/// plugin's saved state (overlay spec 5.2, replays spec section 6), and the
/// plugin still saves their bounds.
fn placed_by_the_app() -> Vec<&'static str> {
    let mut labels = overlay::OVERLAY_LABELS.to_vec();
    labels.push(replay::window::GAME_REPLAY);
    labels.push(console::window::CONSOLE);
    labels
}

/// The window-state plugin.
fn window_state() -> tauri::plugin::TauriPlugin<tauri::Wry> {
    let mut builder = tauri_plugin_window_state::Builder::default();
    for label in placed_by_the_app() {
        builder = builder.skip_initial_state(label);
    }
    builder.build()
}

/// Bring the main window to the front (second launch, tray "Open").
pub fn show_main(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

pub fn run() {
    // First, so a panic anywhere after this is reported (spec 9).
    let _reports = reporting::init(env!("CARGO_PKG_VERSION"));
    let builder = bindings::builder();
    // `tauri dev` writes the page's bindings at every start, so a new or
    // changed command reaches the UI without a test run. Release builds never
    // write; the `export_bindings` test keeps the committed file in step.
    #[cfg(debug_assertions)]
    if let Err(error) = builder.export(
        specta_typescript::Typescript::default(),
        concat!(env!("CARGO_MANIFEST_DIR"), "/../ui/src/bindings.ts"),
    ) {
        eprintln!("could not write the TypeScript bindings: {error}");
    }
    let app = tauri::Builder::default()
        // Must be the first plugin: a second launch focuses this one instead of
        // starting a second writer (spec 3, single instance).
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            show_main(app);
        }))
        .plugin(autostart())
        .plugin(window_state())
        .manage(WindowPrefs::default())
        .manage(archetypes::cache::ArchetypeCaches::default())
        .manage(console::ConsoleState::default())
        .manage(updates::Updates::new(
            env!("CARGO_PKG_VERSION"),
            !cfg!(debug_assertions),
        ))
        .on_window_event(window::on_window_event)
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(usage_plugin())
        // Rust side only (`external::open_external`): no injected click
        // handler, and no capability grants the plugin's JS commands.
        .plugin(
            tauri_plugin_opener::Builder::new()
                .open_js_links_on_click(false)
                .build(),
        )
        .invoke_handler(builder.invoke_handler())
        // Local card images (cards spec section 7): read off the IPC thread.
        .register_asynchronous_uri_scheme_protocol(
            cards::scheme::SCHEME,
            |ctx, request, responder| {
                let app = ctx.app_handle().clone();
                let path = request.uri().path().to_string();
                tauri::async_runtime::spawn_blocking(move || {
                    let folder = app
                        .try_state::<cards::images::ImagesState>()
                        .map(|images| images.folder().path);
                    responder.respond(cards::scheme::respond(folder.as_deref(), &path));
                });
            },
        )
        // The league card's custom background (overlay spec 4).
        .register_asynchronous_uri_scheme_protocol(
            overlay::background::SCHEME,
            |ctx, request, responder| {
                let app = ctx.app_handle().clone();
                let path = request.uri().path().to_string();
                tauri::async_runtime::spawn_blocking(move || {
                    responder.respond(overlay::background::respond_for_app(&app, &path));
                });
            },
        )
        // A deck's own cover image.
        .register_asynchronous_uri_scheme_protocol(
            decks::cover::SCHEME,
            |ctx, request, responder| {
                let app = ctx.app_handle().clone();
                let path = request.uri().path().to_string();
                tauri::async_runtime::spawn_blocking(move || {
                    responder.respond(decks::cover::respond_for_app(&app, &path));
                });
            },
        )
        .setup(move |app| {
            if let Ok(data_dir) = app.path().app_data_dir() {
                match logging::init(&data_dir.join(logging::LOG_FOLDER)) {
                    Ok(guard) => {
                        app.manage(logging::LogGuard(Mutex::new(Some(guard))));
                    }
                    // Logging is a diagnostic: the app still runs without it.
                    Err(error) => eprintln!("logging is off: {error}"),
                }
            }
            logging::install_panic_hook();
            // Leaving the beta name on Linux (release spec 13.1): once, and
            // inert while the product is still "Beta". After the autostart
            // plugin's setup, which runs before this.
            identity::fix_linux_autostart(app.handle());
            reporting::self_test(
                std::env::var("MYMTGO_SENTRY_SELFTEST").is_ok_and(|value| value == "1"),
            );
            tracing::info!(version = %app.package_info().version, "MyMTGO starting");
            builder.mount_events(app);
            let support = window::WindowSupport::detect();
            tracing::info!(
                overlay_windows_available = support.overlay_windows_available,
                "window support"
            );
            app.manage(support);
            let startup = start(app);
            // The helper's registration follows the Endstep switch at every
            // start: on registers it, off removes any stale registration
            // (files and the registry only).
            if let Startup::Ready(state) = &startup {
                endstep::host::spawn_sync(
                    state.data_dir.clone(),
                    state.db_path.clone(),
                    Arc::clone(&state.endstep_helper_missing),
                    || {},
                );
            }
            let close_to_tray = close_to_tray_at_start(&startup);
            app.manage(api_state(app, &startup));
            let tracking = matches!(startup, Startup::Ready(_));
            // "Share anonymous usage data", off until the settings are read.
            let share_usage = match &startup {
                Startup::Ready(state) => commands::read_settings(state)
                    .map(|settings| settings.share_usage)
                    .unwrap_or(false),
                _ => false,
            };
            app.manage(usage::UsageGate::new(share_usage));
            if let Startup::Ready(state) = &startup {
                let settings = commands::read_settings(state);
                if let Some(images) =
                    cards::images::images_state_at_start(settings, &state.data_dir)
                {
                    app.manage(images);
                }
            }
            app.manage(startup);
            // The update channel follows the stored `beta_updates`.
            updates::load_channel(app.handle());
            // After the API state and startup are managed: the worker reads
            // both. Without tracking there is nothing to ship.
            if tracking {
                app.manage(api::worker::ApiWorker::start(app.handle().clone()));
                app.manage(cards::worker::CardsWorker::start(app.handle().clone()));
                app.manage(archetypes::worker::ArchetypesWorker::start(
                    app.handle().clone(),
                ));
                app.manage(recorder::import::ImportWorker::start(app.handle().clone()));
                app.manage(endstep::import::InboxWorker::start(app.handle().clone()));
                app.manage(recorder::supervisor::RecorderWorker::start(
                    app.handle().clone(),
                ));
            }
            // The first run's 0.x question (release spec 13.2): decided, and
            // a migration started, before the main window opens and asks for
            // What's new; after the workers the import hands work to.
            first_run::start(app.handle());
            window::set_close_to_tray(app.handle(), close_to_tray);
            match window::build_tray_safely(|| window::build_tray(app)) {
                Ok(()) => {
                    app.state::<WindowPrefs>()
                        .tray_available
                        .store(true, std::sync::atomic::Ordering::SeqCst);
                }
                Err(error) => {
                    tracing::warn!(%error, "no tray on this desktop; closing the window quits")
                }
            }
            // The window is created only now, so every command finds the state
            // it needs (`"create": false` in tauri.conf.json).
            let config = app.config().app.windows[0].clone();
            tauri::WebviewWindowBuilder::from_config(app.handle(), &config)?.build()?;
            if let Startup::Ready(state) = &*app.state::<Startup>()
                && let Ok(settings) = commands::read_settings(state)
            {
                window::apply_interface_scale(app.handle(), settings.interface_scale);
            }
            updates::start_checking(app.handle().clone());
            if matches!(&*app.state::<Startup>(), Startup::Ready(_)) {
                usage_marks::start(app.handle());
            }
            Ok(())
        })
        .build(context())
        .expect("the app failed to start");
    app.run(|handle, event| {
        if let RunEvent::Exit = event {
            prepare_exit(handle);
        }
    });
}

/// Aptabase when a key was built in (usage analytics spec); otherwise an
/// empty plugin, so a build without the key sends nothing.
fn usage_plugin() -> tauri::plugin::TauriPlugin<tauri::Wry> {
    match usage::app_key() {
        Some(key) => tauri_plugin_aptabase::Builder::new(key).build(),
        None => tauri::plugin::Builder::new("usage-off").build(),
    }
}

/// Launch at login. A debug build names its entry `identity::DEV_LAUNCH_NAME`,
/// so its switch never reads or replaces the installed app's entry, which is
/// named after the product. macOS uses the plugin's default Launch Agent
/// (`macos_launcher` exists only in macOS builds, so it is not called here).
fn autostart() -> tauri::plugin::TauriPlugin<tauri::Wry> {
    let builder = tauri_plugin_autostart::Builder::new();
    if cfg!(debug_assertions) {
        builder.app_name(identity::DEV_LAUNCH_NAME).build()
    } else {
        builder.build()
    }
}

/// The Tauri context with this build's identifier: a debug build gets its own
/// data folder, keychain service and single instance lock
/// (`identity::APP_IDENTIFIER`).
fn context() -> tauri::Context<tauri::Wry> {
    let mut context = tauri::generate_context!();
    context.config_mut().identifier = identity::APP_IDENTIFIER.to_string();
    context
}

/// Whether closing the window hides it to the tray when the app starts.
/// A failed start never does: closing the fatal screen must quit, so opening
/// the app again is a fresh launch that can retry.
fn close_to_tray_at_start(startup: &Startup) -> bool {
    match startup {
        Startup::Ready(state) => commands::read_settings(state)
            .map(|settings| settings.close_to_tray)
            .unwrap_or(true),
        Startup::Failed { .. } => false,
    }
}

/// The MyMTGO API state from the stored settings rows. Nothing is sent and
/// the keychain is not opened here: both happen at first API use, off the
/// main thread. Rows that cannot be read leave the app offline (offline mode
/// fails closed); the status bar starts with the flag the gate has.
fn api_state(app: &App, startup: &Startup) -> api::session::ApiState {
    let rows = match startup {
        Startup::Ready(state) => store::read::settings(&lock(&state.reader)).map_err(|error| {
            tracing::warn!(%error, "could not read the API settings, so offline mode is on");
        }),
        Startup::Failed { .. } => Err(()),
    };
    // Without a data folder there is no secrets file: never the working
    // directory.
    let data_dir = app.path().app_data_dir().ok();
    let api = api::session::ApiState::for_app(
        app.handle().clone(),
        data_dir,
        rows.as_deref().map_err(|_| ()),
    );
    if let Startup::Ready(state) = startup {
        let (offline, _) = api.offline_mode();
        lock(&state.shared.status).offline_mode = offline;
    }
    api
}

/// Start tracking, or describe why it could not start.
fn start(app: &App) -> Startup {
    let data_dir = match app.path().app_data_dir() {
        Ok(dir) => dir,
        Err(error) => {
            return Startup::Failed {
                data_dir: String::new(),
                error: format!("could not find the app data folder: {error}"),
            };
        }
    };
    match start_tracking(app.handle(), &data_dir) {
        Ok(state) => Startup::Ready(Box::new(state)),
        Err(error) => {
            tracing::error!(%error, "startup failed");
            Startup::Failed {
                data_dir: data_dir.to_string_lossy().into_owned(),
                error,
            }
        }
    }
}

fn start_tracking(app: &AppHandle, data_dir: &Path) -> Result<AppState, String> {
    let prepared = startup::prepare(data_dir, timezone::system_timezone())?;
    match store::read::player_names(&prepared.reader) {
        Ok(names) => scrub::remember(names),
        Err(error) => tracing::warn!(%error, "could not read player names for the scrubber"),
    }
    let shared = new_shared(&prepared);
    let (sender, receiver) = mpsc::channel();
    // Made before the bridge, so what the tracker says before the overlay
    // worker runs waits in its channel.
    let (overlay_sender, overlay_inbox) = mpsc::channel();
    // The bridge starts first so it hears the tracker's first status.
    bridge::spawn(
        app.clone(),
        receiver,
        Arc::clone(&shared),
        prepared.db_path.clone(),
        overlay_sender.clone(),
    )
    .map_err(|error| format!("could not start the bridge: {error}"))?;
    let available = app
        .try_state::<window::WindowSupport>()
        .is_some_and(|support| support.overlay_windows_available);
    app.manage(overlay::OverlayWorker::start(
        app.clone(),
        overlay_sender,
        overlay_inbox,
        prepared.db_path.clone(),
        overlay::background::folder(data_dir),
        available,
    ));
    let presence = store::platform::platform_presence(&prepared.reader)
        .map_err(|error| format!("could not read which platforms have data: {error}"))?;
    let config = commands::tracker_config(
        prepared.db_path.clone(),
        prepared.timezone,
        &prepared.settings,
        &presence,
    );
    let tracker = tracker::spawn(config, sender.clone())
        .map_err(|error| format!("could not start tracking: {error}"))?;
    Ok(AppState {
        data_dir: data_dir.to_path_buf(),
        db_path: prepared.db_path,
        timezone: prepared.timezone,
        reader: Mutex::new(prepared.reader),
        shared,
        tracker: Mutex::new(Some(tracker)),
        tracker_events: sender,
        tracking_folders: Mutex::new(prepared.settings.tracked_folders()),
        settings_lock: Mutex::new(()),
        import_running: AtomicBool::new(false),
        game_logs: Mutex::new(matches::log_cache::GameLogCache::new()),
        history: history_import::scan::HistoryScans::new(),
        fresh_install: prepared.fresh_install,
        endstep_helper_missing: Arc::new(AtomicBool::new(false)),
    })
}

/// The state the bridge writes and the commands read, with offline mode
/// set before the tracker's first status, so that status carries it.
fn new_shared(prepared: &startup::Prepared) -> Arc<Shared> {
    let shared = Arc::new(Shared::new(prepared.timezone_warning.clone()));
    lock(&shared.status).offline_mode = prepared.offline_mode;
    shared
}

/// Now, in UTC milliseconds. Only for app state (the last update check),
/// never for match data: that is dated from the logs (spec 5.4).
pub fn wall_clock_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

/// Stop the API, cards, overlay and archetypes workers, then tracking (its last batch commits), and flush
/// the log files and Sentry. Runs on exit, and before an update install
/// closes the process.
pub fn prepare_exit(app: &AppHandle) {
    if let Some(worker) = app.try_state::<api::worker::ApiWorker>() {
        worker.stop();
    }
    if let Some(worker) = app.try_state::<cards::worker::CardsWorker>() {
        worker.stop();
    }
    if let Some(worker) = app.try_state::<overlay::OverlayWorker>() {
        worker.stop();
    }
    if let Some(worker) = app.try_state::<archetypes::worker::ArchetypesWorker>() {
        worker.stop();
    }
    if let Some(worker) = app.try_state::<recorder::import::ImportWorker>() {
        worker.stop();
    }
    if let Some(worker) = app.try_state::<endstep::import::InboxWorker>() {
        worker.stop();
    }
    // The recorder stops gracefully (stdin closed, up to 10 s, then killed).
    if let Some(worker) = app.try_state::<recorder::supervisor::RecorderWorker>() {
        worker.stop();
    }
    stop_tracking(app);
    if let Some(guard) = app.try_state::<logging::LogGuard>() {
        lock(&guard.0).take();
    }
    reporting::flush();
}

/// Stop the tracker so its pending writes commit before the process ends.
pub(crate) fn stop_tracking(app: &AppHandle) {
    if let Some(startup) = app.try_state::<Startup>()
        && let Startup::Ready(state) = startup.inner()
    {
        commands::stop_tracker(state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::window::{CloseAction, close_action};
    use serde_json::Value;

    #[test]
    fn the_app_places_the_overlays_the_replay_window_and_the_console_itself() {
        assert_eq!(
            placed_by_the_app(),
            vec![
                "game-overlay",
                "draft-notes",
                "league-overlay",
                "game-replay",
                "console"
            ]
        );
    }

    #[test]
    fn the_ui_package_has_the_app_version() {
        let package: Value = serde_json::from_str(include_str!("../../ui/package.json"))
            .expect("package.json parses");
        assert_eq!(package["version"], env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn the_shared_status_starts_with_offline_mode() {
        let dir = fixture_tools::TestDir::new("lib-shared-offline");
        let mut prepared =
            startup::prepare(&dir.join("data"), (chrono_tz::Tz::UTC, None)).expect("prepare");
        prepared.offline_mode = true;
        assert!(lock(&new_shared(&prepared).status).offline_mode);
        prepared.offline_mode = false;
        assert!(!lock(&new_shared(&prepared).status).offline_mode);
    }

    #[test]
    fn a_failed_start_quits_when_the_window_closes() {
        let failed = Startup::Failed {
            data_dir: "D".to_string(),
            error: "E".to_string(),
        };
        let close_to_tray = close_to_tray_at_start(&failed);
        assert!(!close_to_tray);
        assert_eq!(close_action(close_to_tray, true), CloseAction::Close);
    }

    fn config() -> Value {
        serde_json::from_str(include_str!("../tauri.conf.json")).expect("tauri.conf.json parses")
    }

    #[test]
    fn the_identifier_is_permanent_and_only_a_prerelease_keeps_the_beta_name() {
        let config = config();
        // The identifier names the data folder: it never changes, so leaving
        // beta moves no data (release spec 13.1). The last beta keeps the
        // beta name; a stable build is "MyMTGO".
        assert_eq!(config["identifier"], "com.mymtgo.desktop");
        assert_eq!(config["version"], env!("CARGO_PKG_VERSION"));
        let name = config["productName"].as_str().expect("productName");
        if identity::is_prerelease(env!("CARGO_PKG_VERSION")) {
            assert!(
                name == identity::BETA_PRODUCT_NAME || name == "MyMTGO",
                "{name}"
            );
        } else {
            assert_eq!(name, "MyMTGO");
        }
    }

    /// (width, height, bits per pixel) from a BMP header.
    fn bmp_shape(path: &std::path::Path) -> (i32, i32, u16) {
        let bytes = std::fs::read(path).expect("read bmp");
        assert_eq!(&bytes[0..2], b"BM", "{} is a BMP", path.display());
        let width = i32::from_le_bytes(bytes[18..22].try_into().expect("width"));
        let height = i32::from_le_bytes(bytes[22..26].try_into().expect("height"));
        let bits = u16::from_le_bytes(bytes[28..30].try_into().expect("bits"));
        (width, height.abs(), bits)
    }

    #[test]
    fn the_installer_uses_the_branded_images() {
        let config = config();
        let nsis = &config["bundle"]["windows"]["nsis"];
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        assert_eq!(nsis["installerIcon"], "icons/icon.ico");
        let header = nsis["headerImage"].as_str().expect("headerImage");
        let sidebar = nsis["sidebarImage"].as_str().expect("sidebarImage");
        // NSIS expects 24-bit BMPs of exactly these sizes.
        assert_eq!(bmp_shape(&root.join(header)), (150, 57, 24));
        assert_eq!(bmp_shape(&root.join(sidebar)), (164, 314, 24));
    }

    #[test]
    fn the_linux_desktop_entry_has_a_category_and_comment() {
        // The AppImage's .desktop entry takes Categories and Comment from these.
        let config = config();
        assert_eq!(config["bundle"]["category"], "Utility");
        assert!(
            config["bundle"]["shortDescription"]
                .as_str()
                .is_some_and(|text| !text.is_empty())
        );
    }

    // Arena replays show the printing played through mymtgo.com's card image
    // redirect (`/api/card-images/arena/{grpId}`), which answers with the
    // site's own copy (cards.mymtgo.com) or Scryfall's image API.
    #[test]
    fn images_load_only_from_the_app_scryfall_mymtgo_and_the_card_deckcover_and_overlay_schemes() {
        let csp = config()["app"]["security"]["csp"]
            .as_str()
            .expect("csp")
            .to_string();
        let img_src = csp
            .split(';')
            .map(str::trim)
            .find(|directive| directive.starts_with("img-src"))
            .expect("img-src");
        assert_eq!(
            img_src,
            "img-src 'self' data: https://cards.scryfall.io https://mymtgo.com https://cards.mymtgo.com https://api.scryfall.com card: http://card.localhost deckcover: http://deckcover.localhost overlay: http://overlay.localhost"
        );
    }

    // "Copy as image" (`html-to-image`) fetches each picture to inline it,
    // so the card images the Metagame tiles show must be fetchable too, and
    // Wrapped's art, which loads through Scryfall's API redirects.
    #[test]
    fn fetches_reach_only_the_app_ipc_the_card_image_hosts_and_the_card_scheme() {
        let csp = config()["app"]["security"]["csp"]
            .as_str()
            .expect("csp")
            .to_string();
        let connect_src = csp
            .split(';')
            .map(str::trim)
            .find(|directive| directive.starts_with("connect-src"))
            .expect("connect-src");
        assert_eq!(
            connect_src,
            "connect-src 'self' ipc: http://ipc.localhost https://cards.scryfall.io https://api.scryfall.com https://cards.mymtgo.com card: http://card.localhost"
        );
    }

    #[test]
    fn the_main_window_matches_0x_and_is_created_by_setup() {
        let window = &config()["app"]["windows"][0];
        assert_eq!(window["label"], "main");
        assert_eq!(
            (window["width"].as_u64(), window["height"].as_u64()),
            (Some(1600), Some(900))
        );
        assert_eq!(
            (window["minWidth"].as_u64(), window["minHeight"].as_u64()),
            (Some(1200), Some(800))
        );
        // Setup creates it after the app state exists (Task 7).
        assert_eq!(window["create"], false);
    }
}
