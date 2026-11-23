//! Draft and sealed runs inside the league fold (limited spec section 5).
//!
//! 0.x ran these as steps of match assignment and as pipeline passes over
//! stored rows: the kind and set code of a run (`LeagueKind`,
//! `ResolveLeagueSetCode`), the pool guard (`LeaguePoolRejectsMatch`,
//! `CloseLeagueRun`), placeholder healing (`HealPlaceholderLeague`), a draft
//! joining or minting its league's run (`ResolveDraftLeague`), and the late
//! links (`LinkUnlinkedDrafts`, `AdoptCourselessDraftLeague`). Here they are
//! more steps of the one fold, so a match has one owner and the same inputs
//! always give the same runs.

use std::collections::HashMap;

use limited::{
    CardFactsMap, Draft, DraftState, RegisteredDeck, deck_fits_pool, main_deck, pool_counts,
    registered_deck_matches_pool, set_code_from_picks, set_code_from_play_format,
};

use crate::fold::{
    Building, CONSTRUCTED_ROUNDS, DRAFT_ROUNDS, Fold, LeagueMatch, Run, RunState, SEALED_ROUNDS,
    complete, is_limited_format_code,
};

/// The start of the token of a draft run minted before MTGO named its real
/// league token (0.x `ResolveDraftLeague::PLACEHOLDER_PREFIX`).
pub const PLACEHOLDER_PREFIX: &str = "draft-";
/// A draft needs this many picks before its pool is evidence for a late link
/// (0.x `MIN_PICKS_FOR_LINKAGE` and `MIN_PICKS_FOR_ADOPTION`).
pub const MIN_PICKS_FOR_LINKAGE: usize = 20;
/// A late link needs the draft and the run to start at most this far apart
/// (0.x: more than 7 days apart never links).
pub const LINK_WINDOW_MS: i64 = 7 * 24 * 60 * 60 * 1000;

/// 0.x `LeagueKind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LeagueKind {
    Constructed,
    Draft,
    Sealed,
}

impl LeagueKind {
    /// The kind a format code names (0.x `LeagueKind::fromFormatCode`):
    /// `S6FRA` is sealed, `DHOB` draft, anything else (`CMODERN`, none)
    /// constructed.
    pub fn from_format_code(format: Option<&str>) -> Self {
        match format {
            Some(format) if is_limited_format_code(format) => {
                if format.starts_with('S') {
                    LeagueKind::Sealed
                } else {
                    LeagueKind::Draft
                }
            }
            _ => LeagueKind::Constructed,
        }
    }

    pub fn is_limited(self) -> bool {
        self != LeagueKind::Constructed
    }

    /// Matches in a full run (0.x `roundCount`): draft 3, constructed 5,
    /// sealed 5.
    pub fn round_count(self) -> usize {
        match self {
            LeagueKind::Constructed => CONSTRUCTED_ROUNDS,
            LeagueKind::Draft => DRAFT_ROUNDS,
            LeagueKind::Sealed => SEALED_ROUNDS,
        }
    }

    /// Matches in a full run of this kind whose league states `stated`
    /// (its panel's `FormatDescription`), else the kind's default.
    pub fn round_count_stated(self, stated: Option<usize>) -> usize {
        stated.unwrap_or_else(|| self.round_count())
    }

    /// `constructed`, `draft` or `sealed`.
    pub fn as_str(self) -> &'static str {
        match self {
            LeagueKind::Constructed => "constructed",
            LeagueKind::Draft => "draft",
            LeagueKind::Sealed => "sealed",
        }
    }
}

/// A draft as a fold input, at its place in the log: its first log line
/// (0.x resolved its league while processing that line).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DraftInput {
    pub order_ms: i64,
    pub file_id: u64,
    pub offset: u64,
    pub draft: Draft,
    /// The imported run (`0x:{id}`) 0.x linked this draft to, for a draft
    /// imported from 0.x. It joins that run and no other.
    pub run_key: Option<String>,
}

/// The `PlayFormatCd` of a `league_joined` line (`HOBx3`), for set codes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PanelFormat {
    pub event_id: i64,
    pub at_ms: i64,
    pub play_format: String,
}

/// What a run imported from 0.x as draft or sealed adds to its seed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LimitedSeed {
    pub kind: LeagueKind,
    pub set_code: Option<String>,
    pub course_id: Option<i64>,
}

/// What the limited rules read besides the league inputs.
#[derive(Debug, Clone, Default)]
pub struct LimitedFacts {
    /// Every draft of the account.
    pub drafts: Vec<DraftInput>,
    /// Each match's registered deck (the newest), by match token.
    pub registered: HashMap<String, RegisteredDeck>,
    /// Card facts by catalog id: basics and cards without data are neutral
    /// in the pool guards; set codes of picked cards.
    pub cards: CardFactsMap,
    /// The format of each `league_joined` line.
    pub panels: Vec<PanelFormat>,
    /// Limited seeds by their key (`0x:{id}`); a seed without one is
    /// constructed.
    pub seeds: HashMap<String, LimitedSeed>,
}

/// The fold's result: every run in mint order, and the tokens of the drafts
/// no run holds (0.x "Draft (league unknown)" rows), in fold order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeagueFold {
    pub runs: Vec<Run>,
    pub unlinked_drafts: Vec<String>,
}

/// A draft the fold met, and the run that holds it.
pub(crate) struct FoldDraft {
    pub(crate) draft: Draft,
    pub(crate) run: Option<usize>,
    /// 0.x linked it to an imported run: it is with that run or unlinked,
    /// never late-linked elsewhere.
    pub(crate) pinned: bool,
}

/// The placeholder token of a draft run (0.x `draft-{leagueId}-{courseId}`).
fn placeholder_token(event_id: i64, course_id: Option<i64>) -> String {
    match course_id {
        Some(course_id) => format!("{PLACEHOLDER_PREFIX}{event_id}-{course_id}"),
        None => format!("{PLACEHOLDER_PREFIX}{event_id}-unknown"),
    }
}

/// 0.x `ResolveDraftLeague::realToken`: a known token replaces none or a
/// placeholder; a real token is never overwritten.
fn real_token(current: &str, known: Option<String>) -> String {
    match known {
        Some(known) if current.is_empty() || current.starts_with(PLACEHOLDER_PREFIX) => known,
        _ => current.to_string(),
    }
}

impl Fold {
    /// After a match joins run `index`: a constructed run turns limited when
    /// the match's format code says so, never back (0.x
    /// `ResolveLeagueSetCode`), and a limited run fills its set code.
    pub(crate) fn after_match(&mut self, index: usize, played: &LeagueMatch) {
        let kind = LeagueKind::from_format_code(played.format.as_deref());
        let run = &mut self.runs[index].run;
        if run.kind == LeagueKind::Constructed && kind.is_limited() {
            run.kind = kind;
        }
        if run.kind.is_limited() {
            self.resolve_set_code(index, played.format.as_deref(), Some(played.started_at));
        }
    }

    /// 0.x `LeaguePoolRejectsMatch`: true when the match's registered main
    /// deck cannot have come from the run's draft pool. Only a limited run
    /// with a finished draft can tell (a draft still being projected has
    /// half a pool); anything it cannot tell is not a rejection.
    pub(crate) fn pool_rejects(&self, index: usize, played: &LeagueMatch) -> bool {
        let building = &self.runs[index];
        if !building.run.kind.is_limited() {
            return false;
        }
        let Some(draft) = building.draft else {
            return false;
        };
        let draft = &self.drafts[draft].draft;
        if draft.state != DraftState::Finished {
            return false;
        }
        let Some(registered) = self.facts.registered.get(&played.token) else {
            return false;
        };
        !deck_fits_pool(
            &pool_counts(draft),
            &main_deck(&registered.cards),
            &self.facts.cards,
        )
    }

    /// 0.x `CloseLeagueRun`: Complete when the run holds a full run of
    /// matches, Partial otherwise.
    pub(crate) fn close_run(&mut self, index: usize, at_ms: i64) {
        let building = &mut self.runs[index];
        if building.run.match_tokens.len() >= building.run.round_count() {
            complete(&mut building.run, at_ms);
        } else {
            building.run.state = RunState::Partial;
        }
    }

    /// 0.x `HealPlaceholderLeague`: with no run on the match's token, the
    /// latest panel view for that token names the league; its newest Active
    /// limited run still on a placeholder token takes the real token and the
    /// match, unless its pool rejects the match's deck.
    pub(crate) fn heal_placeholder(&mut self, played: &LeagueMatch) -> Option<usize> {
        if !self.limited {
            return None;
        }
        let event_id = self
            .joins
            .iter()
            .rev()
            .find(|join| join.token == played.league_token)?
            .event_id;
        let index = self.newest_active(|run| {
            run.event_id == Some(event_id)
                && run.kind.is_limited()
                && run.token.starts_with(PLACEHOLDER_PREFIX)
        })?;
        if self.pool_rejects(index, played) {
            return None;
        }
        self.runs[index].run.token = played.league_token.clone();
        Some(index)
    }

    /// A draft at its first log line (0.x `ResolveDraftLeague`, run by the
    /// standing, joined and pool grant lines). With a league event id it
    /// joins that league's run of its course (any state), or without a
    /// course the newest Active draft run of the league; a run already
    /// holding another draft is not its run. Otherwise it mints a draft run,
    /// closing the league's older Active runs. A draft without a league
    /// waits for the late links; one 0.x linked waits for its run.
    pub(crate) fn draft(&mut self, input: DraftInput, at_ms: i64) {
        if !self.limited
            || self
                .drafts
                .iter()
                .any(|known| known.draft.token == input.draft.token)
        {
            return;
        }
        let index = self.drafts.len();
        let event_id = input
            .draft
            .league_event_id
            .and_then(|id| i64::try_from(id).ok());
        let course_id = input.draft.course_id.and_then(|id| i64::try_from(id).ok());
        self.drafts.push(FoldDraft {
            draft: input.draft,
            run: None,
            pinned: input.run_key.is_some(),
        });

        if let Some(run_key) = input.run_key {
            match self
                .runs
                .iter()
                .position(|building| building.run.key == run_key)
            {
                Some(run) if self.runs[run].draft.is_none() => {
                    self.link_draft(index, run, Some(at_ms));
                }
                Some(_) => {}
                None => self.pending_links.push((run_key, index)),
            }
            return;
        }
        let Some(event_id) = event_id else {
            return;
        };

        let token = self
            .joins
            .iter()
            .rev()
            .find(|join| join.event_id == event_id)
            .map(|join| join.token.clone());
        let found = match course_id {
            Some(course_id) => self
                .newest(|run| run.event_id == Some(event_id) && run.course_id == Some(course_id)),
            None => self.newest_active(|run| {
                run.event_id == Some(event_id) && run.kind == LeagueKind::Draft
            }),
        }
        .filter(|&run| self.runs[run].draft.is_none());

        let run = match found {
            Some(run) => {
                let run_now = &mut self.runs[run].run;
                run_now.kind = LeagueKind::Draft;
                if run_now.course_id.is_none() {
                    run_now.course_id = course_id;
                }
                run_now.token = real_token(&run_now.token, token);
                run
            }
            None => self.mint_for_draft(index, event_id, course_id, token, at_ms),
        };
        self.link_draft(index, run, Some(at_ms));
    }

    /// 0.x `ResolveDraftLeague` creating a league: an Active draft run named
    /// for its league, keyed by the draft, started at the draft's start.
    /// Older Active runs of the league close (Complete with a full run of
    /// matches, else Partial).
    fn mint_for_draft(
        &mut self,
        draft: usize,
        event_id: i64,
        course_id: Option<i64>,
        token: Option<String>,
        at_ms: i64,
    ) -> usize {
        let started_at = self.drafts[draft].draft.started_at_ms;
        let run = Run {
            key: format!("draft:{}", self.drafts[draft].draft.token),
            token: token.unwrap_or_else(|| placeholder_token(event_id, course_id)),
            event_id: Some(event_id),
            joined_at: started_at,
            name: format!("Draft League {event_id}"),
            format: Some("Limited".to_string()),
            deck_version_id: None,
            started_at: started_at.unwrap_or(at_ms),
            state: RunState::Active,
            dropped_at: None,
            completed_at: None,
            match_tokens: Vec::new(),
            kind: LeagueKind::Draft,
            set_code: None,
            course_id,
            draft_token: None,
            former_keys: Vec::new(),
            rounds: None,
        };

        let older: Vec<usize> = (0..self.runs.len())
            .filter(|&index| {
                let run = &self.runs[index].run;
                run.state == RunState::Active && run.event_id == Some(event_id)
            })
            .collect();
        for index in older {
            let at = self.runs[index].last_ended_at.unwrap_or(at_ms);
            self.close_run(index, at);
        }

        self.runs.push(Building::new(run));
        self.runs.len() - 1
    }

    /// Run `run` holds draft `draft`; a limited run fills its set code.
    fn link_draft(&mut self, draft: usize, run: usize, at_ms: Option<i64>) {
        self.drafts[draft].run = Some(run);
        let token = self.drafts[draft].draft.token.clone();
        let building = &mut self.runs[run];
        building.draft = Some(draft);
        building.run.draft_token = Some(token);
        if building.run.kind.is_limited() {
            self.resolve_set_code(run, None, at_ms);
        }
    }

    /// A run imported from 0.x was seeded: it takes the draft 0.x linked to
    /// it, if that came first.
    pub(crate) fn take_pending_links(&mut self, run: usize) {
        let key = self.runs[run].run.key.clone();
        let mut waiting = Vec::new();
        self.pending_links.retain(|(run_key, draft)| {
            if *run_key == key {
                waiting.push(*draft);
                false
            } else {
                true
            }
        });
        for draft in waiting {
            if self.runs[run].draft.is_none() {
                self.link_draft(draft, run, None);
            }
        }
    }

    /// The passes 0.x ran after its pipeline (`LinkUnlinkedDrafts`, then
    /// `AdoptCourselessDraftLeague`), then a last set code pass. The fold is
    /// a rebuild, so 0.x's "within 30 days" limits (there to bound each
    /// pipeline run) do not apply.
    pub(crate) fn link_late(&mut self) {
        if !self.limited {
            return;
        }
        self.link_unlinked_drafts();
        self.adopt_courseless_runs();
        for index in 0..self.runs.len() {
            if !self.runs[index].retired && self.runs[index].run.kind.is_limited() {
                self.resolve_set_code(index, None, None);
            }
        }
    }

    /// 0.x `LinkUnlinkedDrafts::run`: a draft without a run (and with 20 or
    /// more picks) goes to the first draft run without a draft whose set
    /// code agrees, that started within 7 days of it, and whose newest
    /// registered deck was built from its pool.
    fn link_unlinked_drafts(&mut self) {
        for draft in 0..self.drafts.len() {
            let found = &self.drafts[draft];
            if found.run.is_some()
                || found.pinned
                || found.draft.picks.len() < MIN_PICKS_FOR_LINKAGE
            {
                continue;
            }
            let pool = pool_counts(&found.draft);
            let draft_set = set_code_from_picks(&found.draft, &self.facts.cards);
            let started_at = found.draft.started_at_ms;

            let mut chosen = None;
            for (index, building) in self.runs.iter().enumerate() {
                let run = &building.run;
                if building.retired || run.kind != LeagueKind::Draft || building.draft.is_some() {
                    continue;
                }
                if let (Some(draft_set), Some(run_set)) = (&draft_set, &run.set_code)
                    && draft_set != run_set
                {
                    continue;
                }
                if let Some(started_at) = started_at
                    && (started_at - run.started_at).abs() > LINK_WINDOW_MS
                {
                    continue;
                }
                let registered = self
                    .newest_registered(index)
                    .map(|deck| deck.cards.as_slice());
                if registered_deck_matches_pool(&pool, registered, &self.facts.cards) {
                    chosen = Some(index);
                    break;
                }
            }
            if let Some(index) = chosen {
                self.link_draft(draft, index, None);
            }
        }
    }

    /// 0.x `AdoptCourselessDraftLeague`: a draft whose run never held a
    /// match moves to the course-less limited run of its league that holds
    /// the matches its pool built (none started before the draft), and its
    /// own run goes. The adopting run takes the course id, the earlier
    /// start, and becomes Active again (Complete once its full run is
    /// played).
    fn adopt_courseless_runs(&mut self) {
        for draft in 0..self.drafts.len() {
            let found = &self.drafts[draft];
            let Some(minted) = found.run else {
                continue;
            };
            if found.draft.picks.len() < MIN_PICKS_FOR_LINKAGE {
                continue;
            }
            let minted_run = &self.runs[minted];
            if minted_run.retired || !minted_run.run.match_tokens.is_empty() {
                continue;
            }
            let Some(event_id) = minted_run.run.event_id else {
                continue;
            };
            let pool = pool_counts(&found.draft);
            let started_at = found.draft.started_at_ms;

            let mut chosen = None;
            for (index, building) in self.runs.iter().enumerate() {
                let run = &building.run;
                if index == minted
                    || building.retired
                    || run.event_id != Some(event_id)
                    || run.course_id.is_some()
                    || !run.kind.is_limited()
                    || building.draft.is_some()
                {
                    continue;
                }
                let earliest = building.match_starts.iter().min().copied();
                if let (Some(started_at), Some(earliest)) = (started_at, earliest)
                    && earliest < started_at
                {
                    continue;
                }
                let registered = self
                    .newest_registered(index)
                    .map(|deck| deck.cards.as_slice());
                if registered_deck_matches_pool(&pool, registered, &self.facts.cards) {
                    chosen = Some(index);
                    break;
                }
            }
            if let Some(candidate) = chosen {
                self.adopt(draft, minted, candidate);
            }
        }
    }

    /// Move `draft` from its `minted` run to `candidate` and retire the
    /// minted run. A dropped candidate stays dropped (0.x made it Active
    /// whatever its state; a drop is the player's word), and a drop of the
    /// minted run carries over. The candidate keeps the minted run's key in
    /// `former_keys`, so notes and drops saved under `draft:{token}` stay
    /// with the run (0.x soft-deleted the minted league).
    fn adopt(&mut self, draft: usize, minted: usize, candidate: usize) {
        let retired = &mut self.runs[minted];
        retired.retired = true;
        retired.draft = None;
        retired.run.draft_token = None;
        // A drop of the draft's run (by the player or the log) stays a drop.
        let dropped_at = (retired.run.state == RunState::Dropped)
            .then_some(retired.run.dropped_at)
            .flatten();
        retired.run.state = RunState::Partial;
        let mut former_keys = vec![retired.run.key.clone()];
        former_keys.append(&mut retired.run.former_keys);
        let course_id = retired.run.course_id.take();
        let set_code = retired.run.set_code.clone();
        let set_code_from_format = retired.set_code_from_format;
        let started_at = retired.run.started_at;
        let joined_at = retired.run.joined_at;

        let building = &mut self.runs[candidate];
        let mut ends = building.member_ends.clone();
        ends.sort_unstable();
        let run = &mut building.run;
        run.course_id = course_id;
        if run.set_code.is_none() || (set_code_from_format && !building.set_code_from_format) {
            run.set_code = set_code;
            building.set_code_from_format |= set_code_from_format;
        }
        run.started_at = run.started_at.min(started_at);
        if run.joined_at.is_none() {
            run.joined_at = joined_at;
        }
        if run.state != RunState::Dropped {
            run.state = RunState::Active;
            run.completed_at = None;
            // 0.x `CompleteLeague::runIfFinished`: counted at the end of the
            // last round's complete match.
            let rounds = run.round_count();
            if ends.len() >= rounds {
                complete(run, ends[rounds - 1]);
            }
        }
        if let Some(dropped_at) = dropped_at
            && run.state == RunState::Active
        {
            run.state = RunState::Dropped;
            run.dropped_at = Some(dropped_at);
        }
        run.former_keys.extend(former_keys);
        self.drafts[draft].run = None;
        self.link_draft(draft, candidate, None);
    }

    /// The run's newest registered deck by capture time (0.x
    /// `deckSnapshots->last()`), over its matches.
    fn newest_registered(&self, index: usize) -> Option<&RegisteredDeck> {
        let mut newest: Option<&RegisteredDeck> = None;
        for token in &self.runs[index].run.match_tokens {
            if let Some(deck) = self.facts.registered.get(token)
                && newest.is_none_or(|known| deck.captured_at_ms >= known.captured_at_ms)
            {
                newest = Some(deck);
            }
        }
        newest
    }

    /// 0.x `ResolveLeagueSetCode`: the first that answers of the match's
    /// format code, the latest panel view of the run's league (at or before
    /// `at_ms`, any when `None`), and the draft's picked cards. A code from
    /// a format code is never replaced. Unlike 0.x, a guess from the picks
    /// gives way to a later format code: a cube's picks span many sets, so
    /// the guess names a random one ("PRM Draft").
    pub(crate) fn resolve_set_code(
        &mut self,
        index: usize,
        match_format: Option<&str>,
        at_ms: Option<i64>,
    ) {
        let building = &self.runs[index];
        if building.set_code_from_format {
            return;
        }
        let panel = building
            .run
            .event_id
            .and_then(|event_id| self.panel_format(event_id, at_ms));
        let from_format = match_format
            .and_then(set_code_from_play_format)
            .or_else(|| panel.and_then(set_code_from_play_format))
            .filter(|code| !code.is_empty());
        if let Some(code) = from_format {
            let building = &mut self.runs[index];
            building.run.set_code = Some(code);
            building.set_code_from_format = true;
            return;
        }
        if building.run.set_code.is_some() {
            return;
        }
        let draft = building.draft.map(|draft| &self.drafts[draft].draft);
        let guess = draft
            .and_then(|draft| set_code_from_picks(draft, &self.facts.cards))
            .filter(|code| !code.is_empty());
        self.runs[index].run.set_code = guess;
    }

    /// The format of the latest panel view of `event_id` at or before
    /// `at_ms` (any time when `None`).
    fn panel_format(&self, event_id: i64, at_ms: Option<i64>) -> Option<&str> {
        let mut latest: Option<&PanelFormat> = None;
        for panel in &self.facts.panels {
            if panel.event_id != event_id || at_ms.is_some_and(|at| panel.at_ms > at) {
                continue;
            }
            if latest.is_none_or(|known| panel.at_ms >= known.at_ms) {
                latest = Some(panel);
            }
        }
        latest.map(|panel| panel.play_format.as_str())
    }
}
