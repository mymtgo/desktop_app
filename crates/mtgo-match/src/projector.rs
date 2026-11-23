//! The in-memory projector. Feeds events into per-match facts (attributing by
//! token, Match ID, or Game ID), holds events whose mapping is not known yet, and
//! re-derives each touched match.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::panic::{AssertUnwindSafe, catch_unwind};

use mtgo_protocol::{
    EventKind, PreparedEvent, deck_used_username, decode_meta_message, parse_entry,
    parse_game_management, parse_players, parse_state_change,
};

use crate::derive::{ABANDON_AFTER_MS, Derived, derive, local_player};
use crate::facts::{EntryFact, GameStateFact, JoinFact, MatchFacts, StateChangeFact};
use crate::model::{MatchState, MatchView};

const JOINED_STATE: &str = "MatchJoinedEventUnderwayState";

/// Quiet log time after which a match that cannot change on its own (finished,
/// discarded, or never joined) is dropped from memory. Its events stay in storage,
/// and a later event for it asks for them back (`Lookup::Reload`), so retiring never
/// changes a result.
pub const RETIRE_AFTER_MS: i64 = 10 * 60 * 1000;

/// Something only storage can answer. See `Projector::take_lookups`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Lookup {
    /// Load every stored event of this match and pass them to `restore`.
    Reload(String),
    /// Events wait for this Match ID's token.
    MatchId(u64),
    /// Events wait for this Game ID's match.
    GameId(u64),
}

/// How much the projector holds, for tests and diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Memory {
    pub matches: usize,
    pub held_events: usize,
}

/// How many entries each part of the projector holds (issue #49).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Footprint {
    /// Matches with facts in memory.
    pub matches: usize,
    /// Derived results, kept for retired matches too.
    pub results: usize,
    pub revisions: usize,
    /// Match ID to token mappings.
    pub match_ids: usize,
    /// Game ID to Match ID mappings.
    pub game_ids: usize,
    /// Events waiting in holding pens.
    pub held_events: usize,
    pub retired: usize,
    /// Lookups asked and not yet answered.
    pub asked: usize,
    /// Match IDs and Game IDs whose holding pen was dropped.
    pub dropped_ids: usize,
}

#[derive(Debug, Default, Clone)]
pub struct Projector {
    known_accounts: Vec<String>,
    now_ms: Option<i64>,
    facts: HashMap<String, MatchFacts>,
    derived: HashMap<String, Derived>,
    revisions: HashMap<String, u64>,
    token_by_match_id: HashMap<u64, String>,
    match_id_by_game_id: HashMap<u64, u64>,
    held_by_match_id: HashMap<u64, Vec<PreparedEvent>>,
    held_by_game_id: HashMap<u64, Vec<PreparedEvent>>,
    /// Events for retired matches, replayed by `restore`.
    held_by_token: HashMap<String, Vec<PreparedEvent>>,
    retired: HashSet<String>,
    lookups: Vec<Lookup>,
    asked: HashSet<Lookup>,
    /// Holding pens dropped while quiet; learning their mapping later asks for a reload.
    dropped_match_ids: HashSet<u64>,
    dropped_game_ids: HashSet<u64>,
    /// Matches whose far-future derive (the retire check) panicked: the check
    /// is not run again until their result changes, so the panic is reported
    /// once, not at every tick. Such a match stays in memory.
    far_future_failed: HashSet<String>,
    /// Test hook: derive panics for this token.
    #[cfg(test)]
    fail_token: Option<String>,
    /// Test hook: the far-future derive panics.
    #[cfg(test)]
    fail_far_future: bool,
    /// Test hook: far-future derives that panicked.
    #[cfg(test)]
    far_future_panics: u32,
}

/// The retire check's answer for one match.
enum RetireCheck {
    Yes,
    No,
    /// The far-future derive panicked.
    Panicked,
}

impl Projector {
    pub fn new(known_accounts: Vec<String>) -> Self {
        Self {
            known_accounts,
            ..Self::default()
        }
    }

    /// Apply one event. Returns the tokens whose derived result changed.
    pub fn apply(&mut self, event: &PreparedEvent) -> Vec<String> {
        let mut touched = BTreeSet::new();
        self.ingest(event, &mut touched);
        touched
            .into_iter()
            .filter(|token| self.rederive(token))
            .collect()
    }

    /// Apply time-based rules (abandon, disconnect resolution) as of `now_ms`.
    pub fn finalize(&mut self, now_ms: i64) -> Vec<String> {
        self.now_ms = Some(now_ms);
        let tokens: Vec<String> = self
            .facts
            .iter()
            .filter(|(_, facts)| facts.preferred_join().is_some())
            .map(|(token, _)| token.clone())
            .collect();
        tokens
            .into_iter()
            .filter(|token| self.rederive(token))
            .collect()
    }

    pub fn views(&self) -> Vec<&MatchView> {
        let mut views: Vec<&MatchView> = self
            .derived
            .values()
            .filter_map(|derived| match derived {
                Derived::Visible(view) => Some(view.as_ref()),
                _ => None,
            })
            .collect();
        views.sort_by(|a, b| {
            a.started_at_ms
                .cmp(&b.started_at_ms)
                .then_with(|| a.token.cmp(&b.token))
        });
        views
    }

    pub fn view(&self, token: &str) -> Option<&MatchView> {
        match self.derived.get(token)? {
            Derived::Visible(view) => Some(view.as_ref()),
            _ => None,
        }
    }

    pub fn derived(&self, token: &str) -> Option<&Derived> {
        self.derived.get(token)
    }

    /// The local player `derive` picks for a match in memory, read from its
    /// facts without deriving: a failed match still names it, so the
    /// tracker can tell whose match failed. `None` when the match is not in
    /// memory or no game state names its players yet.
    pub fn local_player(&self, token: &str) -> Option<String> {
        let facts = self.facts.get(token)?;
        local_player(facts, &self.known_accounts)
    }

    pub fn revision(&self, token: &str) -> u64 {
        self.revisions.get(token).copied().unwrap_or(0)
    }

    pub fn awaiting_identity(&self) -> Vec<String> {
        let mut tokens: Vec<String> = self
            .derived
            .iter()
            .filter(|(_, derived)| **derived == Derived::AwaitingIdentity)
            .map(|(token, _)| token.clone())
            .collect();
        tokens.sort();
        tokens
    }

    pub fn known_accounts(&self) -> &[String] {
        &self.known_accounts
    }

    /// Learn a local account (from a Login line). Matches waiting to identify the
    /// local player are re-derived. Returns the tokens whose result changed.
    pub fn add_account(&mut self, username: &str) -> Vec<String> {
        if self.known_accounts.iter().any(|known| known == username) {
            return Vec::new();
        }
        self.known_accounts.push(username.to_string());
        let waiting = self.awaiting_identity();
        waiting
            .into_iter()
            .filter(|token| self.rederive(token))
            .collect()
    }

    /// Continue revision numbers from storage after a restart, so they never go back.
    pub fn seed_revision(&mut self, token: &str, revision: u64) {
        let current = self.revisions.entry(token.to_string()).or_insert(0);
        if *current < revision {
            *current = revision;
        }
    }

    pub fn is_retired(&self, token: &str) -> bool {
        self.retired.contains(token)
    }

    /// After a restart: `token` has stored events but nothing in memory. Its next
    /// event asks for its history (`Lookup::Reload`) instead of starting from
    /// scratch.
    pub fn mark_stored(&mut self, token: &str) {
        if !self.facts.contains_key(token) {
            self.retired.insert(token.to_string());
        }
    }

    pub fn memory(&self) -> Memory {
        let held = self.held_by_match_id.values().map(Vec::len).sum::<usize>()
            + self.held_by_game_id.values().map(Vec::len).sum::<usize>()
            + self.held_by_token.values().map(Vec::len).sum::<usize>();
        Memory {
            matches: self.facts.len(),
            held_events: held,
        }
    }

    pub fn footprint(&self) -> Footprint {
        Footprint {
            matches: self.facts.len(),
            results: self.derived.len(),
            revisions: self.revisions.len(),
            match_ids: self.token_by_match_id.len(),
            game_ids: self.match_id_by_game_id.len(),
            held_events: self.memory().held_events,
            retired: self.retired.len(),
            asked: self.asked.len(),
            dropped_ids: self.dropped_match_ids.len() + self.dropped_game_ids.len(),
        }
    }

    /// The visible views `event` would change, without applying it. Used for the
    /// provisional last event of a log file, which may still grow (spec 5.2).
    /// A visible match the event would turn `Failed` is shown as it is now:
    /// a failed match keeps its row, marked once the event commits (#50).
    pub fn preview(&self, event: &PreparedEvent) -> Vec<MatchView> {
        let mut copy = self.clone();
        let mut views = Vec::new();
        for token in copy.apply(event) {
            let view = match copy.derived(&token) {
                Some(Derived::Visible(view)) => Some(view.as_ref()),
                Some(Derived::Failed(_)) => self.view(&token),
                _ => None,
            };
            if let Some(view) = view {
                views.push(view.clone());
            }
        }
        views
    }

    /// Questions for storage raised since the last call. See `Lookup`.
    pub fn take_lookups(&mut self) -> Vec<Lookup> {
        std::mem::take(&mut self.lookups)
    }

    /// Drop from memory every match quiet since `now_ms - RETIRE_AFTER_MS` that
    /// cannot change on its own, and holding pens that waited that long. Returns
    /// the retired tokens.
    pub fn retire_quiet(&mut self, now_ms: i64) -> Vec<String> {
        let cutoff = now_ms - RETIRE_AFTER_MS;
        let mut candidates: Vec<String> = self
            .facts
            .iter()
            .filter(|(_, facts)| facts.last_activity_ms() <= cutoff)
            .map(|(token, _)| token.clone())
            .collect();
        candidates.sort();
        let mut quiet: Vec<String> = Vec::new();
        for token in candidates {
            match self.can_retire(&token) {
                RetireCheck::Yes => quiet.push(token),
                RetireCheck::No => {}
                RetireCheck::Panicked => {
                    #[cfg(test)]
                    {
                        self.far_future_panics += 1;
                    }
                    self.far_future_failed.insert(token);
                }
            }
        }
        for token in &quiet {
            self.retire(token);
        }
        self.drop_stale_holds(cutoff);
        quiet
    }

    /// Bring a match back from storage: `history` is every stored event of the
    /// match (any order, duplicates allowed). Events that arrived while it was
    /// retired are replayed too. Returns the tokens whose result changed.
    pub fn restore(&mut self, token: &str, history: &[PreparedEvent]) -> Vec<String> {
        self.asked.remove(&Lookup::Reload(token.to_string()));
        self.retired.remove(token);
        let held = self.held_by_token.remove(token).unwrap_or_default();
        let mut touched = BTreeSet::new();
        for event in history.iter().chain(held.iter()) {
            self.ingest(event, &mut touched);
        }
        touched
            .into_iter()
            .filter(|token| self.rederive(token))
            .collect()
    }

    fn ingest(&mut self, event: &PreparedEvent, touched: &mut BTreeSet<String>) {
        match event.kind {
            EventKind::MatchStateChanged => self.ingest_state_change(event, touched),
            EventKind::GameManagementJson => self.ingest_game_management(event, touched),
            EventKind::GameStateUpdate => self.ingest_game_state(event, touched),
            EventKind::DeckUsed => self.ingest_deck_used(event, touched),
            _ => {}
        }
    }

    fn facts_for(&mut self, token: &str) -> &mut MatchFacts {
        self.facts
            .entry(token.to_string())
            .or_insert_with(|| MatchFacts::new(token))
    }

    fn ask(&mut self, lookup: Lookup) {
        if self.asked.insert(lookup.clone()) {
            self.lookups.push(lookup);
        }
    }

    /// Hold an event for a retired match and ask for its history. `true` if held.
    fn hold_if_retired(&mut self, token: &str, event: &PreparedEvent) -> bool {
        if !self.retired.contains(token) {
            return false;
        }
        self.held_by_token
            .entry(token.to_string())
            .or_default()
            .push(event.clone());
        self.ask(Lookup::Reload(token.to_string()));
        true
    }

    fn hold_for_match_id(&mut self, match_id: u64, event: &PreparedEvent) {
        self.held_by_match_id
            .entry(match_id)
            .or_default()
            .push(event.clone());
        self.ask(Lookup::MatchId(match_id));
    }

    fn hold_for_game_id(&mut self, game_id: u64, event: &PreparedEvent) {
        self.held_by_game_id
            .entry(game_id)
            .or_default()
            .push(event.clone());
        self.ask(Lookup::GameId(game_id));
    }

    /// Finished, discarded and never-joined matches may retire. In-progress and
    /// ended matches wait for `finalize`; matches waiting for identity wait for
    /// `add_account`.
    fn can_retire(&self, token: &str) -> RetireCheck {
        let Some(facts) = self.facts.get(token) else {
            return RetireCheck::No;
        };
        match self.derived.get(token) {
            // Plan E settled ties in favour of the signal, so a Complete or
            // Abandoned result no longer changes with time: this guard can fire
            // only when the far-future derive panics. It stays as a safety net
            // for future rules.
            Some(Derived::Visible(view)) => {
                if !matches!(view.state, MatchState::Complete | MatchState::Abandoned) {
                    return RetireCheck::No;
                }
                if self.far_future_failed.contains(token) {
                    return RetireCheck::No;
                }
                self.is_time_stable(token, facts)
            }
            Some(Derived::AwaitingIdentity) => RetireCheck::No,
            _ => RetireCheck::Yes,
        }
    }

    /// True when deriving far in the future (two abandon windows past the last
    /// activity) gives the current result, so retiring cannot freeze a stale one.
    fn is_time_stable(&self, token: &str, facts: &MatchFacts) -> RetireCheck {
        let far_future = facts.last_activity_ms() + 2 * ABANDON_AFTER_MS;
        #[cfg(test)]
        let inject_failure = self.fail_far_future;
        let later = catch_unwind(AssertUnwindSafe(|| {
            #[cfg(test)]
            if inject_failure {
                panic!("injected far-future failure");
            }
            derive(facts, &self.known_accounts, Some(far_future))
        }));
        match later {
            Ok(later) if self.derived.get(token) == Some(&later) => RetireCheck::Yes,
            Ok(_) => RetireCheck::No,
            Err(_) => RetireCheck::Panicked,
        }
    }

    fn retire(&mut self, token: &str) {
        self.facts.remove(token);
        let keep_result = matches!(
            self.derived.get(token),
            Some(Derived::Visible(_) | Derived::Failed(_))
        );
        if !keep_result {
            self.derived.remove(token);
        }
        self.retired.insert(token.to_string());
    }

    fn drop_stale_holds(&mut self, cutoff: i64) {
        let stale_matches: Vec<u64> = self
            .held_by_match_id
            .iter()
            .filter(|(_, events)| newest(events) <= cutoff)
            .map(|(id, _)| *id)
            .collect();
        for id in stale_matches {
            self.held_by_match_id.remove(&id);
            self.dropped_match_ids.insert(id);
            self.asked.remove(&Lookup::MatchId(id));
        }
        let stale_games: Vec<u64> = self
            .held_by_game_id
            .iter()
            .filter(|(_, events)| newest(events) <= cutoff)
            .map(|(id, _)| *id)
            .collect();
        for id in stale_games {
            self.held_by_game_id.remove(&id);
            self.dropped_game_ids.insert(id);
            self.asked.remove(&Lookup::GameId(id));
        }
    }

    fn learn_match(&mut self, match_id: u64, token: &str, touched: &mut BTreeSet<String>) {
        if self.token_by_match_id.contains_key(&match_id) {
            return;
        }
        self.token_by_match_id.insert(match_id, token.to_string());
        if self.dropped_match_ids.remove(&match_id) {
            self.ask(Lookup::Reload(token.to_string()));
        }
        if let Some(held) = self.held_by_match_id.remove(&match_id) {
            self.asked.remove(&Lookup::MatchId(match_id));
            for event in held {
                self.ingest(&event, touched);
            }
        }
    }

    fn learn_game(&mut self, game_id: u64, match_id: u64, touched: &mut BTreeSet<String>) {
        if self.match_id_by_game_id.contains_key(&game_id) {
            return;
        }
        self.match_id_by_game_id.insert(game_id, match_id);
        if self.dropped_game_ids.remove(&game_id) {
            match self.token_by_match_id.get(&match_id).cloned() {
                Some(token) => self.ask(Lookup::Reload(token)),
                None => {
                    self.dropped_match_ids.insert(match_id);
                }
            }
        }
        if let Some(held) = self.held_by_game_id.remove(&game_id) {
            self.asked.remove(&Lookup::GameId(game_id));
            for event in held {
                self.ingest(&event, touched);
            }
        }
    }

    fn ingest_state_change(&mut self, event: &PreparedEvent, touched: &mut BTreeSet<String>) {
        let Some(change) = parse_state_change(&event.text) else {
            return;
        };
        if self.hold_if_retired(&change.token, event) {
            return;
        }
        let facts = self.facts_for(&change.token);
        if !facts.mark_seen(event.key()) {
            return;
        }
        if let Some(name) = &event.local_username {
            facts.note_file_username(event.key(), name);
        }
        if event.header.context.contains(JOINED_STATE) {
            facts.add_state_join(JoinFact {
                key: event.key(),
                at_ms: event.at_ms,
                kv: Default::default(),
                raw_text: event.text.clone(),
            });
        }
        facts.add_state_change(StateChangeFact {
            key: event.key(),
            at_ms: event.at_ms,
            from: change.from,
            to: change.to,
        });
        touched.insert(change.token);
    }

    fn ingest_game_management(&mut self, event: &PreparedEvent, touched: &mut BTreeSet<String>) {
        let gm = parse_game_management(&event.text);
        if let (Some(token), Some(match_id)) = (&gm.match_token, gm.match_id) {
            self.learn_match(match_id, token, touched);
        }
        if let (Some(game_id), Some(match_id)) = (gm.game_id, gm.match_id) {
            self.learn_game(game_id, match_id, touched);
        }

        let token = gm.match_token.clone().or_else(|| {
            gm.match_id
                .and_then(|id| self.token_by_match_id.get(&id).cloned())
        });
        let Some(token) = token else {
            if let Some(match_id) = gm.match_id {
                self.hold_for_match_id(match_id, event);
            }
            return;
        };
        if self.hold_if_retired(&token, event) {
            return;
        }

        let entry = gm
            .meta_message
            .as_deref()
            .and_then(decode_meta_message)
            .map(|m| parse_entry(&m));
        let facts = self.facts_for(&token);
        if !facts.mark_seen(event.key()) {
            return;
        }
        if facts.match_id.is_none() {
            facts.match_id = gm.match_id;
        }
        if let Some(name) = &event.local_username {
            facts.note_file_username(event.key(), name);
        }
        if event.header.context.contains(JOINED_STATE) {
            facts.add_gm_join(JoinFact {
                key: event.key(),
                at_ms: event.at_ms,
                kv: gm.kv,
                raw_text: event.text.clone(),
            });
        }
        if let Some(entry) = entry {
            facts.add_entry(EntryFact {
                key: event.key(),
                at_ms: event.at_ms,
                game_id: gm.game_id,
                entry,
            });
        }
        touched.insert(token);
    }

    fn ingest_game_state(&mut self, event: &PreparedEvent, touched: &mut BTreeSet<String>) {
        let (Some(match_id), Some(game_id)) = (event.subjects.match_id, event.subjects.game_id)
        else {
            return;
        };
        self.learn_game(game_id, match_id, touched);
        let Some(token) = self.token_by_match_id.get(&match_id).cloned() else {
            self.hold_for_match_id(match_id, event);
            return;
        };
        if self.hold_if_retired(&token, event) {
            return;
        }

        // Only the first game state that names the players matters, so parse until
        // one is known, and again only for a game state that comes before it.
        let needs_players = match self.facts.get(&token).and_then(|f| f.first_players()) {
            Some((named_key, _)) => event.key() < named_key,
            None => true,
        };
        let players = if needs_players {
            parse_players(&event.text)
        } else {
            None
        };

        let facts = self.facts_for(&token);
        if !facts.mark_seen(event.key()) {
            return;
        }
        if let Some(name) = &event.local_username {
            facts.note_file_username(event.key(), name);
        }
        facts.add_game_state(GameStateFact {
            key: event.key(),
            at_ms: event.at_ms,
            game_id,
            players,
        });
        touched.insert(token);
    }

    fn ingest_deck_used(&mut self, event: &PreparedEvent, touched: &mut BTreeSet<String>) {
        let Some(game_id) = event.subjects.game_id else {
            return;
        };
        // Hold by Game ID until the game's match is known, then by Match ID until the
        // match's token is known. Each `learn_*` call releases its own holding pen.
        let Some(match_id) = self.match_id_by_game_id.get(&game_id).copied() else {
            self.hold_for_game_id(game_id, event);
            return;
        };
        let Some(token) = self.token_by_match_id.get(&match_id).cloned() else {
            self.hold_for_match_id(match_id, event);
            return;
        };
        if self.hold_if_retired(&token, event) {
            return;
        }
        let username = deck_used_username(&event.text);
        let facts = self.facts_for(&token);
        if !facts.mark_seen(event.key()) {
            return;
        }
        if let Some(name) = username {
            facts.note_deck_used_username(event.key(), &name);
        }
        touched.insert(token);
    }

    /// Re-derive one match. Returns `true` when the result changed.
    fn rederive(&mut self, token: &str) -> bool {
        let Some(facts) = self.facts.get(token) else {
            return false;
        };
        if facts.preferred_join().is_none() {
            // Lobby noise for other players' matches: never visible, skip the work.
            return false;
        }
        // A failed match stays failed: re-running it on every later event would only
        // fail (and be reported) again. A rebuild retries it.
        if matches!(self.derived.get(token), Some(Derived::Failed(_))) {
            return false;
        }
        let accounts = &self.known_accounts;
        let now_ms = self.now_ms;
        #[cfg(test)]
        let inject_failure = self.fail_token.as_deref() == Some(token);
        let outcome = catch_unwind(AssertUnwindSafe(|| {
            #[cfg(test)]
            if inject_failure {
                panic!("injected failure");
            }
            derive(facts, accounts, now_ms)
        }));
        let derived = match outcome {
            Ok(derived) => derived,
            Err(payload) => Derived::Failed(format!(
                "projection panicked: {}",
                panic_message(payload.as_ref())
            )),
        };
        if self.derived.get(token) == Some(&derived) {
            return false;
        }
        *self.revisions.entry(token.to_string()).or_insert(0) += 1;
        self.far_future_failed.remove(token);
        self.derived.insert(token.to_string(), derived);
        true
    }
}

/// The text a panic carried (`panic!("...")` gives a `&str` or a `String`).
pub fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(text) = payload.downcast_ref::<&str>() {
        return (*text).to_string();
    }
    if let Some(text) = payload.downcast_ref::<String>() {
        return text.clone();
    }
    "unknown panic payload".to_string()
}

fn newest(events: &[PreparedEvent]) -> i64 {
    events.iter().map(|e| e.at_ms).max().unwrap_or(i64::MIN)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{MatchState, Outcome, Resolution};
    use fixture_tools::synth::Synth;

    const TOKEN: &str = "f5e33a1f-c2e7-4678-b30d-309b63f17a40";
    const ME: &str = "Local_Name";
    const OPP: &str = "Opp Name";

    fn script(s: &mut Synth) -> Vec<PreparedEvent> {
        vec![
            s.joined(
                TOKEN,
                99,
                7,
                &[("PlayFormatCd", "CMODERN"), ("GameStructureCd", "Modern")],
            ),
            s.game_state(99, 7, &[ME, OPP]),
            s.deck_used(7, ME),
            s.entry(TOKEN, 99, 7, "@PLocal_Name wins the game."),
            s.game_state(99, 8, &[ME, OPP]),
            s.entry(TOKEN, 99, 8, "@PLocal_Name wins the game."),
            s.state_change(
                TOKEN,
                "MatchJoinedEventUnderwayState",
                "MatchJoinedCompletedState",
            ),
        ]
    }

    fn project(events: &[PreparedEvent]) -> Projector {
        let mut projector = Projector::new(vec![]);
        for event in events {
            projector.apply(event);
        }
        projector
    }

    #[test]
    fn the_footprint_counts_every_kind_of_entry() {
        let events = script(&mut Synth::new(Some(ME)));
        let mut projector = project(&events);
        let live = projector.footprint();
        assert_eq!((live.matches, live.results, live.revisions), (1, 1, 1));
        assert_eq!((live.match_ids, live.game_ids), (1, 2));
        assert_eq!(
            (live.held_events, live.retired, live.asked, live.dropped_ids),
            (0, 0, 0, 0)
        );

        let last = events.last().expect("events").at_ms;
        projector.retire_quiet(last + RETIRE_AFTER_MS + 1);
        let retired = projector.footprint();
        assert_eq!((retired.matches, retired.retired), (0, 1));
        assert_eq!(retired.results, 1, "a retired match keeps its result");
    }

    #[test]
    fn projects_a_complete_match() {
        let events = script(&mut Synth::new(Some(ME)));
        let projector = project(&events);
        let view = projector.view(TOKEN).expect("view");
        assert_eq!(view.state, MatchState::Complete);
        assert_eq!(view.outcome, Outcome::Win);
        assert_eq!((view.wins, view.losses), (2, 0));
        assert_eq!(view.games.len(), 2);
    }

    #[test]
    fn events_before_their_mapping_are_held_then_attached() {
        let events = script(&mut Synth::new(Some(ME)));
        // Game state (Match ID only) and deck used (Game ID only) arrive first.
        let reordered = vec![
            events[1].clone(),
            events[2].clone(),
            events[0].clone(),
            events[3].clone(),
            events[4].clone(),
            events[5].clone(),
            events[6].clone(),
        ];
        assert_eq!(project(&reordered).views(), project(&events).views());
    }

    #[test]
    fn players_are_read_from_a_later_game_state_when_the_first_names_nobody() {
        let mut s = Synth::new(Some(ME));
        let joined = s.joined(TOKEN, 99, 7, &[]);
        let empty = s.game_state(99, 7, &[]);
        let named = s.game_state(99, 7, &[ME, OPP]);

        let in_order = project(&[joined.clone(), empty.clone(), named.clone()]);
        let view = in_order.view(TOKEN).expect("view");
        assert_eq!(view.opponent.as_deref(), Some(OPP));
        assert_eq!(project(&[joined, named, empty]).views(), in_order.views());
    }

    #[test]
    fn applying_events_twice_changes_nothing() {
        let events = script(&mut Synth::new(Some(ME)));
        let mut twice = project(&events);
        let revision = twice.revision(TOKEN);
        for event in &events {
            assert!(twice.apply(event).is_empty());
        }
        assert_eq!(twice.revision(TOKEN), revision);
        assert_eq!(twice.views(), project(&events).views());
    }

    #[test]
    fn unknown_identity_is_reported_not_shown() {
        let mut s = Synth::new(None);
        let events = vec![s.joined(TOKEN, 99, 7, &[]), s.game_state(99, 7, &[ME, OPP])];
        let projector = project(&events);
        assert!(projector.views().is_empty());
        assert_eq!(projector.awaiting_identity(), vec![TOKEN.to_string()]);
    }

    #[test]
    fn revision_increments_only_on_change() {
        let mut s = Synth::new(Some(ME));
        let mut projector = Projector::new(vec![]);
        projector.apply(&s.joined(TOKEN, 99, 7, &[]));
        assert_eq!(projector.revision(TOKEN), 1);
        let changed = projector.apply(&s.game_state(99, 7, &[ME, OPP]));
        assert_eq!(changed, vec![TOKEN.to_string()]);
        assert_eq!(projector.revision(TOKEN), 2);
    }

    const NOISE: &str = "0f8d0a43-6f8e-4c1e-9a55-2f1f3f6f9a10";

    #[test]
    fn a_quiet_complete_match_is_retired_and_restored_from_history() {
        let mut s = Synth::new(Some(ME));
        let events = script(&mut s);
        let late = s.state_change(TOKEN, "MatchJoinedCompletedState", "MatchClosedState");
        let mut projector = project(&events);
        let view = projector.view(TOKEN).cloned();

        let quiet_at = events.last().expect("events").at_ms + RETIRE_AFTER_MS;
        assert_eq!(projector.retire_quiet(quiet_at), vec![TOKEN.to_string()]);
        assert_eq!(projector.memory().matches, 0);
        assert!(projector.is_retired(TOKEN));
        assert_eq!(
            projector.view(TOKEN).cloned(),
            view,
            "the view stays served"
        );

        assert!(projector.apply(&late).is_empty());
        assert_eq!(
            projector.take_lookups(),
            vec![Lookup::Reload(TOKEN.to_string())]
        );
        projector.restore(TOKEN, &events);
        assert!(!projector.is_retired(TOKEN));

        let mut never_retired = project(&events);
        never_retired.apply(&late);
        assert_eq!(projector.views(), never_retired.views());
    }

    #[test]
    fn an_in_progress_match_is_never_retired() {
        let mut s = Synth::new(Some(ME));
        let events = vec![s.joined(TOKEN, 99, 7, &[]), s.game_state(99, 7, &[ME, OPP])];
        let mut projector = project(&events);
        assert!(
            projector
                .retire_quiet(events[1].at_ms + RETIRE_AFTER_MS)
                .is_empty()
        );
        assert_eq!(projector.memory().matches, 1);
    }

    #[test]
    fn lobby_noise_is_retired_once_quiet() {
        let mut s = Synth::new(Some(ME));
        let noise = s.state_change(NOISE, "A", "B");
        let mut projector = project(std::slice::from_ref(&noise));
        assert_eq!(projector.memory().matches, 1);
        assert_eq!(
            projector.retire_quiet(noise.at_ms + RETIRE_AFTER_MS),
            vec![NOISE.to_string()]
        );
        assert_eq!(projector.memory().matches, 0);
    }

    #[test]
    fn a_held_subject_is_looked_up_once() {
        let mut s = Synth::new(Some(ME));
        let first = s.game_state(99, 7, &[ME, OPP]);
        let second = s.game_state(99, 7, &[ME, OPP]);
        let mut projector = Projector::new(vec![]);
        projector.apply(&first);
        projector.apply(&second);
        assert_eq!(projector.take_lookups(), vec![Lookup::MatchId(99)]);
        assert!(projector.take_lookups().is_empty());
    }

    #[test]
    fn held_events_whose_mapping_never_arrives_are_dropped_then_reloaded() {
        let mut s = Synth::new(Some(ME));
        let early = s.game_state(99, 7, &[ME, OPP]);
        let mut projector = Projector::new(vec![]);
        projector.apply(&early);
        projector.take_lookups();
        projector.retire_quiet(early.at_ms + RETIRE_AFTER_MS);
        assert_eq!(projector.memory().held_events, 0);

        let joined = s.joined(TOKEN, 99, 7, &[]);
        projector.apply(&joined);
        assert_eq!(
            projector.take_lookups(),
            vec![Lookup::Reload(TOKEN.to_string())]
        );
        projector.restore(TOKEN, &[early.clone(), joined.clone()]);
        assert_eq!(projector.views(), project(&[early, joined]).views());
    }

    #[test]
    fn a_new_account_identifies_waiting_matches() {
        let mut s = Synth::new(None);
        let events = vec![s.joined(TOKEN, 99, 7, &[]), s.game_state(99, 7, &[ME, OPP])];
        let mut projector = project(&events);
        assert!(projector.views().is_empty());
        assert_eq!(projector.add_account(ME), vec![TOKEN.to_string()]);
        assert!(projector.add_account(ME).is_empty());
        assert_eq!(
            projector.view(TOKEN).map(|v| v.local_username.clone()),
            Some(ME.to_string())
        );
        assert_eq!(projector.known_accounts(), &[ME.to_string()]);
    }

    #[test]
    fn seeded_revisions_continue_from_storage() {
        let mut s = Synth::new(Some(ME));
        let mut projector = Projector::new(vec![]);
        projector.seed_revision(TOKEN, 7);
        projector.apply(&s.joined(TOKEN, 99, 7, &[]));
        assert_eq!(projector.revision(TOKEN), 8);
    }

    #[test]
    fn a_stored_match_reloads_its_history_on_its_next_event() {
        let mut s = Synth::new(Some(ME));
        let events = script(&mut s);
        let late = s.state_change(TOKEN, "MatchJoinedCompletedState", "MatchClosedState");

        // A fresh projector after a restart: the match is only in storage.
        let mut restarted = Projector::new(vec![]);
        restarted.mark_stored(TOKEN);
        assert!(restarted.apply(&late).is_empty());
        assert_eq!(
            restarted.take_lookups(),
            vec![Lookup::Reload(TOKEN.to_string())]
        );
        restarted.restore(TOKEN, &events);

        let mut never_stopped = project(&events);
        never_stopped.apply(&late);
        assert_eq!(restarted.views(), never_stopped.views());
    }

    #[test]
    fn a_completion_signal_after_a_lost_connection_retires_with_its_signal_result() {
        // The signal settles at once, so the earlier lost connection can never
        // turn the match into a disconnect result later (spec 5.5).
        let mut s = Synth::new(Some(ME));
        let events = vec![
            s.joined(TOKEN, 99, 7, &[]),
            s.game_state(99, 7, &[ME, OPP]),
            s.entry(TOKEN, 99, 7, "@PLocal_Name wins the game."),
            s.game_state(99, 8, &[ME, OPP]),
            s.entry(
                TOKEN,
                99,
                8,
                "@PLocal_Name has lost connection to the game.",
            ),
            s.state_change(
                TOKEN,
                "MatchJoinedEventUnderwayState",
                "MatchJoinedCompletedState",
            ),
        ];
        let last = events.last().expect("events").at_ms;

        let mut straight = project(&events);
        straight.finalize(last + ABANDON_AFTER_MS + 1);

        let mut live = project(&events);
        live.finalize(last + RETIRE_AFTER_MS);
        assert_eq!(
            live.retire_quiet(last + RETIRE_AFTER_MS),
            vec![TOKEN.to_string()]
        );
        live.finalize(last + ABANDON_AFTER_MS + 1);
        assert_eq!(live.views(), straight.views());
        let view = live.view(TOKEN).expect("view");
        assert_eq!(view.resolution, Some(Resolution::Signal));
        assert_eq!(view.outcome, Outcome::Win);
    }

    #[test]
    fn a_far_future_derive_panic_is_reported_once_and_the_match_stays() {
        let events = script(&mut Synth::new(Some(ME)));
        let mut projector = project(&events);
        projector.fail_far_future = true;
        let quiet_at = events.last().expect("events").at_ms + RETIRE_AFTER_MS;
        for round in 0..5 {
            assert!(projector.retire_quiet(quiet_at + round).is_empty());
        }
        assert_eq!(projector.far_future_panics, 1, "tried once");
        assert_eq!(projector.memory().matches, 1, "not retired");
    }

    #[test]
    fn a_failing_match_fails_once_and_keeps_the_panic_message() {
        let events = script(&mut Synth::new(Some(ME)));
        let mut projector = Projector::new(vec![]);
        projector.fail_token = Some(TOKEN.to_string());
        assert_eq!(projector.apply(&events[0]), vec![TOKEN.to_string()]);
        assert_eq!(
            projector.derived(TOKEN),
            Some(&Derived::Failed(
                "projection panicked: injected failure".to_string()
            ))
        );
        let revision = projector.revision(TOKEN);
        for event in &events[1..] {
            assert!(projector.apply(event).is_empty());
        }
        assert_eq!(projector.revision(TOKEN), revision);
    }

    #[test]
    fn a_failed_match_still_names_its_local_player() {
        let events = script(&mut Synth::new(Some(ME)));
        let mut projector = project(&events[..2]);
        assert_eq!(projector.local_player(TOKEN).as_deref(), Some(ME));
        projector.fail_token = Some(TOKEN.to_string());
        projector.apply(&events[2]);
        assert!(matches!(projector.derived(TOKEN), Some(Derived::Failed(_))));
        assert_eq!(
            projector.local_player(TOKEN).as_deref(),
            Some(ME),
            "the players are read without deriving"
        );
        assert_eq!(projector.local_player("unknown"), None);
        assert_eq!(
            project(&events[..1]).local_player(TOKEN),
            None,
            "no game state names the players yet"
        );
    }

    /// A visible match the provisional event would turn `Failed` is still in
    /// the preview, as it is now: its row stays (marked once the event
    /// commits), and an earlier preview of a shorter version of the event
    /// must not linger (#50).
    #[test]
    fn preview_keeps_a_match_the_event_would_fail_as_it_is() {
        let events = script(&mut Synth::new(Some(ME)));
        let mut projector = project(&events[..6]);
        let current = projector.view(TOKEN).cloned().expect("visible");
        projector.fail_token = Some(TOKEN.to_string());

        assert_eq!(projector.preview(&events[6]), vec![current]);
        assert!(projector.view(TOKEN).is_some(), "nothing applied");
        projector.apply(&events[6]);
        assert!(matches!(projector.derived(TOKEN), Some(Derived::Failed(_))));
    }

    #[test]
    fn preview_shows_the_change_without_applying_it() {
        let events = script(&mut Synth::new(Some(ME)));
        let mut projector = project(&events[..6]);
        let before: Vec<MatchView> = projector.views().into_iter().cloned().collect();
        let revision = projector.revision(TOKEN);

        let previewed = projector.preview(&events[6]);
        assert_eq!(previewed.len(), 1);
        assert_eq!(previewed[0].state, MatchState::Complete);

        let after: Vec<MatchView> = projector.views().into_iter().cloned().collect();
        assert_eq!(after, before);
        assert_eq!(projector.revision(TOKEN), revision);
        assert!(projector.take_lookups().is_empty());
        assert_eq!(projector.apply(&events[6]), vec![TOKEN.to_string()]);
    }

    /// Park a deck-used event (held by Game ID 7), let it go stale so the pen is
    /// dropped, and clear the lookups raised so far.
    fn projector_with_a_dropped_game_pen(s: &mut Synth) -> Projector {
        let deck = s.deck_used(7, ME);
        let mut projector = Projector::new(vec![]);
        projector.apply(&deck);
        assert_eq!(projector.take_lookups(), vec![Lookup::GameId(7)]);
        projector.retire_quiet(deck.at_ms + RETIRE_AFTER_MS);
        assert_eq!(projector.memory().held_events, 0);
        projector
    }

    #[test]
    fn a_dropped_game_id_whose_match_token_is_known_asks_for_a_reload() {
        let mut s = Synth::new(Some(ME));
        let mut projector = projector_with_a_dropped_game_pen(&mut s);
        projector.apply(&s.joined(TOKEN, 99, 8, &[]));
        assert!(projector.take_lookups().is_empty());

        projector.apply(&s.game_state(99, 7, &[ME, OPP]));
        assert_eq!(
            projector.take_lookups(),
            vec![Lookup::Reload(TOKEN.to_string())]
        );
    }

    #[test]
    fn a_dropped_game_id_whose_match_token_is_unknown_reloads_when_the_token_arrives() {
        let mut s = Synth::new(Some(ME));
        let mut projector = projector_with_a_dropped_game_pen(&mut s);

        // Learns Game 7 belongs to Match 99, but Match 99 has no token yet.
        projector.apply(&s.game_state(99, 7, &[ME, OPP]));
        assert_eq!(projector.take_lookups(), vec![Lookup::MatchId(99)]);

        projector.apply(&s.joined(TOKEN, 99, 8, &[]));
        assert_eq!(
            projector.take_lookups(),
            vec![Lookup::Reload(TOKEN.to_string())]
        );
    }

    #[test]
    fn restore_reports_only_the_tokens_whose_result_changed() {
        let mut s = Synth::new(Some(ME));
        let events = script(&mut s);
        let mut projector = project(&events);
        projector.retire_quiet(events.last().expect("events").at_ms + RETIRE_AFTER_MS);
        // Restoring the same history gives the same result: nothing to report.
        assert!(projector.restore(TOKEN, &events).is_empty());

        // A fresh projector knows nothing, so the restored match is new.
        let mut fresh = Projector::new(vec![]);
        fresh.mark_stored(TOKEN);
        assert_eq!(fresh.restore(TOKEN, &events), vec![TOKEN.to_string()]);
    }

    #[test]
    fn a_failed_match_retires_and_keeps_its_failure() {
        let events = script(&mut Synth::new(Some(ME)));
        let mut projector = Projector::new(vec![]);
        projector.fail_token = Some(TOKEN.to_string());
        for event in &events {
            projector.apply(event);
        }
        let failed = projector.derived(TOKEN).cloned();
        assert!(matches!(failed, Some(Derived::Failed(_))));

        let quiet_at = events.last().expect("events").at_ms + RETIRE_AFTER_MS;
        assert_eq!(projector.retire_quiet(quiet_at), vec![TOKEN.to_string()]);
        assert_eq!(projector.memory().matches, 0);
        assert!(projector.is_retired(TOKEN));
        assert_eq!(projector.derived(TOKEN).cloned(), failed);
    }

    #[test]
    fn panic_message_reads_str_string_and_unknown_payloads() {
        let from_str: Box<dyn std::any::Any + Send> = Box::new("plain text");
        assert_eq!(panic_message(from_str.as_ref()), "plain text");
        let from_string: Box<dyn std::any::Any + Send> = Box::new(String::from("formatted 7"));
        assert_eq!(panic_message(from_string.as_ref()), "formatted 7");
        let other: Box<dyn std::any::Any + Send> = Box::new(42_i32);
        assert_eq!(panic_message(other.as_ref()), "unknown panic payload");
    }
}
