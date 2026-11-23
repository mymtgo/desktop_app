//! The deck the user played in each game, from the sideboard prompt that
//! opens a game and the user's `SIDEBOARD_SUBMIT` answer to it.
//!
//! Game 1 is the match deck. Endstep shows the prompt for game N at the start
//! of game N (the game about to be played), so a prompt and its answer belong
//! to the game of the state that carried the prompt. The answer lists the
//! option ids that make the main deck, and is kept with the prompt it
//! answered: Forge sends the prompt again after the submit with a new
//! `promptVersion` (trap 50). When no answer was captured the next game's
//! prompt shows the deck that was played (its first `main_count` options),
//! and when that is missing too the game keeps the previous game's deck.

use std::collections::{BTreeMap, BTreeSet};

use endstep_protocol::{GameAction, Prompt};

use crate::EndstepDeck;
use crate::games::GameRecord;

/// A submit that keeps fewer cards than this is not a deck.
const MIN_MAIN_CARDS: usize = 40;

/// The deck one game was played with: card name and quantity, sorted by name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GameDeck {
    pub main: Vec<(String, u32)>,
    pub side: Vec<(String, u32)>,
}

impl GameDeck {
    /// The match deck by card name (printings add up).
    pub(crate) fn from_match(deck: &EndstepDeck) -> GameDeck {
        let main = deck
            .main
            .iter()
            .map(|(key, quantity)| (key.name.clone(), *quantity));
        let side = deck
            .side
            .iter()
            .map(|(key, quantity)| (key.name.clone(), *quantity));
        GameDeck {
            main: by_name(main),
            side: by_name(side),
        }
    }

    fn from_names(main: Vec<String>, side: Vec<String>) -> GameDeck {
        GameDeck {
            main: by_name(main.into_iter().map(|name| (name, 1))),
            side: by_name(side.into_iter().map(|name| (name, 1))),
        }
    }
}

fn by_name(lines: impl Iterator<Item = (String, u32)>) -> Vec<(String, u32)> {
    let mut counts: BTreeMap<String, u32> = BTreeMap::new();
    for (name, quantity) in lines {
        let total = counts.entry(name).or_insert(0);
        *total = total.saturating_add(quantity);
    }
    counts.into_iter().collect()
}

/// The user's answer to a sideboard prompt, with the prompt it answered.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Answer {
    prompt: Prompt,
    ids: Vec<u64>,
    match_id: Option<String>,
}

/// One game's latest sideboard prompt and the latest answer.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct SideboardTally {
    prompt: Option<Prompt>,
    answer: Option<Answer>,
}

impl SideboardTally {
    /// Keeps the latest sideboard prompt. An answer already taken stays:
    /// Forge sends the prompt again with a new version after the submit.
    pub(crate) fn read_prompt(&mut self, prompt: &Prompt) {
        if prompt.context == "sideboard" {
            self.prompt = Some(prompt.clone());
        }
    }

    /// Keeps a `SIDEBOARD_SUBMIT` that answers the latest prompt, together
    /// with that prompt; a later one replaces it (the user can submit again).
    /// An answer with more ids than the prompt has options is hostile: it is
    /// kept as an empty list, which makes no deck.
    pub(crate) fn read_action(&mut self, action: &GameAction) {
        if action.kind != "SIDEBOARD_SUBMIT" {
            return;
        }
        let Some(prompt) = &self.prompt else {
            return;
        };
        if action.prompt_version.is_none() || action.prompt_version != prompt.version {
            return;
        }
        let ids = if action.ordered_cards.len() > prompt.options.len() {
            Vec::new()
        } else {
            action.ordered_cards.clone()
        };
        self.answer = Some(Answer {
            prompt: prompt.clone(),
            ids,
            match_id: action.match_id.clone(),
        });
    }

    /// Forgets an answer that names another match.
    pub(crate) fn keep_match(&mut self, match_id: &str) {
        let foreign = self
            .answer
            .as_ref()
            .and_then(|answer| answer.match_id.as_deref())
            .is_some_and(|id| id != match_id);
        if foreign {
            self.answer = None;
        }
    }

    /// The deck the answer chose, from the options of the prompt it
    /// answered.
    fn submitted_deck(&self) -> Option<GameDeck> {
        let answer = self.answer.as_ref()?;
        let prompt = &answer.prompt;
        main_count(prompt)?;
        let mut names_by_id: BTreeMap<u64, &str> = BTreeMap::new();
        for option in &prompt.options {
            names_by_id.entry(option.id).or_insert(option.name.as_str());
        }
        let mut chosen: BTreeSet<u64> = BTreeSet::new();
        let mut main = Vec::new();
        for id in &answer.ids {
            if let Some(name) = names_by_id.get(id)
                && chosen.insert(*id)
            {
                main.push((*name).to_string());
            }
        }
        if main.len() < MIN_MAIN_CARDS {
            return None;
        }
        let mut side = Vec::new();
        let mut listed: BTreeSet<u64> = BTreeSet::new();
        for option in &prompt.options {
            if !chosen.contains(&option.id) && listed.insert(option.id) {
                side.push(option.name.clone());
            }
        }
        Some(GameDeck::from_names(main, side))
    }

    /// The deck the latest prompt shows: its first `main_count` options are
    /// the main deck and the rest the sideboard.
    fn shown_deck(&self) -> Option<GameDeck> {
        let prompt = self.prompt.as_ref()?;
        let main_count = main_count(prompt)?;
        let names: Vec<String> = prompt
            .options
            .iter()
            .map(|option| option.name.clone())
            .collect();
        let (main, side) = names.split_at(main_count);
        Some(GameDeck::from_names(main.to_vec(), side.to_vec()))
    }

    /// The latest prompt can give a main deck.
    fn has_usable_prompt(&self) -> bool {
        self.prompt.as_ref().and_then(main_count).is_some()
    }
}

/// A prompt's `main_count`, when it can give a main deck: known, not zero
/// and not above the option count.
fn main_count(prompt: &Prompt) -> Option<usize> {
    let main_count = usize::try_from(prompt.main_count?).ok()?;
    if main_count == 0 || main_count > prompt.options.len() {
        return None;
    }
    Some(main_count)
}

/// The deck of each game (game 0 is left out), in game order.
pub(crate) fn decks_by_game(
    games: &[GameRecord],
    match_deck: Option<&EndstepDeck>,
) -> Vec<(u32, Option<GameDeck>)> {
    let first = match_deck.map(GameDeck::from_match);
    let mut previous = first.clone();
    let mut out = Vec::new();
    for game in games {
        if game.number == 0 {
            continue;
        }
        let deck = if game.number == 1 {
            first.clone()
        } else {
            played_deck(game, games).or_else(|| previous.clone())
        };
        previous = deck.clone();
        out.push((game.number, deck));
    }
    out
}

/// The deck game `game` was played with, when its prompt or the next game's
/// shows it.
fn played_deck(game: &GameRecord, games: &[GameRecord]) -> Option<GameDeck> {
    if let Some(deck) = game.sideboard.submitted_deck() {
        return Some(deck);
    }
    if !game.sideboard.has_usable_prompt() {
        return None;
    }
    let next = games
        .iter()
        .find(|other| other.number == game.number.saturating_add(1))?;
    next.sideboard.shown_deck()
}
