//! `timing`: p50/p95 of the timing lines in MyMTGO's log files (spec 8).
//!   cargo run --release -p fixture-tools --bin timing -- mymtgo.2026-10-07.log ...

use std::process::ExitCode;

use fixture_tools::timing::{summarise, table};

fn main() -> ExitCode {
    let paths: Vec<String> = std::env::args().skip(1).collect();
    if paths.is_empty() {
        eprintln!("usage: timing <log file>...");
        return ExitCode::FAILURE;
    }
    let mut text = String::new();
    for path in &paths {
        match std::fs::read(path) {
            Ok(bytes) => {
                text.push_str(&String::from_utf8_lossy(&bytes));
                text.push('\n');
            }
            Err(error) => {
                eprintln!("could not read {path}: {error}");
                return ExitCode::FAILURE;
            }
        }
    }
    print!("{}", table(&summarise(&text)));
    ExitCode::SUCCESS
}
