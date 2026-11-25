//! The closing cards: trophies, limited and the player type.

use std::collections::HashSet;

use crate::bundle::*;
use crate::input::*;

/// The fewest drafts before the limited card shows.
const LIMITED_FLOOR: u32 = 3;
/// A league run with this many wins is worth a card even without a trophy.
const STRONG_RUN_WINS: u32 = 4;
/// Matches that make a Grinder.
const GRINDER_MATCHES: u32 = 500;
/// A Deck Loyalist plays one deck in at least this percent of matches.
const LOYALIST_PERCENT: u32 = 60;
/// Formats that make a Format Explorer.
const EXPLORER_FORMATS: usize = 4;
/// The most pills a player type shows.
const PILLS_SHOWN: usize = 3;

/// The trophy cabinet: league and course trophies, runs, Challenge Top 8s and
/// the latest top-cut finish. Shows only when there is a trophy, a top-cut
/// finish or a run with 4 or more wins, so it never shows an empty cabinet.
pub fn trophies(input: &WrappedInput) -> Option<WrappedCard> {
    let league_trophies = input.runs.iter().filter(|run| run.trophy).count() as u32;
    let top_cuts: Vec<&InputFinish> = input
        .finishes
        .iter()
        .filter(|finish| finish.top_cut)
        .collect();
    let strong_run = input.runs.iter().any(|run| run.wins >= STRONG_RUN_WINS);
    if league_trophies == 0 && top_cuts.is_empty() && !strong_run {
        return None;
    }
    let best_finish = top_cuts
        .iter()
        .max_by_key(|finish| finish.finished_at_ms)
        .map(|finish| finish.text.clone());
    Some(WrappedCard::Trophies(TrophiesCard {
        league_trophies,
        league_runs: input.runs.len() as u32,
        top8s: top_cuts.len() as u32,
        best_finish,
    }))
}

/// The most common colour pair across drafts (ties: the name that sorts
/// first).
fn favourite_pair(drafts: &[InputDraft]) -> Option<String> {
    let mut counts: Vec<(String, u32)> = Vec::new();
    for pair in drafts.iter().filter_map(|draft| draft.colour_pair.as_ref()) {
        match counts.iter_mut().find(|(name, _)| name == pair) {
            Some(entry) => entry.1 += 1,
            None => counts.push((pair.clone(), 1)),
        }
    }
    counts.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    counts.into_iter().next().map(|(name, _)| name)
}

/// The best draft record as `W-L`: most wins, then fewest losses. A record
/// with no wins is not worth showing.
fn best_record(drafts: &[InputDraft]) -> Option<String> {
    let best = drafts
        .iter()
        .max_by(|a, b| a.wins.cmp(&b.wins).then_with(|| b.losses.cmp(&a.losses)))?;
    (best.wins > 0).then(|| format!("{}-{}", best.wins, best.losses))
}

/// Drafts done, the favourite colour pair and the best record. Needs 3 drafts.
pub fn limited(input: &WrappedInput) -> Option<WrappedCard> {
    let drafts = input.drafts.len() as u32;
    if drafts < LIMITED_FLOOR {
        return None;
    }
    Some(WrappedCard::Limited(LimitedCard {
        drafts,
        colour_pair: favourite_pair(&input.drafts),
        best_record: best_record(&input.drafts),
    }))
}

/// 1284 as "1,284".
fn with_commas(number: u32) -> String {
    let digits = number.to_string();
    let mut out = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

fn count_label(number: u32, one: &str, many: &str) -> String {
    let word = if number == 1 { one } else { many };
    format!("{} {word}", with_commas(number))
}

fn distinct_formats(input: &WrappedInput) -> usize {
    let labels: HashSet<&str> = input
        .matches
        .iter()
        .map(|m| m.format_label.as_str())
        .collect();
    labels.len()
}

fn deck_of_year_card(cards: &[WrappedCard]) -> Option<&DeckCard> {
    cards.iter().find_map(|card| match card {
        WrappedCard::DeckOfYear(deck) => Some(deck),
        _ => None,
    })
}

fn comebacks_card(cards: &[WrappedCard]) -> Option<&ComebacksCard> {
    cards.iter().find_map(|card| match card {
        WrappedCard::Comebacks(found) => Some(found),
        _ => None,
    })
}

fn trophies_card(cards: &[WrappedCard]) -> Option<&TrophiesCard> {
    cards.iter().find_map(|card| match card {
        WrappedCard::Trophies(found) => Some(found),
        _ => None,
    })
}

fn limited_card(cards: &[WrappedCard]) -> Option<&LimitedCard> {
    cards.iter().find_map(|card| match card {
        WrappedCard::Limited(found) => Some(found),
        _ => None,
    })
}

fn favourite_format(cards: &[WrappedCard]) -> Option<&str> {
    cards.iter().find_map(|card| match card {
        WrappedCard::Formats(found) => Some(found.favourite.label.as_str()),
        _ => None,
    })
}

/// The player type: one positive label from a fixed table, first match wins.
///
/// 1. Grinder: 500 or more matches.
/// 2. Limited Lover: half or more of the matches are limited.
/// 3. Deck Loyalist: the deck of the year is 60% or more of all matches.
/// 4. Comeback Kid: the comebacks card is in the bundle.
/// 5. Format Explorer: 4 or more formats played.
/// 6. All-Rounder: everyone else.
///
/// `cards` are the cards built so far; the label reads them rather than
/// recounting, so it can never disagree with what the story showed.
pub fn player_type(input: &WrappedInput, totals: &Totals, cards: &[WrappedCard]) -> WrappedCard {
    let limited_matches = input.matches.iter().filter(|m| m.limited).count() as u32;
    let deck = deck_of_year_card(cards);
    let label = if totals.matches >= GRINDER_MATCHES {
        PlayerType::Grinder
    } else if limited_matches * 2 >= totals.matches {
        PlayerType::LimitedLover
    } else if deck.is_some_and(|d| d.matches * 100 >= LOYALIST_PERCENT * totals.matches) {
        PlayerType::DeckLoyalist
    } else if comebacks_card(cards).is_some() {
        PlayerType::ComebackKid
    } else if distinct_formats(input) >= EXPLORER_FORMATS {
        PlayerType::FormatExplorer
    } else {
        PlayerType::AllRounder
    };
    let mut pills = pills_for(label, input, totals, cards);
    pills.truncate(PILLS_SHOWN);
    WrappedCard::PlayerType(PlayerTypeCard { label, pills })
}

/// Up to three short facts that back the label up.
fn pills_for(
    label: PlayerType,
    input: &WrappedInput,
    totals: &Totals,
    cards: &[WrappedCard],
) -> Vec<String> {
    let matches = count_label(totals.matches, "match", "matches");
    let deck_name = deck_of_year_card(cards).map(|d| d.name.clone());
    let mut pills = Vec::new();
    match label {
        PlayerType::Grinder => {
            pills.push(matches);
            if let Some(found) = trophies_card(cards).filter(|t| t.league_trophies > 0) {
                pills.push(count_label(found.league_trophies, "trophy", "trophies"));
            }
            pills.extend(deck_name);
        }
        PlayerType::LimitedLover => match limited_card(cards) {
            Some(found) => {
                pills.push(count_label(found.drafts, "draft", "drafts"));
                pills.extend(found.colour_pair.clone());
                pills.extend(found.best_record.clone());
            }
            None => pills.push(matches),
        },
        PlayerType::DeckLoyalist => {
            pills.push(matches);
            pills.extend(deck_name);
        }
        PlayerType::ComebackKid => {
            if let Some(found) = comebacks_card(cards) {
                pills.push(count_label(
                    found.wins_after_mulligan,
                    "comeback win",
                    "comeback wins",
                ));
                if let Some(turns) = found.quickest_win_turns {
                    pills.push(format!("Won on turn {turns}"));
                }
            }
            pills.push(matches);
        }
        PlayerType::FormatExplorer => {
            pills.push(matches);
            pills.push(format!("{} formats", distinct_formats(input)));
            pills.extend(favourite_format(cards).map(|label| label.to_string()));
        }
        PlayerType::AllRounder => {
            pills.push(matches);
            pills.extend(deck_name);
            pills.push(count_label(
                distinct_formats(input) as u32,
                "format",
                "formats",
            ));
        }
    }
    pills
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::build;
    use crate::profiles::{self, day_ms, plain_match};

    fn run(wins: u32, trophy: bool) -> InputRun {
        InputRun {
            deck_id: None,
            limited: false,
            wins,
            losses: 2,
            trophy,
        }
    }

    fn finish(top_cut: bool, text: &str, at: i64) -> InputFinish {
        InputFinish {
            top_cut,
            text: text.to_string(),
            finished_at_ms: at,
        }
    }

    fn draft(pair: Option<&str>, wins: u32, losses: u32) -> InputDraft {
        InputDraft {
            colour_pair: pair.map(str::to_string),
            wins,
            losses,
        }
    }

    #[test]
    fn trophies_show_for_a_trophy_a_top_cut_or_a_four_win_run() {
        let mut input = profiles::empty(chrono_tz::Europe::London);
        assert_eq!(trophies(&input), None);
        input.runs.push(run(3, false));
        input
            .finishes
            .push(finish(false, "Round 5, Modern Challenge, 2 Mar", 5));
        assert_eq!(
            trophies(&input),
            None,
            "a 3-2 run and a missed cut are not a cabinet"
        );

        let mut strong = input.clone();
        strong.runs.push(run(4, false));
        assert!(trophies(&strong).is_some());
        let mut trophy = input.clone();
        trophy.runs.push(run(5, true));
        let Some(WrappedCard::Trophies(card)) = trophies(&trophy) else {
            panic!("a card");
        };
        assert_eq!(
            (card.league_trophies, card.league_runs, card.top8s),
            (1, 2, 0)
        );
        assert_eq!(card.best_finish, None);
        let mut cut = input.clone();
        cut.finishes
            .push(finish(true, "Top 8, Modern Challenge, 14 Sep", 10));
        assert!(trophies(&cut).is_some());
    }

    #[test]
    fn the_best_finish_is_the_latest_top_cut() {
        let mut input = profiles::empty(chrono_tz::Europe::London);
        input
            .finishes
            .push(finish(true, "Top 8, Pauper Challenge, 3 Feb", 100));
        input
            .finishes
            .push(finish(true, "Top 4, Modern Challenge, 14 Sep", 300));
        input
            .finishes
            .push(finish(false, "Round 6, Legacy Challenge, 1 Nov", 500));
        let Some(WrappedCard::Trophies(card)) = trophies(&input) else {
            panic!("a card");
        };
        assert_eq!(card.top8s, 2);
        assert_eq!(
            card.best_finish.as_deref(),
            Some("Top 4, Modern Challenge, 14 Sep")
        );
    }

    #[test]
    fn limited_needs_three_drafts() {
        let mut input = profiles::empty(chrono_tz::Europe::London);
        input.drafts.push(draft(Some("Izzet"), 3, 0));
        input.drafts.push(draft(Some("Izzet"), 1, 2));
        assert_eq!(limited(&input), None);
        input.drafts.push(draft(Some("Golgari"), 3, 1));
        input.drafts.push(draft(None, 0, 3));
        let Some(WrappedCard::Limited(card)) = limited(&input) else {
            panic!("a card");
        };
        assert_eq!(card.drafts, 4);
        assert_eq!(card.colour_pair.as_deref(), Some("Izzet"));
        assert_eq!(
            card.best_record.as_deref(),
            Some("3-0"),
            "3-0 beats 3-1 on fewer losses"
        );
    }

    #[test]
    fn a_winless_best_record_is_not_shown() {
        let mut input = profiles::empty(chrono_tz::Europe::London);
        for _ in 0..3 {
            input.drafts.push(draft(None, 0, 3));
        }
        let Some(WrappedCard::Limited(card)) = limited(&input) else {
            panic!("a card");
        };
        assert_eq!((card.colour_pair, card.best_record), (None, None));
    }

    #[test]
    fn numbers_get_thousands_separators() {
        assert_eq!(with_commas(7), "7");
        assert_eq!(with_commas(1284), "1,284");
        assert_eq!(with_commas(1_000_000), "1,000,000");
    }

    fn label_of(bundle: &WrappedBundle) -> PlayerTypeCard {
        match bundle.cards.iter().rev().nth(1) {
            Some(WrappedCard::PlayerType(card)) => card.clone(),
            other => panic!("a player type, got {other:?}"),
        }
    }

    fn many_matches(n: u32, format: impl Fn(u32) -> &'static str) -> WrappedInput {
        let tz = chrono_tz::Europe::London;
        let mut input = profiles::empty(tz);
        for i in 0..n {
            let label = format(i);
            input.matches.push(plain_match(
                day_ms(tz, 1 + i % 300, 20),
                label,
                None,
                MatchResult::Win,
            ));
        }
        input
    }

    #[test]
    fn the_rule_table_picks_the_first_match() {
        // Grinder beats Limited Lover: 500 limited matches.
        let bundle = build(&many_matches(500, |_| "Limited")).expect("bundle");
        assert_eq!(label_of(&bundle).label, PlayerType::Grinder);
        // Limited Lover at exactly half.
        let bundle = build(&many_matches(40, |i| {
            if i % 2 == 0 { "Limited" } else { "Modern" }
        }))
        .expect("bundle");
        assert_eq!(label_of(&bundle).label, PlayerType::LimitedLover);
        // All-Rounder: 3 formats, no deck, no comebacks.
        let bundle = build(&many_matches(33, |i| {
            ["Modern", "Pauper", "Legacy"][(i % 3) as usize]
        }))
        .expect("bundle");
        let card = label_of(&bundle);
        assert_eq!(card.label, PlayerType::AllRounder);
        assert_eq!(card.pills, ["33 matches", "3 formats"]);
        // Format Explorer: 4 formats.
        let bundle = build(&many_matches(40, |i| {
            ["Modern", "Pauper", "Legacy", "Vintage"][(i % 4) as usize]
        }))
        .expect("bundle");
        let card = label_of(&bundle);
        assert_eq!(card.label, PlayerType::FormatExplorer);
        assert_eq!(card.pills[..2], ["40 matches", "4 formats"]);
    }

    #[test]
    fn deck_loyalty_needs_sixty_percent() {
        let tz = chrono_tz::Europe::London;
        let mut input = profiles::empty(tz);
        input
            .decks
            .push(profiles::deck(1, "Mono Red Burn", "Modern", &["R"], &[]));
        input
            .decks
            .push(profiles::deck(2, "Amulet Titan", "Modern", &["G"], &[]));
        // 18 of 30 is exactly 60%.
        for i in 0..30 {
            let deck = if i < 18 { 1 } else { 2 };
            input.matches.push(plain_match(
                day_ms(tz, 1 + i, 20),
                "Modern",
                Some(deck),
                MatchResult::Win,
            ));
        }
        let card = label_of(&build(&input).expect("bundle"));
        assert_eq!(card.label, PlayerType::DeckLoyalist);
        assert_eq!(card.pills, ["30 matches", "Mono Red Burn"]);
        input.matches[0].deck_id = Some(2);
        let card = label_of(&build(&input).expect("bundle"));
        assert_eq!(card.label, PlayerType::AllRounder, "17 of 30 is under 60%");
    }

    #[test]
    fn comebacks_make_a_comeback_kid() {
        let mut input = many_matches(30, |_| "Modern");
        let game = InputGame {
            won: Some(true),
            on_play: Some(true),
            mulligans: 1,
            turns: Some(4),
        };
        input.matches[0].games = vec![game; 5];
        let card = label_of(&build(&input).expect("bundle"));
        assert_eq!(card.label, PlayerType::ComebackKid);
        assert_eq!(
            card.pills,
            ["5 comeback wins", "Won on turn 4", "30 matches"]
        );
    }
}

#[cfg(test)]
mod snapshots {
    use crate::build::build;
    use crate::profiles;

    /// The bundle as JSON, with the 334-entry activity grid folded to a
    /// summary so a snapshot stays readable (the grid has its own tests).
    fn readable(input: &crate::input::WrappedInput) -> serde_json::Value {
        let bundle = build(input).expect("a bundle");
        let mut value = serde_json::to_value(bundle).expect("serialise");
        for card in value["cards"].as_array_mut().expect("cards") {
            if card["kind"] == "yearInNumbers" {
                let days: Vec<u64> = card["byDay"]
                    .as_array()
                    .expect("byDay")
                    .iter()
                    .map(|n| n.as_u64().expect("count"))
                    .collect();
                let busiest = days.iter().max().copied().unwrap_or(0);
                card["byDay"] = serde_json::json!(format!(
                    "{} days, {} with matches, {} matches, busiest day {}",
                    days.len(),
                    days.iter().filter(|n| **n > 0).count(),
                    days.iter().sum::<u64>(),
                    busiest
                ));
            }
        }
        value
    }

    /// Every `winRate` anywhere in a bundle is 50 or more, and the rival card
    /// has none.
    fn assert_never_under_fifty(value: &serde_json::Value) {
        match value {
            serde_json::Value::Object(map) => {
                if map.get("kind").and_then(|k| k.as_str()) == Some("toughestRival") {
                    assert!(!map.contains_key("winRate"));
                }
                for (key, inner) in map {
                    if key == "winRate" && inner.is_number() {
                        assert!(inner.as_u64().expect("a rate") >= 50, "{map:?}");
                    }
                    assert_never_under_fifty(inner);
                }
            }
            serde_json::Value::Array(items) => items.iter().for_each(assert_never_under_fifty),
            _ => {}
        }
    }

    #[test]
    fn no_profile_shows_a_win_rate_under_fifty() {
        for input in [profiles::light(), profiles::grinder(), profiles::limited()] {
            assert_never_under_fifty(&readable(&input));
        }
    }

    #[test]
    fn light_player() {
        insta::assert_json_snapshot!(readable(&profiles::light()));
    }

    #[test]
    fn grinder_player() {
        insta::assert_json_snapshot!(readable(&profiles::grinder()));
    }

    #[test]
    fn limited_player() {
        insta::assert_json_snapshot!(readable(&profiles::limited()));
    }
}
