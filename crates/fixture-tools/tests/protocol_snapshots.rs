//! Snapshot of what mtgo-protocol extracts from each fixture. A change here means
//! classification, decoding, or time reconstruction changed: review it deliberately.

use std::collections::BTreeMap;

use fixture_tools::{fixture_names, load_fixture, prepared_events};
use mtgo_protocol::{EventKind, decode_meta_message, parse_entry, parse_game_management};
use serde_json::{Value, json};

#[test]
fn protocol_output_per_fixture() {
    for name in fixture_names() {
        let fixture = load_fixture(&name);
        let events = prepared_events(&fixture);

        let mut kinds: BTreeMap<String, usize> = BTreeMap::new();
        let mut entry_types: BTreeMap<String, usize> = BTreeMap::new();
        for event in &events {
            let kind = serde_json::to_value(event.kind).expect("kind");
            *kinds
                .entry(kind.as_str().unwrap_or("?").to_string())
                .or_default() += 1;
            if event.kind == EventKind::GameManagementJson
                && let Some(meta) = parse_game_management(&event.text).meta_message
                && let Some(message) = decode_meta_message(&meta)
            {
                let entry = serde_json::to_value(parse_entry(&message)).expect("entry");
                let entry_type = entry["type"].as_str().unwrap_or("?").to_string();
                *entry_types.entry(entry_type).or_default() += 1;
            }
        }

        let summary: Value = json!({
            "events": events.len(),
            "kinds": kinds,
            "entry_types": entry_types,
            "first_at_ms": events.first().map(|e| e.at_ms),
            "last_at_ms": events.last().map(|e| e.at_ms),
        });
        insta::assert_json_snapshot!(format!("protocol_{name}"), summary);
    }
}
