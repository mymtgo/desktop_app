//! Endstep matches as `.mtgr` 0.3.0 records: one `mtgr::Game` per game of
//! the match that has a full state, written from the capture frames alone.
//!
//! - The frames come from the fold's one walk (`fold::replay_source`), so
//!   the replay and the stored rows agree on which match is dropped, on game
//!   numbers and on each game's deck.
//! - Tags: the required roster with `Site "Endstep"` and `Round "-"`;
//!   `Player1` and `Player2` are seats 0 and 1, `Perspective` is the user;
//!   archetypes when known; `ENDSTEP_MatchId`.
//! - Body: the user's `deck`, the `pregame` (keyframe, who chose to play
//!   first, opening hands, mulligans, keeps and bottomed cards), then one
//!   `turn` per `TURN_BEGAN` with its keyframe, a step per `TURN_PHASE`, and
//!   under the current step what each game event says (`events.rs`: casts,
//!   plays, abilities, combat, damage, life, reveals, shuffles and the moves
//!   an event names), then whatever each full state shows that the record
//!   does not have yet (`board.rs`). Keyframes are in `keyframe.rs`.
//! - Tags `Omits` and `Partial` say what Endstep's frames never give, or
//!   give only in part.
//! - A game the capture joined after its start has a pregame with only the
//!   keyframe, then `incomplete` and each object where the first state
//!   shows it.
//! - A game that reaches `MAX_LINES` lines or `MAX_OBJECTS` live objects
//!   is cut short with `incomplete`: no later frame is read.
//! - End: `concede` (or `lose`) at `GAME_OUTCOME`, with the winner the fold
//!   kept for the game (the `Result` tag's); a game whose end was not
//!   captured ends with `incomplete`.
//!
//! Times are whole seconds from the game's first frame.

mod board;
mod combat;
#[cfg(test)]
mod event_tests;
mod events;
mod keyframe;
mod objects;
mod shown;
mod stack;
#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, BTreeSet};

use ::mtgr::{
    Cause, DeckCard, Game, GameResult, Line, Record, Statement, Step, Tag, or_unknown, result_text,
    tag, utc_date,
};
use endstep_protocol::{GameEvent, GameState, MatchBody, Unit};
use events::{Ahead, Batch};

use crate::fold::{ReplayGame, ReplaySource, replay_source};
use crate::games::{GameRecord, KeptKind};
use crate::sideboard::GameDeck;
use board::{Board, SEATS, Written};

/// The `.mtgr` version written.
const VERSION: &str = "0.3.0";

/// The statement families Endstep never reports, in the standard's word
/// order (spec 4.7):
/// - `mana`: full states carry each player's `manaPool` totals, but no frame
///   says which permanent made mana or what spent it (`MANA_LOG` is text
///   only);
/// - `costs`: no frame says what a spell cost;
/// - `priority`: only the user's own `PASS_PRIORITY` actions are captured;
/// - `effects`: Forge's lasting effects show as objects without types
///   (trap 47), which the builder does not track.
const OMITS: &str = "mana costs priority effects";

/// The statement families written only in part:
/// - `damage`: written from `PLAYER_DAMAGED` and `CARD_DAMAGED` when the
///   source is an object the record has; whether Forge sends one for every
///   damage is not known;
/// - `library`: shuffles and the cards a reveal names are written; searches
///   are not, a scry or surveil names no card, and library order is unknown.
///
/// The standard has no word for targets, which are also written only when a
/// full state shows the stack entry.
const PARTIAL: &str = "damage library";

/// The most lines and live objects one game's record holds (hostile input:
/// a crafted capture must not grow the record, and each diff's scan of the
/// objects, without bound). A real game has a few thousand lines and a few
/// hundred objects. A game that reaches either is cut short with
/// `incomplete`, the standard's mark for a record that stops.
const MAX_LINES: usize = 50_000;
pub(super) const MAX_OBJECTS: usize = 2_000;

/// A game's limits: `MAX_LINES` and `MAX_OBJECTS`, smaller in tests.
#[derive(Debug, Clone, Copy)]
struct Limits {
    lines: usize,
    objects: usize,
}

/// The archetype names the store knows for the match, written as tags.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReplayNames {
    pub local_archetype: Option<String>,
    pub opponent_archetype: Option<String>,
}

/// The match as a `.mtgr` record: `None` when `fold_match` would drop the
/// match (no match body, a bot, a spectator), or when the user's seat is not
/// one of a two-player game. A game with no full state is left out, and a
/// match with no game left is `None` too.
pub fn match_record(forge_game_id: &str, units: &[Unit], names: &ReplayNames) -> Option<Record> {
    let source = replay_source(forge_game_id, units)?;
    if !SEATS.contains(&source.seat) {
        return None;
    }
    let mut record = Record::default();
    for game in &source.games {
        let seat_names = seat_names(&game.record, &source.body);
        let mut writer = GameWriter::new(source.seat, game, seat_names.clone());
        if !writer.write() {
            continue;
        }
        let result = result_of(&game.record);
        record.games.push(Game {
            tags: tags(&source, names, &seat_names, game.record.number, result),
            lines: writer.lines,
            result,
        });
    }
    if record.games.is_empty() {
        return None;
    }
    Some(record)
}

/// Seat 0 is `Player1`, seat 1 is `Player2`.
fn result_of(record: &GameRecord) -> GameResult {
    match (record.decided, record.winner_seat) {
        (true, Some(0)) => GameResult::Player1,
        (true, Some(1)) => GameResult::Player2,
        _ => GameResult::Unknown,
    }
}

/// The names of seats 0 and 1: from the game's states, else the match
/// body's participants, else the seat itself.
fn seat_names(record: &GameRecord, body: &MatchBody) -> BTreeMap<u32, String> {
    let mut names = BTreeMap::new();
    for seat in SEATS {
        let from_states = record.frames.iter().find_map(|frame| match &frame.kind {
            KeptKind::State(state) => state
                .players
                .iter()
                .find(|(player, name)| *player == seat && !name.is_empty())
                .map(|(_, name)| name.clone()),
            _ => None,
        });
        let from_body = body
            .participants
            .get(seat as usize)
            .map(|person| person.username.clone())
            .filter(|name| !name.is_empty());
        let name = from_states
            .or(from_body)
            .unwrap_or_else(|| format!("Seat {seat}"));
        names.insert(seat, name);
    }
    names
}

fn tags(
    source: &ReplaySource,
    names: &ReplayNames,
    seat_names: &BTreeMap<u32, String>,
    number: u32,
    result: GameResult,
) -> Vec<Tag> {
    let name = |seat: u32| seat_names.get(&seat).cloned().unwrap_or_default();
    let event = if source.body.ranked {
        "Ranked"
    } else {
        "Casual"
    };
    let format = source.body.format_id.as_deref().unwrap_or("");
    let mut tags = vec![
        tag("Mtgr", VERSION),
        tag("Event", event),
        tag("Site", "Endstep"),
        tag("Date", &utc_date(source.started_at_ms)),
        tag("Round", "-"),
        tag("Player1", &name(0)),
        tag("Player2", &name(1)),
        tag("Result", result_text(result)),
        tag("Format", or_unknown(format)),
        tag("Match", &source.body.id),
        tag("Game", &number.to_string()),
        tag("Perspective", &name(source.seat)),
        tag("Omits", OMITS),
        tag("Partial", PARTIAL),
        tag("ENDSTEP_MatchId", &source.body.id),
    ];
    let archetype = |seat: u32| {
        if seat == source.seat {
            names.local_archetype.clone()
        } else {
            names.opponent_archetype.clone()
        }
    };
    if let Some(name) = archetype(0) {
        tags.push(tag("Player1Archetype", &name));
    }
    if let Some(name) = archetype(1) {
        tags.push(tag("Player2Archetype", &name));
    }
    tags
}

/// The user's `deck` block: the game's deck, main then side, each by name;
/// `unknown` when no deck is known. The standard has one `deck` line, so the
/// opponent's (never sent) is not written.
fn deck_lines(player: &str, deck: Option<&GameDeck>) -> Vec<Line> {
    let line = |level: u8, statement: Statement| Line {
        time_ms: 0,
        level,
        statement,
        cause: None,
    };
    let mut lines = vec![line(
        0,
        Statement::Deck {
            player: player.to_string(),
            name: None,
            unknown: deck.is_none(),
        },
    )];
    let Some(deck) = deck else {
        return lines;
    };
    for (side, cards) in [(false, &deck.main), (true, &deck.side)] {
        for (name, count) in cards {
            lines.push(line(
                1,
                Statement::DeckEntry {
                    side,
                    count: *count,
                    card: DeckCard::Named(name.clone()),
                },
            ));
        }
    }
    lines
}

/// Forge's step names as the standard's steps. First strike damage is a
/// combat damage step.
fn step_of(name: &str) -> Option<Step> {
    match name {
        "UNTAP" => Some(Step::Untap),
        "UPKEEP" => Some(Step::Upkeep),
        "DRAW" => Some(Step::Draw),
        "MAIN1" => Some(Step::PrecombatMain),
        "COMBAT_BEGIN" => Some(Step::BeginningOfCombat),
        "COMBAT_DECLARE_ATTACKERS" => Some(Step::DeclareAttackers),
        "COMBAT_DECLARE_BLOCKERS" => Some(Step::DeclareBlockers),
        "COMBAT_FIRST_STRIKE_DAMAGE" | "COMBAT_DAMAGE" => Some(Step::CombatDamage),
        "COMBAT_END" => Some(Step::EndOfCombat),
        "MAIN2" => Some(Step::PostcombatMain),
        "END_OF_TURN" => Some(Step::End),
        "CLEANUP" => Some(Step::Cleanup),
        _ => None,
    }
}

fn other_seat(seat: u32) -> u32 {
    if seat == 0 { 1 } else { 0 }
}

/// One game's lines, written frame by frame.
struct GameWriter<'a> {
    game: &'a ReplayGame,
    user_seat: u32,
    board: Board,
    lines: Vec<Line>,
    /// The first frame's time: line times count from it.
    origin: i64,
    /// The current line time (ms since `origin`), never decreasing.
    time: i64,
    /// The pregame is written: a full state came.
    started: bool,
    /// The current turn, 0 in the pregame.
    turn: u32,
    /// The last step line of the current turn.
    step: Option<Step>,
    /// The step of the last full state, for a turn with no step line yet.
    state_step: Option<Step>,
    /// An `incomplete` line is written and no turn has begun since.
    incomplete: bool,
    /// A turn began that no frame names the player of: its steps are not
    /// written (they would sit under the last turn).
    unnamed_turn: bool,
    /// Who chose the starting player, and whom (`GAME_STARTED`).
    choice: Option<(u32, u32)>,
    /// The seats that kept: a card they put back now is bottomed.
    kept: BTreeSet<u32>,
    /// The Forge ids of the cards the user bottomed (from the fold's hands).
    bottomed: Vec<u64>,
    /// The line that ends the game is written.
    ended: bool,
    /// The index of the frame being read: an event looks ahead from it.
    index: usize,
    /// What the frames after `index` show, read on first use.
    ahead: Ahead<'a>,
    /// The owner each state gives a Forge card id, over the whole game.
    owners: BTreeMap<u64, u32>,
    /// The names the game's frames give each Forge card id.
    card_names: events::CardNames,
    /// What the events since the last full state established.
    batch: Batch,
    /// The objects that attacked and blocked this turn.
    attackers: BTreeSet<u32>,
    blockers: BTreeSet<u32>,
    /// The stack objects whose targets were read, and those that had any.
    targets_read: BTreeSet<u32>,
    targeted: BTreeSet<u32>,
    /// Forge's text for each spell on the stack, by object.
    spell_texts: BTreeMap<u32, String>,
    limits: Limits,
}

impl<'a> GameWriter<'a> {
    fn new(user_seat: u32, game: &'a ReplayGame, names: BTreeMap<u32, String>) -> Self {
        let hands = game.record.hands.hands_of(user_seat);
        GameWriter {
            game,
            user_seat,
            board: Board::new(user_seat, names),
            lines: Vec::new(),
            origin: game.record.frames.first().map_or(0, |frame| frame.at_ms),
            time: 0,
            started: false,
            turn: 0,
            step: None,
            state_step: None,
            incomplete: false,
            unnamed_turn: false,
            choice: None,
            kept: BTreeSet::new(),
            bottomed: hands.bottomed.iter().map(|(id, _)| *id).collect(),
            ended: false,
            index: 0,
            ahead: Ahead::default(),
            owners: events::owners(game),
            card_names: events::card_names(game),
            batch: Batch::default(),
            attackers: BTreeSet::new(),
            blockers: BTreeSet::new(),
            targets_read: BTreeSet::new(),
            targeted: BTreeSet::new(),
            spell_texts: BTreeMap::new(),
            limits: Limits {
                lines: MAX_LINES,
                objects: MAX_OBJECTS,
            },
        }
    }

    /// Writes the game. False when no full state came: there is no game to
    /// write.
    fn write(&mut self) -> bool {
        let game = self.game;
        let player = self.board.name(self.user_seat);
        self.lines = deck_lines(&player, game.deck.as_ref());
        for (index, frame) in game.record.frames.iter().enumerate() {
            if self.ended {
                break;
            }
            if self.started && self.too_big() {
                self.incomplete("The game is too big to write whole");
                self.ended = true;
                break;
            }
            self.index = index;
            self.ahead = Ahead::default();
            self.time = self.time.max(frame.at_ms.saturating_sub(self.origin));
            match &frame.kind {
                // A sideboarding state has empty zones: it is not a game
                // state.
                KeptKind::State(state) if !state.sideboarding => self.read_state(state),
                KeptKind::State(_) => {}
                KeptKind::Event(event) => self.read_event(event),
                KeptKind::Delta(delta) => {
                    for (seat, life) in &delta.life {
                        self.write_life(*seat, *life);
                    }
                }
            }
        }
        if self.started && !self.ended {
            let reason = "Endstep did not send the end of the game".to_string();
            self.event(Statement::Incomplete { reason }, None);
        }
        self.started
    }

    /// Whether the record has reached a limit: no more frames are read.
    fn too_big(&self) -> bool {
        self.lines.len() >= self.limits.lines || self.board.objects.count() >= self.limits.objects
    }

    fn line(&mut self, level: u8, statement: Statement, cause: Option<Cause>) {
        self.lines.push(Line {
            time_ms: self.time,
            level,
            statement,
            cause,
        });
    }

    /// An event line: level 1 in the pregame, level 2 under a step in a
    /// turn. A turn with no step line yet gets the step of the last full
    /// state first, as the standard has no events directly under a turn.
    fn event(&mut self, statement: Statement, cause: Option<Cause>) {
        if self.turn == 0 {
            self.line(1, statement, cause);
            return;
        }
        if self.step.is_none() {
            self.write_step(self.state_step.unwrap_or(Step::Untap));
        }
        self.line(2, statement, cause);
    }

    fn events(&mut self, written: Vec<Written>) {
        for (statement, cause) in written {
            self.event(statement, cause);
        }
    }

    fn incomplete(&mut self, reason: &str) {
        let reason = reason.to_string();
        self.event(Statement::Incomplete { reason }, None);
        self.incomplete = true;
    }

    /// A step line, once while the step lasts. None while a turn with no
    /// named player lasts: its events stay under the `incomplete` line.
    fn write_step(&mut self, step: Step) {
        if self.turn > 0 && !self.unnamed_turn && self.step != Some(step) {
            self.step = Some(step);
            self.line(1, Statement::Step(step), None);
        }
    }

    fn keyframe(&mut self, level: u8, entries: Vec<Statement>) {
        self.line(level, Statement::Keyframe, None);
        for entry in entries {
            self.line(level + 1, entry, None);
        }
    }

    fn read_state(&mut self, state: &GameState) {
        let turn = state.turn_number.unwrap_or(0);
        if !self.started {
            self.write_pregame(state, turn == 0);
            if turn > 0 {
                // A game joined late: its objects are introduced where they
                // are before the first turn line.
                let written = self.board.diff(state);
                self.events(written);
            }
        }
        if turn > self.turn {
            // The `TURN_BEGAN` of this turn was not captured.
            match state.active_player.filter(|seat| SEATS.contains(seat)) {
                Some(seat) => self.begin_turn(turn, seat, Some(state)),
                None => self.turn_without_player(turn),
            }
        }
        self.state_step = state.step.as_deref().and_then(step_of);
        self.cease_abilities_gone(state);
        let written = self.board.diff(state);
        self.events(written);
        self.write_targets(state);
        self.batch = Batch::default();
    }

    /// `pregame` and its keyframe. A game joined after its start is
    /// `incomplete` from here: the diff of this first state then introduces
    /// each object where it is.
    fn write_pregame(&mut self, state: &GameState, from_start: bool) {
        self.started = true;
        self.line(0, Statement::Pregame, None);
        let entries = self.board.start(state, from_start);
        self.keyframe(1, entries);
        if from_start {
            self.write_choose_first();
        } else {
            self.incomplete("Endstep did not send the start of the game");
        }
    }

    /// `"P" choose first "Q"`, once.
    fn write_choose_first(&mut self) {
        if let Some((chooser, first)) = self.choice.take() {
            let player = self.board.name(chooser);
            let first = self.board.name(first);
            self.line(1, Statement::ChooseFirst { player, first }, None);
        }
    }

    /// `turn` and its keyframe. A turn that does not follow the last one,
    /// numbers no statement could bring in line, or a spell still on the
    /// stack (a keyframe lists none) are marked `incomplete` first: the
    /// keyframe then replaces what a reader holds. Then the untap step's
    /// untaps, read from `state`: the first full state of the turn.
    fn begin_turn(&mut self, number: u32, seat: u32, state: Option<&GameState>) {
        if number <= self.turn {
            return;
        }
        let spell_left = self.empty_stack();
        if !self.incomplete {
            if self.board.drifted() || spell_left {
                self.incomplete("Endstep did not show every card that changed zone");
            } else if number != self.turn + 1 {
                self.incomplete("Endstep did not send every turn");
            }
        }
        self.board.end_pregame();
        self.turn = number;
        self.step = None;
        self.state_step = None;
        self.incomplete = false;
        self.unnamed_turn = false;
        let player = self.board.name(seat);
        self.line(0, Statement::Turn { number, player }, None);
        let entries = self.board.keyframe();
        self.keyframe(1, entries);
        if let Some(state) = state.filter(|state| state.turn_number == Some(number)) {
            self.write_untaps(seat, state);
        }
    }

    /// The untap step: each permanent of the active player that the record
    /// has tapped and the turn's first full state shows untapped. That
    /// state often comes after the step events of the turn's start, and its
    /// diff would put these untaps under a later step.
    fn write_untaps(&mut self, seat: u32, state: &GameState) {
        let written = self.board.untap_step(seat, state);
        if !written.is_empty() {
            self.write_step(Step::Untap);
            self.events(written);
        }
    }

    /// A turn began that no frame names the player of: no seat is guessed
    /// (an extra turn breaks any order). The record is `incomplete` until a
    /// later `TURN_BEGAN`, or a state with an active player, begins a turn.
    fn turn_without_player(&mut self, number: u32) {
        if !self.incomplete {
            self.incomplete(&format!("Endstep did not say who began turn {number}"));
        }
        // After the `incomplete` line, which may need a step line first.
        self.unnamed_turn = true;
    }

    fn read_event(&mut self, event: &GameEvent) {
        let seat = event.player_index.filter(|seat| SEATS.contains(seat));
        match event.kind.as_str() {
            "GAME_STARTED" if self.turn == 0 => self.read_game_started(event),
            "GAME_OUTCOME" if self.started => self.write_outcome(),
            _ if !self.started => {}
            "TURN_BEGAN" => {
                if let (Some(number), Some(seat)) = (event.turn_number, seat) {
                    let state = self.next_state();
                    self.begin_turn(number, seat, state);
                }
            }
            "TURN_PHASE" => {
                if let Some(step) = event.phase.as_deref().and_then(step_of) {
                    self.write_step(step);
                }
            }
            _ if self.turn > 0 => self.read_turn_event(event),
            // A `MULLIGAN` with an amount is the redraw done: the next full
            // state shows the new hand.
            "MULLIGAN" if event.amount.is_none() => {
                if let Some(seat) = seat {
                    let player = self.board.name(seat);
                    self.line(1, Statement::Mulligan { player }, None);
                    let written = self.board.send_hand_back(seat);
                    self.events(written);
                }
            }
            "HAND_KEPT" => {
                if let Some(seat) = seat
                    && self.kept.insert(seat)
                {
                    let player = self.board.name(seat);
                    self.line(1, Statement::Keep { player }, None);
                }
            }
            "CARD_ZONE_CHANGE" => self.read_bottom(event),
            "CARD_REVEALED" => self.read_reveal(event),
            _ => {}
        }
    }

    /// `GAME_STARTED`: `tossPlayerIndex` chose, `playerIndex` goes first.
    fn read_game_started(&mut self, event: &GameEvent) {
        let seat = |seat: Option<u32>| seat.filter(|seat| SEATS.contains(seat));
        let Some(first) = seat(event.player_index) else {
            return;
        };
        let chooser = seat(event.toss_player).unwrap_or(first);
        self.choice = Some((chooser, first));
        if self.started {
            self.write_choose_first();
        }
    }

    /// A card a seat put back after it kept is bottomed. The event names
    /// neither the card nor its id.
    fn read_bottom(&mut self, event: &GameEvent) {
        let hand_to_library = event.during_mulligan
            && event.from_zone.as_deref() == Some("Hand")
            && event.to_zone.as_deref() == Some("Library");
        let Some(seat) = event.zone_owner.filter(|seat| self.kept.contains(seat)) else {
            return;
        };
        if hand_to_library {
            let written = self.board.bottom(seat, &self.bottomed);
            self.events(written);
        }
    }

    fn write_life(&mut self, seat: u32, life: i32) {
        if self.started
            && let Some((statement, cause)) = self.board.set_life(seat, life)
        {
            self.event(statement, cause);
        }
    }

    /// The line that ends the game, at its `GAME_OUTCOME`: the loser
    /// concedes, or loses for a reason Endstep has not been seen to send.
    /// The winner and the reason are the fold's (`GameRecord`), the same the
    /// `Result` tag reads, so the two never disagree when a capture holds
    /// two outcomes for one game. Without a winner the game ends
    /// `incomplete`.
    fn write_outcome(&mut self) {
        let record = &self.game.record;
        let winner = match result_of(record) {
            GameResult::Player1 => 0,
            GameResult::Player2 => 1,
            _ => {
                // A draw, or a winner outside the two seats: no statement
                // says it.
                self.incomplete("Endstep sent the end of the game without a winner");
                self.ended = true;
                return;
            }
        };
        let player = self.board.name(other_seat(winner));
        if record.end_reason.as_deref() == Some("concede") {
            self.event(Statement::Concede { player }, None);
        } else {
            self.event(Statement::Lose { player }, Some(Cause::Unknown));
        }
        self.ended = true;
    }
}
