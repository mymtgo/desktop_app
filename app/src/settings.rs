//! Typed settings (spec 4.5). Each field is one row of the `settings` table, its
//! value JSON. Unknown keys are ignored and a missing or unreadable value takes
//! its default, so adding a setting never needs a migration.

use std::path::PathBuf;

use overlay::{
    DEFAULT_FINISHED_MINUTES, LeagueDisplay, LeagueTheme, MAX_FINISHED_MINUTES,
    MIN_FINISHED_MINUTES, OverlayArtwork, OverlaySize, PublishError,
};
use serde::{Deserialize, Serialize};

pub const LOG_FOLDER: &str = "log_folder";
pub const CLOSE_TO_TRAY: &str = "close_to_tray";
/// "Share anonymous usage data" (usage analytics spec, default on).
pub const SHARE_USAGE: &str = "share_usage";
pub const MTGO_DATA_FOLDER: &str = "mtgo_data_folder";
/// The Storage page's Watcher (release spec 4.1; 0.x `watcher_active`).
pub const WATCHER_ACTIVE: &str = "watcher_active";
/// The header's last account switch (release spec 5 and 14).
pub const ACTIVE_ACCOUNT: &str = store::accounts::ACTIVE_ACCOUNT;
/// The General page's "Enable beta updates" (release spec 6.1 and 14).
pub const BETA_UPDATES: &str = "beta_updates";
/// The Replays page's "Record MTGO replays" switch (recorded replays spec 4)
/// and the acceptance of its risk notice (spec 5).
pub const RECORDER_ENABLED: &str = "recorder_enabled";
pub const RECORDER_ACCEPTANCE: &str = "recorder_acceptance";
/// The Integrations switches (endstep spec 8.1): Arena (absent until chosen)
/// and Endstep (default off).
pub const ARENA_ENABLED: &str = store::platform::ARENA_ENABLED;
pub const ENDSTEP_ENABLED: &str = store::platform::ENDSTEP_ENABLED;
pub const HIDE_ARCHIVED_DECKS: &str = "hide_archived_decks";
pub const DECKS_PER_PAGE: &str = "decks_per_page";
pub const DECK_CARD_SIZE: &str = "deck_card_size";
/// The General page's "Interface scale" (a percent from `INTERFACE_SCALE_OPTIONS`).
pub const INTERFACE_SCALE: &str = "interface_scale";
/// Card images (cards spec sections 4 and 13).
pub const CARD_IMAGES_PATH: &str = "card_images_path";
pub const CARD_STATS_TRUST: &str = "card_stats_trust";
/// The overlay windows and the hosted OBS overlay (overlay spec section 4).
pub const GAME_OVERLAY: &str = "game_overlay";
pub const DRAFT_NOTES_WINDOW: &str = "draft_notes_window";
pub const LEAGUE_WINDOW: &str = "league_window";
pub const OVERLAY_SHOW_OPPONENT: &str = "overlay_show_opponent";
pub const OVERLAY_SHOW_DRAW_ODDS: &str = "overlay_show_draw_odds";
pub const OVERLAY_SHOW_REVEALS: &str = "overlay_show_reveals";
pub const OVERLAY_SHOW_SIDEBOARD: &str = "overlay_show_sideboard";
pub const OVERLAY_ARTWORK: &str = "overlay_artwork";
pub const OVERLAY_SIZE: &str = "overlay_size";
pub const OVERLAY_PUBLISH: &str = "overlay_publish";
/// What the league card shows (league card options spec section 3): one
/// JSON object. Not an `OVERLAY_SETTINGS` key, so a 0.x import never writes it.
pub const OVERLAY_LEAGUE_DISPLAY: &str = "overlay_league_display";
/// How many minutes a complete or dropped run stays on the league card. v1
/// only (0.x always kept it 5 minutes), so a 0.x import never writes it.
pub const OVERLAY_FINISHED_MINUTES: &str = "overlay_finished_minutes";
pub const OVERLAY_LEAGUE_THEME: &str = "overlay_league_theme";
/// The other event strip under the event card (event overlay spec 8). v1
/// only, so a 0.x import never writes it.
pub const OVERLAY_EVENT_STRIP: &str = "overlay_event_strip";
/// The two 0.x windows the game overlay replaced: with no `game_overlay`
/// of its own, either one on turns it on (0.x `showGameOverlay`).
const LEGACY_DECK_WINDOW: &str = "deck_window";
const LEGACY_OPPONENT_WINDOW: &str = "opponent_window";
/// Overlay rows outside `Settings`, written by the overlay commands and
/// threads. UI state: whether the game overlay is
/// collapsed, and its height before it was. Internal: the custom background
/// file name in `<app data>/overlay/`, its uploaded copy, when the hosted
/// overlay was last published (ms) and why the last push failed.
pub const OVERLAY_COLLAPSED: &str = "overlay_collapsed";
pub const OVERLAY_EXPANDED_HEIGHT: &str = "overlay_expanded_height";
pub const OVERLAY_BACKGROUND_FILE: &str = "overlay_background_file";
pub const OVERLAY_BACKGROUND_REMOTE: &str = "overlay_background_remote";
pub const OVERLAY_LAST_PUBLISHED_AT: &str = "overlay_last_published_at";
pub const OVERLAY_PUBLISH_ERROR: &str = "overlay_publish_error";
/// The MyMTGO API's device id (a UUID v4, minted at first API use) and when
/// its key is assumed to expire (ms). Not `Settings` fields: the `api`
/// module reads and saves them (07 spec section 4).
pub const API_DEVICE_ID: &str = "api_device_id";
pub const API_KEY_EXPIRES_AT: &str = "api_key_expires_at";
/// When the access token expires (ms, `null` once signed out) and the
/// confirmed MTGO logins (JSON map of login id to username). Earlier v1
/// builds' `api_activated_at` row is no longer read or written (owner ruling
/// 2026-10-11, 07 spec ruling 2).
pub const API_TOKEN_EXPIRES_AT: &str = "api_token_expires_at";
pub const API_ATTESTED: &str = "api_attested";
/// Whether the 0.x launch at login question is settled (release spec
/// section 13 item 4, ruling 16): the import applied 0.x's value once, or
/// the user changed v1's switch. Default false. Not a `Settings` field: the
/// import and the switch write it.
pub const AUTOSTART_FROM_0X_APPLIED: &str = "autostart_from_0x_applied";
/// The version whose What's new was last shown or seeded (release spec
/// sections 7 and 14). Default unset. Not a `Settings` field: only the start
/// command writes it.
pub const WHATS_NEW_SEEN_VERSION: &str = "whats_new_seen_version";
/// The first run's 0.x answer (release spec section 13.2): `"migrate"` or
/// `"fresh"`, unset until chosen. Once set, the question is never asked
/// again. Not a `Settings` field: the first run writes it.
pub const FIRST_RUN_CHOICE: &str = "first_run_choice";
/// `true` from the start that created the database until its first-run
/// decision is saved (final review F4): a start that ended before deciding
/// leaves the next one a fresh install. Not a `Settings` field.
pub const FRESH_INSTALL: &str = "fresh_install";

/// The deck list page sizes (0.x `DECKS_PER_PAGE_OPTIONS`).
pub const DECKS_PER_PAGE_OPTIONS: [u32; 3] = [12, 24, 48];
const DEFAULT_DECKS_PER_PAGE: u32 = 12;

/// The interface scale choices, in percent. Anything else reads as 100.
pub const INTERFACE_SCALE_OPTIONS: [u32; 6] = [80, 90, 100, 110, 125, 150];
pub const DEFAULT_INTERFACE_SCALE: u32 = 100;

/// The webview zoom factor for a scale percent (`110` is `1.1`).
pub fn zoom_factor(percent: u32) -> f64 {
    f64::from(percent) / 100.0
}

/// The card stats trust threshold (0.x `cardStatsTrust`): default 50,
/// clamped to 0 to 100,000.
pub const DEFAULT_CARD_STATS_TRUST: u32 = 50;
pub const MAX_CARD_STATS_TRUST: u32 = 100_000;

/// How the deck list draws its cards (0.x `DECK_CARD_SIZES`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "lowercase")]
pub enum DeckCardSize {
    #[default]
    Large,
    Compact,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    /// A folder (or an `mtgo.log` file) searched instead of the detected
    /// locations (spec 5.1, manual override). `None` means auto-detect.
    pub log_folder: Option<String>,
    /// Closing the window hides it to the tray instead of quitting (spec 6).
    pub close_to_tray: bool,
    /// The folder searched for MTGO's account folders and deck files instead
    /// of the detected `AppData/Local/Apps/2.0/Data` folders (decks spec 3).
    /// `None` means auto-detect.
    pub mtgo_data_folder: Option<String>,
    /// The deck list hides archived decks (0.x default: on).
    pub hide_archived_decks: bool,
    /// Decks per deck list page: 12, 24 or 48 (anything else reads as 12).
    pub decks_per_page: u32,
    pub deck_card_size: DeckCardSize,
    /// How large the main window draws the interface, in percent: one of
    /// `INTERFACE_SCALE_OPTIONS` (anything else reads as 100). Only the main
    /// window is scaled.
    pub interface_scale: u32,
    /// A chosen folder for card images (v1 keeps its files in its own
    /// `mymtgo-desktop-card-images` subfolder). `None` is the default folder
    /// in v1's app data. Never seeded from 0.x (spec ruling 3).
    pub card_images_path: Option<String>,
    /// Card stats trust threshold, 0 to 100,000 (0.x `card_stats_trust`).
    /// Only the trust slider's command changes it (`set_card_stats_trust`);
    /// a settings save keeps the stored value.
    pub card_stats_trust: u32,
    /// The game overlay window (0.x default off).
    pub game_overlay: bool,
    /// The live draft notes window (0.x default on).
    pub draft_notes_window: bool,
    /// The league progress window (0.x default off).
    pub league_window: bool,
    /// The game overlay's sections (0.x default on).
    pub overlay_show_opponent: bool,
    pub overlay_show_draw_odds: bool,
    pub overlay_show_reveals: bool,
    pub overlay_show_sideboard: bool,
    /// The league card's artwork; `None` is the read-time default (spec
    /// section 9.4).
    pub overlay_artwork: Option<OverlayArtwork>,
    pub overlay_size: OverlaySize,
    /// Publish the league card to the hosted OBS overlay (0.x default off).
    pub overlay_publish: bool,
    /// What the league card shows, on the window and the hosted overlay.
    pub overlay_league_display: LeagueDisplay,
    /// Minutes a complete or dropped run stays on the league card, on the
    /// window and the hosted overlay: 1 to 60, default 5 (0.x's fixed
    /// grace).
    pub overlay_finished_minutes: u32,
    /// The league card's look (league overlay themes spec 4.1).
    pub overlay_league_theme: LeagueTheme,
    /// The other event strip under the event card, on the window and the
    /// hosted overlay (event overlay spec 8, default on). Off, the card
    /// carries no other event.
    pub overlay_event_strip: bool,
    /// The watcher is on (release spec 4.1, default on): off, the tracker
    /// reads no log file and no deck XML. Only the Watcher card's command
    /// changes it (`set_watcher_active`); a settings save keeps the stored
    /// value, and saving a path never switches it off (ruling 6).
    pub watcher_active: bool,
    /// The header's last account switch (release spec 5, default unset).
    /// Only `switch_account` changes it (an `AccountEdit` through the
    /// tracker); a settings save keeps the stored value.
    pub active_account: Option<ActiveAccount>,
    /// "Enable beta updates" (release spec 6.1, default unset): `None`
    /// follows the build (beta for a prerelease build, stable otherwise),
    /// an explicit choice is kept (`updates::channel`). Only the Updates
    /// card's command changes it (`set_beta_updates`); a settings save keeps
    /// the stored value.
    pub beta_updates: Option<bool>,
    /// "Record MTGO replays" (recorded replays spec 4, default off). Only the
    /// Replays page's commands change it; a settings save keeps the stored
    /// value, so it never turns itself on.
    pub recorder_enabled: bool,
    /// The risk notice the user accepted (spec 5). Only
    /// `accept_recorder_notice` writes it; a settings save keeps it.
    pub recorder_acceptance: Option<RecorderAcceptance>,
    /// The Arena switch (endstep spec 8.1). `None` is not chosen: Arena is
    /// then on for anyone with Arena data and off for a fresh install
    /// (`integrations::arena_effective`). Only `set_integration` changes it;
    /// a settings save keeps the stored value.
    pub arena_enabled: Option<bool>,
    /// The Endstep switch (default off). Only `set_integration` changes it;
    /// a settings save keeps the stored value.
    pub endstep_enabled: bool,
    /// "Share anonymous usage data" (usage analytics spec, default on):
    /// off, `usage::track` sends nothing. Offline mode stops it too.
    pub share_usage: bool,
}

/// The risk notice as accepted: the notice wording version, the recorder's
/// consent version at that time, and when (UTC unix ms).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct RecorderAcceptance {
    pub notice_version: u32,
    pub consent_version: u32,
    #[specta(type = specta_typescript::Number)]
    pub accepted_at_ms: i64,
}

/// An account switch as stored: which account and when (UTC unix ms). The
/// same JSON as `store::accounts::ActiveAccount`, which the current account
/// rule reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct ActiveAccount {
    #[specta(type = specta_typescript::Number)]
    pub account_id: i64,
    #[specta(type = specta_typescript::Number)]
    pub switched_at_ms: i64,
}

/// The custom background as uploaded to the API: its public URL and the
/// SHA-256 (hex) of the local file it was made from (ruling C11; 0.x kept a
/// sha1 as `hash`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackgroundRemote {
    pub url: String,
    pub sha256: String,
}

/// The folders a running tracker was started with: a save that changes
/// either restarts tracking.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TrackedFolders {
    pub log_folder: Option<String>,
    pub mtgo_data_folder: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            log_folder: None,
            close_to_tray: true,
            mtgo_data_folder: None,
            hide_archived_decks: true,
            decks_per_page: DEFAULT_DECKS_PER_PAGE,
            deck_card_size: DeckCardSize::Large,
            interface_scale: DEFAULT_INTERFACE_SCALE,
            card_images_path: None,
            card_stats_trust: DEFAULT_CARD_STATS_TRUST,
            game_overlay: false,
            draft_notes_window: true,
            league_window: false,
            overlay_show_opponent: true,
            overlay_show_draw_odds: true,
            overlay_show_reveals: true,
            overlay_show_sideboard: true,
            overlay_artwork: None,
            overlay_size: OverlaySize::Full,
            overlay_publish: false,
            overlay_league_display: LeagueDisplay::default(),
            overlay_finished_minutes: DEFAULT_FINISHED_MINUTES,
            overlay_league_theme: LeagueTheme::Classic,
            overlay_event_strip: true,
            watcher_active: true,
            active_account: None,
            beta_updates: None,
            recorder_enabled: false,
            recorder_acceptance: None,
            arena_enabled: None,
            endstep_enabled: false,
            share_usage: true,
        }
    }
}

impl Settings {
    /// Read stored rows. A row that does not parse keeps that field's default.
    /// A deck setting with no row of its own takes the 0.x value kept by the
    /// import (`legacy_0x_settings`), so the first read is seeded from 0.x and
    /// the next save stores it.
    pub fn from_rows(rows: &[(String, String)]) -> Self {
        let mut settings = Settings::default();
        if let Some((_, legacy)) = rows
            .iter()
            .find(|(key, _)| key == crate::legacy::LEGACY_SETTINGS)
        {
            settings.seed_from_legacy(legacy);
        }
        for (key, value) in rows {
            match key.as_str() {
                LOG_FOLDER => {
                    if let Ok(folder) = serde_json::from_str::<Option<String>>(value) {
                        settings.log_folder = folder;
                    }
                }
                CLOSE_TO_TRAY => {
                    if let Ok(flag) = serde_json::from_str::<bool>(value) {
                        settings.close_to_tray = flag;
                    }
                }
                SHARE_USAGE => read_bool(value, &mut settings.share_usage),
                MTGO_DATA_FOLDER => {
                    if let Ok(folder) = serde_json::from_str::<Option<String>>(value) {
                        settings.mtgo_data_folder = folder;
                    }
                }
                HIDE_ARCHIVED_DECKS => {
                    if let Ok(flag) = serde_json::from_str::<bool>(value) {
                        settings.hide_archived_decks = flag;
                    }
                }
                DECKS_PER_PAGE => {
                    if let Ok(count) = serde_json::from_str::<u32>(value) {
                        settings.decks_per_page = count;
                    }
                }
                DECK_CARD_SIZE => {
                    if let Ok(size) = serde_json::from_str::<DeckCardSize>(value) {
                        settings.deck_card_size = size;
                    }
                }
                INTERFACE_SCALE => {
                    if let Ok(percent) = serde_json::from_str::<u32>(value) {
                        settings.interface_scale = percent;
                    }
                }
                CARD_IMAGES_PATH => {
                    if let Ok(folder) = serde_json::from_str::<Option<String>>(value) {
                        settings.card_images_path = folder;
                    }
                }
                CARD_STATS_TRUST => {
                    if let Ok(json) = serde_json::from_str::<serde_json::Value>(value)
                        && let Some(trust) = loose_trust(&json)
                    {
                        settings.card_stats_trust = trust;
                    }
                }
                GAME_OVERLAY => read_bool(value, &mut settings.game_overlay),
                DRAFT_NOTES_WINDOW => read_bool(value, &mut settings.draft_notes_window),
                LEAGUE_WINDOW => read_bool(value, &mut settings.league_window),
                OVERLAY_SHOW_OPPONENT => read_bool(value, &mut settings.overlay_show_opponent),
                OVERLAY_SHOW_DRAW_ODDS => read_bool(value, &mut settings.overlay_show_draw_odds),
                OVERLAY_SHOW_REVEALS => read_bool(value, &mut settings.overlay_show_reveals),
                OVERLAY_SHOW_SIDEBOARD => read_bool(value, &mut settings.overlay_show_sideboard),
                OVERLAY_ARTWORK => {
                    // 0.x: a value that is not a mode is unset.
                    settings.overlay_artwork = serde_json::from_str(value).ok();
                }
                OVERLAY_SIZE => {
                    // 0.x: anything but `compact` is full.
                    settings.overlay_size = serde_json::from_str(value).unwrap_or_default();
                }
                OVERLAY_PUBLISH => read_bool(value, &mut settings.overlay_publish),
                OVERLAY_LEAGUE_DISPLAY => {
                    settings.overlay_league_display = LeagueDisplay::from_stored(value);
                }
                OVERLAY_FINISHED_MINUTES => {
                    if let Ok(minutes) = serde_json::from_str::<u32>(value) {
                        settings.overlay_finished_minutes = minutes;
                    }
                }
                OVERLAY_LEAGUE_THEME => {
                    settings.overlay_league_theme = serde_json::from_str(value).unwrap_or_default();
                }
                OVERLAY_EVENT_STRIP => read_bool(value, &mut settings.overlay_event_strip),
                WATCHER_ACTIVE => read_bool(value, &mut settings.watcher_active),
                ACTIVE_ACCOUNT => {
                    settings.active_account = serde_json::from_str(value).ok().flatten();
                }
                BETA_UPDATES => {
                    settings.beta_updates = serde_json::from_str(value).ok().flatten();
                }
                RECORDER_ENABLED => read_bool(value, &mut settings.recorder_enabled),
                RECORDER_ACCEPTANCE => {
                    settings.recorder_acceptance = serde_json::from_str(value).ok().flatten();
                }
                ARENA_ENABLED => {
                    settings.arena_enabled = serde_json::from_str(value).ok().flatten();
                }
                ENDSTEP_ENABLED => read_bool(value, &mut settings.endstep_enabled),
                _ => {}
            }
        }
        settings.cleaned()
    }

    /// One row per field, values as JSON.
    pub fn to_rows(&self) -> Vec<(String, String)> {
        vec![
            (LOG_FOLDER.to_string(), json(&self.log_folder)),
            (CLOSE_TO_TRAY.to_string(), json(&self.close_to_tray)),
            (SHARE_USAGE.to_string(), json(&self.share_usage)),
            (MTGO_DATA_FOLDER.to_string(), json(&self.mtgo_data_folder)),
            (
                HIDE_ARCHIVED_DECKS.to_string(),
                json(&self.hide_archived_decks),
            ),
            (DECKS_PER_PAGE.to_string(), json(&self.decks_per_page)),
            (DECK_CARD_SIZE.to_string(), json(&self.deck_card_size)),
            (INTERFACE_SCALE.to_string(), json(&self.interface_scale)),
            (CARD_IMAGES_PATH.to_string(), json(&self.card_images_path)),
            (CARD_STATS_TRUST.to_string(), json(&self.card_stats_trust)),
            (GAME_OVERLAY.to_string(), json(&self.game_overlay)),
            (
                DRAFT_NOTES_WINDOW.to_string(),
                json(&self.draft_notes_window),
            ),
            (LEAGUE_WINDOW.to_string(), json(&self.league_window)),
            (
                OVERLAY_SHOW_OPPONENT.to_string(),
                json(&self.overlay_show_opponent),
            ),
            (
                OVERLAY_SHOW_DRAW_ODDS.to_string(),
                json(&self.overlay_show_draw_odds),
            ),
            (
                OVERLAY_SHOW_REVEALS.to_string(),
                json(&self.overlay_show_reveals),
            ),
            (
                OVERLAY_SHOW_SIDEBOARD.to_string(),
                json(&self.overlay_show_sideboard),
            ),
            (OVERLAY_ARTWORK.to_string(), json(&self.overlay_artwork)),
            (OVERLAY_SIZE.to_string(), json(&self.overlay_size)),
            (OVERLAY_PUBLISH.to_string(), json(&self.overlay_publish)),
            (
                OVERLAY_LEAGUE_DISPLAY.to_string(),
                json(&self.overlay_league_display),
            ),
            (
                OVERLAY_FINISHED_MINUTES.to_string(),
                json(&self.overlay_finished_minutes),
            ),
            (
                OVERLAY_LEAGUE_THEME.to_string(),
                json(&self.overlay_league_theme),
            ),
            (
                OVERLAY_EVENT_STRIP.to_string(),
                json(&self.overlay_event_strip),
            ),
            (WATCHER_ACTIVE.to_string(), json(&self.watcher_active)),
            (ACTIVE_ACCOUNT.to_string(), json(&self.active_account)),
            (BETA_UPDATES.to_string(), json(&self.beta_updates)),
            (RECORDER_ENABLED.to_string(), json(&self.recorder_enabled)),
            (
                RECORDER_ACCEPTANCE.to_string(),
                json(&self.recorder_acceptance),
            ),
            (ARENA_ENABLED.to_string(), json(&self.arena_enabled)),
            (ENDSTEP_ENABLED.to_string(), json(&self.endstep_enabled)),
        ]
    }

    /// Trim the folders; a blank folder means auto-detect. A deck page size
    /// that is not an option is the default (0.x `decksPerPage`). The trust
    /// threshold is clamped (0.x `setCardStatsTrust`), and so are the
    /// league card's finished minutes.
    pub fn cleaned(mut self) -> Self {
        self.log_folder = cleaned_folder(self.log_folder);
        self.mtgo_data_folder = cleaned_folder(self.mtgo_data_folder);
        self.card_images_path = cleaned_folder(self.card_images_path);
        self.card_stats_trust = self.card_stats_trust.min(MAX_CARD_STATS_TRUST);
        self.overlay_finished_minutes = self
            .overlay_finished_minutes
            .clamp(MIN_FINISHED_MINUTES, MAX_FINISHED_MINUTES);
        if !DECKS_PER_PAGE_OPTIONS.contains(&self.decks_per_page) {
            self.decks_per_page = DEFAULT_DECKS_PER_PAGE;
        }
        if !INTERFACE_SCALE_OPTIONS.contains(&self.interface_scale) {
            self.interface_scale = DEFAULT_INTERFACE_SCALE;
        }
        self
    }

    /// Keep the fields that have their own command from `stored`, so a full
    /// settings save from what a page last read never undoes a newer switch:
    /// the card image folder move (`card_images_path`), the Watcher card (`watcher_active`, so a path
    /// that finds no log never stops it: release spec ruling 6), the header's
    /// account switch (`active_account`), the Updates card's beta switch
    /// (`beta_updates`), the Replays page's switch and acceptance
    /// (`recorder_enabled`, `recorder_acceptance`), the Integrations switches
    /// (`arena_enabled`, `endstep_enabled`) and the trust slider (`card_stats_trust`). A new
    /// field with its own command belongs here too.
    pub fn keep_command_owned(&mut self, stored: &Settings) {
        self.card_images_path = stored.card_images_path.clone();
        self.watcher_active = stored.watcher_active;
        self.active_account = stored.active_account;
        self.beta_updates = stored.beta_updates;
        self.recorder_enabled = stored.recorder_enabled;
        self.recorder_acceptance = stored.recorder_acceptance;
        self.arena_enabled = stored.arena_enabled;
        self.endstep_enabled = stored.endstep_enabled;
        self.card_stats_trust = stored.card_stats_trust;
    }

    /// Take the deck settings and `card_stats_trust` from the
    /// kept 0.x settings (a JSON object, as 0.x wrote them to settings.json).
    /// 0.x casts loosely (`(bool)`, `(int)`), so a number flag or a numeric
    /// string page size also reads. 0.x's `card_images_path` is never taken:
    /// v1 keeps its images in its own folder (cards spec ruling 3).
    fn seed_from_legacy(&mut self, legacy: &str) {
        let Ok(serde_json::Value::Object(kept)) = serde_json::from_str(legacy) else {
            return;
        };
        if let Some(flag) = kept.get(HIDE_ARCHIVED_DECKS).and_then(loose_bool) {
            self.hide_archived_decks = flag;
        }
        if let Some(count) = kept.get(DECKS_PER_PAGE).and_then(loose_count) {
            self.decks_per_page = count;
        }
        if let Some(size) = kept
            .get(DECK_CARD_SIZE)
            .and_then(|value| serde_json::from_value::<DeckCardSize>(value.clone()).ok())
        {
            self.deck_card_size = size;
        }
        if let Some(trust) = kept.get(CARD_STATS_TRUST).and_then(loose_trust) {
            self.card_stats_trust = trust;
        }
        self.seed_overlay_from_legacy(&kept);
    }

    /// The overlay settings from the kept 0.x settings. `game_overlay` with
    /// no 0.x value of its own (missing or null) is on when either window it
    /// replaced was (0.x `showGameOverlay`).
    fn seed_overlay_from_legacy(&mut self, kept: &serde_json::Map<String, serde_json::Value>) {
        let flag = |key: &str| kept.get(key).and_then(loose_bool);
        match kept.get(GAME_OVERLAY) {
            Some(value) if !value.is_null() => {
                if let Some(on) = loose_bool(value) {
                    self.game_overlay = on;
                }
            }
            _ => {
                self.game_overlay = flag(LEGACY_DECK_WINDOW).unwrap_or(false)
                    || flag(LEGACY_OPPONENT_WINDOW).unwrap_or(false);
            }
        }
        let flags: [(&str, &mut bool); 7] = [
            (DRAFT_NOTES_WINDOW, &mut self.draft_notes_window),
            (LEAGUE_WINDOW, &mut self.league_window),
            (OVERLAY_SHOW_OPPONENT, &mut self.overlay_show_opponent),
            (OVERLAY_SHOW_DRAW_ODDS, &mut self.overlay_show_draw_odds),
            (OVERLAY_SHOW_REVEALS, &mut self.overlay_show_reveals),
            (OVERLAY_SHOW_SIDEBOARD, &mut self.overlay_show_sideboard),
            (OVERLAY_PUBLISH, &mut self.overlay_publish),
        ];
        for (key, field) in flags {
            if let Some(on) = flag(key) {
                *field = on;
            }
        }
        if let Some(value) = kept.get(OVERLAY_ARTWORK) {
            self.overlay_artwork = serde_json::from_value(value.clone()).ok();
        }
        if let Some(value) = kept.get(OVERLAY_SIZE) {
            self.overlay_size = serde_json::from_value(value.clone()).unwrap_or_default();
        }
    }

    /// The tracker's `mtgo_data_folder`: the chosen folder, or none to
    /// auto-detect.
    pub fn mtgo_data_folder_path(&self) -> Option<PathBuf> {
        self.mtgo_data_folder.as_ref().map(PathBuf::from)
    }

    /// The folders tracking runs with.
    pub fn tracked_folders(&self) -> TrackedFolders {
        TrackedFolders {
            log_folder: self.log_folder.clone(),
            mtgo_data_folder: self.mtgo_data_folder.clone(),
        }
    }

    /// The tracker's `log_overrides`: the chosen folder, or none to auto-detect.
    pub fn log_overrides(&self) -> Vec<PathBuf> {
        self.log_folder.iter().map(PathBuf::from).collect()
    }
}

/// The setting rows a 0.x import stores (the deck settings
/// and `card_stats_trust`): each one v1 still has
/// at its default (`stored` holds v1's rows) takes the kept 0.x value
/// (`legacy`, as `legacy_0x_settings` stores it). A row is written only where
/// the value changes. Seeding in `from_rows` alone would miss a setting that
/// already has a row of its own, as every setting does once v1 saved them.
pub fn legacy_seeded_rows(stored: &[(String, String)], legacy: &str) -> Vec<(String, String)> {
    // Only the first import fills them: after it, a default value may be
    // the user's own choice, which a later import must not replace.
    if stored
        .iter()
        .any(|(key, _)| key == crate::legacy::LEGACY_LAST_IMPORT)
    {
        return Vec::new();
    }
    let own: Vec<(String, String)> = stored
        .iter()
        .filter(|(key, _)| key != crate::legacy::LEGACY_SETTINGS)
        .cloned()
        .collect();
    let current = Settings::from_rows(&own);
    let defaults = Settings::default();
    let mut from_0x = Settings::default();
    from_0x.seed_from_legacy(legacy);
    let from_0x = from_0x.cleaned();

    let mut rows = Vec::new();
    if current.hide_archived_decks == defaults.hide_archived_decks
        && from_0x.hide_archived_decks != current.hide_archived_decks
    {
        rows.push((
            HIDE_ARCHIVED_DECKS.to_string(),
            json(&from_0x.hide_archived_decks),
        ));
    }
    if current.decks_per_page == defaults.decks_per_page
        && from_0x.decks_per_page != current.decks_per_page
    {
        rows.push((DECKS_PER_PAGE.to_string(), json(&from_0x.decks_per_page)));
    }
    if current.deck_card_size == defaults.deck_card_size
        && from_0x.deck_card_size != current.deck_card_size
    {
        rows.push((DECK_CARD_SIZE.to_string(), json(&from_0x.deck_card_size)));
    }
    if current.card_stats_trust == defaults.card_stats_trust
        && from_0x.card_stats_trust != current.card_stats_trust
    {
        rows.push((
            CARD_STATS_TRUST.to_string(),
            json(&from_0x.card_stats_trust),
        ));
    }
    // The overlay settings (overlay spec section 12), by the same rule.
    let current_rows = current.to_rows();
    let default_rows = defaults.to_rows();
    let from_0x_rows = from_0x.to_rows();
    for index in 0..current_rows.len() {
        let (key, current_value) = &current_rows[index];
        if !OVERLAY_SETTINGS.contains(&key.as_str()) {
            continue;
        }
        let from_0x_value = &from_0x_rows[index].1;
        if current_value == &default_rows[index].1 && from_0x_value != current_value {
            rows.push((key.clone(), from_0x_value.clone()));
        }
    }
    rows
}

/// The overlay fields of `Settings`, in `to_rows` order.
const OVERLAY_SETTINGS: [&str; 10] = [
    GAME_OVERLAY,
    DRAFT_NOTES_WINDOW,
    LEAGUE_WINDOW,
    OVERLAY_SHOW_OPPONENT,
    OVERLAY_SHOW_DRAW_ODDS,
    OVERLAY_SHOW_REVEALS,
    OVERLAY_SHOW_SIDEBOARD,
    OVERLAY_ARTWORK,
    OVERLAY_SIZE,
    OVERLAY_PUBLISH,
];

/// Whether the game overlay is collapsed (UI state, default no): v1's own
/// row, else the value the 0.x import kept.
pub fn overlay_collapsed(rows: &[(String, String)]) -> bool {
    let Some(value) = own_or_legacy(rows, OVERLAY_COLLAPSED) else {
        return false;
    };
    loose_bool(&value).unwrap_or(false)
}

pub fn overlay_collapsed_row(collapsed: bool) -> (String, String) {
    (OVERLAY_COLLAPSED.to_string(), json(&collapsed))
}

/// The game overlay's height before it was collapsed (0.x `is_numeric`, as
/// a whole number); `None` while expanded. v1's own row, else the kept 0.x
/// value.
pub fn overlay_expanded_height(rows: &[(String, String)]) -> Option<u32> {
    let value = own_or_legacy(rows, OVERLAY_EXPANDED_HEIGHT)?;
    match value {
        serde_json::Value::Number(number) => number
            .as_u64()
            .or_else(|| number.as_f64().filter(|n| *n >= 0.0).map(|n| n as u64))
            .and_then(|n| u32::try_from(n).ok()),
        serde_json::Value::String(text) => text
            .trim()
            .parse::<f64>()
            .ok()
            .filter(|n| *n >= 0.0 && *n <= f64::from(u32::MAX))
            .map(|n| n as u32),
        _ => None,
    }
}

pub fn overlay_expanded_height_row(height: Option<u32>) -> (String, String) {
    (OVERLAY_EXPANDED_HEIGHT.to_string(), json(&height))
}

/// Whether the 0.x launch at login question is settled (default no).
pub fn autostart_from_0x_applied(rows: &[(String, String)]) -> bool {
    own_row(rows, AUTOSTART_FROM_0X_APPLIED)
        .and_then(|value| serde_json::from_str::<bool>(value).ok())
        .unwrap_or(false)
}

/// The row that settles the 0.x launch at login question for good.
pub fn autostart_from_0x_applied_row() -> (String, String) {
    (AUTOSTART_FROM_0X_APPLIED.to_string(), json(&true))
}

/// The version whose What's new was last shown or seeded; `None` when
/// unset, blank or not a string.
pub fn whats_new_seen_version(rows: &[(String, String)]) -> Option<String> {
    let value = own_row(rows, WHATS_NEW_SEEN_VERSION)?;
    let version = serde_json::from_str::<String>(value).ok()?;
    let version = version.trim().to_string();
    if version.is_empty() {
        None
    } else {
        Some(version)
    }
}

/// The row that records `version` as the What's new last shown or seeded.
pub fn whats_new_seen_version_row(version: &str) -> (String, String) {
    (WHATS_NEW_SEEN_VERSION.to_string(), json(&version))
}

/// The first run's saved 0.x answer; a value that is not one is unset.
pub fn first_run_choice(rows: &[(String, String)]) -> Option<crate::first_run::FirstRunChoice> {
    own_row(rows, FIRST_RUN_CHOICE)
        .and_then(|value| serde_json::from_str::<crate::first_run::FirstRunChoice>(value).ok())
}

pub fn first_run_choice_row(choice: crate::first_run::FirstRunChoice) -> (String, String) {
    (FIRST_RUN_CHOICE.to_string(), json(&choice))
}

/// Whether the `fresh_install` marker is set; unset or unreadable is not.
pub fn fresh_install(rows: &[(String, String)]) -> bool {
    own_row(rows, FRESH_INSTALL)
        .and_then(|value| serde_json::from_str::<bool>(value).ok())
        .unwrap_or(false)
}

pub fn fresh_install_row(fresh: bool) -> (String, String) {
    (FRESH_INSTALL.to_string(), json(&fresh))
}

/// The custom background's file name in `<app data>/overlay/`.
pub fn overlay_background_file(rows: &[(String, String)]) -> Option<String> {
    let value = own_row(rows, OVERLAY_BACKGROUND_FILE)?;
    serde_json::from_str::<Option<String>>(value)
        .ok()
        .flatten()
        .filter(|name| !name.is_empty())
}

pub fn overlay_background_file_row(file_name: Option<&str>) -> (String, String) {
    (OVERLAY_BACKGROUND_FILE.to_string(), json(&file_name))
}

/// The uploaded copy of the background (internal, never imported). A row
/// without both parts reads as none (0.x `isset`).
pub fn overlay_background_remote(rows: &[(String, String)]) -> Option<BackgroundRemote> {
    let value = own_row(rows, OVERLAY_BACKGROUND_REMOTE)?;
    serde_json::from_str::<Option<BackgroundRemote>>(value)
        .ok()
        .flatten()
}

pub fn overlay_background_remote_row(remote: Option<&BackgroundRemote>) -> (String, String) {
    (OVERLAY_BACKGROUND_REMOTE.to_string(), json(&remote))
}

/// When the hosted overlay was last published (ms; internal).
pub fn overlay_last_published_at(rows: &[(String, String)]) -> Option<i64> {
    let value = own_row(rows, OVERLAY_LAST_PUBLISHED_AT)?;
    serde_json::from_str::<Option<i64>>(value).ok().flatten()
}

pub fn overlay_last_published_at_row(at_ms: Option<i64>) -> (String, String) {
    (OVERLAY_LAST_PUBLISHED_AT.to_string(), json(&at_ms))
}

/// Why the last hosted overlay push failed; `None` after a success or for
/// a value that is not a known reason.
pub fn overlay_publish_error(rows: &[(String, String)]) -> Option<PublishError> {
    let value = own_row(rows, OVERLAY_PUBLISH_ERROR)?;
    serde_json::from_str::<Option<PublishError>>(value)
        .ok()
        .flatten()
}

pub fn overlay_publish_error_row(error: Option<PublishError>) -> (String, String) {
    (OVERLAY_PUBLISH_ERROR.to_string(), json(&error))
}

/// The stored value of v1's own row.
fn own_row<'a>(rows: &'a [(String, String)], key: &str) -> Option<&'a str> {
    rows.iter()
        .find(|(stored, _)| stored == key)
        .map(|(_, value)| value.as_str())
}

/// v1's own row as JSON, else the value the 0.x import kept for that key.
fn own_or_legacy(rows: &[(String, String)], key: &str) -> Option<serde_json::Value> {
    if let Some(own) = own_row(rows, key) {
        return serde_json::from_str(own).ok();
    }
    let legacy = own_row(rows, crate::legacy::LEGACY_SETTINGS)?;
    let Ok(serde_json::Value::Object(mut kept)) = serde_json::from_str(legacy) else {
        return None;
    };
    kept.remove(key)
}

/// Read a stored flag; an unreadable value keeps the field as it was.
fn read_bool(value: &str, field: &mut bool) {
    if let Ok(flag) = serde_json::from_str::<bool>(value) {
        *field = flag;
    }
}

/// A trimmed folder, or `None` when blank.
fn cleaned_folder(folder: Option<String>) -> Option<String> {
    folder
        .map(|folder| folder.trim().to_string())
        .filter(|folder| !folder.is_empty())
}

/// A 0.x flag: a boolean, or a number (0 is off).
pub(crate) fn loose_bool(value: &serde_json::Value) -> Option<bool> {
    match value {
        serde_json::Value::Bool(flag) => Some(*flag),
        serde_json::Value::Number(number) => number.as_f64().map(|n| n != 0.0),
        _ => None,
    }
}

/// A trust threshold: a whole number (or a string holding one), clamped to
/// 0 to 100,000 as 0.x `cardStatsTrust` reads it. Anything else is `None`.
fn loose_trust(value: &serde_json::Value) -> Option<u32> {
    let number = match value {
        serde_json::Value::Number(number) => number
            .as_i64()
            .or_else(|| number.as_f64().map(|n| n.trunc() as i64))?,
        serde_json::Value::String(text) => text.trim().parse::<i64>().ok()?,
        _ => return None,
    };
    let clamped = number.clamp(0, i64::from(MAX_CARD_STATS_TRUST));
    u32::try_from(clamped).ok()
}

/// A 0.x count: a whole number, or a string holding one.
fn loose_count(value: &serde_json::Value) -> Option<u32> {
    match value {
        serde_json::Value::Number(number) => number.as_u64().and_then(|n| u32::try_from(n).ok()),
        serde_json::Value::String(text) => text.trim().parse::<u32>().ok(),
        _ => None,
    }
}

fn json<T: Serialize>(value: &T) -> String {
    // Strings, options and booleans always serialise.
    serde_json::to_string(value).unwrap_or_else(|_| "null".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect()
    }

    #[test]
    fn the_watcher_defaults_on_and_round_trips() {
        assert!(Settings::default().watcher_active, "0.x default: on");
        let off = Settings {
            watcher_active: false,
            ..Settings::default()
        };
        let stored = off.to_rows();
        assert!(stored.contains(&(WATCHER_ACTIVE.to_string(), "false".to_string())));
        assert_eq!(Settings::from_rows(&stored), off);
        let unreadable = rows(&[(WATCHER_ACTIVE, "\"maybe\"")]);
        assert!(Settings::from_rows(&unreadable).watcher_active);
    }

    #[test]
    fn nothing_stored_gives_the_defaults() {
        let settings = Settings::from_rows(&[]);
        assert_eq!(settings, Settings::default());
        assert_eq!(settings.log_folder, None);
        assert!(
            settings.close_to_tray,
            "close to tray is on by default (spec 6)"
        );
        assert!(settings.log_overrides().is_empty());
    }

    /// The last What's new version shown (release spec 7, 14): unset until
    /// the first start decides; junk or blank reads as unset.
    #[test]
    fn whats_new_seen_version_stays_unset_until_saved() {
        assert_eq!(whats_new_seen_version(&[]), None);
        let saved = vec![whats_new_seen_version_row("1.0.0-beta.2")];
        assert_eq!(saved[0].0, "whats_new_seen_version");
        assert_eq!(
            whats_new_seen_version(&saved).as_deref(),
            Some("1.0.0-beta.2")
        );
        for junk in ["true", "\"\"", "\"  \"", "not json"] {
            let stored = rows(&[("whats_new_seen_version", junk)]);
            assert_eq!(whats_new_seen_version(&stored), None, "{junk}");
        }
        // Not a user setting: a generic settings save never writes it.
        let generic = Settings::default().to_rows();
        assert!(generic.iter().all(|(key, _)| key != WHATS_NEW_SEEN_VERSION));
    }

    /// Unset until the user chooses (release spec 14): the build decides.
    #[test]
    fn beta_updates_stay_unset_until_chosen() {
        assert_eq!(Settings::default().beta_updates, None);
        let unset = Settings::default().to_rows();
        assert_eq!(Settings::from_rows(&unset).beta_updates, None);
        let chosen = rows(&[("beta_updates", "true")]);
        assert_eq!(Settings::from_rows(&chosen).beta_updates, Some(true));
        let junk = rows(&[("beta_updates", "\"yes\"")]);
        assert_eq!(Settings::from_rows(&junk).beta_updates, None);
    }

    /// The recorder switch is off until the Replays page's command turns it
    /// on, and a general save (a page that read the settings earlier) never
    /// turns it on or drops the acceptance.
    #[test]
    fn the_switch_is_off_by_default_and_survives_a_general_save() {
        let fresh = Settings::from_rows(&[]);
        assert!(!fresh.recorder_enabled);
        assert_eq!(fresh.recorder_acceptance, None);
        let acceptance = RecorderAcceptance {
            notice_version: 1,
            consent_version: 1,
            accepted_at_ms: 5,
        };
        let stored = Settings {
            recorder_enabled: true,
            recorder_acceptance: Some(acceptance),
            ..Settings::default()
        };
        // A stale page saves the old (off, unaccepted) values.
        let mut saved = Settings::default();
        saved.keep_command_owned(&stored);
        assert!(saved.recorder_enabled);
        assert_eq!(saved.recorder_acceptance, Some(acceptance));
        // And a page that sends "on" cannot switch it on either.
        let mut sneaky = Settings {
            recorder_enabled: true,
            recorder_acceptance: Some(acceptance),
            ..Settings::default()
        };
        sneaky.keep_command_owned(&Settings::default());
        assert!(!sneaky.recorder_enabled);
        assert_eq!(sneaky.recorder_acceptance, None);
    }

    /// The Integrations switches: Arena is unset until chosen, Endstep is
    /// off, and a general save keeps both.
    #[test]
    fn integration_settings_round_trip() {
        let fresh = Settings::from_rows(&[]);
        assert_eq!(fresh.arena_enabled, None);
        assert!(!fresh.endstep_enabled);
        let off = Settings::from_rows(&rows(&[("arena_enabled", "false")]));
        assert_eq!(off.arena_enabled, Some(false));
        let on = Settings::from_rows(&rows(&[
            ("arena_enabled", "true"),
            ("endstep_enabled", "true"),
        ]));
        assert_eq!(on.arena_enabled, Some(true));
        assert!(on.endstep_enabled);
        let junk = Settings::from_rows(&rows(&[("arena_enabled", "\"yes\"")]));
        assert_eq!(junk.arena_enabled, None);
        let mut saved = Settings::default();
        saved.keep_command_owned(&on);
        assert_eq!(saved.arena_enabled, Some(true));
        assert!(saved.endstep_enabled);
    }

    #[test]
    fn stored_values_round_trip() {
        let settings = Settings {
            log_folder: Some("C:/Users/Q/AppData/Local/Apps/2.0".to_string()),
            close_to_tray: false,
            mtgo_data_folder: None,
            hide_archived_decks: false,
            decks_per_page: 48,
            deck_card_size: DeckCardSize::Compact,
            interface_scale: 125,
            card_images_path: Some("D:/Q".to_string()),
            card_stats_trust: 120,
            game_overlay: true,
            draft_notes_window: false,
            league_window: true,
            overlay_show_opponent: false,
            overlay_show_draw_odds: false,
            overlay_show_reveals: false,
            overlay_show_sideboard: false,
            overlay_artwork: Some(OverlayArtwork::NoArt),
            overlay_size: OverlaySize::Compact,
            overlay_publish: true,
            overlay_league_display: LeagueDisplay::default(),
            overlay_finished_minutes: 10,
            overlay_league_theme: LeagueTheme::Classic,
            overlay_event_strip: false,
            watcher_active: false,
            active_account: Some(ActiveAccount {
                account_id: 2,
                switched_at_ms: 1_800_000_000_000,
            }),
            beta_updates: Some(false),
            recorder_enabled: true,
            recorder_acceptance: Some(RecorderAcceptance {
                notice_version: 1,
                consent_version: 1,
                accepted_at_ms: 1_800_000_000_000,
            }),
            arena_enabled: Some(false),
            endstep_enabled: true,
            share_usage: false,
        };
        assert_eq!(Settings::from_rows(&settings.to_rows()), settings);
        assert_eq!(
            settings.to_rows(),
            rows(&[
                ("log_folder", "\"C:/Users/Q/AppData/Local/Apps/2.0\""),
                ("close_to_tray", "false"),
                ("share_usage", "false"),
                ("mtgo_data_folder", "null"),
                ("hide_archived_decks", "false"),
                ("decks_per_page", "48"),
                ("deck_card_size", "\"compact\""),
                ("interface_scale", "125"),
                ("card_images_path", "\"D:/Q\""),
                ("card_stats_trust", "120"),
                ("game_overlay", "true"),
                ("draft_notes_window", "false"),
                ("league_window", "true"),
                ("overlay_show_opponent", "false"),
                ("overlay_show_draw_odds", "false"),
                ("overlay_show_reveals", "false"),
                ("overlay_show_sideboard", "false"),
                ("overlay_artwork", "\"none\""),
                ("overlay_size", "\"compact\""),
                ("overlay_publish", "true"),
                (
                    "overlay_league_display",
                    r#"{"games":"dots","event":true,"format":true,"title":true,"progress":true,"status":true,"record":true,"standing":false}"#,
                ),
                ("overlay_finished_minutes", "10"),
                ("overlay_league_theme", "\"classic\""),
                ("overlay_event_strip", "false"),
                ("watcher_active", "false"),
                (
                    "active_account",
                    "{\"account_id\":2,\"switched_at_ms\":1800000000000}",
                ),
                ("beta_updates", "false"),
                ("recorder_enabled", "true"),
                (
                    "recorder_acceptance",
                    r#"{"noticeVersion":1,"consentVersion":1,"acceptedAtMs":1800000000000}"#,
                ),
                ("arena_enabled", "false"),
                ("endstep_enabled", "true"),
            ])
        );
        assert_eq!(
            settings.log_overrides(),
            vec![PathBuf::from("C:/Users/Q/AppData/Local/Apps/2.0")]
        );
    }

    #[test]
    fn unknown_keys_and_unreadable_values_are_ignored() {
        let settings = Settings::from_rows(&rows(&[
            ("theme", "\"dark\""),
            ("close_to_tray", "\"yes\""),
            ("log_folder", "42"),
        ]));
        assert_eq!(settings, Settings::default());
    }

    #[test]
    fn a_blank_log_folder_means_auto_detect() {
        let settings = Settings {
            log_folder: Some("   ".to_string()),
            close_to_tray: true,
            mtgo_data_folder: None,
            ..Settings::default()
        }
        .cleaned();
        assert_eq!(settings.log_folder, None);
        let trimmed = Settings {
            log_folder: Some("  /home/q/.wine  ".to_string()),
            close_to_tray: true,
            mtgo_data_folder: None,
            ..Settings::default()
        }
        .cleaned();
        assert_eq!(trimmed.log_folder.as_deref(), Some("/home/q/.wine"));
        let stored_blank = Settings::from_rows(&rows(&[("log_folder", "\"\"")]));
        assert_eq!(stored_blank.log_folder, None);
    }

    #[test]
    fn the_mtgo_data_folder_round_trips_and_a_blank_one_means_auto_detect() {
        let settings = Settings {
            mtgo_data_folder: Some("  C:/Users/Q/AppData/Local/Apps/2.0/Data  ".to_string()),
            ..Settings::default()
        }
        .cleaned();
        assert_eq!(
            settings.mtgo_data_folder.as_deref(),
            Some("C:/Users/Q/AppData/Local/Apps/2.0/Data")
        );
        assert_eq!(Settings::from_rows(&settings.to_rows()), settings);
        assert!(settings.to_rows().contains(&(
            "mtgo_data_folder".to_string(),
            "\"C:/Users/Q/AppData/Local/Apps/2.0/Data\"".to_string()
        )));
        assert_eq!(
            settings.mtgo_data_folder_path(),
            Some(PathBuf::from("C:/Users/Q/AppData/Local/Apps/2.0/Data"))
        );

        let blank = Settings::from_rows(&rows(&[("mtgo_data_folder", "\"  \"")]));
        assert_eq!(blank.mtgo_data_folder, None);
        assert_eq!(blank.mtgo_data_folder_path(), None);
        assert_eq!(Settings::default().mtgo_data_folder, None);
    }

    #[test]
    fn the_ui_sees_camel_case_fields() {
        let json = serde_json::to_value(Settings::default()).expect("json");
        assert_eq!(
            json,
            serde_json::json!({
                "activeAccount": null,
                "logFolder": null,
                "closeToTray": true,
                "shareUsage": true,
                "mtgoDataFolder": null,
                "hideArchivedDecks": true,
                "decksPerPage": 12,
                "interfaceScale": 100,
                "deckCardSize": "large",
                "cardImagesPath": null,
                "cardStatsTrust": 50,
                "gameOverlay": false,
                "draftNotesWindow": true,
                "leagueWindow": false,
                "overlayShowOpponent": true,
                "overlayShowDrawOdds": true,
                "overlayShowReveals": true,
                "overlayShowSideboard": true,
                "overlayArtwork": null,
                "overlaySize": "full",
                "overlayPublish": false,
                "overlayLeagueDisplay": {
                    "games": "dots", "event": true, "format": true, "title": true,
                    "progress": true, "status": true, "record": true,
                    "standing": false
                },
                "overlayFinishedMinutes": 5,
                "overlayLeagueTheme": "classic",
                "overlayEventStrip": true,
                "watcherActive": true,
                "betaUpdates": null,
                "recorderEnabled": false,
                "recorderAcceptance": null,
                "arenaEnabled": null,
                "endstepEnabled": false,
            })
        );
    }

    #[test]
    fn the_deck_settings_default_as_0x() {
        let settings = Settings::from_rows(&[]);
        assert!(settings.hide_archived_decks);
        assert_eq!(settings.decks_per_page, 12);
        assert_eq!(settings.deck_card_size, DeckCardSize::Large);
    }

    #[test]
    fn an_interface_scale_that_is_not_an_option_reads_as_100() {
        assert_eq!(Settings::from_rows(&[]).interface_scale, 100);
        for (stored, expected) in [
            ("80", 80),
            ("150", 150),
            ("95", 100),
            ("0", 100),
            ("\"x\"", 100),
        ] {
            let settings = Settings::from_rows(&rows(&[("interface_scale", stored)]));
            assert_eq!(settings.interface_scale, expected, "{stored}");
        }
        let saved = Settings {
            interface_scale: 400,
            ..Settings::default()
        }
        .cleaned();
        assert_eq!(saved.interface_scale, 100);
    }

    #[test]
    fn the_zoom_factor_is_the_percent_over_100() {
        assert_eq!(zoom_factor(100), 1.0);
        assert_eq!(zoom_factor(125), 1.25);
        assert_eq!(zoom_factor(80), 0.8);
    }

    #[test]
    fn a_deck_page_size_that_is_not_an_option_reads_as_12() {
        for (stored, expected) in [("24", 24), ("48", 48), ("13", 12), ("0", 12), ("\"x\"", 12)] {
            let settings = Settings::from_rows(&rows(&[("decks_per_page", stored)]));
            assert_eq!(settings.decks_per_page, expected, "{stored}");
        }
        let saved = Settings {
            decks_per_page: 30,
            ..Settings::default()
        }
        .cleaned();
        assert_eq!(saved.decks_per_page, 12);
        let size = Settings::from_rows(&rows(&[("deck_card_size", "\"huge\"")]));
        assert_eq!(size.deck_card_size, DeckCardSize::Large);
    }

    #[test]
    fn deck_settings_are_seeded_from_the_kept_0x_settings() {
        let legacy = r#"{"hide_archived_decks":false,"decks_per_page":"24","deck_card_size":"compact","log_path":"C:/q"}"#;
        let seeded = Settings::from_rows(&rows(&[("legacy_0x_settings", legacy)]));
        assert!(!seeded.hide_archived_decks);
        assert_eq!(seeded.decks_per_page, 24);
        assert_eq!(seeded.deck_card_size, DeckCardSize::Compact);
        assert_eq!(seeded.log_folder, None, "only the deck settings are seeded");

        // A stored row of v1's own wins over 0.x.
        let own = Settings::from_rows(&rows(&[
            ("decks_per_page", "48"),
            ("hide_archived_decks", "true"),
            ("legacy_0x_settings", legacy),
        ]));
        assert_eq!(own.decks_per_page, 48);
        assert!(own.hide_archived_decks);
        assert_eq!(own.deck_card_size, DeckCardSize::Compact);

        // 0.x casts loosely; values it would refuse keep the defaults.
        let loose = Settings::from_rows(&rows(&[(
            "legacy_0x_settings",
            r#"{"hide_archived_decks":0,"decks_per_page":100,"deck_card_size":"huge"}"#,
        )]));
        assert!(!loose.hide_archived_decks);
        assert_eq!(loose.decks_per_page, 12);
        assert_eq!(loose.deck_card_size, DeckCardSize::Large);

        let broken = Settings::from_rows(&rows(&[("legacy_0x_settings", "not json")]));
        assert_eq!(broken, Settings::default());
    }

    #[test]
    fn an_import_gives_the_0x_deck_settings_to_those_v1_left_at_their_defaults() {
        let legacy =
            r#"{"hide_archived_decks":false,"decks_per_page":24,"deck_card_size":"compact"}"#;
        // v1 saved its settings once (every field has a row), changing only
        // the page size; an earlier import's kept settings are ignored.
        let saved = Settings {
            decks_per_page: 48,
            ..Settings::default()
        };
        let mut stored = saved.to_rows();
        stored.push((
            "legacy_0x_settings".to_string(),
            r#"{"deck_card_size":"large"}"#.to_string(),
        ));
        assert_eq!(
            legacy_seeded_rows(&stored, legacy),
            rows(&[
                ("hide_archived_decks", "false"),
                ("deck_card_size", "\"compact\""),
            ])
        );

        // Nothing differs from 0.x, or 0.x kept nothing readable: no rows.
        let same = r#"{"hide_archived_decks":true,"decks_per_page":12}"#;
        assert!(legacy_seeded_rows(&[], same).is_empty());
        assert!(legacy_seeded_rows(&[], "not json").is_empty());
    }

    // The deck index always groups by archetype, as 0.x does: a row left by
    // the removed toggle is ignored.
    #[test]
    fn a_stored_grouping_toggle_is_ignored() {
        let stored = Settings::from_rows(&rows(&[("decks_grouped_by_archetype", "false")]));
        assert_eq!(stored, Settings::default());
    }

    #[test]
    fn trust_is_seeded_and_the_images_folder_and_flag_are_not() {
        let legacy = r#"{"local_images":true,"card_stats_trust":"200","card_images_path":"D:/Q/mymtgo-card-images"}"#;
        let seeded = Settings::from_rows(&rows(&[("legacy_0x_settings", legacy)]));
        assert_eq!(seeded.card_stats_trust, 200);
        assert_eq!(
            seeded.card_images_path, None,
            "v1 keeps its own folder (spec ruling 3)"
        );

        // The first import writes the rows v1 left at their defaults.
        let written = legacy_seeded_rows(&Settings::default().to_rows(), legacy);
        assert_eq!(written, rows(&[("card_stats_trust", "200")]));
        // A choice of v1's own is kept.
        let own = Settings {
            card_stats_trust: 10,
            ..Settings::default()
        };
        assert_eq!(legacy_seeded_rows(&own.to_rows(), legacy), rows(&[]));
        // An old local_images row of v1's own is ignored.
        let old = Settings::from_rows(&rows(&[("local_images", "true")]));
        assert_eq!(old, Settings::default());
    }

    #[test]
    fn the_trust_threshold_is_clamped_as_0x_reads_it() {
        for (stored, expected) in [
            ("0", 0),
            ("75", 75),
            ("-5", 0),
            ("250000", 100_000),
            ("\"12\"", 12),
            ("12.7", 12),
            ("\"x\"", 50),
            ("true", 50),
        ] {
            let read = Settings::from_rows(&rows(&[("card_stats_trust", stored)]));
            assert_eq!(read.card_stats_trust, expected, "{stored}");
            let legacy = format!("{{\"card_stats_trust\":{stored}}}");
            let seeded = Settings::from_rows(&rows(&[("legacy_0x_settings", &legacy)]));
            assert_eq!(seeded.card_stats_trust, expected, "0.x {stored}");
        }
        let saved = Settings {
            card_stats_trust: 1_000_000,
            ..Settings::default()
        }
        .cleaned();
        assert_eq!(saved.card_stats_trust, 100_000);
        let blank = Settings::from_rows(&rows(&[("card_images_path", "\"  \"")]));
        assert_eq!(blank.card_images_path, None);
    }

    #[test]
    fn a_later_import_leaves_deck_settings_alone() {
        // After the first import the user chose 12 per page (the default);
        // running the import again must not bring back 0.x's 24.
        let legacy = r#"{"decks_per_page":24}"#;
        let mut stored = Settings::default().to_rows();
        stored.push((
            crate::legacy::LEGACY_LAST_IMPORT.to_string(),
            "{}".to_string(),
        ));
        assert!(legacy_seeded_rows(&stored, legacy).is_empty());
    }

    // 0.x GameOverlaySettingsTest "defaults the overlay off and every section
    // on" (and the other overlay defaults of spec section 4).
    #[test]
    fn defaults_the_overlay_off_and_every_section_on() {
        let settings = Settings::from_rows(&[]);
        assert!(!settings.game_overlay);
        assert!(settings.overlay_show_opponent);
        assert!(settings.overlay_show_draw_odds);
        assert!(settings.overlay_show_reveals);
        assert!(settings.overlay_show_sideboard);
        assert!(settings.draft_notes_window);
        assert!(!settings.league_window);
        assert_eq!(settings.overlay_artwork, None);
        assert!(!settings.overlay_publish);
    }

    // 0.x "inherits the overlay setting from the legacy deck window key".
    #[test]
    fn inherits_the_overlay_setting_from_the_legacy_deck_window_key() {
        let legacy = r#"{"deck_window":true}"#;
        let settings = Settings::from_rows(&rows(&[("legacy_0x_settings", legacy)]));
        assert!(settings.game_overlay);
    }

    // 0.x "inherits the overlay setting from the legacy opponent window key".
    #[test]
    fn inherits_the_overlay_setting_from_the_legacy_opponent_window_key() {
        let legacy = r#"{"opponent_window":1,"deck_window":false}"#;
        let settings = Settings::from_rows(&rows(&[("legacy_0x_settings", legacy)]));
        assert!(settings.game_overlay);
    }

    // 0.x "prefers an explicit game overlay setting over the legacy keys":
    // 0.x's own `game_overlay`, and v1's row.
    #[test]
    fn prefers_an_explicit_game_overlay_setting_over_the_legacy_keys() {
        let legacy = r#"{"deck_window":true,"game_overlay":false}"#;
        let settings = Settings::from_rows(&rows(&[("legacy_0x_settings", legacy)]));
        assert!(!settings.game_overlay);
        // A null `game_overlay` is no value of its own (0.x `!== null`).
        let null = r#"{"deck_window":true,"game_overlay":null}"#;
        assert!(Settings::from_rows(&rows(&[("legacy_0x_settings", null)])).game_overlay);

        let own = Settings::from_rows(&rows(&[
            ("game_overlay", "false"),
            ("legacy_0x_settings", r#"{"deck_window":true}"#),
        ]));
        assert!(!own.game_overlay);
    }

    // 0.x "persists section toggles".
    #[test]
    fn persists_section_toggles() {
        let saved = Settings {
            overlay_show_sideboard: false,
            ..Settings::default()
        };
        let read = Settings::from_rows(&saved.to_rows());
        assert!(!read.overlay_show_sideboard);
        assert!(read.overlay_show_draw_odds);
    }

    // 0.x OverlayPublishSettingsTest "defaults publishing off and persists the
    // toggle".
    #[test]
    fn defaults_publishing_off_and_persists_the_toggle() {
        assert!(!Settings::from_rows(&[]).overlay_publish);
        let saved = Settings {
            overlay_publish: true,
            ..Settings::default()
        };
        assert!(Settings::from_rows(&saved.to_rows()).overlay_publish);
    }

    /// `stored` with `row` replacing any row of the same key, as the
    /// writer's upsert does.
    fn upsert(stored: &mut Vec<(String, String)>, row: (String, String)) {
        stored.retain(|(key, _)| key != &row.0);
        stored.push(row);
    }

    // 0.x "round-trips the remote background and last published time".
    #[test]
    fn round_trips_the_remote_background_and_last_published_time() {
        let mut stored = Vec::new();
        assert_eq!(overlay_background_remote(&stored), None);
        assert_eq!(overlay_last_published_at(&stored), None);

        let remote = BackgroundRemote {
            url: "https://cdn.example/bg.png".to_string(),
            sha256: "abc".to_string(),
        };
        upsert(&mut stored, overlay_background_remote_row(Some(&remote)));
        upsert(
            &mut stored,
            overlay_last_published_at_row(Some(1_790_000_000_000)),
        );
        assert_eq!(overlay_background_remote(&stored), Some(remote));
        assert_eq!(overlay_last_published_at(&stored), Some(1_790_000_000_000));
        assert_eq!(
            overlay_background_remote_row(None),
            ("overlay_background_remote".to_string(), "null".to_string())
        );

        upsert(&mut stored, overlay_background_remote_row(None));
        assert_eq!(overlay_background_remote(&stored), None);
        // A row without both parts reads as none (0.x `isset`).
        let partial = rows(&[("overlay_background_remote", r#"{"url":"https://q"}"#)]);
        assert_eq!(overlay_background_remote(&partial), None);
    }

    // 0.x "round-trips the publish failure reason".
    #[test]
    fn round_trips_the_publish_failure_reason() {
        let mut stored = Vec::new();
        assert_eq!(overlay_publish_error(&stored), None);
        upsert(
            &mut stored,
            overlay_publish_error_row(Some(PublishError::NotClaimed)),
        );
        assert_eq!(
            stored,
            rows(&[("overlay_publish_error", "\"not_claimed\"")])
        );
        assert_eq!(
            overlay_publish_error(&stored),
            Some(PublishError::NotClaimed)
        );
        upsert(&mut stored, overlay_publish_error_row(None));
        assert_eq!(overlay_publish_error(&stored), None);
        let unknown = rows(&[("overlay_publish_error", "\"boom\"")]);
        assert_eq!(overlay_publish_error(&unknown), None);
    }

    // 0.x ResolveOverlayArtTest "defaults the size to full and stores
    // compact".
    #[test]
    fn defaults_the_size_to_full_and_stores_compact() {
        assert_eq!(Settings::from_rows(&[]).overlay_size, OverlaySize::Full);
        let saved = Settings {
            overlay_size: OverlaySize::Compact,
            ..Settings::default()
        };
        assert_eq!(
            Settings::from_rows(&saved.to_rows()).overlay_size,
            OverlaySize::Compact
        );
        // 0.x: anything but `compact` is full; an unknown artwork is unset.
        let odd = Settings::from_rows(&rows(&[
            ("overlay_size", "\"huge\""),
            ("overlay_artwork", "\"photo\""),
        ]));
        assert_eq!(odd.overlay_size, OverlaySize::Full);
        assert_eq!(odd.overlay_artwork, None);
    }

    #[test]
    fn the_overlay_settings_are_seeded_from_the_kept_0x_settings() {
        let legacy = r#"{"draft_notes_window":false,"league_window":true,
            "overlay_show_opponent":false,"overlay_show_draw_odds":0,
            "overlay_show_reveals":false,"overlay_show_sideboard":false,
            "overlay_artwork":"custom","overlay_size":"compact","overlay_publish":true,
            "overlay_collapsed":true,"overlay_expanded_height":"420"}"#;
        let stored = rows(&[("legacy_0x_settings", legacy)]);
        let seeded = Settings::from_rows(&stored);
        assert!(!seeded.game_overlay);
        assert!(!seeded.draft_notes_window);
        assert!(seeded.league_window);
        assert!(!seeded.overlay_show_opponent);
        assert!(!seeded.overlay_show_draw_odds);
        assert!(!seeded.overlay_show_reveals);
        assert!(!seeded.overlay_show_sideboard);
        assert_eq!(seeded.overlay_artwork, Some(OverlayArtwork::Custom));
        assert_eq!(seeded.overlay_size, OverlaySize::Compact);
        assert!(seeded.overlay_publish);
        // The UI state rows too, until v1 writes its own.
        assert!(overlay_collapsed(&stored));
        assert_eq!(overlay_expanded_height(&stored), Some(420));
        let mut own = stored.clone();
        own.push(overlay_collapsed_row(false));
        own.push(overlay_expanded_height_row(None));
        assert!(!overlay_collapsed(&own));
        assert_eq!(overlay_expanded_height(&own), None);

        // The first import writes the rows v1 left at their defaults.
        let written = legacy_seeded_rows(&Settings::default().to_rows(), legacy);
        assert_eq!(
            written,
            rows(&[
                ("draft_notes_window", "false"),
                ("league_window", "true"),
                ("overlay_show_opponent", "false"),
                ("overlay_show_draw_odds", "false"),
                ("overlay_show_reveals", "false"),
                ("overlay_show_sideboard", "false"),
                ("overlay_artwork", "\"custom\""),
                ("overlay_size", "\"compact\""),
                ("overlay_publish", "true"),
            ])
        );
        let windows = legacy_seeded_rows(&Settings::default().to_rows(), r#"{"deck_window":true}"#);
        assert_eq!(windows, rows(&[("game_overlay", "true")]));
    }

    #[test]
    fn the_overlay_ui_rows_round_trip() {
        let mut stored = Vec::new();
        assert!(!overlay_collapsed(&stored));
        assert_eq!(overlay_expanded_height(&stored), None);
        assert_eq!(overlay_background_file(&stored), None);
        upsert(&mut stored, overlay_collapsed_row(true));
        upsert(&mut stored, overlay_expanded_height_row(Some(512)));
        upsert(
            &mut stored,
            overlay_background_file_row(Some("background-1790000000000.png")),
        );
        assert_eq!(
            stored,
            rows(&[
                ("overlay_collapsed", "true"),
                ("overlay_expanded_height", "512"),
                (
                    "overlay_background_file",
                    "\"background-1790000000000.png\""
                ),
            ])
        );
        assert!(overlay_collapsed(&stored));
        assert_eq!(overlay_expanded_height(&stored), Some(512));
        assert_eq!(
            overlay_background_file(&stored).as_deref(),
            Some("background-1790000000000.png")
        );
        upsert(&mut stored, overlay_background_file_row(None));
        assert_eq!(overlay_background_file(&stored), None);
        // Unreadable values take the defaults.
        let odd = rows(&[
            ("overlay_collapsed", "\"yes\""),
            ("overlay_expanded_height", "\"tall\""),
        ]);
        assert!(!overlay_collapsed(&odd));
        assert_eq!(overlay_expanded_height(&odd), None);
    }

    #[test]
    fn the_league_card_display_defaults_on_and_round_trips() {
        assert_eq!(
            Settings::from_rows(&[]).overlay_league_display,
            LeagueDisplay::default()
        );
        let saved = Settings {
            overlay_league_display: LeagueDisplay {
                games: overlay::LeagueGames::Letters,
                title: false,
                ..LeagueDisplay::default()
            },
            ..Settings::default()
        };
        assert_eq!(Settings::from_rows(&saved.to_rows()), saved);
    }

    #[test]
    fn fills_missing_display_fields_with_defaults() {
        let read = Settings::from_rows(&rows(&[("overlay_league_display", r#"{"record":false}"#)]));
        assert!(!read.overlay_league_display.record);
        assert!(read.overlay_league_display.title);
        // A row stored before the standing switch existed reads it as off.
        assert!(!read.overlay_league_display.standing);
        let standing =
            Settings::from_rows(&rows(&[("overlay_league_display", r#"{"standing":true}"#)]));
        assert!(standing.overlay_league_display.standing);
        assert!(standing.overlay_league_display.record);
        let junk = Settings::from_rows(&rows(&[("overlay_league_display", "\"nope\"")]));
        assert_eq!(junk.overlay_league_display, LeagueDisplay::default());
    }

    #[test]
    fn a_0x_import_never_writes_the_display_row() {
        let legacy = r#"{"overlay_league_display":{"record":false},"overlay_size":"compact"}"#;
        let written = legacy_seeded_rows(&Settings::default().to_rows(), legacy);
        assert!(
            written
                .iter()
                .all(|(key, _)| key != "overlay_league_display")
        );
        assert!(written.iter().any(|(key, _)| key == "overlay_size"));
    }

    #[test]
    fn the_finished_minutes_default_to_five_round_trip_and_are_clamped() {
        assert_eq!(Settings::from_rows(&[]).overlay_finished_minutes, 5);
        let saved = Settings {
            overlay_finished_minutes: 2,
            ..Settings::default()
        };
        assert_eq!(Settings::from_rows(&saved.to_rows()), saved);
        let read = |value| {
            Settings::from_rows(&rows(&[("overlay_finished_minutes", value)]))
                .overlay_finished_minutes
        };
        assert_eq!(read("0"), 1);
        assert_eq!(read("600"), 60);
        assert_eq!(read("\"nope\""), 5);
        let typed = Settings {
            overlay_finished_minutes: 0,
            ..Settings::default()
        };
        assert_eq!(typed.cleaned().overlay_finished_minutes, 1);
    }

    #[test]
    fn the_league_theme_round_trips_and_defaults_to_classic() {
        assert_eq!(
            Settings::from_rows(&[]).overlay_league_theme,
            LeagueTheme::Classic
        );
        let saved = Settings {
            overlay_league_theme: LeagueTheme::Versus,
            ..Settings::default()
        };
        assert_eq!(
            Settings::from_rows(&saved.to_rows()).overlay_league_theme,
            LeagueTheme::Versus
        );
        let newer = Settings::from_rows(&rows(&[("overlay_league_theme", "\"pixel\"")]));
        assert_eq!(
            newer.overlay_league_theme,
            LeagueTheme::Classic,
            "a newer build's theme"
        );
    }

    #[test]
    fn the_event_strip_defaults_on_and_round_trips() {
        assert!(Settings::from_rows(&[]).overlay_event_strip);
        let off = Settings {
            overlay_event_strip: false,
            ..Settings::default()
        };
        assert_eq!(Settings::from_rows(&off.to_rows()), off);
        let junk = Settings::from_rows(&rows(&[("overlay_event_strip", "\"no\"")]));
        assert!(junk.overlay_event_strip);
        let legacy = r#"{"overlay_event_strip":false}"#;
        let written = legacy_seeded_rows(&Settings::default().to_rows(), legacy);
        assert!(written.iter().all(|(key, _)| key != "overlay_event_strip"));
    }

    #[test]
    fn the_league_theme_is_not_seeded_from_0x() {
        let legacy = r#"{"overlay_league_theme":"versus","overlay_size":"compact"}"#;
        let written = legacy_seeded_rows(&Settings::default().to_rows(), legacy);
        assert!(written.iter().all(|(key, _)| key != "overlay_league_theme"));
    }

    #[test]
    fn the_thumbnail_artwork_round_trips() {
        let saved = Settings {
            overlay_artwork: Some(OverlayArtwork::Thumbnail),
            ..Settings::default()
        };
        assert_eq!(
            Settings::from_rows(&saved.to_rows()).overlay_artwork,
            Some(OverlayArtwork::Thumbnail)
        );
    }

    #[test]
    fn a_settings_save_keeps_the_platform_keys() {
        use crate::commands::tests::ready_state;
        use fixture_tools::TestDir;

        let dir = TestDir::new("settings-keeps-platform");
        let (state, _events) = ready_state(&dir);
        crate::platform::tests::seed(&state, true, true);
        crate::platform::switch(&state, crate::platform::PlatformView::Arena).expect("platform");
        crate::accounts::switch(&state, "Qarena#12345", 5_000).expect("arena switch");
        let read = |key: &str| -> Option<String> {
            let conn = crate::bridge::lock(&state.reader);
            conn.query_row("SELECT value FROM settings WHERE key = ?1", [key], |row| {
                row.get(0)
            })
            .ok()
        };
        let switch_before = read(store::accounts::ACTIVE_ARENA_ACCOUNT);
        assert!(switch_before.is_some());
        crate::commands::apply_settings(&state, Settings::default()).expect("saved");
        assert_eq!(
            store::platform::stored_platform(&crate::bridge::lock(&state.reader)).expect("read"),
            Some(store::Platform::Arena)
        );
        assert_eq!(read(store::accounts::ACTIVE_ARENA_ACCOUNT), switch_before);
        crate::bridge::lock(&state.tracker)
            .take()
            .expect("tracker")
            .stop();
    }
}
