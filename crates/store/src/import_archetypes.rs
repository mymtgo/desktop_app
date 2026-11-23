//! 0.x's archetype data imported into v1 (archetypes spec sections 15 and
//! 21.5):
//! - **archetypes**: every manual row, and every row an imported match row,
//!   deck link, sideboard guide or matchup note references, with the
//!   parents of their merges; not the rest of 0.x's catalog (ruling 4).
//!   0.x's fallbacks map to v1's seeds by uuid. A downloaded row keeps
//!   0.x's name, format and colours with no definition, so the next change
//!   check fetches it. Upsert by uuid: a row v1 holds is left alone, except
//!   a manual row v1 has not edited since (its `updated_at` is not newer
//!   than 0.x's), which takes 0.x's name, format, colours, merge and source
//!   match again;
//! - **manual lists**: a new manual archetype's variants in 0.x id order
//!   become lists 1, 2, ...; one without a variant takes its legacy card
//!   list as list 1 (0.x `backfill_manual_archetype_variants`). Lists are
//!   written only when the archetype is added, so a rerun never touches a
//!   v1 list edit (and does not bring variants 0.x added since);
//! - **match rows**: a hand-set 0.x row is written unless v1's seat is hand
//!   set too (or the player chose "none" for the opponent in v1); a
//!   detected one only where v1 has no row and has not decided the seat. A
//!   marker set at or after the import's start (detection that ran during
//!   the import) is no decision: its auto rows give way to 0.x's. A match
//!   that gets a row and ends with an opponent row is marked detected at
//!   the import time; the rest are detected by the queue;
//! - **decks**: `decks.archetype_id` where v1's is NULL;
//! - **guides and notes**: a pair v1 already has a guide for keeps v1's
//!   guide and cards; notes are added once per `(deck, archetype,
//!   created_at, body)`; a noted pair without a guide gets an empty one (0.x
//!   backfill migration).
//!
//! Every step only adds or fills, so a rerun adds and changes nothing that
//! v1 owns.

use std::collections::{BTreeSet, HashMap};

use rusqlite::{OptionalExtension, Transaction, params};

use crate::{Connection, StoreError};

/// What kind of row a 0.x archetype is (0.x `manual`, `is_fallback`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LegacySource {
    /// From 0.x's catalog download (or a sync stub): the server's.
    Downloaded,
    /// Made by the player.
    Manual,
    /// Homebrew or Rogue.
    Fallback,
}

/// One card of a manual archetype's list, by catalog id (0.x
/// `cards.mtgo_id`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyListCard {
    pub catalog_id: i64,
    pub oracle_id: Option<String>,
    pub quantity: i64,
    pub sideboard: bool,
}

/// One 0.x `archetypes` row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyArchetype {
    /// 0.x's row id, which the other rows refer to.
    pub legacy_id: i64,
    pub uuid: String,
    pub name: String,
    /// 0.x's format (a key such as `modern`).
    pub format: Option<String>,
    /// The format of a match or deck that uses it, for a row 0.x stored
    /// without one (a sync stub); an MTGO code or a key.
    pub format_hint: Option<String>,
    pub color_identity: Option<String>,
    pub source: LegacySource,
    /// The 0.x id of the archetype it is merged into.
    pub merged_into: Option<i64>,
    /// The token of the match it was made from.
    pub source_match_token: Option<String>,
    pub incomplete: bool,
    pub created_at_ms: Option<i64>,
    pub updated_at_ms: Option<i64>,
    /// A manual archetype's lists, list 1 first (empty for the others).
    pub lists: Vec<Vec<LegacyListCard>>,
}

/// One 0.x `match_archetypes` row with its seat (the first row per match
/// and seat).
#[derive(Debug, Clone, PartialEq)]
pub struct LegacyMatchArchetype {
    pub match_token: String,
    pub mtgo_match_id: Option<i64>,
    pub is_opponent: bool,
    /// The 0.x archetype id.
    pub archetype: i64,
    pub confidence: f64,
    /// Set by hand.
    pub manual: bool,
}

/// A 0.x deck's archetype (`decks.archetype_id`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyDeckArchetype {
    pub deck_mtgo_id: String,
    pub archetype: i64,
}

/// One card of a 0.x sideboard plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyGuideCard {
    pub oracle_id: String,
    pub direction: String,
    pub quantity: i64,
}

/// One 0.x `sideboard_guides` row with its cards.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyGuide {
    pub deck_mtgo_id: String,
    pub archetype: i64,
    pub created_at_ms: Option<i64>,
    pub updated_at_ms: Option<i64>,
    pub cards: Vec<LegacyGuideCard>,
}

/// One 0.x `deck_archetype_notes` row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyNote {
    pub deck_mtgo_id: String,
    pub archetype: i64,
    pub body: String,
    pub created_at_ms: Option<i64>,
}

/// Everything 0.x knew about archetypes, guides and notes.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ImportedArchetypes {
    /// Every 0.x archetype, in id order; the import picks the ones it needs.
    pub archetypes: Vec<LegacyArchetype>,
    pub match_rows: Vec<LegacyMatchArchetype>,
    /// 0.x match rows whose player sat at no game of the match.
    pub rows_without_seat: u32,
    /// When this import began, UTC unix ms. A match detected at or after it
    /// was detected during the import (before its 0.x rows landed), so its
    /// marker and auto rows are not a v1 decision. `None`: every marker
    /// counts.
    pub import_started_at_ms: Option<i64>,
    pub deck_links: Vec<LegacyDeckArchetype>,
    pub guides: Vec<LegacyGuide>,
    pub notes: Vec<LegacyNote>,
}

/// What one archetype import changed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ArchetypeImportCounts {
    pub archetypes_added: u32,
    /// Manual archetypes that took 0.x's newer name, format or colours.
    pub archetypes_updated: u32,
    /// Held already (the fallbacks, v1's own or an earlier import's).
    pub archetypes_unchanged: u32,
    pub match_rows_added: u32,
    /// v1's row (or the player's "none") stayed.
    pub match_rows_kept: u32,
    /// Of a match v1 does not have, without a seat, or of an archetype not
    /// imported.
    pub match_rows_skipped: u32,
    /// Matches marked detected by the import.
    pub matches_detected: u32,
    pub decks_linked: u32,
    /// Guides added, empty ones for noted pairs included.
    pub guides_added: u32,
    /// 0.x guides of a pair v1 has a guide for.
    pub guides_kept: u32,
    pub guide_cards_added: u32,
    pub notes_added: u32,
    /// Notes v1 has already.
    pub notes_unchanged: u32,
    /// Notes of a deck or archetype not imported, or with an empty or too
    /// long body.
    pub notes_skipped: u32,
}

/// How long a note may be (`deck_archetype_notes` CHECK).
const NOTE_MAX_CHARS: usize = 2000;
/// How long an oracle id may be (`sideboard_guide_cards` CHECK).
const ORACLE_MAX_CHARS: usize = 64;

/// What the upsert did with one archetype.
enum Upserted {
    /// v1's row, left as it is.
    Held(i64),
    /// Added, or a manual row v1 left alone that follows 0.x again: its
    /// merge and source match are set after every row has its id.
    Written(i64),
    /// Not importable (a fallback v1 does not seed, a row without any
    /// format).
    Missing,
}

/// Import 0.x's archetype data in one transaction (spec sections 15 and
/// 21.5). Send it after the matches and decks, so their rows are found.
pub fn import_archetypes(
    conn: &mut Connection,
    data: &ImportedArchetypes,
    now_ms: i64,
) -> Result<ArchetypeImportCounts, StoreError> {
    let tx = conn.transaction()?;
    let mut counts = ArchetypeImportCounts {
        match_rows_skipped: data.rows_without_seat,
        ..ArchetypeImportCounts::default()
    };

    // The v1 rows each 0.x row belongs to.
    let mut seats: Vec<(i64, &LegacyMatchArchetype)> = Vec::new();
    for row in &data.match_rows {
        match v1_match(&tx, &row.match_token, row.mtgo_match_id)? {
            Some(match_id) => seats.push((match_id, row)),
            None => counts.match_rows_skipped += 1,
        }
    }
    let mut links: Vec<(i64, i64)> = Vec::new();
    for link in &data.deck_links {
        if let Some(deck_id) = v1_deck(&tx, &link.deck_mtgo_id)? {
            links.push((deck_id, link.archetype));
        }
    }
    let mut guides: Vec<(i64, &LegacyGuide)> = Vec::new();
    for guide in &data.guides {
        if let Some(deck_id) = v1_deck(&tx, &guide.deck_mtgo_id)? {
            guides.push((deck_id, guide));
        }
    }
    let mut notes: Vec<(i64, &LegacyNote)> = Vec::new();
    for note in &data.notes {
        match v1_deck(&tx, &note.deck_mtgo_id)? {
            Some(deck_id) => notes.push((deck_id, note)),
            None => counts.notes_skipped += 1,
        }
    }

    // The archetypes those rows need, every manual one, and the parents of
    // their merges.
    let by_legacy: HashMap<i64, &LegacyArchetype> = data
        .archetypes
        .iter()
        .map(|archetype| (archetype.legacy_id, archetype))
        .collect();
    let mut wanted: BTreeSet<i64> = BTreeSet::new();
    for archetype in &data.archetypes {
        if archetype.source == LegacySource::Manual {
            wanted.insert(archetype.legacy_id);
        }
    }
    wanted.extend(seats.iter().map(|(_, row)| row.archetype));
    wanted.extend(links.iter().map(|(_, archetype)| *archetype));
    wanted.extend(guides.iter().map(|(_, guide)| guide.archetype));
    wanted.extend(notes.iter().map(|(_, note)| note.archetype));
    let mut parents: Vec<i64> = wanted.iter().copied().collect();
    while let Some(legacy_id) = parents.pop() {
        if let Some(parent) = by_legacy.get(&legacy_id).and_then(|row| row.merged_into)
            && wanted.insert(parent)
        {
            parents.push(parent);
        }
    }

    let mut ids: HashMap<i64, i64> = HashMap::new();
    let mut written: Vec<(&LegacyArchetype, i64)> = Vec::new();
    for archetype in &data.archetypes {
        if !wanted.contains(&archetype.legacy_id) {
            continue;
        }
        match upsert_archetype(&tx, archetype, now_ms, &mut counts)? {
            Upserted::Held(id) => {
                ids.insert(archetype.legacy_id, id);
            }
            Upserted::Written(id) => {
                ids.insert(archetype.legacy_id, id);
                written.push((archetype, id));
            }
            Upserted::Missing => {}
        }
    }
    for (archetype, id) in written {
        let parent = archetype
            .merged_into
            .and_then(|legacy_id| ids.get(&legacy_id).copied())
            .filter(|parent| *parent != id);
        let source_match = match &archetype.source_match_token {
            Some(token) => tx
                .prepare_cached("SELECT id FROM matches WHERE token = ?1")?
                .query_row([token], |row| row.get::<_, i64>(0))
                .optional()?,
            None => None,
        };
        tx.prepare_cached(
            "UPDATE archetypes SET merged_into_id = ?2, source_match_id = ?3 WHERE id = ?1",
        )?
        .execute(params![id, parent, source_match])?;
    }

    import_match_rows(
        &tx,
        &seats,
        &ids,
        data.import_started_at_ms,
        now_ms,
        &mut counts,
    )?;

    for (deck_id, legacy_id) in links {
        if let Some(archetype_id) = ids.get(&legacy_id) {
            counts.decks_linked += tx
                .prepare_cached(
                    "UPDATE decks SET archetype_id = ?2 WHERE id = ?1 AND archetype_id IS NULL",
                )?
                .execute(params![deck_id, archetype_id])? as u32;
        }
    }

    for (deck_id, guide) in guides {
        if let Some(archetype_id) = ids.get(&guide.archetype) {
            import_guide(&tx, deck_id, *archetype_id, guide, now_ms, &mut counts)?;
        }
    }
    for (deck_id, note) in notes {
        match ids.get(&note.archetype) {
            Some(archetype_id) => {
                import_note(&tx, deck_id, *archetype_id, note, now_ms, &mut counts)?;
            }
            None => counts.notes_skipped += 1,
        }
    }

    tx.commit()?;
    Ok(counts)
}

/// The v1 match a 0.x match row belongs to: the row with its token, else
/// the live row with its MTGO match id (logs win, so v1 may track it under
/// another token).
fn v1_match(
    tx: &Transaction,
    token: &str,
    mtgo_match_id: Option<i64>,
) -> Result<Option<i64>, StoreError> {
    let by_token = tx
        .prepare_cached("SELECT id FROM matches WHERE token = ?1")?
        .query_row([token], |row| row.get::<_, i64>(0))
        .optional()?;
    if by_token.is_some() {
        return Ok(by_token);
    }
    let Some(live) = crate::import::live_row_token(tx, token, mtgo_match_id)? else {
        return Ok(None);
    };
    Ok(tx
        .prepare_cached("SELECT id FROM matches WHERE token = ?1")?
        .query_row([live], |row| row.get::<_, i64>(0))
        .optional()?)
}

fn v1_deck(tx: &Transaction, mtgo_id: &str) -> Result<Option<i64>, StoreError> {
    Ok(tx
        .prepare_cached("SELECT id FROM decks WHERE mtgo_id = ?1")?
        .query_row([mtgo_id], |row| row.get::<_, i64>(0))
        .optional()?)
}

/// Add one archetype, or find v1's (see the module doc for which rows
/// follow 0.x again).
fn upsert_archetype(
    tx: &Transaction,
    archetype: &LegacyArchetype,
    now_ms: i64,
    counts: &mut ArchetypeImportCounts,
) -> Result<Upserted, StoreError> {
    let existing: Option<(i64, String, i64)> = tx
        .prepare_cached("SELECT id, source, updated_at FROM archetypes WHERE uuid = ?1")?
        .query_row([&archetype.uuid], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .optional()?;
    if archetype.source == LegacySource::Fallback {
        return Ok(match existing {
            Some((id, source, _)) if source == "fallback" => {
                counts.archetypes_unchanged += 1;
                Upserted::Held(id)
            }
            _ => Upserted::Missing,
        });
    }
    let format = archetype
        .format
        .as_deref()
        .and_then(archetypes::format_key)
        .or_else(|| {
            archetype
                .format_hint
                .as_deref()
                .and_then(archetypes::format_key)
        });
    let colors = archetypes::normalize_colors(archetype.color_identity.as_deref());
    let updated_at = archetype
        .updated_at_ms
        .or(archetype.created_at_ms)
        .unwrap_or(now_ms);
    let created_at = archetype.created_at_ms.unwrap_or(updated_at);
    let incomplete = archetype.incomplete;

    if let Some((id, source, v1_updated_at)) = existing {
        let follows_0x = archetype.source == LegacySource::Manual
            && source == "manual"
            && v1_updated_at <= updated_at;
        let Some(format) = format.filter(|_| follows_0x) else {
            counts.archetypes_unchanged += 1;
            return Ok(Upserted::Held(id));
        };
        let changed = tx
            .prepare_cached(
                "UPDATE archetypes SET name = ?2, format = ?3, color_identity = ?4,
                     incomplete = ?5, updated_at = ?6
                 WHERE id = ?1 AND (name IS NOT ?2 OR format IS NOT ?3
                     OR color_identity IS NOT ?4 OR incomplete IS NOT ?5
                     OR updated_at IS NOT ?6)",
            )?
            .execute(params![
                id,
                archetype.name,
                format,
                colors,
                incomplete,
                updated_at
            ])?;
        if changed > 0 {
            counts.archetypes_updated += 1;
        } else {
            counts.archetypes_unchanged += 1;
        }
        return Ok(Upserted::Written(id));
    }

    let Some(format) = format else {
        return Ok(Upserted::Missing);
    };
    let source = match archetype.source {
        LegacySource::Manual => "manual",
        LegacySource::Downloaded | LegacySource::Fallback => "downloaded",
    };
    let id: i64 = tx
        .prepare_cached(
            "INSERT INTO archetypes (uuid, name, format, color_identity, source, incomplete,
                 created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             RETURNING id",
        )?
        .query_row(
            params![
                archetype.uuid,
                archetype.name,
                format,
                colors,
                source,
                incomplete,
                created_at,
                updated_at
            ],
            |row| row.get(0),
        )?;
    counts.archetypes_added += 1;
    if archetype.source == LegacySource::Manual {
        write_lists(tx, id, &archetype.lists)?;
    }
    Ok(Upserted::Written(id))
}

/// A new manual archetype's lists, numbered from 1; a list with no card v1
/// can hold is left out, so the numbers stay contiguous.
fn write_lists(
    tx: &Transaction,
    archetype_id: i64,
    lists: &[Vec<LegacyListCard>],
) -> Result<(), StoreError> {
    let mut list = 0;
    for cards in lists {
        let usable: Vec<&LegacyListCard> = cards
            .iter()
            .filter(|card| {
                card.catalog_id >= 1 && ::cards::storable(card.catalog_id) && card.quantity >= 1
            })
            .collect();
        if usable.is_empty() {
            continue;
        }
        list += 1;
        let refs: Vec<crate::cards::CardRef> = usable
            .iter()
            .map(|card| crate::cards::CardRef {
                catalog_id: card.catalog_id,
                name: None,
            })
            .collect();
        crate::cards::ensure_stubs(tx, &refs)?;
        let mut insert = tx.prepare_cached(
            "INSERT INTO archetype_cards (archetype_id, list, catalog_id, oracle_id, quantity,
                 sideboard)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT (archetype_id, list, catalog_id, sideboard)
             DO UPDATE SET quantity = archetype_cards.quantity + excluded.quantity",
        )?;
        for card in usable {
            insert.execute(params![
                archetype_id,
                list,
                card.catalog_id,
                card.oracle_id,
                card.quantity,
                card.sideboard
            ])?;
        }
    }
    Ok(())
}

/// The match rows (see the module doc), then the detected markers.
fn import_match_rows(
    tx: &Transaction,
    seats: &[(i64, &LegacyMatchArchetype)],
    ids: &HashMap<i64, i64>,
    started_at_ms: Option<i64>,
    now_ms: i64,
    counts: &mut ArchetypeImportCounts,
) -> Result<(), StoreError> {
    let mut touched: BTreeSet<i64> = BTreeSet::new();
    for (match_id, row) in seats {
        let Some(archetype_id) = ids.get(&row.archetype) else {
            counts.match_rows_skipped += 1;
            continue;
        };
        let held_manual: Option<bool> = tx
            .prepare_cached(
                "SELECT manual FROM match_archetypes WHERE match_id = ?1 AND is_opponent = ?2",
            )?
            .query_row(params![match_id, row.is_opponent], |found| found.get(0))
            .optional()?;
        // Detected during this import, before its 0.x rows landed: the
        // marker and the auto rows are not a v1 decision.
        let marker: Option<i64> = tx
            .prepare_cached("SELECT archetypes_detected_at FROM matches WHERE id = ?1")?
            .query_row([match_id], |found| found.get(0))?;
        let during_import = marker
            .zip(started_at_ms)
            .is_some_and(|(at, started)| at >= started);
        let write = match held_manual {
            // The player's "none" in v1 is input, whatever the marker says.
            _ if row.is_opponent && opponent_none(tx, *match_id)? => false,
            // The player's own choice in v1.
            Some(true) => false,
            Some(false) => row.manual || during_import,
            None => {
                during_import || !(row.is_opponent && opponent_decided(tx, *match_id, row.manual)?)
            }
        };
        if !write {
            counts.match_rows_kept += 1;
            continue;
        }
        let confidence = if row.confidence.is_finite() {
            row.confidence.clamp(0.0, 1.0)
        } else {
            0.0
        };
        tx.prepare_cached(
            "INSERT INTO match_archetypes (match_id, is_opponent, archetype_id, confidence, manual)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT (match_id, is_opponent) DO UPDATE SET
                 archetype_id = excluded.archetype_id, confidence = excluded.confidence,
                 manual = excluded.manual",
        )?
        .execute(params![
            match_id,
            row.is_opponent,
            archetype_id,
            confidence,
            row.manual
        ])?;
        counts.match_rows_added += 1;
        touched.insert(*match_id);
    }
    // Only a match with an opponent row: an empty opponent seat still
    // needs detecting.
    for match_id in touched {
        counts.matches_detected += tx
            .prepare_cached(
                "UPDATE matches SET archetypes_detected_at = ?2
                 WHERE id = ?1 AND archetypes_detected_at IS NULL
                   AND EXISTS (SELECT 1 FROM match_archetypes x
                               WHERE x.match_id = ?1 AND x.is_opponent = 1)",
            )?
            .execute(params![match_id, now_ms])? as u32;
    }
    Ok(())
}

/// The player chose "none" for the match's opponent
/// (`matches.opponent_archetype_none`, input, final review I2): no import
/// or pull writes an opponent row on it, hand-set or detected.
pub(crate) fn opponent_none(tx: &Transaction, match_id: i64) -> Result<bool, StoreError> {
    Ok(tx
        .prepare_cached("SELECT opponent_archetype_none = 1 FROM matches WHERE id = ?1")?
        .query_row([match_id], |row| row.get(0))
        .optional()?
        .unwrap_or(false))
}

/// v1 decided an empty opponent seat: the player chose "none"
/// (`opponent_none`), or, for a detected incoming row, the match is marked
/// detected (detection found nothing). A hand-set incoming row gives way
/// only to the player's "none": the flag is the one record of that choice
/// (#300 F3), so an empty seat with a marker (a deleted archetype took the
/// row, say) does not keep it out.
pub(crate) fn opponent_decided(
    tx: &Transaction,
    match_id: i64,
    manual: bool,
) -> Result<bool, StoreError> {
    if opponent_none(tx, match_id)? {
        return Ok(true);
    }
    if manual {
        return Ok(false);
    }
    Ok(tx
        .prepare_cached("SELECT archetypes_detected_at IS NOT NULL FROM matches WHERE id = ?1")?
        .query_row([match_id], |row| row.get(0))?)
}

/// One 0.x guide: added with its cards (rows breaking a CHECK left out)
/// unless v1 has a guide for the pair.
fn import_guide(
    tx: &Transaction,
    deck_id: i64,
    archetype_id: i64,
    guide: &LegacyGuide,
    now_ms: i64,
    counts: &mut ArchetypeImportCounts,
) -> Result<(), StoreError> {
    if pair_guide(tx, deck_id, archetype_id)?.is_some() {
        counts.guides_kept += 1;
        return Ok(());
    }
    let created_at = guide
        .created_at_ms
        .or(guide.updated_at_ms)
        .unwrap_or(now_ms);
    let updated_at = guide.updated_at_ms.unwrap_or(created_at);
    let guide_id = insert_guide(tx, deck_id, archetype_id, created_at, updated_at)?;
    counts.guides_added += 1;
    for card in &guide.cards {
        let oracle_chars = card.oracle_id.chars().count();
        let fits = (1..=ORACLE_MAX_CHARS).contains(&oracle_chars)
            && (card.direction == "in" || card.direction == "out")
            && (1..=99).contains(&card.quantity);
        if !fits {
            tracing::info!(
                direction = %card.direction,
                quantity = card.quantity,
                "a 0.x sideboard guide card was left out"
            );
            continue;
        }
        counts.guide_cards_added += tx
            .prepare_cached(
                "INSERT OR IGNORE INTO sideboard_guide_cards (guide_id, oracle_id, direction,
                     quantity)
                 VALUES (?1, ?2, ?3, ?4)",
            )?
            .execute(params![
                guide_id,
                card.oracle_id,
                card.direction,
                card.quantity
            ])? as u32;
    }
    Ok(())
}

/// One 0.x note, added once per `(deck, archetype, created_at, body)` (a
/// note without a time once per body); its pair gets an empty guide when it
/// has none.
fn import_note(
    tx: &Transaction,
    deck_id: i64,
    archetype_id: i64,
    note: &LegacyNote,
    now_ms: i64,
    counts: &mut ArchetypeImportCounts,
) -> Result<(), StoreError> {
    let chars = note.body.chars().count();
    if note.body.trim().is_empty() || chars > NOTE_MAX_CHARS {
        counts.notes_skipped += 1;
        return Ok(());
    }
    let held: bool = tx
        .prepare_cached(
            "SELECT EXISTS (SELECT 1 FROM deck_archetype_notes
                            WHERE deck_id = ?1 AND archetype_id = ?2 AND body = ?3
                              AND (?4 IS NULL OR created_at = ?4))",
        )?
        .query_row(
            params![deck_id, archetype_id, note.body, note.created_at_ms],
            |row| row.get(0),
        )?;
    if held {
        counts.notes_unchanged += 1;
    } else {
        tx.prepare_cached(
            "INSERT INTO deck_archetype_notes (deck_id, archetype_id, body, created_at)
             VALUES (?1, ?2, ?3, ?4)",
        )?
        .execute(params![
            deck_id,
            archetype_id,
            note.body,
            note.created_at_ms.unwrap_or(now_ms)
        ])?;
        counts.notes_added += 1;
    }
    if pair_guide(tx, deck_id, archetype_id)?.is_none() {
        insert_guide(tx, deck_id, archetype_id, now_ms, now_ms)?;
        counts.guides_added += 1;
    }
    Ok(())
}

fn pair_guide(
    tx: &Transaction,
    deck_id: i64,
    archetype_id: i64,
) -> Result<Option<i64>, StoreError> {
    Ok(tx
        .prepare_cached("SELECT id FROM sideboard_guides WHERE deck_id = ?1 AND archetype_id = ?2")?
        .query_row([deck_id, archetype_id], |row| row.get(0))
        .optional()?)
}

fn insert_guide(
    tx: &Transaction,
    deck_id: i64,
    archetype_id: i64,
    created_at: i64,
    updated_at: i64,
) -> Result<i64, StoreError> {
    Ok(tx
        .prepare_cached(
            "INSERT INTO sideboard_guides (deck_id, archetype_id, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4)
             RETURNING id",
        )?
        .query_row(
            params![deck_id, archetype_id, created_at, updated_at],
            |row| row.get(0),
        )?)
}
