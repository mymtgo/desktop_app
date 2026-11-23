//! Endstep card identity (endstep support spec, section 7). Endstep has no
//! card ids of its own: a card is its name plus printing. Each key seen gets
//! a row here, and its catalog id is `cards::endstep_catalog_id(row id)`, so
//! `cards`, `game_cards` and deck rows can hold Endstep cards like Arena's.

use std::collections::BTreeMap;

use endstep_protocol::EndstepCardKey;
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use sync_bundles::EndstepCardRef;

use crate::StoreError;
use crate::cards::{CardRef, ensure_stubs};

/// The catalog id of each key, in input order: a key seen before keeps its
/// id, a new one gets the next. Each card gets a `cards` stub with its name
/// so it shows by name before any card data. The set code is stored lower
/// case; empty strings mean the printing is unknown.
///
/// The transaction must be on a connection from `open_writer` (the stubs
/// are counted there, see `ensure_stubs`).
pub fn endstep_card_ids(
    tx: &Transaction,
    keys: &[EndstepCardKey],
    now_ms: i64,
) -> Result<Vec<u32>, StoreError> {
    let mut find = tx.prepare_cached(
        "SELECT id FROM endstep_cards
         WHERE name = ?1 AND set_code = ?2 AND collector_number = ?3",
    )?;
    let mut insert = tx.prepare_cached(
        "INSERT INTO endstep_cards (name, set_code, collector_number, first_seen_at)
         VALUES (?1, ?2, ?3, ?4) RETURNING id",
    )?;
    let mut ids = Vec::new();
    let mut refs = Vec::new();
    for key in keys {
        let set_code = key.set_code.to_lowercase();
        let lookup = params![key.name, set_code, key.collector_number];
        let found: Option<i64> = find.query_row(lookup, |row| row.get(0)).optional()?;
        let row_id = match found {
            Some(row_id) => row_id,
            None => insert.query_row(
                params![key.name, set_code, key.collector_number, now_ms],
                |row| row.get(0),
            )?,
        };
        let catalog_id = catalog_id_of(row_id)?;
        ids.push(catalog_id);
        refs.push(CardRef {
            catalog_id: i64::from(catalog_id),
            name: Some(key.name.clone()),
        });
    }
    ensure_stubs(tx, &refs)?;
    Ok(ids)
}

/// The `endstep_cards` name of each Endstep catalog id in `catalog_ids`
/// (the card's front face as Endstep showed it). An id outside the Endstep
/// range, or without a row, is left out.
pub fn names_of(
    conn: &Connection,
    catalog_ids: &[u32],
) -> Result<BTreeMap<u32, String>, StoreError> {
    let mut find = conn.prepare_cached("SELECT name FROM endstep_cards WHERE id = ?1")?;
    let mut names = BTreeMap::new();
    for &catalog_id in catalog_ids {
        let Some(row) = cards::endstep_row_of(i64::from(catalog_id)) else {
            continue;
        };
        let name: Option<String> = find.query_row([row], |row| row.get(0)).optional()?;
        if let Some(name) = name {
            names.insert(catalog_id, name);
        }
    }
    Ok(names)
}

/// The portable ref of each Endstep catalog id in `catalog_ids` (its
/// `endstep_cards` name and printing), for an Endstep bundle (cross-device
/// sync spec section 5.3). An id outside the Endstep range, or without a
/// row, is left out.
pub fn refs_of(
    conn: &Connection,
    catalog_ids: &[u32],
) -> Result<BTreeMap<u32, EndstepCardRef>, StoreError> {
    let mut find = conn.prepare_cached(
        "SELECT name, set_code, collector_number FROM endstep_cards WHERE id = ?1",
    )?;
    let mut refs = BTreeMap::new();
    for &catalog_id in catalog_ids {
        let Some(row) = cards::endstep_row_of(i64::from(catalog_id)) else {
            continue;
        };
        let found = find
            .query_row([row], |row| {
                Ok(EndstepCardRef {
                    name: row.get(0)?,
                    set: row.get(1)?,
                    number: row.get(2)?,
                })
            })
            .optional()?;
        if let Some(card) = found {
            refs.insert(catalog_id, card);
        }
    }
    Ok(refs)
}

/// This device's catalog id of a pulled card ref: the row with its name and
/// printing, made (seen at `now_ms`, with a named `cards` stub) when this
/// device has none, as `endstep_card_ids` makes one.
pub fn catalog_for_ref(
    tx: &Transaction,
    card: &EndstepCardRef,
    now_ms: i64,
) -> Result<u32, StoreError> {
    let key = EndstepCardKey {
        name: card.name.clone(),
        set_code: card.set.clone(),
        collector_number: card.number.clone(),
    };
    let ids = endstep_card_ids(tx, &[key], now_ms)?;
    ids.first()
        .copied()
        .ok_or_else(|| StoreError::Corrupt("no catalog id for an Endstep card".to_string()))
}

/// The catalog id of an `endstep_cards` row: refused once the row ids
/// outgrow the Endstep range.
fn catalog_id_of(row_id: i64) -> Result<u32, StoreError> {
    let row = u32::try_from(row_id)
        .ok()
        .filter(|&row| row < cards::ENDSTEP_ID_OFFSET);
    match row {
        Some(row) => Ok(cards::endstep_catalog_id(row)),
        None => Err(StoreError::Corrupt(format!(
            "endstep_cards row {row_id} is past the Endstep id range"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::db;
    use rusqlite::Connection;

    fn key(name: &str, set_code: &str, collector_number: &str) -> EndstepCardKey {
        EndstepCardKey {
            name: name.to_string(),
            set_code: set_code.to_string(),
            collector_number: collector_number.to_string(),
        }
    }

    fn ids(conn: &mut Connection, keys: &[EndstepCardKey], now_ms: i64) -> Vec<u32> {
        let tx = conn.transaction().expect("tx");
        let ids = endstep_card_ids(&tx, keys, now_ms).expect("ids");
        tx.commit().expect("commit");
        ids
    }

    #[test]
    fn the_same_key_gets_the_same_id() {
        let (_dir, mut conn) = db("endstep-cards-same-key");
        let bolt = key("Lightning Bolt", "M10", "146");
        let first = ids(&mut conn, std::slice::from_ref(&bolt), 1);
        let second = ids(&mut conn, &[bolt.clone(), bolt], 2);
        assert_eq!(second, vec![first[0], first[0]]);
        assert!(cards::endstep_row_of(i64::from(first[0])).is_some());
        // The set code is stored lower case, so either case is one key.
        let lower = ids(&mut conn, &[key("Lightning Bolt", "m10", "146")], 3);
        assert_eq!(lower, first);
        let first_seen: i64 = conn
            .query_row("SELECT first_seen_at FROM endstep_cards", [], |row| {
                row.get(0)
            })
            .expect("one row");
        assert_eq!(first_seen, 1);
    }

    #[test]
    fn printings_are_separate_ids() {
        let (_dir, mut conn) = db("endstep-cards-printings");
        let got = ids(
            &mut conn,
            &[
                key("Lightning Bolt", "m10", "146"),
                key("Lightning Bolt", "2xm", "129"),
                key("Lightning Bolt", "", ""),
            ],
            1,
        );
        assert_eq!(got.len(), 3);
        assert!(got[0] != got[1] && got[1] != got[2] && got[0] != got[2]);
    }

    #[test]
    fn ids_come_back_in_input_order() {
        let (_dir, mut conn) = db("endstep-cards-order");
        let first = ids(&mut conn, &[key("Qb", "", ""), key("Qa", "", "")], 1);
        let again = ids(
            &mut conn,
            &[key("Qa", "", ""), key("Qc", "", ""), key("Qb", "", "")],
            2,
        );
        assert_eq!(again[0], first[1]);
        assert_eq!(again[2], first[0]);
        assert!(!first.contains(&again[1]));
    }

    #[test]
    fn names_of_gives_each_endstep_id_its_name() {
        let (_dir, mut conn) = db("endstep-cards-names-of");
        let got = ids(
            &mut conn,
            &[
                key("Qend", "m10", "146"),
                key("Qend", "2xm", "129"),
                key("Qother", "", ""),
            ],
            1,
        );
        let unknown = cards::endstep_catalog_id(9_999);
        let names = names_of(&conn, &[got[0], got[1], got[2], unknown, 126_519]).expect("names");
        let expected: BTreeMap<u32, String> = BTreeMap::from([
            (got[0], "Qend".to_string()),
            (got[1], "Qend".to_string()),
            (got[2], "Qother".to_string()),
        ]);
        assert_eq!(
            names, expected,
            "an unknown row and an MTGO id are left out"
        );
    }

    fn card_ref(name: &str, set: &str, number: &str) -> EndstepCardRef {
        EndstepCardRef {
            name: name.to_string(),
            set: set.to_string(),
            number: number.to_string(),
        }
    }

    fn catalog(conn: &mut Connection, card: &EndstepCardRef, now_ms: i64) -> u32 {
        let tx = conn.transaction().expect("tx");
        let id = catalog_for_ref(&tx, card, now_ms).expect("catalog id");
        tx.commit().expect("commit");
        id
    }

    #[test]
    fn a_ref_reads_back_to_the_local_catalog_id() {
        let (_dir, mut conn) = db("endstep-cards-ref");
        // Another device made its rows in another order: the ref, not the
        // number, finds the card here.
        let local = ids(
            &mut conn,
            &[key("Qfirst", "", ""), key("Lightning Bolt", "m10", "146")],
            1,
        );
        let bolt = card_ref("Lightning Bolt", "m10", "146");
        assert_eq!(catalog(&mut conn, &bolt, 2), local[1]);
        let refs = refs_of(&conn, &[local[1], local[0], 126_519]).expect("refs");
        assert_eq!(refs.get(&local[1]), Some(&bolt));
        assert_eq!(refs.get(&local[0]), Some(&card_ref("Qfirst", "", "")));
        assert_eq!(refs.len(), 2, "an MTGO id has no ref");
        // An upper case set code is the same printing.
        assert_eq!(
            catalog(&mut conn, &card_ref("Lightning Bolt", "M10", "146"), 3),
            local[1]
        );
    }

    #[test]
    fn a_ref_new_here_gets_a_row_and_a_named_stub() {
        let (_dir, mut conn) = db("endstep-cards-ref-new");
        let card = card_ref("Brazen Borrower", "eld", "39");
        let id = catalog(&mut conn, &card, 5);
        assert!(cards::endstep_row_of(i64::from(id)).is_some());
        assert_eq!(catalog(&mut conn, &card, 6), id, "made once");
        let (name, first_seen): (String, i64) = conn
            .query_row("SELECT name, first_seen_at FROM endstep_cards", [], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .expect("one row");
        assert_eq!((name.as_str(), first_seen), ("Brazen Borrower", 5));
        let stub: Option<String> = conn
            .query_row(
                "SELECT name FROM cards WHERE catalog_id = ?1",
                [id],
                |row| row.get(0),
            )
            .expect("a cards row");
        assert_eq!(stub.as_deref(), Some("Brazen Borrower"));
    }

    #[test]
    fn a_new_card_gets_a_named_stub() {
        let (_dir, mut conn) = db("endstep-cards-stub");
        let got = ids(&mut conn, &[key("Brazen Borrower", "eld", "39")], 1);
        let name: Option<String> = conn
            .query_row(
                "SELECT name FROM cards WHERE catalog_id = ?1",
                [got[0]],
                |row| row.get(0),
            )
            .expect("a cards row");
        assert_eq!(name.as_deref(), Some("Brazen Borrower"));
    }
}
