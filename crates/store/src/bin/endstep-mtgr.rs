//! Dev command: write every Endstep log match as a `.mtgr` file, read from
//! a database opened read-only.
//!
//! `cargo run -p store --bin endstep-mtgr -- <db path> <out dir>`
//!
//! Prints one line per match: its token, then its games, bytes and build
//! milliseconds; or that it wrote no record.

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use rusqlite::{Connection, OpenFlags};
use store::endstep_replay::{endstep_match_text, endstep_tokens};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() != 2 {
        eprintln!("usage: endstep-mtgr <db path> <out dir>");
        return ExitCode::FAILURE;
    }
    match run(&args[0], &PathBuf::from(&args[1])) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("endstep-mtgr: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(db_path: &str, out_dir: &PathBuf) -> Result<(), String> {
    let conn = Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|error| format!("open {db_path}: {error}"))?;
    std::fs::create_dir_all(out_dir).map_err(|error| format!("create out dir: {error}"))?;
    let tokens = endstep_tokens(&conn).map_err(|error| error.to_string())?;
    for token in tokens {
        let started = Instant::now();
        let text = endstep_match_text(&conn, &token).map_err(|error| error.to_string())?;
        let build_ms = started.elapsed().as_millis();
        let Some(text) = text else {
            println!("{token} no record");
            continue;
        };
        let path = out_dir.join(file_name(&token));
        std::fs::write(&path, &text)
            .map_err(|error| format!("write {}: {error}", path.display()))?;
        let games = text
            .lines()
            .filter(|line| line.starts_with("[Game \""))
            .count();
        println!(
            "{token} games {games} bytes {} build {build_ms} ms",
            text.len()
        );
    }
    Ok(())
}

/// The file a match is written to, named after its token. The token comes
/// from a capture (untrusted input), so only ASCII letters, digits and `-`
/// are kept: no separator or `..` can leave the out dir.
fn file_name(token: &str) -> String {
    let stem: String = token
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || *ch == '-')
        .collect();
    format!("{stem}.mtgr")
}

#[cfg(test)]
mod tests {
    use super::file_name;

    #[test]
    fn a_captured_token_names_a_file_in_the_out_dir() {
        assert_eq!(
            file_name("endstep:00000000-0000-4a"),
            "endstep00000000-0000-4a.mtgr"
        );
        assert_eq!(file_name("../../etc/passwd"), "etcpasswd.mtgr");
        assert_eq!(file_name("a/b\\c d"), "abcd.mtgr");
    }
}
