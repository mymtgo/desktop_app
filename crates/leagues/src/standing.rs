//! A tournament's standing for the event card (event overlay spec 5.4, 7.1):
//! one tournament's log facts, folded in log order, into what the card and
//! the store's `tournament_standings` and `tournament_rounds` rows need.
//!
//! Pure. The fold sees every fact of one tournament token in
//! `(log_file_id, byte_offset)` order, plus the account's login id and the
//! account's own matches in that tournament, and gives the same standing
//! whether the facts arrive one by one or all at once.

use std::collections::{BTreeMap, HashSet};

use mtgo_protocol::tournament::{
    EventSnapshot, RoundPairing, RoundResult, RoundsRange, TournamentFact, is_joined_state,
    is_match_over_state, is_not_joined_state,
};

/// The version of the standing fold's rules. The store's tournament sync
/// records it in `derivations ('tournament_standing')` and refolds its window
/// when the stored version is lower: bump it whenever a rule change can change
/// a stored standing.
pub const STANDING_VERSION: i64 = 1;

/// The pairing estimate after a round result (spec 5.2): MTGO's between-round
/// chat always says "approximately 2 minutes" (measured 2 min 09 s).
pub const PAIRING_GAP_MS: i64 = 2 * 60 * 1000;

/// Where the account is in the event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StandingStatus {
    /// Joined, round 1 not paired yet.
    Registered,
    /// Paired in the current round and the account's match is not over (the
    /// card reads in game or sideboarding from the live match).
    Playing,
    /// The account's match of the current round is over (or the round's
    /// result is in) and the event goes on.
    WaitingForRound,
    /// The account has a bye in the current round.
    Bye,
    /// The Swiss rounds are over, the account is in the top cut and the first
    /// playoff round is not paired yet.
    MadeCut,
    /// The event is over for the account (`finish` says how).
    Finished,
    /// The account left the event before it ended without a result (a state
    /// change to a `NotJoined` state after a joined one). The card drops it.
    Unregistered,
}

/// How the event ended for the account.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StandingFinish {
    /// Won the final.
    Trophied,
    /// Lost a playoff match (a top cut finish).
    TopCut,
    /// Swiss over, not in the cut.
    MissedCut,
    /// Finished an event with no cut (a Preliminary).
    Complete,
    /// The account dropped (`EliminationReason: 2`, inferred).
    Dropped,
    /// Removed by the event for another reason (`EliminationReason: 1`
    /// outside the cut and the playoffs).
    Eliminated,
}

/// The playoff stage of a round.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StandingStage {
    Swiss,
    /// The first playoff round of a top 8 cut (quarterfinals).
    Top8,
    Semifinal,
    Final,
}

/// The account's result in one round, from that round's result row (games
/// won, lost and drawn against the opponent; a bye is a win).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoundOutcome {
    Win,
    Loss,
    Draw,
    Bye,
}

/// One round of the tournament as the store keeps it (`tournament_rounds`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StandingRound {
    /// 1-based; playoff rounds continue the count.
    pub number: u32,
    pub stage: StandingStage,
    /// Matches in the round (byes not counted).
    pub match_count: u32,
    /// Players with a bye this round.
    pub bye_count: u32,
    /// The account's match token this round, when it was paired.
    pub account_match_token: Option<String>,
    /// The account had the bye.
    pub account_bye: bool,
    /// Log time the round was paired (its round info line).
    pub paired_at_ms: Option<i64>,
    /// The round's matches' scheduled length (`MatchTimeInMinutes` of the
    /// round's own matches, not the template's).
    pub match_minutes: Option<u32>,
    /// Log time of the round's result line.
    pub result_at_ms: Option<i64>,
    /// The account's rank in the round's result.
    pub rank: Option<u32>,
    /// Rows in the round's result (dropped players included).
    pub result_rows: Option<u32>,
    /// The account's result this round, from the round's result.
    pub outcome: Option<RoundOutcome>,
    /// Match tokens of the round still open by the facts (every player's);
    /// empty once the round's result is in.
    pub open_match_tokens: Vec<String>,
}

/// One tournament's standing for the account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TournamentStanding {
    pub token: String,
    /// `EventID` / round info `ID` (`matches.tournament_event_id`).
    pub event_id: Option<i64>,
    /// The log's event name (`EventSyncData_t.Description`).
    pub name: Option<String>,
    /// The log's format code (`GameStructureCd`, such as `CLEGACY`).
    pub format: Option<String>,
    /// Players in the top cut; `None`: no cut (a Preliminary) or unknown
    /// (no snapshot seen).
    pub cut_size: Option<u32>,
    /// Swiss rounds by spec 7a; `None` when unknown.
    pub swiss_rounds: Option<u32>,
    /// The event's round length (`RoundLengthInMinutes`).
    pub round_minutes: Option<u32>,
    /// The scheduled start, UTC ms.
    pub starts_at_ms: Option<i64>,
    /// The account joined (a `Joined` tournament state, not a later
    /// `NotJoined` one unless the event had already finished for it: MTGO may
    /// move an eliminated account to a `NotJoined` state). The sync shows only
    /// joined tournaments.
    pub joined: bool,
    pub status: StandingStatus,
    pub finish: Option<StandingFinish>,
    /// The current round (newest paired), or, once finished, the account's
    /// last round (later rounds pairing do not move it).
    pub round: Option<u32>,
    /// The stage of the current round: the one being played or just played.
    /// While waiting for the next round it is still the last one's (B5 adds
    /// the stage of the round to come, for "Next").
    pub stage: StandingStage,
    /// MTGO says the playoffs started (`InPlayoffs`).
    pub in_playoffs: bool,
    /// The account's rank in the newest round result.
    pub rank: Option<u32>,
    /// Players: rows of the newest round result, else round 1's pairings
    /// (2 x matches + byes); `None` before round 1.
    pub players: Option<u32>,
    /// Open matches of the current round (every player's); 0 once its result
    /// is in; `None` before round 1.
    pub matches_left: Option<u32>,
    /// Byes the account had.
    pub byes: u32,
    /// Final place (finishes with a known place).
    pub place: Option<u32>,
    /// Estimated next round start, UTC ms: the current round's result time +
    /// `PAIRING_GAP_MS`, only while waiting after that result or after the
    /// cut.
    pub next_round_at_ms: Option<i64>,
    /// Log time the event ended for the account.
    pub finished_at_ms: Option<i64>,
    /// Log time of the newest real transition (spec 6).
    pub last_transition_at_ms: i64,
    /// Log time of the newest fact of any kind (staleness, spec 6).
    pub last_event_at_ms: i64,
    /// Every round seen, in round order.
    pub rounds: Vec<StandingRound>,
}

/// One decoded fact with its log position and log time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StandingFact {
    pub log_file_id: i64,
    pub byte_offset: u64,
    pub logged_at_ms: i64,
    pub fact: TournamentFact,
}

/// One of the account's own matches in the tournament (its `matches` row).
/// Compaction moves a finished match's log events out of `log_events` after
/// 10 minutes, so whether it is over comes from here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnMatch {
    pub token: String,
    /// The match is over (complete or abandoned).
    pub over: bool,
    pub started_at_ms: i64,
    pub ended_at_ms: Option<i64>,
}

/// Players at or below which `ceil(log2(players))` agrees with every Swiss
/// table MTGO showed (spec 7a). Above it the formula is not trusted.
pub const LOG2_TRUSTED_MAX_PLAYERS: u32 = 128;

/// Fold one tournament's facts into the account's standing. `facts` may come
/// in any order: they are put in `(log_file_id, byte_offset)` order first.
pub fn fold_standing(
    token: &str,
    facts: &[StandingFact],
    own_matches: &[OwnMatch],
) -> TournamentStanding {
    let mut ordered: Vec<&StandingFact> = facts.iter().collect();
    ordered.sort_by_key(|fact| (fact.log_file_id, fact.byte_offset));
    let mut folder = Folder::new(token);
    for fact in ordered {
        folder.apply(fact);
    }
    folder.standing(own_matches)
}

/// A round as the fold keeps it.
#[derive(Debug, Clone, Default)]
struct RoundState {
    pairing: Option<RoundPairing>,
    paired_at_ms: Option<i64>,
    /// `InPlayoffs` when the round was paired live; `None` from a snapshot.
    playoff: Option<bool>,
    result: Option<RoundResult>,
    result_at_ms: Option<i64>,
}

/// The account's elimination, with what was known when it came.
#[derive(Debug, Clone, Copy)]
struct Elimination {
    at_ms: i64,
    reason: u32,
    /// The newest round with a result when it came.
    after_result_round: Option<u32>,
}

struct Folder {
    token: String,
    event_id: Option<i64>,
    name: Option<String>,
    format: Option<String>,
    cut_size: Option<u32>,
    /// An `EventSyncData_t` was seen, so `cut_size: None` means no cut rather
    /// than unknown.
    snapshot_seen: bool,
    number_of_rounds: Option<u32>,
    rounds_table: Vec<RoundsRange>,
    round_minutes: Option<u32>,
    starts_at_ms: Option<i64>,
    joined_at_ms: Option<i64>,
    unregistered_at_ms: Option<i64>,
    in_playoffs_at_ms: Option<i64>,
    rounds: BTreeMap<u32, RoundState>,
    elimination: Option<Elimination>,
    closed: HashSet<String>,
    ended_at_ms: Option<i64>,
    last_event_at_ms: i64,
}

impl Folder {
    fn new(token: &str) -> Self {
        Self {
            token: token.to_string(),
            event_id: None,
            name: None,
            format: None,
            cut_size: None,
            snapshot_seen: false,
            number_of_rounds: None,
            rounds_table: Vec::new(),
            round_minutes: None,
            starts_at_ms: None,
            joined_at_ms: None,
            unregistered_at_ms: None,
            in_playoffs_at_ms: None,
            rounds: BTreeMap::new(),
            elimination: None,
            closed: HashSet::new(),
            ended_at_ms: None,
            last_event_at_ms: 0,
        }
    }

    fn apply(&mut self, fact: &StandingFact) {
        let at = fact.logged_at_ms;
        self.last_event_at_ms = self.last_event_at_ms.max(at);
        match &fact.fact {
            TournamentFact::Snapshot(snapshot) => self.apply_snapshot(snapshot, at),
            TournamentFact::StateChanged { to } => {
                if is_joined_state(to) {
                    if self.joined_at_ms.is_none() || self.unregistered_at_ms.is_some() {
                        self.joined_at_ms = Some(at);
                    }
                    self.unregistered_at_ms = None;
                } else if is_not_joined_state(to) && self.joined_at_ms.is_some() {
                    self.unregistered_at_ms.get_or_insert(at);
                }
            }
            TournamentFact::Round(pairing) => {
                self.event_id = self.event_id.or(pairing.event_id);
                let playoff = self.in_playoffs_at_ms.is_some();
                let round = self.rounds.entry(pairing.number).or_default();
                if round.pairing.is_none() {
                    round.pairing = Some(pairing.clone());
                    round.paired_at_ms = Some(at);
                    round.playoff = Some(playoff);
                }
            }
            TournamentFact::Result(result) => self.apply_result(result, at),
            TournamentFact::Eliminated { of_account, reason } => {
                if *of_account && self.elimination.is_none() {
                    self.elimination = Some(Elimination {
                        at_ms: at,
                        reason: *reason,
                        after_result_round: self.newest_result_round(),
                    });
                }
            }
            TournamentFact::PlayerState { in_playoffs, .. } => {
                if *in_playoffs {
                    self.in_playoffs_at_ms.get_or_insert(at);
                }
            }
            TournamentFact::Ended => {
                self.ended_at_ms.get_or_insert(at);
            }
            TournamentFact::MatchState { match_token, to } => {
                if is_match_over_state(to) {
                    self.closed.insert(match_token.clone());
                }
            }
        }
    }

    fn apply_snapshot(&mut self, snapshot: &EventSnapshot, at: i64) {
        self.snapshot_seen = true;
        self.event_id = self.event_id.or(snapshot.event_id);
        self.name = self.name.take().or_else(|| snapshot.name.clone());
        self.format = self.format.take().or_else(|| snapshot.format.clone());
        self.cut_size = self.cut_size.or(snapshot.cut_size);
        self.number_of_rounds = self.number_of_rounds.or(snapshot.number_of_rounds);
        if self.rounds_table.is_empty() {
            self.rounds_table = snapshot.rounds_table.clone();
        }
        self.round_minutes = self.round_minutes.or(snapshot.round_minutes);
        self.starts_at_ms = self.starts_at_ms.or(snapshot.starts_at_ms);
        if snapshot.in_playoffs {
            self.in_playoffs_at_ms.get_or_insert(at);
        }
        for pairing in &snapshot.rounds {
            let round = self.rounds.entry(pairing.number).or_default();
            if round.pairing.is_none() {
                round.pairing = Some(pairing.clone());
            }
        }
        for result in &snapshot.results {
            let round = self.rounds.entry(result.number).or_default();
            if round.result.is_none() {
                round.result = Some(result.clone());
            }
        }
        if snapshot.account_eliminated && self.elimination.is_none() {
            self.elimination = Some(Elimination {
                at_ms: at,
                reason: 0,
                after_result_round: self.newest_result_round(),
            });
        }
    }

    fn apply_result(&mut self, result: &RoundResult, at: i64) {
        let round = self.rounds.entry(result.number).or_default();
        if round.result.is_none() {
            round.result = Some(result.clone());
            round.result_at_ms = Some(at);
        }
    }

    fn newest_result_round(&self) -> Option<u32> {
        self.rounds
            .iter()
            .rev()
            .find(|(_, round)| round.result.is_some())
            .map(|(number, _)| *number)
    }

    /// Players in round 1: its result's rows, else its pairings.
    fn round_one_players(&self) -> Option<u32> {
        let one = self.rounds.get(&1)?;
        if let Some(result) = &one.result {
            return Some(result.rows);
        }
        let pairing = one.pairing.as_ref()?;
        Some(2 * pairing.match_tokens.len() as u32 + pairing.bye_count)
    }

    /// Swiss rounds by spec 7a, before the corrections.
    fn base_swiss_rounds(&self) -> Option<u32> {
        if let Some(rounds) = self.number_of_rounds {
            return Some(rounds);
        }
        let players = self.round_one_players()?;
        if let Some(range) = self
            .rounds_table
            .iter()
            .find(|range| range.min <= players && players <= range.max)
        {
            return Some(range.rounds);
        }
        if (2..=LOG2_TRUSTED_MAX_PLAYERS).contains(&players) {
            return Some(ceil_log2(players));
        }
        None
    }

    /// Swiss rounds with the corrections, never below the base: a round
    /// paired live in the playoffs ends the Swiss just before it; else, in the
    /// playoffs, the highest round paired live before them; before the
    /// playoffs, the newest round. Rounds known only from a restart snapshot
    /// have no playoff flag, so they never lower it.
    fn swiss_rounds(&self) -> Option<u32> {
        let base = self.base_swiss_rounds();
        let live_numbers = |playoff: bool| {
            self.rounds
                .iter()
                .filter(move |(_, round)| round.playoff == Some(playoff))
                .map(|(number, _)| *number)
        };
        let in_playoffs = self.in_playoffs_at_ms.is_some();
        let seen = if let Some(first_playoff) = live_numbers(true).min() {
            Some(first_playoff.saturating_sub(1))
        } else if in_playoffs {
            live_numbers(false).max()
        } else {
            self.rounds.keys().max().copied()
        };
        match (base, seen) {
            (Some(base), Some(seen)) => Some(base.max(seen)),
            (Some(base), None) => Some(base),
            // Without a base, only the playoffs say where the Swiss ended.
            (None, seen) if in_playoffs => seen,
            (None, _) => None,
        }
    }

    fn is_playoff(&self, number: u32, round: &RoundState, swiss: Option<u32>) -> bool {
        match round.playoff {
            Some(playoff) => playoff,
            None => self.in_playoffs_at_ms.is_some() && swiss.is_some_and(|swiss| number > swiss),
        }
    }

    fn standing(&self, own_matches: &[OwnMatch]) -> TournamentStanding {
        let swiss = self.swiss_rounds();
        let mut rounds: Vec<StandingRound> = Vec::new();
        for (number, round) in &self.rounds {
            rounds.push(self.round_view(*number, round, swiss, own_matches));
        }
        let current = rounds
            .iter()
            .rev()
            .find(|round| round.paired_at_ms.is_some() || round.match_count > 0);
        let newest_result = rounds
            .iter()
            .rev()
            .find(|round| round.result_at_ms.is_some() || round.result_rows.is_some());

        let mut standing = TournamentStanding {
            token: self.token.clone(),
            event_id: self.event_id,
            name: self.name.clone(),
            format: self.format.clone(),
            cut_size: self.cut_size,
            swiss_rounds: swiss,
            round_minutes: self.round_minutes,
            starts_at_ms: self.starts_at_ms,
            joined: false,
            status: StandingStatus::Registered,
            finish: None,
            round: current.map(|round| round.number),
            stage: current.map_or(StandingStage::Swiss, |round| round.stage),
            in_playoffs: self.in_playoffs_at_ms.is_some(),
            rank: rounds.iter().rev().find_map(|round| round.rank),
            players: newest_result
                .and_then(|round| round.result_rows)
                .or_else(|| self.round_one_players()),
            matches_left: current.map(|round| round.open_match_tokens.len() as u32),
            byes: rounds.iter().filter(|round| round.account_bye).count() as u32,
            place: None,
            next_round_at_ms: None,
            finished_at_ms: None,
            last_transition_at_ms: 0,
            last_event_at_ms: self.last_event_at_ms,
            rounds: Vec::new(),
        };

        let finish = self.finish(&rounds, swiss);
        // An unregister counts only before a finish.
        standing.joined =
            self.joined_at_ms.is_some() && (self.unregistered_at_ms.is_none() || finish.is_some());
        if let Some((finish, place, at)) = finish {
            standing.status = StandingStatus::Finished;
            standing.finish = Some(finish);
            standing.place = place;
            standing.finished_at_ms = Some(at);
            // Frozen at the account's last round.
            if let Some(last) = rounds
                .iter()
                .rev()
                .find(|round| round.account_match_token.is_some() || round.account_bye)
            {
                standing.round = Some(last.number);
                standing.stage = last.stage;
                standing.matches_left = Some(last.open_match_tokens.len() as u32);
            }
        } else if self.unregistered_at_ms.is_some() {
            standing.status = StandingStatus::Unregistered;
        } else if let Some(current) = current {
            standing.status = self.status_in(current, swiss_rank(&rounds), own_matches);
            if matches!(
                standing.status,
                StandingStatus::WaitingForRound | StandingStatus::MadeCut
            ) && let Some(result_at) = current.result_at_ms
            {
                standing.next_round_at_ms = Some(result_at + PAIRING_GAP_MS);
            }
            if standing.status == StandingStatus::MadeCut {
                standing.stage = StandingStage::Swiss;
            }
        }

        standing.last_transition_at_ms = self.last_transition(&rounds, own_matches, &standing);
        for match_row in own_matches {
            standing.last_event_at_ms = standing
                .last_event_at_ms
                .max(match_row.ended_at_ms.unwrap_or(match_row.started_at_ms));
        }
        standing.rounds = rounds;
        standing
    }

    fn round_view(
        &self,
        number: u32,
        round: &RoundState,
        swiss: Option<u32>,
        own_matches: &[OwnMatch],
    ) -> StandingRound {
        let pairing = round.pairing.as_ref();
        let match_count = pairing.map_or(0, |p| p.match_tokens.len() as u32);
        let stage = if self.is_playoff(number, round, swiss) {
            match match_count {
                1 => StandingStage::Final,
                2 => StandingStage::Semifinal,
                _ => StandingStage::Top8,
            }
        } else {
            StandingStage::Swiss
        };
        let account = round
            .result
            .as_ref()
            .and_then(|result| result.account.as_ref());
        let account_bye = pairing.is_some_and(|p| p.account_bye);
        let outcome = account
            .and_then(|row| row.rounds.iter().find(|games| games.round == number))
            .map(|games| {
                if games.bye {
                    RoundOutcome::Bye
                } else if games.won > games.lost {
                    RoundOutcome::Win
                } else if games.won < games.lost {
                    RoundOutcome::Loss
                } else {
                    RoundOutcome::Draw
                }
            })
            .or(if account_bye && round.result.is_some() {
                Some(RoundOutcome::Bye)
            } else {
                None
            });
        let open_match_tokens = match (pairing, &round.result) {
            (Some(pairing), None) => pairing
                .match_tokens
                .iter()
                .filter(|token| !self.closed.contains(*token) && !own_over(own_matches, token))
                .cloned()
                .collect(),
            _ => Vec::new(),
        };
        StandingRound {
            number,
            stage,
            match_count,
            bye_count: pairing.map_or(0, |p| p.bye_count),
            account_match_token: pairing.and_then(|p| p.account_match_token.clone()),
            account_bye,
            paired_at_ms: round.paired_at_ms,
            match_minutes: pairing.and_then(|p| p.match_minutes),
            result_at_ms: round.result_at_ms,
            rank: account.map(|row| row.rank),
            result_rows: round.result.as_ref().map(|result| result.rows),
            outcome,
            open_match_tokens,
        }
    }

    fn status_in(
        &self,
        current: &StandingRound,
        swiss_rank: Option<u32>,
        own_matches: &[OwnMatch],
    ) -> StandingStatus {
        let result_in = current.result_rows.is_some();
        if self.in_playoffs_at_ms.is_some() && current.stage == StandingStage::Swiss {
            // A rank outside the cut is a MissedCut finish (`finish`); an
            // unknown rank or cut is not a made cut.
            let in_cut =
                matches!((swiss_rank, self.cut_size), (Some(rank), Some(cut)) if rank <= cut);
            return if in_cut {
                StandingStatus::MadeCut
            } else {
                StandingStatus::WaitingForRound
            };
        }
        if current.account_bye && !result_in {
            return StandingStatus::Bye;
        }
        let Some(token) = &current.account_match_token else {
            return StandingStatus::WaitingForRound;
        };
        if result_in || self.closed.contains(token) || own_over(own_matches, token) {
            StandingStatus::WaitingForRound
        } else {
            StandingStatus::Playing
        }
    }

    /// How the event ended for the account, its place and when; `None` while
    /// it goes on.
    fn finish(
        &self,
        rounds: &[StandingRound],
        swiss: Option<u32>,
    ) -> Option<(StandingFinish, Option<u32>, i64)> {
        let rank_in = |number: u32| {
            rounds
                .iter()
                .find(|round| round.number == number)
                .and_then(|round| round.rank)
        };
        if let Some(elimination) = self.elimination {
            if elimination.reason == 2 {
                return Some((StandingFinish::Dropped, None, elimination.at_ms));
            }
            let last_playoff = rounds
                .iter()
                .filter(|round| {
                    round.stage != StandingStage::Swiss && round.account_match_token.is_some()
                })
                .map(|round| round.number)
                .max();
            if let Some(lost_in) = last_playoff {
                return Some((StandingFinish::TopCut, rank_in(lost_in), elimination.at_ms));
            }
            let at_the_cut = self.cut_size.is_some()
                && swiss.is_some_and(|swiss| {
                    elimination
                        .after_result_round
                        .is_some_and(|after| after >= swiss)
                });
            if at_the_cut {
                let place = elimination.after_result_round.and_then(rank_in);
                return Some((StandingFinish::MissedCut, place, elimination.at_ms));
            }
            return Some((StandingFinish::Eliminated, None, elimination.at_ms));
        }
        // The playoffs started without the account: outside the cut.
        let plays_playoffs = rounds.iter().any(|round| {
            round.stage != StandingStage::Swiss && round.account_match_token.is_some()
        });
        if let (Some(cut), Some(started_at)) = (self.cut_size, self.in_playoffs_at_ms)
            && !plays_playoffs
            && let Some(rank) = swiss_rank(rounds)
            && rank > cut
        {
            return Some((StandingFinish::MissedCut, Some(rank), started_at));
        }
        if let Some(last) = rounds.last()
            && last.stage == StandingStage::Final
            && let Some(rank) = last.rank
        {
            let at = last
                .result_at_ms
                .or(self.ended_at_ms)
                .unwrap_or(self.last_event_at_ms);
            let finish = if rank == 1 {
                StandingFinish::Trophied
            } else {
                StandingFinish::TopCut
            };
            return Some((finish, Some(rank), at));
        }
        if self.snapshot_seen
            && self.cut_size.is_none()
            && let Some(swiss) = swiss
            && let Some(rank) = rank_in(swiss)
        {
            let at = rounds
                .iter()
                .find(|round| round.number == swiss)
                .and_then(|round| round.result_at_ms)
                .unwrap_or(self.last_event_at_ms);
            return Some((StandingFinish::Complete, Some(rank), at));
        }
        let ended_at = self.ended_at_ms?;
        let rank = rounds.iter().rev().find_map(|round| round.rank);
        let finish = match (self.cut_size, rank) {
            (Some(cut), Some(rank)) if self.in_playoffs_at_ms.is_some() && rank <= cut => {
                if rank == 1 {
                    StandingFinish::Trophied
                } else {
                    StandingFinish::TopCut
                }
            }
            (Some(_), Some(_)) if self.in_playoffs_at_ms.is_some() => StandingFinish::MissedCut,
            _ => StandingFinish::Complete,
        };
        Some((finish, rank, ended_at))
    }

    /// Log time of the newest real transition (spec 6): joined, a round paired,
    /// a round result, the cut, the end, an unregister, the account's own
    /// match start and end. Match closes of other players never count.
    fn last_transition(
        &self,
        rounds: &[StandingRound],
        own_matches: &[OwnMatch],
        standing: &TournamentStanding,
    ) -> i64 {
        let mut times: Vec<i64> = Vec::new();
        times.extend(self.joined_at_ms);
        times.extend(self.unregistered_at_ms);
        times.extend(self.in_playoffs_at_ms);
        times.extend(standing.finished_at_ms);
        for round in rounds {
            times.extend(round.paired_at_ms);
            times.extend(round.result_at_ms);
        }
        for own in own_matches {
            times.push(own.started_at_ms);
            times.extend(own.ended_at_ms);
        }
        times.into_iter().max().unwrap_or(0)
    }
}

/// The account's rank in the newest Swiss round result.
fn swiss_rank(rounds: &[StandingRound]) -> Option<u32> {
    rounds
        .iter()
        .rev()
        .filter(|round| round.stage == StandingStage::Swiss)
        .find_map(|round| round.rank)
}

fn own_over(own_matches: &[OwnMatch], token: &str) -> bool {
    own_matches.iter().any(|own| own.token == token && own.over)
}

/// `ceil(log2(players))` for `players >= 2`.
fn ceil_log2(players: u32) -> u32 {
    let mut rounds = 0;
    while (1u64 << rounds) < u64::from(players) {
        rounds += 1;
    }
    rounds
}

#[cfg(test)]
mod tests {
    use super::*;
    use mtgo_protocol::tournament::{decode_fact, decode_player_state};
    use mtgo_protocol::{EventKind, classify, header_time, parse_header, split_complete};

    const ACCOUNT: u64 = 900_001;
    const SHOWCASE: &str = "6ab6648d-3371-4daa-ae6b-e81bb8001148";
    const MODERN: &str = "63c90ead-8c2c-407d-9f10-67378e0b96d0";
    /// 2026-11-22T00:00:00 UTC; the fixture's local times are read as UTC
    /// offsets from it (only their order and gaps matter here).
    const DAY_MS: i64 = 1_795_305_600_000;

    /// Every fact of `token` in the Challenge fixture, as the account with
    /// `login_id` sees it, plus every match state change (the fold keeps the
    /// round's own).
    fn facts(token: &str, login_id: u64) -> Vec<StandingFact> {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/tournaments/challenge_showcase.log"
        );
        let bytes = std::fs::read(path).expect("the Challenge fixture exists");
        let mut found = Vec::new();
        for raw in split_complete(&bytes) {
            let Some(header) = parse_header(&raw.text) else {
                continue;
            };
            let first_line = raw.text.lines().next().unwrap_or("");
            let fact = if first_line.contains("Handler for FlsPlayerEventStateChangedMessage") {
                if raw.text.contains(token) {
                    decode_player_state(&raw.text)
                } else {
                    None
                }
            } else {
                match classify(&header, &raw.text) {
                    Some(c) if c.kind == EventKind::MatchStateChanged => {
                        decode_fact(c.kind, &raw.text, Some(login_id))
                    }
                    Some(c) if c.subjects.tournament_token.as_deref() == Some(token) => {
                        decode_fact(c.kind, &raw.text, Some(login_id))
                    }
                    _ => None,
                }
            };
            let Some(fact) = fact else {
                continue;
            };
            let time = header_time(first_line).expect("a header time");
            let seconds = i64::from(chrono::Timelike::num_seconds_from_midnight(&time));
            found.push(StandingFact {
                log_file_id: 1,
                byte_offset: raw.offset,
                logged_at_ms: DAY_MS + seconds * 1000,
                fact,
            });
        }
        found
    }

    fn at(hms: &str) -> i64 {
        let parts: Vec<i64> = hms
            .split(':')
            .map(|part| part.parse().expect("number"))
            .collect();
        DAY_MS + (parts[0] * 3600 + parts[1] * 60 + parts[2]) * 1000
    }

    /// The facts logged up to and including `hms`.
    fn until(facts: &[StandingFact], hms: &str) -> Vec<StandingFact> {
        facts
            .iter()
            .filter(|fact| fact.logged_at_ms <= at(hms))
            .cloned()
            .collect()
    }

    fn showcase_at(hms: &str, own: &[OwnMatch]) -> TournamentStanding {
        fold_standing(SHOWCASE, &until(&facts(SHOWCASE, ACCOUNT), hms), own)
    }

    fn modern_as(login_id: u64, hms: &str) -> TournamentStanding {
        fold_standing(MODERN, &until(&facts(MODERN, login_id), hms), &[])
    }

    #[test]
    fn registered_before_round_one() {
        let standing = showcase_at("16:59:59", &[]);
        assert!(standing.joined);
        assert_eq!(standing.status, StandingStatus::Registered);
        assert_eq!(standing.name.as_deref(), Some("Legacy Showcase Challenge"));
        assert_eq!(standing.event_id, Some(12_858_913));
        assert_eq!(standing.cut_size, Some(8));
        assert_eq!(standing.round_minutes, Some(50));
        assert_eq!(standing.starts_at_ms, Some(at("16:00:00")));
        assert_eq!(
            (standing.round, standing.players, standing.rank),
            (None, None, None)
        );
        assert_eq!(standing.swiss_rounds, None, "no players yet");
    }

    #[test]
    fn round_one_in_game_then_waiting_with_matches_left() {
        let playing = showcase_at("17:10:00", &[]);
        assert_eq!(playing.status, StandingStatus::Playing);
        assert_eq!(playing.round, Some(1));
        assert_eq!(playing.players, Some(302), "2 x 151 matches");
        assert_eq!(
            playing.swiss_rounds,
            Some(9),
            "the prize table's 213-384 range"
        );
        assert_eq!(playing.stage, StandingStage::Swiss);

        let waiting = showcase_at("17:40:00", &[]);
        assert_eq!(waiting.status, StandingStatus::WaitingForRound);
        assert_eq!(waiting.matches_left, Some(14));
        assert_eq!(
            waiting.next_round_at_ms, None,
            "no estimate while the round runs"
        );
        assert_eq!(showcase_at("17:54:04", &[]).matches_left, Some(0));
    }

    #[test]
    fn the_account_match_counts_as_over_from_its_row_after_compaction() {
        let facts = until(&facts(SHOWCASE, ACCOUNT), "17:40:00");
        let own_token = facts
            .iter()
            .find_map(|fact| match &fact.fact {
                TournamentFact::Round(round) => round.account_match_token.clone(),
                _ => None,
            })
            .expect("the account's round 1 match");
        // Compaction moved the account's match events out of `log_events`.
        let compacted: Vec<StandingFact> = facts
            .iter()
            .filter(|fact| !matches!(&fact.fact, TournamentFact::MatchState { match_token, .. } if *match_token == own_token))
            .cloned()
            .collect();
        let own = [OwnMatch {
            token: own_token,
            over: true,
            started_at_ms: at("17:00:08"),
            ended_at_ms: Some(at("17:22:03")),
        }];
        let before = fold_standing(SHOWCASE, &facts, &[]);
        let after = fold_standing(SHOWCASE, &compacted, &own);
        assert_eq!(after.matches_left, before.matches_left);
        assert_eq!(after.status, StandingStatus::WaitingForRound);
    }

    #[test]
    fn between_rounds_with_rank_and_the_pairing_estimate() {
        let standing = showcase_at("17:55:00", &[]);
        assert_eq!(standing.status, StandingStatus::WaitingForRound);
        assert_eq!(standing.rank, Some(139));
        assert_eq!(standing.players, Some(302));
        assert_eq!(standing.matches_left, Some(0));
        assert_eq!(
            standing.next_round_at_ms,
            Some(at("17:54:05") + PAIRING_GAP_MS)
        );
        assert_eq!(standing.rounds[0].outcome, Some(RoundOutcome::Win));
    }

    #[test]
    fn round_two_is_paired() {
        let standing = showcase_at("18:00:00", &[]);
        assert_eq!(standing.status, StandingStatus::Playing);
        assert_eq!(standing.round, Some(2));
        assert_eq!(standing.rounds[1].match_count, 143);
        assert_eq!(standing.rounds[1].bye_count, 1);
        assert_eq!(standing.last_transition_at_ms, at("17:56:14"));
        assert!(
            standing.last_event_at_ms > at("17:59:00"),
            "match closes keep it alive"
        );
    }

    // The Modern Challenge 32 seen from the lobby, folded with a real
    // participant's (anonymised) login id standing in for the account.

    #[test]
    fn a_quarterfinalist_made_the_cut_then_plays_the_top_8() {
        let made = modern_as(900_017, "18:03:30");
        assert!(!made.joined, "a lobby event is never joined");
        assert_eq!(made.status, StandingStatus::MadeCut);
        assert_eq!(made.swiss_rounds, Some(6));
        assert_eq!(made.rank, Some(2));
        assert_eq!(made.next_round_at_ms, Some(at("18:02:56") + PAIRING_GAP_MS));

        let playing = modern_as(900_017, "18:10:00");
        assert_eq!(playing.status, StandingStatus::Playing);
        assert_eq!(
            (playing.round, playing.stage),
            (Some(7), StandingStage::Top8)
        );

        let waiting = modern_as(900_017, "18:21:30");
        assert_eq!(waiting.status, StandingStatus::WaitingForRound);
        assert_eq!(waiting.stage, StandingStage::Top8);

        let semifinal = modern_as(900_017, "18:24:00");
        assert_eq!(
            (semifinal.round, semifinal.stage),
            (Some(8), StandingStage::Semifinal)
        );
    }

    #[test]
    fn a_quarterfinal_loser_is_a_top_cut_finish_with_its_place() {
        let lost = modern_as(9_000_641, "18:20:00");
        assert_eq!(lost.finish, Some(StandingFinish::TopCut));
        assert_eq!(lost.place, None, "the round 7 result is not in yet");
        let placed = modern_as(9_000_641, "18:21:00");
        assert_eq!(placed.place, Some(5));
        assert_eq!(placed.finished_at_ms, Some(at("18:19:14")));
    }

    #[test]
    fn ninth_after_swiss_missed_the_cut() {
        let standing = modern_as(9_000_339, "18:05:00");
        assert_eq!(standing.status, StandingStatus::Finished);
        assert_eq!(standing.finish, Some(StandingFinish::MissedCut));
        assert_eq!(standing.place, Some(9));
    }

    #[test]
    fn a_missed_cut_is_known_before_in_playoffs_arrives() {
        // The eliminations come in the same second as the last Swiss result,
        // before `InPlayoffs: true`.
        let facts = facts(MODERN, 9_000_339);
        let cut = facts
            .iter()
            .position(|fact| {
                matches!(
                    fact.fact,
                    TournamentFact::Eliminated {
                        of_account: true,
                        ..
                    }
                )
            })
            .expect("the account's elimination");
        let standing = fold_standing(MODERN, &facts[..=cut], &[]);
        assert_eq!(standing.finish, Some(StandingFinish::MissedCut));
    }

    /// Reason 2 as a drop is inferred from other players' lines, not documented.
    #[test]
    fn a_reason_two_elimination_is_an_inferred_drop() {
        let standing = modern_as(9_000_130, "18:30:00");
        assert_eq!(standing.finish, Some(StandingFinish::Dropped));
    }

    #[test]
    fn a_round_one_removal_is_an_elimination() {
        let facts = facts(SHOWCASE, 0);
        let removed = facts.iter().find_map(|fact| match &fact.fact {
            TournamentFact::Eliminated { reason: 1, .. } => Some(fact.logged_at_ms),
            _ => None,
        });
        assert!(removed.is_some(), "the fixture has round 1 removals");
        let mut as_removed = facts.clone();
        for fact in &mut as_removed {
            if Some(fact.logged_at_ms) == removed
                && let TournamentFact::Eliminated { of_account, .. } = &mut fact.fact
            {
                *of_account = true;
                break;
            }
        }
        let standing = fold_standing(SHOWCASE, &as_removed, &[]);
        assert_eq!(standing.finish, Some(StandingFinish::Eliminated));
    }

    #[test]
    fn a_bye_in_round_six() {
        let standing = modern_as(9_000_014, "17:20:00");
        assert_eq!(standing.status, StandingStatus::Bye);
        assert_eq!(standing.byes, 1);
    }

    #[test]
    fn the_finished_snapshots_give_the_winner_and_a_restart_rebuild() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/tournaments/challenge_showcase.log"
        );
        let text = std::fs::read_to_string(path).expect("fixture");
        let line = text
            .lines()
            .find(|line| {
                line.contains("Pioneer Challenge 16") && line.contains("\"PlayerEventState\":25")
            })
            .expect("the finished Pioneer snapshot");
        let Some(TournamentFact::Snapshot(snapshot)) =
            decode_fact(EventKind::TournamentSync, line, None)
        else {
            panic!("a snapshot")
        };
        let final_round = snapshot.rounds.last().expect("rounds");
        assert_eq!(final_round.match_tokens.len(), 1);
        // The final's winner: rank 1 of the last result. Find their id from the
        // raw line's last result and fold as them.
        let body = mtgo_protocol::json_repair::first_object(line).expect("json");
        let last = body["PremiereEventSyncData"]["TournamentSyncData"]["Rounds"]
            .as_array()
            .and_then(|rounds| rounds.last())
            .expect("a last round");
        let rows = last["Results"].as_array().expect("results");
        let id_at = |rank: u64| {
            rows.iter()
                .find(|row| row["Rank"].as_u64() == Some(rank))
                .and_then(|row| row["LoginID"].as_u64())
                .expect("a row")
        };
        let fold_as = |login_id: u64| {
            let fact =
                decode_fact(EventKind::TournamentSync, line, Some(login_id)).expect("snapshot");
            fold_standing(
                "pioneer",
                &[StandingFact {
                    log_file_id: 1,
                    byte_offset: 0,
                    logged_at_ms: DAY_MS,
                    fact,
                }],
                &[],
            )
        };
        let winner = fold_as(id_at(1));
        assert_eq!(winner.finish, Some(StandingFinish::Trophied));
        assert_eq!(winner.place, Some(1));
        assert_eq!(winner.swiss_rounds, Some(5));
        let finalist = fold_as(id_at(2));
        assert_eq!(
            (finalist.finish, finalist.place),
            (Some(StandingFinish::TopCut), Some(2))
        );
        assert_eq!(fold_as(id_at(20)).finish, Some(StandingFinish::MissedCut));
    }

    #[test]
    fn swiss_rounds_follow_the_spec_order() {
        let mut folder = Folder::new("t");
        folder.rounds.insert(
            1,
            RoundState {
                pairing: Some(RoundPairing {
                    number: 1,
                    match_tokens: (0..110).map(|n| n.to_string()).collect(),
                    bye_count: 0,
                    ..RoundPairing::default()
                }),
                ..RoundState::default()
            },
        );
        assert_eq!(
            folder.swiss_rounds(),
            None,
            "220 players: the formula is not trusted"
        );
        folder.rounds_table = vec![RoundsRange {
            min: 213,
            max: 384,
            rounds: 9,
        }];
        assert_eq!(folder.swiss_rounds(), Some(9), "the prize table");
        folder.number_of_rounds = Some(4);
        assert_eq!(folder.swiss_rounds(), Some(4), "NumberOfRounds first");
        folder.number_of_rounds = None;
        folder.rounds_table.clear();
        folder
            .rounds
            .get_mut(&1)
            .expect("round")
            .pairing
            .as_mut()
            .expect("pairing")
            .match_tokens
            .truncate(23);
        assert_eq!(folder.swiss_rounds(), Some(6), "46 players: ceil(log2)");
        folder.rounds.insert(
            7,
            RoundState {
                playoff: Some(false),
                ..RoundState::default()
            },
        );
        assert_eq!(
            folder.swiss_rounds(),
            Some(7),
            "raised by a higher Swiss round"
        );
        assert_eq!(ceil_log2(32), 5);
        assert_eq!(ceil_log2(33), 6);
    }

    #[test]
    fn a_preliminary_completes_after_its_last_round() {
        let snapshot = TournamentFact::Snapshot(EventSnapshot {
            number_of_rounds: Some(4),
            cut_size: None,
            ..EventSnapshot::default()
        });
        let mut facts = vec![StandingFact {
            log_file_id: 1,
            byte_offset: 0,
            logged_at_ms: DAY_MS,
            fact: snapshot,
        }];
        for round in 1..=4u32 {
            facts.push(StandingFact {
                log_file_id: 1,
                byte_offset: u64::from(round) * 10,
                logged_at_ms: DAY_MS + i64::from(round),
                fact: TournamentFact::Result(RoundResult {
                    number: round,
                    rows: 40,
                    account: Some(mtgo_protocol::tournament::AccountResult {
                        rank: 12,
                        points: 9,
                        rounds: Vec::new(),
                    }),
                }),
            });
        }
        let three = fold_standing("prelim", &facts[..4], &[]);
        assert_eq!(three.finish, None);
        let four = fold_standing("prelim", &facts, &[]);
        assert_eq!(
            (four.finish, four.place),
            (Some(StandingFinish::Complete), Some(12))
        );
    }

    #[test]
    fn an_unregister_drops_the_event() {
        let mut facts = until(&facts(SHOWCASE, ACCOUNT), "16:59:00");
        facts.push(StandingFact {
            log_file_id: 2,
            byte_offset: 0,
            logged_at_ms: at("16:59:30"),
            fact: TournamentFact::StateChanged {
                to: "PremierNotJoinedAwaitingMaxPlayersState".to_string(),
            },
        });
        let standing = fold_standing(SHOWCASE, &facts, &[]);
        assert_eq!(standing.status, StandingStatus::Unregistered);
        assert!(!standing.joined);
    }

    #[test]
    fn repeats_and_order_do_not_change_the_standing() {
        let facts = facts(SHOWCASE, ACCOUNT);
        let all = fold_standing(SHOWCASE, &facts, &[]);
        // Every line delivered twice (round info, snapshots, results...).
        let mut doubled = facts.clone();
        doubled.extend(facts.iter().cloned().map(|mut fact| {
            fact.byte_offset += 1;
            fact
        }));
        assert_eq!(fold_standing(SHOWCASE, &doubled, &[]), all);
        // Shuffled input is put back in log order.
        let mut reversed = facts.clone();
        reversed.reverse();
        assert_eq!(fold_standing(SHOWCASE, &reversed, &[]), all);
    }

    /// The account's own match rows as the store had them at `at_ms`: a
    /// row from the round's pairing, over from its close or the round's result.
    fn own_rows_at(facts: &[StandingFact], at_ms: i64) -> Vec<OwnMatch> {
        let mut rows: Vec<OwnMatch> = Vec::new();
        for fact in facts {
            let TournamentFact::Round(round) = &fact.fact else {
                continue;
            };
            let Some(token) = &round.account_match_token else {
                continue;
            };
            if fact.logged_at_ms > at_ms || rows.iter().any(|row| row.token == *token) {
                continue;
            }
            let ended = facts.iter().find_map(|other| match &other.fact {
                TournamentFact::MatchState { match_token, to }
                    if match_token == token && is_match_over_state(to) =>
                {
                    Some(other.logged_at_ms)
                }
                TournamentFact::Result(result) if result.number == round.number => {
                    Some(other.logged_at_ms)
                }
                _ => None,
            });
            let ended = ended.filter(|ended| *ended <= at_ms);
            rows.push(OwnMatch {
                token: token.clone(),
                over: ended.is_some(),
                started_at_ms: fact.logged_at_ms,
                ended_at_ms: ended,
            });
        }
        rows
    }

    /// Folding live (one fact at a time, read after each with the own match
    /// rows of that moment: playing, then over) gives at every point the
    /// standing a fresh fold of that prefix gives, and at the end the fold of
    /// everything.
    #[test]
    fn live_equals_rebuild() {
        for (token, login_id) in [(SHOWCASE, ACCOUNT), (MODERN, 900_017)] {
            let facts = facts(token, login_id);
            let mut folder = Folder::new(token);
            let mut playing_seen = false;
            for (index, fact) in facts.iter().enumerate() {
                folder.apply(fact);
                let last = index + 1 == facts.len();
                if matches!(fact.fact, TournamentFact::MatchState { .. }) && !last {
                    continue;
                }
                let own = own_rows_at(&facts, fact.logged_at_ms);
                let live = folder.standing(&own);
                playing_seen |= live.status == StandingStatus::Playing;
                assert_eq!(
                    live,
                    fold_standing(token, &facts[..=index], &own),
                    "{token} at fact {index}"
                );
            }
            assert!(playing_seen, "{token}: the rows were read while playing");
            let end = facts.last().expect("facts").logged_at_ms;
            let own = own_rows_at(&facts, end);
            assert_eq!(
                folder.standing(&own),
                fold_standing(token, &facts, &own),
                "{token}"
            );
        }
    }

    fn fact(offset: u64, fact: TournamentFact) -> StandingFact {
        StandingFact {
            log_file_id: 1,
            byte_offset: offset,
            logged_at_ms: DAY_MS + offset as i64 * 1000,
            fact,
        }
    }

    fn pairing(number: u32, matches: u32, account_token: Option<&str>) -> RoundPairing {
        let mut match_tokens: Vec<String> =
            (0..matches).map(|n| format!("r{number}-m{n}")).collect();
        if let Some(token) = account_token {
            match_tokens[0] = token.to_string();
        }
        RoundPairing {
            number,
            match_tokens,
            account_match_token: account_token.map(str::to_string),
            ..RoundPairing::default()
        }
    }

    fn result(number: u32, rows: u32, rank: u32) -> RoundResult {
        RoundResult {
            number,
            rows,
            account: Some(mtgo_protocol::tournament::AccountResult {
                rank,
                points: 0,
                rounds: Vec::new(),
            }),
        }
    }

    /// A Challenge of 46 players (6 Swiss rounds by `ceil(log2)`), restarted
    /// after round 2: rounds 3 to 6 come only from the restart snapshot.
    fn restarted_challenge(rank_after_swiss: u32) -> Vec<StandingFact> {
        let mut facts = vec![fact(
            0,
            TournamentFact::Snapshot(EventSnapshot {
                cut_size: Some(8),
                ..EventSnapshot::default()
            }),
        )];
        for number in 1..=2 {
            let offset = u64::from(number) * 10;
            let token = format!("own-{number}");
            facts.push(fact(
                offset,
                TournamentFact::Round(pairing(number, 23, Some(&token))),
            ));
            facts.push(fact(
                offset + 1,
                TournamentFact::Result(result(number, 46, 20)),
            ));
        }
        let snapshot = EventSnapshot {
            cut_size: Some(8),
            rounds: (3..=6)
                .map(|number| pairing(number, 23, Some(&format!("own-{number}"))))
                .collect(),
            results: (3..=6)
                .map(|number| result(number, 46, if number == 6 { rank_after_swiss } else { 20 }))
                .collect(),
            ..EventSnapshot::default()
        };
        facts.push(fact(100, TournamentFact::Snapshot(snapshot)));
        facts
    }

    #[test]
    fn snapshot_only_middle_rounds_never_lower_the_swiss_rounds() {
        // Ninth: eliminated at the cut, then the playoffs start without it.
        let mut ninth = restarted_challenge(9);
        assert_eq!(fold_standing("c", &ninth, &[]).swiss_rounds, Some(6));
        ninth.push(fact(
            110,
            TournamentFact::Eliminated {
                of_account: true,
                reason: 1,
            },
        ));
        ninth.push(fact(
            111,
            TournamentFact::PlayerState {
                new_state: 24,
                in_playoffs: true,
            },
        ));
        ninth.push(fact(112, TournamentFact::Round(pairing(7, 4, None))));
        let standing = fold_standing("c", &ninth, &[]);
        assert_eq!(
            standing.swiss_rounds,
            Some(6),
            "round 7 is the first playoff"
        );
        assert!(
            standing.rounds[2..6]
                .iter()
                .all(|round| round.stage == StandingStage::Swiss),
            "the snapshot's rounds stay Swiss"
        );
        assert_eq!(
            (standing.finish, standing.place),
            (Some(StandingFinish::MissedCut), Some(9))
        );

        // Third: plays the quarterfinal and loses it.
        let mut third = restarted_challenge(3);
        third.push(fact(
            111,
            TournamentFact::PlayerState {
                new_state: 24,
                in_playoffs: true,
            },
        ));
        third.push(fact(
            112,
            TournamentFact::Round(pairing(7, 4, Some("own-7"))),
        ));
        third.push(fact(113, TournamentFact::Result(result(7, 46, 6))));
        third.push(fact(
            114,
            TournamentFact::Eliminated {
                of_account: true,
                reason: 1,
            },
        ));
        let standing = fold_standing("c", &third, &[]);
        assert_eq!(standing.swiss_rounds, Some(6));
        assert_eq!(standing.rounds[6].stage, StandingStage::Top8);
        assert_eq!(
            (standing.finish, standing.place),
            (Some(StandingFinish::TopCut), Some(6))
        );
    }

    /// No cut is known only from a snapshot without a playoff; without any
    /// snapshot the cut is unknown and the last Swiss round finishes nothing.
    #[test]
    fn no_snapshot_means_the_cut_is_unknown() {
        let mut facts = vec![fact(1, TournamentFact::Round(pairing(1, 8, Some("own-1"))))];
        for number in 1..=4 {
            facts.push(fact(
                u64::from(number) * 10,
                TournamentFact::Result(result(number, 16, 5)),
            ));
        }
        let unknown = fold_standing("p", &facts, &[]);
        assert_eq!(unknown.swiss_rounds, Some(4), "16 players: ceil(log2)");
        assert_eq!(unknown.finish, None);
        assert_eq!(unknown.status, StandingStatus::WaitingForRound);

        facts.push(fact(0, TournamentFact::Snapshot(EventSnapshot::default())));
        let no_cut = fold_standing("p", &facts, &[]);
        assert_eq!(
            (no_cut.finish, no_cut.place),
            (Some(StandingFinish::Complete), Some(5))
        );
    }

    fn joined(offset: u64) -> StandingFact {
        fact(
            offset,
            TournamentFact::StateChanged {
                to: "PremierJoinedAwaitingMaxPlayersState".to_string(),
            },
        )
    }

    fn playoffs_start(offset: u64) -> StandingFact {
        fact(
            offset,
            TournamentFact::PlayerState {
                new_state: 24,
                in_playoffs: true,
            },
        )
    }

    #[test]
    fn a_not_joined_state_after_a_finish_keeps_the_result() {
        let mut facts = restarted_challenge(9);
        facts.push(joined(1));
        facts.push(fact(
            110,
            TournamentFact::Eliminated {
                of_account: true,
                reason: 1,
            },
        ));
        facts.push(fact(
            111,
            TournamentFact::StateChanged {
                to: "TournamentNotJoinedBetweenRoundsState".to_string(),
            },
        ));
        let standing = fold_standing("c", &facts, &[]);
        assert!(standing.joined);
        assert_eq!(standing.status, StandingStatus::Finished);
        assert_eq!(standing.finish, Some(StandingFinish::MissedCut));
    }

    #[test]
    fn in_playoffs_is_a_made_cut_only_inside_the_cut() {
        let mut third = restarted_challenge(3);
        third.push(playoffs_start(111));
        let made = fold_standing("c", &third, &[]);
        assert_eq!((made.status, made.finish), (StandingStatus::MadeCut, None));

        let mut twelfth = restarted_challenge(12);
        twelfth.push(playoffs_start(111));
        let missed = fold_standing("c", &twelfth, &[]);
        assert_eq!(missed.status, StandingStatus::Finished);
        assert_eq!(
            (missed.finish, missed.place, missed.finished_at_ms),
            (
                Some(StandingFinish::MissedCut),
                Some(12),
                Some(DAY_MS + 111_000)
            )
        );
    }

    #[test]
    fn a_finished_card_keeps_its_last_round_as_later_rounds_pair() {
        let mut facts = restarted_challenge(3);
        facts.push(playoffs_start(111));
        facts.push(fact(
            112,
            TournamentFact::Round(pairing(7, 4, Some("own-7"))),
        ));
        facts.push(fact(113, TournamentFact::Result(result(7, 46, 6))));
        facts.push(fact(
            114,
            TournamentFact::Eliminated {
                of_account: true,
                reason: 1,
            },
        ));
        let lost = fold_standing("c", &facts, &[]);
        facts.push(fact(120, TournamentFact::Round(pairing(8, 2, None))));
        facts.push(fact(130, TournamentFact::Round(pairing(9, 1, None))));
        let later = fold_standing("c", &facts, &[]);
        for standing in [&lost, &later] {
            assert_eq!(standing.finish, Some(StandingFinish::TopCut));
            assert_eq!(
                (standing.round, standing.stage),
                (Some(7), StandingStage::Top8)
            );
        }
        assert_eq!(later.rounds.len(), 9, "the rounds still list every round");
    }

    #[test]
    fn a_missed_cut_after_swiss_stays_on_its_last_swiss_round() {
        let mut facts = restarted_challenge(12);
        facts.push(playoffs_start(111));
        facts.push(fact(112, TournamentFact::Round(pairing(7, 4, None))));
        let standing = fold_standing("c", &facts, &[]);
        assert_eq!(standing.finish, Some(StandingFinish::MissedCut));
        assert_eq!(
            (standing.round, standing.stage),
            (Some(6), StandingStage::Swiss)
        );
    }
}
