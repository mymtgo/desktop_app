//! Anonymise a real MTGO game log (`*Match_GameLog*.dat`) into fixtures/gamelogs.
//!
//! cargo run -p fixture-tools --bin anonymise_dat -- --input <real.dat> --output fixtures/gamelogs/<name>.dat [--extra "Name"]...
//!
//! Player names (from result phrases, plus any `--extra`) become
//! same-byte-length fakes starting with Q, in every record. Prints counts
//! only, never the real names.

use std::collections::BTreeSet;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use fixture_tools::anonymise::build_mapping;
use fixture_tools::dat::{
    anonymise_dat, contains_name, dat_names, dat_short_names, read_dat, write_dat,
};

fn main() -> Result<()> {
    let mut input = None;
    let mut output = None;
    let mut extras = BTreeSet::new();

    let mut args = std::env::args().skip(1);
    while let Some(flag) = args.next() {
        let value = args
            .next()
            .with_context(|| format!("{flag} needs a value"))?;
        match flag.as_str() {
            "--input" => input = Some(PathBuf::from(value)),
            "--output" => output = Some(PathBuf::from(value)),
            "--extra" => {
                extras.insert(value);
            }
            other => bail!("unknown flag {other}"),
        }
    }
    let input = input.context("--input is required")?;
    let output = output.context("--output is required")?;

    let bytes = std::fs::read(&input).context("reading input")?;
    let file = read_dat(&bytes).context("not a game log: no complete header")?;
    if write_dat(&file) != bytes {
        bail!("the file does not round-trip; inspect manually");
    }
    if !dat_short_names(&file).is_empty() {
        bail!("a player name is under 3 bytes and cannot be replaced safely; handle it by hand");
    }
    let mut names = dat_names(&file);
    names.extend(extras);
    let mapping = build_mapping(&names);
    let anonymised = write_dat(&anonymise_dat(&file, &mapping));

    for real in mapping.keys() {
        if contains_name(&anonymised, real) {
            bail!("a detected name survived anonymisation; inspect manually");
        }
    }
    if anonymised.len() != bytes.len() {
        bail!("the file length changed; a name was replaced by a fake of another length");
    }
    let after = read_dat(&anonymised).context("the anonymised file does not read")?;
    if after.records.len() != file.records.len() {
        bail!(
            "records changed from {} to {}; a record was corrupted",
            file.records.len(),
            after.records.len()
        );
    }

    std::fs::write(&output, &anonymised).context("writing output")?;

    for (index, (real, fake)) in mapping.iter().enumerate() {
        let count = bytes
            .windows(real.len())
            .filter(|w| *w == real.as_bytes())
            .count();
        println!(
            "name {index}: {} bytes -> {fake}, {count} occurrences",
            real.len()
        );
    }
    println!("records: {}", after.records.len());
    println!("wrote {}", output.display());
    Ok(())
}
