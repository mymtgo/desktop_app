//! `cargo run -p log-player -- --input <mtgo.log> --output <folder> [--speed 1.0]`
//!
//! Replays a log into `<folder>/mtgo.log`, replacing an earlier replay. Prints
//! counts only, never log content.

use anyhow::{Context, anyhow};
use log_player::cli::parse_args;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args = parse_args(&args).map_err(|problem| anyhow!(problem))?;
    let source =
        std::fs::read(&args.input).with_context(|| format!("reading {}", args.input.display()))?;
    let dest = args.output.join("mtgo.log");
    log_player::start_fresh(&dest).with_context(|| format!("creating {}", dest.display()))?;
    let mut writes = 0_u64;
    log_player::play(&source, &dest, args.speed, |_| writes += 1)?;
    println!(
        "wrote {} bytes in {writes} writes to {}",
        source.len(),
        dest.display()
    );
    Ok(())
}
