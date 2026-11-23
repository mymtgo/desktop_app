//! Rotating local log files (spec 9) with built-in timing (spec 8). The
//! `timing` tool in fixture-tools summarises the `mymtgo::timing` lines.
//! Errors also go to Sentry (`reporting`).

use std::path::Path;
use std::sync::Mutex;
use std::time::Instant;

use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::EnvFilter;

/// Log files live in this folder under the app data folder.
pub const LOG_FOLDER: &str = "logs";
const DEFAULT_FILTER: &str = "info,mymtgo::timing=debug";

/// Keeps the background log writer alive; taken on exit so nothing is lost.
pub struct LogGuard(pub Mutex<Option<WorkerGuard>>);

/// Send log lines to daily files in `log_dir`, keeping a week of them. The
/// `MYMTGO_LOG` environment variable, if set, picks what is logged.
pub fn init(log_dir: &Path) -> Result<WorkerGuard, String> {
    let filter = std::env::var("MYMTGO_LOG").unwrap_or_else(|_| DEFAULT_FILTER.to_string());
    init_with_filter(log_dir, &filter)
}

/// As `init`, with the filter given (for example `info,mymtgo::timing=debug`).
/// A filter that does not parse falls back to the default. Tests use this
/// instead of changing the environment.
pub fn init_with_filter(log_dir: &Path, filter: &str) -> Result<WorkerGuard, String> {
    std::fs::create_dir_all(log_dir)
        .map_err(|error| format!("could not create the log folder: {error}"))?;
    let appender = RollingFileAppender::builder()
        .rotation(Rotation::DAILY)
        .filename_prefix("mymtgo")
        .filename_suffix("log")
        .max_log_files(7)
        .build(log_dir)
        .map_err(|error| format!("could not open the log file: {error}"))?;
    make_logs_private(log_dir);
    let (writer, guard) = tracing_appender::non_blocking(appender);
    let filter = EnvFilter::try_new(filter).unwrap_or_else(|_| EnvFilter::new(DEFAULT_FILTER));
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;
    tracing_subscriber::registry()
        .with(filter)
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(writer)
                .with_ansi(false),
        )
        // Does nothing until `reporting::init` has started a client.
        .with(sentry::integrations::tracing::layer().event_filter(crate::reporting::event_filter))
        .try_init()
        .map_err(|error| error.to_string())?;
    Ok(guard)
}

/// Make the app data folder (the log folder's parent) and the log folder
/// `0700`, and the log files in it `0600`, on unix. Logging starts before
/// the database opens, so this is the first place the data folder exists.
/// Files the appender creates later (a new day's file) take the default
/// mode, but sit in a folder only the user can enter.
fn make_logs_private(log_dir: &Path) {
    if let Some(data_dir) = log_dir.parent()
        && !data_dir.as_os_str().is_empty()
    {
        store::make_private(data_dir, store::PRIVATE_FOLDER_MODE);
    }
    store::make_private(log_dir, store::PRIVATE_FOLDER_MODE);
    let Ok(entries) = std::fs::read_dir(log_dir) else {
        return;
    };
    for entry in entries.flatten() {
        if entry.file_type().is_ok_and(|kind| kind.is_file()) {
            store::make_private(&entry.path(), store::PRIVATE_FILE_MODE);
        }
    }
}

/// Log every panic, then let the default hook print it as before. The
/// tracker's panics are caught and restarted; this keeps a record of them.
pub fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        tracing::error!(target: crate::reporting::PANIC_TARGET, panic = %info, "panic");
        previous(info);
    }));
}

/// Times a command from `start` until it is dropped (spec 8).
pub struct Timer {
    name: &'static str,
    started: Instant,
}

impl Timer {
    pub fn start(name: &'static str) -> Self {
        Self {
            name,
            started: Instant::now(),
        }
    }
}

impl Drop for Timer {
    fn drop(&mut self) {
        tracing::debug!(
            target: "mymtgo::timing",
            command = self.name,
            elapsed_us = self.started.elapsed().as_micros() as u64,
            "command"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fixture_tools::TestDir;

    #[cfg(unix)]
    fn mode_of(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path)
            .expect("metadata")
            .permissions()
            .mode()
            & 0o777
    }

    #[cfg(unix)]
    #[test]
    fn the_log_folder_and_its_files_are_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TestDir::new("logging-private");
        dir.write("data/logs/mymtgo.2026-10-13.log", b"x");
        dir.write("data/logs/mymtgo.2026-10-14.log", b"x");
        for folder in ["data", "data/logs"] {
            std::fs::set_permissions(dir.join(folder), std::fs::Permissions::from_mode(0o755))
                .expect("chmod");
        }
        make_logs_private(&dir.join("data/logs"));
        assert_eq!(mode_of(&dir.join("data")), 0o700);
        assert_eq!(mode_of(&dir.join("data/logs")), 0o700);
        for name in ["mymtgo.2026-10-13.log", "mymtgo.2026-10-14.log"] {
            assert_eq!(mode_of(&dir.join("data/logs").join(name)), 0o600, "{name}");
        }
    }
}
