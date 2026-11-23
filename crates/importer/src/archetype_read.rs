//! Reading 0.x's archetype data (archetypes spec sections 15 and 21.5): its
//! archetypes with the lists of the manual ones, the archetype of each match
//! seat and deck, the sideboard guides and the matchup notes. Read loosely,
//! like the other readers: a table or column an older 0.x lacks reads as
//! nothing. Which archetypes come over is the store's decision
//! (`store::import_archetypes`); this reader hands it every row.
//!
//! Not read: the sample decks of downloaded archetypes (decklists are
//! fetched live), `archetype_match_attempts` (a debug log) and
//! `archetype_deck_id` (v1 keeps no sample decks, ruling 9).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

use rusqlite::Connection;
use rusqlite::types::Value;
use store::import_archetypes::{
    ImportedArchetypes, LegacyArchetype, LegacyDeckArchetype, LegacyGuide, LegacyGuideCard,
    LegacyListCard, LegacyMatchArchetype, LegacyNote, LegacySource,
};

use crate::read::{ImportError, open, read_error};
use crate::sql::{
    deck_mtgo_ids, flag, has_column, has_columns, int, parse_id, real, rows, select_list,
    table_columns, text, time,
};

/// `archetypes` columns without which a row means nothing.
const ARCHETYPE_KEYS: [&str; 3] = ["id", "uuid", "name"];
/// `archetypes` columns read when present, in the order `read_rows` reads
/// them.
const ARCHETYPE_COLUMNS: [&str; 9] = [
    "format",
    "color_identity",
    "manual",
    "is_fallback",
    "merged_into_id",
    "source_match_id",
    "incomplete",
    "created_at",
    "updated_at",
];

/// Every `(table, columns)` this reader selects or joins on.
pub const READ_COLUMNS: [(&str, &[&str]); 14] = [
    ("archetypes", &ARCHETYPE_KEYS),
    ("archetypes", &ARCHETYPE_COLUMNS),
    ("archetype_decks", &["id", "archetype_id"]),
    (
        "archetype_deck_cards",
        &[
            "id",
            "archetype_deck_id",
            "card_id",
            "quantity",
            "sideboard",
        ],
    ),
    (
        "archetype_cards",
        &["id", "archetype_id", "card_id", "quantity", "sideboard"],
    ),
    ("cards", &["id", "mtgo_id", "oracle_id"]),
    (
        "match_archetypes",
        &[
            "id",
            "archetype_id",
            "mtgo_match_id",
            "player_id",
            "confidence",
            "manual",
        ],
    ),
    ("matches", &["id", "token", "mtgo_id", "format"]),
    ("games", &["id", "match_id"]),
    ("game_player", &["game_id", "player_id", "is_local"]),
    ("decks", &["id", "mtgo_id", "format", "archetype_id"]),
    (
        "sideboard_guides",
        &["id", "deck_id", "archetype_id", "created_at", "updated_at"],
    ),
    (
        "sideboard_guide_cards",
        &[
            "id",
            "sideboard_guide_id",
            "oracle_id",
            "direction",
            "quantity",
        ],
    ),
    (
        "deck_archetype_notes",
        &["id", "deck_id", "archetype_id", "body", "created_at"],
    ),
];

/// 0.x's archetype data from the database at `path`, opened read-only.
pub fn read_archetypes_at(path: &Path) -> Result<ImportedArchetypes, ImportError> {
    read_archetypes(&open(path)?)
}

/// 0.x's archetype data, read in one transaction.
pub fn read_archetypes(conn: &Connection) -> Result<ImportedArchetypes, ImportError> {
    let tx = conn.unchecked_transaction().map_err(read_error)?;
    let mut data = ImportedArchetypes::default();
    if !has_columns(&tx, "archetypes", &ARCHETYPE_KEYS)? {
        drop(tx);
        return Ok(data);
    }
    let tokens = match_tokens(&tx)?;
    let deck_ids = deck_mtgo_ids(&tx)?;
    let hints = format_hints(&tx)?;
    data.archetypes = read_rows(&tx, &tokens, &hints)?;
    add_manual_lists(&tx, &mut data.archetypes)?;
    read_match_rows(&tx, &mut data)?;
    if has_columns(&tx, "decks", &["mtgo_id", "archetype_id"])? {
        for row in rows(
            &tx,
            "SELECT mtgo_id, archetype_id FROM decks WHERE archetype_id IS NOT NULL ORDER BY id",
        )? {
            if let (Some(deck_mtgo_id), Some(archetype)) = (text(&row, 0), int(&row, 1)) {
                data.deck_links.push(LegacyDeckArchetype {
                    deck_mtgo_id,
                    archetype,
                });
            }
        }
    }
    data.guides = read_guides(&tx, &deck_ids)?;
    data.notes = read_notes(&tx, &deck_ids)?;
    drop(tx);
    Ok(data)
}

/// Each 0.x match id's token.
fn match_tokens(conn: &Connection) -> Result<HashMap<i64, String>, ImportError> {
    let mut tokens = HashMap::new();
    if has_columns(conn, "matches", &["id", "token"])? {
        for row in rows(conn, "SELECT id, token FROM matches")? {
            if let (Some(id), Some(token)) = (int(&row, 0), text(&row, 1)) {
                tokens.insert(id, token);
            }
        }
    }
    Ok(tokens)
}

/// For each archetype, the format of the first match (then deck) that uses
/// it: the format of a row 0.x stored without one.
fn format_hints(conn: &Connection) -> Result<HashMap<i64, String>, ImportError> {
    let mut hints: HashMap<i64, String> = HashMap::new();
    let mut sources: Vec<&str> = Vec::new();
    if has_columns(conn, "match_archetypes", &["archetype_id", "mtgo_match_id"])?
        && has_columns(conn, "matches", &["id", "format"])?
    {
        sources.push(
            "SELECT ma.archetype_id, m.format FROM match_archetypes ma
             JOIN matches m ON m.id = ma.mtgo_match_id ORDER BY ma.id",
        );
    }
    if has_columns(conn, "decks", &["format", "archetype_id"])? {
        sources.push(
            "SELECT archetype_id, format FROM decks WHERE archetype_id IS NOT NULL ORDER BY id",
        );
    }
    for sql in sources {
        for row in rows(conn, sql)? {
            if let (Some(archetype), Some(format)) = (int(&row, 0), text(&row, 1)) {
                hints.entry(archetype).or_insert(format);
            }
        }
    }
    Ok(hints)
}

/// Every archetype row, in id order, without lists.
fn read_rows(
    conn: &Connection,
    tokens: &HashMap<i64, String>,
    hints: &HashMap<i64, String>,
) -> Result<Vec<LegacyArchetype>, ImportError> {
    let present = table_columns(conn, "archetypes")?;
    let sql = format!(
        "SELECT id, uuid, name, {} FROM archetypes ORDER BY id",
        select_list(&present, &ARCHETYPE_COLUMNS)
    );
    let mut archetypes = Vec::new();
    for row in rows(conn, &sql)? {
        let (Some(legacy_id), Some(uuid), Some(name)) =
            (int(&row, 0), text(&row, 1), text(&row, 2))
        else {
            continue;
        };
        let uuid = uuid.trim().to_string();
        if uuid.is_empty() {
            continue;
        }
        let source = if flag(&row, 6) {
            LegacySource::Fallback
        } else if flag(&row, 5) {
            LegacySource::Manual
        } else {
            LegacySource::Downloaded
        };
        archetypes.push(LegacyArchetype {
            legacy_id,
            uuid,
            name,
            format: text(&row, 3).filter(|format| !format.trim().is_empty()),
            format_hint: hints.get(&legacy_id).cloned(),
            color_identity: text(&row, 4),
            source,
            merged_into: int(&row, 7),
            source_match_token: int(&row, 8).and_then(|id| tokens.get(&id).cloned()),
            incomplete: flag(&row, 9),
            created_at_ms: time(&row, 10),
            updated_at_ms: time(&row, 11),
            lists: Vec::new(),
        });
    }
    Ok(archetypes)
}

/// One list card from `(card_id, quantity, sideboard)` columns at `start`,
/// with the card's catalog id and oracle id from 0.x `cards` (columns
/// `start + 3`, `start + 4`); a card without a numeric catalog id is left
/// out.
fn list_card(row: &[Value], start: usize) -> Option<LegacyListCard> {
    let catalog_id = text(row, start + 3).as_deref().and_then(parse_id)?;
    Some(LegacyListCard {
        catalog_id,
        oracle_id: text(row, start + 4),
        quantity: int(row, start + 1)?,
        sideboard: flag(row, start + 2),
    })
}

/// The lists of the manual archetypes: their variants in id order, else
/// their legacy card list as list 1 (0.x
/// `backfill_manual_archetype_variants`).
fn add_manual_lists(
    conn: &Connection,
    archetypes: &mut [LegacyArchetype],
) -> Result<(), ImportError> {
    let has_cards = has_columns(conn, "cards", &["id", "mtgo_id", "oracle_id"])?;
    if !has_cards {
        return Ok(());
    }
    // archetype id -> variant id -> cards, both in id order.
    let mut variants: HashMap<i64, BTreeMap<i64, Vec<LegacyListCard>>> = HashMap::new();
    if has_columns(conn, "archetype_decks", &["id", "archetype_id"])?
        && has_columns(
            conn,
            "archetype_deck_cards",
            &["archetype_deck_id", "card_id", "quantity", "sideboard"],
        )?
    {
        for row in rows(
            conn,
            "SELECT id, archetype_id FROM archetype_decks ORDER BY id",
        )? {
            if let (Some(id), Some(archetype)) = (int(&row, 0), int(&row, 1)) {
                variants
                    .entry(archetype)
                    .or_default()
                    .insert(id, Vec::new());
            }
        }
        for row in rows(
            conn,
            "SELECT d.archetype_id, dc.archetype_deck_id, dc.card_id, dc.quantity, dc.sideboard,
                 c.mtgo_id, c.oracle_id
             FROM archetype_deck_cards dc
             JOIN archetype_decks d ON d.id = dc.archetype_deck_id
             JOIN cards c ON c.id = dc.card_id
             ORDER BY dc.id",
        )? {
            let (Some(archetype), Some(variant)) = (int(&row, 0), int(&row, 1)) else {
                continue;
            };
            if let Some(card) = list_card(&row, 2)
                && let Some(cards) = variants
                    .get_mut(&archetype)
                    .and_then(|lists| lists.get_mut(&variant))
            {
                cards.push(card);
            }
        }
    }
    let mut legacy: HashMap<i64, Vec<LegacyListCard>> = HashMap::new();
    if has_columns(
        conn,
        "archetype_cards",
        &["archetype_id", "card_id", "quantity", "sideboard"],
    )? {
        for row in rows(
            conn,
            "SELECT ac.archetype_id, ac.card_id, ac.quantity, ac.sideboard, c.mtgo_id,
                 c.oracle_id
             FROM archetype_cards ac JOIN cards c ON c.id = ac.card_id
             ORDER BY ac.id",
        )? {
            if let (Some(archetype), Some(card)) = (int(&row, 0), list_card(&row, 1)) {
                legacy.entry(archetype).or_default().push(card);
            }
        }
    }
    for archetype in archetypes.iter_mut() {
        if archetype.source != LegacySource::Manual {
            continue;
        }
        match variants.remove(&archetype.legacy_id) {
            Some(lists) if !lists.is_empty() => {
                archetype.lists = lists.into_values().collect();
            }
            _ => {
                if let Some(cards) = legacy.remove(&archetype.legacy_id) {
                    archetype.lists = vec![cards];
                }
            }
        }
    }
    Ok(())
}

/// The match rows query. `+gp.player_id` keeps SQLite off the player
/// index: the local player sits in every game, so that index scans every
/// game per row (about a minute for 5,200 matches, Plan T task 15); by match
/// the seat is a few rows away (guarded by a query plan test, #300 F19).
fn match_rows_sql(manual: &str, seats: bool) -> String {
    let seat = if seats {
        "(SELECT max(gp.is_local) FROM games g JOIN game_player gp ON gp.game_id = g.id
          WHERE g.match_id = ma.mtgo_match_id AND +gp.player_id = ma.player_id)"
    } else {
        "NULL"
    };
    format!(
        "SELECT m.token, m.mtgo_id, {seat}, ma.archetype_id, ma.confidence, {manual}
         FROM match_archetypes ma JOIN matches m ON m.id = ma.mtgo_match_id
         ORDER BY ma.id"
    )
}

/// The match rows: each row's seat from `game_player.is_local` of its
/// player in that match, the first row by id per match and seat.
fn read_match_rows(conn: &Connection, data: &mut ImportedArchetypes) -> Result<(), ImportError> {
    if !has_columns(
        conn,
        "match_archetypes",
        &["archetype_id", "mtgo_match_id", "player_id", "confidence"],
    )? || !has_columns(conn, "matches", &["id", "token", "mtgo_id"])?
    {
        return Ok(());
    }
    let present = table_columns(conn, "match_archetypes")?;
    // Without `manual` (an older 0.x) nothing was set by hand.
    let manual = if has_column(&present, "manual") {
        "ma.manual"
    } else {
        "0"
    };
    let seats = has_columns(conn, "game_player", &["game_id", "player_id", "is_local"])?
        && has_columns(conn, "games", &["id", "match_id"])?;
    let sql = match_rows_sql(manual, seats);
    let mut seen: HashSet<(String, bool)> = HashSet::new();
    for row in rows(conn, &sql)? {
        let (Some(match_token), Some(archetype)) = (text(&row, 0), int(&row, 3)) else {
            continue;
        };
        let Some(is_local) = int(&row, 2) else {
            data.rows_without_seat += 1;
            continue;
        };
        let is_opponent = is_local == 0;
        let key = (match_token.clone(), is_opponent);
        if !seen.insert(key) {
            continue;
        }
        data.match_rows.push(LegacyMatchArchetype {
            match_token,
            mtgo_match_id: text(&row, 1).as_deref().and_then(parse_id),
            is_opponent,
            archetype,
            confidence: real(&row, 4).unwrap_or(0.0),
            manual: flag(&row, 5),
        });
    }
    Ok(())
}

/// The sideboard guides with their cards, by deck MTGO id.
fn read_guides(
    conn: &Connection,
    deck_ids: &HashMap<i64, String>,
) -> Result<Vec<LegacyGuide>, ImportError> {
    if !has_columns(conn, "sideboard_guides", &["id", "deck_id", "archetype_id"])? {
        return Ok(Vec::new());
    }
    let present = table_columns(conn, "sideboard_guides")?;
    let sql = format!(
        "SELECT id, deck_id, archetype_id, {} FROM sideboard_guides ORDER BY id",
        select_list(&present, &["created_at", "updated_at"])
    );
    let mut cards: HashMap<i64, Vec<LegacyGuideCard>> = HashMap::new();
    if has_columns(
        conn,
        "sideboard_guide_cards",
        &["sideboard_guide_id", "oracle_id", "direction", "quantity"],
    )? {
        for row in rows(
            conn,
            "SELECT sideboard_guide_id, oracle_id, direction, quantity
             FROM sideboard_guide_cards ORDER BY id",
        )? {
            if let (Some(guide), Some(oracle_id), Some(direction), Some(quantity)) =
                (int(&row, 0), text(&row, 1), text(&row, 2), int(&row, 3))
            {
                cards.entry(guide).or_default().push(LegacyGuideCard {
                    oracle_id,
                    direction,
                    quantity,
                });
            }
        }
    }
    let mut guides = Vec::new();
    for row in rows(conn, &sql)? {
        let (Some(id), Some(deck), Some(archetype)) = (int(&row, 0), int(&row, 1), int(&row, 2))
        else {
            continue;
        };
        let Some(deck_mtgo_id) = deck_ids.get(&deck).cloned() else {
            continue;
        };
        guides.push(LegacyGuide {
            deck_mtgo_id,
            archetype,
            created_at_ms: time(&row, 3),
            updated_at_ms: time(&row, 4),
            cards: cards.remove(&id).unwrap_or_default(),
        });
    }
    Ok(guides)
}

/// The matchup notes, by deck MTGO id.
fn read_notes(
    conn: &Connection,
    deck_ids: &HashMap<i64, String>,
) -> Result<Vec<LegacyNote>, ImportError> {
    if !has_columns(
        conn,
        "deck_archetype_notes",
        &["deck_id", "archetype_id", "body"],
    )? {
        return Ok(Vec::new());
    }
    let present = table_columns(conn, "deck_archetype_notes")?;
    let sql = format!(
        "SELECT deck_id, archetype_id, body, {} FROM deck_archetype_notes ORDER BY id",
        select_list(&present, &["created_at"])
    );
    let mut notes = Vec::new();
    for row in rows(conn, &sql)? {
        let (Some(deck), Some(archetype), Some(body)) = (int(&row, 0), int(&row, 1), text(&row, 2))
        else {
            continue;
        };
        let Some(deck_mtgo_id) = deck_ids.get(&deck).cloned() else {
            continue;
        };
        notes.push(LegacyNote {
            deck_mtgo_id,
            archetype,
            body,
            created_at_ms: time(&row, 3),
        });
    }
    Ok(notes)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #300 F19: the seat lookup never takes `game_player`'s player index
    /// (on 0.x's own schema, with its indexes).
    #[test]
    fn the_seat_lookup_stays_off_the_player_index() {
        let conn = Connection::open_in_memory().expect("memory");
        conn.execute_batch(include_str!("../../../fixtures/0x/schema.sql"))
            .expect("0.x schema");
        let sql = format!("EXPLAIN QUERY PLAN {}", match_rows_sql("ma.manual", true));
        let mut statement = conn.prepare(&sql).expect("plan");
        let details: Vec<String> = statement
            .query_map([], |row| row.get::<_, String>(3))
            .expect("rows")
            .map(|row| row.expect("detail"))
            .collect();
        assert!(
            details.iter().any(|detail| detail.contains("gp")),
            "the seat lookup is in the plan: {details:?}"
        );
        assert!(
            !details
                .iter()
                .any(|detail| detail.contains("game_player_player_id_index")),
            "{details:?}"
        );
    }
}
