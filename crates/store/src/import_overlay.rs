//! 0.x's overlay deck labels imported into v1 (overlay spec section 12).
//!
//! 0.x kept the league card's deck labels in `settings.json`
//! (`overlay_deck_labels`, keyed by 0.x deck id); the importer maps each id
//! to the deck's `mtgo_id`, and here each label is written to that v1
//! deck's `decks.overlay_label` where v1's is still NULL: trimmed, at most
//! 40 characters, as the deck edit stores it. A label v1 already has is
//! kept, so a rerun changes nothing, and `updated_at` never moves (the label
//! is not part of the deck touch key, overlay ruling 9).

use rusqlite::OptionalExtension;

use crate::decks::overlay_label;
use crate::{Connection, StoreError};

/// One 0.x deck label, its deck named by `decks.mtgo_id`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyDeckLabel {
    pub deck_mtgo_id: String,
    pub label: String,
}

/// What the overlay import brought over.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct OverlayImportCounts {
    pub labels_added: u32,
    /// v1's deck already had a label: v1's stays.
    pub labels_kept: u32,
    /// No v1 deck has the label's `mtgo_id`.
    pub labels_unknown: u32,
}

/// Write the labels in one transaction and return the counts. A blank
/// label is no label and is not counted.
pub fn import_overlay_labels(
    conn: &mut Connection,
    labels: &[LegacyDeckLabel],
) -> Result<OverlayImportCounts, StoreError> {
    let tx = conn.transaction()?;
    let mut counts = OverlayImportCounts::default();
    for legacy in labels {
        let Some(label) = overlay_label(Some(&legacy.label)) else {
            continue;
        };
        let current: Option<Option<String>> = tx
            .prepare_cached("SELECT overlay_label FROM decks WHERE mtgo_id = ?1")?
            .query_row([&legacy.deck_mtgo_id], |row| row.get(0))
            .optional()?;
        match current {
            None => counts.labels_unknown += 1,
            Some(Some(_)) => counts.labels_kept += 1,
            Some(None) => {
                tx.execute(
                    "UPDATE decks SET overlay_label = ?2 WHERE mtgo_id = ?1",
                    rusqlite::params![legacy.deck_mtgo_id, label],
                )?;
                counts.labels_added += 1;
            }
        }
    }
    tx.commit()?;
    Ok(counts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::open_writer;
    use crate::test_support::number;
    use fixture_tools::TestDir;

    fn db(label: &str) -> (TestDir, Connection) {
        let dir = TestDir::new(label);
        let conn = open_writer(&dir.join("mymtgo.sqlite")).expect("open");
        conn.execute_batch(
            "INSERT INTO decks (mtgo_id, name, format, created_at, updated_at, source)
             VALUES ('q-1', 'Qburn', 'CMODERN', 5, 5, 'imported'),
                    ('q-2', 'Qtron', 'CMODERN', 5, 5, 'imported');",
        )
        .expect("decks");
        (dir, conn)
    }

    fn label(mtgo_id: &str, text: &str) -> LegacyDeckLabel {
        LegacyDeckLabel {
            deck_mtgo_id: mtgo_id.to_string(),
            label: text.to_string(),
        }
    }

    fn stored(conn: &Connection, mtgo_id: &str) -> Option<String> {
        conn.query_row(
            "SELECT overlay_label FROM decks WHERE mtgo_id = ?1",
            [mtgo_id],
            |row| row.get(0),
        )
        .expect("deck")
    }

    #[test]
    fn labels_are_written_trimmed_and_capped_where_v1_has_none() {
        let (_dir, mut conn) = db("import-overlay-labels");
        let long = "Q".repeat(45);
        let counts = import_overlay_labels(
            &mut conn,
            &[
                label("q-1", "  Qburn for stream  "),
                label("q-2", &long),
                label("q-gone", "Q nobody"),
                label("q-1", "   "),
            ],
        )
        .expect("import");
        assert_eq!(
            counts,
            OverlayImportCounts {
                labels_added: 2,
                labels_kept: 0,
                labels_unknown: 1,
            }
        );
        assert_eq!(stored(&conn, "q-1").as_deref(), Some("Qburn for stream"));
        assert_eq!(stored(&conn, "q-2"), Some("Q".repeat(40)));
        assert_eq!(
            number(&conn, "SELECT COUNT(*) FROM decks WHERE updated_at <> 5"),
            0,
            "a label never moves updated_at"
        );
    }

    #[test]
    fn a_v1_label_is_kept_on_a_rerun() {
        let (_dir, mut conn) = db("import-overlay-rerun");
        conn.execute(
            "UPDATE decks SET overlay_label = 'Q mine' WHERE mtgo_id = 'q-1'",
            [],
        )
        .expect("v1 label");
        let labels = [label("q-1", "Q theirs"), label("q-2", "Q tron")];
        let first = import_overlay_labels(&mut conn, &labels).expect("first");
        assert_eq!((first.labels_added, first.labels_kept), (1, 1));
        let again = import_overlay_labels(&mut conn, &labels).expect("again");
        assert_eq!((again.labels_added, again.labels_kept), (0, 2));
        assert_eq!(stored(&conn, "q-1").as_deref(), Some("Q mine"));
        assert_eq!(stored(&conn, "q-2").as_deref(), Some("Q tron"));
    }
}
