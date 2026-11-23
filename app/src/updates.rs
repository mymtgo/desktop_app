//! Updates from the static stable and edge feeds (spec 9, release spec 6.1).
//! Release builds check ten seconds after start and every hour, download
//! a newer version in the background, and install it when the user says so.
//! `UpdateStatus` holds the rules (ported from 0.x `ResolveUpdateStatus`),
//! `channel` and `offers` pick the feed and the version; they are tested
//! here, and the rest drives the updater plugin.

use std::sync::Mutex;
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Manager};
use tauri_plugin_updater::{Update, UpdaterExt};
use tauri_specta::Event;

use crate::bridge::lock;
use crate::events::UpdateStatusChanged;

pub const FIRST_CHECK_AFTER: Duration = Duration::from_secs(10);
pub const CHECK_EVERY: Duration = Duration::from_secs(60 * 60);
/// A check or download that takes longer gives up, so a stalled connection
/// cannot leave the updater stuck in "downloading" until the app restarts.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// Where the release feeds live (release spec 6.1).
pub const FEED_BASE: &str = "https://releases.mymtgo.com";

/// The feed an install updates from (#221). Not `Channel`: tauri-specta reserves that name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum UpdateChannel {
    Stable,
    /// New versions early: every pre-release, and each stable release that
    /// is newer than the last one. Was "beta" up to 1.0.0-beta.8; the
    /// `beta_updates` setting keeps its name.
    Edge,
}

/// The channel for the `beta_updates` setting and the running version: an
/// explicit choice wins; unset, a prerelease build stays on edge and a stable
/// build is on stable (release spec ruling 8).
pub fn channel(beta_updates: Option<bool>, current_version: &str) -> UpdateChannel {
    let edge = beta_updates.unwrap_or_else(|| crate::identity::is_prerelease(current_version));
    if edge {
        UpdateChannel::Edge
    } else {
        UpdateChannel::Stable
    }
}

/// The channel's name as the app shows it ("edge", "stable"), and its
/// folder on the release domain.
pub fn channel_name(channel: UpdateChannel) -> String {
    match channel {
        UpdateChannel::Stable => "stable".to_string(),
        UpdateChannel::Edge => "edge".to_string(),
    }
}

/// The channel's `latest.json`.
pub fn feed_url(channel: UpdateChannel) -> String {
    format!("{FEED_BASE}/{}/latest.json", channel_name(channel))
}

/// Whether the updater offers `candidate` to an install running `current`:
/// only a newer version (semver precedence), so turning beta off never
/// downgrades; a beta install waits until a stable release passes it. A
/// version that does not parse is never offered.
pub fn offers(current: &str, candidate: &str) -> bool {
    match (Version::parse(current), Version::parse(candidate)) {
        (Some(current), Some(candidate)) => candidate.is_newer_than(&current),
        _ => false,
    }
}

/// A semantic version without its build metadata, enough to order versions
/// as semver does.
#[derive(Debug, PartialEq, Eq)]
struct Version {
    core: [u64; 3],
    /// The dot separated prerelease parts; empty for a release.
    prerelease: Vec<String>,
}

impl Version {
    fn parse(text: &str) -> Option<Version> {
        let text = text.trim();
        let text = text.strip_prefix('v').unwrap_or(text);
        let without_build = text.split('+').next()?;
        let (core_text, prerelease_text) = match without_build.split_once('-') {
            Some((core, pre)) => (core, Some(pre)),
            None => (without_build, None),
        };
        let numbers: Vec<&str> = core_text.split('.').collect();
        if numbers.len() != 3 {
            return None;
        }
        let mut core = [0u64; 3];
        for (slot, number) in core.iter_mut().zip(numbers) {
            if number.is_empty() || !number.bytes().all(|byte| byte.is_ascii_digit()) {
                return None;
            }
            *slot = number.parse().ok()?;
        }
        let mut prerelease = Vec::new();
        if let Some(pre) = prerelease_text {
            for part in pre.split('.') {
                if part.is_empty() {
                    return None;
                }
                prerelease.push(part.to_string());
            }
        }
        Some(Version { core, prerelease })
    }

    fn is_newer_than(&self, other: &Version) -> bool {
        if self.core != other.core {
            return self.core > other.core;
        }
        match (self.prerelease.is_empty(), other.prerelease.is_empty()) {
            (true, true) => false,
            // A release is newer than its prereleases.
            (true, false) => true,
            (false, true) => false,
            (false, false) => prerelease_is_newer(&self.prerelease, &other.prerelease),
        }
    }
}

/// Semver's prerelease order: part by part, numbers by value and below
/// words, words in ASCII order; with equal leading parts, more parts are
/// newer.
fn prerelease_is_newer(left: &[String], right: &[String]) -> bool {
    for (a, b) in left.iter().zip(right) {
        let a_number = a
            .parse::<u64>()
            .ok()
            .filter(|_| a.bytes().all(|c| c.is_ascii_digit()));
        let b_number = b
            .parse::<u64>()
            .ok()
            .filter(|_| b.bytes().all(|c| c.is_ascii_digit()));
        let order = match (a_number, b_number) {
            (Some(x), Some(y)) => x.cmp(&y),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => a.as_str().cmp(b.as_str()),
        };
        if order != std::cmp::Ordering::Equal {
            return order == std::cmp::Ordering::Greater;
        }
    }
    left.len() > right.len()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum UpdatePhase {
    UpToDate,
    Checking,
    Downloading,
    Ready,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct UpdateStatus {
    /// The running version.
    pub current: String,
    /// The version being downloaded or ready to install.
    pub available: Option<String>,
    pub phase: UpdatePhase,
    /// When the last check started (wall clock: app state, not match data).
    #[specta(type = Option<specta_typescript::Number>)]
    pub checked_at_ms: Option<i64>,
    pub error: Option<String>,
    /// False in development builds: they never check.
    pub active: bool,
    /// The feed checks read (release spec 6.1).
    pub channel: UpdateChannel,
}

impl UpdateStatus {
    pub fn new(current: &str, active: bool) -> Self {
        Self {
            current: current.to_string(),
            available: None,
            phase: UpdatePhase::UpToDate,
            checked_at_ms: None,
            error: None,
            active,
            channel: channel(None, current),
        }
    }

    /// Start a check. Refused in development builds, while one runs, and once
    /// an update is ready: a downloaded update stays installable whatever later
    /// checks say.
    pub fn begin_check(&mut self, now_ms: i64) -> bool {
        let busy = matches!(
            self.phase,
            UpdatePhase::Checking | UpdatePhase::Downloading | UpdatePhase::Ready
        );
        if !self.active || busy {
            return false;
        }
        self.phase = UpdatePhase::Checking;
        self.checked_at_ms = Some(now_ms);
        self.error = None;
        true
    }

    pub fn found(&mut self, version: &str) {
        self.phase = UpdatePhase::Downloading;
        self.available = Some(version.to_string());
    }

    pub fn up_to_date(&mut self) {
        self.phase = UpdatePhase::UpToDate;
        self.available = None;
    }

    pub fn downloaded(&mut self) {
        self.phase = UpdatePhase::Ready;
    }

    pub fn failed(&mut self, error: &str) {
        self.phase = UpdatePhase::Error;
        self.available = None;
        self.error = Some(error.to_string());
    }
}

/// Managed state: the status, and the downloaded update until it is installed.
pub struct Updates {
    status: Mutex<UpdateStatus>,
    pending: Mutex<Option<(Update, Vec<u8>)>>,
}

impl Updates {
    pub fn new(current: &str, active: bool) -> Self {
        Self {
            status: Mutex::new(UpdateStatus::new(current, active)),
            pending: Mutex::new(None),
        }
    }

    pub fn snapshot(&self) -> UpdateStatus {
        lock(&self.status).clone()
    }

    /// Follow the `beta_updates` setting. Returns the channel now in use.
    pub fn set_channel(&self, beta_updates: Option<bool>) -> UpdateChannel {
        let mut status = lock(&self.status);
        status.channel = channel(beta_updates, &status.current);
        status.channel
    }
}

/// The stored setting, or `None` when it cannot be read at all (as opposed to
/// `Some(None)`: read, and unset).
fn read_beta_updates(app: &AppHandle) -> Option<Option<bool>> {
    let startup = app.try_state::<crate::commands::Startup>()?;
    let crate::commands::Startup::Ready(state) = startup.inner() else {
        return None;
    };
    crate::commands::read_settings(state)
        .ok()
        .map(|settings| settings.beta_updates)
}

/// The channel after a settings read: a failed read keeps the channel in use
/// instead of falling back to the build's default.
pub fn channel_after_read(
    read: Option<Option<bool>>,
    in_use: UpdateChannel,
    current_version: &str,
) -> UpdateChannel {
    match read {
        Some(beta_updates) => channel(beta_updates, current_version),
        None => in_use,
    }
}

/// Take the channel from the stored settings (at start, once the startup
/// state is managed).
pub fn load_channel(app: &AppHandle) {
    // Read first, then decide and write under one lock, so a channel set in
    // between is not overwritten.
    let read = read_beta_updates(app);
    let updates = app.state::<Updates>();
    let mut status = lock(&updates.status);
    status.channel = channel_after_read(read, status.channel, &status.current);
}

/// The channel the next check reads.
pub fn current_channel(app: &AppHandle) -> UpdateChannel {
    app.state::<Updates>().snapshot().channel
}

/// The beta setting changed: follow it and check its feed now (release spec
/// 6.1), off the calling thread.
pub fn channel_changed(app: &AppHandle, beta_updates: Option<bool>) {
    app.state::<Updates>().set_channel(beta_updates);
    publish(app);
    let app = app.clone();
    tauri::async_runtime::spawn(async move { check_now(&app).await });
}

/// Check now and then every `CHECK_EVERY`, on a thread of its own.
pub fn start_checking(app: AppHandle) {
    if !app.state::<Updates>().snapshot().active {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("updates".to_string())
        .spawn(move || {
            std::thread::sleep(FIRST_CHECK_AFTER);
            loop {
                tauri::async_runtime::block_on(check_now(&app));
                std::thread::sleep(CHECK_EVERY);
            }
        });
    if let Err(error) = spawned {
        tracing::warn!(%error, "could not start update checks");
    }
}

/// Check the feed and download a newer version. Does nothing while a check
/// runs or an update is ready.
pub async fn check_now(app: &AppHandle) {
    loop {
        load_channel(app);
        let used = current_channel(app);
        if !check_once(app).await {
            return;
        }
        // The setting changed while the check ran: that check read the old
        // feed, so read the new one now.
        load_channel(app);
        if current_channel(app) == used {
            return;
        }
    }
}

/// One check. False when none started (one runs, or an update is ready).
async fn check_once(app: &AppHandle) -> bool {
    let started = lock(&app.state::<Updates>().status).begin_check(crate::wall_clock_ms());
    if !started {
        return false;
    }
    publish(app);
    let result = fetch(app).await;
    {
        let updates = app.state::<Updates>();
        let mut status = lock(&updates.status);
        match result {
            Ok(None) => status.up_to_date(),
            Ok(Some((update, bytes))) => {
                tracing::info!(version = %update.version, "update downloaded");
                *lock(&updates.pending) = Some((update, bytes));
                status.downloaded();
            }
            Err(error) => {
                tracing::warn!(%error, "update check failed");
                status.failed(&error);
            }
        }
    }
    publish(app);
    true
}

/// A newer release that ships no build for this platform: a macOS release
/// build skipped while the Mac runner was away. There is nothing to install,
/// which is up to date, not a failed check.
fn no_build_for_this_platform(error: &tauri_plugin_updater::Error) -> bool {
    matches!(
        error,
        tauri_plugin_updater::Error::TargetNotFound(_)
            | tauri_plugin_updater::Error::TargetsNotFound(_)
    )
}

async fn fetch(app: &AppHandle) -> Result<Option<(Update, Vec<u8>)>, String> {
    // Built per check, so the feed follows the channel chosen since the last
    // one; `tauri.conf.json`'s endpoint is only the plugin's default.
    let feed = feed_url(current_channel(app));
    let endpoint = feed
        .parse()
        .map_err(|error| format!("bad feed url {feed}: {error}"))?;
    let updater = app
        .updater_builder()
        .endpoints(vec![endpoint])
        .map_err(|error| error.to_string())?
        .version_comparator(|current, release| {
            offers(&current.to_string(), &release.version.to_string())
        })
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|error| error.to_string())?;
    let update = match updater.check().await {
        Ok(Some(update)) => update,
        Ok(None) => return Ok(None),
        Err(error) if no_build_for_this_platform(&error) => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    lock(&app.state::<Updates>().status).found(&update.version);
    publish(app);
    let bytes = update
        .download(|_, _| {}, || {})
        .await
        .map_err(|error| error.to_string())?;
    Ok(Some((update, bytes)))
}

/// Tell every window, the tray and the taskbar what the status is now.
fn publish(app: &AppHandle) {
    let status = app.state::<Updates>().snapshot();
    crate::window::show_update_ready(app, &status);
    if let Err(error) = (UpdateStatusChanged { status }).emit(app) {
        tracing::warn!(%error, "could not push the update status");
    }
}

/// Install the downloaded update and start the new version. Tracking stops
/// first: on Windows the installer closes this process, and the tracker's
/// last batch must be committed by then. The local log closes only after the
/// result is logged. If the install fails, the current version restarts, so
/// tracking resumes.
///
/// On Windows a successful install ends the process inside `update.install`,
/// so `prepare_exit` does not run and the local log guard is never dropped.
/// The "installing the update" line relies on the non-blocking log worker
/// having written it by then (`reporting::flush` gives it time).
pub fn install(app: &AppHandle) -> Result<(), String> {
    let pending = lock(&app.state::<Updates>().pending).take();
    let Some((update, bytes)) = pending else {
        return Err("no update is ready to install".to_string());
    };
    tracing::info!(version = %update.version, "installing the update");
    // Tracking stops first: on Windows the installer closes this process. The
    // log stays open until the result is known, so a failure reaches it.
    crate::stop_tracking(app);
    // The recorder stops gracefully too, before the installer closes us.
    if let Some(worker) = app.try_state::<crate::recorder::supervisor::RecorderWorker>() {
        worker.stop();
    }
    crate::reporting::flush();
    if let Err(error) = update.install(bytes) {
        tracing::error!(%error, "the update could not be installed; restarting this version");
    }
    crate::prepare_exit(app);
    app.restart()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failed_settings_read_keeps_the_channel_in_use() {
        // a stable build on beta by choice stays on beta
        let kept = channel_after_read(None, UpdateChannel::Edge, "1.0.0");
        assert_eq!(kept, UpdateChannel::Edge);
        // a read that succeeds follows the setting, and unset follows the build
        let set = channel_after_read(Some(Some(false)), UpdateChannel::Edge, "1.0.0-beta.1");
        assert_eq!(set, UpdateChannel::Stable);
        let unset = channel_after_read(Some(None), UpdateChannel::Stable, "1.0.0-beta.1");
        assert_eq!(unset, UpdateChannel::Edge);
    }

    fn active() -> UpdateStatus {
        UpdateStatus::new("0.1.0", true)
    }

    #[test]
    fn development_builds_never_check() {
        let mut status = UpdateStatus::new("0.1.0", false);
        assert!(!status.begin_check(1));
        assert_eq!(status.phase, UpdatePhase::UpToDate);
    }

    #[test]
    fn a_check_runs_once_at_a_time() {
        let mut status = active();
        assert!(status.begin_check(1));
        assert_eq!(
            (status.phase, status.checked_at_ms),
            (UpdatePhase::Checking, Some(1))
        );
        assert!(!status.begin_check(2), "one check at a time");
        status.found("0.1.1");
        assert!(!status.begin_check(3), "not while downloading");
    }

    #[test]
    fn a_release_without_this_platform_is_nothing_to_install() {
        use tauri_plugin_updater::Error;
        assert!(no_build_for_this_platform(&Error::TargetNotFound(
            "darwin-aarch64".to_string()
        )));
        assert!(no_build_for_this_platform(&Error::TargetsNotFound(vec![
            "darwin-aarch64-app".to_string(),
            "darwin-aarch64".to_string(),
        ])));
        assert!(!no_build_for_this_platform(&Error::ReleaseNotFound));
    }

    #[test]
    fn finding_nothing_is_up_to_date() {
        let mut status = active();
        status.begin_check(1);
        status.up_to_date();
        assert_eq!(
            (status.phase, status.available.clone()),
            (UpdatePhase::UpToDate, None)
        );
        assert!(status.begin_check(2), "the next check may run");
    }

    #[test]
    fn a_ready_update_stays_ready_whatever_later_checks_say() {
        let mut status = active();
        status.begin_check(1);
        status.found("0.1.1");
        status.downloaded();
        assert_eq!(
            (status.phase, status.available.as_deref()),
            (UpdatePhase::Ready, Some("0.1.1"))
        );
        assert!(
            !status.begin_check(2),
            "no check replaces a downloaded update"
        );
        assert_eq!(status.phase, UpdatePhase::Ready);
    }

    #[test]
    fn a_failed_check_reports_the_error_and_can_be_retried() {
        let mut status = active();
        status.begin_check(1);
        status.found("0.1.1");
        status.failed("signature did not match");
        assert_eq!(status.phase, UpdatePhase::Error);
        assert_eq!(status.available, None);
        assert_eq!(status.error.as_deref(), Some("signature did not match"));
        assert!(status.begin_check(2));
        assert_eq!(status.error, None, "a new check clears the old error");
    }

    #[test]
    fn the_channel_follows_the_build_until_chosen() {
        assert_eq!(channel(None, "1.0.0-beta.3"), UpdateChannel::Edge);
        assert_eq!(channel(None, "1.0.0"), UpdateChannel::Stable);
        assert_eq!(channel(None, "1.0.1+build-7"), UpdateChannel::Stable);
        assert_eq!(channel(Some(false), "1.0.0-beta.3"), UpdateChannel::Stable);
        assert_eq!(channel(Some(true), "1.0.0"), UpdateChannel::Edge);
        assert_eq!(channel(Some(true), "1.0.0-beta.3"), UpdateChannel::Edge);
        assert_eq!(channel(Some(false), "1.0.0"), UpdateChannel::Stable);
    }

    #[test]
    fn turning_edge_off_never_offers_an_older_version() {
        // A beta install that turned beta off takes the stable release that
        // passes it.
        assert!(offers("1.0.0-beta.3", "1.0.0"));
        // A newer beta waits: the stable feed's 1.0.0 is older.
        assert!(!offers("1.1.0-beta.1", "1.0.0"));
        assert!(!offers("1.0.0", "1.0.0"));
        assert!(!offers("1.0.1", "1.0.0"));
        assert!(offers("1.0.0", "1.0.1"));
        assert!(offers("1.0.0", "1.1.0-beta.1"));
        assert!(
            offers("1.0.0-beta.2", "1.0.0-beta.10"),
            "numeric parts by value"
        );
        assert!(offers("1.0.0-beta.2", "1.0.0-rc.1"));
        assert!(offers("1.0.0-beta", "1.0.0-beta.1"), "more parts are newer");
        assert!(!offers("1.0.0-beta.1", "1.0.0-alpha.9"));
        assert!(!offers("1.0.0+a", "1.0.0+b"), "build metadata is ignored");
        assert!(!offers("1.0.0", "not a version"));
    }

    #[test]
    fn both_feeds_are_on_the_release_domain() {
        assert_eq!(
            feed_url(UpdateChannel::Stable),
            "https://releases.mymtgo.com/stable/latest.json"
        );
        assert_eq!(
            feed_url(UpdateChannel::Edge),
            "https://releases.mymtgo.com/edge/latest.json"
        );
    }

    #[test]
    fn the_status_names_its_channel() {
        let mut status = UpdateStatus::new("1.0.0-beta.3", true);
        assert_eq!(status.channel, UpdateChannel::Edge);
        status.channel = channel(Some(false), &status.current);
        assert_eq!(status.channel, UpdateChannel::Stable);
        assert_eq!(channel_name(UpdateChannel::Stable), "stable");
        assert_eq!(channel_name(UpdateChannel::Edge), "edge");
    }

    #[test]
    fn the_default_feed_is_the_signed_stable_channel() {
        let config: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).expect("tauri.conf.json");
        let updater = &config["plugins"]["updater"];
        let endpoint = updater["endpoints"][0].as_str().expect("an endpoint");
        // The default only: every check names its channel's feed. The release
        // workflow requires the stable feed here (release spec Appendix A).
        assert_eq!(endpoint, feed_url(UpdateChannel::Stable));
        assert!(updater["pubkey"].as_str().is_some_and(|key| key.len() > 40));
        assert_eq!(updater["windows"]["installMode"], "passive");
        // Only the release workflow signs, so local builds need no key.
        assert_ne!(config["bundle"]["createUpdaterArtifacts"], true);
    }
}
