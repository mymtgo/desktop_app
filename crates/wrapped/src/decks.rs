//! The deck and opponent cards: deck of the year, best deck, most faced,
//! best matchup, toughest rival, comebacks and play or draw. Win rates below
//! 50 never reach the bundle, and the rival card never carries one.

use std::collections::{HashMap, HashSet};

use crate::bundle::*;
use crate::input::*;
use crate::numbers::percent;

/// The fewest matches with a deck before it is the deck of the year.
const DECK_OF_YEAR_FLOOR: u32 = 10;
/// The fewest matches with a deck before it can be the best deck.
const BEST_DECK_FLOOR: u32 = 20;
/// The fewest matches against an archetype before it is the most faced.
const MOST_FACED_FLOOR: u32 = 10;
/// The fewest matches against an archetype before it is the best matchup.
const BEST_MATCHUP_FLOOR: u32 = 8;
/// The fewest wins against the toughest rival: the card says "and you still
/// beat them N times", so N must be worth saying.
const RIVAL_WINS_FLOOR: u32 = 3;
/// The fewest games won after a mulligan before the comebacks card shows.
const COMEBACKS_FLOOR: u32 = 5;
/// The fewest games with a known play or draw before the card shows.
const PLAY_DRAW_FLOOR: u32 = 40;
/// How many key cards a deck card shows.
const KEY_CARDS_SHOWN: usize = 5;

/// Matches, wins and losses against or with one deck or archetype. Draws are
/// matches and nothing else.
#[derive(Debug, Clone, Copy, Default)]
struct Tally {
    matches: u32,
    wins: u32,
    losses: u32,
}

impl Tally {
    fn add(&mut self, result: MatchResult) {
        self.matches += 1;
        match result {
            MatchResult::Win => self.wins += 1,
            MatchResult::Loss => self.losses += 1,
            MatchResult::Draw => {}
        }
    }

    /// True when wins are at least `percent` of matches, compared exactly
    /// (no rounding up into a rate the player did not reach).
    fn rate_at_least(&self, percent: u32) -> bool {
        self.matches > 0
            && u64::from(self.wins) * 100 >= u64::from(percent) * u64::from(self.matches)
    }

    /// The win rate to show: `Some` only when it is 50 or more.
    fn shown_rate(&self) -> Option<u32> {
        if self.rate_at_least(50) {
            Some(percent(self.wins, self.matches))
        } else {
            None
        }
    }

    /// True when this tally's win rate is strictly above the other's.
    fn rate_above(&self, other: &Tally) -> bool {
        u64::from(self.wins) * u64::from(other.matches)
            > u64::from(other.wins) * u64::from(self.matches)
    }

    fn rate_equals(&self, other: &Tally) -> bool {
        u64::from(self.wins) * u64::from(other.matches)
            == u64::from(other.wins) * u64::from(self.matches)
    }
}

/// One tally per deck id, for matches that used a known deck.
fn deck_tallies(input: &WrappedInput) -> HashMap<i64, Tally> {
    let mut tallies: HashMap<i64, Tally> = HashMap::new();
    for m in &input.matches {
        if let Some(id) = m.deck_id {
            tallies.entry(id).or_default().add(m.result);
        }
    }
    tallies
}

/// One tally per opponent archetype id (results from the player's side).
fn archetype_tallies(input: &WrappedInput) -> HashMap<i64, Tally> {
    let mut tallies: HashMap<i64, Tally> = HashMap::new();
    for m in &input.matches {
        if let Some(id) = m.opponent_archetype_id {
            tallies.entry(id).or_default().add(m.result);
        }
    }
    tallies
}

/// Deck ids with their tallies, only decks the input describes, in a stable
/// order (lowest id first) so ties never depend on hash order.
fn known_decks<'a>(
    input: &'a WrappedInput,
    tallies: &HashMap<i64, Tally>,
) -> Vec<(&'a InputDeck, Tally)> {
    let mut list: Vec<(&InputDeck, Tally)> = input
        .decks
        .iter()
        .filter_map(|deck| tallies.get(&deck.id).map(|tally| (deck, *tally)))
        .collect();
    list.sort_by_key(|(deck, _)| deck.id);
    list
}

fn known_archetypes<'a>(
    input: &'a WrappedInput,
    tallies: &HashMap<i64, Tally>,
) -> Vec<(&'a InputArchetype, Tally)> {
    let mut list: Vec<(&InputArchetype, Tally)> = input
        .archetypes
        .iter()
        .filter_map(|a| tallies.get(&a.id).map(|tally| (a, *tally)))
        .collect();
    list.sort_by_key(|(a, _)| a.id);
    list
}

/// The deck of the most matches (ties: more wins, then the lower id) and its
/// tally.
fn deck_of_year_pick(input: &WrappedInput) -> Option<(&InputDeck, Tally)> {
    let tallies = deck_tallies(input);
    let mut best: Option<(&InputDeck, Tally)> = None;
    for (deck, tally) in known_decks(input, &tallies) {
        let better = match best {
            None => true,
            Some((_, top)) => {
                tally.matches > top.matches
                    || (tally.matches == top.matches && tally.wins > top.wins)
            }
        };
        if better {
            best = Some((deck, tally));
        }
    }
    best
}

/// Trophy runs played with a deck.
fn trophies_of(input: &WrappedInput, deck_id: i64) -> u32 {
    input
        .runs
        .iter()
        .filter(|run| run.trophy && run.deck_id == Some(deck_id))
        .count() as u32
}

fn deck_card(input: &WrappedInput, deck: &InputDeck, tally: Tally) -> DeckCard {
    DeckCard {
        name: deck.name.clone(),
        format_label: deck.format_label.clone(),
        matches: tally.matches,
        wins: tally.wins,
        losses: tally.losses,
        win_rate: tally.shown_rate(),
        trophies: trophies_of(input, deck.id),
        art: deck.art.clone(),
        colors: deck.colors.clone(),
        cards: deck
            .key_cards
            .iter()
            .take(KEY_CARDS_SHOWN)
            .cloned()
            .collect(),
        rank: None,
    }
}

/// The most played deck, with its numbers; its win rate only when 50 or more.
/// Needs 10 matches with the deck.
pub fn deck_of_year(input: &WrappedInput) -> Option<WrappedCard> {
    let (deck, tally) = deck_of_year_pick(input)?;
    if tally.matches < DECK_OF_YEAR_FLOOR {
        return None;
    }
    Some(WrappedCard::DeckOfYear(deck_card(input, deck, tally)))
}

/// The deck with the best win rate (ties: more matches, then the lower id),
/// when it has 20 matches, wins 50% or more and is not the deck of the year
/// (that deck already has its own card).
pub fn best_deck(input: &WrappedInput) -> Option<WrappedCard> {
    let tallies = deck_tallies(input);
    let mut best: Option<(&InputDeck, Tally)> = None;
    for (deck, tally) in known_decks(input, &tallies) {
        if tally.matches < BEST_DECK_FLOOR || !tally.rate_at_least(50) {
            continue;
        }
        let better = match best {
            None => true,
            Some((_, top)) => {
                tally.rate_above(&top) || (tally.rate_equals(&top) && tally.matches > top.matches)
            }
        };
        if better {
            best = Some((deck, tally));
        }
    }
    let (deck, tally) = best?;
    if deck_of_year_pick(input).is_some_and(|(year_deck, _)| year_deck.id == deck.id) {
        return None;
    }
    Some(WrappedCard::BestDeck(deck_card(input, deck, tally)))
}

/// The distinct opponent archetypes the player faced.
fn unique_archetypes(input: &WrappedInput) -> u32 {
    let faced: HashSet<i64> = input
        .matches
        .iter()
        .filter_map(|m| m.opponent_archetype_id)
        .collect();
    faced.len() as u32
}

/// The archetype faced most (ties: the lower id), when it was faced 10 times.
pub fn most_faced(input: &WrappedInput) -> Option<WrappedCard> {
    let tallies = archetype_tallies(input);
    let mut best: Option<(&InputArchetype, Tally)> = None;
    for (archetype, tally) in known_archetypes(input, &tallies) {
        if best.is_none_or(|(_, top)| tally.matches > top.matches) {
            best = Some((archetype, tally));
        }
    }
    let (archetype, tally) = best?;
    if tally.matches < MOST_FACED_FLOOR {
        return None;
    }
    Some(WrappedCard::MostFaced(MostFacedCard {
        archetype: archetype.name.clone(),
        matches: tally.matches,
        unique_archetypes: unique_archetypes(input),
        look: archetype.look.clone(),
    }))
}

/// The archetype with the best win rate (ties: more matches, then the lower
/// id), with 8 matches against it and a win rate of 60% or more.
pub fn best_matchup(input: &WrappedInput) -> Option<WrappedCard> {
    let tallies = archetype_tallies(input);
    let mut best: Option<(&InputArchetype, Tally)> = None;
    for (archetype, tally) in known_archetypes(input, &tallies) {
        if tally.matches < BEST_MATCHUP_FLOOR || !tally.rate_at_least(60) {
            continue;
        }
        let better = match best {
            None => true,
            Some((_, top)) => {
                tally.rate_above(&top) || (tally.rate_equals(&top) && tally.matches > top.matches)
            }
        };
        if better {
            best = Some((archetype, tally));
        }
    }
    let (archetype, tally) = best?;
    Some(WrappedCard::BestMatchup(MatchupCard {
        archetype: archetype.name.clone(),
        matches: tally.matches,
        wins: tally.wins,
        win_rate: percent(tally.wins, tally.matches),
        look: archetype.look.clone(),
    }))
}

/// The archetype faced most among those the player lost to more than they
/// beat (ties: the lower id), shown only when the player still beat it at
/// least 3 times. It carries the wins, never a win rate.
pub fn toughest_rival(input: &WrappedInput) -> Option<WrappedCard> {
    let tallies = archetype_tallies(input);
    let mut best: Option<(&InputArchetype, Tally)> = None;
    for (archetype, tally) in known_archetypes(input, &tallies) {
        // Under 50%: not at 50 or more.
        if tally.rate_at_least(50) {
            continue;
        }
        if best.is_none_or(|(_, top)| tally.matches > top.matches) {
            best = Some((archetype, tally));
        }
    }
    let (archetype, tally) = best?;
    if tally.wins < RIVAL_WINS_FLOOR {
        return None;
    }
    Some(WrappedCard::ToughestRival(RivalCard {
        archetype: archetype.name.clone(),
        matches: tally.matches,
        wins: tally.wins,
        look: archetype.look.clone(),
    }))
}

/// Every game row of every match.
fn all_games(input: &WrappedInput) -> impl Iterator<Item = &InputGame> {
    input.matches.iter().flat_map(|m| m.games.iter())
}

/// Games won after a mulligan (5 needed) and the fewest turns any won game
/// took. Conceded games are ordinary losses, so they never appear here.
pub fn comebacks(input: &WrappedInput) -> Option<WrappedCard> {
    let wins_after_mulligan = all_games(input)
        .filter(|game| game.won == Some(true) && game.mulligans > 0)
        .count() as u32;
    if wins_after_mulligan < COMEBACKS_FLOOR {
        return None;
    }
    let quickest_win_turns = all_games(input)
        .filter(|game| game.won == Some(true))
        .filter_map(|game| game.turns)
        .filter(|turns| *turns > 0)
        .min();
    Some(WrappedCard::Comebacks(ComebacksCard {
        wins_after_mulligan,
        quickest_win_turns,
    }))
}

/// The better of on the play and on the draw, as a celebration: needs 40
/// games with a known side and result, and the better side must win 50% or
/// more. A tie goes to the play.
pub fn play_draw(input: &WrappedInput) -> Option<WrappedCard> {
    let mut play = (0u32, 0u32);
    let mut draw = (0u32, 0u32);
    for game in all_games(input) {
        let (Some(on_play), Some(won)) = (game.on_play, game.won) else {
            continue;
        };
        let side = if on_play { &mut play } else { &mut draw };
        side.0 += 1;
        if won {
            side.1 += 1;
        }
    }
    if play.0 + draw.0 < PLAY_DRAW_FLOOR {
        return None;
    }
    // Compare win rates exactly: play wins a tie, an empty side never wins.
    let play_better = draw.0 == 0
        || (play.0 > 0
            && u64::from(play.1) * u64::from(draw.0) >= u64::from(draw.1) * u64::from(play.0));
    let (side, (games, won)) = if play_better {
        (PlaySide::Play, play)
    } else {
        (PlaySide::Draw, draw)
    };
    if games == 0 || u64::from(won) * 2 < u64::from(games) {
        return None;
    }
    Some(WrappedCard::PlayDraw(PlayDrawCard {
        side,
        win_rate: percent(won, games),
        games,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profiles::{self, day_ms, deck, game_match, plain_match};

    fn tz() -> chrono_tz::Tz {
        chrono_tz::Europe::London
    }

    /// Adds `wins` wins, `losses` losses and `draws` draws for a deck against
    /// an archetype.
    fn add(
        input: &mut WrappedInput,
        deck_id: i64,
        archetype: Option<i64>,
        wins: u32,
        losses: u32,
        draws: u32,
    ) {
        let results = [
            (MatchResult::Win, wins),
            (MatchResult::Loss, losses),
            (MatchResult::Draw, draws),
        ];
        for (result, count) in results {
            for _ in 0..count {
                let day = 1 + (input.matches.len() as u32 % 300);
                let mut m = plain_match(day_ms(tz(), day, 20), "Modern", Some(deck_id), result);
                m.opponent_archetype_id = archetype;
                input.matches.push(m);
            }
        }
    }

    fn base() -> WrappedInput {
        let mut input = profiles::empty(tz());
        input.decks.push(deck(
            1,
            "Mono Red Burn",
            "Modern",
            &["R"],
            &["A", "B", "C", "D", "E", "F"],
        ));
        input
            .decks
            .push(deck(2, "Amulet Titan", "Modern", &["G"], &[]));
        input
            .archetypes
            .push(profiles::archetype(1, "Domain Zoo", &["R"], &[]));
        input
            .archetypes
            .push(profiles::archetype(2, "Living End", &["B"], &[]));
        input
    }

    #[test]
    fn deck_of_year_needs_ten_matches_and_hides_a_losing_rate() {
        let mut input = base();
        add(&mut input, 1, None, 3, 6, 0);
        assert_eq!(deck_of_year(&input), None, "nine matches");
        add(&mut input, 1, None, 0, 1, 0);
        let Some(WrappedCard::DeckOfYear(card)) = deck_of_year(&input) else {
            panic!("a card");
        };
        assert_eq!((card.matches, card.wins, card.losses), (10, 3, 7));
        assert_eq!(card.win_rate, None);
        assert_eq!(card.cards.len(), 5, "five key cards at most");
        assert_eq!(card.rank, None);
    }

    #[test]
    fn deck_of_year_shows_a_rate_from_fifty_and_counts_trophies() {
        let mut input = base();
        add(&mut input, 1, None, 5, 5, 0);
        input.runs.push(InputRun {
            deck_id: Some(1),
            limited: false,
            wins: 5,
            losses: 0,
            trophy: true,
        });
        input.runs.push(InputRun {
            deck_id: Some(2),
            limited: false,
            wins: 5,
            losses: 0,
            trophy: true,
        });
        let Some(WrappedCard::DeckOfYear(card)) = deck_of_year(&input) else {
            panic!("a card");
        };
        assert_eq!(card.win_rate, Some(50));
        assert_eq!(card.trophies, 1);
    }

    #[test]
    fn draws_count_as_matches_never_wins() {
        let mut input = base();
        add(&mut input, 1, None, 4, 0, 6);
        let Some(WrappedCard::DeckOfYear(card)) = deck_of_year(&input) else {
            panic!("a card");
        };
        assert_eq!((card.matches, card.wins), (10, 4));
        assert_eq!(card.win_rate, None, "4 of 10 is under 50");
    }

    #[test]
    fn best_deck_needs_twenty_matches_and_fifty_percent() {
        let mut input = base();
        add(&mut input, 1, None, 15, 15, 0); // deck of the year
        add(&mut input, 2, None, 12, 7, 0);
        assert_eq!(best_deck(&input), None, "19 matches");
        add(&mut input, 2, None, 0, 1, 0);
        let Some(WrappedCard::BestDeck(card)) = best_deck(&input) else {
            panic!("20 matches at 60% is enough");
        };
        assert_eq!(card.win_rate, Some(60));
        let mut input = base();
        add(&mut input, 1, None, 15, 15, 0);
        add(&mut input, 2, None, 14, 6, 0);
        let Some(WrappedCard::BestDeck(card)) = best_deck(&input) else {
            panic!("a card");
        };
        assert_eq!(card.name, "Amulet Titan");
        assert_eq!(card.win_rate, Some(70));
    }

    #[test]
    fn best_deck_is_never_the_deck_of_the_year_and_never_under_fifty() {
        let mut input = base();
        add(&mut input, 1, None, 40, 10, 0);
        add(&mut input, 2, None, 6, 14, 0);
        assert_eq!(
            best_deck(&input),
            None,
            "the best deck is the deck of the year"
        );
        let mut input = base();
        add(&mut input, 1, None, 8, 22, 0);
        add(&mut input, 2, None, 8, 12, 0);
        assert_eq!(best_deck(&input), None, "nobody reaches 50%");
    }

    #[test]
    fn most_faced_needs_ten_and_counts_unique_archetypes() {
        let mut input = base();
        add(&mut input, 1, Some(1), 5, 4, 0);
        add(&mut input, 1, Some(2), 2, 0, 0);
        assert_eq!(most_faced(&input), None, "nine matches");
        add(&mut input, 1, Some(1), 1, 0, 0);
        let Some(WrappedCard::MostFaced(card)) = most_faced(&input) else {
            panic!("a card");
        };
        assert_eq!(
            (
                card.archetype.as_str(),
                card.matches,
                card.unique_archetypes
            ),
            ("Domain Zoo", 10, 2)
        );
    }

    #[test]
    fn best_matchup_needs_eight_matches() {
        let mut input = base();
        add(&mut input, 1, Some(1), 7, 0, 0);
        assert_eq!(best_matchup(&input), None, "seven matches");
        add(&mut input, 1, Some(1), 1, 0, 0);
        assert!(best_matchup(&input).is_some(), "eight matches");
    }

    #[test]
    fn best_matchup_edges() {
        // 5 of 8 is 62.5 percent: enough.
        let mut input = base();
        add(&mut input, 1, Some(1), 5, 3, 0);
        let Some(WrappedCard::BestMatchup(card)) = best_matchup(&input) else {
            panic!("a card");
        };
        assert_eq!((card.matches, card.wins, card.win_rate), (8, 5, 63));
        // 4 of 8 is 50 percent: not enough.
        let mut input = base();
        add(&mut input, 1, Some(1), 4, 4, 0);
        assert_eq!(best_matchup(&input), None);
        // 6 of 10 is exactly 60: enough; 29 of 50 is 58: not.
        let mut input = base();
        add(&mut input, 1, Some(1), 6, 4, 0);
        assert!(best_matchup(&input).is_some());
        let mut input = base();
        add(&mut input, 1, Some(1), 29, 21, 0);
        assert_eq!(best_matchup(&input), None);
    }

    #[test]
    fn best_matchup_picks_the_highest_rate() {
        let mut input = base();
        add(&mut input, 1, Some(1), 7, 3, 0);
        add(&mut input, 1, Some(2), 9, 1, 0);
        let Some(WrappedCard::BestMatchup(card)) = best_matchup(&input) else {
            panic!("a card");
        };
        assert_eq!(card.archetype, "Living End");
    }

    #[test]
    fn toughest_rival_is_under_fifty_needs_three_wins_and_has_no_rate() {
        let mut input = base();
        add(&mut input, 1, Some(1), 2, 8, 0);
        assert_eq!(toughest_rival(&input), None, "two wins");
        add(&mut input, 1, Some(1), 1, 0, 0);
        let Some(card) = toughest_rival(&input) else {
            panic!("a card");
        };
        let value = serde_json::to_value(&card).expect("serialise");
        assert_eq!(value["kind"], "toughestRival");
        assert_eq!(value["wins"], 3);
        assert_eq!(value["matches"], 11);
        assert!(
            value.get("winRate").is_none(),
            "the rival never carries a win rate"
        );
    }

    #[test]
    fn toughest_rival_skips_archetypes_the_player_beats() {
        let mut input = base();
        add(&mut input, 1, Some(1), 8, 2, 0);
        assert_eq!(toughest_rival(&input), None, "80% is not a rival");
        // 5 of 10 is 50%: not under 50.
        let mut input = base();
        add(&mut input, 1, Some(1), 5, 5, 0);
        assert_eq!(toughest_rival(&input), None);
        // The most faced of the under-50 archetypes wins the card.
        let mut input = base();
        add(&mut input, 1, Some(1), 3, 5, 0);
        add(&mut input, 1, Some(2), 4, 9, 0);
        let Some(WrappedCard::ToughestRival(card)) = toughest_rival(&input) else {
            panic!("a card");
        };
        assert_eq!(card.archetype, "Living End");
    }

    fn with_games(input: &mut WrappedInput, games: Vec<InputGame>) {
        let mut m = game_match(day_ms(tz(), 10, 20), "Modern", 1, MatchResult::Win, 1, 1);
        m.game_count = games.len() as u32;
        m.games = games;
        input.matches.push(m);
    }

    fn game(won: bool, mulligans: u32, turns: u32, on_play: bool) -> InputGame {
        InputGame {
            won: Some(won),
            on_play: Some(on_play),
            mulligans,
            turns: Some(turns),
        }
    }

    #[test]
    fn comebacks_need_five_wins_after_a_mulligan() {
        let mut input = base();
        let mut games = vec![game(true, 1, 9, true); 4];
        games.push(game(false, 1, 8, true)); // lost after a mulligan: no
        games.push(game(true, 0, 4, true)); // won, no mulligan: quickest win only
        with_games(&mut input, games);
        assert_eq!(comebacks(&input), None, "four comeback wins");
        with_games(&mut input, vec![game(true, 2, 11, false)]);
        let Some(WrappedCard::Comebacks(card)) = comebacks(&input) else {
            panic!("a card");
        };
        assert_eq!(card.wins_after_mulligan, 5);
        assert_eq!(card.quickest_win_turns, Some(4));
    }

    #[test]
    fn play_draw_needs_forty_games_and_celebrates_the_better_side() {
        let mut input = base();
        // 20 on the play (14 won), 19 on the draw (all won): 39 games.
        let mut games: Vec<InputGame> = (0..20).map(|n| game(n < 14, 0, 6, true)).collect();
        games.extend((0..19).map(|_| game(true, 0, 6, false)));
        with_games(&mut input, games);
        assert_eq!(play_draw(&input), None, "39 games");
        with_games(&mut input, vec![game(false, 0, 6, false)]);
        let Some(WrappedCard::PlayDraw(card)) = play_draw(&input) else {
            panic!("a card");
        };
        // Draw: 19 of 20 = 95%; play 14 of 20 = 70%.
        assert_eq!(
            (card.side, card.win_rate, card.games),
            (PlaySide::Draw, 95, 20)
        );
    }

    #[test]
    fn play_draw_is_left_out_when_the_better_side_is_under_fifty() {
        let mut input = base();
        let mut games: Vec<InputGame> = (0..20).map(|n| game(n < 8, 0, 6, true)).collect();
        games.extend((0..20).map(|n| game(n < 9, 0, 6, false)));
        with_games(&mut input, games);
        assert_eq!(play_draw(&input), None, "45% on the draw is the best side");
        // Games with an unknown side do not count toward the floor.
        let mut input = base();
        let games: Vec<InputGame> = (0..50)
            .map(|_| InputGame {
                won: Some(true),
                on_play: None,
                mulligans: 0,
                turns: Some(5),
            })
            .collect();
        with_games(&mut input, games);
        assert_eq!(play_draw(&input), None);
    }
}
