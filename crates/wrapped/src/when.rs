//! When the player plays: which part of the day and whether weekends win.
//! The builder (phase 3) passes in local match start counts.

use crate::bundle::PlayPeriod;

/// The part of the day with the most matches started: morning (5am to
/// noon), afternoon (noon to 5pm), evening (5pm to 10pm) or night (10pm to
/// 5am). `by_hour` is matches started in each local hour, 0 to 23; missing
/// hours count as none. A tie goes to the later part of the day, so a
/// player split evenly between evening and night is a night owl.
pub fn play_period(by_hour: &[u32]) -> PlayPeriod {
    let mut morning = 0;
    let mut afternoon = 0;
    let mut evening = 0;
    let mut night = 0;
    for (hour, count) in by_hour.iter().enumerate() {
        match hour {
            5..=11 => morning += count,
            12..=16 => afternoon += count,
            17..=21 => evening += count,
            _ => night += count,
        }
    }

    // Walk the day in order; `>=` lets a later part win a tie.
    let mut best = PlayPeriod::Morning;
    let mut best_count = morning;
    for (period, count) in [
        (PlayPeriod::Afternoon, afternoon),
        (PlayPeriod::Evening, evening),
        (PlayPeriod::Night, night),
    ] {
        if count >= best_count {
            best = period;
            best_count = count;
        }
    }
    best
}

/// True when an average Saturday or Sunday had more matches than an average
/// weekday. `by_weekday` is matches started on each local weekday, 0 is
/// Monday. Comparing averages stops five weekdays winning on count alone.
pub fn prefers_weekends(by_weekday: &[u32; 7]) -> bool {
    let weekdays: u64 = by_weekday[..5].iter().map(|count| u64::from(*count)).sum();
    let weekend: u64 = by_weekday[5..].iter().map(|count| u64::from(*count)).sum();
    // weekend / 2 > weekdays / 5, without dividing.
    weekend * 5 > weekdays * 2
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hours(pairs: &[(usize, u32)]) -> Vec<u32> {
        let mut by_hour = vec![0; 24];
        for (hour, count) in pairs {
            by_hour[*hour] = *count;
        }
        by_hour
    }

    #[test]
    fn each_part_of_the_day_has_its_hours() {
        assert_eq!(play_period(&hours(&[(5, 3)])), PlayPeriod::Morning);
        assert_eq!(play_period(&hours(&[(11, 3)])), PlayPeriod::Morning);
        assert_eq!(play_period(&hours(&[(12, 3)])), PlayPeriod::Afternoon);
        assert_eq!(play_period(&hours(&[(16, 3)])), PlayPeriod::Afternoon);
        assert_eq!(play_period(&hours(&[(17, 3)])), PlayPeriod::Evening);
        assert_eq!(play_period(&hours(&[(21, 3)])), PlayPeriod::Evening);
        assert_eq!(play_period(&hours(&[(22, 3)])), PlayPeriod::Night);
        assert_eq!(play_period(&hours(&[(0, 3)])), PlayPeriod::Night);
        assert_eq!(play_period(&hours(&[(4, 3)])), PlayPeriod::Night);
    }

    #[test]
    fn the_part_with_most_matches_wins_not_the_peak_hour() {
        // The single busiest hour is 9pm, but the night hours add up to more.
        let by_hour = hours(&[(21, 10), (22, 6), (23, 6), (0, 4)]);
        assert_eq!(play_period(&by_hour), PlayPeriod::Night);
    }

    #[test]
    fn a_tie_goes_to_the_later_part_of_the_day() {
        assert_eq!(play_period(&hours(&[(20, 5), (23, 5)])), PlayPeriod::Night);
        assert_eq!(
            play_period(&hours(&[(8, 5), (14, 5)])),
            PlayPeriod::Afternoon
        );
    }

    #[test]
    fn short_or_empty_hours_do_not_panic() {
        assert_eq!(play_period(&[]), PlayPeriod::Night);
        assert_eq!(play_period(&[0, 0, 0, 0, 0, 0, 9]), PlayPeriod::Morning);
    }

    #[test]
    fn weekends_win_on_average_not_on_count() {
        // 50 weekday matches (10 a day) against 30 weekend ones (15 a day).
        assert!(prefers_weekends(&[10, 10, 10, 10, 10, 15, 15]));
        // 60 weekday matches (12 a day) against 20 weekend ones (10 a day).
        assert!(!prefers_weekends(&[12, 12, 12, 12, 12, 10, 10]));
    }

    #[test]
    fn an_even_split_is_a_weekday_regular() {
        assert!(!prefers_weekends(&[4, 4, 4, 4, 4, 4, 4]));
        assert!(!prefers_weekends(&[0; 7]));
    }
}
