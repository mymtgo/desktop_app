//! The data console (spec `2026-11-20-data-console-design.md`): a window
//! where the user runs read-only SQL over their own database, charts it and
//! exports it. The store side is `store::console`; this module holds the
//! commands, the wire types, the saved queries file and the window.

pub mod commands;
pub mod saved;
pub mod types;
pub mod window;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::bridge::lock;

/// The console's managed state. No connection is kept: each query opens
/// its own (`store::console::open_console` is cheap).
#[derive(Debug, Default)]
pub struct ConsoleState {
    /// The cancel flags of every run still going (the user's run and any
    /// picker loads run side by side). Each run removes its own when it
    /// ends (`RunFlag`).
    runs: Arc<Mutex<Vec<Arc<AtomicBool>>>>,
    /// The cancel flag of the latest export.
    export: Mutex<Option<Arc<AtomicBool>>>,
    /// Held while the saved queries file is read (and written back), so two
    /// saves never lose one another's change.
    pub saved: Arc<Mutex<()>>,
}

/// A live run's cancel flag. While it is held, `ConsoleState::cancel`
/// reaches it; dropping it (the run finished, failed or panicked) takes it
/// off the live list.
#[derive(Debug)]
pub struct RunFlag {
    flag: Arc<AtomicBool>,
    runs: Arc<Mutex<Vec<Arc<AtomicBool>>>>,
}

impl RunFlag {
    /// The flag the query checks.
    pub fn flag(&self) -> &Arc<AtomicBool> {
        &self.flag
    }
}

impl Drop for RunFlag {
    fn drop(&mut self) {
        lock(&self.runs).retain(|live| !Arc::ptr_eq(live, &self.flag));
    }
}

impl ConsoleState {
    /// A fresh cancel flag for a run that is starting, on the live list
    /// until the returned `RunFlag` is dropped. Each run has its own flag,
    /// so a cancel never reaches one started after it.
    pub fn start_run(&self) -> RunFlag {
        let flag = Arc::new(AtomicBool::new(false));
        lock(&self.runs).push(Arc::clone(&flag));
        RunFlag {
            flag,
            runs: Arc::clone(&self.runs),
        }
    }

    /// A fresh cancel flag for an export that is starting.
    pub fn start_export(&self) -> Arc<AtomicBool> {
        let flag = Arc::new(AtomicBool::new(false));
        *lock(&self.export) = Some(Arc::clone(&flag));
        flag
    }

    /// Stops every live run and the latest export (the page cancels
    /// whatever is running; a finished export ignores it).
    pub fn cancel(&self) {
        for flag in lock(&self.runs).iter() {
            flag.store(true, Ordering::SeqCst);
        }
        if let Some(flag) = lock(&self.export).as_ref() {
            flag.store(true, Ordering::SeqCst);
        }
    }

    /// How many runs are live.
    #[cfg(test)]
    fn live_runs(&self) -> usize {
        lock(&self.runs).len()
    }
}

/// Run `job` on a blocking worker thread, never on the IPC thread: queries,
/// exports, file writes and window builds all go through here.
pub(crate) async fn off_ipc<T: Send + 'static>(
    job: impl FnOnce() -> T + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(job)
        .await
        .map_err(|error| format!("The data console stopped unexpectedly: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancel_stops_every_live_run_and_the_export() {
        let console = ConsoleState::default();
        let user_run = console.start_run();
        let picker_load = console.start_run();
        let export = console.start_export();
        console.cancel();
        assert!(user_run.flag().load(Ordering::SeqCst));
        assert!(picker_load.flag().load(Ordering::SeqCst));
        assert!(export.load(Ordering::SeqCst));
    }

    #[test]
    fn a_finished_run_leaves_the_live_list() {
        let console = ConsoleState::default();
        let first = console.start_run();
        let second = console.start_run();
        assert_eq!(console.live_runs(), 2);
        drop(first);
        assert_eq!(console.live_runs(), 1);
        console.cancel();
        assert!(
            second.flag().load(Ordering::SeqCst),
            "the other run is still reached"
        );
        drop(second);
        assert_eq!(console.live_runs(), 0);
    }

    #[test]
    fn a_run_started_after_a_cancel_is_not_stopped() {
        let console = ConsoleState::default();
        let first = console.start_run();
        console.cancel();
        let second = console.start_run();
        assert!(first.flag().load(Ordering::SeqCst));
        assert!(!second.flag().load(Ordering::SeqCst));
        console.cancel();
        assert!(second.flag().load(Ordering::SeqCst));
    }

    #[test]
    fn cancel_with_nothing_running_does_nothing() {
        let console = ConsoleState::default();
        console.cancel();
        assert!(!console.start_export().load(Ordering::SeqCst));
        assert!(!console.start_run().flag().load(Ordering::SeqCst));
    }
}

#[cfg(test)]
pub(crate) mod testing {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    static NEXT: AtomicU32 = AtomicU32::new(0);

    /// A fresh folder under the temp folder, removed when dropped.
    pub struct Scratch(pub PathBuf);

    impl Scratch {
        pub fn new() -> Self {
            let dir = std::env::temp_dir().join(format!(
                "mymtgo-console-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::SeqCst)
            ));
            std::fs::remove_dir_all(&dir).ok();
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).ok();
        }
    }
}
