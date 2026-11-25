//! The counting cards: hours played, the year in numbers, formats and when
//! the player plays. Every hour, weekday and day is local time in the
//! input's timezone, never UTC.

use std::collections::HashMap;

use chrono::{Datelike, NaiveDate, TimeZone, Timelike};
use chrono_tz::Tz;

use crate::bundle::*;
use crate::input::*;
use crate::when::{play_period, prefers_weekends};

const HOUR_MS: i64 = 3_600_000;
const MINUTE_MS: i64 = 60_000;

/// The fewest matches in the favourite format before the formats card shows.
const FORMATS_FLOOR: u32 = 10;
/// The fewest matches with start times before the when-you-play card shows.
const WHEN_FLOOR: u32 = 30;

/// A match's start as a local date and time, or `None` for an impossible
/// instant.
fn local_start(tz: Tz, started_at_ms: i64) -> Option<chrono::DateTime<Tz>> {
    tz.timestamp_millis_opt(started_at_ms).single()
}

/// The median duration of the ended matches in one format, in milliseconds;
/// 30 minutes when none of them has ended. Used for a match without an end
/// time, so it still adds a believable stretch of play.
fn median_duration_ms(matches: &[InputMatch], format: &str) -> i64 {
    let mut durations: Vec<i64> = matches
        .iter()
        .filter(|m| m.format_label == format)
        .filter_map(|m| m.ended_at_ms.map(|end| end - m.started_at_ms))
        .filter(|duration| *duration > 0)
        .collect();
    if durations.is_empty() {
        return 30 * MINUTE_MS;
    }
    durations.sort_unstable();
    let middle = durations.len() / 2;
    if durations.len() % 2 == 1 {
        durations[middle]
    } else {
        (durations[middle - 1] + durations[middle]) / 2
    }
}

/// Whole hours of play: the union of every match's `[start, end)` interval, so
/// two matches that overlap (parallel queues) count once. A match with no end
/// adds its format's median duration. Rounded to the nearest hour.
pub fn hours_played(matches: &[InputMatch]) -> u32 {
    let mut medians: HashMap<String, i64> = HashMap::new();
    let mut intervals: Vec<(i64, i64)> = Vec::new();
    for m in matches {
        let end = match m.ended_at_ms {
            Some(end) if end > m.started_at_ms => end,
            Some(_) => m.started_at_ms,
            None => {
                let median = *medians
                    .entry(m.format_label.clone())
                    .or_insert_with(|| median_duration_ms(matches, &m.format_label));
                m.started_at_ms + median
            }
        };
        intervals.push((m.started_at_ms, end));
    }
    intervals.sort_unstable();

    let mut total_ms: i64 = 0;
    let mut current: Option<(i64, i64)> = None;
    for (start, end) in intervals {
        current = match current {
            Some((cur_start, cur_end)) if start <= cur_end => Some((cur_start, cur_end.max(end))),
            Some((cur_start, cur_end)) => {
                total_ms += cur_end - cur_start;
                Some((start, end))
            }
            None => Some((start, end)),
        };
    }
    if let Some((cur_start, cur_end)) = current {
        total_ms += cur_end - cur_start;
    }
    ((total_ms + HOUR_MS / 2) / HOUR_MS) as u32
}

/// The local date the window starts on and the number of local days in it
/// (1 January up to, not including, 1 December).
fn window_days(input: &WrappedInput) -> Option<(NaiveDate, usize)> {
    let start = local_start(input.tz, input.window.start_ms)?.date_naive();
    let end = local_start(input.tz, input.window.end_ms)?.date_naive();
    let days = (end - start).num_days();
    Some((start, days.max(0) as usize))
}

/// Matches started on each local day of the window, in order.
fn matches_by_day(input: &WrappedInput) -> Vec<u32> {
    let Some((first_day, days)) = window_days(input) else {
        return Vec::new();
    };
    let mut by_day = vec![0u32; days];
    for m in &input.matches {
        let Some(time) = local_start(input.tz, m.started_at_ms) else {
            continue;
        };
        let index = (time.date_naive() - first_day).num_days();
        if index >= 0 && (index as usize) < days {
            by_day[index as usize] += 1;
        }
    }
    by_day
}

/// Matches, games, hours, distinct local days played and the activity grid.
/// `top_percent` stays empty until the API's percentiles exist (phase 4).
pub fn year_in_numbers(input: &WrappedInput, totals: &Totals) -> WrappedCard {
    let by_day = matches_by_day(input);
    let days_played = by_day.iter().filter(|count| **count > 0).count() as u32;
    WrappedCard::YearInNumbers(YearInNumbersCard {
        matches: totals.matches,
        games: totals.games,
        hours: totals.hours,
        days_played,
        by_day,
        top_percent: None,
    })
}

/// A whole-number percentage, rounded to the nearest.
pub fn percent(part: u32, whole: u32) -> u32 {
    if whole == 0 {
        return 0;
    }
    ((u64::from(part) * 100 + u64::from(whole) / 2) / u64::from(whole)) as u32
}

/// Matches per format, most matches first (ties by label so the order is
/// stable).
fn format_counts(input: &WrappedInput) -> Vec<(String, u32)> {
    let mut counts: HashMap<String, u32> = HashMap::new();
    for m in &input.matches {
        *counts.entry(m.format_label.clone()).or_insert(0) += 1;
    }
    let mut list: Vec<(String, u32)> = counts.into_iter().collect();
    list.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    list
}

/// The art of the deck played most in a format; empty when no deck was used
/// there (limited matches have no deck).
fn favourite_format_art(input: &WrappedInput, label: &str) -> Art {
    let mut best: Option<(&InputDeck, u32)> = None;
    for deck in input.decks.iter().filter(|d| d.format_label == label) {
        let played = input
            .matches
            .iter()
            .filter(|m| m.deck_id == Some(deck.id))
            .count() as u32;
        let better = match best {
            Some((_, most)) => played > most,
            None => played > 0,
        };
        if better {
            best = Some((deck, played));
        }
    }
    best.map(|(deck, _)| deck.art.clone()).unwrap_or_default()
}

/// The format of choice: the most played format with its share of the year
/// and the art of its most played deck, the rest as honourable mentions. Needs
/// 10 matches in the favourite, so a scattered player gets no card.
pub fn formats(input: &WrappedInput) -> Option<WrappedCard> {
    let counts = format_counts(input);
    let (label, matches) = counts.first()?.clone();
    if matches < FORMATS_FLOOR {
        return None;
    }
    let total = input.matches.len() as u32;
    let mentions = counts
        .iter()
        .skip(1)
        .map(|(label, matches)| FormatShare {
            label: label.clone(),
            matches: *matches,
            percent: percent(*matches, total),
        })
        .collect();
    Some(WrappedCard::Formats(FormatsCard {
        favourite: FavouriteFormat {
            art: favourite_format_art(input, &label),
            percent: percent(matches, total),
            label,
            matches,
        },
        mentions,
    }))
}

/// The index of the largest count; the earliest wins a tie.
fn peak_index(counts: &[u32]) -> usize {
    let mut best = 0;
    for (index, count) in counts.iter().enumerate() {
        if *count > counts[best] {
            best = index;
        }
    }
    best
}

/// When the player plays, by local hour, weekday and month: the part of the
/// day, the peak hour and weekday, the busiest month and whether weekends beat
/// weekdays. Needs 30 matches with start times.
pub fn when_you_play(input: &WrappedInput) -> Option<WrappedCard> {
    let mut by_hour = vec![0u32; 24];
    let mut by_weekday = [0u32; 7];
    let mut by_month = [0u32; 12];
    let mut counted = 0u32;
    for m in &input.matches {
        let Some(time) = local_start(input.tz, m.started_at_ms) else {
            continue;
        };
        by_hour[time.hour() as usize] += 1;
        by_weekday[time.weekday().num_days_from_monday() as usize] += 1;
        by_month[time.month0() as usize] += 1;
        counted += 1;
    }
    if counted < WHEN_FLOOR {
        return None;
    }
    Some(WrappedCard::WhenYouPlay(WhenYouPlayCard {
        peak_weekday: peak_index(&by_weekday) as u8,
        peak_hour: peak_index(&by_hour) as u8,
        busiest_month: peak_index(&by_month) as u8 + 1,
        period: play_period(&by_hour),
        weekends: prefers_weekends(&by_weekday),
        by_hour,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profiles::{self, local_ms, plain_match};

    fn win(start: i64, end: Option<i64>, format: &str) -> InputMatch {
        let mut m = plain_match(start, format, None, MatchResult::Win);
        m.ended_at_ms = end;
        m
    }

    #[test]
    fn overlapping_matches_count_their_hours_once() {
        // 0:00 to 1:00 and 0:30 to 1:30 are one and a half hours together,
        // plus a separate half hour: 2 hours. Summed one by one it would be
        // 2.5 hours, which rounds to 3.
        let hour = HOUR_MS;
        let matches = vec![
            win(0, Some(hour), "Modern"),
            win(hour / 2, Some(hour + hour / 2), "Modern"),
            win(10 * hour, Some(10 * hour + hour / 2), "Modern"),
        ];
        assert_eq!(hours_played(&matches), 2);
        // Two identical matches are one hour, not two.
        let twins = vec![win(0, Some(hour), "Modern"), win(0, Some(hour), "Modern")];
        assert_eq!(hours_played(&twins), 1);
    }

    #[test]
    fn a_match_without_an_end_adds_its_formats_median() {
        let hour = HOUR_MS;
        let matches = vec![
            win(0, Some(hour), "Modern"),
            win(10 * hour, Some(13 * hour), "Modern"),
            // Median of 1h and 3h is 2h.
            win(20 * hour, None, "Modern"),
            // No ended match in this format: 30 minutes.
            win(30 * hour, None, "Pauper"),
        ];
        // 1 + 3 + 2 + 0.5 = 6.5 hours, rounds to 7.
        assert_eq!(hours_played(&matches), 7);
    }

    #[test]
    fn local_time_decides_hour_weekday_and_day() {
        // Sunday 7 June 2026, 23:30 in Los Angeles is already Monday in UTC.
        let la = chrono_tz::America::Los_Angeles;
        let mut input = profiles::empty(la);
        let late = local_ms(la, 2026, 6, 7, 23, 30);
        for _ in 0..30 {
            input
                .matches
                .push(plain_match(late, "Modern", None, MatchResult::Win));
        }
        let Some(WrappedCard::WhenYouPlay(card)) = when_you_play(&input) else {
            panic!("a card");
        };
        assert_eq!(card.peak_hour, 23);
        assert_eq!(card.peak_weekday, 6, "Sunday");
        assert_eq!(card.busiest_month, 6);
        assert_eq!(card.period, PlayPeriod::Night);

        // 00:30 Monday 8 June in Auckland is still Sunday in UTC.
        let nz = chrono_tz::Pacific::Auckland;
        let mut input = profiles::empty(nz);
        let early = local_ms(nz, 2026, 6, 8, 0, 30);
        for _ in 0..30 {
            input
                .matches
                .push(plain_match(early, "Modern", None, MatchResult::Win));
        }
        let Some(WrappedCard::WhenYouPlay(card)) = when_you_play(&input) else {
            panic!("a card");
        };
        assert_eq!(card.peak_hour, 0);
        assert_eq!(card.peak_weekday, 0, "Monday");

        let totals = Totals {
            matches: 30,
            games: 0,
            wins: 30,
            losses: 0,
            draws: 0,
            hours: 0,
        };
        let WrappedCard::YearInNumbers(numbers) = year_in_numbers(&input, &totals) else {
            panic!("numbers");
        };
        // 8 June is day 159 of 2026 (index 158).
        assert_eq!(numbers.by_day[158], 30);
        assert_eq!(numbers.days_played, 1);
    }

    #[test]
    fn by_day_has_one_entry_per_local_day_of_the_window() {
        let numbers = |input: &WrappedInput| {
            let totals = Totals {
                matches: 0,
                games: 0,
                wins: 0,
                losses: 0,
                draws: 0,
                hours: 0,
            };
            match year_in_numbers(input, &totals) {
                WrappedCard::YearInNumbers(card) => card,
                _ => panic!("numbers"),
            }
        };
        // 1 January to 30 November 2026 is 334 days, in any timezone.
        for tz in [
            chrono_tz::Europe::London,
            chrono_tz::Pacific::Auckland,
            chrono_tz::America::Los_Angeles,
        ] {
            assert_eq!(numbers(&profiles::empty(tz)).by_day.len(), 334);
        }
        let light = profiles::light();
        let card = numbers(&light);
        assert_eq!(card.by_day.iter().sum::<u32>(), 30);
        assert_eq!(card.days_played, 30);
    }

    #[test]
    fn the_favourite_format_needs_ten_matches() {
        let tz = chrono_tz::Europe::London;
        let mut input = profiles::empty(tz);
        for n in 0..9 {
            input.matches.push(plain_match(
                profiles::day_ms(tz, 10 + n, 20),
                "Modern",
                Some(1),
                MatchResult::Win,
            ));
        }
        assert_eq!(formats(&input), None);
        input.matches.push(plain_match(
            profiles::day_ms(tz, 30, 20),
            "Modern",
            Some(1),
            MatchResult::Win,
        ));
        assert!(formats(&input).is_some());
    }

    #[test]
    fn formats_have_a_favourite_with_deck_art_and_ordered_mentions() {
        let tz = chrono_tz::Europe::London;
        let mut input = profiles::empty(tz);
        input
            .decks
            .push(profiles::deck(1, "Mono Red Burn", "Modern", &["R"], &[]));
        input
            .decks
            .push(profiles::deck(2, "Amulet Titan", "Modern", &["G"], &[]));
        let mut day = 1;
        let mut add = |input: &mut WrappedInput, n: u32, format: &str, deck: Option<i64>| {
            for _ in 0..n {
                day += 1;
                input.matches.push(plain_match(
                    profiles::day_ms(tz, day, 20),
                    format,
                    deck,
                    MatchResult::Win,
                ));
            }
        };
        add(&mut input, 6, "Modern", Some(1));
        add(&mut input, 10, "Modern", Some(2));
        add(&mut input, 3, "Pauper", None);
        add(&mut input, 5, "Limited", None);
        let Some(WrappedCard::Formats(card)) = formats(&input) else {
            panic!("a card");
        };
        assert_eq!(card.favourite.label, "Modern");
        assert_eq!(card.favourite.matches, 16);
        assert_eq!(card.favourite.percent, 67);
        assert_eq!(card.favourite.art, input.decks[1].art);
        let mentions: Vec<(&str, u32)> = card
            .mentions
            .iter()
            .map(|s| (s.label.as_str(), s.matches))
            .collect();
        assert_eq!(mentions, [("Limited", 5), ("Pauper", 3)]);
    }

    #[test]
    fn when_you_play_reads_the_light_profile() {
        let Some(WrappedCard::WhenYouPlay(card)) = when_you_play(&profiles::light()) else {
            panic!("a card");
        };
        // Every match starts at 20:00 London.
        assert_eq!(card.peak_hour, 20);
        assert_eq!(card.period, PlayPeriod::Evening);
        assert_eq!(card.by_hour.len(), 24);
        assert_eq!(card.by_hour.iter().sum::<u32>(), 30);
    }
}
