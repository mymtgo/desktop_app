//! Archetypes (sub-project 09): the `archetypes` worker thread and the
//! detection commands.
//!
//! - `worker`: the thread that runs every archetype job and API call, one
//!   at a time (the detection queue, "Detect", the deck banner, deck
//!   prefills, missing definitions).
//! - `detect`: one match's and one deck's detection (archetypes spec
//!   section 6).
//! - `definitions`: the definitions check, downloading one archetype's
//!   definition, and the 2 s pacing of every archetype request.
//! - `catalog`: the server's archetypes in the picker, and holding one
//!   picked (#489).
//! - `metagame`: the Metagame page and the archetype detail; `cache` keeps
//!   the API's global shares and live decklists in memory.

pub mod cache;
pub mod catalog;
pub mod definitions;
pub mod detect;
pub mod manage;
pub mod metagame;
pub mod offline;
pub mod worker;

use store::archetypes::DeckFilter;
use tauri::{AppHandle, Manager};

use crate::archetypes::worker::{ArchetypeSignal, ask};
use crate::bridge::lock;
use crate::commands::{CommandError, Startup, off_async_workers, ready, storage};

/// The deck banner's filter as 0.x's `TriggerArchetypeDetectionRequest`
/// validates it: required, `none` or a numeric id (`all` is refused).
pub fn parse_filter(text: &str) -> Result<DeckFilter, String> {
    if text.is_empty() {
        return Err("The filter archetype field is required.".to_string());
    }
    if text == "none" {
        return Ok(DeckFilter::NoOpponentArchetype);
    }
    if text.bytes().all(|byte| byte.is_ascii_digit())
        && let Ok(id) = text.parse::<i64>()
    {
        return Ok(DeckFilter::Opponent(id));
    }
    Err(r#"The archetype filter must be "none" or a numeric id."#.to_string())
}

/// The deck "Detect archetypes" banner: the deck's complete matches under
/// the opponent archetype filter (`none` or an id) are queued for
/// detection; answers how many (0.x `QueueArchetypeDetectionForDeck`).
#[tauri::command]
#[specta::specta]
pub async fn detect_deck_archetypes(
    app: AppHandle,
    deck_id: u32,
    filter: String,
) -> Result<u32, CommandError> {
    let deck_id = i64::from(deck_id);
    off_async_workers(move || {
        let filter = parse_filter(&filter).map_err(CommandError::Refused)?;
        let archived = {
            let startup = app.state::<Startup>();
            let state = ready(&startup)?;
            store::decks::deck_by_id(&lock(&state.reader), deck_id)
                .map_err(storage)?
                .is_some_and(|deck| deck.archived_at.is_some())
        };
        if archived {
            return Err(CommandError::Refused(
                store::decks::ARCHIVED_DECK.to_string(),
            ));
        }
        ask(&app, |reply| ArchetypeSignal::DetectDeck {
            deck_id,
            filter,
            reply,
        })
        .map_err(CommandError::Refused)
    })
    .await
}

/// The deck's complete matches waiting for detection (0.x
/// `pendingArchetypeCount`).
#[tauri::command]
#[specta::specta]
pub async fn pending_archetype_count(app: AppHandle, deck_id: u32) -> Result<u32, CommandError> {
    let deck_id = i64::from(deck_id);
    off_async_workers(move || {
        let startup = app.state::<Startup>();
        let state = ready(&startup)?;
        store::archetypes::pending_archetype_count(&lock(&state.reader), deck_id).map_err(storage)
    })
    .await
}

/// Whether a downloaded archetype is held (0.x `hasArchetypeCatalog`): the
/// offline mode card warns without one.
#[tauri::command]
#[specta::specta]
pub async fn has_archetype_definitions(app: AppHandle) -> Result<bool, CommandError> {
    off_async_workers(move || {
        let startup = app.state::<Startup>();
        let state = ready(&startup)?;
        store::archetypes::has_downloaded_definition(&lock(&state.reader)).map_err(storage)
    })
    .await
}

/// Whether archetypes are held from the offline download: the offline mode
/// card offers "Run detection" only then.
#[tauri::command]
#[specta::specta]
pub async fn has_offline_archetypes(app: AppHandle) -> Result<bool, CommandError> {
    off_async_workers(move || {
        let startup = app.state::<Startup>();
        let state = ready(&startup)?;
        store::archetypes::has_offline_held(&lock(&state.reader)).map_err(storage)
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    // 0.x TriggerArchetypeDetectionTest "rejects request without
    // filter_archetype" and "rejects request with filter_archetype=all".
    #[test]
    fn the_deck_filter_is_none_or_an_id() {
        assert_eq!(parse_filter("none"), Ok(DeckFilter::NoOpponentArchetype));
        assert_eq!(parse_filter("42"), Ok(DeckFilter::Opponent(42)));
        assert_eq!(
            parse_filter(""),
            Err("The filter archetype field is required.".to_string())
        );
        for refused in ["all", "-1", "4a", " 4"] {
            assert_eq!(
                parse_filter(refused),
                Err(r#"The archetype filter must be "none" or a numeric id."#.to_string()),
                "{refused}"
            );
        }
    }
}
