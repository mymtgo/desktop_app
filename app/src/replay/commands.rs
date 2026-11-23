//! The replay window's view and the share commands. Each runs inside
//! `spawn_blocking` (the frames' read, the network and the tracker's commit
//! block), reads with its own read-only connection, and answers a failure
//! as the message the window shows.

use tauri::{AppHandle, Manager};

use crate::api::session::ApiState;
use crate::commands::{AppState, Startup, ready};
use crate::logging::Timer;
use crate::replay::share::{ReplayShareState, ShareContext, revoke, share, share_state};
use crate::replay::view::{ReplayWindowView, replay_window_view};

/// Run `work` off the async runtime's threads, with the app's state.
async fn blocking<T: Send + 'static>(
    app: AppHandle,
    work: impl FnOnce(&AppHandle, &AppState) -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    let joined = tauri::async_runtime::spawn_blocking(move || {
        let startup = app.state::<Startup>();
        let state = ready(&startup).map_err(|error| error.message().to_string())?;
        work(&app, state)
    })
    .await;
    match joined {
        Ok(result) => result,
        Err(error) => Err(format!("the replay share stopped unexpectedly: {error}")),
    }
}

/// A fresh read-only connection.
fn read_only(state: &AppState) -> Result<store::Connection, String> {
    store::open_reader(&state.db_path).map_err(|error| error.to_string())
}

/// The replay window's view of a game: an MTGO game's frames, log, the
/// match's games, sideboards, archetypes and share state, or an Arena or
/// Endstep game's `.mtgr` record; `null` when the game is gone or has no
/// record to show.
#[tauri::command]
#[specta::specta]
pub async fn replay_view(app: AppHandle, game_id: u32) -> Result<Option<ReplayWindowView>, String> {
    let _timer = Timer::start("replay_view");
    blocking(app, move |app, state| {
        let conn = read_only(state)?;
        let game_id = i64::from(game_id);
        let share = || share_state(&app.state::<ApiState>(), &conn, game_id);
        let dat = |token: &str| match crate::matches::match_dat_entries(state, token) {
            Ok(entries) => entries,
            Err(error) => {
                tracing::warn!(?error, "the match's game log file could not be read");
                Vec::new()
            }
        };
        // Never waits: the tokens' art comes on a later open, once the
        // lookup lands.
        let queue_tokens = |tokens| {
            let queued = state
                .tracker_part(tracker::TrackerHandle::card_stub_scanner)
                .is_ok_and(|scanner| scanner.request_tokens(tokens));
            if !queued {
                tracing::warn!(
                    "the tracker is busy; the replay's tokens are asked about next time"
                );
            }
        };
        replay_window_view(&conn, game_id, state.timezone, &dat, &share, &queue_tokens)
    })
    .await
}

/// The share dialog's state for a game: signed in, supporter, and the link
/// opening on this game.
#[tauri::command]
#[specta::specta]
pub async fn replay_share_state(app: AppHandle, game_id: u32) -> Result<ReplayShareState, String> {
    let _timer = Timer::start("replay_share_state");
    blocking(app, move |app, state| {
        let conn = read_only(state)?;
        share_state(&app.state::<ApiState>(), &conn, i64::from(game_id))
    })
    .await
}

/// Share the game's match on mymtgo.com ("Create link", "Update link").
#[tauri::command]
#[specta::specta]
pub async fn share_replay(app: AppHandle, game_id: u32) -> Result<ReplayShareState, String> {
    let _timer = Timer::start("share_replay");
    blocking(app, move |app, state| {
        let editor = state
            .tracker_part(tracker::TrackerHandle::match_editor)
            .map_err(|error| error.message().to_string())?;
        let conn = read_only(state)?;
        let ctx = ShareContext {
            api: &app.state::<ApiState>(),
            editor: &editor,
            conn: &conn,
            timezone: state.timezone,
            now_ms: crate::wall_clock_ms(),
        };
        let dat = |token: &str| match crate::matches::match_dat_entries(state, token) {
            Ok(entries) => entries,
            Err(error) => {
                tracing::warn!(?error, "the match's game log file could not be read");
                Vec::new()
            }
        };
        share(&ctx, i64::from(game_id), &dat)
    })
    .await
}

/// Switch the game's match link off ("Switch off link").
#[tauri::command]
#[specta::specta]
pub async fn revoke_replay(app: AppHandle, game_id: u32) -> Result<ReplayShareState, String> {
    let _timer = Timer::start("revoke_replay");
    blocking(app, move |app, state| {
        let editor = state
            .tracker_part(tracker::TrackerHandle::match_editor)
            .map_err(|error| error.message().to_string())?;
        let conn = read_only(state)?;
        let ctx = ShareContext {
            api: &app.state::<ApiState>(),
            editor: &editor,
            conn: &conn,
            timezone: state.timezone,
            now_ms: crate::wall_clock_ms(),
        };
        revoke(&ctx, i64::from(game_id))
    })
    .await
}
