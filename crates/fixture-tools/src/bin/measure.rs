//! Storage report: raw size of whitelisted events, per-row zstd, and per-match zstd.
//!
//! cargo run -p fixture-tools --bin measure --release

use std::collections::{BTreeMap, BTreeSet};

use fixture_tools::{fixture_names, load_fixture, prepared_events};

fn zstd_len(bytes: &[u8]) -> usize {
    zstd::encode_all(bytes, 3)
        .map(|compressed| compressed.len())
        .unwrap_or(bytes.len())
}

fn main() {
    println!(
        "fixture | events | raw KB | per-row zstd KB | per-match zstd KB | games | raw KB/game | per-match zstd KB/game"
    );
    for name in fixture_names() {
        let fixture = load_fixture(&name);
        let events = prepared_events(&fixture);

        let raw: usize = events.iter().map(|e| e.text.len()).sum();
        let per_row: usize = events.iter().map(|e| zstd_len(e.text.as_bytes())).sum();

        let mut by_match: BTreeMap<u64, Vec<u8>> = BTreeMap::new();
        let mut games: BTreeSet<u64> = BTreeSet::new();
        for event in &events {
            if let Some(match_id) = event.subjects.match_id {
                by_match
                    .entry(match_id)
                    .or_default()
                    .extend_from_slice(event.text.as_bytes());
            }
            if let Some(game_id) = event.subjects.game_id {
                games.insert(game_id);
            }
        }
        let match_raw: usize = by_match.values().map(Vec::len).sum();
        let per_match: usize = by_match
            .values()
            .map(|bytes| zstd_len(bytes.as_slice()))
            .sum();
        let game_count = games.len().max(1);

        println!(
            "{name} | {} | {} | {} | {} | {} | {} | {}",
            events.len(),
            raw / 1024,
            per_row / 1024,
            per_match / 1024,
            games.len(),
            match_raw / game_count / 1024,
            per_match / game_count / 1024,
        );
    }
}
