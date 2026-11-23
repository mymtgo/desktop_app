//! The hosted OBS overlay (overlay spec 10; 0.x `PublishOverlayState`,
//! `ClearPublishedOverlay`, `SyncOverlayBackground`, `PublishOverlayStateJob`).
//!
//! - **Publish**: the league card with public art (`overlay::publish_state`)
//!   goes to `PUT /api/overlay` for the login id of the current MTGO account
//!   (never guessed). Only when `overlay_publish` is on and the gate allows
//!   `Sync` (signed in, online). A 2xx stores the time and clears
//!   the error; 409, 422 and anything else (network included) record why. A
//!   failed push is dropped: the next change or heartbeat sends the state
//!   current then.
//! - **Custom background**: uploaded first when its content hash (SHA-256,
//!   ruling C11) differs from the stored remote copy's; a failed upload
//!   falls back to the deck art.
//! - **Clear** (publishing switched off): the two internal rows are cleared,
//!   then `DELETE /api/overlay/{login_id}`, best effort (404 is done).
//! - **Forget** (the background removed): `DELETE /api/overlay/background`,
//!   best effort, and the remote row cleared.
//!
//! Every call runs on the `overlay-net` thread, one at a time; publishes
//! share one waiting slot, so a burst sends once (0.x's unique job). Each
//! job reads what it sends when it starts. The `overlay` thread decides
//! when (`Worker`): every changed card, a 30 s heartbeat while on and
//! allowed, and when switched on, at least a second apart (`PUBLISH_GAP_MS`).
//! Rows are stored through the tracker.

use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

use mymtgo_api::{GateDecision, GateState, RequestClass};
use overlay::{OverlayArtwork, PublishError, art_mode, publish_error, publish_state, window_art};
use sha2::{Digest, Sha256};
use store::Connection;
use tauri::{AppHandle, Manager};

use crate::api::session::ApiState;
use crate::api::sync_runner::{SyncError, boundary, sync_send};
use crate::overlay::background::background_file;
use crate::overlay::events::OverlayPublishChanged;
use crate::overlay::game::KnownOpponent;
use crate::overlay::game_net::GameNet;
use crate::overlay::league::league_card;
use crate::overlay::net::{NetJob, NetSignal};
use crate::overlay::worker::OverlayMsg;
use crate::settings::{
    BackgroundRemote, Settings, overlay_background_file, overlay_background_remote,
    overlay_background_remote_row, overlay_last_published_at, overlay_last_published_at_row,
    overlay_publish_error, overlay_publish_error_row,
};

/// The heartbeat: the hosted page idles 90 s after the last push.
pub const HEARTBEAT_MS: i64 = 30_000;

/// The least time between two league card publishes (league overlay themes
/// spec 6.5): life changes many times a second in combat. A change inside
/// the gap waits and the newest card goes out when the gap ends. The
/// desktop window is never held.
pub const PUBLISH_GAP_MS: i64 = 1_000;

/// Answers a push reports as a reason rather than a failure: 409 (the
/// player is not claimed) and 422 (refused). Anything else is unreachable.
const PUBLISH_PASSTHROUGH: [u16; 2] = [409, 422];

/// A request for the `overlay-net` thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetRequest {
    /// Push the card as it is when the job starts.
    Publish,
    /// Publishing was switched off: blank the hosted page.
    Clear,
    /// The background may have been removed: delete the uploaded copy.
    ForgetBackground,
}

impl NetRequest {
    /// The job's waiting slot: a newer job with the same key replaces a
    /// waiting one (0.x `ShouldBeUniqueUntilProcessing`).
    pub fn key(self) -> &'static str {
        match self {
            NetRequest::Publish => "overlay-publish",
            NetRequest::Clear => "overlay-clear",
            NetRequest::ForgetBackground => "overlay-forget-background",
        }
    }
}

/// What the `overlay` thread asks of the publish side.
pub trait PublishPort: Send {
    /// The gate allows `Sync` now (signed in, online).
    fn allowed(&self) -> bool;
    /// Queue a request on the `overlay-net` thread.
    fn submit(&mut self, request: NetRequest);
}

/// The gate allows publishing.
pub fn sync_allowed(gate: &GateState) -> bool {
    mymtgo_api::gate(RequestClass::Sync, gate) == GateDecision::Send
}

/// Stores settings rows (through the tracker in the app).
pub type SaveRows<'a> = &'a dyn Fn(Vec<(String, String)>) -> Result<(), String>;

/// What a job needs.
pub struct PublishEnv<'a> {
    pub api: &'a ApiState,
    pub conn: &'a Connection,
    /// `<app data>/overlay/`, where the custom background is.
    pub overlay_folder: Option<&'a Path>,
    pub save: SaveRows<'a>,
    /// The archetype the game overlay named for its live match, for the
    /// card's opponent.
    pub known: Option<KnownOpponent>,
}

/// The settings card's publish state from the rows.
pub fn publish_status(rows: &[(String, String)]) -> OverlayPublishChanged {
    OverlayPublishChanged {
        last_published_at_ms: overlay_last_published_at(rows),
        error: overlay_publish_error(rows),
    }
}

/// Push the card (0.x `PublishOverlayState::run`). `None` when nothing was
/// tried (publishing off, the gate closed); otherwise the new state.
pub fn publish(env: &PublishEnv<'_>, now_ms: i64) -> Option<OverlayPublishChanged> {
    let rows = read_rows(env.conn)?;
    let settings = Settings::from_rows(&rows);
    if !settings.overlay_publish || !sync_allowed(&env.api.gate_now()) {
        return None;
    }
    let Some(login_id) = login_id(env.conn) else {
        // Never guessed: a wrong id would publish to someone else's name.
        set_error(env, &rows, Some(PublishError::NoLoginId));
        return Some(status_after(&rows, None, Some(PublishError::NoLoginId)));
    };
    let card = match league_card(
        env.conn,
        &settings,
        env.overlay_folder,
        now_ms,
        env.known.as_ref(),
        &mut GameNet::default(),
    ) {
        Ok(card) => card.state,
        Err(error) => {
            tracing::warn!(%error, "could not read the league card to publish");
            return None;
        }
    };
    let has_background = background_path(env, &rows).is_some();
    let mode = art_mode(settings.overlay_artwork, has_background);
    let uploaded = match mode {
        OverlayArtwork::Custom => sync_background(env, &rows, now_ms),
        OverlayArtwork::Deck | OverlayArtwork::NoArt | OverlayArtwork::Thumbnail => None,
    };
    let public_art = window_art(mode, card.public_deck_art.clone(), uploaded);
    let state = publish_state(&card, public_art, mode == OverlayArtwork::Thumbnail);
    let request = mymtgo_api::publish_overlay_request(login_id, &state);
    let error = match sync_send(env.api, &request, &PUBLISH_PASSTHROUGH, now_ms) {
        Ok(response) if response.is_success() => {
            save(
                env,
                vec![
                    overlay_last_published_at_row(Some(now_ms)),
                    overlay_publish_error_row(None),
                ],
            );
            return Some(status_after(&rows, Some(now_ms), None));
        }
        Ok(response) => publish_error(Some(response.status)),
        // The gate closed meanwhile: nothing was sent.
        Err(SyncError::NotLinked | SyncError::Held) => return None,
        Err(SyncError::Unreachable(message) | SyncError::Failed(message)) => {
            tracing::debug!(%message, "the overlay publish failed");
            publish_error(None)
        }
    };
    set_error(env, &rows, Some(error));
    Some(status_after(&rows, None, Some(error)))
}

/// Blank the hosted page (0.x `ClearPublishedOverlay`): the rows first,
/// then the delete, best effort, when the account and the gate allow.
pub fn clear(env: &PublishEnv<'_>, now_ms: i64) -> OverlayPublishChanged {
    save(
        env,
        vec![
            overlay_last_published_at_row(None),
            overlay_publish_error_row(None),
        ],
    );
    if let Some(login_id) = login_id(env.conn)
        && sync_allowed(&env.api.gate_now())
    {
        let request = mymtgo_api::clear_overlay_request(login_id);
        if let Err(error) = sync_send(
            env.api,
            &request,
            &mymtgo_api::CLEAR_OVERLAY_PASSTHROUGH,
            now_ms,
        ) {
            tracing::debug!(?error, "the overlay clear failed");
        }
    }
    OverlayPublishChanged {
        last_published_at_ms: None,
        error: None,
    }
}

/// Delete the uploaded background once the local one is gone (0.x
/// `SyncOverlayBackground::forget`): best effort, the row is cleared
/// anyway (the next upload replaces the copy). A background still chosen
/// keeps its copy: the next publish uploads it if it changed.
pub fn forget_background(env: &PublishEnv<'_>, now_ms: i64) {
    let Some(rows) = read_rows(env.conn) else {
        return;
    };
    if overlay_background_remote(&rows).is_none() || background_path(env, &rows).is_some() {
        return;
    }
    if sync_allowed(&env.api.gate_now()) {
        let request = mymtgo_api::delete_overlay_background_request();
        if let Err(error) = sync_send(env.api, &request, &[], now_ms) {
            tracing::debug!(?error, "the overlay background delete failed");
        }
    }
    save(env, vec![overlay_background_remote_row(None)]);
}

/// The public URL of the custom background (0.x `SyncOverlayBackground::run`),
/// uploading it first when its content changed since the last upload.
/// `None` when there is no file or the upload failed: the deck art is used.
fn sync_background(env: &PublishEnv<'_>, rows: &[(String, String)], now_ms: i64) -> Option<String> {
    let path = background_path(env, rows)?;
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) => {
            tracing::warn!(%error, "could not read the overlay background");
            return None;
        }
    };
    let hash = sha256_hex(&bytes);
    if let Some(remote) = overlay_background_remote(rows)
        && remote.sha256 == hash
    {
        return Some(remote.url);
    }
    let file_name = path.file_name()?.to_string_lossy().into_owned();
    let request = mymtgo_api::upload_overlay_background_request(&bytes, &file_name, &boundary());
    let url = match sync_send(env.api, &request, &[], now_ms) {
        Ok(response) => mymtgo_api::parse_background_url(&response)?,
        Err(error) => {
            tracing::debug!(?error, "the overlay background upload failed");
            return None;
        }
    };
    let remote = BackgroundRemote {
        url: url.clone(),
        sha256: hash,
    };
    save(env, vec![overlay_background_remote_row(Some(&remote))]);
    Some(url)
}

/// The chosen background file, when it exists.
fn background_path(env: &PublishEnv<'_>, rows: &[(String, String)]) -> Option<PathBuf> {
    let folder = env.overlay_folder?;
    let name = overlay_background_file(rows)?;
    background_file(folder, Some(&name))
}

/// The login id of the current MTGO account (0.x `FindLocalLoginId`).
fn login_id(conn: &Connection) -> Option<i64> {
    match store::overlay::current_login(conn) {
        Ok(login) => login.and_then(|(_, login_id)| login_id),
        Err(error) => {
            tracing::warn!(%error, "could not read the current MTGO account");
            None
        }
    }
}

fn read_rows(conn: &Connection) -> Option<Vec<(String, String)>> {
    match store::read::settings(conn) {
        Ok(rows) => Some(rows),
        Err(error) => {
            tracing::warn!(%error, "could not read the overlay publish settings");
            None
        }
    }
}

/// Store the error row, only when it changes (a heartbeat repeating the
/// same failure writes nothing).
fn set_error(env: &PublishEnv<'_>, rows: &[(String, String)], error: Option<PublishError>) {
    if overlay_publish_error(rows) != error {
        save(env, vec![overlay_publish_error_row(error)]);
    }
}

fn save(env: &PublishEnv<'_>, rows: Vec<(String, String)>) {
    if let Err(error) = (env.save)(rows) {
        tracing::warn!(%error, "could not store the overlay publish state");
    }
}

/// The state the card should show after a publish: built from the rows read
/// before it and what was meant to be saved, so a failed save does not push
/// the stale stored state. `published_at_ms` is the new time, `None` keeping
/// the stored one.
fn status_after(
    rows: &[(String, String)],
    published_at_ms: Option<i64>,
    error: Option<PublishError>,
) -> OverlayPublishChanged {
    OverlayPublishChanged {
        last_published_at_ms: published_at_ms.or_else(|| overlay_last_published_at(rows)),
        error,
    }
}

/// The lowercase hex SHA-256 of `bytes` (ruling C11).
fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Run one request; the answer for the `overlay` thread, if any.
pub fn run_request(request: NetRequest, env: &PublishEnv<'_>, now_ms: i64) -> Option<OverlayMsg> {
    match request {
        NetRequest::Publish => publish(env, now_ms).map(OverlayMsg::PublishAnswered),
        NetRequest::Clear => Some(OverlayMsg::PublishAnswered(clear(env, now_ms))),
        NetRequest::ForgetBackground => {
            forget_background(env, now_ms);
            None
        }
    }
}

/// The app's publish side: the gate from `ApiState`, jobs on the
/// `overlay-net` thread with their own read-only connection, rows stored
/// through the tracker.
pub struct AppPort {
    app: AppHandle,
    net: Sender<NetSignal>,
    db_path: PathBuf,
    overlay_folder: PathBuf,
    conn: Arc<Mutex<Option<Connection>>>,
}

impl AppPort {
    pub fn new(
        app: AppHandle,
        net: Sender<NetSignal>,
        db_path: PathBuf,
        overlay_folder: PathBuf,
    ) -> AppPort {
        AppPort {
            app,
            net,
            db_path,
            overlay_folder,
            conn: Arc::new(Mutex::new(None)),
        }
    }
}

impl PublishPort for AppPort {
    fn allowed(&self) -> bool {
        match self.app.try_state::<ApiState>() {
            Some(api) => sync_allowed(&api.gate_now()),
            None => false,
        }
    }

    fn submit(&mut self, request: NetRequest) {
        let app = self.app.clone();
        let db_path = self.db_path.clone();
        let folder = self.overlay_folder.clone();
        let conn = Arc::clone(&self.conn);
        let job = NetJob {
            key: request.key().to_string(),
            run: Box::new(move || run_in_app(&app, &db_path, &folder, &conn, request)),
        };
        if self.net.send(NetSignal::Job(job)).is_err() {
            tracing::warn!("the overlay network thread has stopped");
        }
    }
}

/// One request in the running app.
fn run_in_app(
    app: &AppHandle,
    db_path: &Path,
    folder: &Path,
    conn: &Mutex<Option<Connection>>,
    request: NetRequest,
) -> Option<OverlayMsg> {
    let api = app.try_state::<ApiState>()?;
    let mut slot = crate::bridge::lock(conn);
    if slot.is_none() {
        match store::open_reader(db_path) {
            Ok(opened) => *slot = Some(opened),
            Err(error) => {
                tracing::warn!(%error, "the overlay publish could not open the database");
                return None;
            }
        }
    }
    let conn = slot.as_ref()?;
    let save = |rows: Vec<(String, String)>| save_through_tracker(app, rows);
    let env = PublishEnv {
        api: &api,
        conn,
        overlay_folder: Some(folder),
        save: &save,
        known: crate::overlay::league::known_opponent(app),
    };
    run_request(request, &env, crate::wall_clock_ms())
}

/// Store rows through the tracker, the only writer.
pub(crate) fn save_through_tracker(
    app: &AppHandle,
    rows: Vec<(String, String)>,
) -> Result<(), String> {
    let startup = app.state::<crate::commands::Startup>();
    let state = crate::commands::ready(&startup).map_err(|error| format!("{error:?}"))?;
    state.save_setting_rows(rows)
}

#[cfg(test)]
mod tests;
