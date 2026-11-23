//! Snapshot of what arena-protocol reads from each Arena fixture. A change here
//! means splitting or decoding changed: review it deliberately.

use std::collections::BTreeMap;

use arena_protocol::{ArenaEvent, decode_all, split_complete};
use fixture_tools::arena_fixture;
use serde_json::{Value, json};

#[test]
fn arena_protocol_output() {
    snapshot_fixture("bo3-loss-and-match-concede");
}

#[test]
fn arena_protocol_output_bo1() {
    snapshot_fixture("bo1-wins-and-concede");
}

fn snapshot_fixture(name: &str) {
    let fixture = arena_fixture(name);
    let entries = split_complete(&fixture.bytes);

    let mut kinds: BTreeMap<&str, usize> = BTreeMap::new();
    let mut room_states: Vec<Value> = Vec::new();
    let mut ranks: Vec<Value> = Vec::new();
    for entry in &entries {
        for event in decode_all(entry) {
            *kinds.entry(event.kind()).or_default() += 1;
            let decoded = json!({ "offset": entry.offset, "event": event });
            match event {
                ArenaEvent::RoomState { .. } => room_states.push(decoded),
                ArenaEvent::Rank { .. } => ranks.push(decoded),
                _ => {}
            }
        }
    }

    let summary = json!({
        "entries": entries.len(),
        "with_json": entries.iter().filter(|entry| entry.json.is_some()).count(),
        "kinds": kinds,
        "room_states": room_states,
        "ranks": ranks,
    });
    insta::assert_json_snapshot!(format!("arena_protocol_{name}"), summary);
}
