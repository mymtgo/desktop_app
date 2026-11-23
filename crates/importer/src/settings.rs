//! The 0.x `settings.json` keys worth keeping (importer spec section 6): the
//! preferences and UI state later sub-projects will read once. Secrets, sync
//! keys, internal state and OS duplicates are dropped, as is any key not on
//! the keep list.

use serde_json::{Map, Value};

use crate::ImportError;

/// Preference and UI state keys kept as they are.
const KEEP: [&str; 17] = [
    "log_path",
    "log_data_path",
    "card_images_path",
    "local_images",
    "card_stats_trust",
    "offline_mode",
    "debug_mode",
    "dashboard_layout",
    "deck_card_size",
    "decks_per_page",
    "hide_archived_decks",
    "league_window",
    "opponent_window",
    "deck_window",
    "draft_notes_window",
    "game_overlay",
    "overlay_background_path",
];

/// Overlay keys are all preferences or UI state, except 0.x's internal
/// publish records (overlay spec section 12): the uploaded background, the
/// last publish time and the publish error. v1 writes its own; the first
/// publish uploads the background again.
const OVERLAY_PREFIX: &str = "overlay_";
const OVERLAY_INTERNAL: [&str; 3] = [
    "overlay_publish_error",
    "overlay_background_remote",
    "overlay_last_published_at",
];

fn is_kept(key: &str) -> bool {
    if KEEP.contains(&key) {
        return true;
    }
    key.starts_with(OVERLAY_PREFIX) && !OVERLAY_INTERNAL.contains(&key)
}

/// The preference and UI keys of a 0.x settings.json, as one JSON object
/// (spec section 6). Unknown keys are dropped unless they are on the keep list.
pub fn filter_settings(json: &str) -> Result<String, ImportError> {
    let parsed: Value = serde_json::from_str(json)
        .map_err(|error| ImportError::Read(format!("settings.json is not valid JSON: {error}")))?;
    let Value::Object(all) = parsed else {
        return Err(ImportError::Read(
            "settings.json is not a JSON object".to_string(),
        ));
    };
    let mut kept = Map::new();
    for (key, value) in all {
        if is_kept(&key) {
            kept.insert(key, value);
        }
    }
    serde_json::to_string(&Value::Object(kept))
        .map_err(|error| ImportError::Read(format!("settings could not be written: {error}")))
}

/// Whether 0.x had launch at login on: its `autostart_enabled`, read as 0.x
/// reads it (`(bool) $this->get('autostart_enabled', false)`). The key is not
/// kept (v1 asks the OS), so the import reads it here, once (release spec
/// section 13 item 4). A file that does not parse counts as off.
pub fn launch_at_login(json: &str) -> bool {
    let Ok(Value::Object(all)) = serde_json::from_str::<Value>(json) else {
        return false;
    };
    match all.get("autostart_enabled") {
        Some(Value::Bool(on)) => *on,
        Some(Value::Number(number)) => number.as_f64().is_some_and(|n| n != 0.0),
        Some(Value::String(text)) => !text.is_empty() && text != "0",
        Some(Value::Array(items)) => !items.is_empty(),
        Some(Value::Object(_)) => true,
        Some(Value::Null) | None => false,
    }
}
