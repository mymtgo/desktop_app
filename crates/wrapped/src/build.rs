//! Turns a `WrappedInput` into the bundle: totals, every card's floor and
//! celebrate rule, the card order and the player type. Pure: no clock, no
//! database, no filesystem.

use crate::bundle::*;
use crate::card_rules::*;
use crate::decks::*;
use crate::input::*;
use crate::numbers::{formats, hours_played, when_you_play, year_in_numbers};
use crate::story::{limited, player_type, trophies};

/// The bundle for a player's year, or `None` below the 30-match floor.
///
/// Card order is the story's order (the `WrappedCard` enum order). A card
/// that fails its floor or celebrate rule is left out, never shown weak.
pub fn build(input: &WrappedInput) -> Option<WrappedBundle> {
    if (input.matches.len() as u32) < WRAPPED_FLOOR {
        return None;
    }
    let mut totals = totals(input);
    totals.hours = hours_played(&input.matches);

    let mut cards = vec![
        WrappedCard::MagicYear(MagicYearCard {}),
        WrappedCard::Banned(BannedCard {}),
        year_in_numbers(input, &totals),
    ];
    let optional = [
        formats(input),
        when_you_play(input),
        deck_of_year(input),
        best_deck(input),
        mvp_card(input),
        opening_hand(input),
        sideboard_hero(input),
        most_faced(input),
        best_matchup(input),
        toughest_rival(input),
        comebacks(input),
        play_draw(input),
        trophies(input),
        limited(input),
    ];
    cards.extend(optional.into_iter().flatten());

    cards.push(player_type(input, &totals, &cards));
    cards.push(WrappedCard::Wrap(WrapCard {}));

    Some(WrappedBundle {
        version: BUNDLE_VERSION,
        year: input.year,
        window: input.window,
        built_at_ms: input.built_at_ms,
        first_year: input.first_year,
        totals,
        cards,
    })
}

/// Matches, games and results. Draws are matches but never wins.
fn totals(input: &WrappedInput) -> Totals {
    let mut totals = Totals {
        matches: input.matches.len() as u32,
        games: 0,
        wins: 0,
        losses: 0,
        draws: 0,
        hours: 0,
    };
    for m in &input.matches {
        totals.games += m.game_count;
        match m.result {
            MatchResult::Win => totals.wins += 1,
            MatchResult::Loss => totals.losses += 1,
            MatchResult::Draw => totals.draws += 1,
        }
    }
    totals
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profiles;

    fn kinds(bundle: &WrappedBundle) -> Vec<String> {
        bundle
            .cards
            .iter()
            .map(|card| {
                let value = serde_json::to_value(card).expect("serialise");
                value["kind"].as_str().expect("kind").to_string()
            })
            .collect()
    }

    #[test]
    fn below_the_floor_there_is_no_bundle() {
        let mut input = profiles::light();
        input.matches.truncate(29);
        assert_eq!(build(&input), None);
    }

    #[test]
    fn thirty_matches_give_a_bundle_with_the_fixed_ends() {
        let bundle = build(&profiles::light()).expect("a bundle");
        let kinds = kinds(&bundle);
        assert_eq!(&kinds[..3], ["magicYear", "banned", "yearInNumbers"]);
        assert_eq!(&kinds[kinds.len() - 2..], ["playerType", "wrap"]);
        assert_eq!(bundle.totals.matches, 30);
    }

    #[test]
    fn draws_are_matches_never_wins() {
        let bundle = build(&profiles::light()).expect("a bundle");
        let totals = bundle.totals;
        assert_eq!(totals.draws, 1);
        assert_eq!(totals.wins + totals.losses + totals.draws, totals.matches);
    }
}
