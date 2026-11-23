//! Overlay import tests (overlay spec section 12): a 0.x install (database
//! from `fixtures/0x/schema.sql` and a settings.json) with deck labels and a
//! custom background, read with `read_overlay_at` and written after the
//! decks, as the app does. Fake names start with Q.

use std::path::PathBuf;

use fixture_tools::TestDir;
use importer::filter_settings;
use importer::overlay_read::{LegacyBackground, read_overlay_at};
use importer::read_database;
use rusqlite::Connection;
use store::import::import_matches;
use store::import_decks::import_decks;
use store::import_overlay::{LegacyDeckLabel, OverlayImportCounts, import_overlay_labels};

const SCHEMA: &str = include_str!("../../../fixtures/0x/schema.sql");
const NOW: i64 = 1_800_000_000_000;

/// A 0.x install: `<dir>/legacy/database/nativephp.sqlite` with three
/// constructed decks and a limited one.
fn legacy_install(dir: &TestDir) -> PathBuf {
    dir.write("legacy/database/.keep", b"");
    let path = dir.join("legacy/database/nativephp.sqlite");
    let conn = Connection::open(&path).expect("open 0.x test database");
    conn.execute_batch("PRAGMA foreign_keys = OFF;")
        .expect("no foreign keys");
    conn.execute_batch(SCHEMA).expect("0.x schema");
    conn.execute_batch(
        "INSERT INTO decks (id, mtgo_id, name, format)
         VALUES (1, '1001', 'Q Burn', 'CMODERN'),
                (2, '1002', 'Q Tron', 'CMODERN'),
                (3, '1003', 'Q Jund', 'CMODERN'),
                (4, 'limited:q-draft', 'Q Draft', 'DHOB');",
    )
    .expect("0.x decks");
    path
}

/// The kept settings of a 0.x settings.json.
fn kept(json: &str) -> String {
    filter_settings(json).expect("settings")
}

fn label(mtgo_id: &str, text: &str) -> LegacyDeckLabel {
    LegacyDeckLabel {
        deck_mtgo_id: mtgo_id.to_string(),
        label: text.to_string(),
    }
}

#[test]
fn labels_are_mapped_to_their_decks_mtgo_id_and_unknown_ids_counted() {
    let dir = TestDir::new("overlay-read-labels");
    let path = legacy_install(&dir);
    let settings = kept(
        r#"{"overlay_deck_labels": {
            "1": "  Q stream burn ",
            "3": 1234,
            "4": "Q draft deck",
            "9": "Q deleted deck",
            "x": "Q not an id",
            "2": "   ",
            "5": {"nested": true}
        }}"#,
    );
    let overlay = read_overlay_at(&path, Some(&settings)).expect("read");
    assert_eq!(
        overlay.labels,
        vec![
            label("1001", "  Q stream burn "),
            label("1003", "1234"),
            label("limited:q-draft", "Q draft deck"),
        ]
    );
    assert_eq!(
        overlay.labels_unknown, 2,
        "deck 9 and key x name no 0.x deck"
    );
    assert_eq!(overlay.background, LegacyBackground::NotSet);
}

#[test]
fn a_label_list_is_keyed_by_position() {
    let dir = TestDir::new("overlay-read-list");
    let path = legacy_install(&dir);
    let settings = kept(r#"{"overlay_deck_labels": ["Q zero", "Q one"]}"#);
    let overlay = read_overlay_at(&path, Some(&settings)).expect("read");
    assert_eq!(overlay.labels, vec![label("1001", "Q one")]);
    assert_eq!(overlay.labels_unknown, 1);
}

#[test]
fn no_settings_or_no_labels_read_nothing() {
    let dir = TestDir::new("overlay-read-none");
    let path = legacy_install(&dir);
    for settings in [None, Some(kept("{}")), Some("not json".to_string())] {
        let overlay = read_overlay_at(&path, settings.as_deref()).expect("read");
        assert!(overlay.labels.is_empty());
        assert_eq!(overlay.labels_unknown, 0);
        assert_eq!(overlay.background, LegacyBackground::NotSet);
    }
}

#[test]
fn the_background_is_named_in_0xs_overlay_folder() {
    let dir = TestDir::new("overlay-read-background");
    let path = legacy_install(&dir);
    let overlay = read_overlay_at(
        &path,
        Some(&kept(
            r#"{"overlay_background_path": "background-1767225600.png"}"#,
        )),
    )
    .expect("read");
    assert_eq!(
        overlay.background,
        LegacyBackground::File(dir.join("legacy/storage/app/overlay/background-1767225600.png"))
    );
    for name in ["../private/settings.json", "q\\bg.png", ".."] {
        let json = serde_json::json!({ "overlay_background_path": name }).to_string();
        let overlay = read_overlay_at(&path, Some(&kept(&json))).expect("read");
        assert_eq!(overlay.background, LegacyBackground::Unusable, "{name}");
    }
    let empty =
        read_overlay_at(&path, Some(&kept(r#"{"overlay_background_path": ""}"#))).expect("read");
    assert_eq!(empty.background, LegacyBackground::NotSet);
}

#[test]
fn imported_labels_land_on_v1_decks_and_a_v1_label_is_kept_on_rerun() {
    let dir = TestDir::new("overlay-import-labels");
    let path = legacy_install(&dir);
    let mut v1 = store::open_writer(&dir.join("mymtgo.sqlite")).expect("v1 database");
    let read = read_database(&path).expect("read");
    import_matches(&mut v1, &read.matches, NOW).expect("matches");
    import_decks(&mut v1, &read.decks.expect("decks").data, NOW).expect("decks");
    let settings = kept(
        r#"{"overlay_deck_labels": {"1": " Q stream burn ", "2": "Q tron", "4": "Q draft", "9": "Q gone"}}"#,
    );
    let overlay = read_overlay_at(&path, Some(&settings)).expect("read");

    let first = import_overlay_labels(&mut v1, &overlay.labels).expect("labels");
    assert_eq!(
        first,
        OverlayImportCounts {
            labels_added: 2,
            labels_kept: 0,
            labels_unknown: 1,
        },
        "the limited deck is no v1 deck row"
    );
    let label_of = |conn: &Connection, mtgo_id: &str| -> Option<String> {
        conn.query_row(
            "SELECT overlay_label FROM decks WHERE mtgo_id = ?1",
            [mtgo_id],
            |row| row.get(0),
        )
        .expect("deck")
    };
    assert_eq!(label_of(&v1, "1001").as_deref(), Some("Q stream burn"));
    assert_eq!(label_of(&v1, "1002").as_deref(), Some("Q tron"));

    // The streamer renamed the card in v1: a rerun keeps it.
    v1.execute(
        "UPDATE decks SET overlay_label = 'Q renamed' WHERE mtgo_id = '1001'",
        [],
    )
    .expect("v1 label");
    let again = import_overlay_labels(&mut v1, &overlay.labels).expect("again");
    assert_eq!((again.labels_added, again.labels_kept), (0, 2));
    assert_eq!(label_of(&v1, "1001").as_deref(), Some("Q renamed"));
}
