//! The Overlays settings page's read (overlay spec 11; 0.x
//! `release_0.46.0` `OverlaysController`): what the page shows beside the
//! settings themselves (which it reads with `get_settings`).
//!
//! - The preview card's real last played deck and its art, the custom
//!   background and the artwork mode in force (read time, as 0.x).
//! - The hosted OBS overlay: the public URL of the signed-in MTGO account,
//!   the browser source size of each card size, the last publish and its
//!   error (later answers come as `OverlayPublishChanged` pushes), and
//!   whether this device is signed in to MyMTGO.
//! - Whether overlay windows can open in this session (false on native
//!   Wayland, spec 5.3): the page then shows the three window cards as
//!   unavailable.
//!
//! The save side effects (windows opened or closed, publishing switched on
//! or off, the league window resized) are the overlay worker's, on the
//! `SettingsChanged` message `save_settings` sends (ruling C7).
//!
//! The game overlay's commands (overlay spec 7): its first read, fit and
//! collapse (the window work is the overlay worker's, never a synchronous
//! command's), the archetype picker and pick, and the matchup notes. The
//! live match a pick or a note lands on is the one the overlay worker
//! computed the view for, never one the request names (0.x).

use std::path::Path;

use overlay::{
    CardLook, LeagueCardInput, LeagueDeck, OverlayArtwork, OverlaySize, art_mode, finished_ms,
    league_state, obs_size_with_strip, public_url,
};
use serde::Serialize;
use store::{Connection, StoreError};
use tauri::{AppHandle, Manager};

use store::archetype_edit::ArchetypeEdit;

use crate::api::session::ApiState;
use crate::archetypes::cache::{ArchetypeCaches, Cached, metagame_key};
use crate::archetypes::worker::ArchetypeSignal;
use crate::bridge::lock;
use crate::commands::{AppState, CommandError, Startup, off_async_workers, ready, storage};
use crate::logging::Timer;
use crate::overlay::OverlayWorker;
use crate::overlay::events::GameOverlayChanged;
use crate::overlay::events::OverlayPublishChanged;
use crate::overlay::game::LiveContext;
use crate::overlay::game_net::PICKER_DAYS;
use crate::overlay::league::card_look;
use crate::overlay::publish::publish_status;
use crate::overlay::worker::{GameSlot, OverlayMsg};
use crate::settings::Settings;
use crate::window::WindowSupport;

/// An OBS browser source size, in pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, specta::Type)]
pub struct ObsSize {
    pub width: u32,
    pub height: u32,
}

/// The browser source size for each card size, for the current theme (0.x
/// `overlayObsSizes`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, specta::Type)]
pub struct ObsSizes {
    pub full: ObsSize,
    pub compact: ObsSize,
}

/// What the Overlays settings page shows besides the settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct OverlaySettingsView {
    /// The last played deck, swapped into the preview's sample states so
    /// its label can be edited (0.x `overlayDeck`).
    pub deck: Option<LeagueDeck>,
    /// The last played deck's cover art crop as the window shows it: the
    /// downloaded file when there is one, else Scryfall's.
    pub deck_art_url: Option<String>,
    /// The custom background, when its file exists.
    pub background_url: Option<String>,
    /// The artwork mode in force: the setting, else `custom` when a
    /// background exists, else `deck` (0.x `ResolveOverlayArt::mode`).
    pub artwork: OverlayArtwork,
    /// The OBS browser source URL of the signed-in MTGO account, full size
    /// (the page adds `?size=compact`); `None` before any login.
    pub public_url: Option<String>,
    pub obs_sizes: ObsSizes,
    /// The last publish and why it failed.
    pub publish: OverlayPublishChanged,
    /// This device is signed in to MyMTGO: publishing can be switched on.
    pub signed_in: bool,
    /// Overlay windows can open in this session (false on native Wayland).
    pub overlay_windows_available: bool,
}

/// What the page's read takes from outside the database.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageContext {
    /// The MyMTGO API base URL; `None` when the build has no valid one.
    pub api_base: Option<String>,
    pub signed_in: bool,
    pub overlay_windows_available: bool,
}

/// The page's view from the stored rows (0.x `OverlaysController`).
pub fn settings_view(
    conn: &Connection,
    overlay_folder: Option<&Path>,
    context: &PageContext,
    now_ms: i64,
) -> Result<OverlaySettingsView, StoreError> {
    let rows = store::read::settings(conn)?;
    let settings = Settings::from_rows(&rows);
    let look = card_look(&settings, &rows, overlay_folder);
    // The idle card of the last played deck in deck mode is its deck block
    // and its window art (the downloaded art crop, else Scryfall's).
    let deck_look = CardLook {
        artwork: Some(OverlayArtwork::Deck),
        background_url: None,
        ..look.clone()
    };
    let idle = league_state(
        LeagueCardInput {
            runs: Vec::new(),
            last_played_deck: store::overlay::last_played_deck(conn)?,
            ..LeagueCardInput::default()
        },
        now_ms,
        finished_ms(settings.overlay_finished_minutes),
        &deck_look,
    );
    let username = store::overlay::current_login(conn)?.map(|(username, _)| username);
    let public_url = match (context.api_base.as_deref(), username) {
        (Some(base), Some(username)) => Some(public_url(base, &username, OverlaySize::Full)),
        _ => None,
    };
    Ok(OverlaySettingsView {
        deck: idle.deck,
        deck_art_url: idle.art.map(|art| art.url),
        artwork: art_mode(settings.overlay_artwork, look.background_url.is_some()),
        background_url: look.background_url,
        public_url,
        obs_sizes: ObsSizes {
            full: size(&settings, OverlaySize::Full),
            compact: size(&settings, OverlaySize::Compact),
        },
        publish: publish_status(&rows),
        signed_in: context.signed_in,
        overlay_windows_available: context.overlay_windows_available,
    })
}

/// The OBS browser source size for the theme, with room for the other
/// event strip while its switch is on (event overlay spec 8.2, 8.4).
fn size(settings: &Settings, size: OverlaySize) -> ObsSize {
    let (width, height) = obs_size_with_strip(
        settings.overlay_league_theme,
        size,
        settings.overlay_event_strip,
    );
    ObsSize { width, height }
}

/// The Overlays settings page's read. Async: asking whether the device is
/// signed in may open the secret store.
#[tauri::command]
#[specta::specta]
pub async fn overlay_settings_page(app: AppHandle) -> Result<OverlaySettingsView, CommandError> {
    let _timer = Timer::start("overlay_settings_page");
    off_async_workers(move || {
        let startup = app.state::<Startup>();
        let state = ready(&startup)?;
        let api = app.state::<ApiState>();
        let context = PageContext {
            api_base: api.config().ok().map(|config| config.base_url.clone()),
            signed_in: api.session().linked(),
            overlay_windows_available: app
                .try_state::<WindowSupport>()
                .is_some_and(|support| support.overlay_windows_available),
        };
        let folder = crate::overlay::background::folder(&state.data_dir);
        settings_view(
            &lock(&state.reader),
            Some(&folder),
            &context,
            crate::wall_clock_ms(),
        )
        .map_err(storage)
    })
    .await
}

// ---- The game overlay ----------------------------------------------------------

/// The page's measures (0.x `FitGameOverlayWindowController` rules).
pub const FIXED_HEIGHT_MAX: i32 = 2000;
pub const BAR_HEIGHT_MAX: i32 = 200;

/// A measure within 0 to `max`, else Laravel's message for `field`.
pub fn checked_measure(value: i32, field: &str, max: i32) -> Result<u32, CommandError> {
    if value < 0 {
        return Err(CommandError::Refused(format!(
            "The {field} field must be at least 0."
        )));
    }
    if value > max {
        return Err(CommandError::Refused(format!(
            "The {field} field must not be greater than {max}."
        )));
    }
    Ok(value as u32)
}

/// The fit message for the page's measures (0.x `bar_height` nullable).
pub fn fit_message(fixed_height: i32, bar_height: Option<i32>) -> Result<OverlayMsg, CommandError> {
    Ok(OverlayMsg::FitGame {
        fixed_height: checked_measure(fixed_height, "fixed height", FIXED_HEIGHT_MAX)?,
        bar_height: checked_measure(bar_height.unwrap_or(0), "bar height", BAR_HEIGHT_MAX)?,
    })
}

/// The collapse message (0.x every field required).
pub fn collapse_message(
    collapsed: bool,
    fixed_height: i32,
    bar_height: i32,
) -> Result<OverlayMsg, CommandError> {
    Ok(OverlayMsg::CollapseGame {
        collapsed,
        fixed_height: checked_measure(fixed_height, "fixed height", FIXED_HEIGHT_MAX)?,
        bar_height: checked_measure(bar_height, "bar height", BAR_HEIGHT_MAX)?,
    })
}

/// The game overlay's view as last pushed, else the view without a match.
pub fn first_view(slot: &GameSlot, rows: &[(String, String)]) -> GameOverlayChanged {
    match &slot.latest {
        Some(latest) => latest.clone(),
        None => GameOverlayChanged {
            revision: 0,
            view: crate::overlay::game::empty_view(
                &Settings::from_rows(rows),
                crate::settings::overlay_collapsed(rows),
            ),
        },
    }
}

/// One archetype the overlay's picker offers: a held one (with its id), or
/// one of the format's 90-day metagame that v1 has never met (owner
/// answer 3), picked by uuid.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct OverlayArchetypeOption {
    #[specta(type = Option<specta_typescript::Number>)]
    pub id: Option<i64>,
    pub uuid: String,
    pub name: String,
    /// The archetypes format key (`modern`); `null` for a fallback.
    pub format: Option<String>,
    /// Colour identity letters in WUBRG order.
    pub colors: Vec<String>,
    /// Homebrew or Rogue.
    pub is_fallback: bool,
}

/// What the picker chose.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, specta::Type)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum OverlayArchetypeChoice {
    Held {
        id: u32,
    },
    Unheld {
        uuid: String,
        name: String,
        #[serde(default)]
        colors: Option<String>,
    },
}

/// The picker's archetypes for a match format code (spec 7.3): the
/// format's held ones (not retired, by name) and, when `shares` holds the
/// format's metagame, the ones v1 does not hold, by name; then the
/// fallbacks.
pub fn archetype_options(
    conn: &Connection,
    format: Option<&str>,
    shares: Option<&mymtgo_api::MetagameShares>,
) -> Result<Vec<OverlayArchetypeOption>, StoreError> {
    let key = format.and_then(archetypes::format_key);
    let held = store::archetypes::archetype_options(conn, key.as_deref())?;
    let mut regular: Vec<OverlayArchetypeOption> = Vec::new();
    let mut fallbacks: Vec<OverlayArchetypeOption> = Vec::new();
    for option in held {
        let fallback = option.source == archetypes::Source::Fallback;
        let view = OverlayArchetypeOption {
            id: Some(option.id),
            uuid: option.uuid,
            name: option.name,
            format: option.format,
            colors: crate::summary::archetype_colors(option.color_identity.as_deref()),
            is_fallback: fallback,
        };
        if fallback {
            fallbacks.push(view);
        } else {
            regular.push(view);
        }
    }
    if let Some(shares) = shares {
        for share in &shares.archetypes {
            if store::archetypes::held_uuid(conn, &share.uuid)?.is_some() {
                continue;
            }
            regular.push(OverlayArchetypeOption {
                id: None,
                uuid: share.uuid.clone(),
                name: share.name.clone(),
                format: key.clone(),
                colors: crate::summary::archetype_colors(share.color_identity.as_deref()),
                is_fallback: false,
            });
        }
        regular.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.uuid.cmp(&b.uuid)));
    }
    regular.extend(fallbacks);
    Ok(regular)
}

/// 0.x's message for an archetype that does not exist.
pub const UNKNOWN_ARCHETYPE: &str = "The selected archetype id is invalid.";

/// The opponent's archetype picked on the overlay (0.x
/// `UpdateOpponentArchetypeController`): the live match's opponent seat
/// becomes the pick (one merge hop), hand-set at confidence 1. An archetype
/// v1 has never met becomes a stub first, which the archetypes worker
/// fills. No live match or no opponent: nothing happens. Answers whether a
/// stub was written.
pub fn pick_archetype(
    state: &AppState,
    live: Option<&LiveContext>,
    choice: OverlayArchetypeChoice,
    now_ms: i64,
) -> Result<bool, CommandError> {
    let held = match &choice {
        OverlayArchetypeChoice::Held { id } => {
            let id = i64::from(*id);
            let known = store::archetypes::archetype(&lock(&state.reader), id).map_err(storage)?;
            if known.is_none() {
                return Err(CommandError::Refused(UNKNOWN_ARCHETYPE.to_string()));
            }
            Some(id)
        }
        OverlayArchetypeChoice::Unheld { uuid, .. } => {
            store::archetypes::held_uuid(&lock(&state.reader), uuid)
                .map_err(storage)?
                .map(|row| row.id)
        }
    };
    let Some(live) = live.filter(|live| live.opponent_id.is_some()) else {
        return Ok(false);
    };
    let editor = crate::matches::archetype_editor(state)?;
    let mut stubbed = false;
    let archetype_id = match (held, choice) {
        (Some(id), _) => id,
        (None, OverlayArchetypeChoice::Unheld { uuid, name, colors }) => {
            let format = live
                .format
                .as_deref()
                .and_then(archetypes::format_key)
                .unwrap_or_default();
            editor
                .edit(ArchetypeEdit::EnsureStub {
                    uuid: uuid.clone(),
                    name,
                    format,
                    color_identity: colors,
                    now_ms,
                })
                .map_err(CommandError::Tracker)?;
            stubbed = true;
            store::archetypes::held_uuid(&lock(&state.reader), &uuid)
                .map_err(storage)?
                .map(|row| row.id)
                .ok_or_else(|| CommandError::Refused(UNKNOWN_ARCHETYPE.to_string()))?
        }
        (None, OverlayArchetypeChoice::Held { .. }) => {
            return Err(CommandError::Refused(UNKNOWN_ARCHETYPE.to_string()));
        }
    };
    let parent =
        store::archetypes::merged_into(&lock(&state.reader), archetype_id).map_err(storage)?;
    let archetype_id = archetypes::merge_hop(archetype_id, |_| parent);
    let outcome = editor
        .edit(ArchetypeEdit::SetOpponent {
            match_ids: vec![live.match_id],
            archetype_id: Some(archetype_id),
            now_ms,
        })
        .map_err(CommandError::Tracker)?;
    if let Some(message) = outcome.refusal() {
        return Err(CommandError::Refused(message.to_string()));
    }
    Ok(stubbed)
}

/// A matchup note from the overlay (0.x `StoreNoteController`): the body is
/// checked first (Plan T's rules and messages), then stored against the
/// live match's deck and its opponent's held archetype through Plan T's
/// `AddNote` (which makes the pair's empty guide first). Without a deck or
/// an archetype nothing is stored.
pub fn add_note(
    state: &AppState,
    live: Option<&LiveContext>,
    body: &str,
    now_ms: i64,
) -> Result<Option<crate::events::GuidesChanged>, CommandError> {
    let body = archetypes::guides::check_note_body(body)
        .map_err(|message| CommandError::Refused(message.to_string()))?;
    let Some((deck_id, archetype_id)) =
        live.and_then(|live| Some((live.deck_id?, live.archetype_id?)))
    else {
        return Ok(None);
    };
    let (_, changed) = crate::guides::add_one_note(state, deck_id, archetype_id, body, now_ms)?;
    Ok(Some(changed))
}

fn game_slot(app: &AppHandle) -> GameSlot {
    app.try_state::<OverlayWorker>()
        .map(|worker| worker.game_slot())
        .unwrap_or_default()
}

/// The game overlay page's first read: the latest push (with its revision),
/// else the view without a match.
#[tauri::command]
#[specta::specta]
pub async fn game_overlay(app: AppHandle) -> Result<GameOverlayChanged, CommandError> {
    off_async_workers(move || {
        let slot = game_slot(&app);
        let startup = app.state::<Startup>();
        let state = ready(&startup)?;
        let rows = store::read::settings(&lock(&state.reader)).map_err(storage)?;
        Ok(first_view(&slot, &rows))
    })
    .await
}

/// Fit the game overlay around its measured content (0.x
/// `FitGameOverlayWindowController`); the overlay worker resizes it.
#[tauri::command]
#[specta::specta]
pub fn fit_game_overlay(
    app: AppHandle,
    fixed_height: i32,
    bar_height: Option<i32>,
) -> Result<(), CommandError> {
    crate::overlay::signal(&app, fit_message(fixed_height, bar_height)?);
    Ok(())
}

/// Collapse the game overlay to its header and tab bar, or expand it (0.x
/// `CollapseGameOverlayWindowController`); stored even when it is closed.
#[tauri::command]
#[specta::specta]
pub fn set_game_overlay_collapsed(
    app: AppHandle,
    collapsed: bool,
    fixed_height: i32,
    bar_height: i32,
) -> Result<(), CommandError> {
    crate::overlay::signal(&app, collapse_message(collapsed, fixed_height, bar_height)?);
    Ok(())
}

/// The archetype picker's options for the live match's format. Online and
/// without the format's 90-day metagame in Plan T's cache, the archetypes
/// worker fetches it and `MetagameSharesReady` follows.
#[tauri::command]
#[specta::specta]
pub async fn overlay_archetype_options(
    app: AppHandle,
) -> Result<Vec<OverlayArchetypeOption>, CommandError> {
    off_async_workers(move || {
        let format = game_slot(&app).live.and_then(|live| live.format);
        let key = format.as_deref().and_then(archetypes::format_key);
        let online = app
            .try_state::<ApiState>()
            .is_some_and(|api| !api.offline_mode().0);
        let now = crate::wall_clock_ms();
        let mut shares = None;
        let mut fetch = None;
        if online
            && let Some(slug) = key.as_deref().and_then(mymtgo_api::format_slug)
            && let Some(caches) = app.try_state::<ArchetypeCaches>()
        {
            let cache_key = metagame_key(slug, PICKER_DAYS);
            let mut cache = lock(&caches.metagame);
            match cache.get(&cache_key, now) {
                Cached::Fresh(fresh) => shares = Some(fresh),
                Cached::Missing => {
                    cache.begin(&cache_key);
                    fetch = Some(slug.to_string());
                }
                Cached::Failed | Cached::Pending => {}
            }
        }
        if let Some(slug) = fetch {
            crate::archetypes::worker::signal(
                &app,
                ArchetypeSignal::FetchMetagame {
                    slug,
                    days: PICKER_DAYS,
                },
            );
        }
        let startup = app.state::<Startup>();
        let state = ready(&startup)?;
        archetype_options(&lock(&state.reader), format.as_deref(), shares.as_ref()).map_err(storage)
    })
    .await
}

/// Pick the live match opponent's archetype on the overlay.
#[tauri::command]
#[specta::specta]
pub async fn set_overlay_opponent_archetype(
    app: AppHandle,
    choice: OverlayArchetypeChoice,
) -> Result<(), CommandError> {
    off_async_workers(move || {
        let live = game_slot(&app).live;
        let startup = app.state::<Startup>();
        let state = ready(&startup)?;
        if pick_archetype(state, live.as_ref(), choice, crate::wall_clock_ms())? {
            crate::archetypes::worker::signal(&app, ArchetypeSignal::FetchDefinitions);
        }
        Ok(())
    })
    .await
}

/// Add a matchup note for the live match's deck and opponent archetype.
#[tauri::command]
#[specta::specta]
pub async fn add_overlay_note(app: AppHandle, body: String) -> Result<(), CommandError> {
    off_async_workers(move || {
        let live = game_slot(&app).live;
        let startup = app.state::<Startup>();
        let state = ready(&startup)?;
        if let Some(changed) = add_note(state, live.as_ref(), &body, crate::wall_clock_ms())? {
            crate::guides::announce(&app, changed);
        }
        Ok(())
    })
    .await
}

/// Delete a matchup note (0.x `DestroyNoteController`).
#[tauri::command]
#[specta::specta]
pub async fn delete_overlay_note(app: AppHandle, note_id: u32) -> Result<(), CommandError> {
    off_async_workers(move || {
        let startup = app.state::<Startup>();
        let state = ready(&startup)?;
        let changed = crate::guides::delete_one_note(state, i64::from(note_id))?;
        crate::guides::announce(&app, changed);
        Ok(())
    })
    .await
}

#[cfg(test)]
mod tests;
