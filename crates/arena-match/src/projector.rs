//! Folds Arena events into match views, deck uses, opponents and rank points.

use std::collections::HashMap;

use arena_protocol::{
    ArenaEvent, CardCount, EventName, RankReading, ResultEntry, Scope, Seat, commander_format_code,
    parse_event_name,
};
use mtgo_match::games::{outcome_from, threshold_outcome};
use mtgo_match::{
    ABANDON_AFTER_MS, EndReason, GameResult, GameView, MatchState, MatchView, Outcome, Resolution,
};
use serde::Serialize;

use crate::rank::{Ladder, RankGate, RankPoint};

/// One decoded event at its place in the log.
#[derive(Debug, Clone, PartialEq)]
pub struct ArenaInput {
    pub file_id: u64,
    pub offset: u64,
    /// The event's index within its log entry (one entry can hold several).
    pub seq: u32,
    /// The entry's time (header time, or the time before it when the entry
    /// has none). Events that carry a GRE time use that instead.
    pub at_ms: i64,
    pub event: ArenaEvent,
}

/// The logged in account: the display name (`Name#12345`) from the login
/// line or a fast login's `authenticateResponse`, and, once authenticated,
/// the user id. An account known only by its user id (a fast login whose
/// screen name has no `#` tag, trap 29) is `arena:<user_id>`, with the
/// untagged name as `display_name` once one is read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ArenaIdentity {
    pub username: String,
    pub user_id: Option<String>,
    pub display_name: Option<String>,
}

impl ArenaIdentity {
    /// Whether the account is known only by its user id.
    pub fn is_untagged(&self) -> bool {
        self.username.starts_with(UNTAGGED_PREFIX)
    }
}

/// The username prefix of an account known only by its user id. MTGO
/// usernames cannot hold `:`, so it never names an MTGO account.
const UNTAGGED_PREFIX: &str = "arena:";

/// What one event changed.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct ArenaChanges {
    /// Tokens of matches whose view changed.
    pub matches: Vec<String>,
    /// `(opponent_key, display name)` of each newly visible match's opponent.
    pub opponents: Vec<(String, String)>,
    pub deck_uses: Vec<DeckUse>,
    pub rank: Vec<RankPoint>,
}

/// The deck a match was played with: the deck set for its event. Cards are
/// raw Arena grpIds with their copies (the tracker applies the catalog id
/// offset).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DeckUse {
    pub match_token: String,
    pub deck_id: String,
    pub deck_name: String,
    pub format_code: String,
    pub main: Vec<(u32, u32)>,
    pub side: Vec<(u32, u32)>,
    /// The commanders' grpIds of a Brawl deck; empty for any other deck.
    pub commanders: Vec<u32>,
}

/// The opponents registry key of an Arena player: never an MTGO name.
pub fn opponent_key(user_id: &str) -> String {
    format!("arena:{user_id}")
}

/// The Arena projector. See the crate docs.
#[derive(Debug, Clone, Default)]
pub struct ArenaProjector {
    identity: Option<ArenaIdentity>,
    /// The log file of the latest login line or `authenticateResponse`.
    login_file: Option<u64>,
    /// The latest `EventSetDeckV3`: the deck of the next match in its event.
    pending_deck: Option<SetDeck>,
    /// The match the latest room state or game info was about: events that
    /// name no match (turn 1, mulligans) belong to it.
    current: Option<String>,
    matches: HashMap<String, ArenaMatch>,
    views: HashMap<String, MatchView>,
    rank: RankGate,
    /// The latest `finalize` time: quiet matches are settled as of it.
    now_ms: Option<i64>,
}

/// The highest game number read. Bo3 has 3 games; a larger number is a
/// broken or hostile log, and would otherwise grow a match's game list
/// without bound.
const MAX_GAME_NUMBER: u32 = 5;

/// A deck set for an event.
#[derive(Debug, Clone, PartialEq, Eq)]
struct SetDeck {
    event_name: String,
    deck_id: String,
    deck_name: String,
    main: Vec<(u32, u32)>,
    side: Vec<(u32, u32)>,
    commanders: Vec<u32>,
    /// The deck's own format name (`HistoricBrawl`), when the log gave one.
    format: Option<String>,
}

/// The local player's side of a match, fixed when first found.
#[derive(Debug, Clone, PartialEq, Eq)]
struct LocalSide {
    username: String,
    seat: u32,
    team: u32,
    event_id: String,
}

/// Whether a match is one game (Bo1) or best of three (Bo3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Bo1,
    Bo3,
}

impl Kind {
    /// The kind a `matchWinCondition` names; `None` for any other text.
    fn from_win_condition(win_condition: &str) -> Option<Kind> {
        match win_condition {
            "MatchWinCondition_SingleElimination" => Some(Kind::Bo1),
            "MatchWinCondition_Best2of3" => Some(Kind::Bo3),
            _ => None,
        }
    }

    /// The `game_structure` value of the view.
    fn game_structure(self) -> &'static str {
        match self {
            Kind::Bo1 => "bo1",
            Kind::Bo3 => "bo3",
        }
    }

    /// The outcome once a side has the games it needs and is ahead: one
    /// game in Bo1, MTGO's threshold in Bo3.
    fn decided_outcome(self, wins: u32, losses: u32) -> Option<Outcome> {
        match self {
            Kind::Bo3 => threshold_outcome(wins, losses),
            Kind::Bo1 if wins > losses => Some(Outcome::Win),
            Kind::Bo1 if losses > wins => Some(Outcome::Loss),
            Kind::Bo1 => None,
        }
    }
}

/// Everything read about one match so far.
#[derive(Debug, Clone, Default)]
struct ArenaMatch {
    token: String,
    seats: Vec<Seat>,
    local: Option<LocalSide>,
    /// The kind read from the first game info with a win condition, fixed
    /// once read.
    kind: Option<Kind>,
    /// Whether game infos without a win condition (stored before it was
    /// read) said best of three; once true it stays true. Only used while
    /// `kind` is unknown.
    stored_best_of_three: Option<bool>,
    started_at_ms: i64,
    /// Time of the first event carrying the match result entry.
    decided_at_ms: Option<i64>,
    /// Time of the completed room state.
    completed_at_ms: Option<i64>,
    /// The latest cumulative result list (trap 2).
    results: Vec<ResultEntry>,
    games: Vec<ArenaGame>,
    current_game: u32,
    last_activity_ms: i64,
    deck: Option<SetDeck>,
    /// Whether the opponent and deck use were reported.
    announced: bool,
    /// Time of the match's latest event, in log order (not the largest).
    previous_event_ms: Option<i64>,
    /// How the match was settled by silence, fixed for good once set.
    quiet: Option<QuietSettlement>,
}

/// A match that went quiet for `ABANDON_AFTER_MS` while ended or decided
/// (MTGO's quiet settlement): complete by quiet with the outcome and end time
/// of the events before the silence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct QuietSettlement {
    outcome: Outcome,
    ended_at_ms: i64,
}

/// Everything read about one game so far.
#[derive(Debug, Clone, Default)]
struct ArenaGame {
    first_turn_seat: Option<u32>,
    mulligans: Option<u32>,
    kept: bool,
    started_at_ms: Option<i64>,
    ended_at_ms: Option<i64>,
}

impl ArenaProjector {
    pub fn new() -> Self {
        Self::default()
    }

    /// Folds one event; returns what it changed.
    pub fn apply(&mut self, input: &ArenaInput) -> ArenaChanges {
        let mut changes = ArenaChanges::default();
        let touched = match &input.event {
            ArenaEvent::DetailedLogs { .. }
            | ArenaEvent::DeckSubmitted { .. }
            | ArenaEvent::Courses { .. } => None,
            ArenaEvent::Login { display_name } => {
                self.login(display_name, None);
                self.follow_rename();
                self.login_file = Some(input.file_id);
                changes.rank = self.rank.release(true);
                self.current.clone()
            }
            ArenaEvent::Authenticated {
                user_id,
                screen_name: Some(screen_name),
            } if screen_name.contains('#') => {
                // A fast login writes no login line: this names the account.
                self.login(screen_name, Some(user_id));
                self.follow_rename();
                self.login_file = Some(input.file_id);
                changes.rank = self.rank.release(true);
                self.current.clone()
            }
            ArenaEvent::Authenticated {
                user_id,
                screen_name,
            } => {
                self.authenticated_untagged(user_id, screen_name.as_deref());
                self.follow_rename();
                self.login_file = Some(input.file_id);
                changes.rank = self.rank.release(true);
                self.current.clone()
            }
            ArenaEvent::DeckSet {
                event_name,
                deck_id,
                deck_name,
                main,
                side,
                commanders,
                format,
                ..
            } => {
                self.pending_deck = Some(SetDeck {
                    event_name: event_name.clone(),
                    deck_id: deck_id.clone(),
                    deck_name: deck_name.clone(),
                    main: card_pairs(main),
                    side: card_pairs(side),
                    commanders: commanders.clone(),
                    format: format.clone(),
                });
                None
            }
            ArenaEvent::RoomState {
                match_id,
                completed,
                players,
                results,
                at_ms,
            } => {
                let time = at_ms.unwrap_or(input.at_ms);
                let found = self.match_for(match_id, time);
                found.room_state(*completed, players, results, time);
                Some(match_id.clone())
            }
            ArenaEvent::GameInfo {
                match_id,
                game_number,
                over,
                best_of_three,
                win_condition,
                results,
                at_ms,
            } => {
                let time = if *at_ms > 0 { *at_ms } else { input.at_ms };
                if *game_number > MAX_GAME_NUMBER {
                    return changes;
                }
                let Some(token) = self.named_or_current(match_id) else {
                    return changes;
                };
                let found = self.match_for(&token, time);
                found.note_kind(*best_of_three, win_condition.as_deref());
                found.game_info(*game_number, *over, results, time);
                Some(token)
            }
            ArenaEvent::FirstTurn {
                match_id,
                game_number,
                active_seat,
            } => {
                if *game_number > MAX_GAME_NUMBER {
                    return changes;
                }
                let token = self.named_or_current(match_id);
                if let Some(found) = token.as_ref().and_then(|t| self.matches.get_mut(t)) {
                    found.first_turn(*game_number, *active_seat, input.at_ms);
                }
                token
            }
            ArenaEvent::Mulligan { seat, kept, at_ms } => {
                let token = self.current.clone();
                if let Some(found) = token.as_ref().and_then(|t| self.matches.get_mut(t)) {
                    found.mulligan(*seat, *kept, at_ms.unwrap_or(input.at_ms));
                }
                token
            }
            ArenaEvent::Rank {
                constructed,
                limited,
            } => {
                let points = vec![
                    rank_point(input, Ladder::Constructed, constructed),
                    rank_point(input, Ladder::Limited, limited),
                ];
                changes.rank = self.rank.observe(points, self.logged_in_for(input.file_id));
                None
            }
            ArenaEvent::Season { ordinal, .. } => {
                changes.rank = self
                    .rank
                    .season(*ordinal, self.logged_in_for(input.file_id));
                None
            }
        };
        if let Some(token) = touched {
            self.refresh(&token, &mut changes);
        }
        changes
    }

    /// Applies the time rules as of `now_ms` (as MTGO's `finalize`): a match
    /// quiet for `ABANDON_AFTER_MS` with no match result is settled (decided
    /// or ended: complete by quiet; otherwise abandoned). Returns the tokens
    /// whose view changed, in token order.
    pub fn finalize(&mut self, now_ms: i64) -> Vec<String> {
        self.now_ms = Some(now_ms);
        let mut tokens: Vec<String> = self.views.keys().cloned().collect();
        tokens.sort();
        let mut changed = Vec::new();
        for token in tokens {
            let Some(view) = self.matches.get(&token).and_then(|m| m.view(self.now_ms)) else {
                continue;
            };
            if self.views.get(&token) != Some(&view) {
                self.views.insert(token.clone(), view);
                changed.push(token);
            }
        }
        changed
    }

    /// The match's current view: `None` until it is known to be a Bo1 or
    /// Bo3 of a tracked event (a constructed queue, a draft or sealed) with the local player found.
    pub fn view(&self, token: &str) -> Option<&MatchView> {
        self.views.get(token)
    }

    /// The logged in account, once a login line or a named
    /// `authenticateResponse` was seen.
    pub fn account(&self) -> Option<&ArenaIdentity> {
        self.identity.as_ref()
    }

    /// The log file the account last logged in from. A new `Player.log` is
    /// a new launch, maybe of another account, so a reading that names no
    /// account (a courses response) belongs to the account only once that
    /// file logs in.
    pub fn login_file(&self) -> Option<u64> {
        self.login_file
    }

    /// Whether a reading from `file_id` that names no account belongs to the
    /// logged in account: one logged in from that file or a later one.
    fn logged_in_for(&self, file_id: u64) -> bool {
        self.identity.is_some() && self.login_file.is_some_and(|login| login >= file_id)
    }

    /// A login line (`user_id` `None`) or a fast login's tagged
    /// `authenticateResponse` (`user_id` its `clientId`). The same account
    /// logging in again keeps its identity (taking the user id when given).
    /// An account known only by its user id takes the tagged name when its
    /// untagged name is the name before the tag, or is not known (the login
    /// line names the account a user id was read for first). Any other name
    /// is another account, with the given user id.
    fn login(&mut self, display_name: &str, user_id: Option<&str>) {
        if let Some(identity) = &mut self.identity {
            let base = display_name.split('#').next().unwrap_or(display_name);
            let same = identity.username == display_name
                || (identity.is_untagged()
                    && identity
                        .display_name
                        .as_deref()
                        .is_none_or(|name| name == base));
            if same {
                identity.username = display_name.to_string();
                identity.display_name = None;
                if let Some(user_id) = user_id {
                    identity.user_id = Some(user_id.to_string());
                }
                return;
            }
        }
        self.identity = Some(ArenaIdentity {
            username: display_name.to_string(),
            user_id: user_id.map(str::to_string),
            display_name: None,
        });
    }

    /// An `authenticateResponse` without a tagged screen name (trap 29): the
    /// account logged in gets the user id. With no account yet, or another
    /// account's user id, the account is known by its user id until a login
    /// line names it, shown by the untagged screen name.
    fn authenticated_untagged(&mut self, user_id: &str, screen_name: Option<&str>) {
        let screen_name = screen_name.filter(|name| !name.is_empty());
        if let Some(identity) = &mut self.identity
            && identity.user_id.as_deref().is_none_or(|id| id == user_id)
        {
            identity.user_id = Some(user_id.to_string());
            if identity.is_untagged() && screen_name.is_some() {
                identity.display_name = screen_name.map(str::to_string);
            }
            return;
        }
        self.identity = Some(ArenaIdentity {
            username: format!("{UNTAGGED_PREFIX}{user_id}"),
            user_id: Some(user_id.to_string()),
            display_name: screen_name.map(str::to_string),
        });
    }

    /// A rename (#862): each match whose local seat has the logged in
    /// account's user id, under another name, takes the account's current
    /// name, so a later write of it (a sweep, a rebuild) names the renamed
    /// account and never makes one under the old name. The matches are not
    /// reported changed: a stored row already points at the account row the
    /// rename moves, and the tracker renames the writes it still holds. An
    /// untagged name (trap 29) is no rename: it would be undone by the next
    /// tagged login.
    fn follow_rename(&mut self) {
        let Some(identity) = &self.identity else {
            return;
        };
        if identity.is_untagged() {
            return;
        }
        let Some(user_id) = identity.user_id.clone() else {
            return;
        };
        let username = identity.username.clone();
        for (token, found) in &mut self.matches {
            let Some(local) = &mut found.local else {
                continue;
            };
            if local.username == username {
                continue;
            }
            let seat = local.seat;
            if found
                .seats
                .iter()
                .any(|s| s.seat == seat && s.user_id == user_id)
            {
                local.username = username.clone();
                if let Some(view) = self.views.get_mut(token) {
                    view.local_username = username.clone();
                }
            }
        }
    }

    /// `match_id`, or the current match when the event names none.
    fn named_or_current(&self, match_id: &str) -> Option<String> {
        if match_id.is_empty() {
            self.current.clone()
        } else {
            Some(match_id.to_string())
        }
    }

    /// The match `token`, made current; a new match starts at `time` with the
    /// latest deck set before it.
    fn match_for(&mut self, token: &str, time: i64) -> &mut ArenaMatch {
        self.current = Some(token.to_string());
        let pending_deck = self.pending_deck.clone();
        self.matches
            .entry(token.to_string())
            .or_insert_with(|| ArenaMatch {
                token: token.to_string(),
                started_at_ms: time,
                last_activity_ms: time,
                deck: pending_deck,
                ..ArenaMatch::default()
            })
    }

    /// Finds the local side if still unknown, recomputes the view and records
    /// what changed.
    fn refresh(&mut self, token: &str, changes: &mut ArenaChanges) {
        let Some(found) = self.matches.get_mut(token) else {
            return;
        };
        if found.local.is_none() {
            found.local = find_local(&found.seats, self.identity.as_ref());
        }
        // An account known only by its user id is shown by its seat's name.
        if let (Some(identity), Some(local)) = (&mut self.identity, &found.local)
            && identity.is_untagged()
            && identity.display_name.is_none()
            && let Some(seat) = found.seats.iter().find(|seat| seat.seat == local.seat)
            && !seat.name.is_empty()
        {
            identity.display_name = Some(seat.name.clone());
        }
        let Some(view) = found.view(self.now_ms) else {
            return;
        };
        if !found.announced {
            found.announced = true;
            if let Some((key, name)) = found.opponent() {
                changes.opponents.push((key, name));
            }
            if let Some(deck_use) = found.deck_use(&view) {
                changes.deck_uses.push(deck_use);
            }
        }
        if self.views.get(token) != Some(&view) {
            self.views.insert(token.to_string(), view);
            changes.matches.push(token.to_string());
        }
    }
}

impl ArenaMatch {
    /// Notes an event of this match. As in MTGO (`quiet_points`), a gap of
    /// `ABANDON_AFTER_MS` or more since the match's previous event is a quiet
    /// point: if the match was ended or decided then, and no match result
    /// came first, it is settled by quiet for good, before this event counts.
    fn touch(&mut self, time: i64) {
        if let Some(previous) = self.previous_event_ms
            && time - previous >= ABANDON_AFTER_MS
            && self.quiet.is_none()
            && self.match_result().is_none()
        {
            self.quiet = self.quiet_settlement();
        }
        self.previous_event_ms = Some(time);
        self.last_activity_ms = self.last_activity_ms.max(time);
    }

    /// The settlement if the match went quiet now: an ended match completes
    /// from its score, a score at the win threshold completes with it.
    /// `None` when neither (the match is then abandoned, which later events
    /// undo, as in MTGO).
    fn quiet_settlement(&self) -> Option<QuietSettlement> {
        let local = self.local.as_ref()?;
        let kind = self.kind(&local.event_id)?;
        let (wins, losses) = self.score(local.team);
        let outcome = if self.completed_at_ms.is_some() {
            outcome_from(wins, losses)
        } else {
            kind.decided_outcome(wins, losses)?
        };
        Some(QuietSettlement {
            outcome,
            ended_at_ms: self.last_activity_ms,
        })
    }

    /// Games won and lost by `team`.
    fn score(&self, team: u32) -> (u32, u32) {
        let mut wins = 0;
        let mut losses = 0;
        for entry in self
            .game_results()
            .into_iter()
            .take(MAX_GAME_NUMBER as usize)
        {
            match game_result(entry, team) {
                GameResult::Win => wins += 1,
                GameResult::Loss => losses += 1,
                GameResult::Unknown => {}
            }
        }
        (wins, losses)
    }

    fn room_state(
        &mut self,
        completed: bool,
        players: &[Seat],
        results: &[ResultEntry],
        time: i64,
    ) {
        self.touch(time);
        if !players.is_empty() {
            self.seats = players.to_vec();
        }
        self.take_results(results, time);
        if completed && self.completed_at_ms.is_none() {
            self.completed_at_ms = Some(time);
        }
    }

    fn game_info(&mut self, game_number: u32, over: bool, results: &[ResultEntry], time: i64) {
        self.touch(time);
        if game_number > 0 {
            self.current_game = self.current_game.max(game_number);
            let game = self.game_mut(game_number);
            game.started_at_ms.get_or_insert(time);
            if over {
                game.ended_at_ms.get_or_insert(time);
            }
        }
        self.take_results(results, time);
    }

    /// The first turn 1 per game counts (it repeats on every turn 1 diff);
    /// game 0 means the current game.
    fn first_turn(&mut self, game_number: u32, active_seat: u32, time: i64) {
        let number = if game_number == 0 {
            self.current_game
        } else {
            game_number
        };
        if number == 0 {
            return;
        }
        self.touch(time);
        self.game_mut(number)
            .first_turn_seat
            .get_or_insert(active_seat);
    }

    /// A local mulligan decision for the current game (seat 0 is the local
    /// player; another seat's decisions are not counted).
    fn mulligan(&mut self, seat: u32, kept: bool, time: i64) {
        let local_seat = self.local.as_ref().map(|local| local.seat);
        if self.current_game == 0 || (seat != 0 && Some(seat) != local_seat) {
            return;
        }
        self.touch(time);
        let game = self.game_mut(self.current_game);
        if game.kept {
            return;
        }
        let count = game.mulligans.unwrap_or(0);
        if kept {
            game.kept = true;
            game.mulligans = Some(count);
        } else {
            game.mulligans = Some(count + 1);
        }
    }

    /// A win condition fixes the kind once; a game info without one (stored
    /// before it was read) only feeds the fallback in [`Self::kind`].
    fn note_kind(&mut self, best_of_three: bool, win_condition: Option<&str>) {
        if self.kind.is_some() {
            return;
        }
        match win_condition {
            // An unknown win condition leaves the kind unknown: the match gets no view.
            Some(win_condition) => self.kind = Kind::from_win_condition(win_condition),
            None => {
                let before = self.stored_best_of_three == Some(true);
                self.stored_best_of_three = Some(before || best_of_three);
            }
        }
    }

    /// The match's kind. Without a win condition (old stored events),
    /// `best_of_three` true is Bo3, and false is Bo1 only on a queue without
    /// the `Traditional_` prefix (Bo3 queues all have it); else unknown.
    fn kind(&self, event_id: &str) -> Option<Kind> {
        if self.kind.is_some() {
            return self.kind;
        }
        match self.stored_best_of_three? {
            true => Some(Kind::Bo3),
            false if !event_id.starts_with("Traditional_") => Some(Kind::Bo1),
            false => None,
        }
    }

    /// Result lists are cumulative: a list at least as long as the one held
    /// replaces it.
    fn take_results(&mut self, results: &[ResultEntry], time: i64) {
        if results.len() >= self.results.len() {
            self.results = results.to_vec();
        }
        if self.decided_at_ms.is_none() && self.match_result().is_some() {
            self.decided_at_ms = Some(time);
        }
    }

    fn game_mut(&mut self, number: u32) -> &mut ArenaGame {
        while self.games.len() < number as usize {
            self.games.push(ArenaGame::default());
        }
        &mut self.games[number as usize - 1]
    }

    fn match_result(&self) -> Option<&ResultEntry> {
        self.results
            .iter()
            .find(|entry| entry.scope == Scope::Match)
    }

    fn game_results(&self) -> Vec<&ResultEntry> {
        self.results
            .iter()
            .filter(|entry| entry.scope == Scope::Game)
            .collect()
    }

    fn opponent_seat(&self) -> Option<&Seat> {
        let local = self.local.as_ref()?;
        self.seats.iter().find(|seat| seat.seat != local.seat)
    }

    /// `(opponent_key, display name)`.
    fn opponent(&self) -> Option<(String, String)> {
        let seat = self.opponent_seat()?;
        Some((opponent_key(&seat.user_id), seat.name.clone()))
    }

    /// The deck set before the match, when it was set for the match's event.
    /// A limited event's deck is its run's, never a constructed deck.
    fn deck_use(&self, view: &MatchView) -> Option<DeckUse> {
        let deck = self.deck.as_ref()?;
        let local = self.local.as_ref()?;
        if deck.event_name != local.event_id
            || parse_event_name(&local.event_id).is_some_and(|event| event.limited)
        {
            return None;
        }
        Some(DeckUse {
            match_token: self.token.clone(),
            deck_id: deck.deck_id.clone(),
            deck_name: deck.deck_name.clone(),
            format_code: view.format.clone().unwrap_or_default(),
            main: deck.main.clone(),
            side: deck.side.clone(),
            commanders: deck.commanders.clone(),
        })
    }

    /// The format of the view. A commander event takes the format of the
    /// deck set for it (a Brawl queue can hold either Brawl), else the event
    /// name's; every other event keeps the event name's.
    fn view_format(&self, local: &LocalSide, event: &EventName) -> String {
        if !event.commander {
            return event.format_code.clone();
        }
        self.deck
            .as_ref()
            .filter(|deck| deck.event_name == local.event_id)
            .and_then(|deck| deck.format.as_deref())
            .and_then(commander_format_code)
            .unwrap_or_else(|| event.format_code.clone())
    }

    /// The view as of `now_ms` (the latest `finalize`, if any).
    fn view(&self, now_ms: Option<i64>) -> Option<MatchView> {
        let local = self.local.as_ref()?;
        let kind = self.kind(&local.event_id)?;
        let event = parse_event_name(&local.event_id)?;
        let (opponent, _) = self.opponent()?;

        let game_results = self.game_results();
        let game_count = self
            .games
            .len()
            .max(game_results.len())
            .min(MAX_GAME_NUMBER as usize);
        let mut games = Vec::new();
        for index in 0..game_count {
            let game = self.games.get(index).cloned().unwrap_or_default();
            let result = game_results.get(index).copied();
            games.push(GameView {
                mtgo_game_id: None,
                game_number: index as u32 + 1,
                result: result.map_or(GameResult::Unknown, |r| game_result(r, local.team)),
                end_reason: result.map_or(EndReason::Unknown, |r| end_reason(&r.reason)),
                on_play: game.first_turn_seat.map(|seat| seat == local.seat),
                local_mulligans: game.mulligans,
                opp_mulligans: None,
                local_die_roll: None,
                opp_die_roll: None,
                turn_count: None,
                started_at_ms: game.started_at_ms,
                ended_at_ms: game.ended_at_ms,
            });
        }
        let wins = games.iter().filter(|g| g.result == GameResult::Win).count() as u32;
        let losses = games
            .iter()
            .filter(|g| g.result == GameResult::Loss)
            .count() as u32;

        // Settled matches never change (MTGO, owner decision 2026-10-07): a
        // quiet point between events, else the match result. Then the
        // trailing silence up to `now`, which later events can still undo.
        let quiet_now = now_ms.is_some_and(|now| now - self.last_activity_ms >= ABANDON_AFTER_MS);
        let settled_now = if quiet_now {
            self.quiet_settlement()
        } else {
            None
        };
        let (state, resolution, outcome, ended_at_ms) = match (self.quiet, self.match_result()) {
            (Some(quiet), _) => (
                MatchState::Complete,
                Some(Resolution::Quiet),
                quiet.outcome,
                Some(quiet.ended_at_ms),
            ),
            (None, Some(entry)) => (
                MatchState::Complete,
                Some(Resolution::Signal),
                match_outcome(entry, local.team),
                self.completed_at_ms.or(self.decided_at_ms),
            ),
            (None, None) => match settled_now {
                Some(quiet) => (
                    MatchState::Complete,
                    Some(Resolution::Quiet),
                    quiet.outcome,
                    Some(quiet.ended_at_ms),
                ),
                None if self.completed_at_ms.is_some() => (
                    MatchState::Ended,
                    Some(Resolution::Signal),
                    Outcome::Unknown,
                    self.completed_at_ms,
                ),
                None if quiet_now => (
                    MatchState::Abandoned,
                    Some(Resolution::Abandoned),
                    Outcome::Unknown,
                    Some(self.last_activity_ms),
                ),
                None => (MatchState::InProgress, None, Outcome::Unknown, None),
            },
        };
        let tentative_outcome = if state == MatchState::InProgress {
            kind.decided_outcome(wins, losses)
        } else {
            None
        };

        Some(MatchView {
            token: self.token.clone(),
            mtgo_match_id: None,
            local_username: local.username.clone(),
            opponent: Some(opponent),
            match_type: Some(match_type(&event).to_string()),
            format: Some(self.view_format(local, &event)),
            tournament_event_id: None,
            tournament_round: None,
            league_token: None,
            game_structure: Some(kind.game_structure().to_string()),
            state,
            resolution,
            outcome,
            tentative_outcome,
            wins,
            losses,
            started_at_ms: self.started_at_ms,
            ended_at_ms,
            last_activity_ms: self.last_activity_ms,
            games,
        })
    }
}

/// `ranked` and `unranked` for constructed queues; `limited` for a draft or
/// sealed event, so it never takes a constructed rank reading or counts as a
/// ranked or casual match.
fn match_type(event: &EventName) -> &'static str {
    match (event.limited, event.ranked) {
        (true, _) => "limited",
        (false, true) => "ranked",
        (false, false) => "unranked",
    }
}

/// The local seat: the one holding the authenticated user id, or before an
/// authentication the one named like the login without its `#` tag.
fn find_local(seats: &[Seat], identity: Option<&ArenaIdentity>) -> Option<LocalSide> {
    let identity = identity?;
    let seat = match &identity.user_id {
        Some(user_id) => seats.iter().find(|seat| &seat.user_id == user_id)?,
        None => {
            let name = identity
                .username
                .split('#')
                .next()
                .unwrap_or(&identity.username);
            seats.iter().find(|seat| seat.name == name)?
        }
    };
    Some(LocalSide {
        username: identity.username.clone(),
        seat: seat.seat,
        team: seat.team,
        event_id: seat.event_id.clone(),
    })
}

fn card_pairs(cards: &[CardCount]) -> Vec<(u32, u32)> {
    cards
        .iter()
        .map(|card| (card.grp_id, card.quantity))
        .collect()
}

fn rank_point(input: &ArenaInput, ladder: Ladder, reading: &RankReading) -> RankPoint {
    RankPoint {
        ladder,
        reading: reading.clone(),
        at_ms: input.at_ms,
        file_id: input.file_id,
        offset: input.offset,
        seq: input.seq,
    }
}

fn game_result(entry: &ResultEntry, local_team: u32) -> GameResult {
    match entry.winning_team {
        Some(team) if team == local_team => GameResult::Win,
        Some(_) => GameResult::Loss,
        None => GameResult::Unknown,
    }
}

fn match_outcome(entry: &ResultEntry, local_team: u32) -> Outcome {
    match entry.winning_team {
        Some(team) if team == local_team => Outcome::Win,
        Some(_) => Outcome::Loss,
        None => Outcome::Draw,
    }
}

/// The result reason without `ResultReason_`: `Game` is a game played out,
/// `Concede` a concession; anything else is not known yet.
fn end_reason(reason: &str) -> EndReason {
    match reason.strip_prefix("ResultReason_").unwrap_or(reason) {
        "Game" => EndReason::Win,
        "Concede" => EndReason::Concede,
        _ => EndReason::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MATCH: &str = "match-1";
    const LOCAL_ID: &str = "USER_LOCAL";
    const OPP_ID: &str = "USER_OPP";
    const LADDER: &str = "Traditional_Explorer_Ladder";

    /// Feeds events in order (offsets 10, 20, ...); returns every change.
    fn run(projector: &mut ArenaProjector, events: Vec<ArenaEvent>) -> Vec<ArenaChanges> {
        let mut changes = Vec::new();
        for (index, event) in events.into_iter().enumerate() {
            let offset = (index as u64 + 1) * 10;
            changes.push(projector.apply(&ArenaInput {
                file_id: 1,
                offset,
                seq: 0,
                at_ms: 1_000_000 + offset as i64,
                event,
            }));
        }
        changes
    }

    fn all_matches(changes: &[ArenaChanges]) -> Vec<String> {
        changes.iter().flat_map(|c| c.matches.clone()).collect()
    }

    fn all_rank(changes: &[ArenaChanges]) -> Vec<RankPoint> {
        changes.iter().flat_map(|c| c.rank.clone()).collect()
    }

    fn login() -> ArenaEvent {
        ArenaEvent::Login {
            display_name: "Player#00001".to_string(),
        }
    }

    fn authenticated() -> ArenaEvent {
        ArenaEvent::Authenticated {
            user_id: LOCAL_ID.to_string(),
            screen_name: None,
        }
    }

    /// The `authenticateResponse` of a fast login: it names the account.
    fn fast_login() -> ArenaEvent {
        ArenaEvent::Authenticated {
            user_id: LOCAL_ID.to_string(),
            screen_name: Some("Player#00001".to_string()),
        }
    }

    fn seat(seat: u32, user_id: &str, name: &str, event_id: &str) -> Seat {
        Seat {
            seat,
            team: seat,
            user_id: user_id.to_string(),
            name: name.to_string(),
            event_id: event_id.to_string(),
        }
    }

    fn room(event_id: &str, completed: bool, results: Vec<ResultEntry>) -> ArenaEvent {
        ArenaEvent::RoomState {
            match_id: MATCH.to_string(),
            completed,
            players: vec![
                seat(1, LOCAL_ID, "Player", event_id),
                seat(2, OPP_ID, "Opponent", event_id),
            ],
            results,
            at_ms: Some(5_000),
        }
    }

    const BO3: &str = "MatchWinCondition_Best2of3";
    const BO1: &str = "MatchWinCondition_SingleElimination";
    const BO1_PLAY: &str = "Explorer_Play";

    /// A game info as the log writes it: `bo3` picks the win condition.
    fn game_info(number: u32, over: bool, bo3: bool, results: Vec<ResultEntry>) -> ArenaEvent {
        let condition = if bo3 { BO3 } else { BO1 };
        game_info_stored(number, over, bo3, Some(condition), results)
    }

    /// A game info with any win condition, `None` as stored before the
    /// field was decoded.
    fn game_info_stored(
        number: u32,
        over: bool,
        best_of_three: bool,
        win_condition: Option<&str>,
        results: Vec<ResultEntry>,
    ) -> ArenaEvent {
        ArenaEvent::GameInfo {
            match_id: MATCH.to_string(),
            game_number: number,
            over,
            best_of_three,
            win_condition: win_condition.map(str::to_string),
            results,
            at_ms: 6_000 + i64::from(number),
        }
    }

    fn game(team: u32) -> ResultEntry {
        ResultEntry {
            scope: Scope::Game,
            winning_team: Some(team),
            reason: "ResultReason_Game".to_string(),
        }
    }

    fn match_won_by(team: u32) -> ResultEntry {
        ResultEntry {
            scope: Scope::Match,
            winning_team: Some(team),
            reason: "ResultReason_Concede".to_string(),
        }
    }

    fn deck_set() -> ArenaEvent {
        ArenaEvent::DeckSet {
            event_name: LADDER.to_string(),
            deck_id: "deck-guid".to_string(),
            deck_name: "Test deck".to_string(),
            main: vec![arena_protocol::CardCount {
                grp_id: 71291,
                quantity: 4,
            }],
            side: Vec::new(),
            companion: None,
            commanders: Vec::new(),
            format: None,
        }
    }

    fn reading(season: u32) -> RankReading {
        RankReading {
            season,
            class: "Bronze".to_string(),
            level: 4,
            step: 0,
            won: 0,
            lost: 0,
            mythic_percentile: None,
            mythic_placement: None,
        }
    }

    fn rank(season: u32) -> ArenaEvent {
        ArenaEvent::Rank {
            constructed: reading(season),
            limited: reading(season),
        }
    }

    fn season(ordinal: u32) -> ArenaEvent {
        ArenaEvent::Season {
            ordinal,
            start_text: String::new(),
            end_text: String::new(),
        }
    }

    /// A finished match played as `Player#00001`.
    fn played_match() -> Vec<ArenaEvent> {
        vec![
            login(),
            authenticated(),
            room(BO1_PLAY, false, Vec::new()),
            game_info(1, false, false, Vec::new()),
            game_info(1, true, false, vec![game(1), match_won_by(1)]),
            room(BO1_PLAY, true, vec![game(1), match_won_by(1)]),
        ]
    }

    /// #862: a rename gives the account's matches the new name without
    /// reporting them changed: a stored match's row already points at the
    /// account row the rename moves, so nothing is written or synced again.
    #[test]
    fn a_rename_names_the_matches_without_changing_them() {
        let mut projector = ArenaProjector::new();
        run(&mut projector, played_match());
        let changes = run(
            &mut projector,
            vec![
                ArenaEvent::Login {
                    display_name: "Qrenamed#00002".to_string(),
                },
                authenticated(),
            ],
        );
        assert_eq!(all_matches(&changes), Vec::<String>::new());
        let view = projector.view(MATCH).expect("view");
        assert_eq!(view.local_username, "Qrenamed#00002");
    }

    /// #862: an untagged `authenticateResponse` (trap 29) after another
    /// account's login is no rename: the matches keep their tagged name, and
    /// the tagged login after it changes nothing either.
    #[test]
    fn an_untagged_authenticate_renames_nothing() {
        let mut projector = ArenaProjector::new();
        run(&mut projector, played_match());
        run(
            &mut projector,
            vec![
                ArenaEvent::Login {
                    display_name: "Qother#00003".to_string(),
                },
                untagged_login("USER_OTHER", "Qother"),
                untagged_login(LOCAL_ID, "Player"),
            ],
        );
        assert_eq!(
            projector.account().map(|a| a.username.as_str()),
            Some(format!("arena:{LOCAL_ID}").as_str())
        );
        assert_eq!(
            projector.view(MATCH).expect("view").local_username,
            "Player#00001"
        );
        let changes = run(&mut projector, vec![login()]);
        assert_eq!(all_matches(&changes), Vec::<String>::new());
        assert_eq!(
            projector.view(MATCH).expect("view").local_username,
            "Player#00001"
        );
    }

    #[test]
    fn a_bo1_match_has_a_bo1_view() {
        let mut projector = ArenaProjector::new();
        let changes = run(
            &mut projector,
            vec![
                login(),
                authenticated(),
                room(BO1_PLAY, false, Vec::new()),
                game_info(1, false, false, Vec::new()),
                game_info(1, true, false, vec![game(1), match_won_by(1)]),
                room(BO1_PLAY, true, vec![game(1), match_won_by(1)]),
            ],
        );
        assert_eq!(
            all_matches(&changes).first().map(String::as_str),
            Some(MATCH)
        );
        let view = projector.view(MATCH).expect("a bo1 match has a view");
        assert_eq!(view.game_structure.as_deref(), Some("bo1"));
        assert_eq!(view.games.len(), 1);
        assert_eq!(view.outcome, Outcome::Win);
        assert_eq!(view.state, MatchState::Complete);
        assert_eq!(view.match_type.as_deref(), Some("unranked"));
        assert_eq!(view.format.as_deref(), Some("CPIONEER"));
    }

    #[test]
    fn a_fast_login_session_finds_the_account() {
        let mut projector = ArenaProjector::new();
        run(
            &mut projector,
            vec![
                fast_login(),
                room(BO1_PLAY, false, Vec::new()),
                game_info(1, true, false, vec![game(1), match_won_by(1)]),
                room(BO1_PLAY, true, vec![game(1), match_won_by(1)]),
            ],
        );
        let view = projector
            .view(MATCH)
            .expect("a fast login finds the account");
        assert_eq!(view.local_username, "Player#00001");
        assert_eq!(view.outcome, Outcome::Win);
        let account = projector.account().expect("an account");
        assert_eq!(account.user_id.as_deref(), Some(LOCAL_ID));
    }

    #[test]
    fn a_fast_login_releases_held_rank() {
        let mut projector = ArenaProjector::new();
        let changes = run(&mut projector, vec![season(95), rank(95), fast_login()]);
        assert!(changes[1].rank.is_empty(), "no account owns it yet");
        assert_eq!(changes[2].rank.len(), 2);
    }

    #[test]
    fn a_login_line_and_an_authenticated_agree() {
        for events in [
            vec![login(), fast_login()],
            vec![fast_login(), login()],
            vec![authenticated(), login(), fast_login()],
        ] {
            let mut projector = ArenaProjector::new();
            run(&mut projector, events);
            let account = projector.account().expect("one account");
            assert_eq!(account.username, "Player#00001");
            assert_eq!(account.user_id.as_deref(), Some(LOCAL_ID));
        }
    }

    /// The `authenticateResponse` of a fast login whose screen name has no
    /// `#` tag (trap 29).
    fn untagged_login(user_id: &str, name: &str) -> ArenaEvent {
        ArenaEvent::Authenticated {
            user_id: user_id.to_string(),
            screen_name: Some(name.to_string()),
        }
    }

    #[test]
    fn an_untagged_fast_login_is_known_by_its_user_id() {
        let mut projector = ArenaProjector::new();
        let changes = run(
            &mut projector,
            vec![
                season(95),
                rank(95),
                untagged_login(LOCAL_ID, "Player"),
                room(BO1_PLAY, false, Vec::new()),
                game_info(1, true, false, vec![game(1), match_won_by(1)]),
                room(BO1_PLAY, true, vec![game(1), match_won_by(1)]),
            ],
        );
        let account = projector.account().expect("an account");
        assert_eq!(account.username, format!("arena:{LOCAL_ID}"));
        assert_eq!(account.user_id.as_deref(), Some(LOCAL_ID));
        assert_eq!(account.display_name.as_deref(), Some("Player"));
        assert_eq!(changes[2].rank.len(), 2, "the held rank is released");
        let view = projector.view(MATCH).expect("the match has a view");
        assert_eq!(view.local_username, format!("arena:{LOCAL_ID}"));
        assert_eq!(view.outcome, Outcome::Win);
    }

    #[test]
    fn a_nameless_authenticated_takes_its_name_from_the_seat() {
        // Events stored before untagged names were decoded carry none.
        let mut projector = ArenaProjector::new();
        run(
            &mut projector,
            vec![authenticated(), room(BO1_PLAY, false, Vec::new())],
        );
        let account = projector.account().expect("an account");
        assert_eq!(account.username, format!("arena:{LOCAL_ID}"));
        assert_eq!(account.display_name.as_deref(), Some("Player"));
    }

    #[test]
    fn a_login_line_names_the_untagged_account() {
        for events in [
            vec![untagged_login(LOCAL_ID, "Player"), login()],
            vec![login(), untagged_login(LOCAL_ID, "Player")],
            vec![authenticated(), login()],
        ] {
            let mut projector = ArenaProjector::new();
            run(&mut projector, events);
            let account = projector.account().expect("one account");
            assert_eq!(account.username, "Player#00001");
            assert_eq!(account.user_id.as_deref(), Some(LOCAL_ID));
            assert_eq!(account.display_name, None);
        }
    }

    #[test]
    fn a_login_line_with_another_name_is_another_account() {
        let mut projector = ArenaProjector::new();
        run(
            &mut projector,
            vec![untagged_login("USER_OTHER", "Other"), login()],
        );
        let account = projector.account().expect("an account");
        assert_eq!(account.username, "Player#00001");
        assert_eq!(account.user_id, None, "the other account's id is not taken");
    }

    #[test]
    fn an_untagged_fast_login_to_another_account_switches_account() {
        let mut projector = ArenaProjector::new();
        run(
            &mut projector,
            vec![
                login(),
                authenticated(),
                untagged_login("USER_OTHER", "Other"),
            ],
        );
        let account = projector.account().expect("an account");
        assert_eq!(account.username, "arena:USER_OTHER");
        assert_eq!(account.display_name.as_deref(), Some("Other"));
    }

    #[test]
    fn a_fast_login_to_another_account_switches_account() {
        let mut projector = ArenaProjector::new();
        let other = ArenaEvent::Authenticated {
            user_id: "USER_OTHER".to_string(),
            screen_name: Some("Other#00002".to_string()),
        };
        run(&mut projector, vec![login(), authenticated(), other]);
        let account = projector.account().expect("an account");
        assert_eq!(account.username, "Other#00002");
        assert_eq!(account.user_id.as_deref(), Some("USER_OTHER"));
    }

    #[test]
    fn a_bo3_match_keeps_bo3() {
        let mut projector = ArenaProjector::new();
        run(
            &mut projector,
            vec![
                login(),
                authenticated(),
                room(LADDER, false, Vec::new()),
                game_info(1, false, true, Vec::new()),
                game_info(1, true, true, vec![game(1)]),
                game_info(1, true, true, vec![game(1), match_won_by(2)]),
                room(LADDER, true, vec![game(1), match_won_by(2)]),
            ],
        );
        let view = projector.view(MATCH).expect("a bo3 match has a view");
        assert_eq!(view.game_structure.as_deref(), Some("bo3"));
    }

    #[test]
    fn a_bo1_at_one_nil_settles_by_quiet() {
        // One game won is the whole match in Bo1: the tentative outcome and
        // the quiet settlement need one win, not two.
        let mut projector = ArenaProjector::new();
        run(
            &mut projector,
            vec![
                login(),
                authenticated(),
                room(BO1_PLAY, false, Vec::new()),
                game_info(1, false, false, Vec::new()),
                game_info(1, true, false, vec![game(1)]),
            ],
        );
        let live = projector.view(MATCH).expect("view");
        assert_eq!(live.state, MatchState::InProgress);
        assert_eq!(live.tentative_outcome, Some(Outcome::Win));
        projector.finalize(6_001 + ABANDON_AFTER_MS);
        let view = projector.view(MATCH).expect("view");
        assert_eq!(view.state, MatchState::Complete);
        assert_eq!(view.resolution, Some(Resolution::Quiet));
        assert_eq!(view.outcome, Outcome::Win);
    }

    #[test]
    fn an_old_stored_game_info_is_bo1_only_without_the_traditional_prefix() {
        let old_match = |event_id: &str| {
            let mut projector = ArenaProjector::new();
            run(
                &mut projector,
                vec![
                    login(),
                    authenticated(),
                    room(event_id, false, Vec::new()),
                    game_info_stored(1, false, false, None, Vec::new()),
                    game_info_stored(1, true, false, None, vec![game(1), match_won_by(1)]),
                ],
            );
            projector.view(MATCH).cloned()
        };
        let bo1 = old_match("Explorer_Ladder").expect("a bo1 queue");
        assert_eq!(bo1.game_structure.as_deref(), Some("bo1"));
        assert_eq!(bo1.match_type.as_deref(), Some("ranked"));
        assert_eq!(old_match(LADDER), None);
    }

    #[test]
    fn a_later_game_info_without_the_field_keeps_bo3() {
        let mut projector = ArenaProjector::new();
        run(
            &mut projector,
            vec![
                login(),
                authenticated(),
                room(LADDER, false, Vec::new()),
                game_info(1, false, true, Vec::new()),
                game_info(1, true, true, vec![game(1)]),
                game_info_stored(2, false, false, None, vec![game(1)]),
            ],
        );
        let view = projector.view(MATCH).expect("view");
        assert_eq!(view.game_structure.as_deref(), Some("bo3"));
        assert_eq!(view.tentative_outcome, None);
    }

    #[test]
    fn direct_games_have_no_view() {
        for event_id in ["DirectGame", "DirectGameBrawl", "CubeDraft_Arena_20260101"] {
            let mut projector = ArenaProjector::new();
            let changes = run(
                &mut projector,
                vec![
                    login(),
                    authenticated(),
                    room(event_id, false, Vec::new()),
                    game_info(1, false, false, Vec::new()),
                    game_info(1, true, false, vec![game(1), match_won_by(1)]),
                    room(event_id, true, vec![game(1), match_won_by(1)]),
                ],
            );
            assert!(all_matches(&changes).is_empty(), "{event_id}");
            assert_eq!(projector.view(MATCH), None, "{event_id}");
        }
    }

    const BRAWL_QUEUE: &str = "Play_Brawl_Historic";
    const BRAWL_LADDER: &str = "Brawl_Ladder";

    fn brawl_deck(event_name: &str, format: Option<&str>) -> ArenaEvent {
        ArenaEvent::DeckSet {
            event_name: event_name.to_string(),
            deck_id: "brawl-deck".to_string(),
            deck_name: "Brawl deck".to_string(),
            main: vec![arena_protocol::CardCount {
                grp_id: 71291,
                quantity: 1,
            }],
            side: Vec::new(),
            companion: None,
            commanders: vec![83902],
            format: format.map(str::to_string),
        }
    }

    fn brawl_match_events(
        event_id: &str,
        format: Option<&str>,
        deck_event: &str,
    ) -> Vec<ArenaEvent> {
        vec![
            login(),
            authenticated(),
            brawl_deck(deck_event, format),
            room(event_id, false, Vec::new()),
            game_info(1, false, false, Vec::new()),
            game_info(1, true, false, vec![game(1), match_won_by(1)]),
            room(event_id, true, vec![game(1), match_won_by(1)]),
        ]
    }

    #[test]
    fn brawl_queue_match_is_unranked_historic_brawl() {
        let mut projector = ArenaProjector::new();
        let changes = run(
            &mut projector,
            brawl_match_events(BRAWL_QUEUE, Some("HistoricBrawl"), BRAWL_QUEUE),
        );
        let view = projector.view(MATCH).expect("a Brawl match has a view");
        assert_eq!(view.format.as_deref(), Some("ARENA_HISTORIC_BRAWL"));
        assert_eq!(view.match_type.as_deref(), Some("unranked"));
        assert_eq!(view.game_structure.as_deref(), Some("bo1"));
        let deck_uses: Vec<DeckUse> = changes.iter().flat_map(|c| c.deck_uses.clone()).collect();
        assert_eq!(deck_uses.len(), 1);
        assert_eq!(deck_uses[0].commanders, vec![83902]);
        assert_eq!(deck_uses[0].format_code, "ARENA_HISTORIC_BRAWL");
    }

    #[test]
    fn brawl_ladder_is_ranked_and_takes_the_deck_format() {
        let mut projector = ArenaProjector::new();
        run(
            &mut projector,
            brawl_match_events(BRAWL_LADDER, Some("HistoricBrawl"), BRAWL_LADDER),
        );
        let view = projector.view(MATCH).expect("a Brawl match has a view");
        assert_eq!(view.format.as_deref(), Some("ARENA_HISTORIC_BRAWL"));
        assert_eq!(view.match_type.as_deref(), Some("ranked"));
    }

    #[test]
    fn a_brawl_deck_format_wins_over_the_event_name() {
        let format_of = |format: Option<&str>| {
            let mut projector = ArenaProjector::new();
            run(
                &mut projector,
                brawl_match_events("Play_Brawl", format, "Play_Brawl"),
            );
            projector.view(MATCH).expect("view").format.clone()
        };
        assert_eq!(
            format_of(Some("HistoricBrawl")).as_deref(),
            Some("ARENA_HISTORIC_BRAWL")
        );
        assert_eq!(format_of(None).as_deref(), Some("ARENA_BRAWL"));
        assert_eq!(format_of(Some("Unknown")).as_deref(), Some("ARENA_BRAWL"));
    }

    #[test]
    fn a_deck_format_is_ignored_for_a_non_commander_event() {
        let mut projector = ArenaProjector::new();
        let mut events = brawl_match_events(LADDER, Some("HistoricBrawl"), LADDER);
        events.insert(2, deck_set());
        run(&mut projector, events);
        let view = projector.view(MATCH).expect("view");
        assert_eq!(view.format.as_deref(), Some("CPIONEER"));
    }

    #[test]
    fn brawl_live_equals_rebuild() {
        let events = brawl_match_events(BRAWL_QUEUE, Some("HistoricBrawl"), BRAWL_QUEUE);
        let mut live = ArenaProjector::new();
        let live_changes = run(&mut live, events.clone());
        let live_deck_uses: Vec<DeckUse> = live_changes
            .iter()
            .flat_map(|c| c.deck_uses.clone())
            .collect();

        let mut rebuilt = ArenaProjector::new();
        let inputs: Vec<ArenaInput> = events
            .into_iter()
            .enumerate()
            .map(|(index, event)| ArenaInput {
                file_id: 1,
                offset: (index as u64 + 1) * 10,
                seq: 0,
                at_ms: 1_000_000 + (index as i64 + 1) * 10,
                event,
            })
            .collect();
        let mut rebuilt_deck_uses: Vec<DeckUse> = Vec::new();
        for input in &inputs {
            rebuilt_deck_uses.extend(rebuilt.apply(input).deck_uses);
        }
        assert_eq!(live.view(MATCH), rebuilt.view(MATCH));
        assert_eq!(live_deck_uses, rebuilt_deck_uses);
    }

    #[test]
    fn a_draft_match_is_a_limited_match_with_no_deck_use() {
        let mut projector = ArenaProjector::new();
        let draft = "QuickDraft_FDN_20261101";
        let mut set_deck = deck_set();
        if let ArenaEvent::DeckSet { event_name, .. } = &mut set_deck {
            *event_name = draft.to_string();
        }
        let changes = run(
            &mut projector,
            vec![
                login(),
                authenticated(),
                set_deck,
                room(draft, false, Vec::new()),
                game_info(1, true, false, vec![game(1), match_won_by(1)]),
                room(draft, true, vec![game(1), match_won_by(1)]),
            ],
        );
        let view = projector.view(MATCH).expect("a draft match has a view");
        assert_eq!(view.format.as_deref(), Some("DFDNFDNFDN"));
        assert_eq!(view.match_type.as_deref(), Some("limited"));
        assert_eq!(view.outcome, Outcome::Win);
        assert!(changes.iter().all(|c| c.deck_uses.is_empty()));
    }

    #[test]
    fn match_result_comes_from_match_scope() {
        // The local player wins game 1, then concedes the match: a loss at 1-0.
        let mut projector = ArenaProjector::new();
        let changes = run(
            &mut projector,
            vec![
                login(),
                authenticated(),
                deck_set(),
                room(LADDER, false, Vec::new()),
                game_info(1, false, true, Vec::new()),
                game_info(1, true, true, vec![game(1)]),
                game_info(1, true, true, vec![game(1), match_won_by(2)]),
                room(LADDER, true, vec![game(1), match_won_by(2)]),
            ],
        );
        let view = projector.view(MATCH).expect("a bo3 match has a view");
        assert_eq!(view.outcome, Outcome::Loss);
        assert_eq!((view.wins, view.losses), (1, 0));
        assert_eq!(view.state, MatchState::Complete);
        assert_eq!(view.ended_at_ms, Some(5_000));
        assert_eq!(view.games.len(), 1);
        assert_eq!(view.games[0].result, GameResult::Win);
        assert_eq!(view.opponent.as_deref(), Some("arena:USER_OPP"));
        assert_eq!(view.local_username, "Player#00001");
        assert_eq!(view.format.as_deref(), Some("CPIONEER"));
        assert_eq!(view.match_type.as_deref(), Some("ranked"));
        assert_eq!(view.game_structure.as_deref(), Some("bo3"));
        assert_eq!(view.mtgo_match_id, None);
        let opponents: Vec<(String, String)> =
            changes.iter().flat_map(|c| c.opponents.clone()).collect();
        assert_eq!(
            opponents,
            vec![("arena:USER_OPP".to_string(), "Opponent".to_string())]
        );
        let deck_uses: Vec<DeckUse> = changes.iter().flat_map(|c| c.deck_uses.clone()).collect();
        assert_eq!(
            deck_uses,
            vec![DeckUse {
                match_token: MATCH.to_string(),
                deck_id: "deck-guid".to_string(),
                deck_name: "Test deck".to_string(),
                format_code: "CPIONEER".to_string(),
                main: vec![(71291, 4)],
                side: Vec::new(),
                commanders: Vec::new(),
            }]
        );
    }

    #[test]
    fn cumulative_results_are_not_double_counted() {
        let mut projector = ArenaProjector::new();
        run(
            &mut projector,
            vec![
                login(),
                authenticated(),
                room(LADDER, false, Vec::new()),
                game_info(1, false, true, Vec::new()),
                game_info(1, true, true, vec![game(2)]),
                game_info(2, false, true, vec![game(2)]),
                game_info(2, true, true, vec![game(2), game(1)]),
                game_info(3, false, true, vec![game(2), game(1)]),
                game_info(
                    3,
                    true,
                    true,
                    vec![game(2), game(1), game(2), match_won_by(2)],
                ),
            ],
        );
        let view = projector.view(MATCH).expect("view");
        assert_eq!((view.wins, view.losses), (1, 2));
        assert_eq!(view.outcome, Outcome::Loss);
        let results: Vec<GameResult> = view.games.iter().map(|g| g.result).collect();
        assert_eq!(
            results,
            vec![GameResult::Loss, GameResult::Win, GameResult::Loss]
        );
    }

    #[test]
    fn stale_season_rank_is_dropped() {
        // Trap 5: a season 89 answer before the season 95 one.
        let mut projector = ArenaProjector::new();
        let changes = run(
            &mut projector,
            vec![login(), rank(89), season(95), rank(95), rank(89)],
        );
        let points = all_rank(&changes);
        assert_eq!(
            points.len(),
            2,
            "one point per ladder of the season 95 answer"
        );
        assert!(points.iter().all(|p| p.reading.season == 95));
        assert_eq!(points[0].ladder, crate::Ladder::Constructed);
        assert_eq!(points[1].ladder, crate::Ladder::Limited);
        assert_eq!(points[0].offset, 40);
    }

    #[test]
    fn rank_before_any_login_is_held_until_the_login() {
        let mut projector = ArenaProjector::new();
        let changes = run(&mut projector, vec![season(95), rank(95), login()]);
        assert!(changes[1].rank.is_empty(), "no account owns it yet");
        let points = &changes[2].rank;
        assert_eq!(points.len(), 2);
        assert_eq!((points[0].file_id, points[0].offset), (1, 20));
        assert_eq!(points[0].at_ms, 1_000_020);
        assert_eq!(
            projector.account().map(|a| a.username.as_str()),
            Some("Player#00001")
        );
    }

    /// A rank held in a new file goes to the account that logs in there, not
    /// to the account of the earlier file.
    #[test]
    fn a_held_rank_is_released_to_the_different_account_that_logs_in() {
        let mut projector = ArenaProjector::new();
        run(&mut projector, vec![season(95), login()]);
        let apply = |projector: &mut ArenaProjector, offset: u64, event: ArenaEvent| {
            projector.apply(&ArenaInput {
                file_id: 2,
                offset,
                seq: 0,
                at_ms: 2_000_000 + offset as i64,
                event,
            })
        };
        let held = apply(&mut projector, 10, rank(95));
        assert!(held.rank.is_empty());
        let released = apply(
            &mut projector,
            20,
            ArenaEvent::Login {
                display_name: "Other#00002".to_string(),
            },
        );
        assert_eq!(released.rank.len(), 2, "the held rank is released");
        assert_eq!(
            projector.account().map(|a| a.username.as_str()),
            Some("Other#00002"),
            "the rank belongs to the account that logged in, not the earlier one"
        );
    }

    /// A new `Player.log` may be another account's: its rank before its own
    /// login waits for that login, not crediting the earlier file's account.
    #[test]
    fn rank_in_a_new_file_waits_for_that_files_login() {
        let mut projector = ArenaProjector::new();
        run(&mut projector, vec![season(95), login()]);
        let apply = |projector: &mut ArenaProjector, offset: u64, event: ArenaEvent| {
            projector.apply(&ArenaInput {
                file_id: 2,
                offset,
                seq: 0,
                at_ms: 2_000_000 + offset as i64,
                event,
            })
        };
        let held = apply(&mut projector, 10, rank(95));
        assert!(
            held.rank.is_empty(),
            "the earlier file's login is not this file's"
        );
        let released = apply(&mut projector, 20, untagged_login("USER_OTHER", "Other"));
        assert_eq!(released.rank.len(), 2);
        assert_eq!((released.rank[0].file_id, released.rank[0].offset), (2, 10));
        assert_eq!(projector.login_file(), Some(2));
        let after = apply(&mut projector, 30, rank(95));
        assert_eq!(
            after.rank.len(),
            2,
            "after its own login the file's rank is owned"
        );
    }

    #[test]
    fn unknown_event_names_produce_no_match() {
        let mut projector = ArenaProjector::new();
        let changes = run(
            &mut projector,
            vec![
                login(),
                authenticated(),
                room("DirectGame", false, Vec::new()),
                game_info(1, false, true, Vec::new()),
                game_info(1, true, true, vec![game(1), match_won_by(1)]),
                room("DirectGame", true, vec![game(1), match_won_by(1)]),
            ],
        );
        assert!(all_matches(&changes).is_empty());
        assert_eq!(projector.view(MATCH), None);
    }

    #[test]
    fn first_turn_and_mulligans_fill_the_game() {
        // FirstTurn repeats on turn 1 diffs (often with no match or game):
        // the first one per game counts. Mulligan seat 0 is the local player.
        let mut projector = ArenaProjector::new();
        run(
            &mut projector,
            vec![
                login(),
                authenticated(),
                room(LADDER, false, Vec::new()),
                game_info(1, false, true, Vec::new()),
                ArenaEvent::Mulligan {
                    seat: 0,
                    kept: false,
                    at_ms: None,
                },
                ArenaEvent::Mulligan {
                    seat: 0,
                    kept: true,
                    at_ms: None,
                },
                ArenaEvent::FirstTurn {
                    match_id: MATCH.to_string(),
                    game_number: 1,
                    active_seat: 2,
                },
                ArenaEvent::FirstTurn {
                    match_id: String::new(),
                    game_number: 0,
                    active_seat: 1,
                },
                game_info(1, true, true, vec![game(2)]),
                game_info(2, false, true, vec![game(2)]),
                ArenaEvent::FirstTurn {
                    match_id: String::new(),
                    game_number: 0,
                    active_seat: 1,
                },
            ],
        );
        let view = projector.view(MATCH).expect("view");
        assert_eq!(view.games.len(), 2);
        assert_eq!(view.games[0].on_play, Some(false));
        assert_eq!(view.games[0].local_mulligans, Some(1));
        assert_eq!(view.games[0].opp_mulligans, None);
        assert_eq!(view.games[1].on_play, Some(true));
        assert_eq!(view.games[1].local_mulligans, None);
        assert_eq!(view.state, MatchState::InProgress);
        assert_eq!(view.tentative_outcome, None);
    }

    /// A Bo3 match in game 2 with no result yet; last activity 6_002.
    fn unfinished_match() -> Vec<ArenaEvent> {
        vec![
            login(),
            authenticated(),
            room(LADDER, false, Vec::new()),
            game_info(1, false, true, Vec::new()),
            game_info(1, true, true, vec![game(1)]),
            game_info(2, false, true, vec![game(1)]),
        ]
    }

    #[test]
    fn quiet_arena_match_is_abandoned_after_an_hour() {
        let mut projector = ArenaProjector::new();
        run(&mut projector, unfinished_match());
        let last_activity = projector.view(MATCH).expect("view").last_activity_ms;
        assert_eq!(last_activity, 6_002);
        let changed = projector.finalize(last_activity + ABANDON_AFTER_MS);
        assert_eq!(changed, vec![MATCH.to_string()]);
        let view = projector.view(MATCH).expect("view");
        assert_eq!(view.state, MatchState::Abandoned);
        assert_eq!(view.resolution, Some(Resolution::Abandoned));
        assert_eq!(view.outcome, Outcome::Unknown);
        assert_eq!(view.ended_at_ms, Some(last_activity));
    }

    #[test]
    fn finalize_before_the_hour_changes_nothing() {
        let mut projector = ArenaProjector::new();
        run(&mut projector, unfinished_match());
        let before = projector.view(MATCH).cloned();
        let changed = projector.finalize(6_002 + ABANDON_AFTER_MS - 1);
        assert!(changed.is_empty());
        assert_eq!(projector.view(MATCH).cloned(), before);
        assert_eq!(before.map(|view| view.state), Some(MatchState::InProgress));
    }

    #[test]
    fn quiet_decided_match_completes_by_quiet() {
        let mut projector = ArenaProjector::new();
        let mut events = unfinished_match();
        events.push(game_info(2, true, true, vec![game(1), game(1)]));
        run(&mut projector, events);
        projector.finalize(6_002 + ABANDON_AFTER_MS);
        let view = projector.view(MATCH).expect("view");
        assert_eq!(view.state, MatchState::Complete);
        assert_eq!(view.resolution, Some(Resolution::Quiet));
        assert_eq!(view.outcome, Outcome::Win);
    }

    fn game_info_at(number: u32, over: bool, results: Vec<ResultEntry>, at_ms: i64) -> ArenaEvent {
        ArenaEvent::GameInfo {
            match_id: MATCH.to_string(),
            game_number: number,
            over,
            best_of_three: true,
            win_condition: Some(BO3.to_string()),
            results,
            at_ms,
        }
    }

    fn completed_room_at(results: Vec<ResultEntry>, at_ms: i64) -> ArenaEvent {
        let ArenaEvent::RoomState {
            match_id, players, ..
        } = room(LADDER, true, Vec::new())
        else {
            unreachable!("room builds a room state");
        };
        ArenaEvent::RoomState {
            match_id,
            completed: true,
            players,
            results,
            at_ms: Some(at_ms),
        }
    }

    /// A Bo3 at 2-0 for the local player with no match result yet; last
    /// event at 8_000.
    fn decided_match() -> Vec<ArenaEvent> {
        vec![
            login(),
            authenticated(),
            room(LADDER, false, Vec::new()),
            game_info_at(1, false, Vec::new(), 6_000),
            game_info_at(1, true, vec![game(1)], 7_000),
            game_info_at(2, true, vec![game(1), game(1)], 8_000),
        ]
    }

    /// Events of the same match after a two hour silence: a match result for
    /// the opponent and the completed room.
    fn late_opponent_result(after_ms: i64) -> Vec<ArenaEvent> {
        let results = vec![game(1), game(1), match_won_by(2)];
        vec![
            game_info_at(2, true, results.clone(), after_ms),
            completed_room_at(results, after_ms + 1_000),
        ]
    }

    #[test]
    fn a_quiet_gap_between_events_settles_the_match() {
        // As MTGO: the gap is a quiet point that later events never remove.
        let mut projector = ArenaProjector::new();
        let mut events = decided_match();
        events.extend(late_opponent_result(8_000 + 2 * ABANDON_AFTER_MS));
        run(&mut projector, events);
        let view = projector.view(MATCH).expect("view");
        assert_eq!(view.state, MatchState::Complete);
        assert_eq!(view.resolution, Some(Resolution::Quiet));
        assert_eq!(view.outcome, Outcome::Win);
        assert_eq!(view.ended_at_ms, Some(8_000));
    }

    #[test]
    fn a_quiet_gap_before_any_decision_does_not_settle() {
        // MTGO settles by quiet only an ended or decided match; at 1-0 the
        // later match result is the settlement.
        let mut projector = ArenaProjector::new();
        let late = 7_000 + 2 * ABANDON_AFTER_MS;
        run(
            &mut projector,
            vec![
                login(),
                authenticated(),
                room(LADDER, false, Vec::new()),
                game_info_at(1, true, vec![game(1)], 7_000),
                game_info_at(1, true, vec![game(1), match_won_by(2)], late),
                completed_room_at(vec![game(1), match_won_by(2)], late + 1_000),
            ],
        );
        let view = projector.view(MATCH).expect("view");
        assert_eq!(view.resolution, Some(Resolution::Signal));
        assert_eq!(view.outcome, Outcome::Loss);
    }

    #[test]
    fn finalize_then_late_events_keep_the_settlement() {
        let mut projector = ArenaProjector::new();
        run(&mut projector, decided_match());
        projector.finalize(8_000 + ABANDON_AFTER_MS);
        let settled = projector.view(MATCH).cloned().expect("view");
        assert_eq!(settled.state, MatchState::Complete);
        assert_eq!(settled.resolution, Some(Resolution::Quiet));
        assert_eq!(settled.outcome, Outcome::Win);

        let changes = run(
            &mut projector,
            late_opponent_result(8_000 + ABANDON_AFTER_MS + 60_000),
        );
        let view = projector.view(MATCH).expect("view");
        assert_eq!(
            (view.state, view.resolution, view.outcome, view.ended_at_ms),
            (
                settled.state,
                settled.resolution,
                settled.outcome,
                settled.ended_at_ms
            )
        );
        assert_eq!(view.last_activity_ms, 8_000 + ABANDON_AFTER_MS + 61_000);
        assert!(all_matches(&changes).iter().all(|token| token == MATCH));
    }

    #[test]
    fn an_abandoned_match_revives_on_new_events_as_mtgo() {
        // MTGO's `silence_without_a_disconnect_abandons_and_new_events_revive`:
        // abandonment is not a settlement.
        let mut projector = ArenaProjector::new();
        run(&mut projector, unfinished_match());
        projector.finalize(6_002 + ABANDON_AFTER_MS);
        assert_eq!(
            projector.view(MATCH).map(|view| view.state),
            Some(MatchState::Abandoned)
        );
        run(
            &mut projector,
            vec![game_info_at(
                2,
                false,
                vec![game(1)],
                6_002 + ABANDON_AFTER_MS,
            )],
        );
        assert_eq!(
            projector.view(MATCH).map(|view| view.state),
            Some(MatchState::InProgress)
        );
    }

    #[test]
    fn live_with_finalize_equals_rebuild() {
        let late = 8_000 + 2 * ABANDON_AFTER_MS;
        let mut events = decided_match();
        events.extend(late_opponent_result(late));
        let event_times: Vec<i64> = vec![0, 0, 5_000, 6_000, 7_000, 8_000, late, late + 1_000];
        let last_now = late + 1_000 + 2 * ABANDON_AFTER_MS;

        // Live: finalize after every event, half an hour and an hour and a
        // bit after it (the silence before the late events is seen live).
        let mut live = ArenaProjector::new();
        for (event, time) in events.iter().cloned().zip(&event_times) {
            run(&mut live, vec![event]);
            live.finalize(time + ABANDON_AFTER_MS / 2);
            live.finalize(time + ABANDON_AFTER_MS + 1);
        }
        live.finalize(last_now);

        let mut rebuilt = ArenaProjector::new();
        run(&mut rebuilt, events);
        rebuilt.finalize(last_now);

        assert_eq!(live.view(MATCH), rebuilt.view(MATCH));
        let view = rebuilt.view(MATCH).expect("view");
        assert_eq!(view.resolution, Some(Resolution::Quiet));
        assert_eq!(view.outcome, Outcome::Win);
    }

    #[test]
    fn huge_game_number_is_ignored() {
        let mut projector = ArenaProjector::new();
        let mut events = unfinished_match();
        events.push(game_info(u32::MAX, false, true, vec![game(1)]));
        events.push(ArenaEvent::FirstTurn {
            match_id: MATCH.to_string(),
            game_number: u32::MAX,
            active_seat: 1,
        });
        events.push(game_info(6, true, true, vec![game(1)]));
        run(&mut projector, events);
        let view = projector.view(MATCH).expect("view");
        assert_eq!(view.games.len(), 2);
        assert_eq!(view.games[1].on_play, None);
    }

    #[test]
    fn local_seat_is_found_by_user_id() {
        // Seat 2 holds the local user id; seat 1's name matches the login name
        // but the user id wins.
        let mut projector = ArenaProjector::new();
        run(
            &mut projector,
            vec![
                login(),
                authenticated(),
                ArenaEvent::RoomState {
                    match_id: MATCH.to_string(),
                    completed: false,
                    players: vec![
                        seat(1, OPP_ID, "Player", LADDER),
                        seat(2, LOCAL_ID, "Player", LADDER),
                    ],
                    results: Vec::new(),
                    at_ms: Some(5_000),
                },
                game_info(1, true, true, vec![game(2), match_won_by(2)]),
            ],
        );
        let view = projector.view(MATCH).expect("view");
        assert_eq!(view.outcome, Outcome::Win);
        assert_eq!(view.opponent.as_deref(), Some("arena:USER_OPP"));
    }
}
