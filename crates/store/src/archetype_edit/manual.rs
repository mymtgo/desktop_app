//! Manual archetypes (archetypes spec section 7, 0.x survey section 4):
//! create, edit, variants, reassign, merge, unmerge and delete. The guards
//! and messages are 0.x's (`archetypes::check_merge` and friends keep their
//! order); every refusal is decided before anything is written.

use archetypes::{GuardArchetype, ListCard};
use rusqlite::{OptionalExtension, Transaction, params};

use super::{ArchetypeEditOutcome, put_manual_row};
use crate::StoreError;

/// One card of a manual list as the form sends it (0.x `cards.*`).
#[derive(Debug, Clone, PartialEq)]
pub struct ManualCard {
    pub catalog_id: i64,
    /// The oracle id the resolution found; the card row's is used without
    /// one.
    pub oracle_id: Option<String>,
    /// The card's name, for a `cards` stub when v1 has no row for it.
    pub name: Option<String>,
    pub quantity: u32,
    pub sideboard: bool,
}

/// A manual archetype to create (0.x `StoreManualArchetype`).
#[derive(Debug, Clone, PartialEq)]
pub struct NewManualArchetype {
    /// `archetypes::manual_uuid`'s answer, minted by the caller.
    pub uuid: String,
    pub name: String,
    /// An archetypes format key; lower-cased here as 0.x does.
    pub format: String,
    pub color_identity: Option<String>,
    pub incomplete: bool,
    pub source_match_id: Option<i64>,
    pub cards: Vec<ManualCard>,
    pub now_ms: i64,
}

const FALLBACK_EDIT: &str = "Fallback archetypes cannot be edited.";
const MANUAL_ONLY_EDIT: &str = "Only manual archetypes can be edited.";
const MANUAL_ONLY_VARIANTS: &str = "Variants exist only for manual archetypes.";
const COMMANDER_NO_FORMAT: &str = "A commander archetype's format comes from its commander.";
const NOT_THIS_VARIANT: &str = "Variant does not belong to this archetype.";

fn refused(message: &str) -> ArchetypeEditOutcome {
    ArchetypeEditOutcome::Refused(message.to_string())
}

fn done(matches: Vec<i64>, decks: Vec<i64>) -> ArchetypeEditOutcome {
    ArchetypeEditOutcome::Done { matches, decks }
}

/// What the guards read of one archetype row.
#[derive(Debug, Clone)]
struct Head {
    guard: GuardArchetype,
    source: String,
}

fn head(tx: &Transaction, id: i64) -> Result<Option<Head>, StoreError> {
    let row: Option<(Option<String>, String, Option<i64>)> = tx
        .prepare_cached("SELECT format, source, merged_into_id FROM archetypes WHERE id = ?1")?
        .query_row([id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .optional()?;
    Ok(row.map(|(format, source, merged_into_id)| Head {
        guard: GuardArchetype {
            id,
            format,
            fallback: source == "fallback",
            merged_into_id,
        },
        source,
    }))
}

/// An archetype's lists in list order, each as `ListCard`s for the
/// variant hash.
fn lists_of(tx: &Transaction, id: i64) -> Result<Vec<(i64, Vec<ListCard>)>, StoreError> {
    let mut stmt = tx.prepare_cached(
        "SELECT list, catalog_id, oracle_id, quantity, sideboard FROM archetype_cards
         WHERE archetype_id = ?1 ORDER BY list, catalog_id, sideboard",
    )?;
    let mut rows = stmt.query([id])?;
    let mut out: Vec<(i64, Vec<ListCard>)> = Vec::new();
    while let Some(row) = rows.next()? {
        let list: i64 = row.get(0)?;
        let card = ListCard {
            catalog_id: row.get(1)?,
            oracle_id: row.get(2)?,
            card_type: None,
            quantity: u32::try_from(row.get::<_, i64>(3)?).unwrap_or(0),
            sideboard: row.get(4)?,
        };
        match out.last_mut() {
            Some((last, cards)) if *last == list => cards.push(card),
            _ => out.push((list, vec![card])),
        }
    }
    Ok(out)
}

/// The cards a list keeps (0.x `AddArchetypeVariant::resolvePivotData`):
/// each with an oracle id (the form's, else the card row's) and an id that
/// can be a card; a card listed twice on one board keeps its last row.
fn usable_cards(tx: &Transaction, cards: &[ManualCard]) -> Result<Vec<ListCard>, StoreError> {
    let mut oracle_of = tx.prepare_cached("SELECT oracle_id FROM cards WHERE catalog_id = ?1")?;
    let mut out: Vec<ListCard> = Vec::new();
    for card in cards {
        let plausible = card.catalog_id > 0 && cards::storable(card.catalog_id);
        if !plausible || card.quantity == 0 {
            continue;
        }
        let oracle_id = match &card.oracle_id {
            Some(oracle) if !oracle.is_empty() => Some(oracle.clone()),
            _ => oracle_of
                .query_row([card.catalog_id], |row| row.get::<_, Option<String>>(0))
                .optional()?
                .flatten(),
        };
        let Some(oracle_id) = oracle_id else {
            continue;
        };
        out.retain(|kept| {
            !(kept.catalog_id == card.catalog_id && kept.sideboard == card.sideboard)
        });
        out.push(ListCard {
            catalog_id: card.catalog_id,
            oracle_id: Some(oracle_id),
            card_type: None,
            quantity: card.quantity,
            sideboard: card.sideboard,
        });
    }
    Ok(out)
}

/// Write one list: a `cards` stub (with the form's name) for every id v1
/// has no row for, then the rows.
fn write_list(
    tx: &Transaction,
    archetype_id: i64,
    list: i64,
    cards: &[ListCard],
    names: &[ManualCard],
) -> Result<(), StoreError> {
    let refs: Vec<crate::cards::CardRef> = cards
        .iter()
        .map(|card| crate::cards::CardRef {
            catalog_id: card.catalog_id,
            name: names
                .iter()
                .find(|named| named.catalog_id == card.catalog_id)
                .and_then(|named| named.name.clone()),
        })
        .collect();
    crate::cards::ensure_stubs(tx, &refs)?;
    let mut insert = tx.prepare_cached(
        "INSERT INTO archetype_cards (archetype_id, list, catalog_id, oracle_id, quantity, sideboard)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
    )?;
    for card in cards {
        insert.execute(params![
            archetype_id,
            list,
            card.catalog_id,
            card.oracle_id,
            card.quantity,
            card.sideboard
        ])?;
    }
    Ok(())
}

fn next_list(tx: &Transaction, archetype_id: i64) -> Result<i64, StoreError> {
    Ok(tx
        .prepare_cached(
            "SELECT COALESCE(MAX(list), 0) + 1 FROM archetype_cards WHERE archetype_id = ?1",
        )?
        .query_row([archetype_id], |row| row.get(0))?)
}

pub(super) fn create(
    tx: &Transaction,
    new: &NewManualArchetype,
) -> Result<ArchetypeEditOutcome, StoreError> {
    let mut opponent_match = None;
    if let Some(match_id) = new.source_match_id {
        let has_opponent: Option<bool> = tx
            .prepare_cached("SELECT opponent_id IS NOT NULL FROM matches WHERE id = ?1")?
            .query_row([match_id], |row| row.get(0))
            .optional()?;
        match has_opponent {
            None => return Ok(refused("The selected source match id is invalid.")),
            Some(true) => opponent_match = Some(match_id),
            Some(false) => {}
        }
    }
    let cards = usable_cards(tx, &new.cards)?;
    tx.prepare_cached(
        "INSERT INTO archetypes (uuid, name, format, color_identity, source,
             source_match_id, incomplete, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, 'manual', ?5, ?6, ?7, ?7)",
    )?
    .execute(params![
        new.uuid,
        new.name,
        new.format.to_lowercase(),
        archetypes::normalize_colors(new.color_identity.as_deref()),
        new.source_match_id,
        new.incomplete,
        new.now_ms
    ])?;
    let id = tx.last_insert_rowid();
    if !cards.is_empty() {
        write_list(tx, id, 1, &cards, &new.cards)?;
    }
    let mut matches = Vec::new();
    if let Some(match_id) = opponent_match {
        put_manual_row(tx, match_id, true, id)?;
        matches.push(match_id);
    }
    Ok(done(matches, Vec::new()))
}

pub(super) fn update(
    tx: &Transaction,
    id: i64,
    name: &str,
    format: &str,
    color_identity: Option<&str>,
    variant: Option<&[ManualCard]>,
    now_ms: i64,
) -> Result<ArchetypeEditOutcome, StoreError> {
    let Some(row) = head(tx, id)? else {
        return Ok(ArchetypeEditOutcome::NotFound);
    };
    if row.guard.fallback {
        return Ok(refused(FALLBACK_EDIT));
    }
    if row.source == "commander" {
        return update_commander(tx, id, name, format, color_identity, variant, now_ms);
    }
    if row.source != "manual" {
        return Ok(refused(MANUAL_ONLY_EDIT));
    }
    let mut new_list = Vec::new();
    if let Some(cards) = variant {
        new_list = usable_cards(tx, cards)?;
        let existing: Vec<Vec<ListCard>> = lists_of(tx, id)?
            .into_iter()
            .map(|(_, list)| list)
            .collect();
        if !new_list.is_empty()
            && let Err(message) = archetypes::check_new_variant(&existing, &new_list)
        {
            return Ok(ArchetypeEditOutcome::Refused(message));
        }
    }
    tx.prepare_cached(
        "UPDATE archetypes SET name = ?2, format = ?3, color_identity = ?4, updated_at = ?5
         WHERE id = ?1",
    )?
    .execute(params![
        id,
        name,
        format.to_lowercase(),
        archetypes::normalize_colors(color_identity),
        now_ms
    ])?;
    if !new_list.is_empty() {
        let list = next_list(tx, id)?;
        write_list(tx, id, list, &new_list, variant.unwrap_or_default())?;
    }
    Ok(done(Vec::new(), Vec::new()))
}

/// A commander archetype's edit is its name only: its format and colours
/// come from its commanders and it has no variants.
fn update_commander(
    tx: &Transaction,
    id: i64,
    name: &str,
    format: &str,
    color_identity: Option<&str>,
    variant: Option<&[ManualCard]>,
    now_ms: i64,
) -> Result<ArchetypeEditOutcome, StoreError> {
    if variant.is_some() {
        return Ok(refused(MANUAL_ONLY_VARIANTS));
    }
    let (stored_format, stored_colours): (Option<String>, Option<String>) = tx
        .prepare_cached("SELECT format, color_identity FROM archetypes WHERE id = ?1")?
        .query_row([id], |row| Ok((row.get(0)?, row.get(1)?)))?;
    if archetypes::normalize_colors(color_identity)
        != archetypes::normalize_colors(stored_colours.as_deref())
    {
        return Ok(refused(archetypes::COMMANDER_NO_COLOURS));
    }
    if Some(format.to_lowercase()) != stored_format {
        return Ok(refused(COMMANDER_NO_FORMAT));
    }
    tx.prepare_cached("UPDATE archetypes SET name = ?2, updated_at = ?3 WHERE id = ?1")?
        .execute(params![id, name, now_ms])?;
    Ok(done(Vec::new(), Vec::new()))
}

pub(super) fn remove_variant(
    tx: &Transaction,
    id: i64,
    list: i64,
) -> Result<ArchetypeEditOutcome, StoreError> {
    let Some(row) = head(tx, id)? else {
        return Ok(ArchetypeEditOutcome::NotFound);
    };
    if row.guard.fallback {
        return Ok(refused(FALLBACK_EDIT));
    }
    if row.source != "manual" {
        return Ok(refused(MANUAL_ONLY_VARIANTS));
    }
    let lists = lists_of(tx, id)?;
    if !lists.iter().any(|(number, _)| *number == list) {
        return Ok(refused(NOT_THIS_VARIANT));
    }
    if lists.len() == 1 {
        return Ok(refused("Cannot delete the last variant."));
    }
    tx.prepare_cached("DELETE FROM archetype_cards WHERE archetype_id = ?1 AND list = ?2")?
        .execute(params![id, list])?;
    Ok(done(Vec::new(), Vec::new()))
}

pub(super) fn reassign_variant(
    tx: &Transaction,
    id: i64,
    list: i64,
    target_id: i64,
    now_ms: i64,
) -> Result<ArchetypeEditOutcome, StoreError> {
    let Some(source) = head(tx, id)? else {
        return Ok(ArchetypeEditOutcome::NotFound);
    };
    // 0.x's `exists` rule runs before the request's other checks.
    let Some(target) = head(tx, target_id)? else {
        return Ok(refused("The selected target id is invalid."));
    };
    let lists = lists_of(tx, id)?;
    if !lists.iter().any(|(number, _)| *number == list) {
        return Ok(refused(NOT_THIS_VARIANT));
    }
    let outcome = match archetypes::check_reassign(&source.guard, &target.guard, lists.len()) {
        Ok(outcome) => outcome,
        Err(message) => return Ok(ArchetypeEditOutcome::Refused(message)),
    };
    if source.source != "manual" || target.source != "manual" {
        return Ok(refused(MANUAL_ONLY_VARIANTS));
    }
    let new_list = next_list(tx, target_id)?;
    tx.prepare_cached(
        "UPDATE archetype_cards SET archetype_id = ?3, list = ?4
         WHERE archetype_id = ?1 AND list = ?2",
    )?
    .execute(params![id, list, target_id, new_list])?;
    if outcome.merge_source {
        // 0.x merges through `MergeArchetype`; its guards are the ones the
        // reassign just passed.
        set_merged_into(tx, id, Some(target_id), now_ms)?;
    }
    touch(tx, target_id, now_ms)?;
    Ok(done(Vec::new(), Vec::new()))
}

fn touch(tx: &Transaction, id: i64, now_ms: i64) -> Result<(), StoreError> {
    tx.prepare_cached("UPDATE archetypes SET updated_at = ?2 WHERE id = ?1")?
        .execute(params![id, now_ms])?;
    Ok(())
}

fn set_merged_into(
    tx: &Transaction,
    id: i64,
    parent: Option<i64>,
    now_ms: i64,
) -> Result<(), StoreError> {
    tx.prepare_cached("UPDATE archetypes SET merged_into_id = ?2, updated_at = ?3 WHERE id = ?1")?
        .execute(params![id, parent, now_ms])?;
    Ok(())
}

pub(super) fn merge(
    tx: &Transaction,
    source_id: i64,
    parent_id: i64,
    now_ms: i64,
) -> Result<ArchetypeEditOutcome, StoreError> {
    let Some(source) = head(tx, source_id)? else {
        return Ok(ArchetypeEditOutcome::NotFound);
    };
    // 0.x's request checks "itself" before it looks the parent up.
    let parent = if parent_id == source_id {
        Some(source.clone())
    } else {
        head(tx, parent_id)?
    };
    let Some(parent) = parent else {
        return Ok(refused("The selected parent id is invalid."));
    };
    if let Err(message) = archetypes::check_merge(&source.guard, &parent.guard) {
        return Ok(ArchetypeEditOutcome::Refused(message));
    }
    set_merged_into(tx, source_id, Some(parent_id), now_ms)?;
    Ok(done(Vec::new(), Vec::new()))
}

pub(super) fn unmerge(
    tx: &Transaction,
    source_id: i64,
    now_ms: i64,
) -> Result<ArchetypeEditOutcome, StoreError> {
    let Some(source) = head(tx, source_id)? else {
        return Ok(ArchetypeEditOutcome::NotFound);
    };
    if let Err(message) = archetypes::check_unmerge(&source.guard) {
        return Ok(ArchetypeEditOutcome::Refused(message));
    }
    set_merged_into(tx, source_id, None, now_ms)?;
    Ok(done(Vec::new(), Vec::new()))
}

fn ids(tx: &Transaction, sql: &str, id: i64) -> Result<Vec<i64>, StoreError> {
    let mut stmt = tx.prepare_cached(sql)?;
    let rows = stmt.query_map([id], |row| row.get(0))?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

pub(super) fn delete(tx: &Transaction, id: i64) -> Result<ArchetypeEditOutcome, StoreError> {
    let Some(row) = head(tx, id)? else {
        return Ok(ArchetypeEditOutcome::NotFound);
    };
    if row.guard.fallback {
        return Ok(refused("Fallback archetypes cannot be deleted."));
    }
    if row.source == "commander" {
        return Ok(refused(archetypes::COMMANDER_NO_DELETE));
    }
    if row.source != "manual" {
        return Ok(refused("Only manual archetypes can be deleted."));
    }
    let matches = ids(
        tx,
        "SELECT DISTINCT match_id FROM match_archetypes WHERE archetype_id = ?1 ORDER BY match_id",
        id,
    )?;
    let decks = ids(
        tx,
        "SELECT id FROM decks WHERE archetype_id = ?1 ORDER BY id",
        id,
    )?;
    // Match rows and lists go by cascade; deck links, merge children and
    // nothing else by SET NULL.
    tx.prepare_cached("DELETE FROM archetypes WHERE id = ?1")?
        .execute([id])?;
    // The matches it held are detected again, so they do not stay without
    // an archetype.
    for match_id in &matches {
        tx.prepare_cached("UPDATE matches SET archetypes_detected_at = NULL WHERE id = ?1")?
            .execute([match_id])?;
    }
    Ok(done(matches, decks))
}

#[cfg(test)]
mod tests {
    use super::super::*;
    use crate::archetypes::tests::{db, insert_archetype, insert_match, seat};
    use crate::test_support::number;
    use rusqlite::{Connection, params};

    const NOW: i64 = 1_000;

    fn card(conn: &Connection, catalog_id: i64, oracle: &str, name: &str) {
        conn.execute(
            "INSERT INTO cards (catalog_id, oracle_id, name, type, color_identity)
             VALUES (?1, ?2, ?3, 'Instant', 'R')",
            params![catalog_id, oracle, name],
        )
        .expect("card");
    }

    fn mc(catalog_id: i64, oracle: Option<&str>, quantity: u32, sideboard: bool) -> ManualCard {
        ManualCard {
            catalog_id,
            oracle_id: oracle.map(str::to_string),
            name: None,
            quantity,
            sideboard,
        }
    }

    fn new_manual(name: &str, cards: Vec<ManualCard>, source: Option<i64>) -> NewManualArchetype {
        NewManualArchetype {
            uuid: format!("abcdef12-q-{name}"),
            name: name.to_string(),
            format: "Modern".to_string(),
            color_identity: Some("R".to_string()),
            incomplete: source.is_some(),
            source_match_id: source,
            cards,
            now_ms: NOW,
        }
    }

    fn create(conn: &mut Connection, new: NewManualArchetype) -> (ArchetypeEditOutcome, i64) {
        let uuid = new.uuid.clone();
        let outcome =
            edit_archetypes(conn, &ArchetypeEdit::CreateManual(Box::new(new))).expect("create");
        let id = conn
            .query_row("SELECT id FROM archetypes WHERE uuid = ?1", [uuid], |row| {
                row.get(0)
            })
            .unwrap_or(0);
        (outcome, id)
    }

    /// `(list, catalog_id, oracle_id, quantity, sideboard)` by list and card.
    fn lists(conn: &Connection, id: i64) -> Vec<(i64, i64, Option<String>, i64, i64)> {
        let mut stmt = conn
            .prepare(
                "SELECT list, catalog_id, oracle_id, quantity, sideboard FROM archetype_cards
                 WHERE archetype_id = ?1 ORDER BY list, catalog_id, sideboard",
            )
            .expect("prepare");
        stmt.query_map([id], |row| {
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

    fn opponent_row(conn: &Connection, match_id: i64) -> Option<(i64, f64, i64)> {
        conn.query_row(
            "SELECT archetype_id, confidence, manual FROM match_archetypes
             WHERE match_id = ?1 AND is_opponent = 1",
            [match_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .ok()
    }

    fn done(matches: Vec<i64>, decks: Vec<i64>) -> ArchetypeEditOutcome {
        ArchetypeEditOutcome::Done { matches, decks }
    }

    fn refused(message: &str) -> ArchetypeEditOutcome {
        ArchetypeEditOutcome::Refused(message.to_string())
    }

    fn update(id: i64, name: &str, cards: Option<Vec<ManualCard>>) -> ArchetypeEdit {
        ArchetypeEdit::UpdateManual {
            id,
            name: name.to_string(),
            format: "modern".to_string(),
            color_identity: None,
            variant: cards,
            now_ms: NOW + 1,
        }
    }

    fn merged_into(conn: &Connection, id: i64) -> Option<i64> {
        conn.query_row(
            "SELECT merged_into_id FROM archetypes WHERE id = ?1",
            [id],
            |row| row.get(0),
        )
        .expect("merged_into_id")
    }

    // ---- StoreManualArchetypeTest ----

    #[test]
    fn creates_a_manual_archetype_with_cards() {
        let (_dir, mut conn) = db("manual-create");
        card(&conn, 12345, "oracle-bolt", "Qbolt");
        let (outcome, id) = create(
            &mut conn,
            new_manual(
                "Qburn",
                vec![mc(12345, Some("oracle-bolt"), 4, false)],
                None,
            ),
        );
        assert_eq!(outcome, done(Vec::new(), Vec::new()));
        let row = crate::archetypes::archetype(&conn, id)
            .expect("read")
            .expect("row");
        assert_eq!(row.source, archetypes::Source::Manual);
        assert_eq!(row.name, "Qburn");
        assert_eq!(row.format.as_deref(), Some("modern"));
        assert_eq!(row.color_identity.as_deref(), Some("R"));
        assert!(row.uuid.starts_with("abcdef12-"));
        assert_eq!((row.created_at, row.updated_at), (NOW, NOW));
        assert_eq!(
            lists(&conn, id),
            [(1, 12345, Some("oracle-bolt".to_string()), 4, 0)]
        );
    }

    #[test]
    fn persists_the_source_match_and_the_incomplete_flag() {
        let (_dir, mut conn) = db("manual-create-source");
        insert_match(&conn, 7, "log", 100);
        let (_, id) = create(&mut conn, new_manual("Qfrom", Vec::new(), Some(7)));
        let row = crate::archetypes::archetype(&conn, id)
            .expect("read")
            .expect("row");
        assert_eq!(row.source_match_id, Some(7));
        assert!(row.incomplete);
    }

    #[test]
    fn the_source_matchs_opponent_seat_becomes_the_new_archetype() {
        let (_dir, mut conn) = db("manual-create-link");
        insert_match(&conn, 7, "log", 100);
        let (outcome, id) = create(&mut conn, new_manual("Qfrom", Vec::new(), Some(7)));
        assert_eq!(outcome, done(vec![7], Vec::new()));
        assert_eq!(opponent_row(&conn, 7), Some((id, 1.0, 1)));
    }

    #[test]
    fn the_source_matchs_opponent_row_is_replaced_not_doubled() {
        let (_dir, mut conn) = db("manual-create-overwrite");
        insert_match(&conn, 7, "log", 100);
        let old = insert_archetype(&conn, "q-old", "Qold", Some("modern"), "downloaded");
        seat(&conn, 7, true, old);
        let (_, id) = create(&mut conn, new_manual("Qnew", Vec::new(), Some(7)));
        assert_eq!(
            number(
                &conn,
                "SELECT COUNT(*) FROM match_archetypes WHERE match_id = 7 AND is_opponent = 1"
            ),
            1
        );
        assert_eq!(opponent_row(&conn, 7), Some((id, 1.0, 1)));
    }

    #[test]
    fn creates_a_variant_holding_the_cards_so_the_archetype_renders() {
        let (_dir, mut conn) = db("manual-create-variant");
        card(&conn, 12345, "oracle-bolt", "Qbolt");
        // Cards carry no oracle id of their own: the card row's is copied.
        let (_, id) = create(
            &mut conn,
            new_manual("Qburn", vec![mc(12345, None, 4, false)], None),
        );
        assert_eq!(
            lists(&conn, id),
            [(1, 12345, Some("oracle-bolt".to_string()), 4, 0)]
        );
    }

    // 0.x "links the source match opponent to the created variant": v1 keeps
    // no variant link on match rows (ruling 9), so the seat names the
    // archetype whose list 1 is the variant.
    #[test]
    fn the_source_match_opponent_and_the_variant_belong_to_one_archetype() {
        let (_dir, mut conn) = db("manual-create-link-variant");
        card(&conn, 12345, "oracle-bolt", "Qbolt");
        insert_match(&conn, 7, "log", 100);
        let (_, id) = create(
            &mut conn,
            new_manual("Qburn", vec![mc(12345, None, 4, false)], Some(7)),
        );
        assert_eq!(opponent_row(&conn, 7).map(|row| row.0), Some(id));
        assert_eq!(lists(&conn, id).len(), 1);
    }

    #[test]
    fn a_source_match_that_does_not_exist_is_refused() {
        let (_dir, mut conn) = db("manual-create-missing-match");
        let (outcome, id) = create(&mut conn, new_manual("Qfrom", Vec::new(), Some(99)));
        assert_eq!(outcome, refused("The selected source match id is invalid."));
        assert_eq!(id, 0);
    }

    #[test]
    fn a_source_match_without_an_opponent_gets_no_row() {
        let (_dir, mut conn) = db("manual-create-no-opponent");
        insert_match(&conn, 7, "log", 100);
        conn.execute("UPDATE matches SET opponent_id = NULL", [])
            .expect("no opponent");
        let (outcome, _) = create(&mut conn, new_manual("Qfrom", Vec::new(), Some(7)));
        assert_eq!(outcome, done(Vec::new(), Vec::new()));
        assert_eq!(opponent_row(&conn, 7), None);
    }

    // ---- AddArchetypeVariantTest (through "Save & Add Variant") ----

    fn manual_with_list(conn: &mut Connection) -> i64 {
        card(conn, 1, "oracle-1", "Qone");
        card(conn, 2, "oracle-2", "Qtwo");
        create(
            conn,
            new_manual("Qbrew", vec![mc(1, Some("oracle-1"), 4, false)], None),
        )
        .1
    }

    #[test]
    fn a_new_variant_is_the_next_list_with_its_cards() {
        let (_dir, mut conn) = db("manual-variant-add");
        let id = manual_with_list(&mut conn);
        let outcome = edit_archetypes(
            &mut conn,
            &update(id, "Qbrew", Some(vec![mc(2, Some("oracle-2"), 3, false)])),
        )
        .expect("update");
        assert_eq!(outcome, done(Vec::new(), Vec::new()));
        assert_eq!(
            lists(&conn, id),
            [
                (1, 1, Some("oracle-1".to_string()), 4, 0),
                (2, 2, Some("oracle-2".to_string()), 3, 0)
            ]
        );
    }

    #[test]
    fn a_variant_skips_cards_with_no_oracle_id() {
        let (_dir, mut conn) = db("manual-variant-no-oracle");
        let id = manual_with_list(&mut conn);
        // 777 has no card row and no oracle id of its own.
        edit_archetypes(
            &mut conn,
            &update(
                id,
                "Qbrew",
                Some(vec![mc(777, None, 4, false), mc(2, None, 1, false)]),
            ),
        )
        .expect("update");
        assert_eq!(
            lists(&conn, id)[1..],
            [(2, 2, Some("oracle-2".to_string()), 1, 0)]
        );
    }

    // 0.x "skips cards whose oracle_id has no Card row": v1 keys cards by
    // catalog id, and an id that cannot be a card gets no card row.
    #[test]
    fn a_variant_skips_cards_that_cannot_have_a_card_row() {
        let (_dir, mut conn) = db("manual-variant-implausible");
        let id = manual_with_list(&mut conn);
        edit_archetypes(
            &mut conn,
            &update(
                id,
                "Qbrew",
                Some(vec![
                    mc(5_000_000, Some("oracle-x"), 4, false),
                    mc(0, Some("oracle-y"), 4, false),
                    mc(2, None, 1, false),
                ]),
            ),
        )
        .expect("update");
        assert_eq!(
            lists(&conn, id)[1..],
            [(2, 2, Some("oracle-2".to_string()), 1, 0)]
        );
    }

    // Also UpdateControllerTest "returns a validation error for a duplicate
    // variant": refused as a whole, the name included.
    #[test]
    fn an_identical_variant_is_refused_and_nothing_changes() {
        let (_dir, mut conn) = db("manual-variant-duplicate");
        let id = manual_with_list(&mut conn);
        let outcome = edit_archetypes(
            &mut conn,
            &update(id, "Qrenamed", Some(vec![mc(1, None, 4, false)])),
        )
        .expect("update");
        assert_eq!(outcome, refused("This variant matches an existing one."));
        assert_eq!(lists(&conn, id).len(), 1);
        let name: String = conn
            .query_row("SELECT name FROM archetypes WHERE id = ?1", [id], |row| {
                row.get(0)
            })
            .expect("name");
        assert_eq!(name, "Qbrew");
    }

    // Also UpdateControllerTest "allows two variants that differ only in
    // quantity".
    #[test]
    fn two_variants_may_differ_only_in_quantity() {
        let (_dir, mut conn) = db("manual-variant-quantity");
        let id = manual_with_list(&mut conn);
        let outcome = edit_archetypes(
            &mut conn,
            &update(id, "Qbrew", Some(vec![mc(1, None, 3, false)])),
        )
        .expect("update");
        assert_eq!(outcome, done(Vec::new(), Vec::new()));
        assert_eq!(lists(&conn, id).len(), 2);
    }

    #[test]
    fn a_sideboard_difference_is_a_distinct_variant() {
        let (_dir, mut conn) = db("manual-variant-sideboard");
        let id = manual_with_list(&mut conn);
        let outcome = edit_archetypes(
            &mut conn,
            &update(id, "Qbrew", Some(vec![mc(1, None, 4, true)])),
        )
        .expect("update");
        assert_eq!(outcome, done(Vec::new(), Vec::new()));
        assert_eq!(
            lists(&conn, id)[1],
            (2, 1, Some("oracle-1".to_string()), 4, 1)
        );
    }

    #[test]
    fn a_card_in_both_the_maindeck_and_the_sideboard_keeps_both_rows() {
        let (_dir, mut conn) = db("manual-variant-both-boards");
        let id = manual_with_list(&mut conn);
        edit_archetypes(
            &mut conn,
            &update(
                id,
                "Qbrew",
                Some(vec![mc(2, None, 2, false), mc(2, None, 1, true)]),
            ),
        )
        .expect("update");
        assert_eq!(
            lists(&conn, id)[1..],
            [
                (2, 2, Some("oracle-2".to_string()), 2, 0),
                (2, 2, Some("oracle-2".to_string()), 1, 1)
            ]
        );
    }

    // ---- RemoveArchetypeVariantTest, Variants/DestroyControllerTest ----

    fn two_variants(conn: &mut Connection) -> i64 {
        let id = manual_with_list(conn);
        edit_archetypes(
            conn,
            &update(id, "Qbrew", Some(vec![mc(2, None, 3, false)])),
        )
        .expect("second variant");
        id
    }

    #[test]
    fn removing_a_variant_deletes_its_cards_while_another_remains() {
        let (_dir, mut conn) = db("manual-variant-remove");
        let id = two_variants(&mut conn);
        let outcome = edit_archetypes(&mut conn, &ArchetypeEdit::RemoveVariant { id, list: 1 })
            .expect("remove");
        assert_eq!(outcome, done(Vec::new(), Vec::new()));
        assert_eq!(
            lists(&conn, id),
            [(2, 2, Some("oracle-2".to_string()), 3, 0)]
        );
    }

    // 0.x "nulls archetype_deck_id on match_archetypes but preserves
    // archetype_id": v1 has no variant link, the match row stays.
    #[test]
    fn removing_a_variant_keeps_the_archetypes_match_rows() {
        let (_dir, mut conn) = db("manual-variant-remove-rows");
        let id = two_variants(&mut conn);
        insert_match(&conn, 7, "log", 100);
        seat(&conn, 7, true, id);
        edit_archetypes(&mut conn, &ArchetypeEdit::RemoveVariant { id, list: 2 }).expect("remove");
        assert_eq!(opponent_row(&conn, 7).map(|row| row.0), Some(id));
    }

    #[test]
    fn the_last_variant_cannot_be_removed() {
        let (_dir, mut conn) = db("manual-variant-last");
        let id = manual_with_list(&mut conn);
        let outcome = edit_archetypes(&mut conn, &ArchetypeEdit::RemoveVariant { id, list: 1 })
            .expect("remove");
        assert_eq!(outcome, refused("Cannot delete the last variant."));
        assert_eq!(lists(&conn, id).len(), 1);
    }

    #[test]
    fn a_fallback_archetype_has_no_variant_to_remove() {
        let (_dir, mut conn) = db("manual-variant-fallback");
        let outcome = edit_archetypes(&mut conn, &ArchetypeEdit::RemoveVariant { id: 1, list: 1 })
            .expect("remove");
        assert_eq!(outcome, refused("Fallback archetypes cannot be edited."));
    }

    #[test]
    fn a_variant_of_another_archetype_is_not_removed() {
        let (_dir, mut conn) = db("manual-variant-other");
        let id = two_variants(&mut conn);
        let outcome = edit_archetypes(&mut conn, &ArchetypeEdit::RemoveVariant { id, list: 9 })
            .expect("remove");
        assert_eq!(
            outcome,
            refused("Variant does not belong to this archetype.")
        );
        assert_eq!(lists(&conn, id).len(), 2);
    }

    // ---- Variants/ReassignControllerTest and the reassign guards ----

    fn manual(conn: &mut Connection, name: &str, format: &str, catalog_id: i64) -> i64 {
        let mut new = new_manual(name, vec![mc(catalog_id, Some("oracle-q"), 4, false)], None);
        new.format = format.to_string();
        create(conn, new).1
    }

    fn reassign(id: i64, list: i64, target_id: i64) -> ArchetypeEdit {
        ArchetypeEdit::ReassignVariant {
            id,
            list,
            target_id,
            now_ms: NOW + 1,
        }
    }

    #[test]
    fn a_reassigned_variant_becomes_the_targets_next_list() {
        let (_dir, mut conn) = db("manual-reassign");
        let source = manual(&mut conn, "Qsource", "modern", 11);
        edit_archetypes(
            &mut conn,
            &update(
                source,
                "Qsource",
                Some(vec![mc(12, Some("oracle-r"), 2, false)]),
            ),
        )
        .expect("second variant");
        let target = manual(&mut conn, "Qtarget", "modern", 21);
        let outcome = edit_archetypes(&mut conn, &reassign(source, 2, target)).expect("reassign");
        assert_eq!(outcome, done(Vec::new(), Vec::new()));
        assert_eq!(
            lists(&conn, target),
            [
                (1, 21, Some("oracle-q".to_string()), 4, 0),
                (2, 12, Some("oracle-r".to_string()), 2, 0)
            ]
        );
        assert_eq!(lists(&conn, source).len(), 1);
        assert_eq!(merged_into(&conn, source), None);
    }

    #[test]
    fn a_source_left_without_variants_is_merged_into_the_target() {
        let (_dir, mut conn) = db("manual-reassign-merge");
        let source = manual(&mut conn, "Qsource", "modern", 11);
        let target = manual(&mut conn, "Qtarget", "modern", 21);
        edit_archetypes(&mut conn, &reassign(source, 1, target)).expect("reassign");
        assert_eq!(merged_into(&conn, source), Some(target));
        assert!(lists(&conn, source).is_empty());
    }

    #[test]
    fn a_variant_of_another_archetype_is_not_reassigned() {
        let (_dir, mut conn) = db("manual-reassign-other");
        let other = manual(&mut conn, "Qother", "modern", 11);
        let source = manual(&mut conn, "Qsource", "modern", 12);
        let target = manual(&mut conn, "Qtarget", "modern", 13);
        let outcome = edit_archetypes(&mut conn, &reassign(source, 2, target)).expect("reassign");
        assert_eq!(
            outcome,
            refused("Variant does not belong to this archetype.")
        );
        assert_eq!(lists(&conn, other).len(), 1);
        assert_eq!(lists(&conn, target).len(), 1);
    }

    #[test]
    fn a_variant_never_moves_across_formats() {
        let (_dir, mut conn) = db("manual-reassign-format");
        let source = manual(&mut conn, "Qsource", "modern", 11);
        let target = manual(&mut conn, "Qtarget", "pioneer", 12);
        let outcome = edit_archetypes(&mut conn, &reassign(source, 1, target)).expect("reassign");
        assert_eq!(outcome, refused("Target must share the same format."));
        assert_eq!(lists(&conn, source).len(), 1);
    }

    #[test]
    fn variants_move_only_between_manual_archetypes() {
        let (_dir, mut conn) = db("manual-reassign-downloaded");
        let source = manual(&mut conn, "Qsource", "modern", 11);
        let target = insert_archetype(&conn, "q-api", "Qapi", Some("modern"), "downloaded");
        let outcome = edit_archetypes(&mut conn, &reassign(source, 1, target)).expect("reassign");
        assert_eq!(
            outcome,
            refused("Variants exist only for manual archetypes.")
        );
        let missing = edit_archetypes(&mut conn, &reassign(source, 1, 999)).expect("reassign");
        assert_eq!(missing, refused("The selected target id is invalid."));
    }

    // ---- MergeControllerTest, UnmergeControllerTest ----

    fn merge(source_id: i64, parent_id: i64) -> ArchetypeEdit {
        ArchetypeEdit::Merge {
            source_id,
            parent_id,
            now_ms: NOW + 1,
        }
    }

    #[test]
    fn an_archetype_merges_into_its_parent() {
        let (_dir, mut conn) = db("manual-merge");
        let parent = insert_archetype(&conn, "q-parent", "Qparent", Some("modern"), "downloaded");
        let source = insert_archetype(&conn, "q-source", "Qsource", Some("modern"), "manual");
        let outcome = edit_archetypes(&mut conn, &merge(source, parent)).expect("merge");
        assert_eq!(outcome, done(Vec::new(), Vec::new()));
        assert_eq!(merged_into(&conn, source), Some(parent));
    }

    #[test]
    fn a_merge_across_formats_is_refused() {
        let (_dir, mut conn) = db("manual-merge-format");
        let parent = insert_archetype(&conn, "q-parent", "Qparent", Some("modern"), "manual");
        let source = insert_archetype(&conn, "q-source", "Qsource", Some("pioneer"), "manual");
        let outcome = edit_archetypes(&mut conn, &merge(source, parent)).expect("merge");
        assert_eq!(outcome, refused("Parent must share the same format."));
        assert_eq!(merged_into(&conn, source), None);
    }

    #[test]
    fn a_merge_into_a_missing_parent_is_refused() {
        let (_dir, mut conn) = db("manual-merge-missing");
        let source = insert_archetype(&conn, "q-source", "Qsource", Some("modern"), "manual");
        let outcome = edit_archetypes(&mut conn, &merge(source, 999_999)).expect("merge");
        assert_eq!(outcome, refused("The selected parent id is invalid."));
    }

    #[test]
    fn a_self_merge_is_refused() {
        let (_dir, mut conn) = db("manual-merge-self");
        let source = insert_archetype(&conn, "q-source", "Qsource", Some("modern"), "manual");
        let outcome = edit_archetypes(&mut conn, &merge(source, source)).expect("merge");
        assert_eq!(outcome, refused("Cannot merge an archetype into itself."));
        assert_eq!(merged_into(&conn, source), None);
    }

    /// T1 review: the guards keep 0.x's order, and the fallbacks have no
    /// format, so Homebrew against a formatted archetype gives 0.x's format
    /// message before the fallback one.
    #[test]
    fn a_fallback_merge_gives_0xs_format_message_first() {
        let (_dir, mut conn) = db("manual-merge-fallback");
        let other = insert_archetype(&conn, "q-other", "Qother", Some("modern"), "manual");
        let outcome = edit_archetypes(&mut conn, &merge(1, other)).expect("merge");
        assert_eq!(outcome, refused("Parent must share the same format."));
        let both = edit_archetypes(&mut conn, &merge(1, 2)).expect("merge");
        assert_eq!(both, refused("Fallback archetypes cannot be merged."));
    }

    #[test]
    fn an_archetype_unmerges() {
        let (_dir, mut conn) = db("manual-unmerge");
        let parent = insert_archetype(&conn, "q-parent", "Qparent", Some("modern"), "manual");
        let source = insert_archetype(&conn, "q-source", "Qsource", Some("modern"), "manual");
        edit_archetypes(&mut conn, &merge(source, parent)).expect("merge");
        let unmerge = ArchetypeEdit::Unmerge {
            source_id: source,
            now_ms: NOW + 2,
        };
        assert_eq!(
            edit_archetypes(&mut conn, &unmerge).expect("unmerge"),
            done(Vec::new(), Vec::new())
        );
        assert_eq!(merged_into(&conn, source), None);
        assert_eq!(
            edit_archetypes(&mut conn, &unmerge).expect("unmerge again"),
            refused("Archetype is not merged.")
        );
    }

    // ---- UpdateArchetypeMetaTest, UpdateControllerTest ----

    #[test]
    fn an_edit_updates_the_name_format_and_colours() {
        let (_dir, mut conn) = db("manual-update-meta");
        let id = manual_with_list(&mut conn);
        let edit = ArchetypeEdit::UpdateManual {
            id,
            name: "Qnew".to_string(),
            format: "Legacy".to_string(),
            color_identity: Some("BU".to_string()),
            variant: None,
            now_ms: NOW + 5,
        };
        assert_eq!(
            edit_archetypes(&mut conn, &edit).expect("update"),
            done(Vec::new(), Vec::new())
        );
        let row = crate::archetypes::archetype(&conn, id)
            .expect("read")
            .expect("row");
        assert_eq!(
            (
                row.name.as_str(),
                row.format.as_deref(),
                row.color_identity.as_deref()
            ),
            ("Qnew", Some("legacy"), Some("U,B"))
        );
        // The manual source and the creation time stay (0.x "preserves the
        // manual flag", "preserves decklist_downloaded_at").
        assert_eq!(row.source, archetypes::Source::Manual);
        assert_eq!((row.created_at, row.updated_at), (NOW, NOW + 5));
        // Meta only: the variants stay as they were.
        assert_eq!(lists(&conn, id).len(), 1);
    }

    #[test]
    fn an_edit_accepts_no_colours() {
        let (_dir, mut conn) = db("manual-update-null-colours");
        let id = manual_with_list(&mut conn);
        edit_archetypes(&mut conn, &update(id, "Qbrew", None)).expect("update");
        let row = crate::archetypes::archetype(&conn, id)
            .expect("read")
            .expect("row");
        assert_eq!(row.color_identity, None);
    }

    #[test]
    fn a_fallback_archetype_cannot_be_edited() {
        let (_dir, mut conn) = db("manual-update-fallback");
        let outcome = edit_archetypes(&mut conn, &update(1, "Qx", None)).expect("update");
        assert_eq!(outcome, refused("Fallback archetypes cannot be edited."));
    }

    /// Spec ruling 3: downloaded names come from the server.
    #[test]
    fn a_downloaded_archetype_cannot_be_edited() {
        let (_dir, mut conn) = db("manual-update-downloaded");
        let id = insert_archetype(&conn, "q-api", "Qapi", Some("modern"), "downloaded");
        let outcome = edit_archetypes(&mut conn, &update(id, "Qx", None)).expect("update");
        assert_eq!(outcome, refused("Only manual archetypes can be edited."));
        let missing = edit_archetypes(&mut conn, &update(999, "Qx", None)).expect("update");
        assert_eq!(missing, ArchetypeEditOutcome::NotFound);
    }

    // ---- DestroyController, FallbackControllerGuardsTest ----

    #[test]
    fn deleting_a_manual_archetype_cascades_its_rows_and_clears_deck_links() {
        let (_dir, mut conn) = db("manual-delete");
        let id = manual_with_list(&mut conn);
        insert_match(&conn, 7, "log", 100);
        seat(&conn, 7, true, id);
        insert_match(&conn, 8, "log", 200);
        conn.execute("UPDATE matches SET archetypes_detected_at = 5", [])
            .expect("detected");
        crate::archetypes::tests::insert_deck(&conn, 3, "CMODERN");
        conn.execute("UPDATE decks SET archetype_id = ?1 WHERE id = 3", [id])
            .expect("deck link");
        let outcome =
            edit_archetypes(&mut conn, &ArchetypeEdit::DeleteManual { id }).expect("delete");
        assert_eq!(outcome, done(vec![7], vec![3]));
        assert_eq!(number(&conn, "SELECT COUNT(*) FROM match_archetypes"), 0);
        // The matches it held are detected again; the others are left.
        assert_eq!(
            number(
                &conn,
                "SELECT COUNT(*) FROM matches WHERE id = 7 AND archetypes_detected_at IS NULL"
            ),
            1
        );
        assert_eq!(
            number(
                &conn,
                "SELECT COUNT(*) FROM matches WHERE id = 8 AND archetypes_detected_at = 5"
            ),
            1
        );
        assert_eq!(number(&conn, "SELECT COUNT(*) FROM archetype_cards"), 0);
        assert_eq!(
            number(
                &conn,
                "SELECT COUNT(*) FROM decks WHERE archetype_id IS NOT NULL"
            ),
            0
        );
    }

    #[test]
    fn only_manual_archetypes_can_be_deleted() {
        let (_dir, mut conn) = db("manual-delete-guards");
        let api = insert_archetype(&conn, "q-api", "Qapi", Some("modern"), "downloaded");
        assert_eq!(
            edit_archetypes(&mut conn, &ArchetypeEdit::DeleteManual { id: 1 }).expect("delete"),
            refused("Fallback archetypes cannot be deleted.")
        );
        assert_eq!(
            edit_archetypes(&mut conn, &ArchetypeEdit::DeleteManual { id: api }).expect("delete"),
            refused("Only manual archetypes can be deleted.")
        );
        assert_eq!(number(&conn, "SELECT COUNT(*) FROM archetypes"), 3);
    }

    // ---- commander archetypes ----

    fn commander(conn: &Connection, uuid: &str, format: &str) -> i64 {
        conn.execute(
            "INSERT INTO archetypes (uuid, name, format, color_identity, source, created_at,
                 updated_at)
             VALUES (?1, 'Qcmd', ?2, 'W,B', 'commander', 1, 1)",
            params![uuid, format],
        )
        .expect("commander row");
        conn.last_insert_rowid()
    }

    fn commander_edit(id: i64, name: &str, colours: Option<&str>) -> ArchetypeEdit {
        ArchetypeEdit::UpdateManual {
            id,
            name: name.to_string(),
            format: "historic_brawl".to_string(),
            color_identity: colours.map(str::to_string),
            variant: None,
            now_ms: NOW + 1,
        }
    }

    #[test]
    fn a_commander_archetype_can_be_renamed() {
        let (_dir, mut conn) = db("commander-rename");
        let id = commander(&conn, "q-c1", "historic_brawl");
        let outcome =
            edit_archetypes(&mut conn, &commander_edit(id, "Qrenamed", Some("WB"))).expect("edit");
        assert_eq!(outcome, done(Vec::new(), Vec::new()));
        let row = crate::archetypes::archetype(&conn, id)
            .expect("read")
            .expect("row");
        assert_eq!(row.name, "Qrenamed");
        assert_eq!(row.source, archetypes::Source::Commander);
    }

    #[test]
    fn a_commander_archetypes_colours_cannot_change() {
        let (_dir, mut conn) = db("commander-colours");
        let id = commander(&conn, "q-c1", "historic_brawl");
        let outcome =
            edit_archetypes(&mut conn, &commander_edit(id, "Qcmd", Some("R"))).expect("edit");
        assert_eq!(outcome, refused(archetypes::COMMANDER_NO_COLOURS));
        let none = edit_archetypes(&mut conn, &commander_edit(id, "Qcmd", None)).expect("edit");
        assert_eq!(none, refused(archetypes::COMMANDER_NO_COLOURS));
    }

    #[test]
    fn a_commander_archetype_cannot_be_deleted() {
        let (_dir, mut conn) = db("commander-delete");
        let id = commander(&conn, "q-c1", "historic_brawl");
        let outcome =
            edit_archetypes(&mut conn, &ArchetypeEdit::DeleteManual { id }).expect("delete");
        assert_eq!(outcome, refused(archetypes::COMMANDER_NO_DELETE));
        assert_eq!(number(&conn, "SELECT COUNT(*) FROM archetypes"), 3);
    }

    #[test]
    fn a_commander_archetype_merges_both_ways_within_its_format() {
        let (_dir, mut conn) = db("commander-merge");
        let one = commander(&conn, "q-c1", "historic_brawl");
        let two = commander(&conn, "q-c2", "historic_brawl");
        let manual_one = insert_archetype(&conn, "q-m1", "Qm", Some("historic_brawl"), "manual");
        assert_eq!(
            edit_archetypes(&mut conn, &merge(one, manual_one)).expect("merge"),
            done(Vec::new(), Vec::new())
        );
        assert_eq!(merged_into(&conn, one), Some(manual_one));
        assert_eq!(
            edit_archetypes(&mut conn, &merge(two, one)).expect("merge"),
            refused("Parent archetype is already merged.")
        );
        let target = commander(&conn, "q-c3", "historic_brawl");
        assert_eq!(
            edit_archetypes(&mut conn, &merge(manual_one, target)).expect("merge"),
            done(Vec::new(), Vec::new())
        );
        assert_eq!(merged_into(&conn, manual_one), Some(target));
        let other_format = commander(&conn, "q-c4", "brawl");
        assert_eq!(
            edit_archetypes(&mut conn, &merge(two, other_format)).expect("merge"),
            refused("Parent must share the same format.")
        );
    }

    #[test]
    fn a_commander_archetype_has_no_variants() {
        let (_dir, mut conn) = db("commander-variants");
        let id = commander(&conn, "q-c1", "historic_brawl");
        let with_variant = ArchetypeEdit::UpdateManual {
            id,
            name: "Qcmd".to_string(),
            format: "historic_brawl".to_string(),
            color_identity: Some("WB".to_string()),
            variant: Some(vec![mc(1, Some("o1"), 1, false)]),
            now_ms: NOW + 1,
        };
        assert_eq!(
            edit_archetypes(&mut conn, &with_variant).expect("edit"),
            refused("Variants exist only for manual archetypes.")
        );
        assert_eq!(
            edit_archetypes(&mut conn, &ArchetypeEdit::RemoveVariant { id, list: 1 })
                .expect("remove"),
            refused("Variants exist only for manual archetypes.")
        );
        let target = insert_archetype(&conn, "q-m1", "Qm", Some("historic_brawl"), "manual");
        let reassign = ArchetypeEdit::ReassignVariant {
            id,
            list: 1,
            target_id: target,
            now_ms: NOW + 1,
        };
        assert!(matches!(
            edit_archetypes(&mut conn, &reassign).expect("reassign"),
            ArchetypeEditOutcome::Refused(_)
        ));
    }
}
