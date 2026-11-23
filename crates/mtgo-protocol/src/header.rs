//! The header line every mtgo.log event starts with, for example
//! `12:25:49 [INF] (Game Management|Match State Changed for ...)`.

use std::sync::LazyLock;

use chrono::NaiveTime;
use regex::Regex;

static HEADER_LINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\d{2}:\d{2}:\d{2} \[(INF|ERR|DBG|WRN|TRC)\]").expect("valid regex")
});

static HEADER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(\d{2}):(\d{2}):(\d{2}) \[(\w+)\] \(([^|]+)\|([^)]*)\)").expect("valid regex")
});

/// The parsed first line of an event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    /// Time of day only. mtgo.log lines carry no date; see `time::DayClock`.
    pub time: NaiveTime,
    pub level: String,
    pub category: String,
    pub context: String,
}

/// True when `line` starts a new event.
pub fn is_header_line(line: &str) -> bool {
    HEADER_LINE.is_match(line)
}

/// The time of day of a header line, or `None` for any other line.
pub fn header_time(line: &str) -> Option<NaiveTime> {
    if !is_header_line(line) {
        return None;
    }
    NaiveTime::parse_from_str(line.get(..8)?, "%H:%M:%S").ok()
}

/// Parse the header at the start of an event's text.
pub fn parse_header(event_text: &str) -> Option<Header> {
    let caps = HEADER.captures(event_text)?;
    let hour: u32 = caps[1].parse().ok()?;
    let minute: u32 = caps[2].parse().ok()?;
    let second: u32 = caps[3].parse().ok()?;
    Some(Header {
        time: NaiveTime::from_hms_opt(hour, minute, second)?,
        level: caps[4].to_string(),
        category: caps[5].to_string(),
        context: caps[6].to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveTime;

    #[test]
    fn reads_the_time_of_a_header_line_only() {
        assert_eq!(
            header_time("16:17:47 [INF] (UI|x)"),
            NaiveTime::from_hms_opt(16, 17, 47)
        );
        assert_eq!(header_time("Match Token:f5e33a1f"), None);
        assert_eq!(header_time("99:99:99 [INF] (UI|x)"), None);
    }

    #[test]
    fn recognises_header_lines() {
        assert!(is_header_line("12:25:49 [INF] (Game Management|Something)"));
        assert!(is_header_line("01:02:03 [ERR] (UI|x)"));
        assert!(!is_header_line("Match Token:f5e33a1f"));
        assert!(!is_header_line("12:25:49 [XYZ] (UI|x)"));
        assert!(!is_header_line(""));
    }

    #[test]
    fn parses_time_level_category_and_context() {
        let header = parse_header(
            "12:23:54 [INF] (Game Management|Match State Changed for abc from A to B) tail",
        )
        .expect("header");
        assert_eq!(
            header.time,
            NaiveTime::from_hms_opt(12, 23, 54).expect("time")
        );
        assert_eq!(header.level, "INF");
        assert_eq!(header.category, "Game Management");
        assert_eq!(header.context, "Match State Changed for abc from A to B");
    }

    #[test]
    fn rejects_text_without_a_header() {
        assert_eq!(parse_header("Event Id:285746550"), None);
        assert_eq!(parse_header("25:61:00 [INF] (UI|x)"), None);
    }
}
