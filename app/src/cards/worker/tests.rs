use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use serde_json::json;

use super::*;
use crate::api::session::testing::device_api;
use crate::api::stub::{Reply, Stub};
use crate::cards::folder::MoveRequest;
use crate::cards::populate::tests::tracked;

/// Quick timings: nothing runs by itself within a test unless asked. Wide
/// enough for a slow Windows runner (S2-4): every wait for something that
/// must happen polls (`wait_until`), and the fixed sleeps only check that
/// nothing more happened.
fn quiet() -> Timing {
    Timing {
        every: Duration::from_secs(600),
        debounce: Duration::from_millis(400),
        first_run: Duration::from_secs(600),
        retry: Duration::from_millis(100),
    }
}

/// A worker thread whose runs are recorded (`scan_first` of each, and the
/// whole round) and take `run_for`.
struct Running {
    signals: Sender<CardsSignal>,
    /// Runs started so far.
    started: Arc<AtomicUsize>,
    runs: Arc<Mutex<Vec<bool>>>,
    rounds: Arc<Mutex<Vec<Round>>>,
    thread: Option<JoinHandle<()>>,
}

impl Running {
    fn start(timing: Timing, run_for: Duration) -> Running {
        let (signals, inbox) = mpsc::channel();
        let runs = Arc::new(Mutex::new(Vec::new()));
        let rounds = Arc::new(Mutex::new(Vec::new()));
        let started = Arc::new(AtomicUsize::new(0));
        let recorded = Arc::clone(&runs);
        let recorded_rounds = Arc::clone(&rounds);
        let counted = Arc::clone(&started);
        let thread = std::thread::spawn(move || {
            let mut run = |round: &Round, _: &dyn Fn() -> bool| {
                counted.fetch_add(1, Ordering::SeqCst);
                std::thread::sleep(run_for);
                lock(&recorded).push(round.scan_first);
                lock(&recorded_rounds).push(round.clone());
                Ran::done(Ok(()))
            };
            run_cards_worker(&inbox, timing, &mut run);
        });
        Running {
            signals,
            started,
            runs,
            rounds,
            thread: Some(thread),
        }
    }

    fn send(&self, signal: CardsSignal) {
        self.signals.send(signal).expect("send");
    }

    fn runs(&self) -> Vec<bool> {
        lock(&self.runs).clone()
    }

    fn rounds(&self) -> Vec<Round> {
        lock(&self.rounds).clone()
    }

    fn stop(mut self) -> Vec<bool> {
        self.send(CardsSignal::Stop);
        if let Some(thread) = self.thread.take() {
            thread.join().expect("join");
        }
        self.runs()
    }
}

fn wait_until(done: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !done() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn a_populate_with_a_reply_runs_at_once_and_is_answered() {
    let worker = Running::start(quiet(), Duration::ZERO);
    let (reply, answer) = mpsc::channel();
    worker.send(CardsSignal::Populate {
        reply: Some(reply),
        scanned: 0,
    });
    assert_eq!(
        answer
            .recv_timeout(Duration::from_secs(5))
            .expect("answered"),
        Ok(())
    );
    assert_eq!(worker.stop(), vec![false]);
}

// Review of #501: a button's run asks about the ids the API did not know;
// a run nobody waits for does not.
#[test]
fn only_a_button_run_asks_about_unknown_ids() {
    let worker = Running::start(quiet(), Duration::ZERO);
    worker.send(CardsSignal::Populate {
        reply: None,
        scanned: 0,
    });
    wait_until(|| !worker.rounds().is_empty());
    let (reply, answer) = mpsc::channel();
    worker.send(CardsSignal::Populate {
        reply: Some(reply),
        scanned: 0,
    });
    answer
        .recv_timeout(Duration::from_secs(5))
        .expect("answered")
        .expect("ran");
    let asked: Vec<bool> = worker
        .rounds()
        .iter()
        .map(|round| round.ask_unknown)
        .collect();
    assert_eq!(asked, vec![false, true]);
    worker.stop();
}

#[test]
fn a_populate_during_a_run_queues_one_more() {
    let worker = Running::start(quiet(), Duration::from_millis(500));
    worker.send(CardsSignal::Populate {
        reply: None,
        scanned: 0,
    });
    // While the first run is going, three more triggers arrive.
    wait_until(|| worker.started.load(Ordering::SeqCst) >= 1);
    let (reply, answer) = mpsc::channel();
    worker.send(CardsSignal::Populate {
        reply: None,
        scanned: 0,
    });
    worker.send(CardsSignal::SyncPulled);
    worker.send(CardsSignal::Populate {
        reply: Some(reply),
        scanned: 0,
    });
    answer
        .recv_timeout(Duration::from_secs(5))
        .expect("answered")
        .expect("ran");
    wait_until(|| worker.runs().len() >= 2);
    assert_eq!(
        worker.stop(),
        vec![false, true],
        "one more run, with the stub scan the sync pull asked for"
    );
}

#[test]
fn new_stubs_are_debounced_into_one_run() {
    let worker = Running::start(quiet(), Duration::ZERO);
    worker.send(CardsSignal::StubsAdded { count: 1 });
    worker.send(CardsSignal::StubsAdded { count: 1 });
    std::thread::sleep(Duration::from_millis(20));
    worker.send(CardsSignal::StubsAdded { count: 1 });
    assert!(worker.runs().is_empty(), "not before the debounce");
    wait_until(|| !worker.runs().is_empty());
    std::thread::sleep(quiet().debounce * 2);
    assert_eq!(worker.stop(), vec![false]);
}

#[test]
fn population_and_queued_image_work_run_at_once() {
    let worker = Running::start(quiet(), Duration::ZERO);
    worker.send(CardsSignal::Populate {
        reply: None,
        scanned: 0,
    });
    wait_until(|| !worker.rounds().is_empty());
    assert_eq!(
        worker.rounds()[0],
        Round {
            populate: true,
            ..Round::default()
        },
        "a population run (its downloads follow it)"
    );
    worker.send(CardsSignal::ImagesQueued);
    wait_until(|| worker.rounds().len() >= 2);
    assert_eq!(
        worker.rounds()[1],
        Round {
            download: true,
            ..Round::default()
        },
        "queued image work (a 0.x import) downloads"
    );
    worker.stop();
}

#[test]
fn a_move_round_carries_the_move() {
    let worker = Running::start(quiet(), Duration::ZERO);
    let request = MoveRequest {
        from: "/q/from".into(),
        to: "/q/to".into(),
        made: Vec::new(),
    };
    worker.send(CardsSignal::Move(request.clone()));
    wait_until(|| !worker.rounds().is_empty());
    assert_eq!(
        worker.rounds()[0],
        Round {
            moves: vec![request],
            ..Round::default()
        }
    );
    worker.stop();
}

#[test]
fn population_runs_hourly_from_the_last_run() {
    let timing = Timing {
        every: Duration::from_millis(60),
        debounce: Duration::from_millis(10),
        first_run: Duration::from_millis(10),
        retry: Duration::from_millis(10),
    };
    let worker = Running::start(timing, Duration::ZERO);
    wait_until(|| worker.runs().len() >= 3);
    assert!(worker.stop().len() >= 3, "the first run, then every period");
}

#[test]
fn stopping_answers_a_waiting_reply() {
    let (signals, inbox) = mpsc::channel();
    let (reply, answer) = mpsc::channel();
    signals
        .send(CardsSignal::Populate {
            reply: Some(reply),
            scanned: 0,
        })
        .expect("send");
    signals.send(CardsSignal::Stop).expect("send");
    let runs = AtomicUsize::new(0);
    let mut run = |_: &Round, _: &dyn Fn() -> bool| {
        runs.fetch_add(1, Ordering::SeqCst);
        Ran::done(Ok(()))
    };
    run_cards_worker(&inbox, quiet(), &mut run);
    assert_eq!(
        runs.load(Ordering::SeqCst),
        0,
        "stop wins over a queued run"
    );
    assert_eq!(answer.recv().expect("answered"), Err(STOPPED.to_string()));
}

#[test]
fn a_failed_run_answers_its_error() {
    let (signals, inbox) = mpsc::channel();
    let (reply, answer) = mpsc::channel();
    signals
        .send(CardsSignal::Populate {
            reply: Some(reply),
            scanned: 0,
        })
        .expect("send");
    let thread = std::thread::spawn(move || {
        let mut run = |_: &Round, _: &dyn Fn() -> bool| Ran::done(Err("Qunreachable".to_string()));
        run_cards_worker(&inbox, quiet(), &mut run);
    });
    assert_eq!(
        answer
            .recv_timeout(Duration::from_secs(5))
            .expect("answered"),
        Err("Qunreachable".to_string())
    );
    signals.send(CardsSignal::Stop).expect("send");
    thread.join().expect("join");
}

/// The worker loop with a real run: rows fed from the database, the API
/// asked (offline, before sign-in) and the answers stored through the
/// tracker.
#[test]
fn the_worker_stores_what_a_run_learned() {
    let stub = Stub::start(|_| {
        Reply::json(
            200,
            &json!([{"value": 4242, "scryfall_id": "q-scryfall", "name": "Qcard",
                "image": "https://cards.scryfall.io/q.jpg"}])
            .to_string(),
        )
    });
    let api = device_api(&stub.base_url, true);
    let run = tracked("cards-worker-run");
    run.write("INSERT INTO cards (catalog_id) VALUES (4242)");
    let (signals, inbox) = mpsc::channel();
    let (reply, answer) = mpsc::channel();
    signals
        .send(CardsSignal::Populate {
            reply: Some(reply),
            scanned: 0,
        })
        .expect("send");
    // The loop ends once the run is answered and every sender is gone.
    drop(signals);
    let mut populate = |_: &Round, _: &dyn Fn() -> bool| Ran::done(run.populate(&api.state, None));
    run_cards_worker(&inbox, quiet(), &mut populate);
    assert_eq!(answer.recv().expect("answered"), Ok(()));
    assert_eq!(run.card(4242).0.as_deref(), Some("q-scryfall"));
}

/// A worker loop on its own thread whose run is `run`; rounds are recorded.
fn running_with(
    run: impl FnMut(&Round, &dyn Fn() -> bool) -> Ran + Send + 'static,
) -> (Sender<CardsSignal>, Arc<Mutex<Vec<Round>>>, JoinHandle<()>) {
    let (signals, inbox) = mpsc::channel();
    let rounds = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&rounds);
    let mut run = run;
    let thread = std::thread::spawn(move || {
        let mut recording = |round: &Round, should_yield: &dyn Fn() -> bool| {
            lock(&recorded).push(round.clone());
            run(round, should_yield)
        };
        run_cards_worker(&inbox, quiet(), &mut recording);
    });
    (signals, rounds, thread)
}

#[test]
fn a_move_during_a_long_image_run_is_handled_before_it_finishes() {
    let yielded = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&yielded);
    let (signals, rounds, thread) = running_with(move |round, should_yield| {
        if round.download && round.moves.is_empty() {
            // A long download run: one "file" every 10 ms for up to 5 s.
            let started = Instant::now();
            while started.elapsed() < Duration::from_secs(5) {
                if should_yield() {
                    lock(&seen).push(started.elapsed());
                    return Ran {
                        downloads_cut: true,
                        ..Ran::done(Ok(()))
                    };
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        Ran::done(Ok(()))
    });
    let request = MoveRequest {
        from: "/q/from".into(),
        to: "/q/to".into(),
        made: Vec::new(),
    };
    signals.send(CardsSignal::ImagesQueued).expect("send");
    wait_until(|| !lock(&rounds).is_empty());
    signals
        .send(CardsSignal::Move(request.clone()))
        .expect("send");
    wait_until(|| lock(&rounds).len() >= 2);
    let cut_after = lock(&yielded).first().copied().expect("the run yielded");
    assert!(cut_after < Duration::from_secs(2), "{cut_after:?}");
    assert_eq!(
        lock(&rounds)[1],
        Round {
            moves: vec![request],
            download: true,
            ..Round::default()
        },
        "the move runs next, and the cut downloads go on"
    );
    signals.send(CardsSignal::Stop).expect("send");
    thread.join().expect("join");
}

#[test]
fn failed_queued_downloads_are_asked_again() {
    let failures = Arc::new(AtomicUsize::new(1));
    let left = Arc::clone(&failures);
    let (signals, rounds, thread) = running_with(move |round, _| {
        let fail = round.download && left.fetch_sub(1, Ordering::SeqCst) > 0;
        Ran {
            downloads_failed: fail,
            ..Ran::done(Ok(()))
        }
    });
    signals.send(CardsSignal::ImagesQueued).expect("send");
    wait_until(|| lock(&rounds).len() >= 2);
    let download_round = Round {
        download: true,
        ..Round::default()
    };
    assert_eq!(*lock(&rounds), vec![download_round.clone(), download_round]);
    std::thread::sleep(quiet().retry * 4);
    assert_eq!(lock(&rounds).len(), 2, "asked once more, then done");
    signals.send(CardsSignal::Stop).expect("send");
    thread.join().expect("join");
}

// ---- Sweep 8 (#296) -----------------------------------------------------------

// R1-1: a cut download run hands its cursor to the round that goes on.
#[test]
fn a_cut_run_carries_its_cursor_into_the_next_round() {
    let cuts = Arc::new(AtomicUsize::new(1));
    let left = Arc::clone(&cuts);
    let (signals, rounds, thread) = running_with(move |round, _| {
        if round.download && left.fetch_sub(1, Ordering::SeqCst) > 0 {
            return Ran {
                downloads_cut: true,
                resume_after: Some(42),
                ..Ran::done(Ok(()))
            };
        }
        Ran::done(Ok(()))
    });
    signals.send(CardsSignal::ImagesQueued).expect("send");
    wait_until(|| lock(&rounds).len() >= 2);
    let seen = lock(&rounds).clone();
    assert_eq!(seen[0].resume_after, None);
    assert_eq!(
        seen[1],
        Round {
            download: true,
            resume_after: Some(42),
            ..Round::default()
        }
    );
    signals.send(CardsSignal::Stop).expect("send");
    thread.join().expect("join");
}

// R1-2: a download round that could not run (the app not ready, tracking
// restarting) keeps its work and is asked again after the retry wait.
#[test]
fn a_download_round_that_could_not_run_is_asked_again() {
    let failures = Arc::new(AtomicUsize::new(1));
    let left = Arc::clone(&failures);
    let (signals, rounds, thread) = running_with(move |round, _| {
        let fail = round.download && left.fetch_sub(1, Ordering::SeqCst) > 0;
        Ran {
            downloads_failed: fail,
            ..Ran::done(Ok(()))
        }
    });
    signals.send(CardsSignal::ImagesQueued).expect("send");
    wait_until(|| lock(&rounds).len() >= 2);
    let download = Round {
        download: true,
        ..Round::default()
    };
    assert_eq!(*lock(&rounds), vec![download.clone(), download]);
    signals.send(CardsSignal::Stop).expect("send");
    thread.join().expect("join");
}

// S2-2: the stubs the button's own scan added come back as `StubsAdded`;
// they are not a reason for a second run. More than that count still is.
#[test]
fn the_buttons_own_stubs_do_not_queue_a_second_run() {
    let worker = Running::start(quiet(), Duration::ZERO);
    let (reply, answer) = mpsc::channel();
    worker.send(CardsSignal::Populate {
        reply: Some(reply),
        scanned: 3,
    });
    answer
        .recv_timeout(Duration::from_secs(5))
        .expect("answered")
        .expect("ran");
    worker.send(CardsSignal::StubsAdded { count: 3 });
    std::thread::sleep(quiet().debounce * 3);
    assert_eq!(worker.runs().len(), 1, "the scan's own stubs");
    worker.send(CardsSignal::StubsAdded { count: 2 });
    wait_until(|| worker.runs().len() >= 2);
    assert_eq!(worker.stop().len(), 2, "new stubs still run");
}
