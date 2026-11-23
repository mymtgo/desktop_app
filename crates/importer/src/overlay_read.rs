//! Reading 0.x's overlay data (overlay spec section 12): the league card's
//! deck labels and the custom background file. Both live in 0.x's
//! `settings.json` (kept by `filter_settings` as `legacy_0x_settings`):
//! - `overlay_deck_labels` maps a 0.x deck id to its label. v1 keys decks
//!   by `mtgo_id`, so each id is looked up in 0.x's `decks` table; an id no
//!   0.x deck has is counted, not imported;
//! - `overlay_background_path` names a file in 0.x's `overlay` disk,
//!   `<0.x app data>/storage/app/overlay/`, next to the database's folder.
//!
//! The 0.x database is opened read-only; the background file is only named
//! here, never opened (the app copies it with the upload's checks).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::Value;
use store::import_overlay::LegacyDeckLabel;

use crate::read::{ImportError, open};
use crate::sql::deck_mtgo_ids;

/// `decks` columns read to map a label's deck id to its `mtgo_id`
/// (`sql::deck_mtgo_ids`).
const DECK_COLUMNS: [&str; 2] = ["id", "mtgo_id"];

/// Every `(table, columns)` this reader selects, for the coverage ledger.
pub const READ_COLUMNS: [(&str, &[&str]); 1] = [("decks", &DECK_COLUMNS)];

/// The settings.json key of the deck labels.
pub const DECK_LABELS_KEY: &str = "overlay_deck_labels";
/// The settings.json key of the custom background's file name.
pub const BACKGROUND_KEY: &str = "overlay_background_path";

/// 0.x's custom background, as its setting names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LegacyBackground {
    /// 0.x had no custom background.
    NotSet,
    /// The file the setting names in 0.x's overlay folder (it may be
    /// missing).
    File(PathBuf),
    /// The setting names something that is not a plain file name, so no
    /// file of 0.x's overlay folder.
    Unusable,
}

/// What 0.x's overlay settings bring over.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyOverlay {
    /// Each label whose 0.x deck was found, by the deck's `mtgo_id`.
    pub labels: Vec<LegacyDeckLabel>,
    /// Labels whose key names no 0.x deck (or is not a deck id).
    pub labels_unknown: u32,
    pub background: LegacyBackground,
}

/// Read 0.x's overlay data: `kept_settings` is the kept settings.json
/// object (`None` without one), `db_path` the 0.x database.
pub fn read_overlay_at(
    db_path: &Path,
    kept_settings: Option<&str>,
) -> Result<LegacyOverlay, ImportError> {
    let settings: Value = match kept_settings {
        Some(json) => serde_json::from_str(json).unwrap_or(Value::Null),
        None => Value::Null,
    };
    let entries = label_entries(settings.get(DECK_LABELS_KEY));
    let deck_ids = if entries.is_empty() {
        HashMap::new()
    } else {
        deck_mtgo_ids(&open(db_path)?)?
    };
    let mut overlay = LegacyOverlay {
        labels: Vec::new(),
        labels_unknown: 0,
        background: background(db_path, settings.get(BACKGROUND_KEY)),
    };
    for (key, label) in entries {
        let deck = key
            .trim()
            .parse::<i64>()
            .ok()
            .and_then(|id| deck_ids.get(&id));
        match deck {
            Some(mtgo_id) => overlay.labels.push(LegacyDeckLabel {
                deck_mtgo_id: mtgo_id.clone(),
                label,
            }),
            None => overlay.labels_unknown += 1,
        }
    }
    Ok(overlay)
}

/// The `(deck id key, label)` pairs of 0.x's labels, in key order. 0.x
/// stored a PHP array keyed by deck id: JSON writes it as an object, or as
/// a list when the keys happen to be 0, 1, 2, ... Values are read as PHP
/// `strval` reads a scalar; a blank one is no label.
fn label_entries(value: Option<&Value>) -> Vec<(String, String)> {
    let mut entries = Vec::new();
    match value {
        Some(Value::Object(map)) => {
            for (key, value) in map {
                if let Some(label) = scalar_text(value) {
                    entries.push((key.clone(), label));
                }
            }
        }
        Some(Value::Array(list)) => {
            for (index, value) in list.iter().enumerate() {
                if let Some(label) = scalar_text(value) {
                    entries.push((index.to_string(), label));
                }
            }
        }
        _ => {}
    }
    entries.retain(|(_, label)| !label.trim().is_empty());
    entries
}

fn scalar_text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        _ => None,
    }
}

/// 0.x's background file: `<0.x app data>/storage/app/overlay/<name>`
/// where the database is `<0.x app data>/database/nativephp.sqlite` (0.x
/// `filesystems.php` disk `overlay`, as `settings_path` finds settings.json).
fn background(db_path: &Path, value: Option<&Value>) -> LegacyBackground {
    let name = match value {
        Some(Value::String(name)) if !name.is_empty() => name,
        _ => return LegacyBackground::NotSet,
    };
    let plain = !name.contains(['/', '\\']) && name != "." && name != "..";
    let app_data = db_path.parent().and_then(Path::parent);
    match app_data {
        Some(app_data) if plain && !app_data.as_os_str().is_empty() => LegacyBackground::File(
            app_data
                .join("storage")
                .join("app")
                .join("overlay")
                .join(name),
        ),
        _ => LegacyBackground::Unusable,
    }
}
