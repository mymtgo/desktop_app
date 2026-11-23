//! The league overlay window (overlay spec 9; 0.x `release_0.46.0`
//! `leagues/Overlay.vue`, `BuildOverlayState`, `UpdateOverlayDeckLabel`).
//!
//! - The card: `league_card` reads the current account's runs
//!   (`store::overlay::league_card_inputs`) and builds 0.x's state with the
//!   pure `overlay::league_state`, the look coming from the settings
//!   (`overlay_size`, `overlay_artwork`) and the custom background, when
//!   its file exists.
//! - Its page reads the card with `league_overlay`, then applies the
//!   worker's `LeagueOverlayChanged` pushes (nothing polls). The worker
//!   recomputes the card on every trigger of spec section 6 and pushes it
//!   only when it changed: `LeaguesChanged` (told after the league sync's
//!   commit: a league match completing, a game created or settled, a match
//!   linked or unlinked), a `MatchUpdated` of a league match (its commit
//!   may follow, so the `LeaguesChanged` after it reads again), live events
//!   of the card's live match (sideboarding), deck changes (the label
//!   included), settings saves and the background.
//! - The full card's title edits the deck's label with
//!   `set_overlay_deck_label` (the tracker's deck edit).
//!
//! The worker runs from start-up whenever the app has its database (it is
//! started with tracking), so the card is computed and pushed to the main
//! window even when overlay windows are unavailable (native Wayland); the
//! page's own read does not need the worker.

use std::collections::BTreeSet;
use std::path::Path;

use overlay::{
    CardLook, LeagueCardInput, LeagueOverlayState, changes_at, finished_ms, league_state,
    live_tokens, shown_live_match_token,
};
use store::{Connection, StoreError};
use tauri::{AppHandle, Manager, State};

use crate::bridge::lock;
use crate::commands::{
    AppState, CommandError, Startup, off_async_workers, read_settings, ready, storage,
};
use crate::decks::{edit_result, send_edit};
use crate::logging::Timer;
use crate::overlay::OverlayWorker;
use crate::overlay::background::{background_file, background_url, scheme_base};
use crate::overlay::game::{KnownOpponent, card_opponent_archetype};
use crate::overlay::game_net::GameNet;
use crate::settings::{Settings, overlay_background_file};

/// The league card, and what the worker needs to keep it current.
#[derive(Debug, Clone, PartialEq)]
pub struct LeagueCard {
    pub state: LeagueOverlayState,
    /// The live (pending or in progress) matches of the runs and
    /// tournaments read: their live events can change the card
    /// (sideboarding).
    pub live_tokens: BTreeSet<String>,
    /// When the card changes with no new data: a finished run's or
    /// tournament's time on the card (`overlay_finished_minutes`) ends, or a
    /// tournament goes stale or never paired (the worker wakes).
    pub changes_at_ms: Option<i64>,
}

/// What the card looks like: the settings, the custom background when its
/// file exists in `overlay_folder`, and the platform's `card` scheme base.
pub fn card_look(
    settings: &Settings,
    rows: &[(String, String)],
    overlay_folder: Option<&Path>,
) -> CardLook {
    let background_url = overlay_folder.and_then(|folder| {
        let name = overlay_background_file(rows)?;
        background_file(folder, Some(&name))?;
        Some(background_url(&name))
    });
    CardLook {
        size: settings.overlay_size,
        artwork: settings.overlay_artwork,
        display: settings.overlay_league_display,
        theme: settings.overlay_league_theme,
        background_url,
        card_file_base: scheme_base(crate::cards::scheme::SCHEME),
        deck_cover_base: scheme_base(crate::decks::cover::SCHEME),
        strip: settings.overlay_event_strip,
    }
}

/// The league card at `now_ms` for the current account (the local player
/// of the newest log; with no account, the idle card). The live match
/// names the opponent's archetype: the game overlay's answer for it
/// (`known`) or what `net`'s local sources hold, never a network lookup
/// (`card_opponent_archetype`). A caller with no game overlay state passes
/// `None` and a fresh `GameNet`.
pub fn league_card(
    conn: &Connection,
    settings: &Settings,
    overlay_folder: Option<&Path>,
    now_ms: i64,
    known: Option<&KnownOpponent>,
    net: &mut GameNet,
) -> Result<LeagueCard, StoreError> {
    let rows = store::read::settings(conn)?;
    let look = card_look(settings, &rows, overlay_folder);
    let finished = finished_ms(settings.overlay_finished_minutes);
    let mut input = match store::accounts::current_account_id(conn, store::Platform::Mtgo)? {
        Some(account) => store::overlay::league_card_inputs(conn, account, now_ms, finished)?,
        None => LeagueCardInput {
            runs: Vec::new(),
            last_played_deck: store::overlay::last_played_deck(conn)?,
            ..LeagueCardInput::default()
        },
    };
    drop_missing_covers(&mut input, overlay_folder);
    let live_tokens = live_tokens(&input);
    let changes_at_ms = changes_at(&input, now_ms, finished);
    let shown_token = shown_live_match_token(&input, now_ms, finished);
    let mut state = league_state(input, now_ms, finished, &look);
    if let (Some(live), Some(token)) = (state.live_match.as_mut(), shown_token) {
        live.opponent_archetype = card_opponent_archetype(conn, &token, known, now_ms, net)
            .unwrap_or_else(|error| {
                tracing::warn!(%error, "could not resolve the live opponent's archetype");
                None
            });
    }
    Ok(LeagueCard {
        state,
        live_tokens,
        changes_at_ms,
    })
}

/// Whether the card has the other event strip below it (event overlay spec
/// 8): the league window grows by the strip's height while it does. The
/// `overlay_event_strip` switch needs no check here: with it off, the card
/// carries no other event (spec 7).
pub fn strip_showing(state: &LeagueOverlayState) -> bool {
    state.other.is_some()
}

/// Clear each deck's own cover (runs', tournaments' and the last played
/// deck's) whose file is not in the deck covers folder
/// (beside `overlay_folder`, both in the app data folder), so the card
/// shows the cover card's art instead of a broken image, as the custom
/// background does. With no folder, no cover file is shown.
fn drop_missing_covers(input: &mut LeagueCardInput, overlay_folder: Option<&Path>) {
    let covers = overlay_folder
        .and_then(Path::parent)
        .map(crate::decks::cover::folder);
    let runs = input.runs.iter_mut().filter_map(|run| run.deck.as_mut());
    let tournaments = input.tournaments.iter_mut().flat_map(|tournament| {
        let matches = tournament
            .matches
            .iter_mut()
            .filter_map(|played| played.deck.as_mut());
        matches
            .chain(tournament.registered_deck.as_mut())
            .chain(tournament.format_deck.as_mut())
    });
    let decks = runs
        .chain(tournaments)
        .chain(input.last_played_deck.as_mut());
    for deck in decks {
        let exists = covers.as_deref().is_some_and(|folder| {
            crate::decks::cover::cover_file(folder, deck.cover_file.as_deref()).is_some()
        });
        if !exists {
            deck.cover_file = None;
        }
    }
}

/// The league card for the running app (the page's first read).
pub fn current_card(
    state: &AppState,
    known: Option<&KnownOpponent>,
    now_ms: i64,
) -> Result<LeagueOverlayState, CommandError> {
    let settings = read_settings(state)?;
    let folder = crate::overlay::background::folder(&state.data_dir);
    let card = league_card(
        &lock(&state.reader),
        &settings,
        Some(&folder),
        now_ms,
        known,
        &mut GameNet::default(),
    )
    .map_err(storage)?;
    Ok(card.state)
}

/// Save the deck's label for the card: trimmed, at most 40 characters
/// kept, blank clears; an unknown deck is refused.
pub fn set_label(
    state: &AppState,
    deck_id: i64,
    label: Option<String>,
) -> Result<(), CommandError> {
    edit_result(send_edit(
        state,
        store::decks::DeckEdit::SetOverlayLabel { deck_id, label },
    )?)
}

/// The league card now (the league window's first read).
#[tauri::command]
#[specta::specta]
pub fn league_overlay(
    app: AppHandle,
    startup: State<'_, Startup>,
) -> Result<LeagueOverlayState, CommandError> {
    let _timer = Timer::start("league_overlay");
    let known = known_opponent(&app);
    current_card(ready(&startup)?, known.as_ref(), crate::wall_clock_ms())
}

/// The archetype the game overlay last named for its live match, for the
/// cards read off the `overlay` thread (the page's first read, the hosted
/// publish).
pub fn known_opponent(app: &AppHandle) -> Option<KnownOpponent> {
    app.try_state::<OverlayWorker>()
        .and_then(|worker| worker.known_opponent())
}

/// The streamer's name for a deck on the league card. The tracker's
/// `DecksChanged` after the edit makes the worker push the card again.
/// Async so the wait for the tracker's commit never blocks the window.
#[tauri::command]
#[specta::specta]
pub async fn set_overlay_deck_label(
    app: AppHandle,
    deck_id: u32,
    label: Option<String>,
) -> Result<(), CommandError> {
    let _timer = Timer::start("set_overlay_deck_label");
    off_async_workers(move || {
        let startup = app.state::<Startup>();
        set_label(ready(&startup)?, i64::from(deck_id), label)
    })
    .await
}

#[cfg(test)]
mod tests;
