//! Reading one 0.x game's card stats (`card_game_stats`, target schema
//! section 11): per oracle id and side, the maindeck copies, the starting
//! hand (`kept`), the copies seen, the sideboarding flags, the game log
//! counters, the 23 alternative casts, the pregame flags and the zone counts.
//! The store maps each oracle id to a catalog id and lays the rows over the
//! game's rows (`store::imported_detail`).
//!
//! Values are read loosely, as in `read`: a column an older 0.x lacks reads
//! as NULL (0 or unknown). A row without an oracle id is kept with an empty
//! one, so the store counts it among the rows it leaves out.

use rusqlite::Transaction;
use rusqlite::types::Value;
use store::imported_detail::ImportedCardStat;
use sync_bundles::match_bundle::CAST_METHODS;

use crate::read::{ImportError, read_error};
use crate::sql::{as_bool, as_integer, as_text, has_all, select_list, table_columns, value};

/// The other columns selected, in the order `stat_of` reads them.
const STAT_COLUMNS: [&str; 20] = [
    "oracle_id",
    "opponent",
    "quantity",
    "kept",
    "seen",
    "sided_in",
    "sided_out",
    "cast",
    "played",
    "activated",
    "discarded",
    "cast_turn",
    "hand_seen",
    "graveyard_seen",
    "exile_seen",
    "battlefield_seen",
    "has_zone_data",
    "pregame_revealed",
    "pregame_played",
    "game_id",
];

/// Every `(table, columns)` this reader selects or filters on.
pub const READ_COLUMNS: [(&str, &[&str]); 2] = [
    ("card_game_stats", &STAT_COLUMNS),
    ("card_game_stats", &CAST_METHODS),
];

/// Which of the wanted columns this 0.x database has.
pub(crate) struct CardStatsColumns {
    /// `None` when there is no `card_game_stats` table with an oracle id and
    /// a game id.
    present: Option<Vec<String>>,
}

impl CardStatsColumns {
    pub(crate) fn read(conn: &rusqlite::Connection) -> Result<CardStatsColumns, ImportError> {
        let present = table_columns(conn, "card_game_stats")?;
        let usable = has_all(&present, &["oracle_id", "game_id"]);
        Ok(CardStatsColumns {
            present: usable.then_some(present),
        })
    }
}

/// The game's card stats rows, in stored order.
pub(crate) fn card_stats(
    tx: &Transaction,
    columns: &CardStatsColumns,
    game_id: i64,
) -> Result<Vec<ImportedCardStat>, ImportError> {
    let Some(present) = &columns.present else {
        return Ok(Vec::new());
    };
    // A column an older 0.x lacks is selected as NULL.
    let sql = format!(
        "SELECT {}, {} FROM card_game_stats WHERE \"game_id\" = ?1 ORDER BY rowid",
        select_list(present, &STAT_COLUMNS),
        select_list(present, &CAST_METHODS)
    );
    let mut stmt = tx.prepare(&sql).map_err(read_error)?;
    let mut rows = stmt.query([game_id]).map_err(read_error)?;
    let mut stats = Vec::new();
    while let Some(row) = rows.next().map_err(read_error)? {
        let mut values = Vec::new();
        for index in 0..STAT_COLUMNS.len() + CAST_METHODS.len() {
            values.push(value(row, index).map_err(read_error)?);
        }
        stats.push(stat_of(values));
    }
    Ok(stats)
}

/// A count: a whole number at least 0 (text that is one counts), else `None`.
fn count(value: &Value) -> Option<u32> {
    as_integer(value.clone()).and_then(|number| u32::try_from(number).ok())
}

/// One row from its selected values (an empty oracle id when it has none).
fn stat_of(values: Vec<Value>) -> ImportedCardStat {
    let at = |index: usize| values.get(index).cloned().unwrap_or(Value::Null);
    let oracle_id = as_text(at(0)).unwrap_or_default().trim().to_string();
    let flag = |index: usize| as_bool(at(index)).unwrap_or(false);
    let number = |index: usize| count(&at(index)).unwrap_or(0);
    let mut methods = Vec::new();
    for (offset, name) in CAST_METHODS.iter().enumerate() {
        let found = count(&at(STAT_COLUMNS.len() + offset)).unwrap_or(0);
        if found > 0 {
            methods.push((name.to_string(), found));
        }
    }
    ImportedCardStat {
        oracle_id,
        opponent: flag(1),
        quantity: number(2),
        kept: number(3),
        seen: number(4),
        sided_in: flag(5),
        sided_out: flag(6),
        cast: number(7),
        played: count(&at(8)),
        activated: count(&at(9)),
        discarded: number(10),
        cast_turn: count(&at(11)),
        hand_seen: number(12),
        graveyard_seen: number(13),
        exile_seen: number(14),
        battlefield_seen: number(15),
        has_zone_data: flag(16),
        pregame_revealed: flag(17),
        pregame_played: flag(18),
        methods,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_row_reads_its_counters_and_methods_and_keeps_a_missing_oracle_empty() {
        let mut values = vec![Value::Null; STAT_COLUMNS.len() + CAST_METHODS.len()];
        assert_eq!(stat_of(values.clone()).oracle_id, "");
        values[0] = Value::Text("q-oracle".to_string());
        values[1] = Value::Integer(1);
        values[3] = Value::Text("2".to_string());
        values[8] = Value::Integer(0);
        values[STAT_COLUMNS.len() + 5] = Value::Integer(2);
        let stat = stat_of(values);
        assert!(stat.opponent);
        assert_eq!(stat.kept, 2);
        assert_eq!(stat.played, Some(0));
        assert_eq!(stat.activated, None);
        assert_eq!(stat.methods, vec![("free_cast".to_string(), 2)]);
    }
}
