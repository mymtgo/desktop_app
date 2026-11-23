//! The bridge thread (spec 3, UI push): every tracker event updates the live
//! views and the status, then goes to every window as a typed event. The UI
//! path never waits on the database.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use mtgo_match::MatchView;
use store::{Connection, StoreError, read};
use tauri::AppHandle;
use tauri_specta::Event;
use tracker::TrackerEvent;

use crate::api::worker::WorkerSignal;
use crate::archetypes::worker::ArchetypeSignal;
use crate::cards::worker::CardsSignal;
use crate::events::{
    AccountChanged, AppNotification, ArchetypesChanged, CardsChanged, DecksChanged, LeaguesChanged,
    LimitedChanged, MatchArchetypesChanged, MatchAttention, MatchPreviewed, MatchRemoved,
    MatchUpdated, RankChanged, StatusChanged, TrackerRestarting,
};
use crate::live::LiveMatches;
use crate::overlay::worker::OverlayMsg;
use crate::summary::{StatusView, from_manual_view, from_view};

/// How often settled live views are checked against storage.
pub const FORGET_EVERY: Duration = Duration::from_secs(2);

/// State the bridge writes and the commands read.
pub struct Shared {
    pub live: Mutex<LiveMatches>,
    pub status: Mutex<StatusView>,
}

impl Shared {
    pub fn new(timezone_warning: Option<String>) -> Self {
        Self {
            live: Mutex::new(LiveMatches::default()),
            status: Mutex::new(StatusView::waiting(timezone_warning)),
        }
    }
}

/// Lock, recovering a lock poisoned by a panic elsewhere: the data inside is
/// the last state written in full.
pub fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[derive(Debug, Clone, PartialEq)]
pub enum Outgoing {
    Updated(MatchUpdated),
    Previewed(MatchPreviewed),
    Removed(MatchRemoved),
    Attention(MatchAttention),
    Status(StatusChanged),
    Restarting(TrackerRestarting),
    DecksChanged(DecksChanged),
    AccountChanged(AccountChanged),
    RankChanged(RankChanged),
    LeaguesChanged(LeaguesChanged),
    LimitedChanged(LimitedChanged),
    CardsChanged(CardsChanged),
    MatchArchetypesChanged(MatchArchetypesChanged),
    ArchetypesChanged(ArchetypesChanged),
}

/// Keep both players' names for the scrubber (spec 9), with an Arena
/// opponent's display name.
fn remember_players(view: &MatchView, opponent_name: Option<&str>) {
    let mut names = vec![view.local_username.clone()];
    names.extend(view.opponent.clone());
    names.extend(opponent_name.map(str::to_string));
    crate::scrub::remember(names);
}

/// Update the shared state for one tracker event and say what to push.
pub fn apply(shared: &Shared, event: TrackerEvent) -> Vec<Outgoing> {
    match event {
        TrackerEvent::MatchUpdated {
            view,
            revision,
            manual,
            platform,
            opponent_name,
            ..
        } => {
            remember_players(&view, opponent_name.as_deref());
            let summary = if manual {
                from_manual_view(&view, revision, platform, opponent_name)
            } else {
                from_view(&view, revision, platform, opponent_name)
            };
            lock(&shared.live).update(summary.clone());
            vec![Outgoing::Updated(MatchUpdated { summary })]
        }
        TrackerEvent::MatchPreview {
            view,
            platform,
            opponent_name,
        } => {
            remember_players(&view, opponent_name.as_deref());
            let summary = from_view(&view, 0, platform, opponent_name);
            vec![Outgoing::Previewed(MatchPreviewed { summary })]
        }
        TrackerEvent::MatchRemoved { token } => {
            lock(&shared.live).remove(&token);
            vec![Outgoing::Removed(MatchRemoved { token })]
        }
        TrackerEvent::MatchFailed { token, reason } => {
            lock(&shared.live).mark_attention(&token, &reason);
            vec![Outgoing::Attention(MatchAttention { token, reason })]
        }
        TrackerEvent::Status(status) => {
            let mut view = StatusView::from_status(&status);
            let view = {
                let mut current = lock(&shared.status);
                // Offline mode is the app's, not the tracker's: keep it.
                view.offline_mode = current.offline_mode;
                *current = view.clone();
                view
            };
            vec![Outgoing::Status(StatusChanged { status: view })]
        }
        TrackerEvent::Restarting { reason } => {
            lock(&shared.live).clear();
            let status = {
                let mut status = lock(&shared.status);
                status.restarting = Some(reason.clone());
                status.clone()
            };
            vec![
                Outgoing::Restarting(TrackerRestarting { reason }),
                Outgoing::Status(StatusChanged { status }),
            ]
        }
        TrackerEvent::DecksChanged => vec![Outgoing::DecksChanged(DecksChanged {})],
        TrackerEvent::AccountChanged => vec![Outgoing::AccountChanged(AccountChanged {})],
        TrackerEvent::RankChanged => vec![Outgoing::RankChanged(RankChanged {})],
        TrackerEvent::LeaguesChanged => vec![Outgoing::LeaguesChanged(LeaguesChanged {})],
        TrackerEvent::LimitedChanged => vec![Outgoing::LimitedChanged(LimitedChanged {})],
        // Nothing for the pages: the overlay worker takes it
        // (`OverlayMsg::TournamentsChanged`) and pushes the event card.
        TrackerEvent::TournamentsChanged => Vec::new(),
        // Nothing for the UI: the cards worker wakes on it (`wakes_the_cards_worker`).
        TrackerEvent::CardStubsAdded { .. } => Vec::new(),
        TrackerEvent::CardsChanged => vec![Outgoing::CardsChanged(CardsChanged {})],
        // Nothing for the UI: the overlay worker takes it (`run`).
        TrackerEvent::LiveEventsStored { .. } => Vec::new(),
        TrackerEvent::MatchArchetypesChanged { match_ids } => {
            vec![Outgoing::MatchArchetypesChanged(MatchArchetypesChanged {
                match_ids,
            })]
        }
        // The pages that show archetype names and counts refetch
        // (`ui/src/lib/liveEvents.ts`, final review I1).
        TrackerEvent::ArchetypesChanged => vec![Outgoing::ArchetypesChanged(ArchetypesChanged {})],
        // Nothing for the UI: the archetypes worker prefills the decks
        // (`decks_needing_archetypes`).
        TrackerEvent::DeckNeedsArchetype { .. } => Vec::new(),
    }
}

/// The event makes a log match complete that the live views did not hold
/// as complete (0.x `MtgoMatchObserver`: a completion schedules a sync).
/// Read before `apply`. A view dropped once storage caught up counts again
/// if it is pushed again; the worker folds close completions into one run.
pub fn became_complete(shared: &Shared, event: &TrackerEvent) -> bool {
    match event {
        TrackerEvent::MatchUpdated {
            view,
            manual: false,
            ..
        } => {
            view.state == mtgo_match::MatchState::Complete
                && !lock(&shared.live).is_complete(&view.token)
        }
        _ => false,
    }
}

/// How lately a match must have ended for its toast: one the catch-up after
/// a restart completes is not news.
pub const MATCH_TOAST_WINDOW_MS: i64 = 10 * 60 * 1000;

/// The "Match recorded 2-1" toast (0.x `MtgoMatchObserver`) when the event
/// completes a log match the live views held unfinished, ended within
/// `MATCH_TOAST_WINDOW_MS` of `now_ms`. Read before `apply`.
pub fn match_toast(shared: &Shared, event: &TrackerEvent, now_ms: i64) -> Option<AppNotification> {
    let TrackerEvent::MatchUpdated {
        view,
        manual: false,
        ..
    } = event
    else {
        return None;
    };
    if view.state != mtgo_match::MatchState::Complete
        || !lock(&shared.live).is_unfinished(&view.token)
    {
        return None;
    }
    let ended_at = view.ended_at_ms.unwrap_or(view.last_activity_ms);
    if now_ms - ended_at >= MATCH_TOAST_WINDOW_MS {
        return None;
    }
    let (kind, message) = match view.outcome {
        mtgo_match::Outcome::Win => ("match_win", "Win"),
        mtgo_match::Outcome::Loss => ("match_loss", "Loss"),
        mtgo_match::Outcome::Draw => ("match_voided", "Draw"),
        mtgo_match::Outcome::Unknown => ("match_incomplete", "Result unknown"),
    };
    Some(AppNotification {
        kind: kind.to_string(),
        title: format!("Match recorded {}-{}", view.wins, view.losses),
        message: message.to_string(),
        route: Some(format!("/matches/{}", view.token)),
    })
}

/// The league sync changed league or tournament rows: a new tournament's
/// real name may be asked (the worker's query finds the new rows; a run
/// with none makes no request).
pub fn asks_tournament_names(event: &TrackerEvent) -> bool {
    matches!(event, TrackerEvent::LeaguesChanged)
}

/// New card stubs wake the cards worker (debounced there).
pub fn wakes_the_cards_worker(event: &TrackerEvent) -> bool {
    stubs_added(event).is_some()
}

/// How many card stubs the event reports added.
pub fn stubs_added(event: &TrackerEvent) -> Option<u32> {
    match event {
        TrackerEvent::CardStubsAdded { count } => Some(*count),
        _ => None,
    }
}

/// The decks the deck sync wrote without an archetype, for the archetypes
/// worker's prefill (decision C5).
pub fn decks_needing_archetypes(event: &TrackerEvent) -> Vec<i64> {
    match event {
        TrackerEvent::DeckNeedsArchetype { deck_ids } => deck_ids.clone(),
        _ => Vec::new(),
    }
}

/// Offline mode changed: store the flag in the shared status (what
/// `get_status` returns) and say what to push to the status bar.
pub fn set_offline_mode(shared: &Shared, offline: bool) -> StatusChanged {
    let mut status = lock(&shared.status);
    status.offline_mode = offline;
    StatusChanged {
        status: status.clone(),
    }
}

/// Drop live views that storage has caught up with, so memory stays bounded by
/// the matches still in progress.
pub fn forget_stored(shared: &Shared, conn: &Connection) -> Result<(), StoreError> {
    let tokens = lock(&shared.live).settled_tokens();
    for token in tokens {
        let stored = read::match_revision(conn, &token)?;
        let hidden = stored.is_some() && store::match_edits::is_hidden(conn, &token)?;
        lock(&shared.live).forget_stored(&token, stored, hidden);
    }
    Ok(())
}

/// Start the bridge thread. It runs until every sender is gone (app exit).
pub fn spawn(
    app: AppHandle,
    events: Receiver<TrackerEvent>,
    shared: Arc<Shared>,
    db_path: PathBuf,
    overlay: Sender<OverlayMsg>,
) -> std::io::Result<JoinHandle<()>> {
    std::thread::Builder::new()
        .name("bridge".to_string())
        .spawn(move || run(&app, &events, &shared, &db_path, &overlay))
}

fn run(
    app: &AppHandle,
    events: &Receiver<TrackerEvent>,
    shared: &Shared,
    db_path: &std::path::Path,
    overlay: &Sender<OverlayMsg>,
) {
    let reader = match store::open_reader(db_path) {
        Ok(conn) => Some(conn),
        Err(error) => {
            tracing::error!(%error, "the bridge could not open the database; finished matches stay in memory");
            None
        }
    };
    let mut last_forget = Instant::now();
    loop {
        match events.recv_timeout(FORGET_EVERY) {
            Ok(event) => {
                let started = Instant::now();
                let completed = became_complete(shared, &event);
                let toast = match_toast(shared, &event, crate::wall_clock_ms());
                let names = asks_tournament_names(&event);
                let stubs = stubs_added(&event);
                // Never waits: the overlay worker reads its channel.
                if let Some(message) = crate::overlay::worker::from_tracker(&event) {
                    let _ = overlay.send(message);
                }
                let prefill = decks_needing_archetypes(&event);
                let outgoing = apply(shared, event);
                if completed {
                    crate::api::worker::signal(app, WorkerSignal::MatchCompleted);
                    crate::archetypes::worker::signal(app, ArchetypeSignal::MatchCompleted);
                }
                if let Some(toast) = toast
                    && let Err(error) = toast.emit(app)
                {
                    tracing::warn!(%error, "could not raise the match toast");
                }
                for deck_id in prefill {
                    crate::archetypes::worker::signal(app, ArchetypeSignal::DeckSynced(deck_id));
                }
                if names {
                    crate::api::worker::signal(app, WorkerSignal::TournamentAdded);
                }
                if let Some(count) = stubs {
                    crate::cards::worker::signal(app, CardsSignal::StubsAdded { count });
                }
                let count = outgoing.len();
                for item in outgoing {
                    emit(app, item);
                }
                tracing::debug!(
                    target: "mymtgo::timing",
                    emit_us = started.elapsed().as_micros() as u64,
                    events = count,
                    "push"
                );
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
        if last_forget.elapsed() >= FORGET_EVERY {
            last_forget = Instant::now();
            if let Some(conn) = &reader
                && let Err(error) = forget_stored(shared, conn)
            {
                tracing::warn!(%error, "could not check stored matches");
            }
        }
    }
}

fn emit(app: &AppHandle, item: Outgoing) {
    let result = match item {
        Outgoing::Updated(event) => event.emit(app),
        Outgoing::Previewed(event) => event.emit(app),
        Outgoing::Removed(event) => event.emit(app),
        Outgoing::Attention(event) => event.emit(app),
        Outgoing::Status(event) => event.emit(app),
        Outgoing::Restarting(event) => event.emit(app),
        Outgoing::DecksChanged(event) => event.emit(app),
        Outgoing::AccountChanged(event) => event.emit(app),
        Outgoing::RankChanged(event) => event.emit(app),
        Outgoing::LeaguesChanged(event) => event.emit(app),
        Outgoing::LimitedChanged(event) => event.emit(app),
        Outgoing::CardsChanged(event) => event.emit(app),
        Outgoing::MatchArchetypesChanged(event) => event.emit(app),
        Outgoing::ArchetypesChanged(event) => event.emit(app),
    };
    if let Err(error) = result {
        tracing::warn!(%error, "could not push an event to the windows");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::summary::testing::view;
    use fixture_tools::TestDir;
    use mtgo_match::MatchState;
    use store::open_writer;
    use store::write::{MatchWrite, WriteBatch, commit};

    fn updated(token: &str, revision: u64, state: MatchState) -> TrackerEvent {
        TrackerEvent::MatchUpdated {
            view: view(token, 1_000, state),
            revision,
            cause: None,
            manual: false,
            platform: store::Platform::Mtgo,
            opponent_name: None,
        }
    }

    fn page_tokens(shared: &Shared) -> Vec<String> {
        lock(&shared.live)
            .merge(
                Vec::new(),
                true,
                crate::live::PER_PAGE,
                store::Platform::Mtgo,
            )
            .into_iter()
            .map(|row| row.token)
            .collect()
    }

    // 0.x `MtgoMatchObserver`: "Match recorded 2-1", leading to the match.
    #[test]
    fn a_match_seen_in_progress_raises_a_toast_when_it_completes() {
        let shared = Shared::new(None);
        let now = 1_000 + 4_000 + 60_000;
        let complete = updated("t1", 2, MatchState::Complete);
        assert_eq!(
            match_toast(&shared, &complete, now),
            None,
            "never seen live"
        );

        apply(&shared, updated("t1", 1, MatchState::InProgress));
        let toast = match_toast(&shared, &complete, now).expect("toast");
        assert_eq!(toast.kind, "match_win");
        assert_eq!(toast.title, "Match recorded 2-1");
        assert_eq!(toast.message, "Win");
        assert_eq!(toast.route.as_deref(), Some("/matches/t1"));

        let late = now + MATCH_TOAST_WINDOW_MS;
        assert_eq!(
            match_toast(&shared, &complete, late),
            None,
            "caught up later"
        );
        apply(&shared, complete.clone());
        assert_eq!(
            match_toast(&shared, &complete, now),
            None,
            "already complete"
        );
    }

    #[test]
    fn a_match_that_becomes_complete_is_told_to_the_worker_once() {
        let shared = Shared::new(None);
        let event = updated("t1", 1, MatchState::InProgress);
        assert!(!became_complete(&shared, &event));
        apply(&shared, event);
        let complete = updated("t1", 2, MatchState::Complete);
        assert!(became_complete(&shared, &complete));
        apply(&shared, complete);
        let again = updated("t1", 3, MatchState::Complete);
        assert!(!became_complete(&shared, &again), "already complete");
        let unseen = updated("t2", 1, MatchState::Complete);
        assert!(became_complete(&shared, &unseen), "complete on first sight");
        let manual = TrackerEvent::MatchUpdated {
            view: view("t3", 1_000, MatchState::Complete),
            revision: 1,
            cause: None,
            manual: true,
            platform: store::Platform::Mtgo,
            opponent_name: None,
        };
        assert!(!became_complete(&shared, &manual), "a log match only");
        assert!(!became_complete(&shared, &TrackerEvent::DecksChanged));
    }

    // Q7: the league sync's `LeaguesChanged` may mean a new tournament row,
    // so the worker asks for tournament names at its next tick.
    #[test]
    fn a_leagues_change_asks_for_tournament_names() {
        assert!(asks_tournament_names(&TrackerEvent::LeaguesChanged));
        assert!(!asks_tournament_names(&TrackerEvent::DecksChanged));
        assert!(!asks_tournament_names(&TrackerEvent::LimitedChanged));
        assert!(!asks_tournament_names(&updated(
            "t1",
            1,
            MatchState::Complete
        )));
    }

    #[test]
    fn an_update_is_kept_live_and_pushed() {
        let shared = Shared::new(None);
        let out = apply(&shared, updated("t1", 3, MatchState::InProgress));
        let summary = from_view(
            &view("t1", 1_000, MatchState::InProgress),
            3,
            store::Platform::Mtgo,
            None,
        );
        assert_eq!(out, vec![Outgoing::Updated(MatchUpdated { summary })]);
        assert_eq!(page_tokens(&shared), vec!["t1".to_string()]);
    }

    #[test]
    fn an_arena_update_carries_its_platform_and_opponent_name() {
        let shared = Shared::new(None);
        let mut arena = view("t-arena", 1_000, MatchState::InProgress);
        arena.opponent = Some("arena:u1".to_string());
        let out = apply(
            &shared,
            TrackerEvent::MatchUpdated {
                view: arena,
                revision: 1,
                cause: None,
                manual: false,
                platform: store::Platform::Arena,
                opponent_name: Some("Qarenaopp".to_string()),
            },
        );
        let [Outgoing::Updated(MatchUpdated { summary })] = out.as_slice() else {
            panic!("not one update: {out:?}");
        };
        assert_eq!(summary.platform, crate::platform::PlatformView::Arena);
        assert_eq!(summary.opponent.as_deref(), Some("Qarenaopp"));
        let live = lock(&shared.live);
        let arena_page = live.merge(
            Vec::new(),
            true,
            crate::live::PER_PAGE,
            store::Platform::Arena,
        );
        assert_eq!(arena_page.len(), 1);
        assert!(
            live.merge(
                Vec::new(),
                true,
                crate::live::PER_PAGE,
                store::Platform::Mtgo
            )
            .is_empty()
        );
    }

    #[test]
    fn a_pushed_manual_match_is_marked_manual_and_a_log_match_is_not() {
        let shared = Shared::new(None);
        let manual = apply(
            &shared,
            TrackerEvent::MatchUpdated {
                view: view("t-manual", 1_000, MatchState::Complete),
                revision: 1,
                cause: None,
                manual: true,
                platform: store::Platform::Mtgo,
                opponent_name: None,
            },
        );
        let log = apply(&shared, updated("t-log", 1, MatchState::Complete));
        let marked = |out: &[Outgoing]| match out {
            [Outgoing::Updated(MatchUpdated { summary })] => summary.manual,
            other => panic!("not one update: {other:?}"),
        };
        assert!(marked(&manual));
        assert!(!marked(&log));
    }

    #[test]
    fn a_preview_is_pushed_but_never_kept_live() {
        let shared = Shared::new(None);
        let out = apply(
            &shared,
            TrackerEvent::MatchPreview {
                view: view("t1", 1_000, MatchState::InProgress),
                platform: store::Platform::Mtgo,
                opponent_name: None,
            },
        );
        let summary = from_view(
            &view("t1", 1_000, MatchState::InProgress),
            0,
            store::Platform::Mtgo,
            None,
        );
        assert_eq!(out, vec![Outgoing::Previewed(MatchPreviewed { summary })]);
        assert!(lock(&shared.live).is_empty());
    }

    #[test]
    fn a_removal_hides_the_match_and_is_pushed() {
        let shared = Shared::new(None);
        apply(&shared, updated("t1", 3, MatchState::InProgress));
        let out = apply(
            &shared,
            TrackerEvent::MatchRemoved {
                token: "t1".to_string(),
            },
        );
        assert_eq!(
            out,
            vec![Outgoing::Removed(MatchRemoved {
                token: "t1".to_string()
            })]
        );
        assert!(page_tokens(&shared).is_empty());
    }

    #[test]
    fn a_failure_marks_the_live_view_and_is_pushed() {
        let shared = Shared::new(None);
        apply(&shared, updated("t1", 3, MatchState::InProgress));
        let out = apply(
            &shared,
            TrackerEvent::MatchFailed {
                token: "t1".to_string(),
                reason: "boom".to_string(),
            },
        );
        assert_eq!(
            out,
            vec![Outgoing::Attention(MatchAttention {
                token: "t1".to_string(),
                reason: "boom".to_string()
            })]
        );
        let rows = lock(&shared.live).merge(
            Vec::new(),
            true,
            crate::live::PER_PAGE,
            store::Platform::Mtgo,
        );
        assert!(rows[0].needs_attention);
    }

    #[test]
    fn a_status_is_kept_with_the_timezone_warning() {
        let shared = Shared::new(None);
        assert!(!lock(&shared.status).started);
        let status = tracker::Status {
            timezone_warning: Some("tz".to_string()),
            ..tracker::Status::default()
        };
        let out = apply(&shared, TrackerEvent::Status(status.clone()));
        let expected = StatusView::from_status(&status);
        assert_eq!(
            out,
            vec![Outgoing::Status(StatusChanged {
                status: expected.clone()
            })]
        );
        assert_eq!(*lock(&shared.status), expected);
    }

    // 0.x SharedPropTest: "shares offline mode as a top level prop" and
    // "flips the shared prop with the setting". `get_status` returns the
    // shared status, so it carries the flag.
    #[test]
    fn the_status_carries_offline_mode_and_flips_with_it() {
        let shared = Shared::new(None);
        assert!(!lock(&shared.status).offline_mode);
        let pushed = set_offline_mode(&shared, true);
        assert!(pushed.status.offline_mode);
        assert!(lock(&shared.status).offline_mode);
        // A tracker report keeps the flag.
        let out = apply(&shared, TrackerEvent::Status(tracker::Status::default()));
        let Outgoing::Status(StatusChanged { status }) = &out[0] else {
            panic!("a status: {out:?}");
        };
        assert!(status.offline_mode);
        assert!(lock(&shared.status).offline_mode);
        let pushed = set_offline_mode(&shared, false);
        assert!(!pushed.status.offline_mode);
        assert!(!lock(&shared.status).offline_mode);
    }

    #[test]
    fn restarting_forgets_every_live_view_and_reports_it() {
        let shared = Shared::new(None);
        apply(&shared, updated("t1", 3, MatchState::InProgress));
        apply(&shared, TrackerEvent::Status(tracker::Status::default()));
        let out = apply(
            &shared,
            TrackerEvent::Restarting {
                reason: "disk full".to_string(),
            },
        );
        assert!(lock(&shared.live).is_empty());
        let status = lock(&shared.status).clone();
        assert_eq!(status.restarting.as_deref(), Some("disk full"));
        assert_eq!(
            out,
            vec![
                Outgoing::Restarting(TrackerRestarting {
                    reason: "disk full".to_string()
                }),
                Outgoing::Status(StatusChanged { status }),
            ]
        );
        // The restarted tracker's first status clears the banner.
        apply(&shared, TrackerEvent::Status(tracker::Status::default()));
        assert_eq!(lock(&shared.status).restarting, None);
    }

    #[test]
    fn card_changes_are_pushed_and_new_stubs_wake_the_cards_worker() {
        let shared = Shared::new(None);
        assert_eq!(
            apply(&shared, TrackerEvent::CardsChanged),
            vec![Outgoing::CardsChanged(CardsChanged {})]
        );
        let stubs = TrackerEvent::CardStubsAdded { count: 3 };
        assert!(wakes_the_cards_worker(&stubs));
        assert!(apply(&shared, stubs).is_empty(), "nothing for the UI");
        assert!(!wakes_the_cards_worker(&TrackerEvent::CardsChanged));
    }

    #[test]
    fn archetype_changes_are_pushed_and_decks_without_one_go_to_the_worker() {
        let shared = Shared::new(None);
        assert_eq!(
            apply(
                &shared,
                TrackerEvent::MatchArchetypesChanged {
                    match_ids: vec![3, 4]
                }
            ),
            vec![Outgoing::MatchArchetypesChanged(MatchArchetypesChanged {
                match_ids: vec![3, 4]
            })]
        );
        let needs = TrackerEvent::DeckNeedsArchetype {
            deck_ids: vec![7, 8],
        };
        assert_eq!(decks_needing_archetypes(&needs), [7, 8]);
        assert!(apply(&shared, needs).is_empty(), "nothing for the UI");
        assert!(decks_needing_archetypes(&TrackerEvent::DecksChanged).is_empty());
        // Final review I1: definitions (names, moves, new rows) reach the
        // pages that show archetypes.
        assert_eq!(
            apply(&shared, TrackerEvent::ArchetypesChanged),
            vec![Outgoing::ArchetypesChanged(ArchetypesChanged {})]
        );
    }

    #[test]
    fn a_decks_change_is_pushed_and_leaves_the_live_views_alone() {
        let shared = Shared::new(None);
        apply(&shared, updated("t1", 3, MatchState::InProgress));
        let out = apply(&shared, TrackerEvent::DecksChanged);
        assert_eq!(out, vec![Outgoing::DecksChanged(DecksChanged {})]);
        // Release spec 5: one event, every page refetches.
        let out = apply(&shared, TrackerEvent::AccountChanged);
        assert_eq!(out, vec![Outgoing::AccountChanged(AccountChanged {})]);
        let out = apply(&shared, TrackerEvent::RankChanged);
        assert_eq!(out, vec![Outgoing::RankChanged(RankChanged {})]);
        let out = apply(&shared, TrackerEvent::LeaguesChanged);
        assert_eq!(out, vec![Outgoing::LeaguesChanged(LeaguesChanged {})]);
        let out = apply(&shared, TrackerEvent::LimitedChanged);
        assert_eq!(out, vec![Outgoing::LimitedChanged(LimitedChanged {})]);
        assert_eq!(page_tokens(&shared), vec!["t1".to_string()]);
    }

    #[test]
    fn settled_views_are_forgotten_once_stored() {
        let dir = TestDir::new("bridge-forget");
        let mut conn = open_writer(&dir.join("mymtgo.sqlite")).expect("open");
        let shared = Shared::new(None);
        apply(&shared, updated("done", 4, MatchState::Complete));
        apply(&shared, updated("playing", 2, MatchState::InProgress));

        forget_stored(&shared, &conn).expect("forget");
        assert_eq!(lock(&shared.live).len(), 2, "nothing stored yet");

        commit(
            &mut conn,
            &WriteBatch {
                matches: vec![
                    MatchWrite::Upsert {
                        view: Box::new(view("done", 1_000, MatchState::Complete)),
                        revision: 4,
                    },
                    MatchWrite::Upsert {
                        view: Box::new(view("playing", 1_000, MatchState::InProgress)),
                        revision: 2,
                    },
                ],
                ..WriteBatch::default()
            },
        )
        .expect("commit");
        forget_stored(&shared, &conn).expect("forget");
        assert_eq!(page_tokens(&shared), vec!["playing".to_string()]);
    }

    #[test]
    fn a_removal_is_forgotten_once_storage_hides_the_match() {
        let dir = TestDir::new("bridge-forget-hidden");
        let mut conn = open_writer(&dir.join("mymtgo.sqlite")).expect("open");
        let shared = Shared::new(None);
        apply(&shared, updated("gone", 4, MatchState::Complete));
        commit(
            &mut conn,
            &WriteBatch {
                matches: vec![MatchWrite::Upsert {
                    view: Box::new(view("gone", 1_000, MatchState::Complete)),
                    revision: 4,
                }],
                ..WriteBatch::default()
            },
        )
        .expect("commit");
        // "Remove from stats": the tracker reports the match removed.
        apply(
            &shared,
            TrackerEvent::MatchRemoved {
                token: "gone".to_string(),
            },
        );
        forget_stored(&shared, &conn).expect("forget");
        assert_eq!(lock(&shared.live).len(), 1, "still listed: keep hiding it");

        conn.execute("UPDATE matches SET hidden_at = 1 WHERE token = 'gone'", [])
            .expect("hide");
        forget_stored(&shared, &conn).expect("forget");
        assert!(lock(&shared.live).is_empty(), "lists leave it out now");
    }

    #[test]
    fn an_update_remembers_both_players_for_the_scrubber() {
        let shared = Shared::new(None);
        let mut match_view = view("t-names", 1_000, MatchState::InProgress);
        match_view.local_username = "Qbridgelocal".to_string();
        match_view.opponent = Some("Qbridgeopp".to_string());
        apply(
            &shared,
            TrackerEvent::MatchUpdated {
                view: match_view,
                revision: 1,
                cause: None,
                manual: false,
                platform: store::Platform::Mtgo,
                opponent_name: None,
            },
        );
        let known = crate::scrub::known().expect("not busy");
        assert!(known.contains(&"Qbridgelocal".to_string()));
        assert!(known.contains(&"Qbridgeopp".to_string()));
    }
}
