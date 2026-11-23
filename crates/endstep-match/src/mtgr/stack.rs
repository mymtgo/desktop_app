//! The stack: abilities as objects, `resolve`, `cease`, and targets.
//!
//! - `ABILITY_ACTIVATED` and `TRIGGER_FIRED` put a new ability on the stack
//!   when the record has its source. Its text is Forge's, from the stack
//!   entry of the next full state, written without Forge's notes and card
//!   ids (`rules_text`); the record keeps the whole text to tell abilities
//!   apart.
//! - `SPELL_RESOLVED` names what resolved by its source card. `resolve` is
//!   written only for the object on top of the record's stack (a reader
//!   checks that); an ability then ceases by `rule:resolve`.
//! - An ability a full state's stack no longer has ceases, and so does any
//!   still there when a turn begins: a keyframe lists nothing on the stack.
//! - Targets are read from the stack entries of each full state, once per
//!   object.

use std::collections::{BTreeMap, BTreeSet};

use ::mtgr::{AbilityKind, Cause, Statement, TargetRef, Zone};
use endstep_protocol::{GameEvent, GameState, StackEntry};

use super::GameWriter;
use super::board::SEATS;
use super::events::{Batch, CardNames, Resolving, seat_of};
use super::objects::Ability;

/// The most abilities the record keeps on the stack at once (hostile input:
/// each event that adds one also reads the stack).
const MAX_ABILITIES: usize = 200;

/// Forge's text for an ability without Forge's own notes: the card ids it
/// puts after a name (`Hedge Maze (78)`), and the notes it puts at the end
/// (`[Zone Changer: ...]`, `[Card: ..., Activator: ...]`, `(Targeting:
/// ...)`), which carry ids and player names. `cards` holds the names the
/// game gives each card id (`events::card_names`). `None` when nothing is
/// left.
pub(super) fn rules_text(text: &str, cards: &CardNames) -> Option<String> {
    let mut text = without_ids(text, cards);
    loop {
        let trimmed = text.trim_end();
        match trailing_note(trimmed) {
            Some(start) => text = trimmed[..start].to_string(),
            None => {
                text = trimmed.to_string();
                break;
            }
        }
    }
    (!text.is_empty()).then_some(text)
}

/// The text without each ` (NN)` that follows the name of the card the
/// game knows under id NN (`Hedge Maze (78)`). A number in parentheses
/// after anything else (`discards a card (2)`) is rules text and stays.
fn without_ids(text: &str, cards: &CardNames) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(at) = rest.find(" (") {
        let after = &rest[at + 2..];
        let digits = after.chars().take_while(|ch| ch.is_ascii_digit()).count();
        let head = &rest[..at];
        let is_card = after[..digits]
            .parse::<u64>()
            .ok()
            .and_then(|id| cards.get(&id))
            .is_some_and(|names| names.iter().any(|name| ends_with_name(&out, head, name)));
        if is_card && after[digits..].starts_with(')') {
            out.push_str(&rest[..at]);
            rest = &after[digits + 1..];
        } else {
            out.push_str(&rest[..at + 2]);
            rest = after;
        }
    }
    out.push_str(rest);
    out
}

/// Whether `out` followed by `head` ends with `name`, without joining them:
/// joining copies all of `out` at every ` (`, quadratic on long text.
fn ends_with_name(out: &str, head: &str, name: &str) -> bool {
    if head.len() >= name.len() {
        return head.ends_with(name);
    }
    // `name` ends with `head`, so the cut below is on a char boundary.
    name.ends_with(head) && out.ends_with(&name[..name.len() - head.len()])
}

/// Where a note at the end of the text begins: a `[...]`, or a
/// `(Targeting: ...)`. Reminder text in parentheses is rules text and
/// stays.
fn trailing_note(text: &str) -> Option<usize> {
    let (open, close) = match text.chars().last()? {
        ']' => ('[', ']'),
        ')' => ('(', ')'),
        _ => return None,
    };
    let mut depth = 0;
    for (index, ch) in text.char_indices().rev() {
        if ch == close {
            depth += 1;
        } else if ch == open {
            depth -= 1;
            if depth == 0 {
                let is_note = open == '[' || text[index..].starts_with("(Targeting:");
                return is_note.then_some(index);
            }
        }
    }
    None
}

/// Whether a resolving object's text says it counters a spell.
fn says_counter(message: &str) -> bool {
    let text = message.to_lowercase();
    ["counter target", "counter that", "counter it"]
        .iter()
        .any(|phrase| text.contains(phrase))
}

impl<'a> GameWriter<'a> {
    /// The stack object a `SPELL_RESOLVED` names: an ability of that source
    /// card or the card itself as a spell. Forge's text tells two of one
    /// source apart; without it the one nearest the top.
    fn stack_object(&self, event: &GameEvent) -> Option<u32> {
        let forge_id = event.card_id?;
        let objects = &self.board.objects;
        let spell = objects.id_of(forge_id);
        let mut first = None;
        for id in objects.stack() {
            let Some(object) = objects.get(*id) else {
                continue;
            };
            let text = match &object.ability {
                Some(ability) if ability.source == forge_id => ability.text.as_ref(),
                None if spell == Some(*id) => self.spell_texts.get(id),
                _ => continue,
            };
            if text.is_some() && text == event.message.as_ref() {
                return Some(*id);
            }
            first = first.or(Some(*id));
        }
        first
    }

    /// Writes `resolve` now when a `SPELL_RESOLVED` is coming before the
    /// next full state: what Forge sends until then is that object's doing.
    /// Not for an object put on the stack since the last state, where the
    /// events before its `SPELL_RESOLVED` may be its costs.
    pub(super) fn resolve_ahead(&mut self) {
        if self.batch.resolving.is_some() {
            return;
        }
        let Some(event) = self
            .events_ahead()
            .iter()
            .copied()
            .find(|event| event.kind == "SPELL_RESOLVED")
        else {
            return;
        };
        let Some(id) = self.stack_object(event) else {
            return;
        };
        if !self.batch.new_on_stack.contains(&id) {
            self.begin_resolve(id, event);
        }
    }

    /// `resolve`, when the object is on top of the stack (a reader checks
    /// that); either way it is now the cause of what happens.
    fn begin_resolve(&mut self, id: u32, event: &GameEvent) {
        if self.board.objects.top_of_stack() == Some(id) {
            let object = self.board.obj(id);
            self.event(Statement::Resolve { object }, None);
        }
        self.batch.resolving = Some(Resolving {
            id,
            counters: event.message.as_deref().is_some_and(says_counter),
            done: false,
        });
        self.board.hints.cause = Some(Cause::Object(id));
    }

    /// `SPELL_RESOLVED`: the object resolved. An ability ceases here; a
    /// spell's card leaves the stack by its own zone change.
    pub(super) fn read_resolved(&mut self, event: &GameEvent) {
        let Some(id) = self.stack_object(event) else {
            return;
        };
        let already = self.batch.resolving.as_ref().map(|resolving| resolving.id);
        if already != Some(id) {
            self.begin_resolve(id, event);
        }
        let is_ability = self
            .board
            .objects
            .get(id)
            .is_some_and(|object| object.ability.is_some());
        if is_ability {
            self.cease(id, Cause::Rule("resolve".to_string()));
            self.batch.resolving = None;
        } else if let Some(resolving) = &mut self.batch.resolving {
            resolving.done = true;
        }
    }

    fn cease(&mut self, id: u32, cause: Cause) {
        let object = self.board.obj(id);
        self.event(Statement::Cease { object }, Some(cause));
        self.board.objects.end(id);
    }

    /// `ABILITY_ACTIVATED` and `TRIGGER_FIRED`: a new ability on the stack,
    /// when the record has its source.
    pub(super) fn read_ability(&mut self, event: &GameEvent, kind: AbilityKind) {
        let (Some(forge_id), Some(controller)) = (event.card_id, seat_of(event.player_index))
        else {
            return;
        };
        let Some(source) = self.board.objects.id_of(forge_id) else {
            return;
        };
        if self.board.objects.stack().len() >= MAX_ABILITIES {
            return;
        }
        let of = self.board.obj(source);
        let text = self.ability_text(forge_id, event);
        let ability = Ability {
            source: forge_id,
            text: text.clone(),
        };
        let id = self.board.objects.add_ability(ability, controller);
        self.batch.new_on_stack.insert(id);
        let statement = Statement::NewAbility {
            id,
            kind,
            of,
            text: text
                .as_deref()
                .and_then(|text| rules_text(text, &self.card_names)),
            controller: self.board.name(controller),
            in_stack: true,
        };
        self.event(statement, None);
    }

    /// Forge's text for the ability an event announces: that of the stack
    /// entry for its source in the next full state. With several entries of
    /// one source, those the record's abilities already have are passed
    /// over, and one with the event's target is taken first.
    fn ability_text(&self, forge_id: u64, event: &GameEvent) -> Option<String> {
        let state = self.next_state()?;
        let objects = &self.board.objects;
        let mut taken: Vec<&String> = objects
            .stack()
            .iter()
            .filter_map(|id| objects.get(*id))
            .filter_map(|object| object.ability.as_ref())
            .filter(|ability| ability.source == forge_id)
            .filter_map(|ability| ability.text.as_ref())
            .collect();
        let mut free: Vec<&StackEntry> = Vec::new();
        // The bottom of the stack first: the order the abilities went on.
        for entry in state.stack.iter().rev() {
            if !entry.is_ability || entry.source_id != forge_id {
                continue;
            }
            match taken.iter().position(|text| **text == entry.text) {
                Some(position) => {
                    taken.remove(position);
                }
                None => free.push(entry),
            }
        }
        let wanted = event.target_card_id.and_then(|id| i64::try_from(id).ok());
        let fits = |entry: &&StackEntry| match wanted {
            Some(target) => entry.target_ids.contains(&target),
            None => entry.target_ids.iter().all(|id| *id < 0),
        };
        let entry = free.iter().copied().find(fits).or(free.first().copied())?;
        (!entry.text.is_empty()).then(|| entry.text.clone())
    }

    /// Before a full state's diff: an ability the state's stack no longer
    /// has ceases. Its `SPELL_RESOLVED` did not come, so why is unknown.
    pub(super) fn cease_abilities_gone(&mut self, state: &GameState) {
        let mut shown: BTreeMap<u64, usize> = BTreeMap::new();
        for entry in &state.stack {
            if entry.is_ability {
                *shown.entry(entry.source_id).or_insert(0) += 1;
            }
        }
        // The bottom of the stack first: the newest abilities of a source
        // are the ones that left.
        let stack: Vec<u32> = self.board.objects.stack().to_vec();
        for id in stack.into_iter().rev() {
            let source = self
                .board
                .objects
                .get(id)
                .and_then(|object| object.ability.as_ref())
                .map(|ability| ability.source);
            let Some(source) = source else {
                continue;
            };
            let left = shown.entry(source).or_insert(0);
            if *left > 0 {
                *left -= 1;
            } else {
                self.cease(id, Cause::Unknown);
            }
        }
    }

    /// Before a turn line: a keyframe lists nothing on the stack, so every
    /// ability still there ceases. True when a spell is still on the stack
    /// (a card cannot cease): the record is then incomplete up to the
    /// keyframe. The turn's combat and the batch start afresh.
    pub(super) fn empty_stack(&mut self) -> bool {
        self.batch = Batch::default();
        self.attackers.clear();
        self.blockers.clear();
        let stack: Vec<u32> = self.board.objects.stack().to_vec();
        for id in stack {
            let is_ability = self
                .board
                .objects
                .get(id)
                .is_some_and(|object| object.ability.is_some());
            if is_ability {
                self.cease(id, Cause::Unknown);
            }
        }
        !self.board.objects.stack().is_empty()
    }

    /// After a full state's diff: the targets of each stack entry the
    /// record has an object for, written once per object.
    pub(super) fn write_targets(&mut self, state: &GameState) {
        let mut matched: BTreeSet<u32> = BTreeSet::new();
        for entry in state.stack.iter().rev() {
            let Some(id) = self.entry_object(entry, &matched) else {
                continue;
            };
            matched.insert(id);
            if !entry.is_ability {
                self.spell_texts.insert(id, entry.text.clone());
            }
            if entry.target_ids.is_empty() || !self.targets_read.insert(id) {
                continue;
            }
            self.targeted.insert(id);
            let object = self.board.obj(id);
            for target in self.targets_of(entry) {
                let statement = Statement::Target {
                    object: object.clone(),
                    target,
                };
                self.event(statement, None);
            }
        }
    }

    /// The record's stack object for a state's stack entry, other than
    /// those already `matched`: the spell's card, or an ability of the
    /// entry's source (the one with the entry's text first).
    fn entry_object(&self, entry: &StackEntry, matched: &BTreeSet<u32>) -> Option<u32> {
        let objects = &self.board.objects;
        if !entry.is_ability {
            let id = objects.id_of(entry.source_id)?;
            let object = objects.get(id)?;
            let is_spell = object.zone == Zone::Stack && object.ability.is_none();
            return (is_spell && !matched.contains(&id)).then_some(id);
        }
        let mut first = None;
        for id in objects.stack().iter().rev() {
            let ability = objects.get(*id).and_then(|object| object.ability.as_ref());
            let Some(ability) = ability.filter(|ability| ability.source == entry.source_id) else {
                continue;
            };
            if matched.contains(id) {
                continue;
            }
            if ability.text.as_deref() == Some(entry.text.as_str()) {
                return Some(*id);
            }
            first = first.or(Some(*id));
        }
        first
    }

    /// An entry's targets: each object the record has, and each player. A
    /// player target carries no seat (its id is -1), so the players are
    /// read from `targetDescription`, and only when it names as many
    /// players of the game as the entry has player targets.
    fn targets_of(&self, entry: &StackEntry) -> Vec<TargetRef> {
        let named = self.players_named(&entry.target_description);
        let player_targets = entry.target_ids.iter().filter(|id| **id < 0).count();
        let mut players = if named.len() == player_targets {
            named.into_iter()
        } else {
            Vec::new().into_iter()
        };
        let mut targets = Vec::new();
        for target_id in &entry.target_ids {
            let Ok(forge_id) = u64::try_from(*target_id) else {
                if let Some(player) = players.next() {
                    targets.push(TargetRef::Player(player));
                }
                continue;
            };
            if let Some(id) = self.board.objects.id_of(forge_id) {
                targets.push(TargetRef::Object(self.board.obj(id)));
            }
        }
        targets
    }

    /// The players a target description names, in its order: the parts of
    /// `[Player], [Subtlety (109), Oliphaunt (118)]` that equal a player's
    /// name.
    fn players_named(&self, description: &str) -> Vec<String> {
        let mut named = Vec::new();
        for part in description.split(',') {
            let part = part.trim().trim_matches(|ch| ch == '[' || ch == ']');
            if let Some(player) = self.player_named(part.trim()) {
                named.push(self.board.name(player));
            }
        }
        named
    }

    pub(super) fn player_named(&self, name: &str) -> Option<u32> {
        SEATS
            .into_iter()
            .find(|seat| !name.is_empty() && self.board.name(*seat) == name)
    }
}
