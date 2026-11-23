//! Small helpers for a game's tags, shared by every writer of records (the
//! Arena and Endstep builders).

use crate::model::{GameResult, Tag};

/// A tag from a name and a value.
pub fn tag(name: &str, value: &str) -> Tag {
    Tag {
        name: name.to_string(),
        value: value.to_string(),
    }
}

/// `?` for an empty value: a required tag the source has no value for.
pub fn or_unknown(value: &str) -> &str {
    if value.is_empty() { "?" } else { value }
}

/// The result as the `Result` tag and the result marker write it.
pub fn result_text(result: GameResult) -> &'static str {
    match result {
        GameResult::Player1 => "1-0",
        GameResult::Player2 => "0-1",
        GameResult::Draw => "1/2-1/2",
        GameResult::Unknown => "*",
    }
}

/// `YYYY.MM.DD`, the UTC date of a unix ms time.
pub fn utc_date(unix_ms: i64) -> String {
    // Days since 1970-01-01 to a civil date (Howard Hinnant's algorithm).
    let days = unix_ms.div_euclid(86_400_000) + 719_468;
    let era = days.div_euclid(146_097);
    let day_of_era = days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!("{year:04}.{month:02}.{day:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utc_date_of_a_known_day() {
        // 2026-11-10 11:46:37 UTC.
        assert_eq!(utc_date(1_794_311_197_000), "2026.11.10");
        assert_eq!(utc_date(0), "1970.01.01");
        // The last millisecond of a leap day, and the first of the next day.
        assert_eq!(utc_date(1_709_251_199_999), "2024.02.29");
        assert_eq!(utc_date(1_709_251_200_000), "2024.03.01");
        assert_eq!(utc_date(-1), "1969.12.31");
    }

    #[test]
    fn or_unknown_of_empty() {
        assert_eq!(or_unknown(""), "?");
        assert_eq!(or_unknown("Modern"), "Modern");
    }

    #[test]
    fn a_tag_and_a_result() {
        let made = tag("Site", "Endstep");
        assert_eq!(
            (made.name.as_str(), made.value.as_str()),
            ("Site", "Endstep")
        );
        assert_eq!(result_text(GameResult::Player1), "1-0");
        assert_eq!(result_text(GameResult::Player2), "0-1");
        assert_eq!(result_text(GameResult::Draw), "1/2-1/2");
        assert_eq!(result_text(GameResult::Unknown), "*");
    }
}
