//! The guide and note edits (archetypes spec section 21.3, decision C4):
//! a guide and its cards and a matchup note are input, changed only here
//! (by the player), by the 0.x import or a pulled deck bundle, and moved by
//! a server `moved` answer (`move_guides`).

use archetypes::guides::{GUIDE_EXISTS, GuideCardInput, check_note_body, check_plan};
use rusqlite::{OptionalExtension, Transaction, params};

use super::ArchetypeEditOutcome;
use crate::StoreError;

/// The deck of a guide or note edit does not exist (0.x's 404).
pub const DECK_NOT_FOUND: &str = "The deck was not found.";

/// The guide does not exist or belongs to another deck (0.x's 404 through
/// `scopeBindings`).
pub const GUIDE_NOT_FOUND: &str = "The guide was not found.";

/// The note does not exist (0.x's 404).
pub const NOTE_NOT_FOUND: &str = "The note was not found.";

fn refused(message: &str) -> ArchetypeEditOutcome {
    ArchetypeEditOutcome::Refused(message.to_string())
}

fn done() -> ArchetypeEditOutcome {
    ArchetypeEditOutcome::Done {
        matches: Vec::new(),
        decks: Vec::new(),
    }
}

fn deck_exists(tx: &Transaction, deck_id: i64) -> Result<bool, StoreError> {
    Ok(tx
        .prepare_cached("SELECT EXISTS (SELECT 1 FROM decks WHERE id = ?1)")?
        .query_row([deck_id], |row| row.get(0))?)
}

fn archetype_exists(tx: &Transaction, archetype_id: i64) -> Result<bool, StoreError> {
    Ok(tx
        .prepare_cached("SELECT EXISTS (SELECT 1 FROM archetypes WHERE id = ?1)")?
        .query_row([archetype_id], |row| row.get(0))?)
}

/// The pair's guide id, if any.
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

/// The guide's archetype when the guide is the deck's (0.x
/// `scopeBindings`).
fn deck_guide(tx: &Transaction, deck_id: i64, guide_id: i64) -> Result<Option<i64>, StoreError> {
    Ok(tx
        .prepare_cached("SELECT archetype_id FROM sideboard_guides WHERE id = ?1 AND deck_id = ?2")?
        .query_row([guide_id, deck_id], |row| row.get(0))
        .optional()?)
}

fn insert_guide(
    tx: &Transaction,
    deck_id: i64,
    archetype_id: i64,
    now_ms: i64,
) -> Result<i64, StoreError> {
    Ok(tx
        .prepare_cached(
            "INSERT INTO sideboard_guides (deck_id, archetype_id, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?3) RETURNING id",
        )?
        .query_row(params![deck_id, archetype_id, now_ms], |row| row.get(0))?)
}

/// 0.x `StoreController` with `StoreSideboardGuideRequest`: an existing
/// archetype (fallbacks too), one guide per deck and archetype.
pub(super) fn create_guide(
    tx: &Transaction,
    deck_id: i64,
    archetype_id: i64,
    now_ms: i64,
) -> Result<ArchetypeEditOutcome, StoreError> {
    if !archetype_exists(tx, archetype_id)? {
        return Ok(ArchetypeEditOutcome::UnknownArchetype);
    }
    if !deck_exists(tx, deck_id)? {
        return Ok(refused(DECK_NOT_FOUND));
    }
    if pair_guide(tx, deck_id, archetype_id)?.is_some() {
        return Ok(refused(GUIDE_EXISTS));
    }
    let guide_id = insert_guide(tx, deck_id, archetype_id, now_ms)?;
    Ok(ArchetypeEditOutcome::Guide { guide_id })
}

/// 0.x `UpdateCardsController`: the plan checked against the deck's latest
/// version and the saved plan, then replaced whole and the guide touched.
pub(super) fn save_guide_cards(
    tx: &Transaction,
    deck_id: i64,
    guide_id: i64,
    cards: &[GuideCardInput],
    now_ms: i64,
) -> Result<ArchetypeEditOutcome, StoreError> {
    if deck_guide(tx, deck_id, guide_id)?.is_none() {
        return Ok(refused(GUIDE_NOT_FOUND));
    }
    let zones = crate::guides::latest_zones(tx, deck_id)?;
    let saved = crate::guides::guide_cards(tx, guide_id)?;
    let checked = match check_plan(&zones, &saved, cards) {
        Ok(checked) => checked,
        Err(errors) => return Ok(ArchetypeEditOutcome::PlanRejected(errors)),
    };
    tx.prepare_cached("DELETE FROM sideboard_guide_cards WHERE guide_id = ?1")?
        .execute([guide_id])?;
    let mut insert = tx.prepare_cached(
        "INSERT INTO sideboard_guide_cards (guide_id, oracle_id, direction, quantity)
         VALUES (?1, ?2, ?3, ?4)",
    )?;
    for card in &checked {
        insert.execute(params![
            guide_id,
            card.oracle_id,
            card.direction.as_str(),
            card.quantity
        ])?;
    }
    tx.prepare_cached("UPDATE sideboard_guides SET updated_at = ?2 WHERE id = ?1")?
        .execute(params![guide_id, now_ms])?;
    Ok(done())
}

/// 0.x `DestroyController`: the guide, its cards and every note of the
/// same pair (they would otherwise bring an empty guide back).
pub(super) fn delete_guide(
    tx: &Transaction,
    deck_id: i64,
    guide_id: i64,
) -> Result<ArchetypeEditOutcome, StoreError> {
    let Some(archetype_id) = deck_guide(tx, deck_id, guide_id)? else {
        return Ok(refused(GUIDE_NOT_FOUND));
    };
    tx.prepare_cached("DELETE FROM deck_archetype_notes WHERE deck_id = ?1 AND archetype_id = ?2")?
        .execute([deck_id, archetype_id])?;
    tx.prepare_cached("DELETE FROM sideboard_guides WHERE id = ?1")?
        .execute([guide_id])?;
    Ok(done())
}

/// 0.x `StoreNoteController` (and the overlay's, which ensures the pair's
/// guide first so the note shows in the deck's guide list).
pub(super) fn add_note(
    tx: &Transaction,
    deck_id: i64,
    archetype_id: i64,
    body: &str,
    now_ms: i64,
) -> Result<ArchetypeEditOutcome, StoreError> {
    let body = match check_note_body(body) {
        Ok(body) => body,
        Err(message) => return Ok(refused(message)),
    };
    if !archetype_exists(tx, archetype_id)? {
        return Ok(ArchetypeEditOutcome::UnknownArchetype);
    }
    if !deck_exists(tx, deck_id)? {
        return Ok(refused(DECK_NOT_FOUND));
    }
    let guide_id = match pair_guide(tx, deck_id, archetype_id)? {
        Some(id) => id,
        None => insert_guide(tx, deck_id, archetype_id, now_ms)?,
    };
    let note_id = tx
        .prepare_cached(
            "INSERT INTO deck_archetype_notes (deck_id, archetype_id, body, created_at)
             VALUES (?1, ?2, ?3, ?4) RETURNING id",
        )?
        .query_row(params![deck_id, archetype_id, body, now_ms], |row| {
            row.get(0)
        })?;
    Ok(ArchetypeEditOutcome::Note { note_id, guide_id })
}

/// 0.x `DestroyNoteController`.
pub(super) fn delete_note(
    tx: &Transaction,
    note_id: i64,
) -> Result<ArchetypeEditOutcome, StoreError> {
    let deleted = tx
        .prepare_cached("DELETE FROM deck_archetype_notes WHERE id = ?1")?
        .execute([note_id])?;
    if deleted == 0 {
        return Ok(refused(NOTE_NOT_FOUND));
    }
    Ok(done())
}

/// A server `moved` answer (spec section 21.3): every guide and note of
/// `old` moves to `new`. When the deck already has a guide for `new`, that
/// guide keeps its cards if it has any, else takes the moved guide's, and
/// the moved guide is dropped. A moved note whose text the deck already
/// has against `new` is dropped; a moved note whose pair has no guide gets
/// the pair's empty guide. The guides that move, take cards or are made are
/// touched at `now_ms`, so their decks' bundles are built again.
pub(super) fn move_guides(
    tx: &Transaction,
    old: i64,
    new: i64,
    now_ms: i64,
) -> Result<(), StoreError> {
    let mut moving: Vec<(i64, i64)> = Vec::new();
    {
        let mut stmt =
            tx.prepare_cached("SELECT id, deck_id FROM sideboard_guides WHERE archetype_id = ?1")?;
        let rows = stmt.query_map([old], |row| Ok((row.get(0)?, row.get(1)?)))?;
        for row in rows {
            moving.push(row?);
        }
    }
    for (guide_id, deck_id) in moving {
        match pair_guide(tx, deck_id, new)? {
            None => {
                tx.prepare_cached(
                    "UPDATE sideboard_guides SET archetype_id = ?2, updated_at = ?3 WHERE id = ?1",
                )?
                .execute(params![guide_id, new, now_ms])?;
            }
            Some(target) => {
                let target_cards: i64 = tx
                    .prepare_cached(
                        "SELECT COUNT(*) FROM sideboard_guide_cards WHERE guide_id = ?1",
                    )?
                    .query_row([target], |row| row.get(0))?;
                if target_cards == 0 {
                    tx.prepare_cached(
                        "UPDATE sideboard_guide_cards SET guide_id = ?2 WHERE guide_id = ?1",
                    )?
                    .execute([guide_id, target])?;
                    tx.prepare_cached("UPDATE sideboard_guides SET updated_at = ?2 WHERE id = ?1")?
                        .execute(params![target, now_ms])?;
                }
                tx.prepare_cached("DELETE FROM sideboard_guides WHERE id = ?1")?
                    .execute([guide_id])?;
            }
        }
    }
    // A note's pair always has a guide (0.x `EnsureSideboardGuide`): a moved
    // note whose pair has none (a pulled bundle can carry one) gets the
    // empty guide, which also moves its deck's touch key (T11 review m2).
    tx.prepare_cached(
        "INSERT INTO sideboard_guides (deck_id, archetype_id, created_at, updated_at)
         SELECT DISTINCT n.deck_id, ?2, ?3, ?3 FROM deck_archetype_notes n
         WHERE n.archetype_id = ?1
           AND NOT EXISTS (SELECT 1 FROM sideboard_guides g
                           WHERE g.deck_id = n.deck_id AND g.archetype_id = ?2)",
    )?
    .execute(params![old, new, now_ms])?;
    // A note whose text the deck already has against the target is kept
    // once (T14 review I1: a 0.x import rerun after a move brings the old
    // uuid back with the same notes).
    tx.prepare_cached(
        "DELETE FROM deck_archetype_notes
         WHERE archetype_id = ?1
           AND EXISTS (SELECT 1 FROM deck_archetype_notes t
                       WHERE t.deck_id = deck_archetype_notes.deck_id
                         AND t.archetype_id = ?2
                         AND t.body = deck_archetype_notes.body)",
    )?
    .execute([old, new])?;
    tx.prepare_cached("UPDATE deck_archetype_notes SET archetype_id = ?2 WHERE archetype_id = ?1")?
        .execute([old, new])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::archetype_edit::{ArchetypeEdit, ArchetypeEditOutcome, edit_archetypes};
    use crate::guides::tests::{
        archetype, db, deck, guide, guide_cards_on_file, note, plan, version,
    };
    use crate::test_support::number;
    use archetypes::guides::{GUIDE_EXISTS, GuideCardInput, LISTED_TWICE, NOT_IN_ZONE, PlanError};
    use rusqlite::Connection;

    fn create(deck_id: i64, archetype_id: i64) -> ArchetypeEdit {
        ArchetypeEdit::CreateGuide {
            deck_id,
            archetype_id,
            now_ms: 5_000,
        }
    }

    fn input(oracle: &str, direction: &str, quantity: i64) -> GuideCardInput {
        GuideCardInput {
            oracle_id: oracle.to_string(),
            direction: direction.to_string(),
            quantity,
        }
    }

    fn save(deck_id: i64, guide_id: i64, cards: Vec<GuideCardInput>) -> ArchetypeEdit {
        ArchetypeEdit::SaveGuideCards {
            deck_id,
            guide_id,
            cards,
            now_ms: 60_000,
        }
    }

    fn add_note(deck_id: i64, archetype_id: i64, body: &str) -> ArchetypeEdit {
        ArchetypeEdit::AddNote {
            deck_id,
            archetype_id,
            body: body.to_string(),
            now_ms: 7_000,
        }
    }

    fn refused(message: &str) -> ArchetypeEditOutcome {
        ArchetypeEditOutcome::Refused(message.to_string())
    }

    fn rejected(errors: Vec<(usize, &str, &str)>) -> ArchetypeEditOutcome {
        ArchetypeEditOutcome::PlanRejected(
            errors
                .into_iter()
                .map(|(index, field, message)| PlanError {
                    index,
                    field: field.to_string(),
                    message: message.to_string(),
                })
                .collect(),
        )
    }

    fn done() -> ArchetypeEditOutcome {
        ArchetypeEditOutcome::Done {
            matches: Vec::new(),
            decks: Vec::new(),
        }
    }

    fn plan_rows(conn: &Connection, guide_id: i64) -> Vec<(String, String, i64)> {
        let mut stmt = conn
            .prepare(
                "SELECT oracle_id, direction, quantity FROM sideboard_guide_cards
                 WHERE guide_id = ?1 ORDER BY oracle_id, direction",
            )
            .expect("prepare");
        stmt.query_map([guide_id], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .expect("query")
        .map(|row| row.expect("row"))
        .collect()
    }

    fn guides_of(conn: &Connection) -> Vec<(i64, i64)> {
        let mut stmt = conn
            .prepare("SELECT deck_id, archetype_id FROM sideboard_guides ORDER BY id")
            .expect("prepare");
        stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .expect("query")
            .map(|row| row.expect("row"))
            .collect()
    }

    fn notes_of(conn: &Connection) -> Vec<(i64, i64, String)> {
        let mut stmt = conn
            .prepare("SELECT deck_id, archetype_id, body FROM deck_archetype_notes ORDER BY id")
            .expect("prepare");
        stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .expect("query")
            .map(|row| row.expect("row"))
            .collect()
    }

    // ---- StoreControllerTest (as CreateGuide) ---------------------------------

    #[test]
    fn creates_a_guide_and_returns_it_for_the_editor() {
        let (_dir, mut conn) = db("guide-edit-create");
        let deck_id = deck(&conn, "Qdeck");
        let a = archetype(&conn, "Qarch");
        let outcome = edit_archetypes(&mut conn, &create(deck_id, a)).expect("edit");
        let id = number(&conn, "SELECT id FROM sideboard_guides");
        assert_eq!(outcome, ArchetypeEditOutcome::Guide { guide_id: id });
        assert_eq!(guides_of(&conn), [(deck_id, a)]);
        assert_eq!(
            number(
                &conn,
                "SELECT created_at + updated_at FROM sideboard_guides"
            ),
            10_000
        );
    }

    #[test]
    fn rejects_a_second_guide_for_the_same_deck_and_archetype_with_a_clear_message() {
        let (_dir, mut conn) = db("guide-edit-duplicate");
        let deck_id = deck(&conn, "Qdeck");
        let a = archetype(&conn, "Qarch");
        guide(&conn, deck_id, a);
        assert_eq!(
            edit_archetypes(&mut conn, &create(deck_id, a)).expect("edit"),
            refused(GUIDE_EXISTS)
        );
        assert_eq!(number(&conn, "SELECT COUNT(*) FROM sideboard_guides"), 1);
    }

    #[test]
    fn allows_the_same_archetype_on_a_different_deck() {
        let (_dir, mut conn) = db("guide-edit-other-deck");
        let first = deck(&conn, "Qfirst");
        let second = deck(&conn, "Qsecond");
        let a = archetype(&conn, "Qarch");
        guide(&conn, first, a);
        assert!(matches!(
            edit_archetypes(&mut conn, &create(second, a)).expect("edit"),
            ArchetypeEditOutcome::Guide { .. }
        ));
        assert_eq!(number(&conn, "SELECT COUNT(*) FROM sideboard_guides"), 2);
    }

    #[test]
    fn requires_an_existing_archetype_and_deck() {
        let (_dir, mut conn) = db("guide-edit-exists");
        let deck_id = deck(&conn, "Qdeck");
        let a = archetype(&conn, "Qarch");
        assert_eq!(
            edit_archetypes(&mut conn, &create(deck_id, 999)).expect("edit"),
            ArchetypeEditOutcome::UnknownArchetype
        );
        assert_eq!(
            edit_archetypes(&mut conn, &create(999, a)).expect("edit"),
            refused(super::DECK_NOT_FOUND)
        );
        assert_eq!(number(&conn, "SELECT COUNT(*) FROM sideboard_guides"), 0);
    }

    /// Fallback archetypes have guides (0.x offers them first).
    #[test]
    fn a_fallback_archetype_may_have_a_guide() {
        let (_dir, mut conn) = db("guide-edit-fallback");
        let deck_id = deck(&conn, "Qdeck");
        assert!(matches!(
            edit_archetypes(&mut conn, &create(deck_id, 1)).expect("edit"),
            ArchetypeEditOutcome::Guide { .. }
        ));
    }

    // ---- EnsureSideboardGuideTest (as AddNote's guide) ------------------------

    #[test]
    fn adding_a_note_creates_the_empty_guide() {
        let (_dir, mut conn) = db("guide-edit-note-guide");
        let deck_id = deck(&conn, "Qdeck");
        let a = archetype(&conn, "Qarch");
        let outcome =
            edit_archetypes(&mut conn, &add_note(deck_id, a, "Qmull aggressively")).expect("edit");
        let guide_id = number(&conn, "SELECT id FROM sideboard_guides");
        let note_id = number(&conn, "SELECT id FROM deck_archetype_notes");
        assert_eq!(outcome, ArchetypeEditOutcome::Note { note_id, guide_id });
        assert_eq!(guides_of(&conn), [(deck_id, a)]);
        assert!(plan_rows(&conn, guide_id).is_empty());
    }

    #[test]
    fn returns_the_existing_guide_on_repeat_notes() {
        let (_dir, mut conn) = db("guide-edit-note-repeat");
        let deck_id = deck(&conn, "Qdeck");
        let a = archetype(&conn, "Qarch");
        let first = edit_archetypes(&mut conn, &add_note(deck_id, a, "Qone")).expect("edit");
        let second = edit_archetypes(&mut conn, &add_note(deck_id, a, "Qtwo")).expect("edit");
        let guide_of = |outcome: &ArchetypeEditOutcome| match outcome {
            ArchetypeEditOutcome::Note { guide_id, .. } => *guide_id,
            other => panic!("not a note: {other:?}"),
        };
        assert_eq!(guide_of(&first), guide_of(&second));
        assert_eq!(number(&conn, "SELECT COUNT(*) FROM sideboard_guides"), 1);
        assert_eq!(
            number(&conn, "SELECT COUNT(*) FROM deck_archetype_notes"),
            2
        );
    }

    // ---- StoreNoteControllerTest ----------------------------------------------

    #[test]
    fn stores_a_note_against_the_deck_and_archetype() {
        let (_dir, mut conn) = db("guide-edit-note");
        let deck_id = deck(&conn, "Qdeck");
        let a = archetype(&conn, "Qarch");
        edit_archetypes(&mut conn, &add_note(deck_id, a, "  Mull aggressively \n")).expect("edit");
        assert_eq!(
            notes_of(&conn),
            [(deck_id, a, "Mull aggressively".to_string())]
        );
        assert_eq!(
            number(&conn, "SELECT created_at FROM deck_archetype_notes"),
            7_000
        );
    }

    #[test]
    fn requires_a_body_under_two_thousand_characters() {
        let (_dir, mut conn) = db("guide-edit-note-body");
        let deck_id = deck(&conn, "Qdeck");
        let a = archetype(&conn, "Qarch");
        assert_eq!(
            edit_archetypes(&mut conn, &add_note(deck_id, a, "")).expect("edit"),
            refused("The body field is required.")
        );
        assert_eq!(
            edit_archetypes(&mut conn, &add_note(deck_id, a, &"a".repeat(2001))).expect("edit"),
            refused("The body field must not be greater than 2000 characters.")
        );
        assert_eq!(
            edit_archetypes(&mut conn, &add_note(deck_id, 999, "Qbody")).expect("edit"),
            ArchetypeEditOutcome::UnknownArchetype
        );
        assert_eq!(
            number(&conn, "SELECT COUNT(*) FROM deck_archetype_notes"),
            0
        );
        assert_eq!(
            number(&conn, "SELECT COUNT(*) FROM sideboard_guides"),
            0,
            "a refused note leaves no guide"
        );
    }

    #[test]
    fn deleting_a_note_removes_only_that_note() {
        let (_dir, mut conn) = db("guide-edit-note-delete");
        let deck_id = deck(&conn, "Qdeck");
        let a = archetype(&conn, "Qarch");
        let gone = note(&conn, deck_id, a, "Qgone", 1);
        note(&conn, deck_id, a, "Qkept", 2);
        assert_eq!(
            edit_archetypes(&mut conn, &ArchetypeEdit::DeleteNote { note_id: gone }).expect("edit"),
            done()
        );
        assert_eq!(notes_of(&conn), [(deck_id, a, "Qkept".to_string())]);
        assert_eq!(
            edit_archetypes(&mut conn, &ArchetypeEdit::DeleteNote { note_id: gone }).expect("edit"),
            refused(super::NOTE_NOT_FOUND)
        );
    }

    // ---- UpdateCardsControllerTest (as SaveGuideCards) ------------------------

    /// 0.x `savableGuide`: 4 Bolt main, 2 RiP board, 1 Cut Down board.
    fn savable(conn: &Connection) -> (i64, i64) {
        guide_cards_on_file(conn);
        let deck_id = deck(conn, "Qdeck");
        version(
            conn,
            deck_id,
            &[(202, 4, false), (201, 2, true), (203, 1, true)],
        );
        let g = guide(conn, deck_id, archetype(conn, "Qarch"));
        (deck_id, g)
    }

    #[test]
    fn replaces_the_card_set_atomically() {
        let (_dir, mut conn) = db("guide-edit-save");
        let (deck_id, g) = savable(&conn);
        plan(&conn, g, "o-cut", "in", 1);
        let outcome = edit_archetypes(
            &mut conn,
            &save(
                deck_id,
                g,
                vec![input("o-rip", "in", 2), input("o-bolt", "out", 2)],
            ),
        )
        .expect("edit");
        assert_eq!(outcome, done());
        assert_eq!(
            plan_rows(&conn, g),
            [
                ("o-bolt".to_string(), "out".to_string(), 2),
                ("o-rip".to_string(), "in".to_string(), 2)
            ]
        );
        assert_eq!(
            number(&conn, "SELECT updated_at FROM sideboard_guides"),
            60_000,
            "the guide is touched"
        );
    }

    #[test]
    fn clears_the_plan_when_cards_is_empty() {
        let (_dir, mut conn) = db("guide-edit-clear");
        let (deck_id, g) = savable(&conn);
        plan(&conn, g, "o-cut", "in", 1);
        edit_archetypes(&mut conn, &save(deck_id, g, Vec::new())).expect("edit");
        assert!(plan_rows(&conn, g).is_empty());
    }

    #[test]
    fn rejects_bringing_in_more_copies_than_the_sideboard_holds() {
        let (_dir, mut conn) = db("guide-edit-too-many-in");
        let (deck_id, g) = savable(&conn);
        assert_eq!(
            edit_archetypes(&mut conn, &save(deck_id, g, vec![input("o-rip", "in", 3)]))
                .expect("edit"),
            rejected(vec![(0, "quantity", "Only 2 in the sideboard.")])
        );
        assert!(plan_rows(&conn, g).is_empty());
        assert_eq!(
            number(&conn, "SELECT updated_at FROM sideboard_guides"),
            1,
            "a refused plan does not touch the guide"
        );
    }

    #[test]
    fn rejects_taking_out_more_copies_than_the_maindeck_holds() {
        let (_dir, mut conn) = db("guide-edit-too-many-out");
        let (deck_id, g) = savable(&conn);
        assert_eq!(
            edit_archetypes(
                &mut conn,
                &save(deck_id, g, vec![input("o-bolt", "out", 5)])
            )
            .expect("edit"),
            rejected(vec![(0, "quantity", "Only 4 in the maindeck.")])
        );
    }

    #[test]
    fn rejects_a_card_that_is_not_in_that_zone_of_the_current_version_unless_it_is_already_on_the_guide()
     {
        let (_dir, mut conn) = db("guide-edit-zone");
        let (deck_id, g) = savable(&conn);
        assert_eq!(
            edit_archetypes(&mut conn, &save(deck_id, g, vec![input("o-bolt", "in", 1)]))
                .expect("edit"),
            rejected(vec![(0, "oracle_id", NOT_IN_ZONE)])
        );
        plan(&conn, g, "o-gone", "in", 2);
        assert_eq!(
            edit_archetypes(&mut conn, &save(deck_id, g, vec![input("o-gone", "in", 2)]))
                .expect("edit"),
            done()
        );
        assert_eq!(
            plan_rows(&conn, g),
            [("o-gone".to_string(), "in".to_string(), 2)]
        );
    }

    #[test]
    fn rejects_the_same_card_listed_twice_for_one_direction() {
        let (_dir, mut conn) = db("guide-edit-twice");
        let (deck_id, g) = savable(&conn);
        assert_eq!(
            edit_archetypes(
                &mut conn,
                &save(
                    deck_id,
                    g,
                    vec![input("o-rip", "in", 1), input("o-rip", "in", 1)]
                )
            )
            .expect("edit"),
            rejected(vec![(1, "oracle_id", LISTED_TWICE)])
        );
        assert!(plan_rows(&conn, g).is_empty());
    }

    #[test]
    fn validates_the_card_shape() {
        let (_dir, mut conn) = db("guide-edit-shape");
        let (deck_id, g) = savable(&conn);
        assert_eq!(
            edit_archetypes(
                &mut conn,
                &save(deck_id, g, vec![input("o-rip", "sideways", 0)])
            )
            .expect("edit"),
            rejected(vec![
                (0, "direction", "The selected cards.0.direction is invalid."),
                (
                    0,
                    "quantity",
                    "The cards.0.quantity field must be at least 1."
                ),
            ])
        );
    }

    /// 0.x "returns 404 for a guide that belongs to another deck".
    #[test]
    fn a_guide_of_another_deck_is_refused() {
        let (_dir, mut conn) = db("guide-edit-other");
        let (_deck_id, g) = savable(&conn);
        let other = deck(&conn, "Qother");
        assert_eq!(
            edit_archetypes(&mut conn, &save(other, g, Vec::new())).expect("edit"),
            refused(super::GUIDE_NOT_FOUND)
        );
        assert_eq!(
            edit_archetypes(
                &mut conn,
                &ArchetypeEdit::DeleteGuide {
                    deck_id: other,
                    guide_id: g
                }
            )
            .expect("edit"),
            refused(super::GUIDE_NOT_FOUND)
        );
        assert_eq!(number(&conn, "SELECT COUNT(*) FROM sideboard_guides"), 1);
    }

    /// A deck without a version has nothing to plan with: only a stale
    /// entry already saved may stay (0.x checks against empty zones).
    #[test]
    fn a_deck_without_a_version_accepts_only_its_saved_entries() {
        let (_dir, mut conn) = db("guide-edit-no-version");
        let deck_id = deck(&conn, "Qdeck");
        let g = guide(&conn, deck_id, archetype(&conn, "Qarch"));
        plan(&conn, g, "o-old", "out", 1);
        assert_eq!(
            edit_archetypes(&mut conn, &save(deck_id, g, vec![input("o-new", "in", 1)]))
                .expect("edit"),
            rejected(vec![(0, "oracle_id", NOT_IN_ZONE)])
        );
        assert_eq!(
            edit_archetypes(&mut conn, &save(deck_id, g, vec![input("o-old", "out", 1)]))
                .expect("edit"),
            done()
        );
    }

    // ---- DestroyControllerTest ------------------------------------------------

    #[test]
    fn deleting_a_guide_removes_only_that_pairs_notes() {
        let (_dir, mut conn) = db("guide-edit-delete");
        let deck_id = deck(&conn, "Qdeck");
        let other_deck = deck(&conn, "Qother");
        let blink = archetype(&conn, "Qblink");
        let tron = archetype(&conn, "Qtron");
        let g = guide(&conn, deck_id, blink);
        plan(&conn, g, "o-rip", "in", 1);
        note(&conn, deck_id, blink, "Qgone", 1);
        note(&conn, deck_id, tron, "Qother pair", 2);
        note(&conn, other_deck, blink, "Qother deck", 3);

        assert_eq!(
            edit_archetypes(
                &mut conn,
                &ArchetypeEdit::DeleteGuide {
                    deck_id,
                    guide_id: g
                }
            )
            .expect("edit"),
            done()
        );
        assert_eq!(number(&conn, "SELECT COUNT(*) FROM sideboard_guides"), 0);
        assert_eq!(
            number(&conn, "SELECT COUNT(*) FROM sideboard_guide_cards"),
            0
        );
        assert_eq!(
            notes_of(&conn),
            [
                (deck_id, tron, "Qother pair".to_string()),
                (other_deck, blink, "Qother deck".to_string())
            ]
        );
    }

    // ---- Archetype events -----------------------------------------------------

    #[test]
    fn deleting_a_manual_archetype_deletes_its_guides_and_notes() {
        let (_dir, mut conn) = db("guide-edit-delete-manual");
        let deck_id = deck(&conn, "Qdeck");
        let manual = crate::archetypes::tests::insert_archetype(
            &conn,
            "q-manual",
            "Qmanual",
            Some("modern"),
            "manual",
        );
        let kept = archetype(&conn, "Qkept");
        let g = guide(&conn, deck_id, manual);
        plan(&conn, g, "o-rip", "in", 1);
        note(&conn, deck_id, manual, "Qgone", 1);
        guide(&conn, deck_id, kept);
        note(&conn, deck_id, kept, "Qkept", 2);

        assert!(matches!(
            edit_archetypes(&mut conn, &ArchetypeEdit::DeleteManual { id: manual })
                .expect("delete"),
            ArchetypeEditOutcome::Done { .. }
        ));
        assert_eq!(guides_of(&conn), [(deck_id, kept)]);
        assert_eq!(
            number(&conn, "SELECT COUNT(*) FROM sideboard_guide_cards"),
            0
        );
        assert_eq!(notes_of(&conn), [(deck_id, kept, "Qkept".to_string())]);
    }

    #[test]
    fn a_merge_leaves_guides_on_the_source() {
        let (_dir, mut conn) = db("guide-edit-merge");
        let deck_id = deck(&conn, "Qdeck");
        let insert = |uuid: &str, name: &str| {
            crate::archetypes::tests::insert_archetype(&conn, uuid, name, Some("modern"), "manual")
        };
        let source = insert("q-source", "Qsource");
        let parent = insert("q-parent", "Qparent");
        guide(&conn, deck_id, source);
        note(&conn, deck_id, source, "Qnote", 1);

        let merge = ArchetypeEdit::Merge {
            source_id: source,
            parent_id: parent,
            now_ms: 9_000,
        };
        assert!(matches!(
            edit_archetypes(&mut conn, &merge).expect("merge"),
            ArchetypeEditOutcome::Done { .. }
        ));
        assert_eq!(guides_of(&conn), [(deck_id, source)]);
        assert_eq!(notes_of(&conn), [(deck_id, source, "Qnote".to_string())]);
    }

    fn move_plan(from: &str, to: &str) -> ArchetypeEdit {
        ArchetypeEdit::ApplyDefinitions {
            plan: archetypes::ChangePlan {
                moves: vec![(from.to_string(), to.to_string())],
                ..archetypes::ChangePlan::default()
            },
            checked_at: None,
            now_ms: 8_000,
        }
    }

    /// Review Focus 6: a server `moved` answer takes the old archetype's
    /// guides (with their cards) and notes to the target.
    #[test]
    fn a_moved_uuid_takes_its_guides_and_notes() {
        let (_dir, mut conn) = db("guide-edit-move");
        let deck_id = deck(&conn, "Qdeck");
        let other_deck = deck(&conn, "Qother");
        let old = archetype(&conn, "Qold");
        let new = archetype(&conn, "Qnew");
        let g = guide(&conn, deck_id, old);
        plan(&conn, g, "o-rip", "in", 2);
        note(&conn, deck_id, old, "Qfirst", 1);
        note(&conn, other_deck, old, "Qnote without a guide", 2);

        edit_archetypes(&mut conn, &move_plan("q-Qold", "q-Qnew")).expect("move");
        // T11 review m2: a moved note whose pair had no guide gets the
        // pair's empty guide (0.x `EnsureSideboardGuide`), so its deck's
        // touch key moves and the bundle drops the dead uuid.
        assert_eq!(guides_of(&conn), [(deck_id, new), (other_deck, new)]);
        assert_eq!(
            plan_rows(&conn, g),
            [("o-rip".to_string(), "in".to_string(), 2)]
        );
        assert_eq!(
            number(
                &conn,
                "SELECT COUNT(*) FROM sideboard_guides WHERE updated_at = 8000"
            ),
            2,
            "the moved and the new guide are touched, so both bundles are rebuilt"
        );
        assert_eq!(
            notes_of(&conn),
            [
                (deck_id, new, "Qfirst".to_string()),
                (other_deck, new, "Qnote without a guide".to_string())
            ]
        );
    }

    /// T14 review I1: a moved note whose text the target pair already has
    /// is dropped, so a 0.x import rerun after a move never doubles notes.
    #[test]
    fn a_moved_note_the_target_pair_already_has_is_kept_once() {
        let (_dir, mut conn) = db("guide-edit-move-dedupe");
        let deck_id = deck(&conn, "Qdeck");
        let other_deck = deck(&conn, "Qother");
        let old = archetype(&conn, "Qold");
        let new = archetype(&conn, "Qnew");
        note(&conn, deck_id, old, "Qsame", 1);
        note(&conn, deck_id, new, "Qsame", 1);
        note(&conn, deck_id, old, "Qonly old", 2);
        note(&conn, other_deck, new, "Qonly old", 3);

        edit_archetypes(&mut conn, &move_plan("q-Qold", "q-Qnew")).expect("move");
        assert_eq!(
            notes_of(&conn),
            [
                (deck_id, new, "Qsame".to_string()),
                (deck_id, new, "Qonly old".to_string()),
                (other_deck, new, "Qonly old".to_string()),
            ]
        );
    }

    /// Spec 21.3's collision rule: the target guide keeps its cards when it
    /// has any, else takes the moved guide's; the moved guide is dropped;
    /// every note moves.
    #[test]
    fn a_move_onto_a_deck_that_already_has_a_guide_keeps_the_targets_cards() {
        let (_dir, mut conn) = db("guide-edit-move-collision");
        let keeps = deck(&conn, "Qkeeps");
        let takes = deck(&conn, "Qtakes");
        let old = archetype(&conn, "Qold");
        let new = archetype(&conn, "Qnew");
        // Deck "keeps": both guides have cards; the target's stay.
        let moved_keeps = guide(&conn, keeps, old);
        plan(&conn, moved_keeps, "o-moved", "in", 1);
        let target_keeps = guide(&conn, keeps, new);
        plan(&conn, target_keeps, "o-target", "out", 2);
        // Deck "takes": the target guide is empty and takes the moved cards.
        let moved_takes = guide(&conn, takes, old);
        plan(&conn, moved_takes, "o-moved", "in", 3);
        let target_takes = guide(&conn, takes, new);
        note(&conn, keeps, old, "Qold note", 1);
        note(&conn, keeps, new, "Qnew note", 2);

        edit_archetypes(&mut conn, &move_plan("q-Qold", "q-Qnew")).expect("move");
        assert_eq!(guides_of(&conn), [(keeps, new), (takes, new)]);
        assert_eq!(
            plan_rows(&conn, target_keeps),
            [("o-target".to_string(), "out".to_string(), 2)]
        );
        assert_eq!(
            plan_rows(&conn, target_takes),
            [("o-moved".to_string(), "in".to_string(), 3)]
        );
        assert_eq!(
            number(&conn, "SELECT COUNT(*) FROM sideboard_guide_cards"),
            2
        );
        assert_eq!(
            notes_of(&conn),
            [
                (keeps, new, "Qold note".to_string()),
                (keeps, new, "Qnew note".to_string())
            ]
        );
    }

    /// Retired: guides and notes stay (spec 21.3).
    #[test]
    fn a_retired_archetype_keeps_its_guides_and_notes() {
        let (_dir, mut conn) = db("guide-edit-retire");
        let deck_id = deck(&conn, "Qdeck");
        let old = archetype(&conn, "Qold");
        guide(&conn, deck_id, old);
        note(&conn, deck_id, old, "Qnote", 1);
        let retire = ArchetypeEdit::ApplyDefinitions {
            plan: archetypes::ChangePlan {
                retire: vec!["q-Qold".to_string()],
                ..archetypes::ChangePlan::default()
            },
            checked_at: None,
            now_ms: 8_000,
        };
        edit_archetypes(&mut conn, &retire).expect("retire");
        assert_eq!(guides_of(&conn), [(deck_id, old)]);
        assert_eq!(notes_of(&conn).len(), 1);
    }
}
