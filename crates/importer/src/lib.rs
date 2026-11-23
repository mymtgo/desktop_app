//! Reads a MyMTGO 0.x (Laravel/NativePHP) database into matches the v1 store
//! can import (importer spec). The 0.x database is opened read-only and never
//! written.

pub mod account_read;
pub mod api_read;
pub mod archetype_read;
pub mod bundle_read;
pub mod card_stats_read;
pub mod compare_card_stats;
pub mod deck_read;
pub mod detail_read;
mod endstep_manual;
pub mod league_read;
pub mod ledger;
mod legacy_signature;
pub mod limited_read;
pub mod locate;
mod manual_read;
pub mod overlay_read;
mod parity_rules;
pub mod read;
pub mod settings;
mod sql;

pub use deck_read::DeckRead;
pub use detail_read::DetailReader;
pub use league_read::LeagueRead;
pub use limited_read::LimitedRead;
pub use locate::{Os, candidate_paths, current_os, detect, settings_path};
pub use read::{ImportError, NotAccountMatches, ReadResult, Skipped, read_database};
pub use settings::filter_settings;

/// Every `(table, column)` the readers select or join on, for the coverage
/// ledger test.
pub fn read_columns() -> Vec<(&'static str, &'static str)> {
    let lists: [&[(&str, &[&'static str])]; 10] = [
        &api_read::READ_COLUMNS,
        &archetype_read::READ_COLUMNS,
        &read::READ_COLUMNS,
        &card_stats_read::READ_COLUMNS,
        &account_read::READ_COLUMNS,
        &deck_read::READ_COLUMNS,
        &league_read::READ_COLUMNS,
        &detail_read::READ_COLUMNS,
        &limited_read::READ_COLUMNS,
        &overlay_read::READ_COLUMNS,
    ];
    let mut columns = Vec::new();
    for list in lists {
        for (table, names) in list {
            for name in *names {
                columns.push((*table, *name));
            }
        }
    }
    columns
}
