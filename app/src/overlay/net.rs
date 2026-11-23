//! The `overlay-net` thread (overlay spec 6 step 4): the overlay's network
//! calls (scouting, the API estimate, publish, the background upload, clear
//! and delete) run here, one at a time, so a slow request never delays a
//! frame on the `overlay` thread. Answers go back to the `overlay` thread as
//! messages.
//!
//! Jobs with the same key coalesce: one in flight and at most one waiting,
//! the newest (0.x's unique publish job). A job reads what it sends when it
//! starts, so the waiting one sends the state current then.

use std::collections::VecDeque;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;

use crate::overlay::worker::OverlayMsg;

/// One request. `run` performs it and returns the answer for the `overlay`
/// thread, if any.
pub struct NetJob {
    /// Jobs with the same key replace each other while waiting.
    pub key: String,
    pub run: Box<dyn FnOnce() -> Option<OverlayMsg> + Send>,
}

impl std::fmt::Debug for NetJob {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("NetJob")
            .field("key", &self.key)
            .finish()
    }
}

/// What wakes the `overlay-net` thread.
#[derive(Debug)]
pub enum NetSignal {
    Job(NetJob),
    Stop,
}

/// Add a job to the waiting ones: a newer job with a waiting job's key
/// replaces it at the end, so it runs after everything asked before it (a
/// Clear between two Publishes stays between them).
pub fn enqueue(waiting: &mut VecDeque<NetJob>, job: NetJob) {
    if let Some(at) = waiting.iter().position(|queued| queued.key == job.key) {
        waiting.remove(at);
    }
    waiting.push_back(job);
}

/// The thread's loop: run waiting jobs in order, one at a time, sending each
/// answer to `answers`. Ends on `Stop` or when every sender is gone.
pub fn run_net_queue(inbox: &Receiver<NetSignal>, answers: &Sender<OverlayMsg>) {
    let mut waiting: VecDeque<NetJob> = VecDeque::new();
    loop {
        if waiting.is_empty() {
            match inbox.recv() {
                Ok(NetSignal::Job(job)) => enqueue(&mut waiting, job),
                Ok(NetSignal::Stop) | Err(_) => return,
            }
        }
        // Jobs that arrived during the last request join (and coalesce) now.
        while let Ok(signal) = inbox.try_recv() {
            match signal {
                NetSignal::Job(job) => enqueue(&mut waiting, job),
                NetSignal::Stop => return,
            }
        }
        if let Some(job) = waiting.pop_front()
            && let Some(answer) = (job.run)()
        {
            let _ = answers.send(answer);
        }
    }
}

/// The running `overlay-net` thread.
pub struct NetQueue {
    sender: Sender<NetSignal>,
    thread: JoinHandle<()>,
}

impl NetQueue {
    /// Start the thread; `None` when it could not start (logged).
    pub fn start(answers: Sender<OverlayMsg>) -> Option<NetQueue> {
        let (sender, inbox) = mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("overlay-net".to_string())
            .spawn(move || run_net_queue(&inbox, &answers));
        match spawned {
            Ok(thread) => Some(NetQueue { sender, thread }),
            Err(error) => {
                tracing::error!(%error, "could not start the overlay network thread");
                None
            }
        }
    }

    /// A sender for jobs (the publish side's).
    pub fn jobs(&self) -> Sender<NetSignal> {
        self.sender.clone()
    }

    /// Queue a job. Returns false once stopped.
    pub fn submit(&self, job: NetJob) -> bool {
        self.sender.send(NetSignal::Job(job)).is_ok()
    }

    /// Stop the thread without waiting on a request in flight.
    pub fn stop(self) {
        let _ = self.sender.send(NetSignal::Stop);
        if self.thread.is_finished() {
            let _ = self.thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use super::*;

    fn job(key: &str, ran: &Arc<Mutex<Vec<String>>>, name: &str) -> NetJob {
        let ran = Arc::clone(ran);
        let name = name.to_string();
        NetJob {
            key: key.to_string(),
            run: Box::new(move || {
                ran.lock().expect("ran").push(name);
                None
            }),
        }
    }

    #[test]
    fn a_waiting_job_is_replaced_by_a_newer_one_at_the_end() {
        let ran = Arc::new(Mutex::new(Vec::new()));
        let mut waiting = VecDeque::new();
        enqueue(&mut waiting, job("publish", &ran, "publish 1"));
        enqueue(&mut waiting, job("scout", &ran, "scout"));
        enqueue(&mut waiting, job("publish", &ran, "publish 2"));
        let keys: Vec<&str> = waiting.iter().map(|job| job.key.as_str()).collect();
        assert_eq!(keys, vec!["scout", "publish"], "at the end");
        while let Some(next) = waiting.pop_front() {
            (next.run)();
        }
        assert_eq!(*ran.lock().expect("ran"), vec!["scout", "publish 2"]);
    }

    #[test]
    fn a_clear_between_two_publishes_stays_between_them() {
        let ran = Arc::new(Mutex::new(Vec::new()));
        let mut waiting = VecDeque::new();
        enqueue(&mut waiting, job("publish", &ran, "publish 1"));
        enqueue(&mut waiting, job("clear", &ran, "clear"));
        enqueue(&mut waiting, job("publish", &ran, "publish 2"));
        while let Some(next) = waiting.pop_front() {
            (next.run)();
        }
        assert_eq!(*ran.lock().expect("ran"), vec!["clear", "publish 2"]);
    }

    #[test]
    fn jobs_run_one_at_a_time_and_a_burst_sends_the_newest_once() {
        let (signals, inbox) = mpsc::channel();
        let (answers, answered) = mpsc::channel();
        let thread = std::thread::spawn(move || run_net_queue(&inbox, &answers));
        let ran = Arc::new(Mutex::new(Vec::new()));
        // The first publish holds the thread until released.
        let (release, hold) = mpsc::channel::<()>();
        let first_ran = Arc::clone(&ran);
        signals
            .send(NetSignal::Job(NetJob {
                key: "publish".to_string(),
                run: Box::new(move || {
                    first_ran.lock().expect("ran").push("publish 1".to_string());
                    let _ = hold.recv();
                    Some(OverlayMsg::LeaguesChanged)
                }),
            }))
            .expect("send");
        let started = std::time::Instant::now();
        while ran.lock().expect("ran").is_empty() {
            assert!(
                started.elapsed() < Duration::from_secs(5),
                "the first job runs"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        for name in ["publish 2", "publish 3", "publish 4"] {
            signals
                .send(NetSignal::Job(job("publish", &ran, name)))
                .expect("send");
        }
        release.send(()).expect("release");
        assert_eq!(
            answered.recv_timeout(Duration::from_secs(5)),
            Ok(OverlayMsg::LeaguesChanged),
            "the answer goes back to the overlay thread"
        );
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while ran.lock().expect("ran").len() < 2 && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(1));
        }
        signals.send(NetSignal::Stop).expect("stop");
        thread.join().expect("join");
        assert_eq!(*ran.lock().expect("ran"), vec!["publish 1", "publish 4"]);
    }
}
