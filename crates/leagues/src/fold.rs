//! The league run fold (leagues spec section 4).

use std::collections::{HashMap, HashSet};

use chrono_tz::Tz;
use mtgo_match::{MatchState, Outcome};

use crate::limited_runs::{DraftInput, FoldDraft, LeagueFold, LeagueKind, LimitedFacts};

/// Matches in a full constructed league run (0.x `LeagueKind::roundCount`).
pub const CONSTRUCTED_ROUNDS: usize = 5;
/// Matches in a full draft league run.
pub const DRAFT_ROUNDS: usize = 3;
/// Matches in a full sealed league run when its panel states none: MTGO's
/// Competitive Sealed League is "up to five matches" (mtgo.com/limited-events;
/// 0.x said six).
pub const SEALED_ROUNDS: usize = 5;

/// One game of a league match, as stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameResult {
    /// A drawn game is Unknown: neither won nor lost.
    pub result: mtgo_match::GameResult,
    pub on_play: Option<bool>,
    pub started_at: Option<i64>,
    pub ended_at: Option<i64>,
}

/// A log-sourced match that carries a league token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeagueMatch {
    /// The match token.
    pub token: String,
    /// The join block's `League Token`.
    pub league_token: String,
    pub started_at: i64,
    pub ended_at: Option<i64>,
    /// The store leaves out `pending` matches (0.x `Started`).
    pub state: MatchState,
    pub outcome: Outcome,
    /// The join block's `PlayFormatCd` (`CMODERN`).
    pub format: Option<String>,
    /// The join block's `GameStructureCd`, trimmed (`Modern`).
    pub game_structure: Option<String>,
    /// The deck version linked to the match, if any.
    pub deck_version_id: Option<i64>,
    pub games: Vec<GameResult>,
}

/// One input to the fold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LeagueInput {
    Match(LeagueMatch),
    /// A `league_joined` event (0.x "panel view"): the league's EventToken
    /// and EventId, and the matches its `FormatDescription` states
    /// (`panel_round_count`).
    Joined {
        token: String,
        event_id: i64,
        at_ms: i64,
        rounds: Option<usize>,
    },
    /// A `league_join_request` event: the player entered a league. It names
    /// no league; the panel MTGO logs with it does (`ENTRY_PANEL_WINDOW_MS`).
    JoinRequested {
        at_ms: i64,
    },
    /// A `league_dropped` event. It carries no token.
    Dropped {
        at_ms: i64,
    },
    /// The user dropped the run from the Leagues page.
    ManualDrop {
        run_key: String,
        at_ms: i64,
    },
    /// A run imported from 0.x, placed at its `started_at`: the fold starts
    /// it with exactly this state and these members. An Active one then
    /// takes later matches on its token like any Active run.
    ImportedRun(ImportedRunSeed),
}

/// A run imported from 0.x, as 0.x left it. Its members are the matches 0.x
/// put in it that v1 still has (under their live token when v1 tracked them
/// live), with their results.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportedRunSeed {
    /// `0x:{id}`.
    pub key: String,
    pub token: String,
    pub event_id: Option<i64>,
    pub joined_at: Option<i64>,
    pub name: String,
    pub format: Option<String>,
    pub deck_version_id: Option<i64>,
    pub started_at: i64,
    pub state: RunState,
    pub dropped_at: Option<i64>,
    pub completed_at: Option<i64>,
    /// By start.
    pub matches: Vec<LeagueMatch>,
}

/// An input with its place in the log: `(order_ms, file_id, offset, input)`.
/// A match is always placed at its `started_at` (its `order_ms` is ignored),
/// so log order and "the newest run" agree; give it the `(file_id, offset)`
/// of its join line.
pub type OrderedInput = (i64, u64, u64, LeagueInput);

/// 0.x `LeagueState`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunState {
    Active,
    Complete,
    /// The app saw a newer run on the same token start before this one
    /// finished, so it does not know how this one ended.
    Partial,
    Dropped,
}

/// One league run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Run {
    /// Stable identity for notes and overrides: `{token}:{first match token}`.
    pub key: String,
    /// The league token.
    pub token: String,
    pub event_id: Option<i64>,
    pub joined_at: Option<i64>,
    pub name: String,
    /// `PlayFormatCd` of the run's first match.
    pub format: Option<String>,
    pub deck_version_id: Option<i64>,
    pub started_at: i64,
    pub state: RunState,
    pub dropped_at: Option<i64>,
    pub completed_at: Option<i64>,
    /// The run's matches, in log order.
    pub match_tokens: Vec<String>,
    /// Constructed, draft or sealed (0.x `leagues.kind`). `fold_runs` makes
    /// only constructed runs.
    pub kind: LeagueKind,
    /// The set a limited run drafts (`HOB`), once known (0.x
    /// `leagues.set_code`). Always `None` for a constructed run.
    pub set_code: Option<String>,
    /// MTGO's course id of a limited run (0.x `leagues.mtgo_course_id`).
    pub course_id: Option<i64>,
    /// The token of the run's draft, when one is linked.
    pub draft_token: Option<String>,
    /// Keys of runs a late adoption folded into this one (`draft:{token}`):
    /// notes and drops saved under them belong to this run.
    pub former_keys: Vec<String>,
    /// The matches the league's panel states, when a panel was seen.
    pub rounds: Option<usize>,
}

impl Run {
    /// Matches in a full run: the league's stated length, else its kind's.
    pub fn round_count(&self) -> usize {
        self.kind.round_count_stated(self.rounds)
    }
}

/// A join request and the panel that names its league are logged together:
/// in every log on hand that panel is the next one, in the same second (the
/// next line, or after one `Twitch Info` line). The first panel within this
/// window claims the request; a later one is a view.
pub const ENTRY_PANEL_WINDOW_MS: i64 = 5 * 1000;

/// The matches a league panel's `FormatDescription` states
/// (`Play Up to 5 Matches`, `3 Matches`).
pub fn panel_round_count(raw: &str) -> Option<usize> {
    let line = raw
        .lines()
        .find_map(|line| line.trim().strip_prefix("FormatDescription="))?;
    let words: Vec<&str> = line.split_whitespace().collect();
    let at = words
        .iter()
        .position(|word| word.eq_ignore_ascii_case("matches"))?;
    let count: usize = words.get(at.checked_sub(1)?)?.parse().ok()?;
    (count > 0).then_some(count)
}

/// Whether a format code is a limited one (0.x
/// `MtgoMatch::isLimitedFormatCode`: `^(D[A-Z]|S[0-9])`), the `decks`
/// helper (#300 F10: one copy).
pub fn is_limited_format_code(format: &str) -> bool {
    decks::is_limited_match_format(format)
}

/// Fold one account's constructed league inputs into runs, in the order they
/// were minted. The fold sorts the inputs itself, so any arrival order gives
/// the same runs. `tz` is the OS timezone, used only for run names. Limited
/// matches are left out (`fold_league_runs` folds them).
///
/// Mirrors 0.x step by step (leagues spec section 4):
/// - a `Joined` event backfills the newest Active run on its token that has
///   no event id (`ProcessLeagueEvents::backfillFromPanelView`); it never
///   creates a run;
/// - a match joins the newest Active run on its token whose deck version is
///   unset or its own (`FindLogLeagueCandidate` step 2; a match with no deck
///   version takes any Active run on the token); a run already holding five
///   matches is closed as Complete first (`ResolveLeagueRunFromLog` step
///   2.5); with no run, one is minted (`MintLeague`) and older Active runs on
///   the token become Partial;
/// - a run becomes Complete when its fifth complete match ends, if it is
///   still Active then (`MtgoMatchObserver`), so a drop during that match
///   wins;
/// - a `Dropped` event drops the run of the latest panel view before it
///   (`AttributeDropFromLog`);
/// - a `ManualDrop` drops its run unless it is Complete;
/// - an `ImportedRun` starts a run exactly as 0.x left it, with its members
///   (older Active runs on its token go Partial, as for a mint, except a
///   short one 0.x saw only the end of: a constructed seed takes its
///   matches, `earlier_part`). From then on
///   it is like any run: an Active one takes matches, completes and drops
///   under the rules above; any other stays as it is. Callers leave its
///   members out of the plain `Match` inputs.
pub fn fold_runs(inputs: Vec<OrderedInput>, tz: Tz) -> Vec<Run> {
    run_fold(inputs, LimitedFacts::default(), false, tz).runs
}

/// The same fold with the limited rules (limited spec section 5): limited
/// matches make draft and sealed runs, drafts join or mint their league's
/// run, pool guards split runs, placeholder tokens heal, and drafts with no
/// league link late. `facts` carries the drafts (inputs at their place in the
/// log) and what the limited rules read. Pass every league match, constructed
/// ones too, so a match has one owner; constructed runs come out as
/// `fold_runs` makes them.
pub fn fold_league_runs(inputs: Vec<OrderedInput>, facts: LimitedFacts, tz: Tz) -> LeagueFold {
    run_fold(inputs, facts, true, tz)
}

fn run_fold(
    inputs: Vec<OrderedInput>,
    mut facts: LimitedFacts,
    limited: bool,
    tz: Tz,
) -> LeagueFold {
    let drafts = std::mem::take(&mut facts.drafts);
    let mut fold = Fold::new(limited, facts);
    for (at_ms, _, _, step) in sort_steps(inputs, drafts) {
        match step {
            Step::Input(LeagueInput::Match(played)) => fold.add_match(played, tz),
            Step::Input(LeagueInput::Joined {
                token,
                event_id,
                at_ms,
                rounds,
            }) => fold.joined(token, event_id, at_ms, rounds),
            Step::Input(LeagueInput::JoinRequested { at_ms }) => {
                fold.entry_requested = Some(at_ms);
            }
            Step::Input(LeagueInput::Dropped { at_ms }) => fold.dropped(at_ms),
            Step::Input(LeagueInput::ManualDrop { run_key, at_ms }) => {
                fold.manual_drop(run_key, at_ms)
            }
            Step::Input(LeagueInput::ImportedRun(seed)) => fold.seed(seed),
            Step::Draft(input) => fold.draft(input, at_ms),
            Step::Completed { token, ended_at } => fold.completed(token, ended_at),
        }
    }
    fold.finish()
}

/// One step of the fold: an input, a draft, or a complete match turning
/// Complete at its end (0.x's observer fires then, not when the match
/// starts).
enum Step {
    Input(LeagueInput),
    Draft(DraftInput),
    Completed { token: String, ended_at: i64 },
}

/// The inputs plus a `Completed` step per complete match, in log order:
/// `(order_ms, file_id, offset)`, then the step kind and its identity, so
/// even steps sharing a position sort the same way every time.
///
/// A match sits at its `started_at`. Its `Completed` step sits at its end
/// (its start when it has no end) after every log line of that millisecond,
/// so a drop logged in the same millisecond wins (0.x processes league
/// events before match events in a tick).
///
/// The `order_ms` a caller gives a match is ignored: the fold places the
/// match at its `started_at` (and its `Completed` step as above), so a caller
/// cannot move a match by passing another `order_ms`. Only league events and
/// drafts use `order_ms`.
fn sort_steps(inputs: Vec<OrderedInput>, drafts: Vec<DraftInput>) -> Vec<(i64, u64, u64, Step)> {
    let mut steps: Vec<(i64, u64, u64, Step)> = Vec::new();
    for (order_ms, file_id, offset, input) in inputs {
        match input {
            LeagueInput::Match(played) => {
                if played.state == MatchState::Complete {
                    let ended_at = played.ended_at.unwrap_or(played.started_at);
                    steps.push((
                        ended_at.max(played.started_at),
                        u64::MAX,
                        u64::MAX,
                        Step::Completed {
                            token: played.token.clone(),
                            ended_at,
                        },
                    ));
                }
                steps.push((
                    played.started_at,
                    file_id,
                    offset,
                    Step::Input(LeagueInput::Match(played)),
                ));
            }
            LeagueInput::ImportedRun(seed) => {
                // A member completes no earlier than its run starts, so the
                // run is there to count it.
                for member in &seed.matches {
                    if member.state == MatchState::Complete {
                        let ended_at = member.ended_at.unwrap_or(member.started_at);
                        steps.push((
                            ended_at.max(member.started_at).max(seed.started_at),
                            u64::MAX,
                            u64::MAX,
                            Step::Completed {
                                token: member.token.clone(),
                                ended_at,
                            },
                        ));
                    }
                }
                steps.push((
                    seed.started_at,
                    file_id,
                    offset,
                    Step::Input(LeagueInput::ImportedRun(seed)),
                ));
            }
            other => steps.push((order_ms, file_id, offset, Step::Input(other))),
        }
    }
    for draft in drafts {
        steps.push((
            draft.order_ms,
            draft.file_id,
            draft.offset,
            Step::Draft(draft),
        ));
    }
    steps.sort_by(|a, b| {
        (a.0, a.1, a.2)
            .cmp(&(b.0, b.1, b.2))
            .then_with(|| tie_key(&a.3).cmp(&tie_key(&b.3)))
    });
    steps
}

fn tie_key(step: &Step) -> (u8, String, i64) {
    match step {
        Step::Input(LeagueInput::JoinRequested { at_ms }) => (0, String::new(), *at_ms),
        Step::Input(LeagueInput::Joined {
            token, event_id, ..
        }) => (0, token.clone(), *event_id),
        Step::Input(LeagueInput::ImportedRun(seed)) => seed_tie_key(&seed.key),
        Step::Draft(input) => (2, input.draft.token.clone(), 0),
        Step::Input(LeagueInput::Match(played)) => (3, played.token.clone(), played.started_at),
        Step::Input(LeagueInput::Dropped { at_ms }) => (4, String::new(), *at_ms),
        Step::Input(LeagueInput::ManualDrop { run_key, at_ms }) => (5, run_key.clone(), *at_ms),
        Step::Completed { token, ended_at } => (6, token.clone(), *ended_at),
    }
}

/// Seeds sharing a position order by the number in `0x:{id}` (so `0x:9`
/// comes before `0x:10`), then by the key as text for any other key.
fn seed_tie_key(key: &str) -> (u8, String, i64) {
    match key
        .strip_prefix("0x:")
        .and_then(|id| id.parse::<i64>().ok())
    {
        Some(id) => (1, String::new(), id),
        None => (1, key.to_string(), 0),
    }
}

/// A run being built: the run plus what the fold needs about its matches.
pub(crate) struct Building {
    pub(crate) run: Run,
    /// The `ended_at` (or, when it never ended, `started_at`) of each match
    /// that completed while the run was Active, in the order they ended.
    complete_ends: Vec<i64>,
    /// The same for every member that completed, whatever the run's state
    /// then (0.x `CompleteLeague::runIfFinished` counts them all).
    pub(crate) member_ends: Vec<i64>,
    /// The `ended_at` of the last match added, if it ended.
    pub(crate) last_ended_at: Option<i64>,
    /// The `started_at` of each member.
    pub(crate) match_starts: Vec<i64>,
    /// The linked draft (an index into `Fold::drafts`).
    pub(crate) draft: Option<usize>,
    /// Folded into another run by a late adoption: left out of the result.
    pub(crate) retired: bool,
    /// The run's set code came from a format code (a match's or a panel
    /// view's). Any other code (a guess from the draft's picks, or one a 0.x
    /// run carried) gives way to the first format code that names a set.
    pub(crate) set_code_from_format: bool,
}

impl Building {
    pub(crate) fn new(run: Run) -> Self {
        Self {
            run,
            complete_ends: Vec::new(),
            member_ends: Vec::new(),
            last_ended_at: None,
            match_starts: Vec::new(),
            draft: None,
            retired: false,
            set_code_from_format: false,
        }
    }
}

/// A `league_joined` event seen so far.
pub(crate) struct Join {
    pub(crate) token: String,
    pub(crate) event_id: i64,
    pub(crate) at_ms: i64,
    pub(crate) rounds: Option<usize>,
}

pub(crate) struct Fold {
    pub(crate) runs: Vec<Building>,
    pub(crate) joins: Vec<Join>,
    /// The latest join request no panel has claimed yet.
    entry_requested: Option<i64>,
    /// Match token to the index of the run it joined.
    assigned: HashMap<String, usize>,
    /// Match tokens whose `Completed` step has run (a repeated token counts
    /// once).
    completed: HashSet<String>,
    /// Manual drops whose run did not exist yet at their place in the log.
    pending_drops: Vec<(String, i64)>,
    /// The limited rules apply (`fold_league_runs`).
    pub(crate) limited: bool,
    pub(crate) facts: LimitedFacts,
    /// Every draft met so far, in fold order.
    pub(crate) drafts: Vec<FoldDraft>,
    /// Drafts 0.x linked to an imported run not seeded yet: (run key, draft).
    pub(crate) pending_links: Vec<(String, usize)>,
}

impl Fold {
    fn new(limited: bool, facts: LimitedFacts) -> Self {
        Self {
            runs: Vec::new(),
            joins: Vec::new(),
            entry_requested: None,
            assigned: HashMap::new(),
            completed: HashSet::new(),
            pending_drops: Vec::new(),
            limited,
            facts,
            drafts: Vec::new(),
            pending_links: Vec::new(),
        }
    }

    fn add_match(&mut self, played: LeagueMatch, tz: Tz) {
        if !self.takes(&played) || self.assigned.contains_key(&played.token) {
            return;
        }

        let mut index = self.candidate(&played);

        // 2.4: a deck the run's finished draft could not have built belongs
        // to another run (the player re-entered while the app missed the
        // draft). Only limited runs have a pool.
        if let Some(candidate) = index
            && self.pool_rejects(candidate, &played)
        {
            let at = self.runs[candidate]
                .last_ended_at
                .unwrap_or(played.started_at);
            self.close_run(candidate, at);
            index = None;
        }

        // 2.5: a run that already holds a full run of matches never takes
        // another: the player re-entered while the app missed the drop and
        // the join.
        if let Some(candidate) = index {
            let building = &mut self.runs[candidate];
            if building.run.match_tokens.len() >= building.run.round_count() {
                let at = building.last_ended_at.unwrap_or(played.started_at);
                complete(&mut building.run, at);
                index = None;
            }
        }

        // 2.9: a draft run minted before its real token was known.
        if index.is_none() {
            index = self.heal_placeholder(&played);
        }

        let index = match index {
            Some(index) => index,
            None => self.mint(&played, tz),
        };

        let building = &mut self.runs[index];
        building.run.match_tokens.push(played.token.clone());
        building.match_starts.push(played.started_at);
        building.last_ended_at = played.ended_at;
        self.assigned.insert(played.token.clone(), index);
        self.after_match(index, &played);
    }

    /// A match the fold takes: a league token and a constructed format
    /// unless the limited rules apply. Callers already leave out tournament
    /// and `pending` matches.
    fn takes(&self, played: &LeagueMatch) -> bool {
        if played.league_token.trim().is_empty() {
            return false;
        }
        self.limited || !played.format.as_deref().is_some_and(is_limited_format_code)
    }

    /// A match of a run turned Complete. While the run is still Active, its
    /// last round's completion completes it (0.x
    /// `CompleteLeague::runIfFinished`, which the observer only calls for an
    /// Active league); a run dropped or made Partial meanwhile stays so.
    fn completed(&mut self, token: String, ended_at: i64) {
        let Some(&index) = self.assigned.get(&token) else {
            return;
        };
        if !self.completed.insert(token) {
            return;
        }
        let building = &mut self.runs[index];
        building.member_ends.push(ended_at);
        if building.run.state != RunState::Active {
            return;
        }
        building.complete_ends.push(ended_at);
        let rounds = building.run.round_count();
        if building.complete_ends.len() >= rounds {
            let at = building.complete_ends[rounds - 1];
            complete(&mut building.run, at);
        }
    }

    /// The newest Active run on the match's league token whose deck version
    /// is unset or the match's own (0.x `FindLogLeagueCandidate` step 2). A
    /// constructed run without a version adopts the match's; a limited one
    /// never does. A match without a version takes any Active run on its
    /// token. Failing that, a match with a version takes the newest Active
    /// limited run on its token whatever its version (step 2.2: limited decks
    /// change between matches, and the pool guard is what splits them).
    fn candidate(&mut self, played: &LeagueMatch) -> Option<usize> {
        let mut best: Option<usize> = None;
        for (index, building) in self.runs.iter().enumerate() {
            let run = &building.run;
            if run.state != RunState::Active || run.token != played.league_token {
                continue;
            }
            if let Some(version) = played.deck_version_id
                && run.deck_version_id.is_some()
                && run.deck_version_id != Some(version)
            {
                continue;
            }
            if self.is_newer(index, best) {
                best = Some(index);
            }
        }
        if let Some(index) = best {
            let run = &mut self.runs[index].run;
            if run.deck_version_id.is_none() && !run.kind.is_limited() {
                run.deck_version_id = played.deck_version_id;
            }
        }
        if best.is_none() && played.deck_version_id.is_some() {
            best =
                self.newest_active(|run| run.token == played.league_token && run.kind.is_limited());
        }
        best
    }

    /// Whether run `index` started after run `best` (0.x `latest('started_at')`);
    /// on a tie the later minted run is the newer.
    pub(crate) fn is_newer(&self, index: usize, best: Option<usize>) -> bool {
        match best {
            None => true,
            Some(best) => self.runs[index].run.started_at >= self.runs[best].run.started_at,
        }
    }

    /// Mint a run for the match (0.x `MintLeague`), reading the latest join
    /// on its token, and mark older Active runs on the token Partial. Its kind
    /// comes from the match's format code.
    fn mint(&mut self, played: &LeagueMatch, tz: Tz) -> usize {
        let join = self
            .joins
            .iter()
            .rev()
            .find(|join| join.token == played.league_token);
        let run = Run {
            key: format!("{}:{}", played.league_token, played.token),
            token: played.league_token.clone(),
            event_id: join.map(|join| join.event_id),
            joined_at: join.map(|join| join.at_ms),
            rounds: join.and_then(|join| join.rounds),
            name: run_name(played.game_structure.as_deref(), played.started_at, tz),
            format: played.format.clone(),
            deck_version_id: played.deck_version_id,
            started_at: played.started_at,
            state: RunState::Active,
            dropped_at: None,
            completed_at: None,
            match_tokens: Vec::new(),
            kind: LeagueKind::from_format_code(played.format.as_deref()),
            set_code: None,
            course_id: None,
            draft_token: None,
            former_keys: Vec::new(),
        };

        for building in &mut self.runs {
            let older = &mut building.run;
            if older.token == run.token
                && older.state == RunState::Active
                && older.started_at <= run.started_at
            {
                older.state = RunState::Partial;
            }
        }

        self.runs.push(Building::new(run));
        self.runs.len() - 1
    }

    /// A run imported from 0.x starts exactly as 0.x left it, with its
    /// members. Like a mint, it makes older Active runs on its token Partial
    /// (0.x minted it once, as Active, and did so then). From here on it is
    /// a run like any other: an Active one takes matches, completes, drops
    /// and goes Partial under the usual rules; any other stays as it is. A
    /// limited seed keeps its kind, set code and course id, and takes the
    /// draft 0.x linked to it.
    fn seed(&mut self, seed: ImportedRunSeed) {
        if self
            .runs
            .iter()
            .any(|building| building.run.key == seed.key)
        {
            return;
        }

        let index = self.runs.len();
        let limited = self.facts.seeds.get(&seed.key).cloned();
        // A seed 0.x never tied to an event takes the latest panel view
        // before it on its token, as a mint does.
        let (event_id, joined_at) = match seed.event_id {
            Some(_) => (seed.event_id, seed.joined_at),
            None => match self
                .joins
                .iter()
                .rev()
                .find(|join| join.token == seed.token)
            {
                Some(join) => (Some(join.event_id), seed.joined_at.or(Some(join.at_ms))),
                None => (None, seed.joined_at),
            },
        };
        let absorbed = if limited.is_none() {
            self.earlier_part(&seed, event_id)
        } else {
            None
        };
        for (other, building) in self.runs.iter_mut().enumerate() {
            let older = &mut building.run;
            if Some(other) != absorbed
                && older.token == seed.token
                && older.state == RunState::Active
                && older.started_at <= seed.started_at
            {
                older.state = RunState::Partial;
            }
        }
        let run = Run {
            key: seed.key,
            token: seed.token,
            event_id,
            joined_at,
            name: seed.name,
            format: seed.format,
            deck_version_id: seed.deck_version_id,
            started_at: seed.started_at,
            state: seed.state,
            dropped_at: seed.dropped_at,
            completed_at: seed.completed_at,
            match_tokens: Vec::new(),
            kind: limited
                .as_ref()
                .map_or(LeagueKind::Constructed, |limited| limited.kind),
            set_code: limited
                .as_ref()
                .and_then(|limited| limited.set_code.clone()),
            course_id: limited.as_ref().and_then(|limited| limited.course_id),
            draft_token: None,
            former_keys: Vec::new(),
            rounds: None,
        };
        let mut building = Building::new(run);
        if let Some(older) = absorbed {
            self.absorb(older, index, &mut building);
        }
        // The first member's format code: a limited seed's set code may be
        // 0.x's guess from the picks, which a format code replaces.
        let mut member_format = None;
        for member in seed.matches {
            if self.assigned.contains_key(&member.token) {
                continue;
            }
            if member_format.is_none() {
                member_format = member.format.clone();
            }
            building.run.match_tokens.push(member.token.clone());
            building.match_starts.push(member.started_at);
            building.last_ended_at = member.ended_at;
            self.assigned.insert(member.token, index);
        }
        let limited_seed = building.run.kind.is_limited();
        self.runs.push(building);
        if limited_seed && let Some(format) = member_format {
            self.resolve_set_code(index, Some(&format), None);
        }
        self.take_pending_links(index);
    }

    /// The run a constructed seed continues (issue #143, v1 only): 0.x was
    /// not running for an entry's first matches, so its run starts later and
    /// holds fewer matches than v1 saw. That is the newest Active run v1
    /// minted on the seed's token at or before it, not a seed, with no
    /// draft, whose event id is unknown or the seed's, whose deck version
    /// fits, and whose matches and the seed's unplaced members make at most a
    /// full run. The event id names the league, not the entry (re-entries
    /// share it), so the guards that matter are "still Active" and the
    /// five-match cap: an earlier entry v1 still shows Active (its drop was
    /// never read) can be taken in by a short re-entry's seed. That only
    /// regroups matches. Any other earlier run goes Partial, as for a mint.
    fn earlier_part(&self, seed: &ImportedRunSeed, event_id: Option<i64>) -> Option<usize> {
        let index = self.newest_active(|run| {
            run.token == seed.token
                && run.started_at <= seed.started_at
                && (run.event_id.is_none() || (event_id.is_some() && run.event_id == event_id))
                && run.kind == LeagueKind::Constructed
                && !run.key.starts_with("0x:")
                && (run.deck_version_id.is_none()
                    || seed.deck_version_id.is_none()
                    || run.deck_version_id == seed.deck_version_id)
        })?;
        let building = &self.runs[index];
        let unseen = seed
            .matches
            .iter()
            .filter(|member| !self.assigned.contains_key(&member.token))
            .count();
        let fits =
            building.run.match_tokens.len() + unseen <= LeagueKind::Constructed.round_count();
        (building.draft.is_none() && fits).then_some(index)
    }

    /// Move run `older`'s matches, with what the fold knows of them, into the
    /// seed being built at `index`; `older` is left out of the result.
    fn absorb(&mut self, older: usize, index: usize, building: &mut Building) {
        let taken = &mut self.runs[older];
        taken.retired = true;
        taken.run.state = RunState::Partial;
        let tokens = std::mem::take(&mut taken.run.match_tokens);
        building.match_starts = std::mem::take(&mut taken.match_starts);
        building.complete_ends = std::mem::take(&mut taken.complete_ends);
        building.member_ends = std::mem::take(&mut taken.member_ends);
        building.last_ended_at = taken.last_ended_at;
        let run = &mut building.run;
        run.started_at = run.started_at.min(taken.run.started_at);
        if run.deck_version_id.is_none() {
            run.deck_version_id = taken.run.deck_version_id;
        }
        for token in &tokens {
            self.assigned.insert(token.clone(), index);
        }
        run.match_tokens = tokens;
    }

    /// A panel view: backfill the event id and the stated length on the
    /// newest Active run on its token that lacks them. Never creates a run.
    /// Logged with a join request, it is a new entry of its league first.
    fn joined(&mut self, token: String, event_id: i64, at_ms: i64, rounds: Option<usize>) {
        let entered = self
            .entry_requested
            .take()
            .is_some_and(|requested| at_ms - requested <= ENTRY_PANEL_WINDOW_MS);
        if entered {
            self.close_entered(&token, at_ms);
        }
        let target = self.newest_active(|run| run.token == token && run.event_id.is_none());
        if let Some(index) = target {
            let run = &mut self.runs[index].run;
            run.event_id = Some(event_id);
            if run.joined_at.is_none() {
                run.joined_at = Some(at_ms);
            }
        }
        if let Some(rounds) = rounds
            && let Some(index) =
                self.newest_active(|run| run.token == token && run.rounds.is_none())
        {
            self.runs[index].run.rounds = Some(rounds);
        }
        self.joins.push(Join {
            token,
            event_id,
            at_ms,
            rounds,
        });
    }

    /// The player entered `token`'s league again, so its Active runs are
    /// over: MTGO lets a player into a league only once they have left it.
    /// Other leagues' runs, on other tokens, carry on.
    fn close_entered(&mut self, token: &str, at_ms: i64) {
        for index in 0..self.runs.len() {
            let building = &self.runs[index];
            if building.retired
                || building.run.state != RunState::Active
                || building.run.token != token
            {
                continue;
            }
            let at = building.last_ended_at.unwrap_or(at_ms);
            self.close_run(index, at);
        }
    }

    /// A drop signal carries no token: it drops the run of the latest panel
    /// view before it, by event id and then by token. No panel view, no drop
    /// (a wrong drop is worse than a missed one).
    fn dropped(&mut self, at_ms: i64) {
        let Some(join) = self.joins.last() else {
            return;
        };
        let by_event = self.newest_active(|run| run.event_id == Some(join.event_id));
        let target = by_event.or_else(|| self.newest_active(|run| run.token == join.token));
        if let Some(index) = target {
            let run = &mut self.runs[index].run;
            run.state = RunState::Dropped;
            run.dropped_at = Some(at_ms);
        }
    }

    pub(crate) fn newest_active(&self, wanted: impl Fn(&Run) -> bool) -> Option<usize> {
        self.newest(|run| run.state == RunState::Active && wanted(run))
    }

    /// The newest run, in any state, that `wanted` accepts.
    pub(crate) fn newest(&self, wanted: impl Fn(&Run) -> bool) -> Option<usize> {
        let mut best: Option<usize> = None;
        for (index, building) in self.runs.iter().enumerate() {
            if !building.retired && wanted(&building.run) && self.is_newer(index, best) {
                best = Some(index);
            }
        }
        best
    }

    fn manual_drop(&mut self, run_key: String, at_ms: i64) {
        match self
            .runs
            .iter_mut()
            .find(|building| building.run.key == run_key)
        {
            Some(building) => apply_manual_drop(&mut building.run, at_ms),
            None => self.pending_drops.push((run_key, at_ms)),
        }
    }

    /// Link the drafts that are still without a run (limited only), apply
    /// manual drops stamped before their run's first match (the user's clock
    /// against the log's), then hand back the runs and the unlinked drafts.
    fn finish(mut self) -> LeagueFold {
        self.link_late();
        for (run_key, at_ms) in std::mem::take(&mut self.pending_drops) {
            if let Some(building) = self.runs.iter_mut().find(|building| {
                !building.retired
                    && (building.run.key == run_key || building.run.former_keys.contains(&run_key))
            }) {
                apply_manual_drop(&mut building.run, at_ms);
            }
        }
        let tournament_drafts = if self.limited {
            self.tournament_drafts()
        } else {
            Vec::new()
        };
        let unlinked_drafts = self
            .drafts
            .iter()
            .filter(|draft| {
                draft.run.is_none()
                    && !tournament_drafts
                        .iter()
                        .any(|link| link.draft_token == draft.draft.token)
            })
            .map(|draft| draft.draft.token.clone())
            .collect();
        let runs = self
            .runs
            .into_iter()
            .filter(|building| !building.retired)
            .map(|building| building.run)
            .collect();
        LeagueFold {
            runs,
            tournament_drafts,
            unlinked_drafts,
        }
    }
}

/// 0.x `CompleteLeague::run`: Active to Complete once; a Complete run keeps
/// its first completion time.
pub(crate) fn complete(run: &mut Run, at_ms: i64) {
    if run.state == RunState::Complete {
        return;
    }
    run.state = RunState::Complete;
    run.completed_at = Some(at_ms);
}

fn apply_manual_drop(run: &mut Run, at_ms: i64) {
    if run.state == RunState::Complete {
        return;
    }
    // A run already Dropped keeps its earlier time.
    let at_ms = match (run.state, run.dropped_at) {
        (RunState::Dropped, Some(earlier)) => earlier.min(at_ms),
        _ => at_ms,
    };
    run.state = RunState::Dropped;
    run.dropped_at = Some(at_ms);
}

/// `{GameStructureCd} League {dd-mm-YYYY hh:mm}{am|pm}` in local time,
/// trimmed (0.x `MintLeague`, with minutes where 0.x printed the month).
fn run_name(game_structure: Option<&str>, started_at: i64, tz: Tz) -> String {
    let local = chrono::DateTime::<chrono::Utc>::from_timestamp_millis(started_at)
        .unwrap_or_default()
        .with_timezone(&tz);
    let name = format!(
        "{} League {}",
        game_structure.unwrap_or(""),
        local.format("%d-%m-%Y %I:%M%P")
    );
    name.trim().to_string()
}

#[cfg(test)]
mod panel_round_count_tests {
    use super::panel_round_count;

    #[test]
    fn reads_the_stated_matches() {
        let constructed = "League\nEventToken=d2050286\nFormatDescription=Play Up to 5 Matches\nPlayFormatCd=Modern";
        assert_eq!(panel_round_count(constructed), Some(5));
        assert_eq!(panel_round_count("FormatDescription=3 Matches"), Some(3));
        assert_eq!(
            panel_round_count("FormatDescription=Up to 6 matches"),
            Some(6)
        );
    }

    #[test]
    fn anything_else_states_nothing() {
        assert_eq!(panel_round_count("Description=Play up to 5 rounds"), None);
        assert_eq!(panel_round_count("FormatDescription=Matches"), None);
        assert_eq!(panel_round_count("FormatDescription=0 Matches"), None);
        assert_eq!(panel_round_count("FormatDescription=Best of three"), None);
    }
}
