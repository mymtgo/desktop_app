//! One background worker thread with its signal channel: start it, send it
//! signals, and stop it within a wait. The API, cards and archetypes
//! workers each hold one, with their own signal type `S`.

use std::sync::Mutex;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::bridge::lock;

/// How long stopping waits for the thread before leaving it. A request in
/// flight (or an archetypes pause) can outlast it: a claim the API worker
/// leaves is reaped next run, and a cards or archetypes run cut short is
/// asked again by the next start.
pub const STOP_WAIT: Duration = Duration::from_secs(2);

/// A running worker thread and the sender that wakes it.
pub struct WorkerThread<S> {
    signals: Mutex<Option<Sender<S>>>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl<S: Send + 'static> WorkerThread<S> {
    /// Start a thread called `name` running `body` with the signal inbox.
    /// When the thread cannot start, `not_started` is logged and signals
    /// still queue (nothing reads them).
    pub fn spawn(
        name: &str,
        not_started: &str,
        body: impl FnOnce(Receiver<S>) + Send + 'static,
    ) -> WorkerThread<S> {
        let (signals, inbox) = mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name(name.to_string())
            .spawn(move || body(inbox));
        let thread = match spawned {
            Ok(thread) => Some(thread),
            Err(error) => {
                tracing::error!(%error, "{not_started}");
                None
            }
        };
        WorkerThread {
            signals: Mutex::new(Some(signals)),
            thread: Mutex::new(thread),
        }
    }

    /// Wake the worker with a signal. Returns false once stopped.
    pub fn send(&self, signal: S) -> bool {
        match lock(&self.signals).as_ref() {
            Some(signals) => signals.send(signal).is_ok(),
            None => false,
        }
    }

    /// Send `stop` and wait up to `STOP_WAIT` for the thread. A thread
    /// still busy then is left running and `still_running` is logged.
    pub fn stop(&self, stop: S, still_running: &str) {
        self.stop_within(stop, STOP_WAIT, still_running);
    }

    pub fn stop_within(&self, stop: S, wait: Duration, still_running: &str) {
        if let Some(signals) = lock(&self.signals).take() {
            let _ = signals.send(stop);
        }
        let Some(thread) = lock(&self.thread).take() else {
            return;
        };
        let deadline = Instant::now() + wait;
        while !thread.is_finished() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        if thread.is_finished() {
            let _ = thread.join();
        } else {
            tracing::info!("{still_running}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, PartialEq)]
    enum Signal {
        Ping,
        Stop,
    }

    #[test]
    fn signals_reach_the_thread_and_stop_ends_it() {
        let (seen, seen_inbox) = mpsc::channel();
        let worker = WorkerThread::spawn("test-worker", "not started", move |inbox| {
            while let Ok(signal) = inbox.recv() {
                let stop = signal == Signal::Stop;
                let _ = seen.send(signal);
                if stop {
                    return;
                }
            }
        });
        assert!(worker.send(Signal::Ping));
        worker.stop(Signal::Stop, "still running");
        assert!(lock(&worker.thread).is_none());
        assert!(!worker.send(Signal::Ping));
        let seen: Vec<Signal> = seen_inbox.try_iter().collect();
        assert_eq!(seen, vec![Signal::Ping, Signal::Stop]);
    }

    #[test]
    fn stop_leaves_a_busy_thread_after_the_wait() {
        let (release, released) = mpsc::channel::<()>();
        let worker: WorkerThread<Signal> =
            WorkerThread::spawn("test-worker", "not started", move |_inbox| {
                let _ = released.recv();
            });
        let started = Instant::now();
        worker.stop_within(Signal::Stop, Duration::from_millis(50), "still running");
        assert!(started.elapsed() < Duration::from_secs(1));
        assert!(!worker.send(Signal::Ping));
        let _ = release.send(());
    }
}
