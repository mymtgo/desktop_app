//! Local card images (cards spec section 7, survey section 4): the images
//! folder, the downloads, and the Settings "Card images" card.
//!
//! Local images are always on: the cards worker downloads each card's
//! `image` to `{catalog_id}.jpg` and its `art_crop` to
//! `{catalog_id}_art.jpg` in v1's images folder, 50 rows per batch, one
//! request at a time, and records each card's files with `CardImagesStored`.
//! Pages show the local file and fall back to the remote image while a card
//! has none. After a 0.x import, 0.x's files are copied first
//! (`legacy_images`), so only what could not be copied is downloaded.
//! Blocking: runs on the cards worker.

use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use cards::images::{
    IMAGE_ORIGINS, art_file, image_file, is_allowed_image_url_in, is_owned_file, size_text,
};
use serde::Serialize;
use store::Connection;
use store::cards::ImageJob;
use tauri::{AppHandle, Manager};
use tracker::CardImagesRecorder;
use ureq::Agent;
use ureq::tls::{RootCerts, TlsConfig, TlsProvider};

use crate::bridge::lock;
use crate::cards::legacy_images::{PendingCopy, copy_legacy_images_into};
use crate::commands::{CommandError, Startup, off_async_workers, ready};
use crate::settings::Settings;

/// The default images folder, inside v1's app data folder.
pub const DEFAULT_FOLDER: &str = "card-images";
/// Rows read per batch (0.x `DownloadAllCardImages` chunks of 50).
pub const BATCH: u32 = 50;
/// Per request (0.x `Http::timeout(15)`).
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
/// The largest image body read; a larger one is refused.
pub const MAX_IMAGE_BYTES: u64 = 4 * 1024 * 1024;

/// v1's images folder: the chosen one, else `<data dir>/card-images`.
pub fn images_folder(settings: &Settings, data_dir: &Path) -> PathBuf {
    match &settings.card_images_path {
        Some(chosen) => PathBuf::from(chosen),
        None => default_folder(data_dir),
    }
}

/// The default images folder (inside v1's app data folder).
pub fn default_folder(data_dir: &Path) -> PathBuf {
    data_dir.join(DEFAULT_FOLDER)
}

/// The images folder the app uses now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImagesFolder {
    pub path: PathBuf,
    /// The default folder (no chosen one).
    pub is_default: bool,
}

/// Managed state: the images folder, read by the `card` scheme at each
/// request and by the worker at the start of each run, and the move state
/// the Settings card shows (in memory, as 0.x kept it in its cache).
#[derive(Debug)]
pub struct ImagesState {
    inner: Mutex<ImagesInner>,
}

#[derive(Debug)]
struct ImagesInner {
    folder: ImagesFolder,
    default: PathBuf,
    moving: bool,
    move_error: Option<String>,
}

impl ImagesState {
    pub fn new(settings: &Settings, data_dir: &Path) -> ImagesState {
        ImagesState {
            inner: Mutex::new(ImagesInner {
                folder: ImagesFolder {
                    path: images_folder(settings, data_dir),
                    is_default: settings.card_images_path.is_none(),
                },
                default: default_folder(data_dir),
                moving: false,
                move_error: None,
            }),
        }
    }

    pub fn folder(&self) -> ImagesFolder {
        lock(&self.inner).folder.clone()
    }

    /// The default folder (inside v1's app data folder).
    pub fn default_folder(&self) -> PathBuf {
        lock(&self.inner).default.clone()
    }

    /// A move is running, and the last move's error.
    pub fn move_state(&self) -> (bool, Option<String>) {
        let inner = lock(&self.inner);
        (inner.moving, inner.move_error.clone())
    }

    /// Start a move when `check` (given the current folder and whether a
    /// move runs) passes: one check and start under one lock, so two
    /// clicks never start two moves.
    pub fn begin_move(
        &self,
        check: impl FnOnce(&Path, bool) -> Result<(), String>,
    ) -> Result<PathBuf, String> {
        let mut inner = lock(&self.inner);
        check(&inner.folder.path, inner.moving)?;
        inner.moving = true;
        inner.move_error = None;
        Ok(inner.folder.path.clone())
    }

    /// Point the app at `path` (the move switched the setting).
    pub fn switch_to(&self, path: &Path) {
        let mut inner = lock(&self.inner);
        let is_default = path == inner.default;
        inner.folder = ImagesFolder {
            path: path.to_path_buf(),
            is_default,
        };
    }

    /// The move ended, with its error if it failed.
    pub fn end_move(&self, error: Option<String>) {
        let mut inner = lock(&self.inner);
        inner.moving = false;
        inner.move_error = error;
    }
}

/// The images state at start, or none when the settings cannot be read
/// (S4-3): the default folder could be the wrong one, so the images are
/// left unmanaged (the scheme 404s and pages show the remote images) until
/// the next start rather than moved or filled in another folder.
pub fn images_state_at_start<E: std::fmt::Debug>(
    settings: Result<Settings, E>,
    data_dir: &Path,
) -> Option<ImagesState> {
    match settings {
        Ok(settings) => Some(ImagesState::new(&settings, data_dir)),
        Err(error) => {
            tracing::warn!(
                ?error,
                "could not read the settings; card images are left alone until the next start"
            );
            None
        }
    }
}

/// Fetches image bytes from the allowed origins (Scryfall's and MyMTGO's
/// image hosts): a plain `ureq` agent, not the API client (no auth
/// headers), redirects never followed.
pub struct Fetcher {
    agent: Agent,
    origins: Vec<String>,
    /// The largest body read (`MAX_IMAGE_BYTES` unless raised).
    max_bytes: u64,
}

impl Fetcher {
    /// The app's fetcher: `https://cards.scryfall.io` and
    /// `https://cards.mymtgo.com` only.
    pub fn standard() -> Fetcher {
        Fetcher::for_origins(
            IMAGE_ORIGINS
                .iter()
                .map(|origin| origin.to_string())
                .collect(),
        )
    }

    /// A fetcher for one other origin (tests use the stub server's).
    pub fn new(origin: &str) -> Fetcher {
        Fetcher::for_origins(vec![origin.to_string()])
    }

    /// A fetcher for exactly these origins.
    pub fn for_origins(origins: Vec<String>) -> Fetcher {
        let tls = TlsConfig::builder()
            .provider(TlsProvider::Rustls)
            .root_certs(RootCerts::WebPki)
            .build();
        let config = Agent::config_builder()
            .http_status_as_error(false)
            .tls_config(tls)
            // A redirect could lead to any host.
            .max_redirects(0)
            .max_redirects_will_error(false)
            .build();
        Fetcher {
            agent: Agent::new_with_config(config),
            origins,
            max_bytes: MAX_IMAGE_BYTES,
        }
    }

    /// The same fetcher with another body cap (the recorder download is
    /// far larger than an image; the card fetchers keep the small cap).
    pub fn with_max_bytes(mut self, max_bytes: u64) -> Fetcher {
        self.max_bytes = max_bytes;
        self
    }

    /// Whether `url` is on an allowed origin.
    pub fn allows(&self, url: &str) -> bool {
        is_allowed_image_url_in(url, &self.origins)
    }

    /// The body of a 2xx answer, at most `MAX_IMAGE_BYTES` and not empty.
    /// A URL outside the allowed origin is refused before any connection.
    pub fn fetch(&self, url: &str) -> Result<Vec<u8>, String> {
        self.fetch_within(url, REQUEST_TIMEOUT)
    }

    /// `fetch` with its own time limit for the whole request (a screenshot's
    /// image waits 5 s, 0.x `EncodeImageAsDataUrl`).
    pub fn fetch_within(&self, url: &str, timeout: Duration) -> Result<Vec<u8>, String> {
        if !self.allows(url) {
            return Err(format!("not an allowed image host: {url}"));
        }
        let request = ureq::http::Request::builder()
            .method(ureq::http::Method::GET)
            .uri(url)
            .body(())
            .map_err(|error| error.to_string())?;
        let configured = self
            .agent
            .configure_request(request)
            .timeout_global(Some(timeout))
            .build();
        let mut response = self
            .agent
            .run(configured)
            .map_err(|error| error.to_string())?;
        let status = response.status().as_u16();
        if !(200..300).contains(&status) {
            return Err(format!("status {status}"));
        }
        // ureq refuses a body that reaches its limit, so read one byte more
        // and check the cap here: exactly `MAX_IMAGE_BYTES` is fine.
        let body = response
            .body_mut()
            .with_config()
            .limit(self.max_bytes + 1)
            .read_to_vec()
            .map_err(|error| error.to_string())?;
        if body.len() as u64 > self.max_bytes {
            return Err(format!("a body over {} bytes", self.max_bytes));
        }
        if body.is_empty() {
            return Err("an empty image".to_string());
        }
        Ok(body)
    }
}

/// What one image run did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImageReport {
    /// Files copied from 0.x's folder (or already there at the same size).
    pub copied: u32,
    pub downloaded: u32,
    pub failed: u32,
    /// The run stopped for waiting work before the queue was done.
    pub interrupted: bool,
    /// The catalog id of the last card a cut run finished.
    pub resume_after: Option<u32>,
    /// The host answered 429 or 503: the run stopped, and the worker asks
    /// again after its retry wait.
    pub rate_limited: bool,
}

/// What an image run needs, so tests run it without the app.
pub struct ImageRun<'a> {
    /// v1's images folder, resolved at the start of the run.
    pub folder: &'a Path,
    /// The default folder (made on demand); a chosen one that is gone
    /// pauses downloads.
    pub is_default: bool,
    /// The queue starts after this catalog id (0 for the whole queue).
    pub start_after: u32,
    pub fetcher: &'a Fetcher,
    /// A fresh read-only connection, `None` when it cannot be opened.
    pub reader: &'a dyn Fn() -> Option<Connection>,
    pub recorder: &'a CardImagesRecorder,
    /// Forgets the local files not among the given names (`CardImagesPresent`);
    /// each download run starts with it, so rows naming files that are not in
    /// the folder (a move that could not say so) are downloaded again.
    pub present: &'a dyn Fn(BTreeSet<String>) -> Result<u32, String>,
    /// 0.x's folder after an import, copied before any download.
    pub legacy: &'a PendingCopy,
    /// Asked between files: true when other work is waiting, and the run
    /// then stops (the worker goes on with it later).
    pub should_yield: &'a dyn Fn() -> bool,
    /// How long to wait after each fetch (`FETCH_PAUSE`; tests use zero).
    pub fetch_pause: Duration,
}

/// The wait between two fetches of a bulk run: Scryfall asks for 50 to 100
/// ms between requests.
pub const FETCH_PAUSE: Duration = Duration::from_millis(75);

/// Whether a fetch error says the host is asking for less traffic (429, or
/// 503 when it is overloaded): the run stops instead of failing the card.
fn is_back_off(error: &str) -> bool {
    error == "status 429" || error == "status 503"
}

/// The image part of a worker round: the copy of 0.x's files (once) and the
/// downloads, when queued or after a population run (the hourly run retries
/// failures). Local images are always on.
pub fn image_round(run: &ImageRun, queued: bool) -> Result<ImageReport, String> {
    if queued {
        return download_images(run);
    }
    Ok(ImageReport::default())
}

/// Copy 0.x's files (when an import handed its folder), then download every
/// file still missing, `BATCH` rows at a time, stopping between files when
/// other work is waiting (`interrupted`).
pub fn download_images(run: &ImageRun) -> Result<ImageReport, String> {
    let mut report = ImageReport::default();
    // A chosen folder that is gone (drive unplugged): files written to the
    // default folder would be stranded, so nothing runs until it is back.
    if is_gone(run) {
        tracing::info!("the card images folder is gone; downloads wait for it");
        return Ok(report);
    }
    forget_missing_files(run);
    if let Some(source) = run.legacy.take() {
        match copy_from_0x(run, &source) {
            Ok(copied) => report.copied = copied,
            Err(error) => {
                // Kept for the next run: only a copy that ran is done.
                run.legacy.restore(source);
                return Err(error);
            }
        }
    }
    let mut after = run.start_after;
    let mut done_through = run.start_after;
    'queue: loop {
        let jobs = read_jobs(run, after)?;
        let Some(last) = jobs.last() else {
            break;
        };
        after = last.catalog_id;
        for job in &jobs {
            // A switch, a move or a waiting button goes first; the rows
            // left keep their null columns and the next run takes them,
            // from the card after the last one done.
            if (run.should_yield)() {
                report.interrupted = true;
                report.resume_after = Some(done_through);
                break 'queue;
            }
            // A chosen folder whose drive went mid-run: nothing more is
            // fetched until it is back.
            if is_gone(run) {
                tracing::info!("the card images folder went away; downloads wait for it");
                break 'queue;
            }
            download_job(run, job, &mut report);
            if report.rate_limited {
                tracing::info!("the image host asked for fewer requests; downloads wait");
                break 'queue;
            }
            done_through = job.catalog_id;
        }
    }
    if report.downloaded > 0 || report.failed > 0 || report.copied > 0 {
        tracing::info!(
            copied = report.copied,
            downloaded = report.downloaded,
            failed = report.failed,
            "card images stored"
        );
    }
    Ok(report)
}

/// A chosen folder that is not there (the default folder is made on demand).
fn is_gone(run: &ImageRun) -> bool {
    !run.is_default && !run.folder.is_dir()
}

/// Null the local columns whose files are not in the images folder. A
/// failure is logged: the next run does it again.
fn forget_missing_files(run: &ImageRun) {
    // A folder that cannot be listed forgets nothing; the next run retries.
    // The default folder is made on demand: one that was never made (or was
    // deleted) holds no files. A chosen folder that is gone never gets here.
    let never_made = run.is_default
        && matches!(fs::metadata(run.folder), Err(error) if error.kind() == std::io::ErrorKind::NotFound);
    let listed = if never_made {
        Ok(BTreeSet::new())
    } else {
        listed_names(run.folder)
    };
    let files = match listed {
        Ok(files) => files,
        Err(error) => {
            tracing::warn!(%error, "skipped the card images presence check");
            return;
        }
    };
    if let Err(error) = (run.present)(files) {
        tracing::warn!(%error, "could not forget the card images missing from the folder");
    }
}

fn read_jobs(run: &ImageRun, after: u32) -> Result<Vec<ImageJob>, String> {
    let conn = (run.reader)().ok_or_else(|| "could not open the database".to_string())?;
    store::cards::images_to_download(&conn, after, BATCH).map_err(|error| error.to_string())
}

/// Copy every queued file 0.x has into v1's folder (`copy_legacy_images`)
/// and record each card's copies. Returns the files copied.
fn copy_from_0x(run: &ImageRun, source: &Path) -> Result<u32, String> {
    let mut copied = 0;
    let mut after = 0;
    loop {
        // Kept for the next run (`restore`) when the folder went mid-copy.
        if is_gone(run) {
            return Err(FOLDER_GONE.to_string());
        }
        let jobs = read_jobs(run, after)?;
        let Some(last) = jobs.last() else {
            break;
        };
        after = last.catalog_id;
        // The folder is checked before each file, not once per batch.
        let ready = || prepare_folder(run.folder, run.is_default);
        let copy = copy_legacy_images_into(source, run.folder, &jobs, &ready);
        for job in &jobs {
            let mut local_image = None;
            let mut local_art = None;
            for (catalog_id, name) in &copy.copied {
                if *catalog_id != job.catalog_id {
                    continue;
                }
                if *name == image_file(job.catalog_id) {
                    local_image = Some(name.clone());
                } else if *name == art_file(job.catalog_id) {
                    local_art = Some(name.clone());
                }
            }
            if local_image.is_some() || local_art.is_some() {
                copied += u32::from(local_image.is_some()) + u32::from(local_art.is_some());
                record(run, job.catalog_id, local_image, local_art);
            }
        }
        if copy.folder_gone {
            tracing::info!("the card images folder went away mid-copy; the copy waits for it");
            return Err(FOLDER_GONE.to_string());
        }
    }
    Ok(copied)
}

/// Download a card's missing files and record what arrived. A failed file
/// keeps its column null, so the next run asks again.
fn download_job(run: &ImageRun, job: &ImageJob, report: &mut ImageReport) {
    let mut local_image = None;
    let mut local_art = None;
    if let Some(url) = job.image.as_deref().filter(|_| !job.has_local_image) {
        local_image = fetch_into(run, url, &image_file(job.catalog_id), report);
    }
    let art_url = job.art_crop.as_deref().filter(|_| !job.has_local_art);
    if let Some(url) = art_url.filter(|_| !report.rate_limited) {
        local_art = fetch_into(run, url, &art_file(job.catalog_id), report);
    }
    if local_image.is_some() || local_art.is_some() {
        record(run, job.catalog_id, local_image, local_art);
    }
}

/// Record a card's files (`CardImagesStored`). A tracker that is busy or
/// stopped is logged and the run goes on: the row keeps its null column, so
/// a later run fetches the file again.
fn record(run: &ImageRun, catalog_id: u32, local_image: Option<String>, local_art: Option<String>) {
    if let Err(error) = run.recorder.stored(catalog_id, local_image, local_art) {
        tracing::warn!(%error, catalog_id, "could not record a card's image files");
    }
}

/// Fetch `url` into `{folder}/{name}`: the name when it is written.
fn fetch_into(run: &ImageRun, url: &str, name: &str, report: &mut ImageReport) -> Option<String> {
    let fetched = run.fetcher.fetch(url);
    if matches!(&fetched, Err(error) if is_back_off(error)) {
        report.rate_limited = true;
        return None;
    }
    if !run.fetch_pause.is_zero() {
        std::thread::sleep(run.fetch_pause);
    }
    let written = fetched.and_then(|bytes| write_image(run.folder, name, &bytes, run.is_default));
    match written {
        Ok(()) => {
            report.downloaded += 1;
            Some(name.to_string())
        }
        Err(error) => {
            report.failed += 1;
            tracing::warn!(%error, url, "Failed to download card image");
            None
        }
    }
}

/// Write `bytes` to `{folder}/{name}` through a temporary file, so a reader
/// never sees half an image. Only the default folder is made when missing
/// (`may_create`): a chosen folder whose drive went is never made again.
/// Nothing is written through a link: the temporary file is made new (a
/// stale one of v1's own is removed first, anything else fails the write),
/// and an entry with the image's name that is not a plain file is left.
pub fn write_image(
    folder: &Path,
    name: &str,
    bytes: &[u8],
    may_create: bool,
) -> Result<(), String> {
    prepare_folder(folder, may_create)?;
    let target = folder.join(name);
    if fs::symlink_metadata(&target).is_ok_and(|meta| !meta.is_file()) {
        return Err(format!("{name} in the images folder is not a file"));
    }
    let temporary = folder.join(format!("{name}.part"));
    let mut file = create_temporary(&temporary)?;
    let written = file.write_all(bytes).and_then(|()| file.sync_all());
    drop(file);
    let outcome = written
        .and_then(|()| fs::rename(&temporary, &target))
        .map_err(|error| format!("the images folder: {error}"));
    if outcome.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    outcome
}

/// The error of a write to a chosen folder that is gone.
pub const FOLDER_GONE: &str = "the images folder is gone";

/// Ready `folder` for a write, checked before each one: the default folder
/// is made when missing; a chosen folder never is (its subfolder was made
/// by the move, and a missing one means a removed drive or an unmounted
/// path, where making it would strand the images on the system disk).
pub fn prepare_folder(folder: &Path, is_default: bool) -> Result<(), String> {
    if is_default {
        fs::create_dir_all(folder).map_err(|error| format!("the images folder: {error}"))
    } else if folder.is_dir() {
        Ok(())
    } else {
        Err(FOLDER_GONE.to_string())
    }
}

/// A new temporary file at `path`. A stale plain file there (a write cut
/// short) is v1's own and is removed first; a link or a folder is not v1's,
/// so it is left and the write fails.
pub fn create_temporary(path: &Path) -> Result<File, String> {
    if let Ok(meta) = fs::symlink_metadata(path) {
        if !meta.is_file() {
            return Err(format!("{} is not v1's file", path.display()));
        }
        fs::remove_file(path).map_err(|error| format!("the images folder: {error}"))?;
    }
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| format!("the images folder: {error}"))
}

/// v1's own image files in `folder` (`is_owned_file`, regular files only,
/// not in subfolders), with their sizes. A folder that cannot be read lists
/// as empty; callers that must not mistake that for "no files" use
/// `listed_files`.
pub fn owned_files(folder: &Path) -> Vec<(PathBuf, u64)> {
    listed_files(folder).unwrap_or_default()
}

/// Like `owned_files`, but any failure to read the folder or one of its
/// entries is an error, so "could not look" is never taken for "empty".
pub fn listed_files(folder: &Path) -> Result<Vec<(PathBuf, u64)>, String> {
    let entries = fs::read_dir(folder)
        .map_err(|error| format!("could not read {}: {error}", folder.display()))?;
    let mut files = Vec::new();
    for entry in entries {
        let entry =
            entry.map_err(|error| format!("could not read {}: {error}", folder.display()))?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if !is_owned_file(name) {
            continue;
        }
        // `symlink_metadata`: a link with an owned name is not followed.
        if let Ok(meta) = fs::symlink_metadata(entry.path())
            && meta.is_file()
        {
            files.push((entry.path(), meta.len()));
        }
    }
    files.sort();
    Ok(files)
}

/// The names of v1's files in `folder`, for the presence check.
pub fn listed_names(folder: &Path) -> Result<BTreeSet<String>, String> {
    Ok(listed_files(folder)?
        .into_iter()
        .filter_map(|(path, _)| Some(path.file_name()?.to_string_lossy().into_owned()))
        .collect())
}

/// The total size of v1's own files in `folder` (0 when it cannot be read).
pub fn owned_size(folder: &Path) -> u64 {
    owned_files(folder).iter().map(|(_, size)| size).sum()
}

/// The images folder row of the Settings "Card images" card.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct FolderStatus {
    pub path: String,
    pub is_default: bool,
    /// A chosen folder that is gone (drive unplugged): downloads pause and
    /// pages show the remote images.
    pub missing: bool,
    /// A move is running (the card polls until it ends).
    pub moving: bool,
    /// The last move's failure ("Moving card images failed: {message}").
    pub error: Option<String>,
}

/// What the Settings "Card images" card shows (0.x Storage page props).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct CardImagesStatus {
    /// "Current usage: {usage}" (0.x `MeasureLocalImagesSize`, v1's own
    /// files only).
    pub usage: String,
    pub folder: FolderStatus,
}

/// The card's status for the folder and move as they are now.
pub fn images_status(
    folder: &ImagesFolder,
    (moving, error): (bool, Option<String>),
) -> CardImagesStatus {
    CardImagesStatus {
        usage: size_text(owned_size(&folder.path)),
        folder: FolderStatus {
            path: folder.path.to_string_lossy().into_owned(),
            is_default: folder.is_default,
            missing: !folder.is_default && !folder.path.is_dir(),
            moving,
            error,
        },
    }
}

/// The Settings "Card images" card.
#[tauri::command]
#[specta::specta]
pub async fn card_images_status(app: AppHandle) -> Result<CardImagesStatus, CommandError> {
    off_async_workers(move || {
        let startup = app.state::<Startup>();
        ready(&startup)?;
        let images = app
            .try_state::<ImagesState>()
            .ok_or_else(|| CommandError::Refused("the images folder is not set".to_string()))?;
        Ok(images_status(&images.folder(), images.move_state()))
    })
    .await
}

#[cfg(test)]
mod tests;
