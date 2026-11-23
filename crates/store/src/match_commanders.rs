//! A commander format match's commanders (Arena Brawl), both seats, by
//! catalog id. The game sync writes them from game 1's command zone; the
//! local seat falls back to the commander stored with the deck version (a
//! sideboard row) when the match has no messages. Spec 2026-11-12 section 5.

use rusqlite::{Connection, OptionalExtension, Transaction, params};

use crate::StoreError;

/// The commander formats Arena tracks: Historic Brawl and Brawl.
const COMMANDER_FORMATS: [&str; 2] = ["ARENA_HISTORIC_BRAWL", "ARENA_BRAWL"];

/// A match's commanders by catalog id, in position order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MatchCommanders {
    pub player: Vec<i64>,
    pub opponent: Vec<i64>,
}

/// Whether a stored match format is a commander format.
pub fn is_commander_format(format: Option<&str>) -> bool {
    format.is_some_and(|format| COMMANDER_FORMATS.contains(&format))
}

/// The match's stored commanders; empty lists when it has none.
pub fn commanders_of(conn: &Connection, match_id: i64) -> Result<MatchCommanders, StoreError> {
    let mut statement = conn.prepare_cached(
        "SELECT seat, catalog_id FROM match_commanders
         WHERE match_id = ?1 ORDER BY seat, position",
    )?;
    let rows = statement.query_map([match_id], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
    })?;
    let mut commanders = MatchCommanders::default();
    for row in rows {
        let (seat, catalog_id) = row?;
        if seat == "player" {
            commanders.player.push(catalog_id);
        } else {
            commanders.opponent.push(catalog_id);
        }
    }
    Ok(commanders)
}

/// What the sync needs about a match: its stored format and the commander
/// rows (sideboard cards) of its deck version.
pub(crate) struct MatchDeckInfo {
    pub commander_format: bool,
    pub deck_commanders: Vec<i64>,
}

pub(crate) fn match_deck_info(
    tx: &Transaction,
    match_id: i64,
) -> Result<MatchDeckInfo, StoreError> {
    let row: Option<(Option<String>, Option<String>)> = tx
        .prepare_cached(
            "SELECT m.format, dv.signature FROM matches m
             LEFT JOIN deck_versions dv ON dv.id = m.deck_version_id
             WHERE m.id = ?1",
        )?
        .query_row([match_id], |row| Ok((row.get(0)?, row.get(1)?)))
        .optional()?;
    let (format, signature) = row.unwrap_or((None, None));
    let deck_commanders = signature
        .map(|signature| {
            decks::decode_signature(&signature)
                .into_iter()
                .filter(|card| card.sideboard)
                .map(|card| i64::from(card.catalog_id))
                .take(2)
                .collect()
        })
        .unwrap_or_default();
    Ok(MatchDeckInfo {
        commander_format: is_commander_format(format.as_deref()),
        deck_commanders,
    })
}

/// Replace the match's commander rows with `wanted` (delete, then insert).
/// Each commander gets a `cards` stub, so the cards worker resolves it even
/// when no game row or deck version names it (detection waits for it).
/// Returns whether the stored commanders changed.
pub(crate) fn replace_commanders(
    tx: &Transaction,
    match_id: i64,
    wanted: &MatchCommanders,
) -> Result<bool, StoreError> {
    let stub_ids: Vec<u32> = wanted
        .player
        .iter()
        .chain(&wanted.opponent)
        .filter_map(|id| u32::try_from(*id).ok())
        .collect();
    crate::cards::ensure_id_stubs(tx, &stub_ids)?;
    if commanders_of(tx, match_id)? == *wanted {
        return Ok(false);
    }
    tx.prepare_cached("DELETE FROM match_commanders WHERE match_id = ?1")?
        .execute([match_id])?;
    let mut insert = tx.prepare_cached(
        "INSERT INTO match_commanders (match_id, seat, position, catalog_id)
         VALUES (?1, ?2, ?3, ?4)",
    )?;
    for (seat, ids) in [("player", &wanted.player), ("opponent", &wanted.opponent)] {
        for (position, catalog_id) in ids.iter().take(2).enumerate() {
            insert.execute(params![match_id, seat, position as i64, catalog_id])?;
        }
    }
    Ok(true)
}

/// Clear the match's detection so it runs again on the new commanders.
pub(crate) fn ask_detection_again(tx: &Transaction, match_id: i64) -> Result<(), StoreError> {
    tx.prepare_cached(
        "UPDATE matches SET archetypes_detected_at = NULL
         WHERE id = ?1 AND archetypes_detected_at IS NOT NULL",
    )?
    .execute([match_id])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The one commander format rule: the Arena codes only. MTGO Brawl
    /// (`CBRAWL`) keeps its sideboard and the usual detection.
    #[test]
    fn only_the_arena_brawl_codes_are_commander_formats() {
        assert!(is_commander_format(Some("ARENA_HISTORIC_BRAWL")));
        assert!(is_commander_format(Some("ARENA_BRAWL")));
        for format in ["CBRAWL", "ARENA_HISTORIC", "historic_brawl", "brawl", ""] {
            assert!(!is_commander_format(Some(format)), "{format}");
        }
        assert!(!is_commander_format(None));
    }
}
