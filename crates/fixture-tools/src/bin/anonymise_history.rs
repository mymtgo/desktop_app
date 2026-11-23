//! Anonymise a real MTGO `mtgo_game_history` file into fixtures/history.
//!
//! cargo run -p fixture-tools --bin anonymise_history -- --input <real file> --output fixtures/history/mtgo_game_history
//!
//! Every string except `GameStructure` values becomes a same-byte-length
//! fake (see `fixture_tools::history`). Checks that the result reads to the
//! same records with only the text changed and that no player name remains
//! in any case. Prints counts only, never the real text.

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use fixture_tools::history::{anonymise_history, history_mapping, player_names};
use mtgo_history::binary::read_stream;
use mtgo_history::parse_history;

fn main() -> Result<()> {
    let mut input = None;
    let mut output = None;

    let mut args = std::env::args().skip(1);
    while let Some(flag) = args.next() {
        let value = args
            .next()
            .with_context(|| format!("{flag} needs a value"))?;
        match flag.as_str() {
            "--input" => input = Some(PathBuf::from(value)),
            "--output" => output = Some(PathBuf::from(value)),
            other => bail!("unknown flag {other}"),
        }
    }
    let input = input.context("--input is required")?;
    let output = output.context("--output is required")?;

    let bytes = std::fs::read(&input).context("reading input")?;
    let stream = read_stream(&bytes).context("not a game history file")?;
    let names = player_names(&stream);
    let mapping = history_mapping(&bytes)?;
    let anonymised = anonymise_history(&bytes, &mapping)?;

    if anonymised.len() != bytes.len() {
        bail!("the file length changed");
    }
    for name in &names {
        if !mapping.contains_key(name) {
            bail!("a player name has no fake");
        }
    }

    // The same records, only the text changed.
    let tz = chrono_tz::UTC;
    let before = parse_history(&bytes, tz).context("the input does not parse")?;
    let after = parse_history(&anonymised, tz).context("the output does not parse")?;
    if before.len() != after.len() {
        bail!("records changed from {} to {}", before.len(), after.len());
    }
    let fake = |text: &String| mapping.get(text).cloned().unwrap_or(text.clone());
    for (old, new) in before.iter().zip(&after) {
        let mut expected = old.clone();
        expected.opponents = old.opponents.iter().map(fake).collect();
        expected.match_winners = old.match_winners.iter().map(fake).collect();
        expected.match_losers = old.match_losers.iter().map(fake).collect();
        expected.description = fake(&old.description);
        if *new != expected {
            bail!("record {} changed beyond its text", old.id);
        }
    }

    // No replaced string is left as a string, and no player name is left
    // anywhere in the file, in any case.
    let out_stream = read_stream(&anonymised)?;
    let left = out_stream
        .strings
        .iter()
        .filter(|span| {
            let text = String::from_utf8_lossy(&anonymised[span.start..span.end]);
            mapping.contains_key(text.as_ref())
        })
        .count();
    if left > 0 {
        bail!("{left} replaced strings survived");
    }
    let lower = anonymised.to_ascii_lowercase();
    let mut hits = 0;
    for name in &names {
        let needle = name.to_lowercase().into_bytes();
        hits += lower.windows(needle.len()).filter(|w| *w == needle).count();
    }
    if hits > 0 {
        bail!("{hits} player name matches remain (case-insensitive); inspect manually");
    }

    std::fs::write(&output, &anonymised).context("writing output")?;

    println!("records: {}", after.len());
    println!("player names replaced: {}", names.len());
    println!("strings replaced: {} distinct", mapping.len());
    println!(
        "string payloads: {}, kept: {}",
        stream.strings.len(),
        stream.strings.len()
            - stream
                .strings
                .iter()
                .filter(|span| {
                    let text = String::from_utf8_lossy(&bytes[span.start..span.end]);
                    mapping.contains_key(text.as_ref())
                })
                .count()
    );
    println!("wrote {}", output.display());
    Ok(())
}
