//! Dev-only comparison of v1's card rows with 0.x's `card_game_stats`, per
//! imported game (cards spec section 13, import audit section 5.1), so the
//! owner can see what changed before release. Read-only on both databases;
//! counts only, never a card or player name.
//!
//! A 0.x row is one oracle id and side; v1's rows of the same oracle and side
//! (several printings) are summed into one. A v1 row without card data is
//! keyed by its catalog id, so it can only be "only in v1". The alternative
//! cast counts (0.x's 23 method columns, `game_card_casts` rows in v1) are compared
//! per method, under the 0.x column's name.

use std::collections::BTreeMap;

use rusqlite::{Connection, OptionalExtension};
use store::imported_detail::ImportedCardStat;
use sync_bundles::match_bundle::CAST_METHODS;

use crate::ImportError;
use crate::card_stats_read::{CardStatsColumns, card_stats};
use crate::detail_read::match_games;
use crate::read::{numbered_unique_games, read_error};

/// What the comparison found, summed over the imported games.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CompareReport {
    /// Imported games found in both databases.
    pub games: u32,
    /// 0.x games of an imported match that v1 has no game for.
    pub games_missing_in_v1: u32,
    /// Games with at least one row that differs or is only on one side.
    pub games_with_differences: u32,
    pub rows_equal: u32,
    pub rows_differing: u32,
    /// How many differing rows differ in each column (0.x's names).
    pub differing_by_column: BTreeMap<String, u32>,
    pub only_in_v1: u32,
    pub only_in_0x: u32,
}

impl CompareReport {
    /// The report as printed lines.
    pub fn lines(&self) -> Vec<String> {
        let mut lines = vec![
            format!("games compared:          {}", self.games),
            format!("games missing in v1:     {}", self.games_missing_in_v1),
            format!("games with differences:  {}", self.games_with_differences),
            format!("rows equal:              {}", self.rows_equal),
            format!("rows differing:          {}", self.rows_differing),
            format!("rows only in v1:         {}", self.only_in_v1),
            format!("rows only in 0.x:        {}", self.only_in_0x),
        ];
        if !self.differing_by_column.is_empty() {
            lines.push("differing rows by column:".to_string());
            for (column, count) in &self.differing_by_column {
                lines.push(format!("  {column}: {count}"));
            }
        }
        lines
    }
}

/// One card of one side in one game, in 0.x's columns.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Counters {
    quantity: u32,
    kept: u32,
    seen: u32,
    sided_in: bool,
    sided_out: bool,
    cast: u32,
    played: Option<u32>,
    activated: Option<u32>,
    discarded: u32,
    cast_turn: Option<u32>,
    hand_seen: u32,
    graveyard_seen: u32,
    exile_seen: u32,
    battlefield_seen: u32,
    has_zone_data: bool,
    pregame_revealed: bool,
    pregame_played: bool,
    /// The alternative casts above 0, by 0.x column.
    methods: BTreeMap<String, u32>,
}

impl Counters {
    fn of_stat(stat: &ImportedCardStat) -> Counters {
        Counters {
            quantity: stat.quantity,
            kept: stat.kept,
            seen: stat.seen,
            sided_in: stat.sided_in,
            sided_out: stat.sided_out,
            cast: stat.cast,
            played: stat.played,
            activated: stat.activated,
            discarded: stat.discarded,
            cast_turn: stat.cast_turn,
            hand_seen: stat.hand_seen,
            graveyard_seen: stat.graveyard_seen,
            exile_seen: stat.exile_seen,
            battlefield_seen: stat.battlefield_seen,
            has_zone_data: stat.has_zone_data,
            pregame_revealed: stat.pregame_revealed,
            pregame_played: stat.pregame_played,
            methods: stat.methods.iter().cloned().collect(),
        }
    }

    /// Add another printing's row of the same oracle and side.
    fn add(&mut self, other: &Counters) {
        self.quantity += other.quantity;
        self.kept += other.kept;
        self.seen += other.seen;
        self.sided_in |= other.sided_in;
        self.sided_out |= other.sided_out;
        self.cast += other.cast;
        self.played = add_option(self.played, other.played);
        self.activated = add_option(self.activated, other.activated);
        self.discarded += other.discarded;
        self.cast_turn = match (self.cast_turn, other.cast_turn) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        self.hand_seen += other.hand_seen;
        self.graveyard_seen += other.graveyard_seen;
        self.exile_seen += other.exile_seen;
        self.battlefield_seen += other.battlefield_seen;
        self.has_zone_data |= other.has_zone_data;
        self.pregame_revealed |= other.pregame_revealed;
        self.pregame_played |= other.pregame_played;
        for (method, count) in &other.methods {
            *self.methods.entry(method.clone()).or_default() += count;
        }
    }

    /// The 0.x column names in which the two differ.
    fn differences(&self, other: &Counters) -> Vec<&'static str> {
        let checks = [
            ("quantity", self.quantity == other.quantity),
            ("kept", self.kept == other.kept),
            ("seen", self.seen == other.seen),
            ("sided_in", self.sided_in == other.sided_in),
            ("sided_out", self.sided_out == other.sided_out),
            ("cast", self.cast == other.cast),
            ("played", self.played == other.played),
            ("activated", self.activated == other.activated),
            ("discarded", self.discarded == other.discarded),
            ("cast_turn", self.cast_turn == other.cast_turn),
            ("hand_seen", self.hand_seen == other.hand_seen),
            (
                "graveyard_seen",
                self.graveyard_seen == other.graveyard_seen,
            ),
            ("exile_seen", self.exile_seen == other.exile_seen),
            (
                "battlefield_seen",
                self.battlefield_seen == other.battlefield_seen,
            ),
            ("has_zone_data", self.has_zone_data == other.has_zone_data),
            (
                "pregame_revealed",
                self.pregame_revealed == other.pregame_revealed,
            ),
            (
                "pregame_played",
                self.pregame_played == other.pregame_played,
            ),
        ];
        let mut found: Vec<&'static str> = checks
            .iter()
            .filter(|(_, same)| !same)
            .map(|(name, _)| *name)
            .collect();
        for method in CAST_METHODS {
            if self.methods.get(method) != other.methods.get(method) {
                found.push(method);
            }
        }
        found
    }
}

fn add_option(a: Option<u32>, b: Option<u32>) -> Option<u32> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a + b),
        (a, b) => a.or(b),
    }
}

/// A row's key: the oracle id (or `#catalog id` without card data) and
/// whether it is the opponent's.
type RowKey = (String, bool);

/// Compare every imported match's games. Neither database is written.
pub fn compare_card_stats(
    v1: &Connection,
    legacy: &Connection,
) -> Result<CompareReport, ImportError> {
    let columns = CardStatsColumns::read(legacy)?;
    // Read only: dropping the transaction ends it.
    let legacy_tx = legacy.unchecked_transaction().map_err(read_error)?;
    let mut report = CompareReport::default();
    for (match_id, token) in imported_matches(v1)? {
        let legacy_id: Option<i64> = legacy_tx
            .query_row(
                "SELECT id FROM matches WHERE token = ?1 ORDER BY id LIMIT 1",
                [&token],
                |row| row.get(0),
            )
            .optional()
            .map_err(read_error)?;
        let Some(legacy_id) = legacy_id else {
            continue;
        };
        // Numbered as the import numbers them, so the kept games line up
        // with v1's.
        let (numbered, _) = numbered_unique_games(match_games(&legacy_tx, legacy_id)?);
        for kept in numbered {
            let legacy_game = kept.game;
            let Some(v1_game) = v1_game_id(v1, match_id, kept.number)? else {
                report.games_missing_in_v1 += 1;
                continue;
            };
            let mut legacy_rows: BTreeMap<RowKey, Counters> = BTreeMap::new();
            for stat in card_stats(&legacy_tx, &columns, legacy_game)? {
                let key = (stat.oracle_id.clone(), stat.opponent);
                legacy_rows
                    .entry(key)
                    .or_default()
                    .add(&Counters::of_stat(&stat));
            }
            let v1_rows = v1_rows(v1, v1_game)?;
            compare_game(&mut report, &legacy_rows, &v1_rows);
        }
    }
    Ok(report)
}

fn compare_game(
    report: &mut CompareReport,
    legacy_rows: &BTreeMap<RowKey, Counters>,
    v1_rows: &BTreeMap<RowKey, Counters>,
) {
    report.games += 1;
    let mut differs = false;
    for (key, legacy) in legacy_rows {
        let Some(mine) = v1_rows.get(key) else {
            report.only_in_0x += 1;
            differs = true;
            continue;
        };
        let columns = legacy.differences(mine);
        if columns.is_empty() {
            report.rows_equal += 1;
        } else {
            report.rows_differing += 1;
            differs = true;
            for column in columns {
                *report
                    .differing_by_column
                    .entry(column.to_string())
                    .or_default() += 1;
            }
        }
    }
    for key in v1_rows.keys() {
        if !legacy_rows.contains_key(key) {
            report.only_in_v1 += 1;
            differs = true;
        }
    }
    if differs {
        report.games_with_differences += 1;
    }
}

/// v1's 0.x-imported matches: `(id, token)`.
fn imported_matches(v1: &Connection) -> Result<Vec<(i64, String)>, ImportError> {
    let mut stmt = v1
        .prepare("SELECT id, token FROM matches WHERE source = 'imported' ORDER BY id")
        .map_err(read_error)?;
    let rows = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .map_err(read_error)?;
    let mut found = Vec::new();
    for row in rows {
        found.push(row.map_err(read_error)?);
    }
    Ok(found)
}

fn v1_game_id(v1: &Connection, match_id: i64, number: u32) -> Result<Option<i64>, ImportError> {
    v1.query_row(
        "SELECT id FROM games WHERE match_id = ?1 AND game_number = ?2",
        rusqlite::params![match_id, number],
        |row| row.get(0),
    )
    .optional()
    .map_err(read_error)
}

/// v1's rows of one game in 0.x's columns, printings of one oracle summed.
fn v1_rows(v1: &Connection, game_id: i64) -> Result<BTreeMap<RowKey, Counters>, ImportError> {
    let mut stmt = v1
        .prepare(
            "SELECT gc.side, gc.catalog_id, c.oracle_id, gc.main_qty, gc.starting_hand, gc.seen,
                 gc.sided_in, gc.sided_out, gc.cast, gc.played, gc.activated, gc.discarded,
                 gc.cast_turn, gc.hand_seen, gc.graveyard_seen, gc.exile_seen,
                 gc.battlefield_seen, gc.has_zone_data, gc.pregame_revealed, gc.pregame_played
             FROM game_cards gc LEFT JOIN cards c ON c.catalog_id = gc.catalog_id
             WHERE gc.game_id = ?1",
        )
        .map_err(read_error)?;
    let rows = stmt
        .query_map([game_id], |row| {
            let side: String = row.get(0)?;
            let catalog_id: i64 = row.get(1)?;
            let oracle_id: Option<String> = row.get(2)?;
            let key = (
                oracle_id.unwrap_or_else(|| format!("#{catalog_id}")),
                side == "opponent",
            );
            let counters = Counters {
                quantity: row.get(3)?,
                kept: row.get(4)?,
                seen: row.get(5)?,
                sided_in: row.get::<_, u32>(6)? > 0,
                sided_out: row.get::<_, u32>(7)? > 0,
                cast: row.get(8)?,
                played: row.get(9)?,
                activated: row.get(10)?,
                discarded: row.get(11)?,
                cast_turn: row.get(12)?,
                hand_seen: row.get(13)?,
                graveyard_seen: row.get(14)?,
                exile_seen: row.get(15)?,
                battlefield_seen: row.get(16)?,
                has_zone_data: row.get::<_, u32>(17)? > 0,
                pregame_revealed: row.get::<_, u32>(18)? > 0,
                pregame_played: row.get::<_, u32>(19)? > 0,
                methods: BTreeMap::new(),
            };
            Ok((key, counters))
        })
        .map_err(read_error)?;
    let mut found: BTreeMap<RowKey, Counters> = BTreeMap::new();
    for row in rows {
        let (key, counters) = row.map_err(read_error)?;
        found.entry(key).or_default().add(&counters);
    }
    add_v1_casts(v1, game_id, &mut found)?;
    Ok(found)
}

/// v1's alternative casts of one game, summed into the row of the same
/// oracle and side (a cast of a card the rows do not hold is left out: the
/// table's key ties each to a `game_cards` row).
fn add_v1_casts(
    v1: &Connection,
    game_id: i64,
    found: &mut BTreeMap<RowKey, Counters>,
) -> Result<(), ImportError> {
    let mut stmt = v1
        .prepare(
            "SELECT gcc.side, gcc.catalog_id, c.oracle_id, gcc.method, gcc.count
             FROM game_card_casts gcc LEFT JOIN cards c ON c.catalog_id = gcc.catalog_id
             WHERE gcc.game_id = ?1",
        )
        .map_err(read_error)?;
    let rows = stmt
        .query_map([game_id], |row| {
            let side: String = row.get(0)?;
            let catalog_id: i64 = row.get(1)?;
            let oracle_id: Option<String> = row.get(2)?;
            let key = (
                oracle_id.unwrap_or_else(|| format!("#{catalog_id}")),
                side == "opponent",
            );
            Ok((key, row.get::<_, String>(3)?, row.get::<_, u32>(4)?))
        })
        .map_err(read_error)?;
    for row in rows {
        let (key, method, count) = row.map_err(read_error)?;
        if let Some(counters) = found.get_mut(&key) {
            *counters.methods.entry(method).or_default() += count;
        }
    }
    Ok(())
}
