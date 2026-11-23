//! Archetype writes (archetypes spec sections 4, 6 and 8). Every one runs
//! on the writer thread through the tracker's `Command::ArchetypeEdit`, one
//! transaction per edit; the manual match's seats are written inside the
//! manual edit's transaction (`write_manual_seat`, decision C7).
//!
//! Detection owns `manual = 0` rows and `matches.archetypes_detected_at`; a
//! `manual = 1` row and `decks.archetype_id` are input, changed only by the
//! player or by a server `moved` answer (spec ruling 10). The manual
//! archetype edits live in `manual` (T7), the sideboard guide and matchup
//! note edits in `guides` (T11, decision C4).

mod guides;
mod manual;

pub use guides::{DECK_NOT_FOUND, GUIDE_NOT_FOUND, NOTE_NOT_FOUND};
pub use manual::{ManualCard, NewManualArchetype};

use rusqlite::{OptionalExtension, Transaction, params};

use crate::{Connection, StoreError};

/// One seat's detected archetype.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SeatResult {
    pub archetype_id: i64,
    /// 0 to 1.
    pub confidence: f64,
}

/// Detection's answer for one match: `None` leaves the seat without an
/// auto row (a hand-set row always stays).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MatchDetection {
    pub match_id: i64,
    /// The deck archetype detection read. A result whose deck archetype
    /// changed since is stale and is dropped (the match stays due).
    pub deck_archetype_id: Option<i64>,
    /// A commander format match: the deck's archetype becomes this one (the
    /// player's commander archetype) when it is `Some`.
    pub deck_archetype: Option<i64>,
    pub player: Option<SeatResult>,
    pub opponent: Option<SeatResult>,
}

/// An archetype write.
#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::large_enum_variant)]
pub enum ArchetypeEdit {
    /// Detection results (archetypes spec section 6.2): each match's auto
    /// rows are replaced by the given seats, a seat with a hand-set row is
    /// left alone, and the match is marked detected at `now_ms`. A match
    /// that no longer exists, is no longer waiting for detection (the
    /// player chose "none" while the detection was in flight) or whose deck
    /// archetype changed since detection read it is skipped.
    SetDetections {
        results: Vec<MatchDetection>,
        now_ms: i64,
    },
    /// The archetype row of a commander set (Arena Brawl spec section 6,
    /// "Find or create"): the row with this uuid, followed through
    /// `merged_into_id` to the survivor, else a new `commander` row. A held
    /// row is never renamed or recoloured.
    EnsureCommander {
        uuid: String,
        name: String,
        format: String,
        /// The canonical stored form (`"B,G"`), `None` when colourless.
        color_identity: Option<String>,
        now_ms: i64,
    },
    /// Ask for detection again (the match page "Detect", a definition
    /// change): the marker is cleared, the rows stay until detection runs.
    ClearDetected { match_ids: Vec<i64> },
    /// The offline download's "Run detection" (2026-10-27 design section
    /// 3): every detected match is asked for detection again. Hand-set seats
    /// and the "no opponent archetype" choice survive it, as they do any
    /// re-detection.
    ClearAllDetected,
    /// The opponent archetype set by hand (0.x `UpdateArchetypeController`,
    /// `BulkUpdateArchetypeController`): a `manual = 1` row at confidence 1.
    /// `None` deletes the opponent row, records the choice
    /// (`matches.opponent_archetype_none`, input) and marks the match
    /// detected, so no re-detection refills it (spec ruling 12); a hand-set
    /// archetype clears the choice. Matches without an opponent are skipped.
    SetOpponent {
        match_ids: Vec<i64>,
        archetype_id: Option<i64>,
        now_ms: i64,
    },
    /// Decks' archetype set by hand or in bulk (0.x
    /// `BulkUpdateDeckArchetypeController`): the decks share a format, the
    /// archetype is of that format or a fallback, archived decks included,
    /// `None` clears. Each deck's matches whose player seat is not hand-set
    /// are detected again, so the player seat follows the deck.
    SetDeckArchetype {
        deck_ids: Vec<i64>,
        archetype_id: Option<i64>,
    },
    /// The deck sync's prefill (0.x `SyncDecks::prefillArchetype`): written
    /// only while the deck has no archetype. When it lands, the deck's
    /// matches whose player seat is not hand-set are detected again.
    PrefillDeck { deck_id: i64, archetype_id: i64 },
    /// An archetype detection needs (an API answer v1 does not hold, or a
    /// held row known only by uuid), written from its definition (spec
    /// section 5 "Getting one"): a new `downloaded` row, or the definition
    /// filled into a held downloaded row without one. A held definition is
    /// left for the change check; manual and fallback rows are never
    /// touched. `format` is used when the definition has none (a
    /// `/decklist` fallback definition holds only cards).
    EnsureDownloaded {
        definition: Box<archetypes::Definition>,
        format: String,
        now_ms: i64,
    },
    /// The change check's plan (spec section 5, `archetypes::plan_changes`),
    /// in this order:
    /// - `replace`: each definition replaces its downloaded row's name,
    ///   format, colours, definition, version and list 1 and clears
    ///   `retired_at`; an unheld uuid is added;
    /// - `moves`: every match row (hand-set ones too), deck link and merge
    ///   child of the old downloaded row moves to the target, then the old
    ///   row is deleted; nothing is detected again;
    /// - `retire`: `retired_at` is set; the matches keep the archetype;
    /// - `redetect`: matches with an auto row pointing at a replaced
    ///   archetype, or of a replaced format with an auto row below 1.0, are
    ///   asked for detection again (hand-set rows never are).
    ///
    /// Manual and fallback rows are never touched. `checked_at` records the
    /// check in the `archetypes_checked_at` settings row.
    ApplyDefinitions {
        plan: archetypes::ChangePlan,
        checked_at: Option<i64>,
        now_ms: i64,
    },
    /// The offline archetype download (2026-10-27 offline download design
    /// section 2), by uuid: an unheld one is added as a Custom (manual) row,
    /// a held downloaded one becomes a Custom row in place (its definition
    /// dropped, its key list replaced, its matches kept); both are marked
    /// `offline_held`. Manual rows (the player's own, or held offline
    /// already) and fallbacks are never written. Answers `OfflineHeld`.
    HoldOffline {
        definitions: Vec<archetypes::Definition>,
        now_ms: i64,
    },
    /// Going back online (2026-10-27 offline download design section 5):
    /// every `offline_held` row becomes a downloaded row known only by uuid
    /// (no definition, version or lists) for the definitions check to fill.
    /// Merges are kept: one may predate offline mode, and online a
    /// downloaded row may be merged either way. The player's other edits to
    /// those rows are dropped; their matches and decks keep them.
    ReleaseOffline { now_ms: i64 },
    /// A manual archetype made by the player (0.x `StoreManualArchetype`):
    /// list 1 from its cards and, with a source match, that match's
    /// opponent seat set to it by hand. The new row is found by its uuid.
    CreateManual(Box<NewManualArchetype>),
    /// A manual archetype's name, format and colours (0.x
    /// `UpdateArchetypeMeta`), and with `variant` a new list ("Save & Add
    /// Variant", 0.x `AddArchetypeVariant`). Downloaded names come from the
    /// server (spec ruling 3). A duplicate variant refuses the whole edit.
    UpdateManual {
        id: i64,
        name: String,
        format: String,
        color_identity: Option<String>,
        variant: Option<Vec<ManualCard>>,
        now_ms: i64,
    },
    /// Remove one of a manual archetype's lists (0.x
    /// `RemoveArchetypeVariant`); the last one stays.
    RemoveVariant { id: i64, list: i64 },
    /// Move a list to another manual archetype of the format (0.x
    /// `ReassignArchetypeVariant`): it becomes the target's next list, and a
    /// source left with none is merged into the target.
    ReassignVariant {
        id: i64,
        list: i64,
        target_id: i64,
        now_ms: i64,
    },
    /// The player's merge (0.x `MergeArchetype`): only `merged_into_id`
    /// changes; history stays on the source.
    Merge {
        source_id: i64,
        parent_id: i64,
        now_ms: i64,
    },
    /// Undo a merge (0.x `UnmergeArchetype`).
    Unmerge { source_id: i64, now_ms: i64 },
    /// Delete a manual archetype (0.x `DestroyController`): its match rows,
    /// sideboard guides and matchup notes go by cascade, its deck links and
    /// merge children are cleared. The matches it held are detected again
    /// (0.x left them without an archetype).
    DeleteManual { id: i64 },
    /// A sideboard guide for the deck against the archetype (0.x
    /// `Decks/SideboardGuides/StoreController`): `Guide` with its id, or
    /// refused when the pair has one (`GUIDE_EXISTS`).
    CreateGuide {
        deck_id: i64,
        archetype_id: i64,
        now_ms: i64,
    },
    /// The guide's whole plan (0.x `UpdateCardsController`): checked against
    /// the deck's latest version (`archetypes::guides::check_plan`), then
    /// replaced and the guide's `updated_at` set to `now_ms`. A guide of
    /// another deck is refused.
    SaveGuideCards {
        deck_id: i64,
        guide_id: i64,
        cards: Vec<archetypes::guides::GuideCardInput>,
        now_ms: i64,
    },
    /// Delete the guide, its cards and every note of the same pair (0.x
    /// `DestroyController`). A guide of another deck is refused.
    DeleteGuide { deck_id: i64, guide_id: i64 },
    /// A matchup note (0.x `StoreNoteController`): the body trimmed, 1 to
    /// 2000 characters; the pair's empty guide is made first when it has
    /// none (0.x's overlay note), so the note shows in the guide list.
    AddNote {
        deck_id: i64,
        archetype_id: i64,
        body: String,
        now_ms: i64,
    },
    /// Delete one note (0.x `DestroyNoteController`).
    DeleteNote { note_id: i64 },
    /// An archetype the API named (the overlay's scouting, estimate or an
    /// unmet pick, overlay spec 7.3) that v1 does not hold: a `downloaded`
    /// row known only by its uuid, named by the API, with no definition
    /// yet, so the archetypes worker's missing definitions download fills
    /// it (`undefined_uuids`). A held uuid is left alone.
    EnsureStub {
        uuid: String,
        name: String,
        /// The archetypes format key (`modern`).
        format: String,
        color_identity: Option<String>,
        now_ms: i64,
    },
    /// A held downloaded archetype the server lists as active, picked by
    /// the player while v1 holds it retired (a stale check, #489): its
    /// `retired_at` is cleared. Other sources are left alone.
    Reinstate { uuid: String, now_ms: i64 },
}

/// What happened to an `ArchetypeEdit`. Only `Done` changed anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArchetypeEditOutcome {
    /// Done: the matches whose archetype rows or marker changed and the
    /// decks whose archetype changed, by id.
    Done {
        matches: Vec<i64>,
        decks: Vec<i64>,
    },
    /// The archetype does not exist (Laravel's `exists` rule).
    UnknownArchetype,
    MixedFormats,
    WrongFormat,
    /// The archetype to edit does not exist (0.x's 404).
    NotFound,
    /// A manual archetype, guide or note edit refused by one of 0.x's
    /// guards, with its message.
    Refused(String),
    /// `CreateGuide` made this guide.
    Guide {
        guide_id: i64,
    },
    /// `AddNote` wrote this note on the pair's guide.
    Note {
        note_id: i64,
        guide_id: i64,
    },
    /// `SaveGuideCards` refused the plan, with each entry's error.
    PlanRejected(Vec<archetypes::guides::PlanError>),
    /// `EnsureCommander` found or made this archetype (the merge survivor).
    Commander {
        archetype_id: i64,
    },
    /// `HoldOffline` wrote these.
    OfflineHeld(OfflineHeld),
}

/// What one `HoldOffline` slice did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct OfflineHeld {
    /// New Custom rows.
    pub added: u32,
    /// Downloaded rows that became Custom rows.
    pub converted: u32,
    /// Already Custom or a fallback: left alone.
    pub skipped: u32,
}

impl ArchetypeEditOutcome {
    /// The message to show for a refusal (0.x's), `None` for `Done`.
    pub fn refusal(&self) -> Option<&str> {
        match self {
            ArchetypeEditOutcome::Done { .. }
            | ArchetypeEditOutcome::Commander { .. }
            | ArchetypeEditOutcome::OfflineHeld(_)
            | ArchetypeEditOutcome::Guide { .. }
            | ArchetypeEditOutcome::Note { .. } => None,
            ArchetypeEditOutcome::UnknownArchetype => Some("The selected archetype id is invalid."),
            ArchetypeEditOutcome::MixedFormats => {
                Some("Selected decks must all be the same format.")
            }
            ArchetypeEditOutcome::WrongFormat => {
                Some("That archetype belongs to a different format.")
            }
            ArchetypeEditOutcome::NotFound => Some("The archetype was not found."),
            ArchetypeEditOutcome::Refused(message) => Some(message),
            ArchetypeEditOutcome::PlanRejected(errors) => errors
                .first()
                .map(|error| error.message.as_str())
                .or(Some("Could not save your changes.")),
        }
    }

    fn done(matches: Vec<i64>, decks: Vec<i64>) -> ArchetypeEditOutcome {
        ArchetypeEditOutcome::Done { matches, decks }
    }
}

/// Apply an archetype edit in its own transaction (on the writer thread).
/// A refused edit changes nothing.
pub fn edit_archetypes(
    conn: &mut Connection,
    edit: &ArchetypeEdit,
) -> Result<ArchetypeEditOutcome, StoreError> {
    let tx = conn.transaction()?;
    let outcome = match edit {
        ArchetypeEdit::SetDetections { results, now_ms } => set_detections(&tx, results, *now_ms)?,
        ArchetypeEdit::EnsureCommander {
            uuid,
            name,
            format,
            color_identity,
            now_ms,
        } => ensure_commander(&tx, uuid, name, format, color_identity.as_deref(), *now_ms)?,
        ArchetypeEdit::ClearDetected { match_ids } => clear_detected(&tx, match_ids)?,
        ArchetypeEdit::ClearAllDetected => clear_all_detected(&tx)?,
        ArchetypeEdit::HoldOffline {
            definitions,
            now_ms,
        } => hold_offline(&tx, definitions, *now_ms)?,
        ArchetypeEdit::ReleaseOffline { now_ms } => release_offline(&tx, *now_ms)?,
        ArchetypeEdit::SetOpponent {
            match_ids,
            archetype_id,
            now_ms,
        } => set_opponent(&tx, match_ids, *archetype_id, *now_ms)?,
        ArchetypeEdit::SetDeckArchetype {
            deck_ids,
            archetype_id,
        } => set_deck_archetype(&tx, deck_ids, *archetype_id)?,
        ArchetypeEdit::PrefillDeck {
            deck_id,
            archetype_id,
        } => prefill_deck(&tx, *deck_id, *archetype_id)?,
        ArchetypeEdit::EnsureDownloaded {
            definition,
            format,
            now_ms,
        } => {
            write_definition(&tx, definition, Some(format), false, *now_ms)?;
            ArchetypeEditOutcome::done(Vec::new(), Vec::new())
        }
        ArchetypeEdit::ApplyDefinitions {
            plan,
            checked_at,
            now_ms,
        } => apply_definitions(&tx, plan, *checked_at, *now_ms)?,
        ArchetypeEdit::CreateManual(new) => manual::create(&tx, new)?,
        ArchetypeEdit::UpdateManual {
            id,
            name,
            format,
            color_identity,
            variant,
            now_ms,
        } => manual::update(
            &tx,
            *id,
            name,
            format,
            color_identity.as_deref(),
            variant.as_deref(),
            *now_ms,
        )?,
        ArchetypeEdit::RemoveVariant { id, list } => manual::remove_variant(&tx, *id, *list)?,
        ArchetypeEdit::ReassignVariant {
            id,
            list,
            target_id,
            now_ms,
        } => manual::reassign_variant(&tx, *id, *list, *target_id, *now_ms)?,
        ArchetypeEdit::Merge {
            source_id,
            parent_id,
            now_ms,
        } => manual::merge(&tx, *source_id, *parent_id, *now_ms)?,
        ArchetypeEdit::Unmerge { source_id, now_ms } => manual::unmerge(&tx, *source_id, *now_ms)?,
        ArchetypeEdit::DeleteManual { id } => manual::delete(&tx, *id)?,
        ArchetypeEdit::CreateGuide {
            deck_id,
            archetype_id,
            now_ms,
        } => guides::create_guide(&tx, *deck_id, *archetype_id, *now_ms)?,
        ArchetypeEdit::SaveGuideCards {
            deck_id,
            guide_id,
            cards,
            now_ms,
        } => guides::save_guide_cards(&tx, *deck_id, *guide_id, cards, *now_ms)?,
        ArchetypeEdit::DeleteGuide { deck_id, guide_id } => {
            guides::delete_guide(&tx, *deck_id, *guide_id)?
        }
        ArchetypeEdit::AddNote {
            deck_id,
            archetype_id,
            body,
            now_ms,
        } => guides::add_note(&tx, *deck_id, *archetype_id, body, *now_ms)?,
        ArchetypeEdit::DeleteNote { note_id } => guides::delete_note(&tx, *note_id)?,
        ArchetypeEdit::EnsureStub {
            uuid,
            name,
            format,
            color_identity,
            now_ms,
        } => {
            ensure_stub(&tx, uuid, name, format, color_identity.as_deref(), *now_ms)?;
            ArchetypeEditOutcome::done(Vec::new(), Vec::new())
        }
        ArchetypeEdit::Reinstate { uuid, now_ms } => {
            tx.prepare_cached(
                "UPDATE archetypes SET retired_at = NULL, updated_at = ?2
                 WHERE uuid = ?1 AND source = 'downloaded' AND retired_at IS NOT NULL",
            )?
            .execute(params![uuid, now_ms])?;
            ArchetypeEditOutcome::done(Vec::new(), Vec::new())
        }
    };
    // A refusal wrote nothing, so committing it changes nothing either.
    tx.commit()?;
    Ok(outcome)
}

/// The match still waits for detection (marker NULL, T2 review I1) and
/// its deck archetype is still the one detection read.
fn still_due(tx: &Transaction, result: &MatchDetection) -> Result<bool, StoreError> {
    Ok(tx
        .prepare_cached(
            "SELECT EXISTS (SELECT 1 FROM matches m
                 LEFT JOIN deck_versions v ON v.id = m.deck_version_id
                 LEFT JOIN decks d ON d.id = v.deck_id
                 WHERE m.id = ?1 AND m.archetypes_detected_at IS NULL
                   AND (d.archetype_id IS ?2
                        OR (?3 IS NOT NULL AND d.archetype_id IS ?3)))",
        )?
        .query_row(
            params![
                result.match_id,
                result.deck_archetype_id,
                result.deck_archetype
            ],
            |row| row.get(0),
        )?)
}

/// Most merge hops `ensure_commander` follows, a guard against a merge cycle.
const MERGE_CHAIN_LIMIT: usize = 16;

/// The archetype for a commander set: found by uuid and followed through
/// merges (a chain, bounded), else created.
pub(crate) fn ensure_commander(
    tx: &Transaction,
    uuid: &str,
    name: &str,
    format: &str,
    color_identity: Option<&str>,
    now_ms: i64,
) -> Result<ArchetypeEditOutcome, StoreError> {
    // A held row is kept, except the placeholder a sync pull makes for a
    // hand-set commander side this device had not made (#827: a commander
    // row named by its uuid), which takes the name and colours, and comes
    // back if anything retired it.
    tx.prepare_cached(
        "INSERT INTO archetypes
             (uuid, name, format, color_identity, source, definition, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, 'commander', NULL, ?5, ?5)
         ON CONFLICT (uuid) DO UPDATE SET name = excluded.name, format = excluded.format,
             color_identity = excluded.color_identity, retired_at = NULL,
             updated_at = excluded.updated_at
         WHERE archetypes.source = 'commander' AND archetypes.name = archetypes.uuid",
    )?
    .execute(params![uuid, name, format, color_identity, now_ms])?;
    let mut id: i64 = tx
        .prepare_cached("SELECT id FROM archetypes WHERE uuid = ?1")?
        .query_row([uuid], |row| row.get(0))?;
    for _ in 0..MERGE_CHAIN_LIMIT {
        let parent: Option<i64> = tx
            .prepare_cached("SELECT merged_into_id FROM archetypes WHERE id = ?1")?
            .query_row([id], |row| row.get(0))?;
        match parent {
            Some(parent) if parent != id => id = parent,
            _ => break,
        }
    }
    Ok(ArchetypeEditOutcome::Commander { archetype_id: id })
}

/// The archetype's format and source, `None` when there is no such row.
fn archetype_head(
    tx: &Transaction,
    archetype_id: i64,
) -> Result<Option<(Option<String>, String)>, StoreError> {
    Ok(tx
        .prepare_cached("SELECT format, source FROM archetypes WHERE id = ?1")?
        .query_row([archetype_id], |row| Ok((row.get(0)?, row.get(1)?)))
        .optional()?)
}

fn mark_detected(tx: &Transaction, match_id: i64, now_ms: i64) -> Result<(), StoreError> {
    tx.prepare_cached("UPDATE matches SET archetypes_detected_at = ?2 WHERE id = ?1")?
        .execute(params![match_id, now_ms])?;
    Ok(())
}

/// A seat's detected row, unless the seat has a hand-set one or it is the
/// opponent seat the player set to "none".
fn insert_auto_row(
    tx: &Transaction,
    match_id: i64,
    is_opponent: bool,
    seat: &SeatResult,
) -> Result<(), StoreError> {
    tx.prepare_cached(
        "INSERT INTO match_archetypes (match_id, is_opponent, archetype_id, confidence, manual)
         SELECT ?1, ?2, ?3, ?4, 0
         WHERE NOT (?2 AND EXISTS (SELECT 1 FROM matches
                                   WHERE id = ?1 AND opponent_archetype_none = 1))
         ON CONFLICT (match_id, is_opponent) DO NOTHING",
    )?
    .execute(params![
        match_id,
        is_opponent,
        seat.archetype_id,
        seat.confidence.clamp(0.0, 1.0)
    ])?;
    Ok(())
}

fn set_detections(
    tx: &Transaction,
    results: &[MatchDetection],
    now_ms: i64,
) -> Result<ArchetypeEditOutcome, StoreError> {
    let mut matches = Vec::new();
    let mut decks = Vec::new();
    for result in results {
        if !still_due(tx, result)? {
            continue;
        }
        tx.prepare_cached("DELETE FROM match_archetypes WHERE match_id = ?1 AND manual = 0")?
            .execute([result.match_id])?;
        if let Some(seat) = &result.player {
            insert_auto_row(tx, result.match_id, false, seat)?;
        }
        if let Some(seat) = &result.opponent {
            insert_auto_row(tx, result.match_id, true, seat)?;
        }
        if let Some(archetype_id) = result.deck_archetype {
            let deck: Option<i64> = tx
                .prepare_cached(
                    "UPDATE decks SET archetype_id = ?2
                     WHERE id = (SELECT v.deck_id FROM matches m
                                 JOIN deck_versions v ON v.id = m.deck_version_id
                                 WHERE m.id = ?1)
                       AND archetype_id IS NOT ?2
                     RETURNING id",
                )?
                .query_row(params![result.match_id, archetype_id], |row| row.get(0))
                .optional()?;
            decks.extend(deck);
        }
        mark_detected(tx, result.match_id, now_ms)?;
        matches.push(result.match_id);
    }
    decks.sort_unstable();
    decks.dedup();
    Ok(ArchetypeEditOutcome::done(matches, decks))
}

fn clear_all_detected(tx: &Transaction) -> Result<ArchetypeEditOutcome, StoreError> {
    let mut stmt = tx.prepare_cached(
        "UPDATE matches SET archetypes_detected_at = NULL
         WHERE archetypes_detected_at IS NOT NULL
         RETURNING id",
    )?;
    let rows = stmt.query_map([], |row| row.get(0))?;
    let mut matches = Vec::new();
    for row in rows {
        matches.push(row?);
    }
    matches.sort_unstable();
    Ok(ArchetypeEditOutcome::done(matches, Vec::new()))
}

fn clear_detected(tx: &Transaction, match_ids: &[i64]) -> Result<ArchetypeEditOutcome, StoreError> {
    let mut matches = Vec::new();
    for match_id in match_ids {
        let changed = tx
            .prepare_cached(
                "UPDATE matches SET archetypes_detected_at = NULL
                 WHERE id = ?1 AND archetypes_detected_at IS NOT NULL",
            )?
            .execute([match_id])?;
        if changed > 0 {
            matches.push(*match_id);
        }
    }
    Ok(ArchetypeEditOutcome::done(matches, Vec::new()))
}

/// A seat's hand-set row at confidence 1, replacing whatever it had.
fn put_manual_row(
    tx: &Transaction,
    match_id: i64,
    is_opponent: bool,
    archetype_id: i64,
) -> Result<(), StoreError> {
    tx.prepare_cached(
        "INSERT INTO match_archetypes (match_id, is_opponent, archetype_id, confidence, manual)
         VALUES (?1, ?2, ?3, 1.0, 1)
         ON CONFLICT (match_id, is_opponent) DO UPDATE SET
             archetype_id = excluded.archetype_id, confidence = 1.0, manual = 1",
    )?
    .execute(params![match_id, is_opponent, archetype_id])?;
    if is_opponent {
        set_opponent_none(tx, match_id, false)?;
    }
    Ok(())
}

/// The player's "none" for the opponent seat (spec ruling 12, final review
/// I2): input, set with the row deleted and cleared by any hand-set
/// opponent.
fn set_opponent_none(tx: &Transaction, match_id: i64, none: bool) -> Result<(), StoreError> {
    tx.prepare_cached("UPDATE matches SET opponent_archetype_none = ?2 WHERE id = ?1")?
        .execute(params![match_id, none])?;
    Ok(())
}

fn delete_seat(tx: &Transaction, match_id: i64, is_opponent: bool) -> Result<(), StoreError> {
    tx.prepare_cached("DELETE FROM match_archetypes WHERE match_id = ?1 AND is_opponent = ?2")?
        .execute(params![match_id, is_opponent])?;
    Ok(())
}

fn set_opponent(
    tx: &Transaction,
    match_ids: &[i64],
    archetype_id: Option<i64>,
    now_ms: i64,
) -> Result<ArchetypeEditOutcome, StoreError> {
    if let Some(id) = archetype_id
        && archetype_head(tx, id)?.is_none()
    {
        return Ok(ArchetypeEditOutcome::UnknownArchetype);
    }
    let mut matches = Vec::new();
    for match_id in match_ids {
        let has_opponent: bool = tx
            .prepare_cached(
                "SELECT EXISTS (SELECT 1 FROM matches WHERE id = ?1 AND opponent_id IS NOT NULL)",
            )?
            .query_row([match_id], |row| row.get(0))?;
        if !has_opponent {
            continue;
        }
        match archetype_id {
            Some(id) => put_manual_row(tx, *match_id, true, id)?,
            None => {
                delete_seat(tx, *match_id, true)?;
                set_opponent_none(tx, *match_id, true)?;
                mark_detected(tx, *match_id, now_ms)?;
            }
        }
        matches.push(*match_id);
    }
    Ok(ArchetypeEditOutcome::done(matches, Vec::new()))
}

fn set_deck_archetype(
    tx: &Transaction,
    deck_ids: &[i64],
    archetype_id: Option<i64>,
) -> Result<ArchetypeEditOutcome, StoreError> {
    let mut decks: Vec<(i64, String)> = Vec::new();
    for deck_id in deck_ids {
        let format: Option<String> = tx
            .prepare_cached("SELECT format FROM decks WHERE id = ?1")?
            .query_row([deck_id], |row| row.get(0))
            .optional()?;
        if let Some(format) = format {
            decks.push((*deck_id, format));
        }
    }
    if let Some(id) = archetype_id {
        // The format rule guards an assignment only: clearing to
        // unclassified fits every format (0.x `guardFormats`). Formats
        // compare as keys (`MtgoFormat::key`).
        let key = |format: &str| archetypes::format_key(format);
        if decks
            .iter()
            .any(|(_, format)| key(format) != key(&decks[0].1))
        {
            return Ok(ArchetypeEditOutcome::MixedFormats);
        }
        let Some((format, source)) = archetype_head(tx, id)? else {
            return Ok(ArchetypeEditOutcome::UnknownArchetype);
        };
        if source != "fallback"
            && let Some((_, deck_format)) = decks.first()
            && archetypes::format_key(deck_format) != format
        {
            return Ok(ArchetypeEditOutcome::WrongFormat);
        }
    }
    let mut changed_decks = Vec::new();
    let mut changed_matches = Vec::new();
    for (deck_id, _) in &decks {
        let changed = tx
            .prepare_cached(
                "UPDATE decks SET archetype_id = ?2 WHERE id = ?1 AND archetype_id IS NOT ?2",
            )?
            .execute(params![deck_id, archetype_id])?;
        // Unchanged, nothing to detect again (T2 review M6: a re-detection
        // may cost an API call).
        if changed > 0 {
            changed_decks.push(*deck_id);
            changed_matches.extend(redetect_player_seats(tx, *deck_id)?);
        }
    }
    changed_matches.sort_unstable();
    Ok(ArchetypeEditOutcome::done(changed_matches, changed_decks))
}

/// Clear the marker of the deck's matches whose player seat is not
/// hand-set, so detection gives the player seat the deck's archetype.
fn redetect_player_seats(tx: &Transaction, deck_id: i64) -> Result<Vec<i64>, StoreError> {
    let mut stmt = tx.prepare_cached(
        "UPDATE matches SET archetypes_detected_at = NULL
         WHERE archetypes_detected_at IS NOT NULL
           AND deck_version_id IN (SELECT id FROM deck_versions WHERE deck_id = ?1)
           AND NOT EXISTS (SELECT 1 FROM match_archetypes x
                           WHERE x.match_id = matches.id AND x.is_opponent = 0 AND x.manual = 1)
         RETURNING id",
    )?;
    let rows = stmt.query_map([deck_id], |row| row.get(0))?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

fn prefill_deck(
    tx: &Transaction,
    deck_id: i64,
    archetype_id: i64,
) -> Result<ArchetypeEditOutcome, StoreError> {
    if archetype_head(tx, archetype_id)?.is_none() {
        return Ok(ArchetypeEditOutcome::UnknownArchetype);
    }
    let changed = tx
        .prepare_cached(
            "UPDATE decks SET archetype_id = ?2 WHERE id = ?1 AND archetype_id IS NULL",
        )?
        .execute(params![deck_id, archetype_id])?;
    if changed == 0 {
        return Ok(ArchetypeEditOutcome::done(Vec::new(), Vec::new()));
    }
    // A match detected before the prefill holds the scorer's player seat:
    // ask again so it follows the deck (final review FIX NOW 4). The
    // opponent's "none" is input and stays (`opponent_archetype_none`).
    Ok(ArchetypeEditOutcome::done(
        redetect_player_seats(tx, deck_id)?,
        vec![deck_id],
    ))
}

/// Write one definition into its `downloaded` row (see `EnsureDownloaded`
/// and `ApplyDefinitions`). `replace` overwrites a held definition; without
/// it only a row without one is filled. A new row needs a format, from the
/// definition or `format`.
fn write_definition(
    tx: &Transaction,
    definition: &archetypes::Definition,
    format: Option<&String>,
    replace: bool,
    now_ms: i64,
) -> Result<(), StoreError> {
    let held: Option<(i64, String, bool)> = tx
        .prepare_cached(
            "SELECT id, source, definition IS NOT NULL FROM archetypes WHERE uuid = ?1",
        )?
        .query_row([&definition.uuid], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .optional()?;
    let json = serde_json::to_string(definition)
        .map_err(|error| StoreError::Corrupt(format!("archetype definition: {error}")))?;
    let name = Some(definition.name.trim())
        .filter(|name| !name.is_empty())
        .map(str::to_string);
    // A held row keeps its format unless the definition names one.
    let own_format = definition
        .format
        .as_ref()
        .map(|format| format.to_lowercase());
    let colors = archetypes::normalize_colors(definition.color_identity.as_deref());
    let archetype_id = match held {
        Some((_, source, _)) if source != "downloaded" => return Ok(()),
        Some((_, _, true)) if !replace => return Ok(()),
        Some((id, _, _)) => {
            tx.prepare_cached(
                "UPDATE archetypes SET name = COALESCE(?2, name), format = COALESCE(?3, format),
                     color_identity = COALESCE(?4, color_identity), definition = ?5,
                     definition_version = ?6, retired_at = NULL, updated_at = ?7
                 WHERE id = ?1",
            )?
            .execute(params![
                id,
                name,
                own_format,
                colors,
                json,
                definition.version,
                now_ms
            ])?;
            id
        }
        None => {
            let Some(format) = own_format.or_else(|| format.cloned()) else {
                tracing::warn!(uuid = %definition.uuid, "an archetype definition without a format is skipped");
                return Ok(());
            };
            tx.prepare_cached(
                "INSERT INTO archetypes (uuid, name, format, color_identity, source, definition,
                     definition_version, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, 'downloaded', ?5, ?6, ?7, ?7)",
            )?
            .execute(params![
                definition.uuid,
                name.unwrap_or_else(|| definition.uuid.clone()),
                format,
                colors,
                json,
                definition.version,
                now_ms
            ])?;
            tx.last_insert_rowid()
        }
    };
    write_key_list(tx, archetype_id, &definition.cards)
}

/// `ArchetypeEdit::HoldOffline`.
fn hold_offline(
    tx: &Transaction,
    definitions: &[archetypes::Definition],
    now_ms: i64,
) -> Result<ArchetypeEditOutcome, StoreError> {
    let mut held = OfflineHeld::default();
    for definition in definitions {
        let Some(format) = definition
            .format
            .as_deref()
            .map(|format| format.trim().to_lowercase())
            .filter(|format| !format.is_empty())
        else {
            held.skipped += 1;
            continue;
        };
        let name = match definition.name.trim() {
            "" => definition.uuid.clone(),
            name => name.to_string(),
        };
        let colors = archetypes::normalize_colors(definition.color_identity.as_deref());
        let row: Option<(i64, String)> = tx
            .prepare_cached("SELECT id, source FROM archetypes WHERE uuid = ?1")?
            .query_row([&definition.uuid], |row| Ok((row.get(0)?, row.get(1)?)))
            .optional()?;
        let archetype_id = match row {
            None => {
                tx.prepare_cached(
                    "INSERT INTO archetypes (uuid, name, format, color_identity, source,
                         offline_held, created_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4, 'manual', 1, ?5, ?5)",
                )?
                .execute(params![
                    definition.uuid,
                    name,
                    format,
                    colors,
                    now_ms
                ])?;
                held.added += 1;
                tx.last_insert_rowid()
            }
            Some((id, source)) if source == "downloaded" => {
                tx.prepare_cached(
                    "UPDATE archetypes SET source = 'manual', definition = NULL,
                         definition_version = NULL, offline_held = 1, name = ?2, format = ?3,
                         color_identity = COALESCE(?4, color_identity), retired_at = NULL,
                         updated_at = ?5
                     WHERE id = ?1",
                )?
                .execute(params![id, name, format, colors, now_ms])?;
                held.converted += 1;
                id
            }
            Some(_) => {
                held.skipped += 1;
                continue;
            }
        };
        write_key_list(tx, archetype_id, &definition.cards)?;
    }
    Ok(ArchetypeEditOutcome::OfflineHeld(held))
}

/// `ArchetypeEdit::ReleaseOffline`.
fn release_offline(tx: &Transaction, now_ms: i64) -> Result<ArchetypeEditOutcome, StoreError> {
    tx.prepare_cached(
        "DELETE FROM archetype_cards
         WHERE archetype_id IN (SELECT id FROM archetypes WHERE offline_held = 1)",
    )?
    .execute([])?;
    tx.prepare_cached(
        "UPDATE archetypes SET source = 'downloaded', definition = NULL,
             definition_version = NULL, offline_held = 0, updated_at = ?1
         WHERE offline_held = 1",
    )?
    .execute([now_ms])?;
    Ok(ArchetypeEditOutcome::done(Vec::new(), Vec::new()))
}

/// A `downloaded` row known only by its uuid (see `EnsureStub`); a held
/// uuid, of any source, is left alone.
fn ensure_stub(
    tx: &Transaction,
    uuid: &str,
    name: &str,
    format: &str,
    color_identity: Option<&str>,
    now_ms: i64,
) -> Result<(), StoreError> {
    let name = match name.trim() {
        "" => uuid,
        name => name,
    };
    tx.prepare_cached(
        "INSERT INTO archetypes (uuid, name, format, color_identity, source, created_at,
             updated_at)
         VALUES (?1, ?2, ?3, ?4, 'downloaded', ?5, ?5)
         ON CONFLICT (uuid) DO NOTHING",
    )?
    .execute(params![
        uuid,
        name,
        format.to_lowercase(),
        archetypes::normalize_colors(color_identity),
        now_ms
    ])?;
    Ok(())
}

/// The settings row that records the last change check (spec section 4).
pub const ARCHETYPES_CHECKED_AT: &str = "archetypes_checked_at";

fn apply_definitions(
    tx: &Transaction,
    plan: &archetypes::ChangePlan,
    checked_at: Option<i64>,
    now_ms: i64,
) -> Result<ArchetypeEditOutcome, StoreError> {
    for definition in &plan.replace {
        write_definition(tx, definition, None, true, now_ms)?;
    }
    let mut matches = Vec::new();
    let mut decks = Vec::new();
    for (from, to) in &plan.moves {
        let (moved_matches, moved_decks) = move_archetype(tx, from, to, now_ms)?;
        matches.extend(moved_matches);
        decks.extend(moved_decks);
    }
    for uuid in &plan.retire {
        tx.prepare_cached(
            "UPDATE archetypes SET retired_at = ?2, updated_at = ?2
             WHERE uuid = ?1 AND source = 'downloaded' AND retired_at IS NULL",
        )?
        .execute(params![uuid, now_ms])?;
    }
    matches.extend(redetect(tx, &plan.redetect)?);
    if let Some(at) = checked_at {
        tx.prepare_cached(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT (key) DO UPDATE SET value = excluded.value",
        )?
        .execute(params![ARCHETYPES_CHECKED_AT, at.to_string()])?;
    }
    matches.sort_unstable();
    matches.dedup();
    decks.sort_unstable();
    decks.dedup();
    Ok(ArchetypeEditOutcome::done(matches, decks))
}

/// A downloaded row's id by uuid, `None` for an unheld, manual or fallback
/// uuid (the server never moves those).
fn downloaded_id(tx: &Transaction, uuid: &str) -> Result<Option<i64>, StoreError> {
    Ok(tx
        .prepare_cached("SELECT id FROM archetypes WHERE uuid = ?1 AND source = 'downloaded'")?
        .query_row([uuid], |row| row.get(0))
        .optional()?)
}

fn returned_ids(
    tx: &Transaction,
    sql: &str,
    params: impl rusqlite::Params,
) -> Result<Vec<i64>, StoreError> {
    let mut stmt = tx.prepare_cached(sql)?;
    let rows = stmt.query_map(params, |row| row.get(0))?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

/// Move everything pointing at `from` onto `to` (match rows, deck links,
/// merge children, sideboard guides and matchup notes) and delete `from`
/// (0.x `ApplyArchetypeRefresh::remap`). A seat already holding `to` keeps its
/// row; whatever is left on `from` goes with it by cascade. Returns the
/// matches and decks that changed.
fn move_archetype(
    tx: &Transaction,
    from: &str,
    to: &str,
    now_ms: i64,
) -> Result<(Vec<i64>, Vec<i64>), StoreError> {
    let (Some(old), Some(new)) = (downloaded_id(tx, from)?, downloaded_id(tx, to)?) else {
        return Ok((Vec::new(), Vec::new()));
    };
    if old == new {
        return Ok((Vec::new(), Vec::new()));
    }
    let matches = returned_ids(
        tx,
        "UPDATE match_archetypes SET archetype_id = ?2
         WHERE archetype_id = ?1
           -- Mirrors 0.x's remap; it cannot fire under the v1 key (one
           -- row per match and seat), so the seat's own row is moved.
           AND NOT EXISTS (SELECT 1 FROM match_archetypes held
                           WHERE held.match_id = match_archetypes.match_id
                             AND held.is_opponent = match_archetypes.is_opponent
                             AND held.archetype_id = ?2)
         RETURNING match_id",
        params![old, new],
    )?;
    let decks = returned_ids(
        tx,
        "UPDATE decks SET archetype_id = ?2 WHERE archetype_id = ?1 RETURNING id",
        params![old, new],
    )?;
    // Merge children move; a target merged into the old row would be
    // merged into itself, so it is merged into nothing.
    tx.prepare_cached(
        "UPDATE archetypes SET merged_into_id = CASE WHEN id = ?2 THEN NULL ELSE ?2 END
         WHERE merged_into_id = ?1",
    )?
    .execute(params![old, new])?;
    // The old row's own merge carries over when the target has none and no
    // children (its own or the moved ones), so no chain is made (spec
    // section 7: no chains).
    tx.prepare_cached(
        "UPDATE archetypes SET merged_into_id = (SELECT merged_into_id FROM archetypes WHERE id = ?1)
         WHERE id = ?2 AND merged_into_id IS NULL
           AND (SELECT merged_into_id FROM archetypes WHERE id = ?1) <> ?2
           AND NOT EXISTS (SELECT 1 FROM archetypes child WHERE child.merged_into_id = ?2)",
    )?
    .execute(params![old, new])?;
    // Guides and notes move too (spec section 21.3); anything left on the
    // old row would go with it by cascade.
    guides::move_guides(tx, old, new, now_ms)?;
    tx.prepare_cached("DELETE FROM archetypes WHERE id = ?1")?
        .execute([old])?;
    tx.prepare_cached("UPDATE archetypes SET updated_at = ?2 WHERE id = ?1")?
        .execute(params![new, now_ms])?;
    Ok((matches, decks))
}

/// Ask detection again (spec section 5 "changed"): matches with an auto
/// row of a replaced archetype, and matches of a replaced format with an
/// auto row below 1.0. Returns the matches whose marker was cleared.
fn redetect(tx: &Transaction, plan: &archetypes::Redetect) -> Result<Vec<i64>, StoreError> {
    let mut matches = Vec::new();
    for uuid in &plan.uuids {
        matches.extend(returned_ids(
            tx,
            "UPDATE matches SET archetypes_detected_at = NULL
             WHERE archetypes_detected_at IS NOT NULL
               AND id IN (SELECT x.match_id FROM match_archetypes x
                          JOIN archetypes a ON a.id = x.archetype_id
                          WHERE x.manual = 0 AND a.uuid = ?1)
             RETURNING id",
            [uuid],
        )?);
    }
    if plan.formats.is_empty() {
        return Ok(matches);
    }
    // Matches hold MTGO's code (`CMODERN`); the plan holds format keys.
    let codes = returned_codes(tx)?;
    for code in codes {
        let Some(key) = archetypes::format_key(&code) else {
            continue;
        };
        if !plan.formats.contains(&key) {
            continue;
        }
        matches.extend(returned_ids(
            tx,
            "UPDATE matches SET archetypes_detected_at = NULL
             WHERE archetypes_detected_at IS NOT NULL AND format = ?1
               AND EXISTS (SELECT 1 FROM match_archetypes x
                           WHERE x.match_id = matches.id AND x.manual = 0
                             AND x.confidence < 1.0)
             RETURNING id",
            [&code],
        )?);
    }
    Ok(matches)
}

/// Every match format code in use.
fn returned_codes(tx: &Transaction) -> Result<Vec<String>, StoreError> {
    let mut stmt =
        tx.prepare_cached("SELECT DISTINCT format FROM matches WHERE format IS NOT NULL")?;
    let rows = stmt.query_map([], |row| row.get(0))?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

/// The catalog id a key list card is stored under: its MTGO id, else its
/// Arena id at the Arena offset, else none (the card is skipped).
fn key_list_catalog_id(card: &archetypes::DefinitionCard) -> Option<i64> {
    if card.mtgo_id > 0 {
        return Some(card.mtgo_id);
    }
    let offset = i64::from(cards::ARENA_ID_OFFSET);
    match card.arena_id {
        Some(arena_id) if arena_id > 0 && arena_id < offset => Some(offset + arena_id),
        _ => None,
    }
}

/// Replace list 1 with the definition's key list: cards without an MTGO
/// or Arena id, or without copies, are dropped, a card listed twice on a board is summed, every
/// id gets a `cards` stub, and the oracle id is the definition's, else the
/// card row's.
fn write_key_list(
    tx: &Transaction,
    archetype_id: i64,
    cards: &[archetypes::DefinitionCard],
) -> Result<(), StoreError> {
    tx.prepare_cached("DELETE FROM archetype_cards WHERE archetype_id = ?1 AND list = 1")?
        .execute([archetype_id])?;
    let mut usable: Vec<(i64, &archetypes::DefinitionCard)> = Vec::new();
    for card in cards {
        if card.quantity == 0 {
            continue;
        }
        if let Some(catalog_id) = key_list_catalog_id(card) {
            usable.push((catalog_id, card));
        }
    }
    let refs: Vec<crate::cards::CardRef> = usable
        .iter()
        .map(|(catalog_id, card)| crate::cards::CardRef {
            catalog_id: *catalog_id,
            name: card.name.clone(),
        })
        .collect();
    crate::cards::ensure_stubs(tx, &refs)?;
    let mut insert = tx.prepare_cached(
        "INSERT INTO archetype_cards (archetype_id, list, catalog_id, oracle_id, quantity, sideboard)
         VALUES (?1, 1, ?2,
             COALESCE(?3, (SELECT oracle_id FROM cards WHERE catalog_id = ?2)), ?4, ?5)
         ON CONFLICT (archetype_id, list, catalog_id, sideboard)
             DO UPDATE SET quantity = quantity + excluded.quantity",
    )?;
    for (catalog_id, card) in usable {
        insert.execute(params![
            archetype_id,
            catalog_id,
            card.oracle_id,
            card.quantity,
            card.sideboard
        ])?;
    }
    Ok(())
}

/// A manual match's seat (Plan L, decision C7), written inside the manual
/// edit's transaction: the archetype after one merge hop (0.x
/// `CreateManualMatch`) as a hand-set row, or no row for `None`. The match
/// is marked detected: both its seats are the player's.
pub fn write_manual_seat(
    tx: &Transaction,
    match_id: i64,
    is_opponent: bool,
    archetype_id: Option<i64>,
    now_ms: i64,
) -> Result<(), StoreError> {
    match archetype_id {
        Some(id) => {
            let parent: Option<Option<i64>> = tx
                .prepare_cached("SELECT merged_into_id FROM archetypes WHERE id = ?1")?
                .query_row([id], |row| row.get(0))
                .optional()?;
            let resolved = archetypes::merge_hop(id, |_| parent.flatten());
            put_manual_row(tx, match_id, is_opponent, resolved)?;
        }
        None => {
            delete_seat(tx, match_id, is_opponent)?;
            if is_opponent {
                set_opponent_none(tx, match_id, true)?;
            }
        }
    }
    mark_detected(tx, match_id, now_ms)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archetypes::tests::{db, insert_archetype, insert_deck, insert_match, seat};
    use crate::test_support::number;
    use rusqlite::{Connection, params};

    fn rows(conn: &Connection) -> Vec<(i64, i64, i64, f64, i64)> {
        let mut stmt = conn
            .prepare(
                "SELECT match_id, is_opponent, archetype_id, confidence, manual
                 FROM match_archetypes ORDER BY match_id, is_opponent",
            )
            .expect("prepare");
        stmt.query_map([], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
            ))
        })
        .expect("query")
        .map(|row| row.expect("row"))
        .collect()
    }

    fn marker(conn: &Connection, match_id: i64) -> Option<i64> {
        conn.query_row(
            "SELECT archetypes_detected_at FROM matches WHERE id = ?1",
            [match_id],
            |row| row.get(0),
        )
        .expect("marker")
    }

    fn manual_seat(conn: &Connection, match_id: i64, is_opponent: bool, archetype_id: i64) {
        conn.execute(
            "INSERT INTO match_archetypes (match_id, is_opponent, archetype_id, confidence, manual)
             VALUES (?1, ?2, ?3, 1.0, 1)",
            params![match_id, is_opponent, archetype_id],
        )
        .expect("manual seat");
    }

    fn done(matches: Vec<i64>, decks: Vec<i64>) -> ArchetypeEditOutcome {
        ArchetypeEditOutcome::Done { matches, decks }
    }

    #[test]
    fn detections_replace_only_auto_rows_and_mark_the_match_detected() {
        let (_dir, mut conn) = db("archetype-edit-detections");
        let a = insert_archetype(&conn, "q-a", "Qa", Some("modern"), "downloaded");
        let b = insert_archetype(&conn, "q-b", "Qb", Some("modern"), "downloaded");
        insert_match(&conn, 1, "log", 100);
        insert_match(&conn, 2, "log", 200);
        seat(&conn, 1, false, a);
        manual_seat(&conn, 1, true, a);
        seat(&conn, 2, true, a);

        let edit = ArchetypeEdit::SetDetections {
            results: vec![
                MatchDetection {
                    match_id: 1,
                    deck_archetype_id: None,
                    deck_archetype: None,
                    player: Some(SeatResult {
                        archetype_id: b,
                        confidence: 0.85,
                    }),
                    opponent: Some(SeatResult {
                        archetype_id: b,
                        confidence: 0.5,
                    }),
                },
                MatchDetection {
                    match_id: 2,
                    deck_archetype_id: None,
                    deck_archetype: None,
                    player: None,
                    opponent: None,
                },
                MatchDetection {
                    match_id: 99,
                    deck_archetype_id: None,
                    deck_archetype: None,
                    player: None,
                    opponent: None,
                },
            ],
            now_ms: 7_000,
        };
        assert_eq!(
            edit_archetypes(&mut conn, &edit).expect("edit"),
            done(vec![1, 2], Vec::new())
        );
        assert_eq!(
            rows(&conn),
            [(1, 0, b, 0.85, 0), (1, 1, a, 1.0, 1)],
            "the hand-set opponent stays; match 2's auto row goes"
        );
        assert_eq!(marker(&conn, 1), Some(7_000));
        assert_eq!(marker(&conn, 2), Some(7_000));
    }

    #[test]
    fn clearing_the_marker_asks_for_detection_again() {
        let (_dir, mut conn) = db("archetype-edit-clear");
        insert_match(&conn, 1, "log", 100);
        insert_match(&conn, 2, "log", 100);
        conn.execute("UPDATE matches SET archetypes_detected_at = 5", [])
            .expect("detected");
        let edit = ArchetypeEdit::ClearDetected {
            match_ids: vec![1, 99],
        };
        assert_eq!(
            edit_archetypes(&mut conn, &edit).expect("edit"),
            done(vec![1], Vec::new())
        );
        assert_eq!(marker(&conn, 1), None);
        assert_eq!(marker(&conn, 2), Some(5));
    }

    /// 0.x `BulkUpdateArchetypeController`: a hand-set opponent, matches
    /// without an opponent skipped; none deletes the row and marks the match
    /// detected (spec ruling 12).
    #[test]
    fn the_opponent_set_by_hand_is_a_manual_row_and_none_deletes_it() {
        let (_dir, mut conn) = db("archetype-edit-opponent");
        let a = insert_archetype(&conn, "q-a", "Qa", Some("modern"), "downloaded");
        insert_match(&conn, 1, "log", 100);
        insert_match(&conn, 2, "log", 200);
        insert_match(&conn, 3, "log", 300);
        conn.execute("UPDATE matches SET opponent_id = NULL WHERE id = 3", [])
            .expect("no opponent");
        seat(&conn, 1, true, a);
        seat(&conn, 1, false, a);

        let set = ArchetypeEdit::SetOpponent {
            match_ids: vec![1, 2, 3],
            archetype_id: Some(a),
            now_ms: 8_000,
        };
        assert_eq!(
            edit_archetypes(&mut conn, &set).expect("edit"),
            done(vec![1, 2], Vec::new())
        );
        assert_eq!(
            rows(&conn),
            [(1, 0, a, 0.9, 0), (1, 1, a, 1.0, 1), (2, 1, a, 1.0, 1)]
        );
        assert_eq!(marker(&conn, 2), None, "the player seat is still detected");

        let clear = ArchetypeEdit::SetOpponent {
            match_ids: vec![1],
            archetype_id: None,
            now_ms: 9_000,
        };
        assert_eq!(
            edit_archetypes(&mut conn, &clear).expect("edit"),
            done(vec![1], Vec::new())
        );
        assert_eq!(rows(&conn), [(1, 0, a, 0.9, 0), (2, 1, a, 1.0, 1)]);
        assert_eq!(marker(&conn, 1), Some(9_000), "no refill");

        let unknown = ArchetypeEdit::SetOpponent {
            match_ids: vec![2],
            archetype_id: Some(999),
            now_ms: 9_000,
        };
        let outcome = edit_archetypes(&mut conn, &unknown).expect("edit");
        assert_eq!(outcome, ArchetypeEditOutcome::UnknownArchetype);
        assert_eq!(
            outcome.refusal(),
            Some("The selected archetype id is invalid.")
        );
        assert_eq!(rows(&conn).len(), 2);
    }

    /// 0.x `BulkUpdateDeckArchetypeController` rules; the deck's matches
    /// whose player seat is not hand-set are detected again.
    #[test]
    fn a_deck_archetype_follows_0x_rules_and_asks_its_matches_again() {
        let (_dir, mut conn) = db("archetype-edit-deck");
        let modern = insert_archetype(&conn, "q-a", "Qa", Some("modern"), "manual");
        let pauper = insert_archetype(&conn, "q-p", "Qp", Some("pauper"), "manual");
        let homebrew = number(&conn, "SELECT id FROM archetypes WHERE name = 'Homebrew'");
        insert_deck(&conn, 1, "CMODERN");
        insert_deck(&conn, 2, "CMODERN");
        insert_deck(&conn, 3, "CPAUPER");
        conn.execute("UPDATE decks SET archived_at = 5 WHERE id = 2", [])
            .expect("archived");
        insert_match(&conn, 1, "log", 100);
        insert_match(&conn, 2, "log", 100);
        insert_match(&conn, 3, "log", 100);
        conn.execute_batch(
            "UPDATE matches SET deck_version_id = 10, archetypes_detected_at = 5;
             UPDATE matches SET deck_version_id = 30 WHERE id = 3;",
        )
        .expect("links");
        manual_seat(&conn, 2, false, pauper);

        let mixed = ArchetypeEdit::SetDeckArchetype {
            deck_ids: vec![1, 3],
            archetype_id: Some(modern),
        };
        let outcome = edit_archetypes(&mut conn, &mixed).expect("edit");
        assert_eq!(outcome, ArchetypeEditOutcome::MixedFormats);
        assert_eq!(
            outcome.refusal(),
            Some("Selected decks must all be the same format.")
        );
        let wrong = ArchetypeEdit::SetDeckArchetype {
            deck_ids: vec![1],
            archetype_id: Some(pauper),
        };
        let outcome = edit_archetypes(&mut conn, &wrong).expect("edit");
        assert_eq!(outcome, ArchetypeEditOutcome::WrongFormat);
        assert_eq!(
            outcome.refusal(),
            Some("That archetype belongs to a different format.")
        );
        assert_eq!(
            number(
                &conn,
                "SELECT COUNT(*) FROM decks WHERE archetype_id IS NOT NULL"
            ),
            0
        );

        let set = ArchetypeEdit::SetDeckArchetype {
            deck_ids: vec![1, 2],
            archetype_id: Some(modern),
        };
        assert_eq!(
            edit_archetypes(&mut conn, &set).expect("edit"),
            done(vec![1], vec![1, 2]),
            "archived decks included; match 2's hand-set player seat stays"
        );
        assert_eq!(marker(&conn, 1), None);
        assert_eq!(marker(&conn, 2), Some(5));
        assert_eq!(marker(&conn, 3), Some(5));

        let fallback = ArchetypeEdit::SetDeckArchetype {
            deck_ids: vec![3],
            archetype_id: Some(homebrew),
        };
        assert_eq!(
            edit_archetypes(&mut conn, &fallback).expect("edit"),
            done(vec![3], vec![3]),
            "a fallback fits any format"
        );
        let clear = ArchetypeEdit::SetDeckArchetype {
            deck_ids: vec![1],
            archetype_id: None,
        };
        edit_archetypes(&mut conn, &clear).expect("edit");
        assert_eq!(
            number(
                &conn,
                "SELECT COUNT(*) FROM decks WHERE archetype_id IS NULL"
            ),
            1
        );
    }

    // 0.x BulkUpdateDeckArchetypeTest "clears archetypes across formats
    // since unclassified is format-agnostic" (the format guard runs only
    // when an archetype is set), and formats compare as keys
    // (`MtgoFormat::key`), not raw codes.
    #[test]
    fn clearing_skips_the_format_rule_and_formats_compare_as_keys() {
        let (_dir, mut conn) = db("archetype-edit-deck-clear");
        let modern = insert_archetype(&conn, "q-a", "Qa", Some("modern"), "manual");
        let pauper = insert_archetype(&conn, "q-p", "Qp", Some("pauper"), "manual");
        insert_deck(&conn, 1, "CMODERN");
        insert_deck(&conn, 2, "CPAUPER");
        insert_deck(&conn, 3, "CModern");
        conn.execute_batch(&format!(
            "UPDATE decks SET archetype_id = {modern} WHERE id IN (1, 3);
             UPDATE decks SET archetype_id = {pauper} WHERE id = 2;"
        ))
        .expect("set");

        let clear = ArchetypeEdit::SetDeckArchetype {
            deck_ids: vec![1, 2],
            archetype_id: None,
        };
        assert_eq!(
            edit_archetypes(&mut conn, &clear).expect("edit"),
            done(vec![], vec![1, 2])
        );
        assert_eq!(
            number(
                &conn,
                "SELECT COUNT(*) FROM decks WHERE archetype_id IS NULL"
            ),
            2
        );

        // `CMODERN` and `CModern` are one format.
        let same = ArchetypeEdit::SetDeckArchetype {
            deck_ids: vec![1, 3],
            archetype_id: Some(modern),
        };
        assert_eq!(
            edit_archetypes(&mut conn, &same).expect("edit"),
            done(vec![], vec![1])
        );
    }

    #[test]
    fn a_prefill_writes_only_a_deck_without_an_archetype() {
        let (_dir, mut conn) = db("archetype-edit-prefill");
        let a = insert_archetype(&conn, "q-a", "Qa", Some("modern"), "manual");
        let b = insert_archetype(&conn, "q-b", "Qb", Some("modern"), "manual");
        insert_deck(&conn, 1, "CMODERN");
        let prefill = |archetype_id: i64| ArchetypeEdit::PrefillDeck {
            deck_id: 1,
            archetype_id,
        };
        assert_eq!(
            edit_archetypes(&mut conn, &prefill(a)).expect("edit"),
            done(Vec::new(), vec![1])
        );
        assert_eq!(
            edit_archetypes(&mut conn, &prefill(b)).expect("edit"),
            done(Vec::new(), Vec::new()),
            "never over a set archetype"
        );
        assert_eq!(number(&conn, "SELECT archetype_id FROM decks"), a);
    }

    /// Final review FIX NOW 4 (T4 M10): a match detected before the deck's
    /// prefill landed holds the scorer's player seat. The prefill asks its
    /// deck's matches again (a hand-set player seat and the opponent's
    /// "none" stay), so the player seat becomes the deck archetype.
    #[test]
    fn a_prefill_asks_the_decks_matches_again() {
        use crate::archetypes::detection_input;
        let (_dir, mut conn) = db("archetype-edit-prefill-redetect");
        let scored = insert_archetype(&conn, "q-scored", "Qscored", Some("modern"), "manual");
        let deck = insert_archetype(&conn, "q-deck", "Qdeck", Some("modern"), "manual");
        insert_deck(&conn, 1, "CMODERN");
        insert_match(&conn, 1, "log", 100);
        insert_match(&conn, 2, "log", 200);
        conn.execute(
            "UPDATE matches SET deck_version_id = 10, games_synced_at = 1,
                 archetypes_detected_at = 5",
            [],
        )
        .expect("detected before the prefill");
        seat(&conn, 1, false, scored);
        manual_seat(&conn, 2, false, scored);

        let prefill = ArchetypeEdit::PrefillDeck {
            deck_id: 1,
            archetype_id: deck,
        };
        assert_eq!(
            edit_archetypes(&mut conn, &prefill).expect("prefill"),
            done(vec![1], vec![1]),
            "the hand-set player seat of match 2 is left alone"
        );
        assert_eq!(marker(&conn, 1), None, "due again");
        assert_eq!(marker(&conn, 2), Some(5));
        let input = detection_input(&conn, 1).expect("input").expect("ready");
        assert_eq!(input.deck_archetype_id, Some(deck));
        let write = ArchetypeEdit::SetDetections {
            results: vec![MatchDetection {
                match_id: 1,
                deck_archetype_id: Some(deck),
                deck_archetype: None,
                player: Some(SeatResult {
                    archetype_id: deck,
                    confidence: 1.0,
                }),
                opponent: None,
            }],
            now_ms: 6_000,
        };
        edit_archetypes(&mut conn, &write).expect("write");
        assert_eq!(
            rows(&conn),
            [(1, 0, deck, 1.0, 0), (2, 0, scored, 1.0, 1)],
            "the player seat follows the deck"
        );
    }

    /// The manual match's seats (Plan L, decision C7): hand-set rows, one
    /// merge hop, the match marked detected.
    #[test]
    fn a_manual_seat_is_hand_set_follows_one_merge_and_marks_the_match() {
        let (_dir, mut conn) = db("archetype-edit-manual-seat");
        let parent = insert_archetype(&conn, "q-parent", "Qparent", Some("modern"), "manual");
        let source = insert_archetype(&conn, "q-source", "Qsource", Some("modern"), "manual");
        conn.execute(
            "UPDATE archetypes SET merged_into_id = ?1 WHERE id = ?2",
            params![parent, source],
        )
        .expect("merge");
        insert_match(&conn, 1, "manual", 100);
        seat(&conn, 1, false, source);

        let tx = conn.transaction().expect("tx");
        write_manual_seat(&tx, 1, true, Some(source), 4_000).expect("opponent");
        write_manual_seat(&tx, 1, false, Some(parent), 4_000).expect("player");
        tx.commit().expect("commit");
        assert_eq!(
            rows(&conn),
            [(1, 0, parent, 1.0, 1), (1, 1, parent, 1.0, 1)]
        );
        assert_eq!(marker(&conn, 1), Some(4_000));

        let tx = conn.transaction().expect("tx");
        write_manual_seat(&tx, 1, true, None, 5_000).expect("none");
        tx.commit().expect("commit");
        assert_eq!(rows(&conn), [(1, 0, parent, 1.0, 1)]);
        assert_eq!(marker(&conn, 1), Some(5_000));
        assert_eq!(none_flag(&conn, 1), 1, "the opponent's none is recorded");

        let tx = conn.transaction().expect("tx");
        write_manual_seat(&tx, 1, true, Some(parent), 6_000).expect("opponent again");
        tx.commit().expect("commit");
        assert_eq!(none_flag(&conn, 1), 0, "a hand-set opponent clears it");
    }

    fn opponent_result(match_id: i64, archetype_id: i64) -> MatchDetection {
        MatchDetection {
            match_id,
            deck_archetype_id: None,
            deck_archetype: None,
            player: None,
            opponent: Some(SeatResult {
                archetype_id,
                confidence: 0.5,
            }),
        }
    }

    /// T2 review I1: the player chose "none" for the opponent while the
    /// detection was in flight (the row deleted, the marker set). The late
    /// result is dropped, so the choice stands (spec ruling 12).
    #[test]
    fn a_late_detection_never_overwrites_a_players_none_choice() {
        let (_dir, mut conn) = db("archetype-edit-late");
        let a = insert_archetype(&conn, "q-a", "Qa", Some("modern"), "downloaded");
        insert_match(&conn, 1, "log", 100);
        insert_match(&conn, 2, "log", 100);
        let none = ArchetypeEdit::SetOpponent {
            match_ids: vec![1],
            archetype_id: None,
            now_ms: 3_000,
        };
        edit_archetypes(&mut conn, &none).expect("none");
        let late = ArchetypeEdit::SetDetections {
            results: vec![opponent_result(1, a), opponent_result(2, a)],
            now_ms: 4_000,
        };
        assert_eq!(
            edit_archetypes(&mut conn, &late).expect("edit"),
            done(vec![2], Vec::new()),
            "only the match still waiting for detection"
        );
        assert_eq!(rows(&conn), [(2, 1, a, 0.5, 0)]);
        assert_eq!(marker(&conn, 1), Some(3_000));
        assert_eq!(marker(&conn, 2), Some(4_000));
    }

    fn none_flag(conn: &Connection, match_id: i64) -> i64 {
        conn.query_row(
            "SELECT opponent_archetype_none FROM matches WHERE id = ?1",
            [match_id],
            |row| row.get(0),
        )
        .expect("none flag")
    }

    /// Final review I2 (spec ruling 12): the player's "none" is input, kept
    /// in `matches.opponent_archetype_none`, so the re-detection triggers
    /// (a deck archetype change, a definitions change) re-detect the
    /// player seat and never refill the opponent seat. Any hand-set
    /// opponent clears it.
    #[test]
    fn a_none_choice_survives_a_deck_archetype_change_and_a_definition_change() {
        use crate::archetypes::detection_input;
        let (_dir, mut conn) = db("archetype-edit-none-survives");
        let a = insert_archetype(&conn, "q-a", "Qa", Some("modern"), "downloaded");
        insert_deck(&conn, 1, "CMODERN");
        insert_match(&conn, 1, "log", 100);
        conn.execute(
            "UPDATE matches SET deck_version_id = 10, games_synced_at = 1,
                 archetypes_detected_at = 5",
            [],
        )
        .expect("linked, synced, detected");
        seat(&conn, 1, true, a);

        let none = ArchetypeEdit::SetOpponent {
            match_ids: vec![1],
            archetype_id: None,
            now_ms: 6_000,
        };
        edit_archetypes(&mut conn, &none).expect("none");
        assert_eq!(none_flag(&conn, 1), 1, "the choice is recorded");

        // The deck's archetype changes: the match is detected again.
        let deck = ArchetypeEdit::SetDeckArchetype {
            deck_ids: vec![1],
            archetype_id: Some(a),
        };
        assert_eq!(
            edit_archetypes(&mut conn, &deck).expect("deck"),
            done(vec![1], vec![1])
        );
        assert_eq!(marker(&conn, 1), None);
        let input = detection_input(&conn, 1).expect("input").expect("ready");
        assert!(input.opponent_manual, "detection leaves the opponent seat");
        // Even a result that decided the opponent seat writes no row there.
        let result = MatchDetection {
            match_id: 1,
            deck_archetype_id: Some(a),
            deck_archetype: None,
            player: Some(SeatResult {
                archetype_id: a,
                confidence: 1.0,
            }),
            opponent: Some(SeatResult {
                archetype_id: a,
                confidence: 0.5,
            }),
        };
        let write = ArchetypeEdit::SetDetections {
            results: vec![result],
            now_ms: 7_000,
        };
        assert_eq!(
            edit_archetypes(&mut conn, &write).expect("write"),
            done(vec![1], Vec::new())
        );
        assert_eq!(rows(&conn), [(1, 0, a, 1.0, 0)], "no opponent refill");

        // A definitions change re-detects the auto player seat.
        let check = ArchetypeEdit::ApplyDefinitions {
            plan: archetypes::ChangePlan {
                redetect: archetypes::Redetect {
                    uuids: vec!["q-a".to_string()],
                    formats: vec!["modern".to_string()],
                },
                ..archetypes::ChangePlan::default()
            },
            checked_at: None,
            now_ms: 8_000,
        };
        edit_archetypes(&mut conn, &check).expect("check");
        assert_eq!(marker(&conn, 1), None);
        assert!(
            detection_input(&conn, 1)
                .expect("input")
                .expect("ready")
                .opponent_manual
        );
        let write = ArchetypeEdit::SetDetections {
            results: vec![result],
            now_ms: 9_000,
        };
        edit_archetypes(&mut conn, &write).expect("write");
        assert_eq!(rows(&conn), [(1, 0, a, 1.0, 0)], "still no opponent");
        assert_eq!(none_flag(&conn, 1), 1);

        // A hand-set opponent replaces the choice.
        let set = ArchetypeEdit::SetOpponent {
            match_ids: vec![1],
            archetype_id: Some(a),
            now_ms: 10_000,
        };
        edit_archetypes(&mut conn, &set).expect("set");
        assert_eq!(none_flag(&conn, 1), 0);
        assert_eq!(rows(&conn), [(1, 0, a, 1.0, 0), (1, 1, a, 1.0, 1)]);
    }

    /// The deck's archetype changed while the detection was in flight (the
    /// marker stays NULL, so the marker alone cannot tell): the result read
    /// the old deck archetype and is dropped; the match stays due.
    #[test]
    fn a_detection_read_before_a_deck_archetype_change_is_dropped() {
        let (_dir, mut conn) = db("archetype-edit-stale-deck");
        let a = insert_archetype(&conn, "q-a", "Qa", Some("modern"), "manual");
        let b = insert_archetype(&conn, "q-b", "Qb", Some("modern"), "manual");
        insert_deck(&conn, 1, "CMODERN");
        insert_match(&conn, 1, "log", 100);
        conn.execute("UPDATE matches SET deck_version_id = 10", [])
            .expect("link");
        let read_with_a = ArchetypeEdit::SetDetections {
            results: vec![MatchDetection {
                match_id: 1,
                deck_archetype_id: Some(a),
                deck_archetype: None,
                player: Some(SeatResult {
                    archetype_id: a,
                    confidence: 1.0,
                }),
                opponent: None,
            }],
            now_ms: 4_000,
        };
        conn.execute("UPDATE decks SET archetype_id = ?1", [b])
            .expect("deck changed");
        assert_eq!(
            edit_archetypes(&mut conn, &read_with_a).expect("edit"),
            done(Vec::new(), Vec::new())
        );
        assert!(rows(&conn).is_empty());
        assert_eq!(marker(&conn, 1), None, "still due");
        conn.execute("UPDATE decks SET archetype_id = ?1", [a])
            .expect("deck back");
        assert_eq!(
            edit_archetypes(&mut conn, &read_with_a).expect("edit"),
            done(vec![1], Vec::new())
        );
    }

    fn definition(uuid: &str, name: &str, version: &str) -> archetypes::Definition {
        archetypes::Definition {
            uuid: uuid.to_string(),
            name: name.to_string(),
            format: Some("modern".to_string()),
            color_identity: Some("R,G".to_string()),
            version: Some(version.to_string()),
            cards: vec![
                archetypes::DefinitionCard {
                    mtgo_id: 501,
                    arena_id: None,
                    oracle_id: Some("q-oracle-a".to_string()),
                    name: Some("Qcard A".to_string()),
                    quantity: 4,
                    sideboard: false,
                },
                archetypes::DefinitionCard {
                    mtgo_id: 502,
                    arena_id: None,
                    oracle_id: None,
                    name: Some("Qcard B".to_string()),
                    quantity: 2,
                    sideboard: true,
                },
                archetypes::DefinitionCard {
                    mtgo_id: 0,
                    arena_id: None,
                    oracle_id: Some("q-oracle-none".to_string()),
                    name: None,
                    quantity: 1,
                    sideboard: false,
                },
            ],
            ..archetypes::Definition::default()
        }
    }

    fn head(
        conn: &Connection,
        uuid: &str,
    ) -> (String, Option<String>, Option<String>, Option<String>) {
        conn.query_row(
            "SELECT name, format, color_identity, definition_version FROM archetypes
             WHERE uuid = ?1",
            [uuid],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .expect("head")
    }

    fn list(conn: &Connection, uuid: &str) -> Vec<(i64, i64, Option<String>, i64, i64)> {
        let mut stmt = conn
            .prepare(
                "SELECT l.list, l.catalog_id, l.oracle_id, l.quantity, l.sideboard
                 FROM archetype_cards l JOIN archetypes a ON a.id = l.archetype_id
                 WHERE a.uuid = ?1 ORDER BY l.list, l.catalog_id",
            )
            .expect("prepare");
        stmt.query_map([uuid], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
            ))
        })
        .expect("query")
        .map(|row| row.expect("row"))
        .collect()
    }

    /// Spec section 5 "Getting one": an API uuid v1 does not hold is
    /// downloaded before detection writes it; a held stub without a
    /// definition (a sync pull, the 0.x import) is filled; a held
    /// definition is left for the change check.
    #[test]
    fn ensuring_a_download_writes_the_definition_and_its_key_list_once() {
        let (_dir, mut conn) = db("archetype-edit-ensure");
        conn.execute(
            "INSERT INTO cards (catalog_id, oracle_id, name) VALUES (502, 'q-oracle-b', 'Qcard B')",
            [],
        )
        .expect("card");
        let ensure = |definition: archetypes::Definition| ArchetypeEdit::EnsureDownloaded {
            definition: Box::new(definition),
            format: "modern".to_string(),
            now_ms: 6_000,
        };
        assert_eq!(
            edit_archetypes(&mut conn, &ensure(definition("q-new", "Qnew", "v1"))).expect("edit"),
            done(Vec::new(), Vec::new())
        );
        assert_eq!(
            head(&conn, "q-new"),
            (
                "Qnew".to_string(),
                Some("modern".to_string()),
                Some("R,G".to_string()),
                Some("v1".to_string())
            )
        );
        assert_eq!(
            list(&conn, "q-new"),
            [
                (1, 501, Some("q-oracle-a".to_string()), 4, 0),
                (1, 502, Some("q-oracle-b".to_string()), 2, 1),
            ],
            "list 1, an id-less card dropped, the oracle id from the card row"
        );
        assert_eq!(
            number(&conn, "SELECT COUNT(*) FROM cards WHERE catalog_id = 501"),
            1,
            "a stub for the unknown card"
        );
        edit_archetypes(&mut conn, &ensure(definition("q-new", "Qrenamed", "v2"))).expect("again");
        assert_eq!(head(&conn, "q-new").0, "Qnew", "a held definition stays");

        // A stub known only by uuid, and a decklist-only definition.
        insert_archetype(&conn, "q-stub", "q-stub", Some("pauper"), "downloaded");
        let cards_only = archetypes::Definition {
            uuid: "q-stub".to_string(),
            cards: definition("q-stub", "", "v").cards,
            ..archetypes::Definition::default()
        };
        edit_archetypes(&mut conn, &ensure(cards_only)).expect("fill");
        assert_eq!(
            head(&conn, "q-stub"),
            ("q-stub".to_string(), Some("pauper".to_string()), None, None),
            "the stub keeps its name and format"
        );
        assert_eq!(list(&conn, "q-stub").len(), 2);
        assert!(
            conn.query_row(
                "SELECT definition IS NOT NULL FROM archetypes WHERE uuid = 'q-stub'",
                [],
                |row| row.get::<_, bool>(0)
            )
            .expect("defined")
        );

        // Never over a manual or fallback row.
        insert_archetype(&conn, "q-mine", "Qmine", Some("modern"), "manual");
        edit_archetypes(&mut conn, &ensure(definition("q-mine", "Qserver", "v1"))).expect("skip");
        assert_eq!(head(&conn, "q-mine").0, "Qmine");
        assert!(list(&conn, "q-mine").is_empty());
    }

    /// Spec section 5 "changed": name, format, colours, definition, version
    /// and list 1 are replaced and the retirement is cleared.
    #[test]
    fn changed_definitions_replace_the_rows_and_list_1() {
        let (_dir, mut conn) = db("archetype-edit-changed");
        let held = insert_archetype(&conn, "q-held", "Qold", Some("modern"), "downloaded");
        conn.execute_batch(&format!(
            "UPDATE archetypes SET retired_at = 9, definition = '{{}}', definition_version = 'v0'
                 WHERE id = {held};
             INSERT INTO archetype_cards (archetype_id, list, catalog_id, quantity, sideboard)
                 VALUES ({held}, 1, 900, 4, 0);"
        ))
        .expect("held");
        let changed = apply(
            vec![
                definition("q-held", "Qnew", "v1"),
                definition("q-fresh", "Qfresh", "v1"),
            ],
            &[],
            &[],
        );
        assert_eq!(
            edit_archetypes(&mut conn, &changed).expect("edit"),
            done(Vec::new(), Vec::new())
        );
        assert_eq!(
            head(&conn, "q-held"),
            (
                "Qnew".to_string(),
                Some("modern".to_string()),
                Some("R,G".to_string()),
                Some("v1".to_string())
            )
        );
        assert_eq!(
            number(
                &conn,
                "SELECT COUNT(*) FROM archetypes WHERE retired_at IS NOT NULL"
            ),
            0
        );
        assert_eq!(list(&conn, "q-held").len(), 2, "list 1 replaced");
        assert_eq!(
            head(&conn, "q-fresh").0,
            "Qfresh",
            "a moved target is added"
        );
        let stored: String = conn
            .query_row(
                "SELECT definition FROM archetypes WHERE uuid = 'q-held'",
                [],
                |row| row.get(0),
            )
            .expect("stored");
        assert_eq!(
            archetypes::parse_definition(&stored).expect("parses").name,
            "Qnew"
        );
    }

    /// The change check's edit for a plan built from these answers over
    /// the archetypes the test database holds.
    fn apply_plan(conn: &Connection, check: archetypes::DefinitionsCheck) -> ArchetypeEdit {
        let held = crate::archetypes::held_definitions(conn).expect("held");
        ArchetypeEdit::ApplyDefinitions {
            plan: archetypes::plan_changes(&held, &check),
            checked_at: Some(7_000),
            now_ms: 7_000,
        }
    }

    fn apply(
        changed: Vec<archetypes::Definition>,
        moved: &[(&str, &str)],
        retired: &[&str],
    ) -> ArchetypeEdit {
        ArchetypeEdit::ApplyDefinitions {
            plan: archetypes::ChangePlan {
                replace: changed,
                moves: moved
                    .iter()
                    .map(|(from, to)| (from.to_string(), to.to_string()))
                    .collect(),
                retire: retired.iter().map(|uuid| uuid.to_string()).collect(),
                redetect: archetypes::Redetect::default(),
            },
            checked_at: None,
            now_ms: 7_000,
        }
    }

    fn id_of(conn: &Connection, uuid: &str) -> Option<i64> {
        conn.query_row("SELECT id FROM archetypes WHERE uuid = ?1", [uuid], |row| {
            row.get(0)
        })
        .optional()
        .expect("id")
    }

    fn updated_at(conn: &Connection, id: i64) -> i64 {
        number(
            conn,
            &format!("SELECT updated_at FROM archetypes WHERE id = {id}"),
        )
    }

    /// Review Focus 4, with 0.x ApplyArchetypeRefreshTest "remaps matches to
    /// a mapped successor instead of re-detecting" and "moves deck links and
    /// merged children to the mapped successor": every match row (hand-set
    /// ones too, ruling 10), deck link (archived decks too) and merge child
    /// moves; the old row goes; nothing is detected again.
    #[test]
    fn a_moved_uuid_takes_its_matches_decks_and_merge_children() {
        let (_dir, mut conn) = db("archetype-edit-moved");
        let old = insert_archetype(&conn, "q-old", "Qold", Some("modern"), "downloaded");
        let target = insert_archetype(&conn, "q-target", "Qtarget", Some("modern"), "downloaded");
        let child = insert_archetype(&conn, "q-child", "Qchild", Some("modern"), "manual");
        let other = insert_archetype(&conn, "q-other", "Qother", Some("modern"), "downloaded");
        conn.execute_batch(&format!(
            "UPDATE archetypes SET merged_into_id = {old} WHERE id = {child};
             UPDATE archetypes SET merged_into_id = {old} WHERE id = {target};"
        ))
        .expect("merges");
        insert_deck(&conn, 1, "CMODERN");
        insert_deck(&conn, 2, "CMODERN");
        conn.execute_batch(&format!(
            "UPDATE decks SET archetype_id = {old};
             UPDATE decks SET archived_at = 5 WHERE id = 2;"
        ))
        .expect("deck links");
        insert_match(&conn, 1, "log", 100);
        insert_match(&conn, 2, "log", 200);
        insert_match(&conn, 3, "log", 300);
        conn.execute("UPDATE matches SET archetypes_detected_at = 5", [])
            .expect("detected");
        seat(&conn, 1, true, old);
        manual_seat(&conn, 2, true, old);
        seat(&conn, 2, false, other);
        seat(&conn, 3, true, other);

        let outcome = edit_archetypes(&mut conn, &apply(Vec::new(), &[("q-old", "q-target")], &[]))
            .expect("move");
        assert_eq!(outcome, done(vec![1, 2], vec![1, 2]));
        assert_eq!(id_of(&conn, "q-old"), None, "the old row is gone");
        assert_eq!(
            rows(&conn),
            [
                (1, 1, target, 0.9, 0),
                (2, 0, other, 0.9, 0),
                (2, 1, target, 1.0, 1),
                (3, 1, other, 0.9, 0)
            ],
            "the hand-set row moves and stays hand-set"
        );
        assert_eq!(
            number(
                &conn,
                &format!("SELECT COUNT(*) FROM decks WHERE archetype_id IS NOT {target}")
            ),
            0,
            "both decks, the archived one too"
        );
        assert_eq!(
            number(
                &conn,
                &format!("SELECT merged_into_id FROM archetypes WHERE id = {child}")
            ),
            target
        );
        assert_eq!(
            number(
                &conn,
                &format!("SELECT merged_into_id IS NULL FROM archetypes WHERE id = {target}")
            ),
            1,
            "a target merged into the old row is merged into nothing (never into itself)"
        );
        for id in 1..=3 {
            assert_eq!(marker(&conn, id), Some(5), "no re-detection");
        }
        assert_eq!(updated_at(&conn, target), 7_000);
    }

    /// Review Focus 4, with 0.x "drops a remapped row when the successor is
    /// already assigned to that match and player": a seat holds one row, so
    /// a seat already holding the target keeps exactly that one row (its
    /// confidence and hand-set flag), and the other seat's row moves.
    #[test]
    fn a_move_onto_a_seat_already_holding_the_target_keeps_one_row() {
        let (_dir, mut conn) = db("archetype-edit-move-seat");
        let old = insert_archetype(&conn, "q-old", "Qold", Some("modern"), "downloaded");
        let target = insert_archetype(&conn, "q-target", "Qtarget", Some("modern"), "downloaded");
        insert_match(&conn, 1, "log", 100);
        manual_seat(&conn, 1, true, target);
        seat(&conn, 1, false, old);

        edit_archetypes(&mut conn, &apply(Vec::new(), &[("q-old", "q-target")], &[]))
            .expect("move");
        assert_eq!(
            rows(&conn),
            [(1, 0, target, 0.9, 0), (1, 1, target, 1.0, 1)]
        );
        assert_eq!(
            number(&conn, "SELECT COUNT(*) FROM match_archetypes"),
            2,
            "one row per seat"
        );
        // Applied again (0.x "is idempotent"): nothing more happens.
        assert_eq!(
            edit_archetypes(&mut conn, &apply(Vec::new(), &[("q-old", "q-target")], &[]))
                .expect("again"),
            done(Vec::new(), Vec::new())
        );
        assert_eq!(
            rows(&conn),
            [(1, 0, target, 0.9, 0), (1, 1, target, 1.0, 1)]
        );
    }

    /// Review Focus 4: a retired archetype keeps its matches (and its name
    /// on them), is no detection candidate any more, and nothing is
    /// detected again.
    #[test]
    fn a_retired_archetype_keeps_its_matches_and_leaves_detection() {
        let (_dir, mut conn) = db("archetype-edit-retired");
        let gone = insert_archetype(&conn, "q-gone", "Qgone", Some("modern"), "downloaded");
        conn.execute(
            "UPDATE archetypes SET definition = '{}', definition_version = 'v1' WHERE id = ?1",
            [gone],
        )
        .expect("defined");
        insert_match(&conn, 1, "log", 100);
        conn.execute("UPDATE matches SET archetypes_detected_at = 5", [])
            .expect("detected");
        seat(&conn, 1, true, gone);
        let check = archetypes::DefinitionsCheck {
            retired: vec!["q-gone".to_string()],
            ..archetypes::DefinitionsCheck::default()
        };
        assert_eq!(
            crate::archetypes::held_candidates(&conn, "modern")
                .expect("candidates")
                .len(),
            1
        );
        let edit = apply_plan(&conn, check.clone());
        edit_archetypes(&mut conn, &edit).expect("retire");
        assert_eq!(rows(&conn), [(1, 1, gone, 0.9, 0)]);
        assert_eq!(marker(&conn, 1), Some(5));
        assert_eq!(
            number(
                &conn,
                &format!("SELECT retired_at FROM archetypes WHERE id = {gone}")
            ),
            7_000
        );
        assert_eq!(updated_at(&conn, gone), 7_000);
        assert!(
            crate::archetypes::held_candidates(&conn, "modern")
                .expect("candidates")
                .is_empty(),
            "detection skips it"
        );
        // Again: already retired, the time stays.
        let mut again = apply_plan(&conn, check);
        if let ArchetypeEdit::ApplyDefinitions { now_ms, .. } = &mut again {
            *now_ms = 9_000;
        }
        edit_archetypes(&mut conn, &again).expect("again");
        assert_eq!(
            number(
                &conn,
                &format!("SELECT retired_at FROM archetypes WHERE id = {gone}")
            ),
            7_000
        );
    }

    /// Spec section 5 "changed": auto rows of the changed archetype and
    /// auto rows below 1.0 in its format are detected again; hand-set rows
    /// and other formats are not. The check time is recorded.
    #[test]
    fn a_changed_definition_redetects_auto_rows_only() {
        let (_dir, mut conn) = db("archetype-edit-redetect");
        let changed = insert_archetype(&conn, "q-held", "Qheld", Some("modern"), "downloaded");
        let sure = insert_archetype(&conn, "q-sure", "Qsure", Some("modern"), "downloaded");
        let pauper = insert_archetype(&conn, "q-pauper", "Qpauper", Some("pauper"), "downloaded");
        conn.execute_batch(&format!(
            "UPDATE archetypes SET definition = '{{}}', definition_version = 'v0'
                 WHERE id IN ({changed}, {sure}, {pauper});"
        ))
        .expect("defined");
        for id in 1..=5 {
            insert_match(&conn, id, "log", id * 100);
        }
        conn.execute_batch(
            "UPDATE matches SET format = 'CPAUPER' WHERE id = 5;
             UPDATE matches SET archetypes_detected_at = 5;",
        )
        .expect("detected");
        // 1: auto row of the changed archetype at 1.0.
        conn.execute(
            "INSERT INTO match_archetypes (match_id, is_opponent, archetype_id, confidence, manual)
             VALUES (1, 1, ?1, 1.0, 0)",
            [changed],
        )
        .expect("row");
        // 2: hand-set row of the changed archetype.
        manual_seat(&conn, 2, true, changed);
        // 3: another archetype, auto below 1.0, same format.
        seat(&conn, 3, true, sure);
        // 4: another archetype, auto at 1.0.
        conn.execute(
            "INSERT INTO match_archetypes (match_id, is_opponent, archetype_id, confidence, manual)
             VALUES (4, 1, ?1, 1.0, 0)",
            [sure],
        )
        .expect("row");
        // 5: another format, auto below 1.0.
        seat(&conn, 5, true, pauper);

        let edit = apply_plan(
            &conn,
            archetypes::DefinitionsCheck {
                changed: vec![definition("q-held", "Qheld", "v1")],
                ..archetypes::DefinitionsCheck::default()
            },
        );
        assert_eq!(
            edit_archetypes(&mut conn, &edit).expect("apply"),
            done(vec![1, 3], Vec::new())
        );
        assert_eq!(marker(&conn, 1), None);
        assert_eq!(
            marker(&conn, 2),
            Some(5),
            "hand-set rows are never re-detected"
        );
        assert_eq!(marker(&conn, 3), None);
        assert_eq!(marker(&conn, 4), Some(5));
        assert_eq!(marker(&conn, 5), Some(5));
        assert_eq!(rows(&conn).len(), 5, "rows stay until detection runs");
        assert_eq!(
            number(
                &conn,
                "SELECT CAST(value AS INTEGER) FROM settings WHERE key = 'archetypes_checked_at'"
            ),
            7_000
        );
        assert_eq!(updated_at(&conn, changed), 7_000);
    }

    /// 0.x ApplyArchetypeRefreshTest "upserts api archetypes ... while
    /// preserving manual and fallback" and ManualArchetypeTest "does not
    /// overwrite manual archetypes during API sync": an answer naming a
    /// manual or fallback uuid changes nothing, and neither can be moved or
    /// retired by the server.
    #[test]
    fn the_check_never_touches_manual_or_fallback_archetypes() {
        let (_dir, mut conn) = db("archetype-edit-manual-kept");
        let mine = insert_archetype(&conn, "q-mine", "Qmine", Some("modern"), "manual");
        let homebrew = number(&conn, "SELECT id FROM archetypes WHERE name = 'Homebrew'");
        let homebrew_uuid: String = conn
            .query_row(
                "SELECT uuid FROM archetypes WHERE id = ?1",
                [homebrew],
                |row| row.get(0),
            )
            .expect("uuid");
        insert_archetype(&conn, "q-target", "Qtarget", Some("modern"), "downloaded");
        let edit = apply(
            vec![
                definition("q-mine", "Qserver", "v1"),
                definition(&homebrew_uuid, "Qserver", "v1"),
            ],
            &[("q-mine", "q-target"), (homebrew_uuid.as_str(), "q-target")],
            &["q-mine", homebrew_uuid.as_str()],
        );
        edit_archetypes(&mut conn, &edit).expect("apply");
        assert_eq!(head(&conn, "q-mine").0, "Qmine");
        assert_eq!(head(&conn, &homebrew_uuid).0, "Homebrew");
        assert_eq!(id_of(&conn, "q-mine"), Some(mine));
        assert_eq!(id_of(&conn, &homebrew_uuid), Some(homebrew));
        assert_eq!(
            number(
                &conn,
                "SELECT COUNT(*) FROM archetypes WHERE retired_at IS NOT NULL"
            ),
            0
        );
        assert!(list(&conn, "q-mine").is_empty());
    }

    /// The key list's Arena id bounds: 0, negative and ids at or above the
    /// offset are skipped, the nearest ids inside are kept.
    #[test]
    fn the_key_list_bounds_arena_ids() {
        let (_dir, mut conn) = db("archetype-edit-key-list-bounds");
        let offset = i64::from(cards::ARENA_ID_OFFSET);
        let card = |arena_id: i64, name: &str| archetypes::DefinitionCard {
            mtgo_id: 0,
            arena_id: Some(arena_id),
            oracle_id: None,
            name: Some(name.to_string()),
            quantity: 1,
            sideboard: false,
        };
        let cards = vec![
            card(0, "Qzero"),
            card(-5, "Qnegative"),
            card(offset, "Qat offset"),
            card(offset + 2, "Qabove"),
            card(1, "Qfirst"),
            card(offset - 1, "Qlast"),
        ];
        conn.execute(
            "INSERT INTO archetypes (uuid, name, format, source, created_at, updated_at)
             VALUES ('q-bounds', 'Qbounds', 'modern', 'manual', 1, 1)",
            [],
        )
        .expect("archetype");
        let archetype_id = conn.last_insert_rowid();
        let tx = conn.transaction().expect("tx");
        write_key_list(&tx, archetype_id, &cards).expect("write");
        let mut stmt = tx
            .prepare(
                "SELECT catalog_id FROM archetype_cards WHERE archetype_id = ?1 ORDER BY catalog_id",
            )
            .expect("prepare");
        let ids: Vec<i64> = stmt
            .query_map([archetype_id], |row| row.get(0))
            .expect("query")
            .map(|row| row.expect("row"))
            .collect();
        assert_eq!(ids, vec![offset + 1, offset + offset - 1]);
    }

    /// AR6: a card only Arena has is kept under its Arena catalog id.
    #[test]
    fn the_key_list_keeps_arena_only_cards() {
        fn card(mtgo_id: i64, arena_id: Option<i64>, oracle: &str) -> archetypes::DefinitionCard {
            archetypes::DefinitionCard {
                mtgo_id,
                arena_id,
                oracle_id: Some(oracle.to_string()),
                name: Some(format!("Q{oracle}")),
                quantity: 2,
                sideboard: false,
            }
        }
        let cards = vec![
            card(12345, None, "q-mtgo"),
            card(0, Some(91538), "q-arena"),
            card(777, Some(91539), "q-both"),
            card(0, None, "q-neither"),
        ];
        let expected = |conn: &Connection, uuid: &str| {
            let rows = list(conn, uuid);
            assert_eq!(
                rows,
                vec![
                    (1, 777, Some("q-both".to_string()), 2, 0),
                    (1, 12345, Some("q-mtgo".to_string()), 2, 0),
                    (1, 100_091_538, Some("q-arena".to_string()), 2, 0),
                ]
            );
            let stub: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM cards WHERE catalog_id = 100091538",
                    [],
                    |row| row.get(0),
                )
                .expect("stub");
            assert_eq!(stub, 1);
        };

        let (_dir, mut conn) = db("archetype-edit-key-list-arena-online");
        let mut online = definition("q-arena-online", "Qarena", "v1");
        online.cards = cards.clone();
        edit_archetypes(&mut conn, &apply(vec![online], &[], &[])).expect("apply");
        expected(&conn, "q-arena-online");

        let (_dir, mut conn) = db("archetype-edit-key-list-arena-offline");
        let mut offline = definition("q-arena-offline", "Qarena", "v1");
        offline.cards = cards;
        hold(&mut conn, vec![offline]);
        expected(&conn, "q-arena-offline");
    }

    /// 0.x DownloadDecklistTest "skips cards with empty mtgo_id" and "keeps
    /// maindeck and sideboard rows for the same card".
    #[test]
    fn the_key_list_skips_cards_without_an_id_and_keeps_both_boards() {
        let (_dir, mut conn) = db("archetype-edit-key-list");
        let mut both = definition("q-new", "Qnew", "v1");
        both.cards = vec![
            archetypes::DefinitionCard {
                mtgo_id: 501,
                arena_id: None,
                oracle_id: Some("q-oracle-a".to_string()),
                name: Some("Qcard A".to_string()),
                quantity: 3,
                sideboard: false,
            },
            archetypes::DefinitionCard {
                mtgo_id: 501,
                arena_id: None,
                oracle_id: Some("q-oracle-a".to_string()),
                name: Some("Qcard A".to_string()),
                quantity: 1,
                sideboard: true,
            },
            archetypes::DefinitionCard {
                mtgo_id: 0,
                arena_id: None,
                oracle_id: Some("q-oracle-b".to_string()),
                name: Some("Qcard B".to_string()),
                quantity: 4,
                sideboard: false,
            },
        ];
        edit_archetypes(&mut conn, &apply(vec![both], &[], &[])).expect("apply");
        assert_eq!(
            list(&conn, "q-new"),
            [
                (1, 501, Some("q-oracle-a".to_string()), 3, 0),
                (1, 501, Some("q-oracle-a".to_string()), 1, 1)
            ]
        );
    }

    /// T2 review M6: setting a deck to the archetype it already has asks
    /// none of its matches again (each re-detection may cost an API call).
    #[test]
    fn setting_a_deck_to_its_own_archetype_requeues_nothing() {
        let (_dir, mut conn) = db("archetype-edit-same-deck");
        let a = insert_archetype(&conn, "q-a", "Qa", Some("modern"), "manual");
        insert_deck(&conn, 1, "CMODERN");
        insert_match(&conn, 1, "log", 100);
        conn.execute_batch(&format!(
            "UPDATE decks SET archetype_id = {a};
             UPDATE matches SET deck_version_id = 10, archetypes_detected_at = 5;"
        ))
        .expect("set");
        let same = ArchetypeEdit::SetDeckArchetype {
            deck_ids: vec![1],
            archetype_id: Some(a),
        };
        assert_eq!(
            edit_archetypes(&mut conn, &same).expect("edit"),
            done(Vec::new(), Vec::new())
        );
        assert_eq!(marker(&conn, 1), Some(5));
    }

    /// T5 review m5: the old row's own merge carries over to the target
    /// only when that makes no chain (section 7: no chains); a target with
    /// merge children (its own, or the old row's moved to it) keeps none.
    #[test]
    fn a_moved_archetype_carries_its_merge_only_without_a_chain() {
        let (_dir, mut conn) = db("archetype-edit-move-merge");
        let parent = insert_archetype(&conn, "q-parent", "Qparent", Some("modern"), "manual");
        let old = insert_archetype(&conn, "q-old", "Qold", Some("modern"), "downloaded");
        let target = insert_archetype(&conn, "q-target", "Qtarget", Some("modern"), "downloaded");
        conn.execute(
            "UPDATE archetypes SET merged_into_id = ?1 WHERE id = ?2",
            params![parent, old],
        )
        .expect("merge");
        edit_archetypes(&mut conn, &apply(Vec::new(), &[("q-old", "q-target")], &[]))
            .expect("move");
        assert_eq!(
            number(
                &conn,
                &format!("SELECT merged_into_id FROM archetypes WHERE id = {target}")
            ),
            parent,
            "carried: the target has no children"
        );

        let old = insert_archetype(&conn, "q-old-2", "Qold2", Some("modern"), "downloaded");
        let other = insert_archetype(&conn, "q-target-2", "Qt2", Some("modern"), "downloaded");
        let child = insert_archetype(&conn, "q-child", "Qchild", Some("modern"), "manual");
        conn.execute_batch(&format!(
            "UPDATE archetypes SET merged_into_id = {parent} WHERE id = {old};
             UPDATE archetypes SET merged_into_id = {old} WHERE id = {child};"
        ))
        .expect("merges");
        edit_archetypes(
            &mut conn,
            &apply(Vec::new(), &[("q-old-2", "q-target-2")], &[]),
        )
        .expect("move");
        assert_eq!(
            number(
                &conn,
                &format!("SELECT merged_into_id IS NULL FROM archetypes WHERE id = {other}")
            ),
            1,
            "not carried: the moved child would make a chain"
        );
        assert_eq!(
            number(
                &conn,
                &format!("SELECT merged_into_id FROM archetypes WHERE id = {child}")
            ),
            other
        );
    }

    /// Overlay spec 7.3: an archetype the API named that v1 does not hold
    /// becomes a `downloaded` row known only by uuid, which the archetypes
    /// worker's lazy definition download fills (`undefined_uuids`).
    #[test]
    fn a_stub_is_a_downloaded_row_waiting_for_its_definition() {
        let (_dir, mut conn) = db("archetype-edit-stub");
        insert_archetype(&conn, "q-held", "Qheld", Some("modern"), "manual");
        let stub = |uuid: &str, name: &str| ArchetypeEdit::EnsureStub {
            uuid: uuid.to_string(),
            name: name.to_string(),
            format: "modern".to_string(),
            color_identity: Some("UB".to_string()),
            now_ms: 7_000,
        };
        assert_eq!(
            edit_archetypes(&mut conn, &stub("q-unmet", "Qunmet")).expect("edit"),
            done(Vec::new(), Vec::new())
        );
        assert_eq!(
            head(&conn, "q-unmet"),
            (
                "Qunmet".to_string(),
                Some("modern".to_string()),
                Some("U,B".to_string()),
                None
            )
        );
        assert_eq!(
            crate::archetypes::undefined_uuids(&conn).expect("read"),
            vec!["q-unmet".to_string()]
        );
        // A held uuid is left alone, and a second stub changes nothing.
        edit_archetypes(&mut conn, &stub("q-held", "Qrenamed")).expect("edit");
        edit_archetypes(&mut conn, &stub("q-unmet", "Qagain")).expect("edit");
        assert_eq!(head(&conn, "q-held").0, "Qheld");
        assert_eq!(head(&conn, "q-unmet").0, "Qunmet");
        assert_eq!(
            number(
                &conn,
                "SELECT COUNT(*) FROM archetypes WHERE uuid LIKE 'q-%'"
            ),
            2
        );
        // A blank name is no name: the uuid stands in until the definition.
        edit_archetypes(&mut conn, &stub("q-nameless", "  ")).expect("edit");
        assert_eq!(head(&conn, "q-nameless").0, "q-nameless");
    }

    /// #489: a held downloaded archetype the server lists as active again
    /// (a stale check retired it) comes back when the player picks it.
    #[test]
    fn a_reinstated_archetype_is_no_longer_retired() {
        let (_dir, mut conn) = db("archetype-edit-reinstate");
        insert_archetype(&conn, "q-old", "Qold", Some("modern"), "downloaded");
        insert_archetype(&conn, "q-mine", "Qmine", Some("modern"), "manual");
        conn.execute_batch("UPDATE archetypes SET retired_at = 5")
            .expect("retire");
        let reinstate = |uuid: &str| ArchetypeEdit::Reinstate {
            uuid: uuid.to_string(),
            now_ms: 7_000,
        };
        edit_archetypes(&mut conn, &reinstate("q-old")).expect("edit");
        edit_archetypes(&mut conn, &reinstate("q-mine")).expect("edit");
        assert_eq!(
            number(
                &conn,
                "SELECT COUNT(*) FROM archetypes WHERE uuid = 'q-old' AND retired_at IS NULL"
            ),
            1
        );
        assert_eq!(
            number(
                &conn,
                "SELECT COUNT(*) FROM archetypes WHERE uuid = 'q-mine' AND retired_at IS NULL"
            ),
            0,
            "only downloaded rows"
        );
    }

    fn offline_held(conn: &Connection, uuid: &str) -> (String, i64, bool) {
        conn.query_row(
            "SELECT source, offline_held, definition IS NOT NULL FROM archetypes WHERE uuid = ?1",
            [uuid],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("row")
    }

    fn hold(
        conn: &mut Connection,
        definitions: Vec<archetypes::Definition>,
    ) -> ArchetypeEditOutcome {
        edit_archetypes(
            conn,
            &ArchetypeEdit::HoldOffline {
                definitions,
                now_ms: 9_000,
            },
        )
        .expect("hold")
    }

    /// The offline download (2026-10-27 design section 2): an unheld uuid is
    /// added as Custom, a held downloaded row becomes Custom in place, the
    /// player's own Custom rows and the fallbacks are never written.
    #[test]
    fn hold_offline_adds_converts_and_skips() {
        let (_dir, mut conn) = db("archetype-edit-hold-offline");
        let met = insert_archetype(&conn, "q-met", "Qmet", Some("modern"), "downloaded");
        conn.execute(
            "UPDATE archetypes SET definition = '{}', definition_version = 'v1' WHERE id = ?1",
            [met],
        )
        .expect("definition");
        insert_match(&conn, 1, "log", 100);
        seat(&conn, 1, true, met);
        insert_archetype(&conn, "q-own", "Qown", Some("modern"), "manual");

        let outcome = hold(
            &mut conn,
            vec![
                definition("q-new", "Qnew", "v1"),
                definition("q-met", "Qmet renamed", "v2"),
                definition("q-own", "Qserver", "v1"),
            ],
        );
        assert_eq!(
            outcome,
            ArchetypeEditOutcome::OfflineHeld(OfflineHeld {
                added: 1,
                converted: 1,
                skipped: 1,
            })
        );
        assert_eq!(
            offline_held(&conn, "q-new"),
            ("manual".to_string(), 1, false)
        );
        assert_eq!(head(&conn, "q-new").1, Some("modern".to_string()));
        assert_eq!(list(&conn, "q-new").len(), 2, "the key list as list 1");
        assert_eq!(
            offline_held(&conn, "q-met"),
            ("manual".to_string(), 1, false)
        );
        assert_eq!(id_of(&conn, "q-met"), Some(met), "converted in place");
        assert_eq!(head(&conn, "q-met").0, "Qmet renamed");
        assert_eq!(head(&conn, "q-met").3, None, "no definition version");
        assert_eq!(
            number(
                &conn,
                "SELECT archetype_id FROM match_archetypes WHERE is_opponent = 1"
            ),
            met,
            "its matches keep it"
        );
        assert_eq!(
            offline_held(&conn, "q-own"),
            ("manual".to_string(), 0, false)
        );
        assert_eq!(head(&conn, "q-own").0, "Qown");
        assert!(list(&conn, "q-own").is_empty());
    }

    /// Pressing Download again never writes over a row the player owns now.
    #[test]
    fn holding_again_never_rewrites_an_offline_held_row() {
        let (_dir, mut conn) = db("archetype-edit-hold-again");
        hold(&mut conn, vec![definition("q-new", "Qnew", "v1")]);
        let id = id_of(&conn, "q-new").expect("held");
        conn.execute(
            "UPDATE archetypes SET name = 'Qmine now' WHERE id = ?1",
            [id],
        )
        .expect("rename");
        conn.execute(
            "INSERT INTO archetype_cards (archetype_id, list, catalog_id, quantity, sideboard)
             VALUES (?1, 2, 777, 4, 0)",
            [id],
        )
        .expect("variant");
        let outcome = hold(&mut conn, vec![definition("q-new", "Qserver", "v2")]);
        assert_eq!(
            outcome,
            ArchetypeEditOutcome::OfflineHeld(OfflineHeld {
                added: 0,
                converted: 0,
                skipped: 1,
            })
        );
        assert_eq!(head(&conn, "q-new").0, "Qmine now");
        assert_eq!(list(&conn, "q-new").len(), 3, "list 1 and the variant");
    }

    fn release(conn: &mut Connection) {
        edit_archetypes(conn, &ArchetypeEdit::ReleaseOffline { now_ms: 9_500 }).expect("release");
    }

    fn merged_into(conn: &Connection, uuid: &str) -> Option<i64> {
        conn.query_row(
            "SELECT merged_into_id FROM archetypes WHERE uuid = ?1",
            [uuid],
            |row| row.get(0),
        )
        .expect("merge")
    }

    /// Going online (2026-10-27 design section 5): an offline-held row is a
    /// downloaded stub again, its lists gone, for the definitions check to
    /// fill; its matches keep it.
    #[test]
    fn release_offline_returns_rows_to_server_stubs() {
        let (_dir, mut conn) = db("archetype-edit-release");
        hold(&mut conn, vec![definition("q-a", "Qa", "v1")]);
        let id = id_of(&conn, "q-a").expect("held");
        conn.execute(
            "INSERT INTO archetype_cards (archetype_id, list, catalog_id, quantity, sideboard)
             VALUES (?1, 2, 777, 4, 0)",
            [id],
        )
        .expect("variant");
        insert_match(&conn, 1, "log", 100);
        seat(&conn, 1, true, id);
        release(&mut conn);
        assert_eq!(
            offline_held(&conn, "q-a"),
            ("downloaded".to_string(), 0, false)
        );
        assert_eq!(id_of(&conn, "q-a"), Some(id));
        assert!(list(&conn, "q-a").is_empty(), "every list is gone");
        assert_eq!(head(&conn, "q-a").3, None);
        assert_eq!(
            number(
                &conn,
                "SELECT archetype_id FROM match_archetypes WHERE match_id = 1"
            ),
            id
        );
        assert_eq!(
            crate::archetypes::undefined_uuids(&conn).expect("undefined"),
            ["q-a".to_string()]
        );
        assert!(
            crate::archetypes::downloaded_versions(&conn)
                .expect("versions")
                .contains(&("q-a".to_string(), None))
        );
    }

    /// Merges survive going online (review I3): a merge may predate
    /// offline mode, and online a downloaded row may be merged either way.
    #[test]
    fn release_offline_keeps_merges_around_released_rows() {
        let (_dir, mut conn) = db("archetype-edit-release-merges");
        hold(
            &mut conn,
            vec![definition("q-a", "Qa", "v1"), definition("q-b", "Qb", "v1")],
        );
        let a = id_of(&conn, "q-a").expect("a");
        insert_archetype(&conn, "q-own", "Qown", Some("modern"), "manual");
        conn.execute(
            "UPDATE archetypes SET merged_into_id = ?1 WHERE uuid IN ('q-own', 'q-b')",
            [a],
        )
        .expect("merge");
        release(&mut conn);
        assert_eq!(merged_into(&conn, "q-own"), Some(a));
        assert_eq!(merged_into(&conn, "q-b"), Some(a));
        assert_eq!(
            offline_held(&conn, "q-own"),
            ("manual".to_string(), 0, false)
        );
    }

    #[test]
    fn release_offline_leaves_custom_rows_alone() {
        let (_dir, mut conn) = db("archetype-edit-release-custom");
        let own = insert_archetype(&conn, "q-own", "Qown", Some("modern"), "manual");
        conn.execute(
            "INSERT INTO archetype_cards (archetype_id, list, catalog_id, quantity, sideboard)
             VALUES (?1, 1, 777, 4, 0)",
            [own],
        )
        .expect("list");
        release(&mut conn);
        assert_eq!(
            offline_held(&conn, "q-own"),
            ("manual".to_string(), 0, false)
        );
        assert_eq!(list(&conn, "q-own").len(), 1);
    }

    #[test]
    fn clear_all_detected_queues_every_detected_match() {
        let (_dir, mut conn) = db("archetype-edit-clear-all");
        for id in [1, 2, 3] {
            insert_match(&conn, id, "log", id * 100);
        }
        conn.execute(
            "UPDATE matches SET archetypes_detected_at = 5 WHERE id IN (1, 3)",
            [],
        )
        .expect("detected");
        assert_eq!(
            edit_archetypes(&mut conn, &ArchetypeEdit::ClearAllDetected).expect("clear"),
            done(vec![1, 3], Vec::new())
        );
        assert_eq!(marker(&conn, 1), None);
        assert_eq!(marker(&conn, 3), None);
    }

    #[test]
    fn ensure_commander_is_idempotent_and_follows_a_merge() {
        let (_dir, mut conn) = db("archetype-edit-commander");
        let ensure = |conn: &mut Connection, now_ms: i64| {
            edit_archetypes(
                conn,
                &ArchetypeEdit::EnsureCommander {
                    uuid: "q-cmd-uuid".to_string(),
                    name: "Qcommander".to_string(),
                    format: "historic_brawl".to_string(),
                    color_identity: Some("U,R".to_string()),
                    now_ms,
                },
            )
            .expect("edit")
        };
        let ArchetypeEditOutcome::Commander { archetype_id } = ensure(&mut conn, 10) else {
            panic!("a commander outcome");
        };
        assert_eq!(
            ensure(&mut conn, 20),
            ArchetypeEditOutcome::Commander { archetype_id }
        );
        assert_eq!(
            number(
                &conn,
                "SELECT COUNT(*) FROM archetypes WHERE source = 'commander'"
            ),
            1
        );
        assert_eq!(
            number(
                &conn,
                "SELECT created_at * 1000 + updated_at FROM archetypes WHERE source = 'commander'"
            ),
            10_010
        );
        let target = insert_archetype(
            &conn,
            "q-target",
            "Qtarget",
            Some("historic_brawl"),
            "manual",
        );
        conn.execute(
            "UPDATE archetypes SET merged_into_id = ?1 WHERE id = ?2",
            params![target, archetype_id],
        )
        .expect("merge");
        assert_eq!(
            ensure(&mut conn, 30),
            ArchetypeEditOutcome::Commander {
                archetype_id: target
            }
        );
    }

    #[test]
    fn detection_input_carries_commanders_for_a_commander_format_only() {
        use crate::archetypes::detection_input;
        let (_dir, conn) = db("archetype-edit-commander-input");
        conn.execute_batch(
            "INSERT INTO cards (catalog_id, oracle_id, name, color_identity)
                 VALUES (5001, 'q-a', 'Qa', 'R');",
        )
        .expect("card");
        insert_match(&conn, 1, "log", 100);
        insert_match(&conn, 2, "log", 200);
        insert_match(&conn, 3, "log", 300);
        conn.execute_batch(
            "UPDATE matches SET games_synced_at = 1;
             UPDATE matches SET format = 'ARENA_HISTORIC_BRAWL' WHERE id = 1;
             UPDATE matches SET format = 'ARENA_HISTORIC' WHERE id = 2;
             UPDATE matches SET format = 'CBRAWL' WHERE id = 3;
             INSERT INTO match_commanders VALUES (1, 'player', 0, 5001);",
        )
        .expect("setup");
        let brawl = detection_input(&conn, 1).expect("read").expect("ready");
        let seats = brawl.commanders.expect("commanders");
        assert_eq!(
            seats.player.expect("player").oracle_ids,
            vec!["q-a".to_string()]
        );
        assert_eq!(seats.opponent, None);
        let other = detection_input(&conn, 2).expect("read").expect("ready");
        assert_eq!(other.commanders, None);
        // MTGO Brawl keys as `brawl` but is not an Arena commander format.
        let mtgo_brawl = detection_input(&conn, 3).expect("read").expect("ready");
        assert_eq!(mtgo_brawl.commanders, None);
        conn.execute("UPDATE cards SET oracle_id = NULL", [])
            .expect("unresolve");
        assert_eq!(detection_input(&conn, 1).expect("read"), None, "not ready");
    }

    /// A result is stale when the deck archetype it read was cleared meanwhile,
    /// even though a non-commander result carries no new deck archetype.
    #[test]
    fn a_result_is_dropped_when_the_deck_archetype_was_cleared_meanwhile() {
        use crate::archetypes::detection_input;
        let (_dir, mut conn) = db("archetype-edit-stale-clear");
        let a = insert_archetype(&conn, "q-a", "Qa", Some("modern"), "manual");
        insert_deck(&conn, 1, "CMODERN");
        insert_match(&conn, 1, "log", 100);
        conn.execute(
            "UPDATE matches SET deck_version_id = 10, games_synced_at = 1",
            [],
        )
        .expect("link");
        conn.execute("UPDATE decks SET archetype_id = ?1", [a])
            .expect("deck");
        let input = detection_input(&conn, 1).expect("read").expect("ready");
        assert_eq!(input.deck_archetype_id, Some(a));
        conn.execute("UPDATE decks SET archetype_id = NULL", [])
            .expect("cleared");
        let late = ArchetypeEdit::SetDetections {
            results: vec![MatchDetection {
                match_id: 1,
                deck_archetype_id: input.deck_archetype_id,
                deck_archetype: None,
                player: Some(SeatResult {
                    archetype_id: a,
                    confidence: 1.0,
                }),
                opponent: None,
            }],
            now_ms: 9,
        };
        assert_eq!(
            edit_archetypes(&mut conn, &late).expect("edit"),
            done(Vec::new(), Vec::new())
        );
        assert_eq!(marker(&conn, 1), None);
    }
}
