//! The native messaging helper. The browser starts it and talks over stdin and
//! stdout, so nothing else may ever be printed to stdout. Arguments (Chrome
//! passes the origin, and `--parent-window` on Windows) are ignored.

use std::process::ExitCode;

/// The wall clock as UTC unix milliseconds.
fn now_ms() -> i64 {
    match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        Ok(elapsed) => i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX),
        Err(_) => 0,
    }
}

fn main() -> ExitCode {
    let exe = match std::env::current_exe() {
        Ok(path) => path,
        Err(error) => {
            eprintln!("mymtgo-endstep-host: cannot find own path: {error}");
            return ExitCode::from(1);
        }
    };
    let inbox = endstep_host::inbox_beside(&exe);
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut input = stdin.lock();
    let mut output = stdout.lock();
    match endstep_host::serve(&mut input, &mut output, &inbox, &now_ms) {
        Ok(()) => ExitCode::from(0),
        Err(error) => {
            eprintln!("mymtgo-endstep-host: {error}");
            ExitCode::from(1)
        }
    }
}
