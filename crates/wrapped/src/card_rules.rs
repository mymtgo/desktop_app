//! The per-card cards: the MVP, the opening hand favourite and the sideboard
//! hero. Card rows arrive per deck; a card played in several decks is summed
//! by name first.

use crate::bundle::*;
use crate::input::*;
use crate::numbers::percent;

/// The fewest games a card is cast in before it can be the MVP.
const MVP_CAST_FLOOR: u32 = 30;
/// The fewest games with a known opening hand before the card shows.
const OPENING_HAND_FLOOR: u32 = 30;
/// The fewest times a card is sided in before it is the sideboard hero.
const SIDEBOARD_FLOOR: u32 = 10;
/// How many honourable mentions a card shows.
const RUNNERS_UP_SHOWN: usize = 3;

/// One card summed over every deck that ran it.
#[derive(Debug, Clone)]
struct SummedCard {
    name: String,
    is_land: bool,
    cast_games: u32,
    cast_won: u32,
    total_cast: u32,
    opening_hand_games: u32,
    sided_in_games: u32,
    art: Art,
    image: Art,
    /// The decks that ran it, each once, lowest id first.
    deck_ids: Vec<i64>,
    /// The most cast games in one deck so far: the images come from that row.
    best_row_cast: u32,
}

/// Sum `input.cards` by card name, in a stable order (by name).
fn sum_cards(input: &WrappedInput) -> Vec<SummedCard> {
    let mut summed: Vec<SummedCard> = Vec::new();
    for row in &input.cards {
        let index = match summed.iter().position(|card| card.name == row.name) {
            Some(index) => index,
            None => {
                summed.push(SummedCard {
                    name: row.name.clone(),
                    is_land: false,
                    cast_games: 0,
                    cast_won: 0,
                    total_cast: 0,
                    opening_hand_games: 0,
                    sided_in_games: 0,
                    art: row.art.clone(),
                    image: row.image.clone(),
                    deck_ids: Vec::new(),
                    best_row_cast: row.cast_games,
                });
                summed.len() - 1
            }
        };
        let card = &mut summed[index];
        card.is_land |= row.is_land;
        card.cast_games += row.cast_games;
        card.cast_won += row.cast_won;
        card.total_cast += row.total_cast;
        card.opening_hand_games += row.opening_hand_games;
        card.sided_in_games += row.sided_in_games;
        if row.cast_games > card.best_row_cast {
            card.best_row_cast = row.cast_games;
            card.art = row.art.clone();
            card.image = row.image.clone();
        }
        if !card.deck_ids.contains(&row.deck_id) {
            card.deck_ids.push(row.deck_id);
            card.deck_ids.sort_unstable();
        }
    }
    summed.sort_by(|a, b| a.name.cmp(&b.name));
    summed
}

/// Games and wins of the decks that ran a card: its baseline.
fn baseline(input: &WrappedInput, card: &SummedCard) -> (u32, u32) {
    let mut games = 0;
    let mut won = 0;
    for deck_games in input
        .deck_games
        .iter()
        .filter(|d| card.deck_ids.contains(&d.deck_id))
    {
        games += deck_games.games;
        won += deck_games.won;
    }
    (games, won)
}

/// A card's impact: its win rate when cast minus the win rate of every game
/// of the decks that ran it, in whole percentage points. `None` without a
/// baseline.
fn impact_points(input: &WrappedInput, card: &SummedCard) -> Option<i64> {
    let (games, won) = baseline(input, card);
    if games == 0 || card.cast_games == 0 {
        return None;
    }
    let cast_rate = f64::from(card.cast_won) / f64::from(card.cast_games);
    let deck_rate = f64::from(won) / f64::from(games);
    Some(((cast_rate - deck_rate) * 100.0).round() as i64)
}

fn runners_up(ranked: &[(String, u32)]) -> Vec<RunnerUp> {
    ranked
        .iter()
        .take(RUNNERS_UP_SHOWN)
        .map(|(name, value)| RunnerUp {
            name: name.clone(),
            value: *value,
        })
        .collect()
}

/// The player's MVP: the non-land card with the highest impact, cast in 30 or
/// more games and adding more than 0 points (ties: more cast games, then
/// name). Its win rate shows only at 50 or more. Runners up are the next three
/// cards that pass the same rule, valued in impact points.
pub fn mvp_card(input: &WrappedInput) -> Option<WrappedCard> {
    let mut candidates: Vec<(SummedCard, u32)> = Vec::new();
    for card in sum_cards(input) {
        if card.is_land || card.cast_games < MVP_CAST_FLOOR {
            continue;
        }
        match impact_points(input, &card) {
            Some(points) if points > 0 => candidates.push((card, points as u32)),
            _ => {}
        }
    }
    candidates.sort_by(|a, b| {
        b.1.cmp(&a.1)
            .then_with(|| b.0.cast_games.cmp(&a.0.cast_games))
            .then_with(|| a.0.name.cmp(&b.0.name))
    });
    let (mvp, impact) = candidates.first()?.clone();
    let rest: Vec<(String, u32)> = candidates
        .iter()
        .skip(1)
        .map(|(card, points)| (card.name.clone(), *points))
        .collect();
    let rate_shown = u64::from(mvp.cast_won) * 2 >= u64::from(mvp.cast_games);
    Some(WrappedCard::MvpCard(MvpCardCard {
        name: mvp.name,
        cast: mvp.total_cast,
        games: mvp.cast_games,
        win_rate: rate_shown.then(|| percent(mvp.cast_won, mvp.cast_games)),
        impact,
        art: mvp.art,
        image: mvp.image,
        runners_up: runners_up(&rest),
    }))
}

/// Sort cards by a count, biggest first, ties by name; only cards with a count.
fn ranked_by<F: Fn(&SummedCard) -> u32>(cards: Vec<SummedCard>, count: F) -> Vec<SummedCard> {
    let mut list: Vec<SummedCard> = cards.into_iter().filter(|card| count(card) > 0).collect();
    list.sort_by(|a, b| count(b).cmp(&count(a)).then_with(|| a.name.cmp(&b.name)));
    list
}

/// A count card (opening hand or sideboard): the top card, its count out of
/// `games`, and the next three.
fn count_card(
    ranked: &[SummedCard],
    count: impl Fn(&SummedCard) -> u32,
    games: u32,
) -> Option<CardCountCard> {
    let top = ranked.first()?;
    let rest: Vec<(String, u32)> = ranked
        .iter()
        .skip(1)
        .map(|card| (card.name.clone(), count(card)))
        .collect();
    Some(CardCountCard {
        name: top.name.clone(),
        count: count(top),
        games,
        art: top.art.clone(),
        image: top.image.clone(),
        runners_up: runners_up(&rest),
    })
}

/// The non-land card most often in an opening hand, out of the games of the
/// decks that ran it. Needs 30 games with a known opening hand in the year.
pub fn opening_hand(input: &WrappedInput) -> Option<WrappedCard> {
    let known: u32 = input.deck_games.iter().map(|d| d.opening_hand_games).sum();
    if known < OPENING_HAND_FLOOR {
        return None;
    }
    let nonland: Vec<SummedCard> = sum_cards(input)
        .into_iter()
        .filter(|card| !card.is_land)
        .collect();
    let ranked = ranked_by(nonland, |card| card.opening_hand_games);
    let top = ranked.first()?;
    let games: u32 = input
        .deck_games
        .iter()
        .filter(|d| top.deck_ids.contains(&d.deck_id))
        .map(|d| d.opening_hand_games)
        .sum();
    count_card(&ranked, |card| card.opening_hand_games, games).map(WrappedCard::OpeningHand)
}

/// The card sided in most, when it was sided in 10 or more times, out of the
/// games after sideboarding of the decks that ran it.
pub fn sideboard_hero(input: &WrappedInput) -> Option<WrappedCard> {
    let ranked = ranked_by(sum_cards(input), |card| card.sided_in_games);
    let top = ranked.first()?;
    if top.sided_in_games < SIDEBOARD_FLOOR {
        return None;
    }
    let games: u32 = input
        .deck_games
        .iter()
        .filter(|d| top.deck_ids.contains(&d.deck_id))
        .map(|d| d.postboard_games)
        .sum();
    if games == 0 {
        return None;
    }
    count_card(&ranked, |card| card.sided_in_games, games).map(WrappedCard::SideboardHero)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profiles::{self, card};

    fn input_with(rows: Vec<InputCard>) -> WrappedInput {
        let mut input = profiles::empty(chrono_tz::Europe::London);
        // Deck 1 wins 60% of 1000 games; deck 2 wins 50% of 200.
        input.deck_games.push(InputDeckGames {
            deck_id: 1,
            games: 1000,
            won: 600,
            opening_hand_games: 900,
            postboard_games: 400,
        });
        input.deck_games.push(InputDeckGames {
            deck_id: 2,
            games: 200,
            won: 100,
            opening_hand_games: 100,
            postboard_games: 50,
        });
        input.cards = rows;
        input
    }

    fn mvp(input: &WrappedInput) -> MvpCardCard {
        match mvp_card(input) {
            Some(WrappedCard::MvpCard(card)) => card,
            other => panic!("an mvp card, got {other:?}"),
        }
    }

    #[test]
    fn mvp_needs_thirty_cast_games_and_a_positive_impact() {
        // 29 cast games: out. 30 at 70% against 60%: in, +10.
        let input = input_with(vec![card(1, "Ragavan", false, (29, 24), 0, 0)]);
        assert_eq!(mvp_card(&input), None);
        let input = input_with(vec![card(1, "Ragavan", false, (30, 21), 0, 0)]);
        let found = mvp(&input);
        assert_eq!(
            (found.name.as_str(), found.games, found.impact),
            ("Ragavan", 30, 10)
        );
        assert_eq!(found.win_rate, Some(70));
        // At the deck's own rate there is no impact.
        let input = input_with(vec![card(1, "Plains Walker", false, (100, 60), 0, 0)]);
        assert_eq!(mvp_card(&input), None);
        // Below it, still none.
        let input = input_with(vec![card(1, "Dud", false, (100, 50), 0, 0)]);
        assert_eq!(mvp_card(&input), None);
    }

    #[test]
    fn lands_never_count_as_mvp() {
        let input = input_with(vec![
            card(1, "Sacred Foundry", true, (900, 700), 0, 0),
            card(1, "Ragavan", false, (40, 26), 0, 0),
        ]);
        assert_eq!(mvp(&input).name, "Ragavan");
        let input = input_with(vec![card(1, "Sacred Foundry", true, (900, 700), 0, 0)]);
        assert_eq!(mvp_card(&input), None);
    }

    #[test]
    fn impact_rounds_to_whole_points() {
        // 43 of 70 is 61.43% against 60%: 1.43 points, 1.
        let input = input_with(vec![card(1, "Rounded Down", false, (70, 43), 0, 0)]);
        assert_eq!(mvp(&input).impact, 1);
        // 44 of 70 is 62.86%, 2.86 points: 3.
        let input = input_with(vec![card(1, "Rounded Up", false, (70, 44), 0, 0)]);
        assert_eq!(mvp(&input).impact, 3);
        // 42 of 70 is exactly 60%: 0, no card. 0.4 points rounds to 0 too.
        let input = input_with(vec![card(1, "Flat", false, (70, 42), 0, 0)]);
        assert_eq!(mvp_card(&input), None);
    }

    #[test]
    fn a_card_in_two_decks_uses_both_decks_baseline() {
        // Baseline: 700 of 1200 = 58.33%. Cast: 30 of 40 = 75%: 17 points.
        let input = input_with(vec![
            card(1, "Shared Card", false, (20, 15), 0, 0),
            card(2, "Shared Card", false, (20, 15), 0, 0),
        ]);
        let found = mvp(&input);
        assert_eq!((found.games, found.impact), (40, 17));
    }

    #[test]
    fn a_win_rate_under_fifty_is_never_set() {
        // Deck 3 wins 20% of its games. The card wins 48% of the games it is
        // cast in: 28 points of impact, but 48 is under 50, so no win rate.
        let mut input = input_with(vec![card(3, "Hero", false, (50, 24), 0, 0)]);
        input.deck_games.push(InputDeckGames {
            deck_id: 3,
            games: 100,
            won: 20,
            opening_hand_games: 0,
            postboard_games: 0,
        });
        let found = mvp(&input);
        assert_eq!(found.impact, 28);
        assert_eq!(found.win_rate, None);
    }

    #[test]
    fn runners_up_are_the_next_three_by_impact() {
        let input = input_with(vec![
            card(1, "First", false, (50, 40), 0, 0),
            card(1, "Second", false, (50, 38), 0, 0),
            card(1, "Third", false, (50, 36), 0, 0),
            card(1, "Fourth", false, (50, 35), 0, 0),
            card(1, "Fifth", false, (50, 34), 0, 0),
        ]);
        let found = mvp(&input);
        assert_eq!(found.name, "First");
        let names: Vec<&str> = found.runners_up.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["Second", "Third", "Fourth"]);
        assert_eq!(found.runners_up[0].value, 16);
    }

    #[test]
    fn opening_hand_needs_thirty_known_hands_and_skips_lands() {
        let mut input = input_with(vec![
            card(1, "Island", true, (0, 0), 800, 0),
            card(1, "Ragavan", false, (0, 0), 300, 0),
        ]);
        input.deck_games = vec![InputDeckGames {
            deck_id: 1,
            games: 29,
            won: 15,
            opening_hand_games: 29,
            postboard_games: 0,
        }];
        assert_eq!(opening_hand(&input), None, "29 hands");
        input.deck_games[0].opening_hand_games = 30;
        let Some(WrappedCard::OpeningHand(found)) = opening_hand(&input) else {
            panic!("a card");
        };
        assert_eq!(
            (found.name.as_str(), found.count, found.games),
            ("Ragavan", 300, 30)
        );
        assert!(found.runners_up.is_empty(), "the land is no runner up");
    }

    #[test]
    fn opening_hand_games_come_from_the_decks_that_ran_the_card() {
        let input = input_with(vec![
            card(2, "Counterspell", false, (0, 0), 40, 0),
            card(1, "Ragavan", false, (0, 0), 30, 0),
        ]);
        let Some(WrappedCard::OpeningHand(found)) = opening_hand(&input) else {
            panic!("a card");
        };
        assert_eq!(
            (found.name.as_str(), found.count, found.games),
            ("Counterspell", 40, 100)
        );
        assert_eq!(
            found.runners_up,
            vec![RunnerUp {
                name: "Ragavan".to_string(),
                value: 30
            }]
        );
    }

    #[test]
    fn sideboard_hero_needs_ten_sided_in_games() {
        let input = input_with(vec![card(1, "Blood Moon", false, (0, 0), 0, 9)]);
        assert_eq!(sideboard_hero(&input), None);
        let input = input_with(vec![
            card(1, "Blood Moon", false, (0, 0), 0, 10),
            card(1, "Wear // Tear", false, (0, 0), 0, 71),
            card(1, "A", false, (0, 0), 0, 5),
            card(1, "B", false, (0, 0), 0, 4),
            card(1, "C", false, (0, 0), 0, 3),
            card(1, "D", false, (0, 0), 0, 2),
        ]);
        let Some(WrappedCard::SideboardHero(found)) = sideboard_hero(&input) else {
            panic!("a card");
        };
        assert_eq!(
            (found.name.as_str(), found.count, found.games),
            ("Wear // Tear", 71, 400)
        );
        assert_eq!(found.runners_up.len(), 3, "capped at three");
        assert_eq!(
            found.runners_up[0],
            RunnerUp {
                name: "Blood Moon".to_string(),
                value: 10
            }
        );
    }
}
