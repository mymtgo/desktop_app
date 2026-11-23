//! Header times, the header date order, and .NET ticks.
//!
//! Header lines carry the machine's local time in its locale's date order
//! (`03/11/2026 12:33:02` is DD/MM on the sample machine), with no zone and
//! second precision. GRE and client messages carry better times; header times
//! are the fallback, with the date order detected from the file and the
//! machine offset passed in. Until the order is known, an ambiguous date
//! (both fields 12 or under) takes the reading nearest a reference time.

use chrono::NaiveDate;

const UNITY_PREFIX: &str = "[UnityCrossThreadLogger]";

/// .NET ticks (100 ns since 0001-01-01) at the Unix epoch.
const TICKS_AT_UNIX_EPOCH: i64 = 621_355_968_000_000_000;
const TICKS_PER_MS: i64 = 10_000;

/// How far a header time may sit from the GRE time beside it under the right
/// date order: the largest UTC offset (14 h) plus slack for slow writes.
const DETECT_TOLERANCE_MS: i64 = 26 * 3_600_000;

/// How to read header times for one log file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HeaderClock {
    /// `Some(true)` for DD/MM dates, `Some(false)` for MM/DD, `None` when not
    /// known (then only a date with a field above 12 can be read).
    pub day_first: Option<bool>,
    /// Minutes the machine is ahead of UTC.
    pub utc_offset_minutes: i32,
}

/// A header date and time as written, before the date order is applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Stamp {
    /// First and second date fields: day and month in some order, or month
    /// and day after a leading year.
    first: u32,
    second: u32,
    year: i32,
    year_first: bool,
    hour: u32,
    minute: u32,
    second_of_minute: u32,
}

/// UTC unix ms of a header line's time, or `None` when `head` does not start
/// with a header date or the date is not a real one. A response head (date
/// line, `\n`, `<==` line) works too.
///
/// When the date order is unknown and both date fields are 12 or under, the
/// reading nearest `reference_ms` wins (the file's last event time, else its
/// last write, else now). The two readings are a month or more apart unless
/// day equals month, when they are the same instant, so a reference within a
/// couple of weeks of the line picks the right one.
pub fn header_time(clock: &HeaderClock, head: &str, reference_ms: i64) -> Option<i64> {
    let (stamp, _) = parse_stamp(head.strip_prefix(UNITY_PREFIX)?)?;
    // API responses are stamped in UTC, every other header in machine time.
    let offset_minutes = if head.contains("<== ") {
        0
    } else {
        clock.utc_offset_minutes
    };
    let offset_ms = i64::from(offset_minutes) * 60_000;
    let day_first = if stamp.year_first {
        false
    } else {
        match clock.day_first.or_else(|| order_from_fields(&stamp)) {
            Some(day_first) => day_first,
            None => order_nearest(&stamp, reference_ms + offset_ms)?,
        }
    };
    let local_ms = local_ms(&stamp, day_first)?;
    Some(local_ms - offset_ms)
}

/// The date order whose reading of `stamp` is nearest `reference_local_ms`
/// (in the machine's local time), when at least one reading is a real date.
fn order_nearest(stamp: &Stamp, reference_local_ms: i64) -> Option<bool> {
    let distance =
        |day_first: bool| local_ms(stamp, day_first).map(|ms| (ms - reference_local_ms).abs());
    match (distance(true), distance(false)) {
        (Some(day), Some(month)) => Some(day <= month),
        (Some(_), None) => Some(true),
        (None, Some(_)) => Some(false),
        (None, None) => None,
    }
}

/// Whether the file's header dates are day first, from the first header that
/// settles it: a first field above 12 means day first, a second field above
/// 12 means month first. When every date is ambiguous (03/11), a header is
/// compared with the GRE or client time on the line after it, whichever order
/// lands within a day of it wins. `None` when nothing settles it.
pub fn detect_day_first(bytes: &[u8]) -> Option<bool> {
    let mut last_stamp: Option<Stamp> = None;
    for line in bytes.split(|byte| *byte == b'\n') {
        if line.starts_with(UNITY_PREFIX.as_bytes()) {
            last_stamp = None;
            let text = String::from_utf8_lossy(line);
            let Some((stamp, _)) = parse_stamp(&text[UNITY_PREFIX.len()..]) else {
                continue;
            };
            if stamp.year_first {
                continue;
            }
            if let Some(day_first) = order_from_fields(&stamp) {
                return Some(day_first);
            }
            last_stamp = Some(stamp);
        } else if line.starts_with(b"{")
            && let Some(stamp) = last_stamp.take()
            && let Some(message_ms) = message_timestamp(line)
            && let Some(day_first) = order_from_message_time(&stamp, message_ms)
        {
            return Some(day_first);
        }
    }
    None
}

/// Unix ms of .NET ticks (UTC).
pub fn dotnet_ticks_to_ms(ticks: i64) -> i64 {
    (ticks - TICKS_AT_UNIX_EPOCH).div_euclid(TICKS_PER_MS)
}

/// Unix ms from a message `timestamp`: GRE messages write unix ms, client
/// messages and the authenticate response write .NET ticks.
pub(crate) fn timestamp_to_ms(value: i64) -> i64 {
    // Unix ms stay below 1e14 until the year 5138; ticks are above 6e17.
    if value >= 100_000_000_000_000 {
        dotnet_ticks_to_ms(value)
    } else {
        value
    }
}

/// Whether `line` is a `[UnityCrossThreadLogger]` line holding only a date
/// and time (the line before every API response).
pub(crate) fn is_date_only_header(line: &str) -> bool {
    match line.strip_prefix(UNITY_PREFIX).and_then(parse_stamp) {
        Some((_, rest)) => rest.trim().is_empty(),
        None => false,
    }
}

/// The date order a field above 12 proves, if any.
fn order_from_fields(stamp: &Stamp) -> Option<bool> {
    if stamp.first > 12 && stamp.second <= 12 {
        Some(true)
    } else if stamp.second > 12 && stamp.first <= 12 {
        Some(false)
    } else {
        None
    }
}

/// The date order under which `stamp` lands near `message_ms`, when only one
/// order does.
fn order_from_message_time(stamp: &Stamp, message_ms: i64) -> Option<bool> {
    let near = |day_first: bool| match local_ms(stamp, day_first) {
        Some(ms) => (ms - message_ms).abs() <= DETECT_TOLERANCE_MS,
        None => false,
    };
    match (near(true), near(false)) {
        (true, false) => Some(true),
        (false, true) => Some(false),
        _ => None,
    }
}

/// The `"timestamp": "..."` value of a one-line JSON message, in unix ms.
fn message_timestamp(line: &[u8]) -> Option<i64> {
    let text = String::from_utf8_lossy(line);
    let key = "\"timestamp\"";
    let after_key = &text[text.find(key)? + key.len()..];
    let after_colon = after_key.trim_start().strip_prefix(':')?.trim_start();
    let digits_start = after_colon.strip_prefix('"').unwrap_or(after_colon);
    let digits: String = digits_start
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    let value: i64 = digits.parse().ok()?;
    Some(timestamp_to_ms(value))
}

/// The stamp's local time as unix ms (as if it were UTC).
fn local_ms(stamp: &Stamp, day_first: bool) -> Option<i64> {
    let (day, month) = if stamp.year_first || !day_first {
        (stamp.second, stamp.first)
    } else {
        (stamp.first, stamp.second)
    };
    let date = NaiveDate::from_ymd_opt(stamp.year, month, day)?;
    let time = date.and_hms_opt(stamp.hour, stamp.minute, stamp.second_of_minute)?;
    Some(time.and_utc().timestamp_millis())
}

/// Reads `a/b/yyyy h:mm:ss`, `yyyy-mm-dd h:mm:ss` (`/`, `.` or `-` between
/// date fields), with an optional ` AM` or ` PM`; returns the stamp and the
/// text after it.
fn parse_stamp(text: &str) -> Option<(Stamp, &str)> {
    let (a, rest) = take_number(text)?;
    let rest = take_date_separator(rest)?;
    let (b, rest) = take_number(rest)?;
    let rest = take_date_separator(rest)?;
    let (c, rest) = take_number(rest)?;
    let rest = rest.strip_prefix(' ')?;
    let (hour, rest) = take_number(rest)?;
    let (minute, rest) = take_number(rest.strip_prefix(':')?)?;
    let (second_of_minute, mut rest) = take_number(rest.strip_prefix(':')?)?;
    let mut hour = hour;
    if let Some(after) = rest.strip_prefix(" AM") {
        if hour == 12 {
            hour = 0;
        }
        rest = after;
    } else if let Some(after) = rest.strip_prefix(" PM") {
        if hour < 12 {
            hour += 12;
        }
        rest = after;
    }
    let year_first = a > 31;
    let (first, second, year) = if year_first { (b, c, a) } else { (a, b, c) };
    let stamp = Stamp {
        first,
        second,
        year: i32::try_from(year).ok()?,
        year_first,
        hour,
        minute,
        second_of_minute,
    };
    Some((stamp, rest))
}

/// One to four ASCII digits at the start of `text`.
fn take_number(text: &str) -> Option<(u32, &str)> {
    let length = text.bytes().take(4).take_while(u8::is_ascii_digit).count();
    if length == 0 {
        return None;
    }
    let value = text[..length].parse().ok()?;
    Some((value, &text[length..]))
}

fn take_date_separator(text: &str) -> Option<&str> {
    text.strip_prefix(['/', '.', '-'])
}

/// UTC unix ms of a season boundary written `YYYY-MM-DDTHH:MM:SS` with no
/// zone (read as UTC), or `None` when the text is not in that shape.
pub fn season_time_ms(text: &str) -> Option<i64> {
    let time = chrono::NaiveDateTime::parse_from_str(text.trim(), "%Y-%m-%dT%H:%M:%S").ok()?;
    Some(time.and_utc().timestamp_millis())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn season_time_is_utc() {
        assert_eq!(
            season_time_ms("2026-10-31T19:05:00"),
            Some(1_793_473_500_000)
        );
        assert_eq!(
            season_time_ms("2026-11-30T20:00:00"),
            Some(1_796_068_800_000)
        );
    }

    #[test]
    fn season_time_rejects_junk() {
        assert_eq!(season_time_ms(""), None);
        assert_eq!(season_time_ms("tomorrow"), None);
        assert_eq!(season_time_ms("2026-13-40T99:00:00"), None);
        assert_eq!(season_time_ms("2026-10-31"), None);
    }

    const AT_12_33_02: i64 = 1_793_709_182_000; // 2026-11-03T12:33:02Z

    /// `header_time` with the fixture's own time as the reference.
    fn header_time_at(clock: &HeaderClock, head: &str) -> Option<i64> {
        header_time(clock, head, AT_12_33_02)
    }

    fn clock(day_first: Option<bool>, utc_offset_minutes: i32) -> HeaderClock {
        HeaderClock {
            day_first,
            utc_offset_minutes,
        }
    }

    #[test]
    fn header_time_reads_day_first() {
        let head = "[UnityCrossThreadLogger]03/11/2026 12:33:02: Match to P: GreToClientEvent";
        assert_eq!(
            header_time_at(&clock(Some(true), 0), head),
            Some(AT_12_33_02)
        );
    }

    #[test]
    fn header_time_applies_the_offset() {
        let head = "[UnityCrossThreadLogger]03/11/2026 12:33:02: Match to P: GreToClientEvent";
        assert_eq!(
            header_time_at(&clock(Some(true), -300), head),
            Some(AT_12_33_02 + 5 * 3_600_000)
        );
    }

    #[test]
    fn header_time_reads_month_first_with_a_twelve_hour_clock() {
        let head = "[UnityCrossThreadLogger]11/3/2026 12:33:02 PM: Match to P";
        assert_eq!(
            header_time_at(&clock(Some(false), 0), head),
            Some(AT_12_33_02)
        );
        let morning = "[UnityCrossThreadLogger]11/3/2026 12:33:02 AM";
        assert_eq!(
            header_time_at(&clock(Some(false), 0), morning),
            Some(AT_12_33_02 - 12 * 3_600_000)
        );
    }

    #[test]
    fn response_headers_are_utc() {
        let clock = clock(Some(false), -300);
        let response = "[UnityCrossThreadLogger]11/7/2026 3:58:45 PM\n<== EventGetCoursesV2(x)";
        assert_eq!(
            header_time(&clock, response, 0),
            Some(1_794_067_125_000) // 2026-11-07T15:58:45Z
        );
        let local =
            "[UnityCrossThreadLogger]11/7/2026 10:59:01 AM: Match to P: AuthenticateResponse";
        assert_eq!(
            header_time(&clock, local, 0),
            Some(1_794_067_141_000) // 2026-11-07T15:59:01Z
        );
    }

    #[test]
    fn header_time_reads_a_response_head() {
        let head = "[UnityCrossThreadLogger]03/11/2026 12:33:02\n<== RankGetCombinedRankInfo(x)";
        assert_eq!(
            header_time_at(&clock(Some(true), 0), head),
            Some(AT_12_33_02)
        );
    }

    #[test]
    fn header_time_picks_the_order_nearest_the_reference() {
        let head = "[UnityCrossThreadLogger]03/11/2026 12:33:02";
        // 2026-03-11T12:33:02Z, the month first reading.
        let march = 1_773_232_382_000;
        let hour = 3_600_000;
        let unknown = clock(None, 0);
        assert_eq!(
            header_time(&unknown, head, AT_12_33_02 + 30 * 24 * hour),
            Some(AT_12_33_02),
            "a reference a month later still picks 3 November"
        );
        assert_eq!(header_time(&unknown, head, march + hour), Some(march));
        assert_eq!(
            header_time(&clock(None, -300), head, AT_12_33_02 + 5 * hour),
            Some(AT_12_33_02 + 5 * hour),
            "the offset applies to the reading picked"
        );
        // A known order or a field above 12 ignores the reference.
        assert_eq!(
            header_time(&clock(Some(true), 0), head, march),
            Some(AT_12_33_02)
        );
        let decided = "[UnityCrossThreadLogger]13/11/2026 12:33:02";
        assert_eq!(
            header_time(&unknown, decided, march),
            Some(AT_12_33_02 + 10 * 24 * hour)
        );
    }

    #[test]
    fn header_time_rejects_other_lines() {
        assert_eq!(header_time_at(&clock(Some(true), 0), "<== Name(x)"), None);
        assert_eq!(
            header_time_at(&clock(Some(true), 0), "[UnityCrossThreadLogger]==> Name {}"),
            None
        );
        assert_eq!(
            header_time_at(
                &clock(Some(true), 0),
                "[UnityCrossThreadLogger]31/02/2026 1:00:00"
            ),
            None
        );
    }

    #[test]
    fn detect_day_first_from_a_day_above_twelve() {
        assert_eq!(
            detect_day_first(b"[UnityCrossThreadLogger]13/11/2026 12:33:02\n"),
            Some(true)
        );
        assert_eq!(
            detect_day_first(b"[UnityCrossThreadLogger]11/13/2026 12:33:02\n"),
            Some(false)
        );
        assert_eq!(
            detect_day_first(b"[UnityCrossThreadLogger]03/11/2026 12:33:02\n"),
            None
        );
    }

    #[test]
    fn detect_day_first_from_the_fixture() {
        // Every header date in the fixture is 03/11: the order is settled by the
        // GRE time on the line after a header.
        let fixture = fixture_tools::arena_fixture("bo3-loss-and-match-concede");
        assert_eq!(detect_day_first(&fixture.bytes), Some(true));
    }

    #[test]
    fn dotnet_ticks_match_the_gre_time() {
        // The authenticate response's ticks, and the room state GRE time just after it.
        let ms = dotnet_ticks_to_ms(639_293_059_326_051_309);
        assert_eq!(ms, 1_793_709_132_605);
        assert!((1_793_709_133_013_i64 - ms).abs() < 1_000);
    }

    #[test]
    fn date_only_headers_are_recognised() {
        assert!(is_date_only_header(
            "[UnityCrossThreadLogger]03/11/2026 12:28:25"
        ));
        assert!(!is_date_only_header(
            "[UnityCrossThreadLogger]03/11/2026 12:28:25: Match to P"
        ));
        assert!(!is_date_only_header(
            "[UnityCrossThreadLogger]STATE CHANGED"
        ));
    }
}
