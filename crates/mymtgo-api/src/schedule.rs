//! When the API worker runs each job (07 spec sections 7 and 8, survey
//! section 14, 0.x `MtgoManager::schedule` and `MtgoMatchObserver`).
//!
//! Like Laravel's scheduler with its `->everyThirtySeconds()->skip(...)`
//! filters, but as plain data: `Schedule::due` says which jobs to run now,
//! from when each last ran and the gate's flags. The worker runs them and
//! records each run.

use std::collections::{BTreeMap, BTreeSet};

use crate::attest::KnownAccount;
use crate::gate::{GateDecision, GateState, RequestClass, gate};

/// Match reports ship (ruling 3, archetypes spec section 13): #9 gives
/// matches their player archetype (`match_archetypes`), and every eligible
/// match without a `match_report` outbox row is reported.
pub const MATCH_REPORTS_LIVE: bool = true;

/// Card stats ship (ruling 3, cards spec section 11). #6 (cards) gives a
/// live game's cards their oracle ids
/// (`a_live_game_payload_has_oracle_ids_once_cards_resolve`), the payload
/// matches 0.x's for the same live game with the allowlist
/// (`the_card_stats_payload_matches_the_0x_golden`), nothing 0.x queued is
/// queued again (`nothing_0x_shipped_is_enqueued_again`), #9 gives matches
/// their player archetype, the payload sends only the rows 0.x had (#295)
/// and games without a player archetype no longer fill the batch (F23).
pub const CARD_STATS_LIVE: bool = true;

/// A completed match asks for a sync this long after: the store's 5 minute
/// send hold (`store::api_reads::SEND_HOLD_MS`) plus 30 seconds. A match is
/// synced only once its hold is over, so an earlier run would skip the very
/// match that asked for it and leave it to the 30 minute run (0.x waited 30
/// seconds and had no hold).
pub const SYNC_AFTER_COMPLETION_MS: i64 = 330_000;

const SECOND: i64 = 1_000;
const MINUTE: i64 = 60 * SECOND;

/// One piece of the worker's work.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Job {
    /// Return claims of a dead worker to `pending` (local only).
    Reap,
    EnqueueObservations,
    ShipObservations,
    EnqueueReports,
    ShipReports,
    EnqueueCardStats,
    ShipCardStats,
    /// Ask the API for real tournament names.
    TournamentNames,
    /// A cross-device sync run.
    Sync,
    /// Attest the MTGO logins the API has not confirmed.
    Attest,
}

/// Every job, in the order a tick runs them: the reaper first, so a dead
/// claim is shipped in the same tick; each enqueue before its ship.
pub const JOBS: [Job; 10] = [
    Job::Reap,
    Job::EnqueueObservations,
    Job::ShipObservations,
    Job::EnqueueReports,
    Job::ShipReports,
    Job::EnqueueCardStats,
    Job::ShipCardStats,
    Job::TournamentNames,
    Job::Sync,
    Job::Attest,
];

impl Job {
    /// How often the job runs; `None` for attestation, which only follows
    /// an event (the API allows 10 an hour).
    pub fn interval_ms(self) -> Option<i64> {
        match self {
            Job::EnqueueObservations | Job::ShipObservations | Job::ShipCardStats => {
                Some(30 * SECOND)
            }
            Job::EnqueueReports | Job::ShipReports | Job::EnqueueCardStats | Job::Reap => {
                Some(MINUTE)
            }
            Job::TournamentNames | Job::Sync => Some(30 * MINUTE),
            Job::Attest => None,
        }
    }

    /// The gate class of the job's requests; `None` for local work.
    fn class(self) -> Option<RequestClass> {
        match self {
            Job::Reap => None,
            Job::Sync | Job::Attest => Some(RequestClass::Sync),
            _ => Some(RequestClass::Community),
        }
    }
}

/// The waits before a run that could not reach MyMTGO (`ApiError::
/// is_transient`: at launch or wake the network is often not up yet) is
/// tried again: 1, 2, 4, then 8 minutes. After those the job's normal wait
/// applies again.
pub const TRANSIENT_RETRIES_MS: [i64; 4] = [MINUTE, 2 * MINUTE, 4 * MINUTE, 8 * MINUTE];

/// How many runs in a row could not reach MyMTGO: the retry rule the sync
/// and the archetype definitions check share.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TransientRetry {
    failures: u32,
}

impl TransientRetry {
    /// A run ended; `transient` when it could not reach MyMTGO. Any other
    /// end (done, or a failure the server answered) starts the count over.
    pub fn ended(&mut self, transient: bool) {
        self.failures = if transient {
            self.failures.saturating_add(1)
        } else {
            0
        };
    }

    /// The wait before the next try while retries are left; `None` for the
    /// job's normal wait.
    pub fn wait_ms(&self) -> Option<i64> {
        let index = usize::try_from(self.failures).ok()?.checked_sub(1)?;
        TRANSIENT_RETRIES_MS.get(index).copied()
    }

    /// Whether a run that cannot reach MyMTGO now is shown as a failure:
    /// every retry was used.
    pub fn retries_used(&self) -> bool {
        usize::try_from(self.failures).unwrap_or(usize::MAX) >= TRANSIENT_RETRIES_MS.len()
    }
}

/// The settings key holding when the long-interval jobs last ran (#504).
pub const LAST_RUNS_KEY: &str = "api_job_last_runs";

impl Job {
    /// The name a job's last run is stored under, for the jobs whose last
    /// run outlives a restart (#504): those every 30 minutes, so a restart
    /// does not sync or ask tournament names again at once. The 30 s and
    /// 1 min jobs only send what is queued, so they run at start as before.
    pub fn persisted_name(self) -> Option<&'static str> {
        match self {
            Job::Sync => Some("sync"),
            Job::TournamentNames => Some("tournament_names"),
            _ => None,
        }
    }
}

/// What the worker remembers between ticks: when each job last ran and what
/// asked for one out of turn. Kept in memory, but for the jobs with a
/// `Job::persisted_name`, whose last runs are stored (`persisted_row`) and
/// read back at start (`restore`).
#[derive(Debug, Clone, Default)]
pub struct LastRuns {
    ran: BTreeMap<Job, i64>,
    /// A sync was asked for, due from this time (a completed match, "Sync
    /// now").
    sync_at: Option<i64>,
    /// A tournament row appeared: ask its name now.
    names_asked: bool,
    /// A new MTGO login was learned: attest now.
    attest_asked: bool,
    /// "Submit pending": queue and ship the match reports now.
    reports_asked: bool,
    /// The `(login id, username)` pairs seen so far; `None` before the
    /// first look.
    seen_logins: Option<BTreeSet<(i64, String)>>,
    /// Syncs in a row that could not reach MyMTGO.
    sync_retry: TransientRetry,
}

impl LastRuns {
    /// The job ran at `now_ms`; what asked for it is answered.
    pub fn record(&mut self, job: Job, now_ms: i64) {
        self.ran.insert(job, now_ms);
        match job {
            Job::Sync => self.sync_at = None,
            Job::TournamentNames => self.names_asked = false,
            Job::Attest => self.attest_asked = false,
            Job::ShipReports => self.reports_asked = false,
            _ => {}
        }
    }

    /// A sync ran at `now_ms`; `transient` when it could not reach MyMTGO,
    /// so it is tried again after `TRANSIENT_RETRIES_MS`. An asked run that
    /// could not reach MyMTGO was shown at once and leaves the retries alone.
    pub fn record_sync(&mut self, now_ms: i64, transient: bool) {
        let asked = self.sync_asked(now_ms);
        self.record(Job::Sync, now_ms);
        if !(asked && transient) {
            self.sync_retry.ended(transient);
        }
    }

    /// Whether a sync at `now_ms` stores a failure to reach MyMTGO for the
    /// settings card: it was asked for ("Sync now", a completed match), or
    /// the retries were used.
    pub fn sync_reports_unreachable(&self, now_ms: i64) -> bool {
        self.sync_asked(now_ms) || self.sync_retry.retries_used()
    }

    fn sync_asked(&self, now_ms: i64) -> bool {
        self.sync_at.is_some_and(|at| at <= now_ms)
    }

    /// When the job last ran, if it has.
    pub fn last_run(&self, job: Job) -> Option<i64> {
        self.ran.get(&job).copied()
    }

    /// The settings row (`LAST_RUNS_KEY`) of the persisted jobs' last runs,
    /// a JSON object such as `{"sync":1800000000000}`. A sync whose retry
    /// waits is left out, so a restart syncs at start instead of waiting
    /// out the normal interval.
    pub fn persisted_row(&self) -> (String, String) {
        let mut object = serde_json::Map::new();
        for job in JOBS {
            if job == Job::Sync && self.sync_retry.wait_ms().is_some() {
                continue;
            }
            if let (Some(name), Some(ran)) = (job.persisted_name(), self.last_run(job)) {
                object.insert(name.to_string(), serde_json::Value::from(ran));
            }
        }
        (
            LAST_RUNS_KEY.to_string(),
            serde_json::Value::Object(object).to_string(),
        )
    }

    /// Read back a `persisted_row` value at start. A run after `now_ms` (a
    /// clock set back) is left out, so the job runs; anything unreadable is
    /// ignored.
    pub fn restore(&mut self, value: &str, now_ms: i64) {
        let Ok(serde_json::Value::Object(object)) = serde_json::from_str(value) else {
            return;
        };
        for job in JOBS {
            let Some(name) = job.persisted_name() else {
                continue;
            };
            if let Some(ran) = object.get(name).and_then(serde_json::Value::as_i64)
                && ran <= now_ms
            {
                self.ran.insert(job, ran);
            }
        }
    }

    /// A match completed: on a linked, online device a sync follows once its
    /// send hold is over (`SYNC_AFTER_COMPLETION_MS`). Completions close
    /// together fold into one run, moved to the last one's time so it carries
    /// every match. A sync already due ("Sync now") is left as it is.
    pub fn match_completed(&mut self, now_ms: i64, gate_state: &GateState) {
        if gate(RequestClass::Sync, gate_state) != GateDecision::Send {
            return;
        }
        if self.sync_at.is_some_and(|at| at <= now_ms) {
            return;
        }
        self.sync_at = Some(now_ms + SYNC_AFTER_COMPLETION_MS);
    }

    /// "Sync now": a sync is due at once.
    pub fn sync_now(&mut self, now_ms: i64) {
        let at = match self.sync_at {
            Some(at) => at.min(now_ms),
            None => now_ms,
        };
        self.sync_at = Some(at);
    }

    /// "Submit pending" (0.x `RunSubmitMatchesController`): the match reports
    /// are queued and shipped at once.
    pub fn submit_reports(&mut self) {
        self.reports_asked = true;
    }

    /// A tournament row appeared: its name is asked at once.
    pub fn tournament_added(&mut self) {
        self.names_asked = true;
    }

    /// The local accounts as they are now. A login id with a username not
    /// seen before asks for an attestation; the first look only remembers.
    pub fn accounts_seen(&mut self, accounts: &[KnownAccount]) {
        let mut now = BTreeSet::new();
        for account in accounts {
            if let Some(login_id) = account.login_id {
                now.insert((login_id, account.username.clone()));
            }
        }
        if let Some(seen) = &self.seen_logins
            && now.iter().any(|pair| !seen.contains(pair))
        {
            self.attest_asked = true;
        }
        let mut seen = self.seen_logins.take().unwrap_or_default();
        seen.extend(now);
        self.seen_logins = Some(seen);
    }
}

/// Which jobs run. The two once dormant kinds have a switch each, the consts in
/// the app; tests turn them on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Schedule {
    pub match_reports_live: bool,
    pub card_stats_live: bool,
}

impl Default for Schedule {
    fn default() -> Self {
        Schedule {
            match_reports_live: MATCH_REPORTS_LIVE,
            card_stats_live: CARD_STATS_LIVE,
        }
    }
}

impl Schedule {
    /// The jobs to run now, in `JOBS` order. A job whose requests the gate
    /// would refuse is not due: community jobs wait while offline, sync and
    /// attestation also while signed out.
    pub fn due(&self, now_ms: i64, last: &LastRuns, gate_state: &GateState) -> Vec<Job> {
        let mut due = Vec::new();
        for job in JOBS {
            if self.switched_on(job) && gate_allows(job, gate_state) && is_time(job, now_ms, last) {
                due.push(job);
            }
        }
        due
    }

    fn switched_on(&self, job: Job) -> bool {
        match job {
            Job::EnqueueReports | Job::ShipReports => self.match_reports_live,
            Job::EnqueueCardStats | Job::ShipCardStats => self.card_stats_live,
            _ => true,
        }
    }
}

fn gate_allows(job: Job, gate_state: &GateState) -> bool {
    match job.class() {
        Some(class) => gate(class, gate_state) == GateDecision::Send,
        None => true,
    }
}

/// Never run, its interval passed, or asked for out of turn.
fn is_time(job: Job, now_ms: i64, last: &LastRuns) -> bool {
    let asked = match job {
        Job::Sync => last.sync_at.is_some_and(|at| at <= now_ms),
        Job::TournamentNames => last.names_asked,
        Job::Attest => last.attest_asked,
        Job::EnqueueReports | Job::ShipReports => last.reports_asked,
        _ => false,
    };
    if asked {
        return true;
    }
    let Some(mut interval) = job.interval_ms() else {
        return false;
    };
    if job == Job::Sync
        && let Some(retry) = last.sync_retry.wait_ms()
    {
        interval = retry;
    }
    match last.last_run(job) {
        Some(ran) => now_ms - ran >= interval,
        None => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::attest::KnownAccount;

    const T: i64 = 1_800_000_000_000;
    const SECOND: i64 = 1_000;
    const MINUTE: i64 = 60 * SECOND;

    fn online() -> GateState {
        GateState {
            offline: false,
            linked: true,
        }
    }

    fn offline() -> GateState {
        GateState {
            offline: true,
            ..online()
        }
    }

    fn unlinked() -> GateState {
        GateState {
            linked: false,
            ..online()
        }
    }

    /// Both switches on, so their jobs can be tested.
    fn live() -> Schedule {
        Schedule {
            match_reports_live: true,
            card_stats_live: true,
        }
    }

    const COMMUNITY: [Job; 7] = [
        Job::EnqueueObservations,
        Job::ShipObservations,
        Job::EnqueueReports,
        Job::ShipReports,
        Job::EnqueueCardStats,
        Job::ShipCardStats,
        Job::TournamentNames,
    ];

    // 0.x SchedulerGatingTest: "skips the shipping jobs while offline".
    #[test]
    fn skips_the_shipping_jobs_while_offline() {
        let due = live().due(T, &LastRuns::default(), &offline());
        for job in COMMUNITY {
            assert!(!due.contains(&job), "{job:?}");
        }
        assert!(!due.contains(&Job::Sync));
        assert_eq!(due, vec![Job::Reap], "only local work while offline");
    }

    // 0.x SchedulerGatingTest: "runs the shipping jobs while online".
    #[test]
    fn runs_the_shipping_jobs_while_online() {
        let due = live().due(T, &LastRuns::default(), &online());
        assert!(due.contains(&Job::ShipCardStats));
        for job in COMMUNITY {
            assert!(due.contains(&job), "{job:?}");
        }
    }

    #[test]
    fn jobs_run_in_a_fixed_order_reap_first() {
        let due = live().due(T, &LastRuns::default(), &online());
        assert_eq!(
            due,
            vec![
                Job::Reap,
                Job::EnqueueObservations,
                Job::ShipObservations,
                Job::EnqueueReports,
                Job::ShipReports,
                Job::EnqueueCardStats,
                Job::ShipCardStats,
                Job::TournamentNames,
                Job::Sync,
            ]
        );
    }

    #[test]
    fn a_device_that_never_signed_in_runs_every_community_job_but_no_sync() {
        let signed_out = GateState::default();
        assert_eq!(
            live().due(T, &LastRuns::default(), &signed_out),
            vec![
                Job::Reap,
                Job::EnqueueObservations,
                Job::ShipObservations,
                Job::EnqueueReports,
                Job::ShipReports,
                Job::EnqueueCardStats,
                Job::ShipCardStats,
                Job::TournamentNames,
            ]
        );
    }

    // Archetypes spec section 13 and cards spec section 11: both switches
    // are on as shipped, so the default schedule runs every job of both
    // kinds, as `live()` does.
    #[test]
    fn report_and_card_stat_jobs_run_when_live() {
        assert_eq!((MATCH_REPORTS_LIVE, CARD_STATS_LIVE), (true, true));
        let schedule = Schedule::default();
        assert_eq!(
            schedule,
            Schedule {
                match_reports_live: MATCH_REPORTS_LIVE,
                card_stats_live: CARD_STATS_LIVE,
            }
        );
        let due = schedule.due(T, &LastRuns::default(), &online());
        assert!(due.contains(&Job::EnqueueReports) && due.contains(&Job::ShipReports));
        assert!(due.contains(&Job::EnqueueCardStats) && due.contains(&Job::ShipCardStats));
        assert!(due.contains(&Job::ShipObservations));
        let due = live().due(T, &LastRuns::default(), &online());
        for job in [
            Job::EnqueueReports,
            Job::ShipReports,
            Job::EnqueueCardStats,
            Job::ShipCardStats,
        ] {
            assert!(due.contains(&job), "{job:?}");
        }
        // The gate still decides: nothing of either kind while offline.
        let due = live().due(T, &LastRuns::default(), &offline());
        assert!(!due.contains(&Job::ShipReports) && !due.contains(&Job::ShipCardStats));
    }

    // 0.x "Submit pending" (RunSubmitMatchesController): the reports run
    // now, once, and only while their switch is on and the gate is open.
    #[test]
    fn submitting_pending_reports_runs_them_now_once() {
        let mut runs = LastRuns::default();
        runs.record(Job::EnqueueReports, T);
        runs.record(Job::ShipReports, T);
        assert!(
            !live()
                .due(T + 1, &runs, &online())
                .contains(&Job::ShipReports)
        );
        runs.submit_reports();
        let due = live().due(T + 1, &runs, &online());
        assert!(due.contains(&Job::EnqueueReports) && due.contains(&Job::ShipReports));
        let switched_off = Schedule {
            match_reports_live: false,
            card_stats_live: false,
        };
        assert!(
            !switched_off
                .due(T + 1, &runs, &online())
                .contains(&Job::ShipReports)
        );
        assert!(
            !live()
                .due(T + 1, &runs, &offline())
                .contains(&Job::ShipReports)
        );
        runs.record(Job::EnqueueReports, T + 1);
        runs.record(Job::ShipReports, T + 1);
        assert!(
            !live()
                .due(T + 2, &runs, &online())
                .contains(&Job::ShipReports)
        );
    }

    #[test]
    fn each_job_waits_out_its_interval() {
        let intervals = [
            (Job::EnqueueObservations, 30 * SECOND),
            (Job::ShipObservations, 30 * SECOND),
            (Job::EnqueueReports, MINUTE),
            (Job::ShipReports, MINUTE),
            (Job::EnqueueCardStats, MINUTE),
            (Job::ShipCardStats, 30 * SECOND),
            (Job::Reap, MINUTE),
            (Job::TournamentNames, 30 * MINUTE),
            (Job::Sync, 30 * MINUTE),
        ];
        for (job, interval) in intervals {
            assert_eq!(job.interval_ms(), Some(interval), "{job:?}");
            let mut runs = LastRuns::default();
            runs.record(job, T);
            assert_eq!(runs.last_run(job), Some(T));
            let early = live().due(T + interval - 1, &runs, &online());
            assert!(!early.contains(&job), "{job:?} early");
            let due = live().due(T + interval, &runs, &online());
            assert!(due.contains(&job), "{job:?} on time");
        }
        assert_eq!(Job::Attest.interval_ms(), None, "never on a timer");
    }

    // 0.x SyncSchedulingTest: "schedules RunSyncJob every thirty minutes".
    #[test]
    fn schedules_the_sync_every_thirty_minutes() {
        let mut runs = LastRuns::default();
        assert!(
            Schedule::default()
                .due(T, &runs, &online())
                .contains(&Job::Sync)
        );
        runs.record(Job::Sync, T);
        let schedule = Schedule::default();
        assert!(
            !schedule
                .due(T + 29 * MINUTE, &runs, &online())
                .contains(&Job::Sync)
        );
        assert!(
            schedule
                .due(T + 30 * MINUTE, &runs, &online())
                .contains(&Job::Sync)
        );
    }

    // 0.x SyncSchedulingTest: "does not run the scheduled sync while the
    // device is not linked" and "runs the scheduled sync once the device is
    // linked".
    #[test]
    fn the_scheduled_sync_runs_only_once_linked() {
        let runs = LastRuns::default();
        let schedule = Schedule::default();
        assert!(!schedule.due(T, &runs, &unlinked()).contains(&Job::Sync));
        assert!(schedule.due(T, &runs, &online()).contains(&Job::Sync));
    }

    // 0.x SyncSchedulingTest: "does not run the scheduled sync while offline
    // mode is on, even when linked".
    #[test]
    fn no_scheduled_sync_while_offline_even_when_linked() {
        let mut runs = LastRuns::default();
        runs.sync_now(T);
        assert!(
            !Schedule::default()
                .due(T, &runs, &offline())
                .contains(&Job::Sync)
        );
    }

    // 0.x SyncSchedulingTest: "schedules a sync when a match completes on a
    // linked, online device". v1 waits out the 5 minute send hold first, so
    // the run includes the match that asked for it.
    #[test]
    fn a_completed_match_schedules_a_sync_after_the_send_hold() {
        let schedule = Schedule::default();
        let mut runs = LastRuns::default();
        runs.record(Job::Sync, T);
        let completed = T + MINUTE;
        runs.match_completed(completed, &online());
        let at = completed + SYNC_AFTER_COMPLETION_MS;
        assert_eq!(SYNC_AFTER_COMPLETION_MS, 5 * MINUTE + 30 * SECOND);
        assert!(!schedule.due(at - 1, &runs, &online()).contains(&Job::Sync));
        assert!(schedule.due(at, &runs, &online()).contains(&Job::Sync));
        runs.record(Job::Sync, at);
        assert!(
            !schedule
                .due(at + MINUTE, &runs, &online())
                .contains(&Job::Sync),
            "the run took the request"
        );
    }

    // Completions close together fold into one run, due once the last one's
    // send hold is over, so that run carries every match.
    #[test]
    fn completions_close_together_fold_into_one_run_after_the_last() {
        let schedule = Schedule::default();
        let mut runs = LastRuns::default();
        runs.record(Job::Sync, T);
        runs.match_completed(T + MINUTE, &online());
        let last = T + 4 * MINUTE;
        runs.match_completed(last, &online());
        let first = T + MINUTE + SYNC_AFTER_COMPLETION_MS;
        assert!(!schedule.due(first, &runs, &online()).contains(&Job::Sync));
        let at = last + SYNC_AFTER_COMPLETION_MS;
        assert!(schedule.due(at, &runs, &online()).contains(&Job::Sync));
    }

    #[test]
    fn a_completion_does_not_hold_back_a_sync_now() {
        let mut runs = LastRuns::default();
        runs.record(Job::Sync, T);
        runs.sync_now(T + SECOND);
        runs.match_completed(T + SECOND, &online());
        assert!(
            Schedule::default()
                .due(T + SECOND, &runs, &online())
                .contains(&Job::Sync)
        );
    }

    // 0.x SyncSchedulingTest: "does not schedule a completion sync while
    // unlinked".
    #[test]
    fn no_completion_sync_while_unlinked_or_offline() {
        let schedule = Schedule::default();
        for gate in [unlinked(), offline()] {
            let mut runs = LastRuns::default();
            runs.record(Job::Sync, T);
            runs.match_completed(T, &gate);
            let later = T + SYNC_AFTER_COMPLETION_MS;
            assert!(
                !schedule.due(later, &runs, &online()).contains(&Job::Sync),
                "{gate:?}"
            );
        }
    }

    #[test]
    fn sync_now_is_due_at_once() {
        let mut runs = LastRuns::default();
        runs.record(Job::Sync, T);
        runs.sync_now(T + SECOND);
        assert!(
            Schedule::default()
                .due(T + SECOND, &runs, &online())
                .contains(&Job::Sync)
        );
    }

    #[test]
    fn a_new_tournament_asks_for_names_at_once() {
        let mut runs = LastRuns::default();
        runs.record(Job::TournamentNames, T);
        assert!(
            !live()
                .due(T + SECOND, &runs, &online())
                .contains(&Job::TournamentNames)
        );
        runs.tournament_added();
        assert!(
            live()
                .due(T + SECOND, &runs, &online())
                .contains(&Job::TournamentNames)
        );
        runs.record(Job::TournamentNames, T + SECOND);
        assert!(
            !live()
                .due(T + 2 * SECOND, &runs, &online())
                .contains(&Job::TournamentNames)
        );
        runs.tournament_added();
        assert!(
            !live()
                .due(T + 2 * SECOND, &runs, &offline())
                .contains(&Job::TournamentNames),
            "community: not offline"
        );
    }

    fn account(username: &str, login_id: Option<i64>) -> KnownAccount {
        KnownAccount {
            username: username.to_string(),
            login_id,
        }
    }

    // Attestation: when a new (login id, username) pair is learned, never on
    // a timer (the API allows 10 an hour).
    #[test]
    fn a_newly_learned_login_asks_for_an_attestation() {
        let schedule = Schedule::default();
        let mut runs = LastRuns::default();
        runs.record(Job::Sync, T);
        let attest = |runs: &LastRuns, gate: &GateState| {
            schedule.due(T + SECOND, runs, gate).contains(&Job::Attest)
        };
        runs.accounts_seen(&[account("Qplayer", Some(7))]);
        assert!(!attest(&runs, &online()), "the first look is the baseline");
        runs.accounts_seen(&[account("Qplayer", Some(7)), account("Qalt", None)]);
        assert!(!attest(&runs, &online()), "no login id, nothing to attest");
        runs.accounts_seen(&[account("Qplayer", Some(7)), account("Qalt", Some(8))]);
        assert!(attest(&runs, &online()));
        assert!(!attest(&runs, &unlinked()), "attestation needs a session");
        assert!(!attest(&runs, &offline()));
        runs.record(Job::Attest, T + SECOND);
        assert!(!attest(&runs, &online()), "asked once");
        runs.accounts_seen(&[account("Qrenamed", Some(7)), account("Qalt", Some(8))]);
        assert!(attest(&runs, &online()), "a new username behind a login");
    }

    #[test]
    fn reaping_is_local_and_always_due() {
        let mut runs = LastRuns::default();
        for gate in [offline(), unlinked(), GateState::default()] {
            assert!(live().due(T, &runs, &gate).contains(&Job::Reap), "{gate:?}");
        }
        runs.record(Job::Reap, T);
        assert!(
            !live()
                .due(T + SECOND, &runs, &offline())
                .contains(&Job::Reap)
        );
    }

    // #504: the 30-minute jobs' last runs outlive a restart; the others run
    // at start as before.
    #[test]
    fn the_long_jobs_last_runs_round_trip_through_a_settings_row() {
        let mut runs = LastRuns::default();
        runs.record(Job::Sync, T);
        runs.record(Job::TournamentNames, T + 1);
        runs.record(Job::ShipObservations, T + 2);
        let (key, value) = runs.persisted_row();
        assert_eq!(key, LAST_RUNS_KEY);
        assert_eq!(
            value,
            r#"{"sync":1800000000000,"tournament_names":1800000000001}"#
        );
        let mut restored = LastRuns::default();
        restored.restore(&value, T + 60 * SECOND);
        assert_eq!(restored.last_run(Job::Sync), Some(T));
        assert_eq!(restored.last_run(Job::TournamentNames), Some(T + 1));
        assert_eq!(restored.last_run(Job::ShipObservations), None);
        let online = GateState {
            offline: false,
            linked: true,
        };
        let due = Schedule::default().due(T + 60 * SECOND, &restored, &online);
        assert!(!due.contains(&Job::Sync), "{due:?}");
        assert!(due.contains(&Job::ShipObservations), "{due:?}");
    }

    #[test]
    fn a_future_or_unreadable_last_run_is_ignored() {
        let mut runs = LastRuns::default();
        runs.restore(r#"{"sync":1800000000000}"#, T - 1);
        assert_eq!(runs.last_run(Job::Sync), None, "a clock set back");
        runs.restore("not json", T);
        runs.restore(r#"{"sync":"soon"}"#, T);
        assert_eq!(runs.last_run(Job::Sync), None);
    }

    #[test]
    fn a_transient_retry_waits_one_two_four_then_eight_minutes() {
        let mut retry = TransientRetry::default();
        assert_eq!(retry.wait_ms(), None, "nothing failed: the normal wait");
        let mut waits = Vec::new();
        let mut shown = Vec::new();
        for _ in 0..6 {
            shown.push(retry.retries_used());
            retry.ended(true);
            waits.push(retry.wait_ms());
        }
        assert_eq!(
            waits,
            vec![
                Some(MINUTE),
                Some(2 * MINUTE),
                Some(4 * MINUTE),
                Some(8 * MINUTE),
                None,
                None
            ]
        );
        assert_eq!(
            shown,
            vec![false, false, false, false, true, true],
            "shown once the 8 minute retry runs"
        );
        retry.ended(false);
        assert_eq!(
            retry,
            TransientRetry::default(),
            "any other end starts over"
        );
    }

    #[test]
    fn a_sync_that_could_not_connect_is_tried_again_with_backoff() {
        let schedule = Schedule::default();
        let mut runs = LastRuns::default();
        let mut at = T;
        for wait in [MINUTE, 2 * MINUTE, 4 * MINUTE, 8 * MINUTE, 30 * MINUTE] {
            assert!(!runs.sync_reports_unreachable(at) || wait == 30 * MINUTE);
            runs.record_sync(at, true);
            let early = schedule.due(at + wait - 1, &runs, &online());
            assert!(!early.contains(&Job::Sync), "{wait} early");
            at += wait;
            assert!(
                schedule.due(at, &runs, &online()).contains(&Job::Sync),
                "{wait} on time"
            );
        }
        assert!(runs.sync_reports_unreachable(at));
        runs.record_sync(at, false);
        assert!(!runs.sync_reports_unreachable(at));
        assert!(
            !schedule
                .due(at + 29 * MINUTE, &runs, &online())
                .contains(&Job::Sync),
            "a run that reached MyMTGO waits 30 minutes"
        );
        runs.record_sync(at, true);
        assert!(
            schedule
                .due(at + MINUTE, &runs, &online())
                .contains(&Job::Sync),
            "the count started over"
        );
    }

    // "Sync now" (or a completed match) asks for a run: one that cannot
    // reach MyMTGO is shown at once and leaves the launch retries alone.
    #[test]
    fn an_asked_sync_that_could_not_connect_is_shown_and_keeps_the_retries() {
        let schedule = Schedule::default();
        let mut runs = LastRuns::default();
        runs.record_sync(T, true);
        assert!(!runs.sync_reports_unreachable(T + SECOND));
        runs.sync_now(T + SECOND);
        assert!(runs.sync_reports_unreachable(T + SECOND), "asked: shown");
        for _ in 0..5 {
            runs.sync_now(T + SECOND);
            runs.record_sync(T + SECOND, true);
        }
        assert!(!runs.sync_reports_unreachable(T + 2 * SECOND));
        assert!(
            schedule
                .due(T + SECOND + MINUTE, &runs, &online())
                .contains(&Job::Sync),
            "the retry chain is still at its first wait"
        );
    }

    // A restart while a retry waits syncs at start rather than 30 minutes
    // after the failed run; once the retries are used the run is stored.
    #[test]
    fn a_waiting_retry_leaves_the_sync_out_of_the_stored_row() {
        let mut runs = LastRuns::default();
        runs.record(Job::TournamentNames, T);
        runs.record_sync(T, true);
        let (_, value) = runs.persisted_row();
        assert_eq!(value, r#"{"tournament_names":1800000000000}"#);
        let mut restored = LastRuns::default();
        restored.restore(&value, T + SECOND);
        assert!(
            Schedule::default()
                .due(T + SECOND, &restored, &online())
                .contains(&Job::Sync)
        );
        for _ in 0..4 {
            runs.record_sync(T + MINUTE, true);
        }
        let (_, value) = runs.persisted_row();
        assert_eq!(
            value,
            r#"{"sync":1800000060000,"tournament_names":1800000000000}"#
        );
    }
}
