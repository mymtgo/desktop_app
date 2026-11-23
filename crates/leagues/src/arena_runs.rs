//! Arena limited runs (Arena limited runs spec section 3).
//!
//! Arena has no league token: a limited entry is a course, and every
//! `EventGetCoursesV2` response is a snapshot of the account's whole current
//! course set. `fold_arena_runs` replays one account's snapshots and limited
//! matches in log order and returns a run per course, plus a run per group of
//! matches whose course never showed. State is recomputed from all the facts
//! on every fold, so the same inputs give the same runs whatever order they
//! arrived in, and a run key never changes once its course is known.
//!
//! Across devices (cross-device sync spec section 8) a course's run is also
//! fed by what other devices pushed of it: their newest snapshot of the
//! course (`PulledCourseRun`) and their matches in it (`PulledRunMatch`).
//! `fold_arena_runs_merged` takes both beside the local facts; the newest
//! snapshot by observed time is the run's record, so matches played on a
//! device MyMTGO never sees still count.

use std::collections::{HashMap, HashSet};

use arena_protocol::parse_event_name;
use limited::set_code_from_play_format;

use crate::fold::RunState;
use crate::limited_runs::LeagueKind;

/// The separator in a run name: `Premier Draft · FRA`.
const NAME_SEPARATOR: &str = " \u{b7} ";

/// The run key prefix of every Arena run.
const ARENA_KEY_PREFIX: &str = "arena:";

/// Whether a stored run is an Arena run: its key starts `arena:`.
pub fn is_arena_run_key(run_key: &str) -> bool {
    run_key.starts_with(ARENA_KEY_PREFIX)
}

/// The course id of an Arena run's key (`arena:<course id>`); `None` for an
/// orphan's key (`arena:<event name>:<first match token>`) or a key that is
/// not Arena's.
pub fn arena_course_id(run_key: &str) -> Option<&str> {
    let rest = run_key.strip_prefix(ARENA_KEY_PREFIX)?;
    (!rest.contains(':')).then_some(rest)
}

/// One `EventGetCoursesV2` response: the account's current limited courses
/// at that log place. Empty `courses` is an empty snapshot, which completes
/// every course until one is listed again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CourseSnapshot {
    pub file_id: i64,
    pub offset: i64,
    pub observed_at_ms: i64,
    pub courses: Vec<CourseState>,
}

/// One course in a snapshot. Decks and the pool are catalog ids
/// (`ARENA_ID_OFFSET` plus the grpId); the deck rows are `(card, quantity)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CourseState {
    pub course_id: String,
    pub event_name: String,
    /// Arena's `CurrentModule`: `CreateMatch` while playing, `Complete` or
    /// `ClaimPrize` once the entry is over.
    pub module: String,
    pub wins: u32,
    pub losses: u32,
    pub main: Vec<(u32, u32)>,
    pub side: Vec<(u32, u32)>,
    pub pool: Vec<u32>,
}

/// A limited Arena match as a fold input, at the log place of its first
/// stored Arena event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArenaRunMatch {
    pub token: String,
    pub event_name: String,
    pub format_code: String,
    pub file_id: i64,
    pub offset: i64,
    pub started_at_ms: i64,
    pub ended_at_ms: Option<i64>,
    /// The catalog ids of the local deck and sideboard of the match's first
    /// game, each once; empty when its game data is unknown. In limited the
    /// sideboard is the rest of the pool, so this is the pool (plus basic
    /// lands), which tells one entry of an event from the next.
    pub cards: Vec<u32>,
}

/// The course deck a match was played with: the deck of the latest
/// observation before the match, and that observation's log place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchDeck {
    pub token: String,
    pub file_id: i64,
    pub offset: i64,
    pub main: Vec<(u32, u32)>,
    pub side: Vec<(u32, u32)>,
}

/// A course's record as one snapshot stated it: the wins and losses Arena
/// counted, whether the course was over and when it was observed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CourseRecord {
    pub wins: u32,
    pub losses: u32,
    /// `Active` or `Complete`.
    pub state: RunState,
    pub observed_at_ms: i64,
}

/// Whether `candidate` is newer than `current`: later observed, and on a
/// tie of observed time the one later in a fixed order of its counts (more
/// matches, then more wins), so every device picks the same counts whatever
/// order the records arrive in.
pub fn newer_record(candidate: &CourseRecord, current: &CourseRecord) -> bool {
    record_order(candidate) > record_order(current)
}

fn record_order(record: &CourseRecord) -> (i64, u32, u32, u32) {
    (
        record.observed_at_ms,
        record.wins.saturating_add(record.losses),
        record.wins,
        record.losses,
    )
}

/// Two records of one course as one: the newer one's counts and time, and
/// complete when either is (a course never reopens). Commutative,
/// associative and idempotent, so devices merging in any order agree.
pub fn join_records(a: &CourseRecord, b: &CourseRecord) -> CourseRecord {
    let mut joined = if newer_record(b, a) {
        b.clone()
    } else {
        a.clone()
    };
    if a.state == RunState::Complete || b.state == RunState::Complete {
        joined.state = RunState::Complete;
    }
    joined
}

/// A course run as another device last pushed it: its event and the newest
/// snapshot of the course that device knew.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PulledCourseRun {
    pub course_id: String,
    pub event_name: String,
    pub record: CourseRecord,
}

/// A limited match pulled from another device, in the course its bundle's
/// run names. It has no place in this device's log, so it joins its
/// course's run after the local matches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PulledRunMatch {
    pub token: String,
    pub course_id: String,
    pub started_at_ms: i64,
    pub ended_at_ms: Option<i64>,
}

/// One Arena limited run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArenaRun {
    /// `arena:<course id>`, or `arena:<event name>:<first match token>` for
    /// matches whose course never showed.
    pub run_key: String,
    pub course_id: Option<String>,
    pub event_name: String,
    pub name: String,
    pub kind: LeagueKind,
    pub format_code: String,
    pub set_code: Option<String>,
    /// `Active` or `Complete`.
    pub state: RunState,
    /// The first match's start, or the first observation when no match has
    /// joined.
    pub started_at_ms: i64,
    /// The observation that completed the run (its module, or the first
    /// snapshot not listing it), or the last match's end when that is later.
    pub completed_at_ms: Option<i64>,
    /// The local matches in log order, then the pulled ones by start.
    pub match_tokens: Vec<String>,
    /// One per match that an observation preceded, in log order.
    pub decks: Vec<MatchDeck>,
    /// The newest snapshot of the course, local or pulled; `None` for an
    /// orphan.
    pub record: Option<CourseRecord>,
}

/// A snapshot or a match, in log order. A snapshot and a match never share
/// a log place; the snapshot sorts first all the same.
#[derive(Debug, Clone, Copy)]
enum Input<'a> {
    Snapshot(&'a CourseSnapshot),
    Match(&'a ArenaRunMatch),
}

impl Input<'_> {
    fn place(self) -> (i64, i64, u8) {
        match self {
            Input::Snapshot(snapshot) => (snapshot.file_id, snapshot.offset, 0),
            Input::Match(played) => (played.file_id, played.offset, 1),
        }
    }
}

/// The deck of a course's latest observation and where it was seen.
#[derive(Debug, Clone)]
struct ObservedDeck {
    file_id: i64,
    offset: i64,
    main: Vec<(u32, u32)>,
    side: Vec<(u32, u32)>,
}

/// A run being folded.
#[derive(Debug, Clone)]
struct Building {
    run: ArenaRun,
    deck: Option<ObservedDeck>,
    /// The end of the latest match that ended, for the completion time.
    last_match_end_ms: Option<i64>,
}

/// Fold one account's course snapshots and limited Arena matches into runs,
/// ordered by start and then key. Inputs may arrive in any order: the fold
/// sorts them by log place itself. This device's facts only:
/// `fold_arena_runs_merged` with nothing pulled.
pub fn fold_arena_runs(snapshots: &[CourseSnapshot], matches: &[ArenaRunMatch]) -> Vec<ArenaRun> {
    fold_arena_runs_merged(snapshots, matches, &[], &[])
}

/// `fold_arena_runs` with what other devices pushed: each pulled course's
/// record is joined with the local one (`join_records`: the newer counts,
/// complete when either is); a course known only from a pull
/// still has its run; pulled matches join the run of their course, and a
/// pulled match whose course has no run here is left out.
pub fn fold_arena_runs_merged(
    snapshots: &[CourseSnapshot],
    matches: &[ArenaRunMatch],
    pulled_runs: &[PulledCourseRun],
    pulled_matches: &[PulledRunMatch],
) -> Vec<ArenaRun> {
    let mut inputs: Vec<Input> = Vec::with_capacity(snapshots.len() + matches.len());
    inputs.extend(snapshots.iter().map(Input::Snapshot));
    inputs.extend(matches.iter().map(Input::Match));
    inputs.sort_by_key(|input| input.place());

    let mut fold = Fold::default();
    for input in inputs {
        match input {
            Input::Snapshot(snapshot) => fold.observe(snapshot),
            Input::Match(played) => fold.play(played),
        }
    }
    fold.merge_pulled(pulled_runs, pulled_matches);
    fold.finish()
}

#[derive(Debug, Default)]
struct Fold {
    runs: Vec<Building>,
    /// Run index by course id.
    by_course: HashMap<String, usize>,
    /// The latest stated pool of each course's run, by run index.
    pools: HashMap<usize, Vec<u32>>,
    /// The course of each event in the latest snapshot.
    current: HashMap<String, String>,
    /// Matches of an event with no open course, waiting for one.
    waiting: HashMap<String, Vec<Waiting>>,
}

/// A match waiting for its course.
#[derive(Debug, Clone)]
struct Waiting {
    played: ArenaRunMatch,
    /// The event's course that had completed when the match was played: the
    /// match belongs to a new entry, so it never joins that course while the
    /// course stays complete.
    after_course: Option<String>,
}

impl Fold {
    fn observe(&mut self, snapshot: &CourseSnapshot) {
        let mut current: HashMap<String, String> = HashMap::new();
        let mut listed: Vec<usize> = Vec::new();
        for course in &snapshot.courses {
            let index = self.open_course(course, snapshot.observed_at_ms);
            if listed.contains(&index) {
                continue;
            }
            listed.push(index);
            if !course.pool.is_empty() {
                self.pools.insert(index, course.pool.clone());
            }

            let building = &mut self.runs[index];
            building.deck = Some(ObservedDeck {
                file_id: snapshot.file_id,
                offset: snapshot.offset,
                main: course.main.clone(),
                side: course.side.clone(),
            });
            if module_completes(&course.module) {
                complete(building, snapshot.observed_at_ms);
            } else {
                building.run.state = RunState::Active;
                building.run.completed_at_ms = None;
            }
            building.run.record = Some(CourseRecord {
                wins: course.wins,
                losses: course.losses,
                state: building.run.state,
                observed_at_ms: snapshot.observed_at_ms,
            });

            // Arena allows one open entry per event, but a snapshot may still
            // list a finished entry beside it, in course id order. The open
            // one is current; a finished one only when it is all there is.
            let replaces = match current.get(&course.event_name) {
                None => true,
                Some(listed) => {
                    self.runs[self.by_course[listed]].run.state == RunState::Complete
                        && !module_completes(&course.module)
                }
            };
            if replaces {
                current.insert(course.event_name.clone(), course.course_id.clone());
            }
        }

        for (event_name, course_id) in &current {
            self.join_waiting(event_name, course_id);
        }

        for (index, building) in self.runs.iter_mut().enumerate() {
            if building.run.course_id.is_some() && !listed.contains(&index) {
                complete(building, snapshot.observed_at_ms);
                // Leaving the set says the course is over, nothing of its
                // counts: they and their time stay the last listing's.
                if let Some(record) = &mut building.run.record {
                    record.state = RunState::Complete;
                }
            }
        }
        self.current = current;
    }

    /// The run of a course, opened at its first observation.
    fn open_course(&mut self, course: &CourseState, observed_at_ms: i64) -> usize {
        if let Some(index) = self.by_course.get(&course.course_id) {
            return *index;
        }
        let format_code = parse_event_name(&course.event_name)
            .map(|event| event.format_code)
            .unwrap_or_default();
        let run = new_run(
            format!("{ARENA_KEY_PREFIX}{}", course.course_id),
            Some(course.course_id.clone()),
            &course.event_name,
            format_code,
            observed_at_ms,
        );
        self.runs.push(Building {
            run,
            deck: None,
            last_match_end_ms: None,
        });
        let index = self.runs.len() - 1;
        self.by_course.insert(course.course_id.clone(), index);
        index
    }

    /// Waiting matches of an event join its current course, except those
    /// played after that course had completed while it is still complete.
    fn join_waiting(&mut self, event_name: &str, course_id: &str) {
        let Some(waited) = self.waiting.remove(event_name) else {
            return;
        };
        let index = self.by_course[course_id];
        let course_complete = self.runs[index].run.state == RunState::Complete;
        let mut still_waiting: Vec<Waiting> = Vec::new();
        for waiting in waited {
            let blocked = (course_complete && waiting.after_course.as_deref() == Some(course_id))
                || !self.pool_fits(index, &waiting.played);
            if blocked {
                still_waiting.push(waiting);
            } else {
                join(&mut self.runs[index], &waiting.played, None);
            }
        }
        if !still_waiting.is_empty() {
            self.waiting.insert(event_name.to_string(), still_waiting);
        }
    }

    /// Whether a match can belong to a run's course: false only when both
    /// the course's pool and the match's cards are known and differ, as they
    /// do between two entries of one event.
    fn pool_fits(&self, index: usize, played: &ArenaRunMatch) -> bool {
        self.pools
            .get(&index)
            .is_none_or(|pool| same_pool(pool, &played.cards) != Some(false))
    }

    fn play(&mut self, played: &ArenaRunMatch) {
        let current = self
            .current
            .get(&played.event_name)
            .map(|course_id| (course_id.clone(), self.by_course[course_id]));
        let after_course = match current {
            Some((course_id, index)) => {
                if self.runs[index].run.state != RunState::Complete && self.pool_fits(index, played)
                {
                    let deck = self.runs[index].deck.clone();
                    join(&mut self.runs[index], played, deck);
                    return;
                }
                Some(course_id)
            }
            None => None,
        };
        self.waiting
            .entry(played.event_name.clone())
            .or_default()
            .push(Waiting {
                played: played.clone(),
                after_course,
            });
    }

    /// What other devices pushed: each pulled record is joined with the
    /// run's own (`join_records`), a course with no run here gets one, and
    /// pulled matches join their course's run by start.
    fn merge_pulled(&mut self, pulled_runs: &[PulledCourseRun], pulled_matches: &[PulledRunMatch]) {
        let mut pulled_runs: Vec<&PulledCourseRun> = pulled_runs.iter().collect();
        pulled_runs.sort_by(|a, b| a.course_id.cmp(&b.course_id));
        for pulled in pulled_runs {
            let index = match self.by_course.get(&pulled.course_id) {
                Some(index) => *index,
                None => self.open_pulled_course(pulled),
            };
            let run = &mut self.runs[index].run;
            let joined = match &run.record {
                Some(local) => join_records(local, &pulled.record),
                None => pulled.record.clone(),
            };
            if joined.state == RunState::Complete && run.state != RunState::Complete {
                run.state = RunState::Complete;
                run.completed_at_ms = Some(pulled.record.observed_at_ms);
            }
            run.record = Some(joined);
        }

        let mut pulled_matches: Vec<&PulledRunMatch> = pulled_matches.iter().collect();
        pulled_matches.sort_by(|a, b| {
            a.started_at_ms
                .cmp(&b.started_at_ms)
                .then_with(|| a.token.cmp(&b.token))
        });
        for played in pulled_matches {
            let Some(index) = self.by_course.get(&played.course_id).copied() else {
                continue;
            };
            let building = &mut self.runs[index];
            if building.run.match_tokens.contains(&played.token) {
                continue;
            }
            if building.run.match_tokens.is_empty()
                || played.started_at_ms < building.run.started_at_ms
            {
                building.run.started_at_ms = played.started_at_ms;
            }
            building.run.match_tokens.push(played.token.clone());
            if let Some(ended) = played.ended_at_ms
                && building
                    .last_match_end_ms
                    .is_none_or(|latest| ended > latest)
            {
                building.last_match_end_ms = Some(ended);
            }
        }
    }

    /// The run of a course known only from another device, starting at its
    /// pulled snapshot.
    fn open_pulled_course(&mut self, pulled: &PulledCourseRun) -> usize {
        let format_code = parse_event_name(&pulled.event_name)
            .map(|event| event.format_code)
            .unwrap_or_default();
        let run = new_run(
            format!("{ARENA_KEY_PREFIX}{}", pulled.course_id),
            Some(pulled.course_id.clone()),
            &pulled.event_name,
            format_code,
            pulled.record.observed_at_ms,
        );
        self.runs.push(Building {
            run,
            deck: None,
            last_match_end_ms: None,
        });
        let index = self.runs.len() - 1;
        self.by_course.insert(pulled.course_id.clone(), index);
        index
    }

    fn finish(mut self) -> Vec<ArenaRun> {
        let mut waiting: Vec<(String, Vec<Waiting>)> = self.waiting.drain().collect();
        waiting.sort_by(|a, b| a.0.cmp(&b.0));
        for (event_name, waited) in waiting {
            let played: Vec<ArenaRunMatch> =
                waited.into_iter().map(|waiting| waiting.played).collect();
            for entry in split_by_pool(played) {
                let first = &entry[0];
                let run = new_run(
                    format!("{ARENA_KEY_PREFIX}{event_name}:{}", first.token),
                    None,
                    &event_name,
                    first.format_code.clone(),
                    first.started_at_ms,
                );
                let mut building = Building {
                    run,
                    deck: None,
                    last_match_end_ms: None,
                };
                for played in &entry {
                    join(&mut building, played, None);
                }
                self.runs.push(building);
            }
        }

        let mut runs: Vec<ArenaRun> = Vec::with_capacity(self.runs.len());
        for building in self.runs {
            let mut run = building.run;
            if run.state == RunState::Complete
                && let Some(ended) = building.last_match_end_ms
                && run.completed_at_ms.is_some_and(|at| ended > at)
            {
                run.completed_at_ms = Some(ended);
            }
            runs.push(run);
        }
        runs.sort_by(|a, b| {
            a.started_at_ms
                .cmp(&b.started_at_ms)
                .then_with(|| a.run_key.cmp(&b.run_key))
        });
        runs
    }
}

fn new_run(
    run_key: String,
    course_id: Option<String>,
    event_name: &str,
    format_code: String,
    started_at_ms: i64,
) -> ArenaRun {
    ArenaRun {
        run_key,
        course_id,
        event_name: event_name.to_string(),
        name: arena_run_name(event_name),
        kind: LeagueKind::from_format_code(Some(&format_code)),
        set_code: set_code_from_play_format(&format_code),
        format_code,
        state: RunState::Active,
        started_at_ms,
        completed_at_ms: None,
        match_tokens: Vec::new(),
        decks: Vec::new(),
        record: None,
    }
}

/// A match joins a run; `deck` is the course deck observed before it, if any.
fn join(building: &mut Building, played: &ArenaRunMatch, deck: Option<ObservedDeck>) {
    if building.run.match_tokens.is_empty() {
        building.run.started_at_ms = played.started_at_ms;
    }
    building.run.match_tokens.push(played.token.clone());
    if let Some(deck) = deck {
        building.run.decks.push(MatchDeck {
            token: played.token.clone(),
            file_id: deck.file_id,
            offset: deck.offset,
            main: deck.main,
            side: deck.side,
        });
    }
    if let Some(ended) = played.ended_at_ms
        && building
            .last_match_end_ms
            .is_none_or(|latest| ended > latest)
    {
        building.last_match_end_ms = Some(ended);
    }
}

/// An observation completes a run; a run already complete keeps the time of
/// the observation that completed it.
fn complete(building: &mut Building, observed_at_ms: i64) {
    if building.run.state != RunState::Complete {
        building.run.state = RunState::Complete;
        building.run.completed_at_ms = Some(observed_at_ms);
    }
}

/// Course-less matches of one event, in log order, cut into entries: a
/// match starts a new entry when its cards are known and are not the pool of
/// the entry's latest match with known cards. A match with unknown cards
/// stays in the entry before it.
fn split_by_pool(played: Vec<ArenaRunMatch>) -> Vec<Vec<ArenaRunMatch>> {
    let mut entries: Vec<Vec<ArenaRunMatch>> = Vec::new();
    let mut entry_pool: Option<Vec<u32>> = None;
    for one in played {
        let new_pool = entry_pool
            .as_ref()
            .is_some_and(|pool| same_pool(pool, &one.cards) == Some(false));
        if !one.cards.is_empty() {
            entry_pool = Some(one.cards.clone());
        }
        match entries.last_mut() {
            Some(entry) if !new_pool => entry.push(one),
            _ => entries.push(vec![one]),
        }
    }
    entries
}

/// Whether two card lists come from one limited pool: at least nine in ten
/// of the shorter list's distinct cards are in the other. One entry's deck
/// and sideboard is its whole pool, so the shorter list is inside the other
/// (1.0 in the `sealed-direct-fast-login` fixture, game 1's cards against
/// the course's `CardPool`). Two pools of one set share commons: random
/// six-pack sealed pools share at most about 0.6 of their cards, and a
/// 23-card deck list against another pool at most about 0.85 (simulated,
/// basic lands left out, which callers should do: every list has them).
/// `None` when either list is empty (unknown).
pub fn same_pool(a: &[u32], b: &[u32]) -> Option<bool> {
    if a.is_empty() || b.is_empty() {
        return None;
    }
    let a: HashSet<u32> = a.iter().copied().collect();
    let b: HashSet<u32> = b.iter().copied().collect();
    let shared = a.intersection(&b).count();
    Some(shared * 10 >= a.len().min(b.len()) * 9)
}

fn module_completes(module: &str) -> bool {
    module == "Complete" || module == "ClaimPrize"
}

/// The name of an Arena run, from its event name: `Arena Direct · EOE
/// Sealed`, `Premier Draft · FRA`, `Quick Draft · FDN`, `Traditional Draft ·
/// FRA`, `Sealed · FDN`, `Traditional Sealed · FRA`; any other limited event
/// is its set code and `Draft` or `Sealed` (`FDN Draft`). An event that is
/// not limited keeps its name.
pub fn arena_run_name(event_name: &str) -> String {
    let Some(event) = parse_event_name(event_name).filter(|event| event.limited) else {
        return event_name.to_string();
    };
    let set = set_code_from_play_format(&event.format_code).unwrap_or_default();
    let kind = if event.format_code.starts_with('S') {
        "Sealed"
    } else {
        "Draft"
    };
    let queue = event_name.split('_').next().unwrap_or_default();
    let (first, second) = match queue {
        "ArenaDirect" => ("Arena Direct".to_string(), format!("{set} {kind}")),
        "PremierDraft" => ("Premier Draft".to_string(), set),
        "QuickDraft" => ("Quick Draft".to_string(), set),
        "TradDraft" => ("Traditional Draft".to_string(), set),
        "Sealed" => ("Sealed".to_string(), set),
        "TradSealed" => ("Traditional Sealed".to_string(), set),
        _ => return format!("{set} {kind}"),
    };
    format!("{first}{NAME_SEPARATOR}{second}")
}
