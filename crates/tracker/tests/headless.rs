//! The CI gate (spec 7, item 5): the log player writes each fixture into a
//! temporary log folder while the real tracker thread runs. Pushed views, stored
//! rows, rebuild from storage and the stored event count must all agree with the
//! projection. Write-to-push latency is reported, not asserted (spec 7, item 8).

mod common;

use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use common::*;
use fixture_tools::{TestDir, fixture_names, load_fixture, prepared_events};
use mtgo_match::{ABANDON_AFTER_MS, MatchState, Projector};
use mtgo_protocol::{EventKind, split_complete};
use store::names::{outcome_str, state_str};
use store::{open_reader, read};
use tracker::{Clock, TrackerConfig, TrackerEvent};

fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(120);
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn every_fixture_flows_through_the_running_tracker() {
    for name in fixture_names() {
        let fixture = load_fixture(&name);
        let dir = TestDir::new(&format!("headless-{name}"));
        let config = config(&dir, &fixture);
        // An empty log first, so the file's row is committed before the player
        // writes a byte and every write is picked up by a poll. Replay mode never
        // dates events from the read time, so this is not a live-dating test.
        let log_path = dir.write("logs/mtgo.log", b"");

        let (sink, receiver) = mpsc::channel();
        let collector = thread::spawn(move || {
            receiver
                .iter()
                .map(|event| (Instant::now(), event))
                .collect::<Vec<(Instant, TrackerEvent)>>()
        });
        // The log clock (#858): the time rules judge the fixture's own time.
        let tracker =
            tracker::spawn_with_clock(config.clone(), Clock::Log, sink).expect("spawn tracker");
        wait_until("the log file row", || committed_cursor(&config).is_some());

        let mut writes = Vec::new();
        log_player::play(&fixture.bytes, &log_path, 0.0, |written| {
            writes.push(written)
        })
        .expect("play");
        let size = fixture.bytes.len() as i64;
        wait_until("the whole log to commit", || {
            committed_cursor(&config) == Some(size)
        });
        tracker.stop();
        let received = collector.join().expect("collector");
        // The log's time where it ends. The tracker's clock ran on a few real
        // seconds past it; no rule turns within seconds.
        let now = prepared_events(&fixture)
            .iter()
            .map(|event| event.at_ms)
            .max()
            .expect("events");

        let events: Vec<TrackerEvent> = received.iter().map(|(_, event)| event.clone()).collect();
        let want = expected(&fixture, now);
        assert_eq!(last_views(&events), want, "pushed views, fixture {name}");

        let reader = open_reader(&config.db_path).expect("reader");
        let mut stored: Vec<(String, String)> = read::list_matches(&reader, 100, 0)
            .expect("list")
            .into_iter()
            .map(|row| (row.token, row.outcome))
            .collect();
        stored.sort();
        let mut wanted: Vec<(String, String)> = want
            .iter()
            .map(|view| (view.token.clone(), outcome_str(view.outcome).to_string()))
            .collect();
        wanted.sort();
        assert_eq!(stored, wanted, "stored rows, fixture {name}");

        let tokens: Vec<String> = want.iter().map(|view| view.token.clone()).collect();
        assert_eq!(
            rebuilt_from_storage(&config, &tokens, now),
            want,
            "rebuild from storage, fixture {name}"
        );

        // Every match the store holds, not only the expected ones: each must rebuild
        // to a visible view that matches its stored row.
        let mut stored_tokens = read::stored_match_tokens(&reader).expect("stored tokens");
        stored_tokens.sort();
        let mut wanted_tokens: Vec<String> = want.iter().map(|view| view.token.clone()).collect();
        wanted_tokens.sort();
        assert_eq!(
            stored_tokens, wanted_tokens,
            "stored tokens, fixture {name}"
        );
        let mut rebuilt_rows: Vec<(String, String)> =
            rebuilt_from_storage(&config, &stored_tokens, now)
                .iter()
                .map(|view| (view.token.clone(), outcome_str(view.outcome).to_string()))
                .collect();
        rebuilt_rows.sort();
        assert_eq!(
            rebuilt_rows, stored,
            "every stored match rebuilds to its stored row, fixture {name}"
        );
        assert_eq!(
            stored_event_count(&config),
            prepared_events(&fixture).len() as i64,
            "every whitelisted event stored exactly once, fixture {name}"
        );
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, TrackerEvent::Restarting { .. })),
            "the tracker never restarted, fixture {name}"
        );
        assert_eq!(
            last_status(&events).map(|status| status.quarantined),
            Some(0),
            "no event was quarantined, fixture {name}"
        );

        let mut latencies: Vec<Duration> = Vec::new();
        for (at, event) in &received {
            if let TrackerEvent::MatchUpdated {
                cause: Some((_, offset)),
                ..
            } = event
                && let Some(write) = writes.iter().find(|w| w.end_offset > *offset)
            {
                latencies.push(at.saturating_duration_since(write.at));
            }
        }
        latencies.sort();
        if !latencies.is_empty() {
            let count = latencies.len();
            eprintln!(
                "fixture {name}: {count} match updates, write-to-push p50 {:?}, p95 {:?}",
                latencies[count / 2],
                latencies[(count * 95 / 100).min(count - 1)]
            );
        }
    }
}

/// The stored state of the match `token`, if it has a row.
fn stored_state(config: &TrackerConfig, token: &str) -> Option<String> {
    let reader = open_reader(&config.db_path).ok()?;
    read::list_matches(&reader, 100, 0)
        .ok()?
        .into_iter()
        .find(|row| row.token == token)
        .map(|row| row.state)
}

/// A replay on the log clock (#858): a match still in progress where the log
/// stops is not abandoned because the replay runs months later (the wall
/// clock did that), and it is abandoned once the log itself moves an hour
/// past it.
#[test]
fn a_replay_on_the_log_clock_runs_the_time_rules_on_log_time() {
    let fixture = load_fixture("league_join_drop");
    let events = prepared_events(&fixture);

    // The first event after which a match is in progress as of that event.
    let mut projector = Projector::new(vec![]);
    let mut found = None;
    for (index, event) in events.iter().enumerate() {
        if event.kind == EventKind::Login
            && let Some(name) = &event.local_username
        {
            projector.add_account(name);
        }
        projector.apply(event);
        projector.finalize(event.at_ms);
        if let Some(view) = projector
            .views()
            .into_iter()
            .find(|view| view.state == MatchState::InProgress)
        {
            found = Some((index, view.token.clone()));
            break;
        }
    }
    let (index, token) = found.expect("a match in progress somewhere in the fixture");
    let last = &events[index];
    // The log stops where the next stored event would start.
    let cut = events[index + 1].offset as usize;
    let prefix = &fixture.bytes[..cut];

    let dir = TestDir::new("headless-log-clock");
    let config = config(&dir, &fixture);
    let log_path = dir.write("logs/mtgo.log", b"");
    let (sink, receiver) = mpsc::channel();
    let collector = thread::spawn(move || receiver.iter().count());
    let tracker =
        tracker::spawn_with_clock(config.clone(), Clock::Log, sink).expect("spawn tracker");
    wait_until("the log file row", || committed_cursor(&config).is_some());

    log_player::play(prefix, &log_path, 0.0, |_| {}).expect("play the prefix");
    wait_until("the prefix to commit", || {
        committed_cursor(&config) == Some(cut as i64)
    });
    wait_until("the match row", || stored_state(&config, &token).is_some());
    // Several idle ticks: on the wall clock the first one abandons it.
    thread::sleep(Duration::from_millis(1_500));
    assert_eq!(
        stored_state(&config, &token).as_deref(),
        Some("in_progress"),
        "the match the log stops in is still in progress on log time"
    );

    // The log moves on: a copy of the fixture's Login, stamped an hour and a
    // minute after the match's last event. Its time alone abandons the match.
    let login = events
        .iter()
        .find(|event| event.kind == EventKind::Login)
        .expect("a Login in the fixture");
    let raw = split_complete(&fixture.bytes)
        .into_iter()
        .find(|raw| raw.offset == login.offset)
        .expect("the Login's raw event");
    let start = raw.offset as usize;
    let end = ((raw.offset + raw.len) as usize).min(fixture.bytes.len());
    let mut later = fixture.bytes[start..end].to_vec();
    let stamp = last.header.time + chrono::Duration::milliseconds(ABANDON_AFTER_MS + 60_000);
    assert!(
        stamp > last.header.time,
        "the new stamp stays on the same day"
    );
    later[..8].copy_from_slice(stamp.format("%H:%M:%S").to_string().as_bytes());
    projector.finalize(last.at_ms + ABANDON_AFTER_MS + 60_000);
    let abandoned = projector.view(&token).expect("the match").state;
    assert_ne!(abandoned, MatchState::InProgress, "an hour on, it is over");
    log_player::play(&later, &log_path, 0.0, |_| {}).expect("play the Login");
    wait_until("the match to end on log time", || {
        stored_state(&config, &token).as_deref() == Some(state_str(abandoned))
    });

    tracker.stop();
    collector.join().expect("collector");
}
