//! Sideboard guides and matchup notes on the deck tab and the archetype
//! page (archetypes spec section 21.4, survey section 13): the deck's guide
//! list, one guide's editor, the archetype page's block and its deck
//! select, and the guide and note edits. Every write goes through the
//! tracker's `ArchetypeEdit`; the strings are 0.x's.
//!
//! A guide or note edit changes no match or deck the tracker announces, so
//! each edit here pushes `GuidesChanged` with its pair once committed: the
//! deck tab, the archetype page's block and the overlay (#8) refetch.

use archetypes::guides::{ArchetypeNotes, EditorRow, GuideCardInput, Note, PlanError};
use chrono::TimeZone;
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};
use store::Connection;
use store::archetype_edit::{ArchetypeEdit, ArchetypeEditOutcome};
use store::guides::{ArchetypeGuideRow, GuideDeckOption, GuideSummary, PlannedCard};
use tauri::{AppHandle, Manager, State};
use tauri_specta::Event;

use crate::bridge::lock;
use crate::commands::{AppState, CommandError, Startup, off_async_workers, ready, storage};
use crate::events::GuidesChanged;
use crate::logging::Timer;
use crate::summary::archetype_colors;

// ---- Views -------------------------------------------------------------------

/// One row of a deck's guide list, and the editor's header (0.x
/// `SideboardGuideSummaryData`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct GuideSummaryView {
    #[specta(type = specta_typescript::Number)]
    pub id: i64,
    #[specta(type = specta_typescript::Number)]
    pub deck_id: i64,
    #[specta(type = specta_typescript::Number)]
    pub archetype_id: i64,
    pub archetype_name: String,
    /// Colour identity letters in WUBRG order.
    pub colors: Vec<String>,
    pub retired: bool,
    /// Homebrew or Rogue.
    pub fallback: bool,
    /// Planned copies per direction.
    pub cards_in: u32,
    pub cards_out: u32,
    pub notes_count: u32,
    #[specta(type = specta_typescript::Number)]
    pub updated_at_ms: i64,
    /// "9 Mar 2027", local time (0.x "d MMM yyyy").
    pub updated_label: String,
    /// The deck's all-time matches against the archetype.
    pub matches: u32,
    /// "W - L", `null` when never faced.
    pub match_record: Option<String>,
    pub match_winrate: Option<u32>,
    pub game_winrate: Option<u32>,
}

/// One matchup note (0.x `ArchetypeNoteData`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct NoteView {
    #[specta(type = specta_typescript::Number)]
    pub id: i64,
    #[specta(type = specta_typescript::Number)]
    pub deck_id: i64,
    pub deck_name: String,
    pub body: String,
    #[specta(type = specta_typescript::Number)]
    pub created_at_ms: i64,
    /// "9 Mar 2027, 14:05", local time (0.x "d MMM yyyy, HH:mm").
    pub created_label: String,
}

/// An archetype's notes: this deck's and the other decks', newest first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct NotesView {
    pub current: Vec<NoteView>,
    pub other: Vec<NoteView>,
}

/// One card row of an editor column (0.x `SideboardCardData`,
/// `SidedOutCardData`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct EditorRowView {
    pub oracle_id: String,
    pub name: String,
    pub type_line: Option<String>,
    pub image: Option<String>,
    pub local_image: Option<String>,
    /// Copies in the zone (the stepper's ceiling); 0 on a stale row.
    pub quantity: u32,
    /// The plan's copies, `null` when the plan does not list the card.
    pub planned: Option<u32>,
    /// Planned but no longer in this zone of the latest version.
    pub stale: bool,
    pub sided_in_games: u32,
    pub wins: u32,
    pub losses: u32,
    pub winrate: Option<u32>,
    pub sided_out_games: u32,
}

/// The editor's columns over the deck's latest version (0.x
/// `SideboardGuideData`, Editor scope).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct GuideColumnsView {
    pub bring_in: Vec<EditorRowView>,
    pub take_out: Vec<EditorRowView>,
    pub postboard_games: u32,
    /// "W - L".
    pub postboard_record: String,
    pub has_plan: bool,
}

/// The guide editor page (0.x `EditController`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct GuideEditorView {
    pub guide: GuideSummaryView,
    pub has_version: bool,
    /// Copies in the latest version's sideboard (the balance chip).
    pub sideboard_size: u32,
    /// `null` without a version.
    pub sideboard: Option<GuideColumnsView>,
    pub notes: NotesView,
}

/// A planned card named, for the archetype page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct PlannedCardView {
    pub oracle_id: String,
    pub name: String,
    pub quantity: u32,
}

/// One deck's card in the archetype page's block (spec section 21.4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ArchetypeGuideView {
    #[specta(type = specta_typescript::Number)]
    pub deck_id: i64,
    pub deck_name: String,
    pub deck_archived: bool,
    /// `null` when the deck has only notes.
    #[specta(type = Option<specta_typescript::Number>)]
    pub guide_id: Option<i64>,
    pub cards_in: u32,
    pub cards_out: u32,
    pub planned_in: Vec<PlannedCardView>,
    pub planned_out: Vec<PlannedCardView>,
    pub matches: u32,
    pub match_record: Option<String>,
    pub match_winrate: Option<u32>,
    pub game_winrate: Option<u32>,
    pub notes: Vec<NoteView>,
}

/// A deck the archetype page's "Add note" and "Create guide" offer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct GuideDeckOptionView {
    #[specta(type = specta_typescript::Number)]
    pub deck_id: i64,
    pub name: String,
    /// Matches against the archetype.
    pub matches: u32,
}

/// One planned card the editor sends (0.x `cards.*`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct GuideCardInputView {
    pub oracle_id: String,
    /// `in` or `out`.
    pub direction: String,
    #[specta(type = specta_typescript::Number)]
    pub quantity: i64,
}

/// One refused entry of a plan: its index, field and 0.x's message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct PlanErrorView {
    pub index: u32,
    pub field: String,
    pub message: String,
}

// ---- Reads -------------------------------------------------------------------

/// `ms` as local time in `tz`, formatted.
fn local(ms: i64, tz: Tz, pattern: &str) -> String {
    match tz.timestamp_millis_opt(ms).single() {
        Some(time) => time.format(pattern).to_string(),
        None => String::new(),
    }
}

/// "9 Mar 2027" in `tz`.
pub fn day_label(ms: i64, tz: Tz) -> String {
    local(ms, tz, "%-d %b %Y")
}

/// "9 Mar 2027, 14:05" in `tz`.
pub fn time_label(ms: i64, tz: Tz) -> String {
    local(ms, tz, "%-d %b %Y, %H:%M")
}

pub fn summary_view(summary: &GuideSummary, tz: Tz) -> GuideSummaryView {
    GuideSummaryView {
        id: summary.id,
        deck_id: summary.deck_id,
        archetype_id: summary.archetype_id,
        archetype_name: summary.archetype_name.clone(),
        colors: archetype_colors(summary.archetype_color_identity.as_deref()),
        retired: summary.archetype_retired,
        fallback: summary.archetype_fallback,
        cards_in: summary.cards_in,
        cards_out: summary.cards_out,
        notes_count: summary.notes_count,
        updated_at_ms: summary.updated_at,
        updated_label: day_label(summary.updated_at, tz),
        matches: summary.matchup.matches,
        match_record: summary.matchup.match_record.clone(),
        match_winrate: summary.matchup.match_winrate,
        game_winrate: summary.matchup.game_winrate,
    }
}

fn note_view(note: &Note, tz: Tz) -> NoteView {
    NoteView {
        id: note.id,
        deck_id: note.deck_id,
        deck_name: note.deck_name.clone(),
        body: note.body.clone(),
        created_at_ms: note.created_at,
        created_label: time_label(note.created_at, tz),
    }
}

fn notes_view(notes: &ArchetypeNotes, tz: Tz) -> NotesView {
    NotesView {
        current: notes
            .current
            .iter()
            .map(|note| note_view(note, tz))
            .collect(),
        other: notes.other.iter().map(|note| note_view(note, tz)).collect(),
    }
}

fn row_view(row: &EditorRow) -> EditorRowView {
    EditorRowView {
        oracle_id: row.oracle_id.clone(),
        name: row.name.clone(),
        type_line: row.card_type.clone(),
        image: row.image.clone(),
        local_image: row.local_image.clone(),
        quantity: row.quantity,
        planned: row.planned,
        stale: row.stale,
        sided_in_games: row.sided_in_games,
        wins: row.wins,
        losses: row.losses,
        winrate: row.winrate,
        sided_out_games: row.sided_out_games,
    }
}

fn planned_view(card: &PlannedCard) -> PlannedCardView {
    PlannedCardView {
        oracle_id: card.oracle_id.clone(),
        name: card.name.clone(),
        quantity: card.quantity,
    }
}

fn archetype_row_view(row: &ArchetypeGuideRow, tz: Tz) -> ArchetypeGuideView {
    ArchetypeGuideView {
        deck_id: row.deck_id,
        deck_name: row.deck_name.clone(),
        deck_archived: row.deck_archived,
        guide_id: row.guide_id,
        cards_in: row.cards_in,
        cards_out: row.cards_out,
        planned_in: row.planned_in.iter().map(planned_view).collect(),
        planned_out: row.planned_out.iter().map(planned_view).collect(),
        matches: row.matchup.matches,
        match_record: row.matchup.match_record.clone(),
        match_winrate: row.matchup.match_winrate,
        game_winrate: row.matchup.game_winrate,
        notes: row.notes.iter().map(|note| note_view(note, tz)).collect(),
    }
}

fn option_view(option: &GuideDeckOption) -> GuideDeckOptionView {
    GuideDeckOptionView {
        deck_id: option.deck_id,
        name: option.name.clone(),
        matches: option.matches,
    }
}

/// A deck's guide list (0.x `SideboardGuidesController`); archived decks
/// included. The create dialog's archetypes are a separate read
/// (`archetype_options` for the deck's format), asked after first paint,
/// as 0.x deferred them.
pub fn list_view(
    conn: &Connection,
    deck_id: i64,
    tz: Tz,
) -> Result<Vec<GuideSummaryView>, CommandError> {
    let guides = store::guides::deck_guides(conn, deck_id).map_err(storage)?;
    Ok(guides.iter().map(|guide| summary_view(guide, tz)).collect())
}

/// The guide editor (0.x `EditController`); `None` for a missing guide or
/// a guide of another deck (0.x `scopeBindings`' 404).
pub fn editor_view(
    conn: &Connection,
    deck_id: i64,
    guide_id: i64,
    tz: Tz,
) -> Result<Option<GuideEditorView>, CommandError> {
    let Some(page) = store::guides::guide_editor(conn, guide_id).map_err(storage)? else {
        return Ok(None);
    };
    if page.summary.deck_id != deck_id {
        return Ok(None);
    }
    let sideboard = page.editor.as_ref().map(|editor| GuideColumnsView {
        bring_in: editor.bring_in.iter().map(row_view).collect(),
        take_out: editor.take_out.iter().map(row_view).collect(),
        postboard_games: editor.postboard_games,
        postboard_record: editor.postboard_record.clone(),
        has_plan: editor.has_plan,
    });
    Ok(Some(GuideEditorView {
        guide: summary_view(&page.summary, tz),
        has_version: page.has_version,
        sideboard_size: page.sideboard_size,
        sideboard,
        notes: notes_view(&page.notes, tz),
    }))
}

/// The archetype page's block.
pub fn archetype_view(
    conn: &Connection,
    archetype_id: i64,
    tz: Tz,
) -> Result<Vec<ArchetypeGuideView>, CommandError> {
    let scope = crate::platform::page_scope(conn).map_err(storage)?;
    let rows =
        store::guides::archetype_guides(conn, scope.platform, archetype_id).map_err(storage)?;
    Ok(rows.iter().map(|row| archetype_row_view(row, tz)).collect())
}

/// The archetype page's deck select, for the current account.
pub fn deck_options_view(
    conn: &Connection,
    archetype_id: i64,
) -> Result<Vec<GuideDeckOptionView>, CommandError> {
    let scope = crate::platform::page_scope(conn).map_err(storage)?;
    let options =
        store::guides::guide_deck_options(conn, scope.account, scope.platform, archetype_id)
            .map_err(storage)?;
    Ok(options.iter().map(option_view).collect())
}

// ---- Edits -------------------------------------------------------------------

/// Apply a guide or note edit through the tracker; a refusal is the user's
/// message. A rejected plan comes back as the outcome, for its entries.
fn send(state: &AppState, edit: ArchetypeEdit) -> Result<ArchetypeEditOutcome, CommandError> {
    let outcome = crate::matches::archetype_editor(state)?
        .edit(edit)
        .map_err(CommandError::Tracker)?;
    match outcome {
        ArchetypeEditOutcome::PlanRejected(_) => Ok(outcome),
        _ => match outcome.refusal() {
            Some(message) => Err(CommandError::Refused(message.to_string())),
            None => Ok(outcome),
        },
    }
}

fn changed(deck_id: i64, archetype_id: i64) -> GuidesChanged {
    GuidesChanged {
        deck_id,
        archetype_id,
    }
}

/// 0.x's 404s as v1 refusals: the edit itself refuses them too, but the
/// pair to announce is read first.
fn guide_of(state: &AppState, guide_id: i64) -> Result<(i64, i64), CommandError> {
    store::guides::guide_pair(&lock(&state.reader), guide_id)
        .map_err(storage)?
        .ok_or_else(|| CommandError::Refused(store::archetype_edit::GUIDE_NOT_FOUND.to_string()))
}

/// Create the deck's guide against the archetype; answers it and the pair
/// to announce.
pub fn create(
    state: &AppState,
    deck_id: i64,
    archetype_id: i64,
    now_ms: i64,
) -> Result<(i64, GuidesChanged), CommandError> {
    let outcome = send(
        state,
        ArchetypeEdit::CreateGuide {
            deck_id,
            archetype_id,
            now_ms,
        },
    )?;
    match outcome {
        ArchetypeEditOutcome::Guide { guide_id } => Ok((guide_id, changed(deck_id, archetype_id))),
        _ => Err(CommandError::Tracker(
            "the guide was not created".to_string(),
        )),
    }
}

fn error_view(error: &PlanError) -> PlanErrorView {
    PlanErrorView {
        index: u32::try_from(error.index).unwrap_or(u32::MAX),
        field: error.field.clone(),
        message: error.message.clone(),
    }
}

/// Save the guide's whole plan: the refused entries (nothing saved), empty
/// when saved, and the pair to announce once saved.
pub fn save_cards(
    state: &AppState,
    deck_id: i64,
    guide_id: i64,
    cards: Vec<GuideCardInputView>,
    now_ms: i64,
) -> Result<(Vec<PlanErrorView>, Option<GuidesChanged>), CommandError> {
    let (guide_deck, archetype_id) = guide_of(state, guide_id)?;
    let cards = cards
        .into_iter()
        .map(|card| GuideCardInput {
            oracle_id: card.oracle_id,
            direction: card.direction,
            quantity: card.quantity,
        })
        .collect();
    let outcome = send(
        state,
        ArchetypeEdit::SaveGuideCards {
            deck_id,
            guide_id,
            cards,
            now_ms,
        },
    )?;
    match outcome {
        ArchetypeEditOutcome::PlanRejected(errors) => {
            Ok((errors.iter().map(error_view).collect(), None))
        }
        _ => Ok((Vec::new(), Some(changed(guide_deck, archetype_id)))),
    }
}

/// Delete the guide and the pair's notes.
pub fn delete(
    state: &AppState,
    deck_id: i64,
    guide_id: i64,
) -> Result<GuidesChanged, CommandError> {
    let (guide_deck, archetype_id) = guide_of(state, guide_id)?;
    send(state, ArchetypeEdit::DeleteGuide { deck_id, guide_id })?;
    Ok(changed(guide_deck, archetype_id))
}

/// Add a note for the pair; answers the note's id.
pub fn add_one_note(
    state: &AppState,
    deck_id: i64,
    archetype_id: i64,
    body: String,
    now_ms: i64,
) -> Result<(i64, GuidesChanged), CommandError> {
    let outcome = send(
        state,
        ArchetypeEdit::AddNote {
            deck_id,
            archetype_id,
            body,
            now_ms,
        },
    )?;
    match outcome {
        ArchetypeEditOutcome::Note { note_id, .. } => Ok((note_id, changed(deck_id, archetype_id))),
        _ => Err(CommandError::Tracker("the note was not added".to_string())),
    }
}

/// Delete one note.
pub fn delete_one_note(state: &AppState, note_id: i64) -> Result<GuidesChanged, CommandError> {
    let (deck_id, archetype_id) = store::guides::note_pair(&lock(&state.reader), note_id)
        .map_err(storage)?
        .ok_or_else(|| CommandError::Refused(store::archetype_edit::NOTE_NOT_FOUND.to_string()))?;
    send(state, ArchetypeEdit::DeleteNote { note_id })?;
    Ok(changed(deck_id, archetype_id))
}

/// Push `GuidesChanged` once an edit committed, and tell the overlay
/// worker (the event is an app event the bridge never sees: ruling C6).
pub(crate) fn announce(app: &AppHandle, event: GuidesChanged) {
    crate::overlay::signal(
        app,
        crate::overlay::worker::OverlayMsg::GuidesChanged {
            deck_id: event.deck_id,
            archetype_id: event.archetype_id,
        },
    );
    if let Err(error) = event.emit(app) {
        tracing::warn!(%error, "could not push the guides change");
    }
}

// ---- Commands ----------------------------------------------------------------

/// A deck's Sideboard Guides tab.
#[tauri::command]
#[specta::specta]
pub fn deck_guides(
    deck_id: u32,
    startup: State<'_, Startup>,
) -> Result<Vec<GuideSummaryView>, CommandError> {
    let _timer = Timer::start("deck_guides");
    let state = ready(&startup)?;
    list_view(&lock(&state.reader), i64::from(deck_id), state.timezone)
}

/// One guide's editor; `null` for a missing guide or another deck's.
#[tauri::command]
#[specta::specta]
pub fn guide_editor(
    deck_id: u32,
    guide_id: u32,
    startup: State<'_, Startup>,
) -> Result<Option<GuideEditorView>, CommandError> {
    let _timer = Timer::start("guide_editor");
    let state = ready(&startup)?;
    editor_view(
        &lock(&state.reader),
        i64::from(deck_id),
        i64::from(guide_id),
        state.timezone,
    )
}

/// The archetype page's guides and notes block.
#[tauri::command]
#[specta::specta]
pub fn archetype_guides(
    archetype_id: u32,
    startup: State<'_, Startup>,
) -> Result<Vec<ArchetypeGuideView>, CommandError> {
    let _timer = Timer::start("archetype_guides");
    let state = ready(&startup)?;
    archetype_view(
        &lock(&state.reader),
        i64::from(archetype_id),
        state.timezone,
    )
}

/// The archetype page's deck select: decks of the archetype's format, the
/// most played against it first.
#[tauri::command]
#[specta::specta]
pub fn guide_deck_options(
    archetype_id: u32,
    startup: State<'_, Startup>,
) -> Result<Vec<GuideDeckOptionView>, CommandError> {
    let state = ready(&startup)?;
    deck_options_view(&lock(&state.reader), i64::from(archetype_id))
}

/// Create a guide (0.x `StoreController`); answers its id. Async, as every
/// edit: it waits for the tracker's commit.
#[tauri::command]
#[specta::specta]
pub async fn create_guide(
    app: AppHandle,
    deck_id: u32,
    archetype_id: u32,
) -> Result<u32, CommandError> {
    let _timer = Timer::start("create_guide");
    off_async_workers(move || {
        let startup = app.state::<Startup>();
        let (guide_id, event) = create(
            ready(&startup)?,
            i64::from(deck_id),
            i64::from(archetype_id),
            crate::wall_clock_ms(),
        )?;
        announce(&app, event);
        u32::try_from(guide_id).map_err(|_| CommandError::Storage("guide id".to_string()))
    })
    .await
}

/// Save a guide's whole plan (0.x `UpdateCardsController`): the refused
/// entries with 0.x's messages (nothing saved), empty once saved.
#[tauri::command]
#[specta::specta]
pub async fn save_guide_cards(
    app: AppHandle,
    deck_id: u32,
    guide_id: u32,
    cards: Vec<GuideCardInputView>,
) -> Result<Vec<PlanErrorView>, CommandError> {
    let _timer = Timer::start("save_guide_cards");
    off_async_workers(move || {
        let startup = app.state::<Startup>();
        let (errors, event) = save_cards(
            ready(&startup)?,
            i64::from(deck_id),
            i64::from(guide_id),
            cards,
            crate::wall_clock_ms(),
        )?;
        if let Some(event) = event {
            announce(&app, event);
        }
        Ok(errors)
    })
    .await
}

/// Delete a guide and its pair's notes (0.x `DestroyController`).
#[tauri::command]
#[specta::specta]
pub async fn delete_guide(app: AppHandle, deck_id: u32, guide_id: u32) -> Result<(), CommandError> {
    let _timer = Timer::start("delete_guide");
    off_async_workers(move || {
        let startup = app.state::<Startup>();
        let event = delete(ready(&startup)?, i64::from(deck_id), i64::from(guide_id))?;
        announce(&app, event);
        Ok(())
    })
    .await
}

/// Add a matchup note (0.x `StoreNoteController`); answers its id.
#[tauri::command]
#[specta::specta]
pub async fn add_note(
    app: AppHandle,
    deck_id: u32,
    archetype_id: u32,
    body: String,
) -> Result<u32, CommandError> {
    let _timer = Timer::start("add_note");
    off_async_workers(move || {
        let startup = app.state::<Startup>();
        let (note_id, event) = add_one_note(
            ready(&startup)?,
            i64::from(deck_id),
            i64::from(archetype_id),
            body,
            crate::wall_clock_ms(),
        )?;
        announce(&app, event);
        u32::try_from(note_id).map_err(|_| CommandError::Storage("note id".to_string()))
    })
    .await
}

/// Delete one note (0.x `DestroyNoteController`).
#[tauri::command]
#[specta::specta]
pub async fn delete_note(app: AppHandle, note_id: u32) -> Result<(), CommandError> {
    let _timer = Timer::start("delete_note");
    off_async_workers(move || {
        let startup = app.state::<Startup>();
        let event = delete_one_note(ready(&startup)?, i64::from(note_id))?;
        announce(&app, event);
        Ok(())
    })
    .await
}

#[cfg(test)]
mod tests;
