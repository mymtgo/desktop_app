//! Arena matches as `.mtgr` 0.3.0 records (the arena replays spec, section
//! 4.3): one `mtgr::Game` per game that started, written from the fold of
//! its stored messages and the card data.
//!
//! - Tags: the required roster with `Site "Arena"` and `Round "-"`;
//!   `Player1` and `Player2` are Arena seats 1 and 2, `Perspective` is the
//!   local player; archetypes when known; `ARENA_MatchId`.
//! - Body: the local `deck` block, the `pregame` (keyframe, die rolls, who
//!   chose to play first, the opening hands, mulligans and keeps), then one
//!   `turn` per turn with its keyframe, its steps and its events: zone moves
//!   and hidden cards (`moves.rs`), abilities, targets and resolution
//!   (`abilities.rs`), tokens, tapping, counters and permanent changes
//!   (`objects.rs`, with `control` from each object's controller), exile `with` links (`moves.rs`: Arena's
//!   `DisplayCardUnderCard`, ended by `ids.rs` as either object leaves),
//!   `unattach` when an Attachment ends, combat, damage and life
//!   (`combat.rs`), mana (`mana.rs`), and priority with stack snapshots
//!   (`priority.rs`).
//! - `Omits` and `Partial` (spec 4.7, decision 0043), from the plan's
//!   evidence: Arena never reports cost changes; it reports the library
//!   family (no searches, `library.rs`) and lasting effects (no text,
//!   `effects.rs`) in part, and the record writes what it reports.
//! - Faces and halves (`faces.rs`): cards named by their card, with
//!   `face`, `transform`, and day and night.
//! - Gaps in the stored messages (`gaps.rs`): `incomplete`, then the diffs
//!   after it, resynced at the next keyframe.
//! - End: the concede or loss line and the result, from the client's
//!   concede and the game's `gameInfo.results`.
//!
//! Times are whole seconds from the game's first stored message. A game
//! whose fold never started (a match conceded between games) writes nothing.

mod abilities;
mod combat;
mod copies;
mod effects;
mod ending;
mod faces;
mod gaps;
mod ids;
mod keyframe;
mod library;
mod mana;
mod moves;
mod objects;
mod outside;
mod pregame;
mod priority;
#[cfg(test)]
mod tests;
mod turns;

use std::collections::{BTreeMap, BTreeSet};

use ::mtgr::{
    Cause, DeckCard, Game, GameResult, Line, Record, Statement, Tag, Zone, or_unknown, result_text,
    tag, utc_date,
};

use crate::message::{SHUFFLE, ZONE_TRANSFER};
use crate::summary::game_deck;
use crate::{Annotation, CardKey, ClientAction, Fold, GameInput, fold_game, front_face};
use abilities::{
    ABILITY_CREATED, ABILITY_DELETED, CHOICE_RESULT, RESOLUTION_COMPLETE, RESOLUTION_START,
    SUBMITTED_TARGETS, USER_ACTION,
};
use combat::{DAMAGE_DEALT, MODIFIED_LIFE};
use effects::{LAYERED_EFFECT_CREATED, LAYERED_EFFECT_DESTROYED};
use faces::GAIN_DESIGNATION;
use ids::Ids;
use library::SCRY;
use mana::{MANA_PAID, ManaBook};
use objects::{
    ATTACHMENT_CREATED, COUNTER_ADDED, COUNTER_REMOVED, PT_MODIFIED, TAPPED_UNTAPPED,
    TOKEN_CREATED, TOKEN_DELETED,
};
use outside::REVEALED_CARD_CREATED;
use pregame::CHOOSE_STARTING_PLAYER;
use priority::{SELECTING_TARGETS, StackView};
use turns::{PHASE_OR_STEP, state_pair, step_pair, turn_cut};

/// A card's name and the grpId of the card data that named it, written as the
/// card's source id (`src arena:N`, spec 6.4) where it is named: `new card`
/// and `identify`. A hidden card has none (its grpId would name it). A
/// `None` name is a card the card data does not list (yet): it is written
/// `?` with its source id (decision 0048).
#[derive(Debug, Clone)]
pub(super) struct Named {
    pub(super) name: Option<String>,
    pub(super) grp_id: u32,
}

impl Named {
    pub(super) fn sources(&self) -> Vec<String> {
        vec![format!("arena:{}", self.grp_id)]
    }
}

/// The `.mtgr` version this module writes (spec 4.2): the format is
/// baselined at 0.3.0, with the `Partial` tag, the effect text `?`, `top N`
/// and `bottom ?` (decisions 0043 to 0047).
const VERSION: &str = "0.3.0";

/// The statement family Arena never reports (spec 4.7; evidence in the
/// plan's ledger): `costs`, as no annotation reports a change to a spell's
/// cost.
const OMITS: &str = "costs";

/// The statement families Arena reports only in part, in the standard's
/// word order (spec 4.7, decision 0043; evidence in the plan's ledger):
/// - `library`: shuffles, library order and scry results are reported,
///   searches are not (https://github.com/mtgreplay/standards/issues/1);
/// - `effects`: each continuous effect's creation, source, affected
///   objects and end are reported, but no rules text, no stated duration
///   and no delayed triggers
///   (https://github.com/mtgreplay/standards/issues/2).
const PARTIAL: &str = "library effects";

/// What the writer knows about a card: its name (a multi-faced card uses
/// its `A // B` name) and its face names, front first (empty for a
/// one-faced card).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardData {
    pub name: String,
    pub faces: Vec<String>,
}

/// Card data by the card's grpId: a double-faced card's front face, a split
/// card's or Room's own grpId (not its halves').
pub type CardTable = BTreeMap<u32, CardData>;

/// One Arena match and its games' stored messages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchInput {
    pub match_token: String,
    pub event: String,
    pub format: String,
    pub local_name: String,
    pub opponent_name: String,
    pub local_archetype: Option<String>,
    pub opponent_archetype: Option<String>,
    /// When the match started (UTC unix ms): the date of a game with no
    /// messages.
    pub started_at: i64,
    pub games: Vec<GameMessages>,
}

/// One game's stored messages, in log order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameMessages {
    pub number: u32,
    pub messages: Vec<GameInput>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MtgrError {
    /// Cards whose card data names the card but not a face or half the
    /// record must name (`face` has no `?` form, decision 0048), by the
    /// card's grpId: the record is not built. A card with no card data at
    /// all is no error: it is written `?` with its source id.
    MissingCardData(Vec<u32>),
}

/// A token the record names by its subtypes followed by "Token" (CR
/// 111.4), as the card data has nothing for its grpId. Its subtypes are the
/// name Scryfall gives the token ("Map" for "Map Token"), so a reader can
/// find its art by them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubtypeToken {
    pub name: String,
    pub subtypes: Vec<String>,
    pub grp_id: u32,
}

/// A written match: the record, its subtype-named tokens (one per grpId,
/// in grpId order) and its double-faced cards.
#[derive(Debug, Clone, PartialEq)]
pub struct WrittenMatch {
    pub record: Record,
    pub tokens: Vec<SubtypeToken>,
    /// The front grpId of every card Arena showed with an `othersideGrpId`:
    /// a transforming or modal double-faced card, whose faces each have
    /// their own picture. A split card, Room or Adventure has one picture.
    pub double_faced: BTreeSet<u32>,
}

/// The match as a `.mtgr` record: one game per game that started.
pub fn match_record(input: &MatchInput, cards: &CardTable) -> Result<Record, MtgrError> {
    write_match(input, cards).map(|written| written.record)
}

/// The match as a `.mtgr` record, with the tokens it names by their
/// subtypes.
pub fn write_match(input: &MatchInput, cards: &CardTable) -> Result<WrittenMatch, MtgrError> {
    let mut folds: Vec<(&GameMessages, Fold)> = Vec::new();
    for game in &input.games {
        let fold = fold_game(&game.messages);
        if fold.started && !fold.states.is_empty() {
            folds.push((game, fold));
        }
    }
    // The local seat is the recipient of the first GRE message sent to one
    // seat alone. A started game always had such messages in the fixtures
    // and the owner's logs; if none of the games has one, seat 1 is assumed
    // (the roster may then be swapped).
    let local_seat = folds
        .iter()
        .find_map(|(_, fold)| fold.local_seat)
        .unwrap_or(1);
    let roster = Roster {
        local_seat,
        local_name: input.local_name.clone(),
        opponent_name: input.opponent_name.clone(),
    };
    let mut missing: BTreeSet<u32> = BTreeSet::new();
    let mut tokens: BTreeMap<u32, SubtypeToken> = BTreeMap::new();
    let mut record = Record::default();
    for (game, fold) in &folds {
        let mut writer = GameWriter::new(fold, cards, &roster, game);
        writer.write_deck(game_deck(&game.messages));
        writer.write_body();
        let ending = writer.finish(&game.messages, game.number);
        missing.append(&mut writer.missing);
        tokens.append(&mut writer.subtype_tokens);
        record.games.push(Game {
            tags: tags(input, &roster, game, &ending),
            lines: writer.lines,
            result: ending.result,
        });
    }
    if !missing.is_empty() {
        return Err(MtgrError::MissingCardData(missing.into_iter().collect()));
    }
    Ok(WrittenMatch {
        record,
        tokens: tokens.into_values().collect(),
        double_faced: double_faced(&folds),
    })
}

/// The front grpId of every object any state shows with another side.
fn double_faced(folds: &[(&GameMessages, Fold)]) -> BTreeSet<u32> {
    let mut fronts = BTreeSet::new();
    for (_, fold) in folds {
        for state in &fold.states {
            for object in state.objects.values() {
                if object.otherside_grp_id != 0 {
                    fronts.insert(front_face(object.grp_id, object.otherside_grp_id));
                }
            }
        }
    }
    fronts
}

/// The match's two players by Arena seat.
struct Roster {
    local_seat: u32,
    local_name: String,
    opponent_name: String,
}

impl Roster {
    fn name(&self, seat: u32) -> String {
        if seat == self.local_seat {
            self.local_name.clone()
        } else {
            self.opponent_name.clone()
        }
    }

    /// The seat that is `Player1` or `Player2` for a result.
    fn result_for(&self, winner_seat: u32) -> GameResult {
        if winner_seat == 1 {
            GameResult::Player1
        } else {
            GameResult::Player2
        }
    }
}

/// The other seat of a two-player game.
fn other_seat(seat: u32) -> u32 {
    if seat == 1 { 2 } else { 1 }
}

/// How a game ended.
struct Ending {
    result: GameResult,
    termination: Option<&'static str>,
}

fn tags(input: &MatchInput, roster: &Roster, game: &GameMessages, ending: &Ending) -> Vec<Tag> {
    let started_at = game
        .messages
        .first()
        .map(|message| message.logged_at)
        .unwrap_or(input.started_at);
    let mut tags = vec![
        tag("Mtgr", VERSION),
        tag("Event", or_unknown(&input.event)),
        tag("Site", "Arena"),
        tag("Date", &utc_date(started_at)),
        tag("Round", "-"),
        tag("Player1", &roster.name(1)),
        tag("Player2", &roster.name(2)),
        tag("Result", result_text(ending.result)),
        tag("Format", or_unknown(&input.format)),
        tag("Match", &input.match_token),
        tag("Game", &game.number.to_string()),
        tag("Perspective", &roster.local_name),
        tag("Omits", OMITS),
        tag("Partial", PARTIAL),
        tag("ARENA_MatchId", &input.match_token),
    ];
    let archetype = |seat: u32| {
        if seat == roster.local_seat {
            input.local_archetype.clone()
        } else {
            input.opponent_archetype.clone()
        }
    };
    if let Some(name) = archetype(1) {
        tags.push(tag("Player1Archetype", &name));
    }
    if let Some(name) = archetype(2) {
        tags.push(tag("Player2Archetype", &name));
    }
    if let Some(termination) = ending.termination {
        tags.push(tag("Termination", termination));
    }
    tags
}

/// One game's lines, written state by state.
struct GameWriter<'a> {
    fold: &'a Fold,
    cards: &'a CardTable,
    roster: &'a Roster,
    /// The first stored message's time: line times count from it.
    origin: i64,
    /// The current line time (ms since `origin`), never decreasing.
    time: i64,
    /// The state being written.
    index: usize,
    lines: Vec<Line>,
    ids: Ids,
    /// Cards whose card data has no name for a face the record must name.
    missing: BTreeSet<u32>,
    /// Tokens named by their subtypes, by grpId.
    subtype_tokens: BTreeMap<u32, SubtypeToken>,
    /// The current turn, 0 in the pregame.
    turn: u32,
    /// The last step line of the current turn.
    last_step: Option<(i64, i64)>,
    /// The next `Fold::transfers` entry to write.
    next_transfer: usize,
    /// Seats whose opening hand was drawn.
    hand_drawn: BTreeSet<u32>,
    /// Who chose the starting player, and whom (written once).
    choice: Option<(u32, u32)>,
    chose_first: bool,
    /// The seat of the first turn, opening hands first.
    starting_seat: u32,
    /// The seat that conceded, ending the game.
    conceded: Option<u32>,
    /// The `UserActionTaken` action type of each instance it names.
    actions: BTreeMap<u32, i64>,
    /// Every instance id a state shows on the stack.
    stacked: BTreeSet<u32>,
    /// Spells and abilities whose targets are written (since their cast).
    targeted: BTreeSet<CardKey>,
    /// The targets written for each spell or ability, with the zone each
    /// was in then (`None`: a player, or an object the record lacks).
    target_zones: BTreeMap<CardKey, Vec<Option<(CardKey, Zone)>>>,
    /// Spells and abilities whose X is written (since their cast).
    x_chosen: BTreeSet<CardKey>,
    /// Creatures whose attack or block this turn is written.
    attacks: BTreeSet<CardKey>,
    blocks: BTreeSet<CardKey>,
    /// Each seat's life total as the record last gave it.
    life: BTreeMap<u32, i32>,
    /// Triggered abilities not yet shown on the stack.
    pending_triggers: Vec<abilities::PendingTrigger>,
    /// The objects resolving now (`ResolutionStart` without its
    /// `ResolutionComplete` yet), innermost last.
    resolving_now: Vec<CardKey>,
    /// The revealed companions are written (once, in the pregame).
    companions_written: bool,
    /// Companions whose special action is written.
    specials: BTreeSet<CardKey>,
    /// Mana in the pools, floating and being paid.
    mana: ManaBook,
    /// Each written ability's source.
    ability_sources: BTreeMap<CardKey, CardKey>,
    /// The ability grpId of each instance a `UserActionTaken` names.
    ability_grp_ids: BTreeMap<u32, u32>,
    /// Copies whose original the record cannot tell (not written).
    untold_copies: BTreeSet<CardKey>,
    /// Every grpId each card is shown by (`faces::shown_grp_ids`).
    shown_grp_ids: BTreeMap<CardKey, BTreeSet<u32>>,
    /// Triggered mana abilities (instance ids).
    mana_triggers: BTreeSet<u32>,
    /// The stack as the record's lines leave it.
    stack_view: StackView,
    /// The player who holds priority as the record last wrote it, and the
    /// stack then (ids, bottom first).
    priority: Option<(u32, Vec<u32>)>,
    /// Spells and abilities being cast or activated (until their
    /// `UserActionTaken`): no one receives priority meanwhile.
    announced: BTreeSet<CardKey>,
    /// Objects whose targets a player is choosing.
    selecting: BTreeSet<CardKey>,
    /// The seats that passed with `passed_stack` (ids, bottom first).
    passed: BTreeSet<u32>,
    passed_stack: Vec<u32>,
    /// What a payment into an object pays for: a special action's ability
    /// pays for its card.
    paid_for: BTreeMap<CardKey, CardKey>,
    /// Day (`false`) or night (`true`), once the record made it either.
    day_night: Option<bool>,
    /// A gap was written: the next keyframe replaces the record's state.
    after_gap: bool,
    /// Library sizes as the record's lines leave them.
    library_view: gaps::LibraryView,
    /// Library positions as the record's lines leave them.
    positions: library::PositionView,
    /// Arena effect ids that outlive the cleanup step of their turn.
    lasting_effects: BTreeSet<u32>,
    /// Arena effect ids some state shows in force.
    shown_effects: BTreeSet<u32>,
    /// The lasting effects written and not ended.
    effects: Vec<effects::LiveEffect>,
    /// The cards prompts showed, with the asking object's instance id and
    /// the state's index: each is looked at once per state (Arena sends a
    /// prompt again after a wrong answer), and again by a later prompt.
    looked: BTreeSet<(CardKey, u32, usize)>,
}

impl<'a> GameWriter<'a> {
    fn new(fold: &'a Fold, cards: &'a CardTable, roster: &'a Roster, game: &GameMessages) -> Self {
        let starting_seat = fold
            .states
            .iter()
            .find(|state| state.turn >= 1)
            .and_then(|state| state.active_seat)
            .or_else(|| fold.states.iter().find_map(|state| state.active_seat))
            .unwrap_or(1);
        let actions = abilities::user_actions(fold);
        let mana_triggers = mana::triggered_mana_abilities(fold, &actions);
        GameWriter {
            fold,
            cards,
            roster,
            origin: game.messages.first().map(|m| m.logged_at).unwrap_or(0),
            time: 0,
            index: 0,
            lines: Vec::new(),
            ids: Ids::default(),
            missing: BTreeSet::new(),
            subtype_tokens: BTreeMap::new(),
            turn: 0,
            last_step: None,
            next_transfer: 0,
            hand_drawn: BTreeSet::new(),
            choice: starting_choice(fold, starting_seat),
            chose_first: false,
            starting_seat,
            conceded: None,
            actions,
            stacked: abilities::stacked_ids(fold),
            targeted: BTreeSet::new(),
            target_zones: BTreeMap::new(),
            x_chosen: BTreeSet::new(),
            attacks: BTreeSet::new(),
            blocks: BTreeSet::new(),
            life: BTreeMap::new(),
            pending_triggers: Vec::new(),
            resolving_now: Vec::new(),
            companions_written: false,
            specials: BTreeSet::new(),
            mana: ManaBook::default(),
            ability_sources: BTreeMap::new(),
            ability_grp_ids: mana::ability_grp_ids(fold),
            mana_triggers,
            untold_copies: BTreeSet::new(),
            shown_grp_ids: faces::shown_grp_ids(fold),
            stack_view: StackView::default(),
            priority: None,
            announced: BTreeSet::new(),
            selecting: BTreeSet::new(),
            passed: BTreeSet::new(),
            passed_stack: Vec::new(),
            paid_for: BTreeMap::new(),
            day_night: None,
            after_gap: false,
            library_view: gaps::LibraryView::default(),
            positions: library::PositionView::default(),
            lasting_effects: effects::lasting_effect_ids(fold),
            shown_effects: effects::shown_effect_ids(fold),
            effects: Vec::new(),
            looked: BTreeSet::new(),
        }
    }

    /// The seats in turn order.
    fn seat_order(&self) -> [u32; 2] {
        [self.starting_seat, other_seat(self.starting_seat)]
    }

    fn line(&mut self, level: u8, statement: Statement, cause: Option<Cause>) {
        self.lines.push(Line {
            time_ms: self.time,
            level,
            statement,
            cause,
        });
    }

    /// An event line: level 1 in the pregame, level 2 in a step (writing the
    /// current step first if the turn has none yet).
    fn event(&mut self, statement: Statement, cause: Option<Cause>) {
        if self.turn == 0 {
            self.line(1, statement, cause);
            return;
        }
        if self.last_step.is_none() {
            let pair = state_pair(&self.fold.states[self.index]).unwrap_or((1, 1));
            self.write_step(pair);
        }
        // Declarations come before anything else in their step: an
        // attacker tapping, a "whenever this attacks or blocks" trigger.
        self.write_declarations();
        self.line(2, statement, cause);
    }

    /// A card's name with its grpId as the source id; `?` (a `None` name)
    /// when the card data has none.
    fn named(&self, grp_id: u32) -> Named {
        Named {
            name: self.card_name(grp_id),
            grp_id,
        }
    }

    /// The card name of a card's grpId (`faces::shown_card`), `None` when
    /// the card data has none.
    fn card_name(&self, grp_id: u32) -> Option<String> {
        self.cards.get(&grp_id).map(|card| card.name.clone())
    }

    fn write_deck(&mut self, deck: Option<(BTreeMap<u32, u32>, BTreeMap<u32, u32>)>) {
        let player = self.roster.local_name.clone();
        let Some((main, side)) = deck else {
            self.line(
                0,
                Statement::Deck {
                    player,
                    name: None,
                    unknown: true,
                },
                None,
            );
            return;
        };
        self.line(
            0,
            Statement::Deck {
                player,
                name: None,
                unknown: false,
            },
            None,
        );
        for (is_side, cards) in [(false, main), (true, side)] {
            // One entry per name (printings merge), by name, then one per
            // unknown card by its source id as text (decision 0048).
            let mut by_name: BTreeMap<String, u32> = BTreeMap::new();
            let mut by_source: BTreeMap<String, u32> = BTreeMap::new();
            for (grp_id, count) in cards {
                match self.card_name(grp_id) {
                    Some(name) => *by_name.entry(name).or_insert(0) += count,
                    None => *by_source.entry(format!("arena:{grp_id}")).or_insert(0) += count,
                }
            }
            let named = by_name
                .into_iter()
                .map(|(name, count)| (DeckCard::Named(name), count));
            let unknown = by_source
                .into_iter()
                .map(|(source, count)| (DeckCard::Unknown(source), count));
            for (card, count) in named.chain(unknown) {
                self.line(
                    1,
                    Statement::DeckEntry {
                        side: is_side,
                        count,
                        card,
                    },
                    None,
                );
            }
        }
    }

    fn write_body(&mut self) {
        let fold = self.fold;
        for index in 0..fold.states.len() {
            let state = &fold.states[index];
            let step = &fold.steps[index];
            self.index = index;
            self.time = self.time.max(step.logged_at - self.origin);
            if index == 0 {
                self.write_pregame_head(state);
            } else if let Some((first, last)) = gaps::missing_before(fold, index) {
                self.write_gap(first, last);
            }
            for action in &step.client {
                match action {
                    ClientAction::ChooseStartingPlayer { .. } if self.turn == 0 => {
                        self.write_choose_first();
                    }
                    ClientAction::Concede { .. } => {
                        let seat = self.roster.local_seat;
                        self.concede(seat);
                        return;
                    }
                    _ => {}
                }
            }
            if self.turn == 0 {
                self.pregame_state(index);
            }
            if self.turn > 0 {
                self.write_specials_ahead(index);
            }
            if self.turn > 0 {
                self.write_x_choices(index);
            }
            let cut = (state.turn > self.turn).then(|| turn_cut(&step.annotations));
            for (position, annotation) in step.annotations.iter().enumerate() {
                if cut == Some(position) {
                    self.start_turn(index, position);
                }
                self.write_annotation(index, annotation);
            }
            self.write_unattached(index);
            if cut == Some(step.annotations.len()) {
                self.start_turn(index, step.annotations.len());
            }
            self.write_shown_cards(index);
            self.end_of_state(index);
            if self.turn > 0 {
                self.step_from_state(state);
            }
            // The pools as the state leaves them (after any step it begins).
            self.sync_pool(index, None);
            self.lose_vanished_mana(index);
            self.priority_after_state(index);
        }
    }

    fn write_annotation(&mut self, index: usize, annotation: &Annotation) {
        if !mana::is_payment_part(annotation) {
            self.flush_payment();
        }
        if annotation.is(PHASE_OR_STEP) {
            let phase = annotation.int("phase").unwrap_or(0);
            let step = annotation.int("step").unwrap_or(0);
            if self.turn > 0
                && let Some(pair) = step_pair(phase, step)
            {
                self.write_step(pair);
            }
        } else if annotation.is(ZONE_TRANSFER) {
            self.write_transfers(index, annotation);
        } else if annotation.is(SHUFFLE) {
            self.write_shuffle(index, annotation);
        } else if annotation.is(TOKEN_CREATED) {
            self.write_tokens_created(index, annotation);
        } else if annotation.is(TOKEN_DELETED) {
            self.write_tokens_deleted(annotation);
        } else if annotation.is(ABILITY_CREATED) {
            self.write_abilities_created(index, annotation);
        } else if annotation.is(ABILITY_DELETED) {
            self.write_abilities_deleted(index, annotation);
        } else if annotation.is(RESOLUTION_START) {
            self.write_resolution_start(index, annotation);
        } else if annotation.is(RESOLUTION_COMPLETE) {
            self.write_resolution_complete(index, annotation);
        } else if annotation.is(ATTACHMENT_CREATED) {
            self.write_attachment(index, annotation);
        } else if annotation.is(REVEALED_CARD_CREATED) {
            self.write_reveal(index, annotation);
        } else if annotation.is(SUBMITTED_TARGETS) {
            self.write_submitted_targets(index, annotation);
        } else if annotation.is(TAPPED_UNTAPPED) {
            self.write_tapped(annotation);
        } else if annotation.is(COUNTER_ADDED) || annotation.is(COUNTER_REMOVED) {
            self.write_counters(index, annotation);
        } else if annotation.is(PT_MODIFIED) {
            self.write_pt_modified(index, annotation);
        } else if annotation.is(DAMAGE_DEALT) {
            self.write_damage(annotation);
        } else if annotation.is(MODIFIED_LIFE) {
            self.write_life(annotation);
        } else if annotation.is(MANA_PAID) {
            self.write_mana_paid(index, annotation);
        } else if annotation.is(USER_ACTION) {
            self.write_user_action(annotation);
        } else if annotation.is(SELECTING_TARGETS) {
            self.write_selecting_targets(annotation);
        } else if annotation.is(GAIN_DESIGNATION) {
            self.write_designation(index, annotation);
        } else if annotation.is(CHOICE_RESULT) {
            self.write_choice_result(annotation);
        } else if annotation.is(SCRY) {
            self.write_scry(index, annotation);
        } else if annotation.is(LAYERED_EFFECT_CREATED) {
            self.write_effects_created(index, annotation);
        } else if annotation.is(LAYERED_EFFECT_DESTROYED) {
            self.write_effects_destroyed(annotation);
        }
    }

    /// What state `index` shows that its annotations did not write:
    /// targets, permanent changes and life totals.
    fn end_of_state(&mut self, index: usize) {
        self.flush_payment();
        self.write_pending_triggers(index);
        self.write_pending_targets(index);
        self.write_transforms(index, false);
        self.write_control(index);
        self.sync_permanents(index);
        self.write_removed_cards(index);
        let life: Vec<(u32, i32)> = self.fold.states[index]
            .players
            .iter()
            .map(|(seat, player)| (*seat, player.life))
            .collect();
        self.sync_life(&life);
    }

    fn concede(&mut self, seat: u32) {
        let player = self.roster.name(seat);
        self.event(Statement::Concede { player }, None);
        self.conceded = Some(seat);
    }
}

/// Who chose the starting player, and the seat chosen. Arena reports the
/// chooser: the seat waited on for `ChooseStartingPlayerResp`. Arena games
/// have two seats (the writer's `other_seat` and `seat_order` assume it),
/// and one of them chooses, so the chooser is the seat a state waits on
/// alone: a state waiting on more than one seat names none. The seat chosen
/// is the client's choice when the local player chose, else the seat of the
/// first turn. `None` when no state names a chooser.
fn starting_choice(fold: &Fold, starting_seat: u32) -> Option<(u32, u32)> {
    let chooser = fold.states.iter().find_map(|state| {
        let waiting: Vec<u32> = state
            .pending
            .iter()
            .filter(|(_, message)| *message == CHOOSE_STARTING_PLAYER)
            .map(|(seat, _)| *seat)
            .collect();
        match waiting.as_slice() {
            [seat] => Some(*seat),
            _ => None,
        }
    })?;
    for step in &fold.steps {
        for action in &step.client {
            if let ClientAction::ChooseStartingPlayer { seat } = action {
                return Some((chooser, *seat));
            }
        }
    }
    Some((chooser, starting_seat))
}
