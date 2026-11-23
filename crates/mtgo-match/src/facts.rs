//! Everything observed about one match token, kept sorted by event key so the
//! result never depends on arrival order.

use std::collections::{BTreeMap, BTreeSet};

use mtgo_protocol::GameEntry;

/// `(file_id, byte_offset)`.
pub type EventKey = (u64, u64);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JoinFact {
    pub key: EventKey,
    pub at_ms: i64,
    pub kv: BTreeMap<String, String>,
    pub raw_text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateChangeFact {
    pub key: EventKey,
    pub at_ms: i64,
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntryFact {
    pub key: EventKey,
    pub at_ms: i64,
    pub game_id: Option<u64>,
    pub entry: GameEntry,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameStateFact {
    pub key: EventKey,
    pub at_ms: i64,
    pub game_id: u64,
    /// Parsed only until a game state naming the players is known (see
    /// `MatchFacts::first_players`); later ones keep `None`.
    pub players: Option<Vec<String>>,
}

#[derive(Debug, Clone, Default)]
pub struct MatchFacts {
    pub token: String,
    pub match_id: Option<u64>,
    /// Earliest joined-state game_management event (carries the key-value block).
    pub gm_join: Option<JoinFact>,
    /// Earliest joined-state state change (no key-value block).
    pub state_join: Option<JoinFact>,
    pub state_changes: Vec<StateChangeFact>,
    pub entries: Vec<EntryFact>,
    pub game_states: Vec<GameStateFact>,
    pub file_username: Option<(EventKey, String)>,
    pub deck_used_username: Option<(EventKey, String)>,
    seen: BTreeSet<EventKey>,
}

fn insert_sorted<T>(items: &mut Vec<T>, item: T, key_of: impl Fn(&T) -> EventKey) {
    let key = key_of(&item);
    let index = items.partition_point(|existing| key_of(existing) < key);
    items.insert(index, item);
}

fn keep_earliest<T>(slot: &mut Option<T>, item: T, key_of: impl Fn(&T) -> EventKey) {
    let replace = match slot {
        Some(existing) => key_of(&item) < key_of(existing),
        None => true,
    };
    if replace {
        *slot = Some(item);
    }
}

impl MatchFacts {
    pub fn new(token: &str) -> Self {
        Self {
            token: token.to_string(),
            ..Self::default()
        }
    }

    /// Record an event key; `false` if it was already applied (idempotency).
    pub fn mark_seen(&mut self, key: EventKey) -> bool {
        self.seen.insert(key)
    }

    /// The game_management join is preferred because it carries the key-value block.
    pub fn preferred_join(&self) -> Option<&JoinFact> {
        self.gm_join.as_ref().or(self.state_join.as_ref())
    }

    pub fn add_gm_join(&mut self, join: JoinFact) {
        keep_earliest(&mut self.gm_join, join, |j| j.key);
    }

    pub fn add_state_join(&mut self, join: JoinFact) {
        keep_earliest(&mut self.state_join, join, |j| j.key);
    }

    pub fn add_state_change(&mut self, change: StateChangeFact) {
        insert_sorted(&mut self.state_changes, change, |c| c.key);
    }

    pub fn add_entry(&mut self, entry: EntryFact) {
        insert_sorted(&mut self.entries, entry, |e| e.key);
    }

    pub fn add_game_state(&mut self, state: GameStateFact) {
        insert_sorted(&mut self.game_states, state, |g| g.key);
    }

    /// The players named by the earliest game state that names any. A game state
    /// whose JSON is unreadable or lists nobody is skipped, so one bad snapshot
    /// never hides the match for good.
    pub fn first_players(&self) -> Option<(EventKey, &Vec<String>)> {
        self.game_states
            .iter()
            .find_map(|state| match &state.players {
                Some(players) if !players.is_empty() => Some((state.key, players)),
                _ => None,
            })
    }

    pub fn note_file_username(&mut self, key: EventKey, name: &str) {
        keep_earliest(
            &mut self.file_username,
            (key, name.to_string()),
            |(k, _)| *k,
        );
    }

    pub fn note_deck_used_username(&mut self, key: EventKey, name: &str) {
        keep_earliest(
            &mut self.deck_used_username,
            (key, name.to_string()),
            |(k, _)| *k,
        );
    }

    /// A copy holding only the facts at or before `key` in event order.
    pub fn up_to(&self, key: EventKey) -> MatchFacts {
        let mut copy = MatchFacts::new(&self.token);
        copy.match_id = self.match_id;
        copy.gm_join = self.gm_join.clone().filter(|j| j.key <= key);
        copy.state_join = self.state_join.clone().filter(|j| j.key <= key);
        copy.state_changes = self
            .state_changes
            .iter()
            .filter(|c| c.key <= key)
            .cloned()
            .collect();
        copy.entries = self
            .entries
            .iter()
            .filter(|e| e.key <= key)
            .cloned()
            .collect();
        copy.game_states = self
            .game_states
            .iter()
            .filter(|g| g.key <= key)
            .cloned()
            .collect();
        copy.file_username = self.file_username.clone().filter(|(k, _)| *k <= key);
        copy.deck_used_username = self.deck_used_username.clone().filter(|(k, _)| *k <= key);
        copy.seen = self.seen.iter().copied().filter(|k| *k <= key).collect();
        copy
    }

    /// Latest event time seen for the match.
    pub fn last_activity_ms(&self) -> i64 {
        let joins = self
            .gm_join
            .iter()
            .chain(self.state_join.iter())
            .map(|j| j.at_ms);
        let changes = self.state_changes.iter().map(|c| c.at_ms);
        let entries = self.entries.iter().map(|e| e.at_ms);
        let states = self.game_states.iter().map(|g| g.at_ms);
        joins
            .chain(changes)
            .chain(entries)
            .chain(states)
            .max()
            .unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn facts_stay_sorted_by_key_whatever_the_arrival_order() {
        let mut facts = MatchFacts::new("t");
        for offset in [30, 10, 20] {
            facts.add_state_change(StateChangeFact {
                key: (1, offset),
                at_ms: offset as i64,
                from: "A".into(),
                to: "B".into(),
            });
        }
        let keys: Vec<_> = facts.state_changes.iter().map(|c| c.key).collect();
        assert_eq!(keys, vec![(1, 10), (1, 20), (1, 30)]);
    }

    #[test]
    fn mark_seen_rejects_duplicates() {
        let mut facts = MatchFacts::new("t");
        assert!(facts.mark_seen((1, 5)));
        assert!(!facts.mark_seen((1, 5)));
    }

    #[test]
    fn earliest_join_and_usernames_win() {
        let mut facts = MatchFacts::new("t");
        let join = |offset: u64| JoinFact {
            key: (1, offset),
            at_ms: 0,
            kv: Default::default(),
            raw_text: offset.to_string(),
        };
        facts.add_gm_join(join(20));
        facts.add_gm_join(join(10));
        facts.add_state_join(join(5));
        assert_eq!(facts.preferred_join().map(|j| j.key), Some((1, 10)));
        facts.note_file_username((1, 9), "late");
        facts.note_file_username((1, 3), "early");
        assert_eq!(
            facts.file_username.as_ref().map(|(_, n)| n.as_str()),
            Some("early")
        );
    }
}
