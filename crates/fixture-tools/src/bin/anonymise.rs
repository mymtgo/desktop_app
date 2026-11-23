//! Anonymise a real mtgo.log into fixtures/logs.
//!
//! cargo run -p fixture-tools --bin anonymise -- --input <real.log> --output fixtures/logs/<name>.log --date 2026-08-22 --timezone UTC [--extra "Name"]...
//!
//! Prints counts only, never the real names.

use std::collections::BTreeSet;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use chrono::{Duration, NaiveDate};
use fixture_tools::FixtureMeta;
use fixture_tools::anonymise::{
    anonymise, build_mapping, decoded_entry_count, detect_names, login_name,
};

fn main() -> Result<()> {
    let mut input = None;
    let mut output = None;
    let mut date = None;
    let mut timezone = String::from("UTC");
    let mut extras = BTreeSet::new();

    let mut args = std::env::args().skip(1);
    while let Some(flag) = args.next() {
        let value = args
            .next()
            .with_context(|| format!("{flag} needs a value"))?;
        match flag.as_str() {
            "--input" => input = Some(PathBuf::from(value)),
            "--output" => output = Some(PathBuf::from(value)),
            "--date" => date = Some(value),
            "--timezone" => timezone = value,
            "--extra" => {
                extras.insert(value);
            }
            other => bail!("unknown flag {other}"),
        }
    }
    let input = input.context("--input is required")?;
    let output = output.context("--output is required")?;
    let date = date.context("--date is required")?;

    let bytes = std::fs::read(&input).context("reading input")?;
    let local = login_name(&bytes).context("no MtGO Login Success line found")?;
    let mut names = detect_names(&bytes);
    names.extend(extras);
    let mapping = build_mapping(&names);
    let anonymised = anonymise(&bytes, &mapping);

    for real in mapping.keys() {
        if anonymised
            .windows(real.len())
            .any(|window| window == real.as_bytes())
        {
            bail!("a detected name survived anonymisation; inspect manually");
        }
    }
    let before = decoded_entry_count(&bytes);
    let after = decoded_entry_count(&anonymised);
    if before != after {
        bail!("decodable MetaMessages changed from {before} to {after}; a frame was corrupted");
    }

    std::fs::write(&output, &anonymised).context("writing output")?;

    let anchor =
        NaiveDate::parse_from_str(&date, "%Y-%m-%d").context("--date must be YYYY-MM-DD")?;
    let now = (anchor + Duration::days(2))
        .format("%Y-%m-%dT13:00:00Z")
        .to_string();
    let meta = FixtureMeta {
        date,
        timezone,
        local_username: mapping
            .get(&local)
            .cloned()
            .context("local name not mapped")?,
        now,
    };
    let meta_path = output.with_extension("meta.json");
    std::fs::write(&meta_path, serde_json::to_string_pretty(&meta)? + "\n")?;

    for (index, (real, fake)) in mapping.iter().enumerate() {
        let count = bytes
            .windows(real.len())
            .filter(|w| *w == real.as_bytes())
            .count();
        println!(
            "name {index}: {} bytes -> {fake}, {count} plain-text occurrences",
            real.len()
        );
    }
    println!("decodable MetaMessages: {after}");
    println!("wrote {} and {}", output.display(), meta_path.display());
    Ok(())
}
