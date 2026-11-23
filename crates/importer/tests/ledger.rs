//! The coverage ledger (plan O task 3): every column of the full 0.x schema has
//! exactly one disposition, every `Imported` column is read by a reader, and
//! every `Deferred` row names a real destination.

use std::collections::BTreeSet;

use importer::ledger::{DEFERRED_TARGETS, Disposition, entries, settings_entries};
use importer::{filter_settings, read_columns};
use rusqlite::Connection;

const SCHEMA: &str = include_str!("../../../fixtures/0x/schema.sql");

/// Every `(table, column)` of the fixture schema, SQLite internals left out.
fn schema_columns() -> BTreeSet<(String, String)> {
    let conn = Connection::open_in_memory().expect("memory database");
    conn.execute_batch(SCHEMA).expect("0.x schema");
    let mut tables = conn
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'")
        .expect("tables");
    let names: Vec<String> = tables
        .query_map([], |row| row.get(0))
        .expect("table names")
        .map(|name| name.expect("name"))
        .collect();
    let mut columns = BTreeSet::new();
    for table in names {
        let mut info = conn
            .prepare(&format!("PRAGMA table_info(\"{table}\")"))
            .expect("table info");
        let names: Vec<String> = info
            .query_map([], |row| row.get(1))
            .expect("columns")
            .map(|name| name.expect("column"))
            .collect();
        for column in names {
            columns.insert((table.clone(), column));
        }
    }
    columns
}

#[test]
fn the_fixture_is_the_full_schema_without_data() {
    let columns = schema_columns();
    let tables: BTreeSet<&str> = columns.iter().map(|(table, _)| table.as_str()).collect();
    assert_eq!(tables.len(), 42, "the 42 tables of the final 0.x schema");
    assert!(!SCHEMA.to_uppercase().contains("INSERT INTO"));
}

#[test]
fn every_fixture_column_has_exactly_one_ledger_entry() {
    let mut seen = BTreeSet::new();
    for entry in entries() {
        let key = (entry.table.to_string(), entry.column.to_string());
        assert!(
            seen.insert(key),
            "{}.{} has two ledger entries",
            entry.table,
            entry.column
        );
    }
    let schema = schema_columns();
    let missing: Vec<_> = schema.difference(&seen).collect();
    assert!(
        missing.is_empty(),
        "columns without a ledger entry: {missing:?}"
    );
    let unknown: Vec<_> = seen.difference(&schema).collect();
    assert!(
        unknown.is_empty(),
        "ledger entries for no column: {unknown:?}"
    );
}

#[test]
fn every_imported_column_is_selected_by_a_reader() {
    let read: BTreeSet<(&str, &str)> = read_columns().into_iter().collect();
    let mut imported = 0;
    for entry in entries() {
        if matches!(entry.disposition, Disposition::Imported(_)) {
            imported += 1;
            assert!(
                read.contains(&(entry.table, entry.column)),
                "{}.{} is Imported but no reader selects it",
                entry.table,
                entry.column
            );
        }
    }
    assert!(imported > 0, "the ledger has Imported rows");
}

#[test]
fn every_deferred_row_names_a_real_destination_and_every_note_is_filled() {
    for entry in entries() {
        match entry.disposition {
            Disposition::Imported(home) => {
                assert!(
                    !home.is_empty(),
                    "{}.{} needs a home",
                    entry.table,
                    entry.column
                )
            }
            Disposition::Deferred(target, note) => {
                assert!(
                    DEFERRED_TARGETS.contains(&target),
                    "{}.{} defers to {target}",
                    entry.table,
                    entry.column
                );
                assert!(
                    !note.is_empty(),
                    "{}.{} needs a note",
                    entry.table,
                    entry.column
                );
            }
            Disposition::Skipped(reason) => {
                assert!(
                    !reason.is_empty(),
                    "{}.{} needs a reason",
                    entry.table,
                    entry.column
                )
            }
        }
    }
}

// Plan Q task 12: every 07 column has its home or its reason now.
#[test]
fn no_07_row_is_left_deferred() {
    let left: Vec<String> = entries()
        .into_iter()
        .filter(|entry| matches!(entry.disposition, Disposition::Deferred("07", _)))
        .map(|entry| format!("{}.{}", entry.table, entry.column))
        .collect();
    assert!(left.is_empty(), "07 rows still deferred: {left:?}");
}

// Issue #310: the last deferred rows (accounts.active and tracked) have
// their home, so every 0.x column is imported or skipped for a reason.
#[test]
fn no_row_is_left_deferred() {
    let left: Vec<String> = entries()
        .into_iter()
        .filter(|entry| matches!(entry.disposition, Disposition::Deferred(..)))
        .map(|entry| format!("{}.{}", entry.table, entry.column))
        .collect();
    assert!(left.is_empty(), "rows still deferred: {left:?}");
}

// Plan V task 3: the replay links have their home (matches) or their
// reason (the pre-release per-game links), and frame times are imported.
#[test]
fn no_11_row_is_left_deferred() {
    let left: Vec<String> = entries()
        .into_iter()
        .filter(|entry| matches!(entry.disposition, Disposition::Deferred("11", _)))
        .map(|entry| format!("{}.{}", entry.table, entry.column))
        .collect();
    assert!(left.is_empty(), "11 rows still deferred: {left:?}");
    for entry in entries() {
        if entry.table == "games" && entry.column.starts_with("replay_share") {
            assert!(
                matches!(entry.disposition, Disposition::Skipped(_)),
                "games.{}",
                entry.column
            );
        }
        if entry.table == "game_timelines" && entry.column == "timestamp" {
            assert!(matches!(entry.disposition, Disposition::Imported(_)));
        }
    }
}

// Plan S task 3: every 06 card column has its home or its reason now.
#[test]
fn the_card_columns_are_imported_or_left_to_v1() {
    let of = |column: &str| {
        entries()
            .into_iter()
            .find(|entry| entry.table == "cards" && entry.column == column)
            .map(|entry| entry.disposition)
    };
    assert!(matches!(of("scryfall_id"), Some(Disposition::Imported(_))));
    for column in ["local_image", "local_art_crop"] {
        let Some(Disposition::Skipped(reason)) = of(column) else {
            panic!("cards.{column} is not skipped");
        };
        assert!(
            reason.contains("copied or re-downloaded by v1"),
            "cards.{column}: {reason}"
        );
    }
    let left: Vec<String> = entries()
        .into_iter()
        .filter(|entry| {
            entry.table == "cards" && matches!(entry.disposition, Disposition::Deferred(..))
        })
        .map(|entry| entry.column.to_string())
        .collect();
    assert!(left.is_empty(), "cards columns still deferred: {left:?}");
}

// Plan T task 14: every 09 column has its home or its reason now.
#[test]
fn no_09_row_is_left_deferred() {
    let left: Vec<String> = entries()
        .into_iter()
        .filter(|entry| matches!(entry.disposition, Disposition::Deferred("09", _)))
        .map(|entry| format!("{}.{}", entry.table, entry.column))
        .collect();
    assert!(left.is_empty(), "09 rows still deferred: {left:?}");
}

// Plan T task 14: what comes over of 0.x's archetype data and why the rest
// does not (archetypes spec sections 15 and 21.5).
#[test]
fn the_archetype_columns_are_imported_or_skipped_with_their_reason() {
    let of = |table: &str, column: &str| {
        entries()
            .into_iter()
            .find(|entry| entry.table == table && entry.column == column)
            .map(|entry| entry.disposition)
            .unwrap_or_else(|| panic!("{table}.{column} has no entry"))
    };
    let imported = [
        ("archetypes", "uuid"),
        ("archetypes", "merged_into_id"),
        ("archetype_cards", "card_id"),
        ("archetype_decks", "archetype_id"),
        ("archetype_deck_cards", "card_id"),
        ("match_archetypes", "player_id"),
        ("match_archetypes", "manual"),
        ("decks", "archetype_id"),
        ("sideboard_guides", "deck_id"),
        ("sideboard_guide_cards", "direction"),
        ("deck_archetype_notes", "body"),
    ];
    for (table, column) in imported {
        assert!(
            matches!(of(table, column), Disposition::Imported(_)),
            "{table}.{column} is not imported"
        );
    }
    let skipped = [
        (
            "archetypes",
            "decklist_downloaded_at",
            "fetched as definitions",
        ),
        (
            "archetype_decks",
            "seen_count",
            "decklists are fetched live",
        ),
        ("match_archetypes", "archetype_deck_id", "no sample decks"),
        ("archetype_match_attempts", "payload", "debug log"),
    ];
    for (table, column, reason) in skipped {
        let Disposition::Skipped(text) = of(table, column) else {
            panic!("{table}.{column} is not skipped");
        };
        assert!(text.contains(reason), "{table}.{column}: {text}");
    }
}

// Overlay spec section 12: the 0.x settings.json keys of the overlays, each
// with its v1 home or its reason.
#[test]
fn the_overlay_settings_keys_are_imported_or_skipped_with_their_reason() {
    let of = |key: &str| {
        settings_entries()
            .into_iter()
            .find(|entry| entry.key == key)
            .map(|entry| entry.disposition)
            .unwrap_or_else(|| panic!("{key} has no entry"))
    };
    let imported = [
        ("overlay_deck_labels", "decks.overlay_label"),
        ("overlay_background_path", "<app data>/overlay/"),
        ("game_overlay", "Settings"),
        ("overlay_size", "Settings"),
        ("overlay_collapsed", "UI state"),
    ];
    for (key, home) in imported {
        let Disposition::Imported(text) = of(key) else {
            panic!("{key} is not imported");
        };
        assert!(text.contains(home), "{key}: {text}");
    }
    let Disposition::Imported(background) = of("overlay_background_path") else {
        panic!("the background is imported");
    };
    assert!(background.contains("skipped"), "{background}");
    for key in [
        "overlay_publish_error",
        "overlay_background_remote",
        "overlay_last_published_at",
        "app_server_url",
    ] {
        assert!(
            matches!(of(key), Disposition::Skipped(_)),
            "{key} is not skipped"
        );
    }
}

// The settings ledger and the settings filter agree: an imported key is
// kept in `legacy_0x_settings`, a skipped one is dropped.
#[test]
fn the_settings_ledger_agrees_with_the_settings_filter() {
    let mut all = serde_json::Map::new();
    for entry in settings_entries() {
        all.insert(entry.key.to_string(), serde_json::json!(1));
    }
    let kept: serde_json::Value = serde_json::from_str(
        &filter_settings(&serde_json::Value::Object(all).to_string()).expect("filter"),
    )
    .expect("json");
    for entry in settings_entries() {
        let is_kept = kept.get(entry.key).is_some();
        match entry.disposition {
            Disposition::Imported(_) => assert!(is_kept, "{} is imported but dropped", entry.key),
            Disposition::Skipped(_) => assert!(!is_kept, "{} is skipped but kept", entry.key),
            Disposition::Deferred(..) => panic!("{} is deferred", entry.key),
        }
    }
}
