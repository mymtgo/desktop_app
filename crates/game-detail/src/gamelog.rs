//! MTGO's per-match game log file (`*Match_GameLog*.dat`), port of 0.x
//! `ParseGameLogBinary`.
//!
//! Layout: a header (u8 version, u8 flag, u8 length + match UUID, u8 type,
//! u8 flag, u8 length + game UUID), then records until the end of the file:
//! i64 little-endian .NET ticks, a u8 flag, a 7-bit varint length (.NET
//! `Write7BitEncodedInt`) and that many message bytes. A file under 78 bytes
//! reads as `None`. A record cut short at the end (MTGO still writing) is
//! dropped.
//!
//! The ticks are local wall-clock time on the machine that ran MTGO, so they
//! are read in the timezone passed in and converted to UTC.

use chrono::{DateTime, NaiveDateTime};
use chrono_tz::Tz;

/// Smallest file 0.x reads: anything shorter has no complete header.
pub const MIN_GAME_LOG_BYTES: usize = 78;

/// Ticks between 0001-01-01 and 1970-01-01.
const DOTNET_EPOCH_OFFSET: i128 = 621_355_968_000_000_000;
const TICKS_PER_SECOND: i128 = 10_000_000;

/// One record of a game log: when it was written and its text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameLogEntry {
    /// UTC unix milliseconds (whole seconds, as 0.x kept them).
    pub timestamp_ms: i64,
    pub message: String,
}

/// A decoded game log file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameLog {
    pub version: u8,
    pub kind: u8,
    pub match_uuid: String,
    pub game_uuid: String,
    pub entries: Vec<GameLogEntry>,
}

/// Decode a game log file's bytes. `tz` is the timezone of the machine that
/// ran MTGO (the ticks are its wall-clock time).
pub fn parse_game_log(raw: &[u8], tz: Tz) -> Option<GameLog> {
    if raw.len() < MIN_GAME_LOG_BYTES {
        return None;
    }
    let mut pos = 0;
    let version = raw[pos];
    pos += 2; // version, then an unknown flag
    let (match_uuid, after) = read_short_string(raw, pos)?;
    pos = after;
    let kind = *raw.get(pos)?;
    pos += 2; // type, then an unknown flag
    let (game_uuid, after) = read_short_string(raw, pos)?;
    pos = after;

    let mut entries = Vec::new();
    while pos + 10 <= raw.len() {
        let Some((ticks, message, next)) = read_record(raw, pos) else {
            break;
        };
        pos = next;
        if let Some(timestamp_ms) = ticks_to_utc_ms(ticks, tz) {
            entries.push(GameLogEntry {
                timestamp_ms,
                message,
            });
        }
    }

    Some(GameLog {
        version,
        kind,
        match_uuid,
        game_uuid,
        entries,
    })
}

/// A u8 length then that many bytes, from `pos`. Returns the text and the
/// position after it.
fn read_short_string(raw: &[u8], pos: usize) -> Option<(String, usize)> {
    let len = *raw.get(pos)? as usize;
    let start = pos + 1;
    let bytes = raw.get(start..start + len)?;
    Some((String::from_utf8_lossy(bytes).into_owned(), start + len))
}

/// One record from `pos`: its ticks, its message and the position after it.
/// `None` when the record is cut short.
fn read_record(raw: &[u8], pos: usize) -> Option<(u64, String, usize)> {
    let tick_bytes: [u8; 8] = raw.get(pos..pos + 8)?.try_into().ok()?;
    let ticks = u64::from_le_bytes(tick_bytes);
    let mut at = pos + 9; // ticks, then a flag byte
    let (len, after) = read_varint(raw, at)?;
    at = after;
    let message = raw.get(at..at.checked_add(len)?)?;
    Some((
        ticks,
        String::from_utf8_lossy(message).into_owned(),
        at + len,
    ))
}

/// The most bytes a 7-bit encoded length may take (a `u64` needs ten).
const MAX_VARINT_BYTES: usize = 10;

/// A .NET 7-bit encoded length from `pos`, and the position after it. `None`
/// past the end of `raw` or after [`MAX_VARINT_BYTES`] bytes.
fn read_varint(raw: &[u8], mut pos: usize) -> Option<(usize, usize)> {
    let mut value: u64 = 0;
    let mut shift = 0;
    let mut read = 0;
    loop {
        if read == MAX_VARINT_BYTES {
            return None;
        }
        read += 1;
        let byte = *raw.get(pos)?;
        pos += 1;
        if shift < 64 {
            value |= u64::from(byte & 0x7F) << shift;
        }
        shift += 7;
        if byte & 0x80 == 0 {
            break;
        }
    }
    Some((usize::try_from(value).ok()?, pos))
}

/// .NET ticks of a local wall-clock time in `tz` to UTC unix milliseconds,
/// truncated to the second as 0.x did. A wall-clock time inside a DST gap is
/// moved on by the real length of the gap; an ambiguous one takes the earlier instant.
fn ticks_to_utc_ms(ticks: u64, tz: Tz) -> Option<i64> {
    let seconds = (i128::from(ticks) - DOTNET_EPOCH_OFFSET) / TICKS_PER_SECOND;
    let seconds = i64::try_from(seconds).ok()?;
    let wall_clock: NaiveDateTime = DateTime::from_timestamp(seconds, 0)?.naive_utc();
    mtgo_protocol::local_to_utc_ms(wall_clock, tz)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MATCH_UUID: &str = "11111111-2222-4333-8444-555555555555";

    /// .NET ticks of a UTC wall-clock time.
    fn ticks(wall_clock: &str) -> u64 {
        let at = NaiveDateTime::parse_from_str(wall_clock, "%Y-%m-%d %H:%M:%S").expect("time");
        let seconds = at.and_utc().timestamp();
        (i128::from(seconds) * TICKS_PER_SECOND + DOTNET_EPOCH_OFFSET) as u64
    }

    fn header() -> Vec<u8> {
        let mut out = vec![1, 0, MATCH_UUID.len() as u8];
        out.extend_from_slice(MATCH_UUID.as_bytes());
        out.extend_from_slice(&[4, 0, MATCH_UUID.len() as u8]);
        out.extend_from_slice(MATCH_UUID.as_bytes());
        out
    }

    fn record(ticks: u64, message: &str) -> Vec<u8> {
        let mut out = ticks.to_le_bytes().to_vec();
        out.push(0);
        let mut len = message.len();
        loop {
            let byte = (len & 0x7F) as u8;
            len >>= 7;
            if len == 0 {
                out.push(byte);
                break;
            }
            out.push(byte | 0x80);
        }
        out.extend_from_slice(message.as_bytes());
        out
    }

    fn file(records: &[(&str, &str)]) -> Vec<u8> {
        let mut out = header();
        for (at, message) in records {
            out.extend(record(ticks(at), message));
        }
        out
    }

    fn ms(utc: &str) -> i64 {
        NaiveDateTime::parse_from_str(utc, "%Y-%m-%d %H:%M:%S")
            .expect("time")
            .and_utc()
            .timestamp_millis()
    }

    #[test]
    fn reads_the_header_and_every_record() {
        let raw = file(&[
            ("2026-05-26 10:00:00", "@PQalpha rolled a 4."),
            ("2026-05-26 10:00:02", "@P@PQalpha joined the game."),
        ]);
        let log = parse_game_log(&raw, chrono_tz::UTC).expect("log");
        assert_eq!(log.version, 1);
        assert_eq!(log.kind, 4);
        assert_eq!(log.match_uuid, MATCH_UUID);
        assert_eq!(log.game_uuid, MATCH_UUID);
        assert_eq!(
            log.entries,
            vec![
                GameLogEntry {
                    timestamp_ms: ms("2026-05-26 10:00:00"),
                    message: "@PQalpha rolled a 4.".to_string(),
                },
                GameLogEntry {
                    timestamp_ms: ms("2026-05-26 10:00:02"),
                    message: "@P@PQalpha joined the game.".to_string(),
                },
            ]
        );
    }

    #[test]
    fn returns_none_for_empty_input() {
        assert_eq!(parse_game_log(b"", chrono_tz::UTC), None);
    }

    #[test]
    fn returns_none_for_a_truncated_header() {
        assert_eq!(parse_game_log(&[0u8; 10], chrono_tz::UTC), None);
        assert_eq!(parse_game_log(&[0u8; 77], chrono_tz::UTC), None);
    }

    #[test]
    fn a_header_with_no_records_has_no_entries() {
        let log = parse_game_log(&header(), chrono_tz::UTC).expect("78 byte header");
        assert!(log.entries.is_empty());
    }

    #[test]
    fn a_uuid_length_past_the_end_reads_as_none() {
        let mut raw = vec![1, 0, 200];
        raw.resize(MIN_GAME_LOG_BYTES, b'a');
        assert_eq!(parse_game_log(&raw, chrono_tz::UTC), None);
    }

    #[test]
    fn a_cut_short_last_record_is_dropped() {
        let full = file(&[
            ("2026-05-26 10:00:00", "@PQalpha rolled a 4."),
            ("2026-05-26 10:00:02", "@PQalpha wins the game."),
        ]);
        for cut in 1..20 {
            let log = parse_game_log(&full[..full.len() - cut], chrono_tz::UTC).expect("log");
            assert_eq!(log.entries.len(), 1, "cut {cut} bytes");
        }
    }

    #[test]
    fn a_message_longer_than_127_bytes_uses_a_two_byte_length() {
        let long = format!("@PQalpha casts @[{}@:1000,1:@].", "x".repeat(300));
        let raw = file(&[("2026-05-26 10:00:00", &long)]);
        let log = parse_game_log(&raw, chrono_tz::UTC).expect("log");
        assert_eq!(log.entries[0].message, long);
    }

    #[test]
    fn a_length_prefix_longer_than_ten_bytes_is_refused() {
        let mut raw = vec![0x80; 10];
        raw.push(0x01);
        assert_eq!(read_varint(&raw, 0), None);
        let mut raw = vec![0x80; 9];
        raw.push(0x00);
        assert_eq!(read_varint(&raw, 0), Some((0, 10)));
        assert_eq!(read_varint(&[0x81, 0x01], 0), Some((129, 2)));
    }

    #[test]
    fn ticks_are_local_wall_clock_in_the_given_timezone() {
        let raw = file(&[("2026-05-26 10:00:00", "@PQalpha rolled a 4.")]);
        let utc = parse_game_log(&raw, chrono_tz::UTC).expect("log");
        let la = parse_game_log(&raw, chrono_tz::America::Los_Angeles).expect("log");
        // Los Angeles is UTC-7 in May: 10:00 there is 17:00 UTC.
        assert_eq!(
            la.entries[0].timestamp_ms - utc.entries[0].timestamp_ms,
            7 * 3_600_000
        );
    }

    #[test]
    fn sub_second_ticks_are_truncated_to_the_second() {
        let mut raw = header();
        raw.extend(record(
            ticks("2026-05-26 10:00:00") + 9_999_999,
            "@PQalpha rolled a 4.",
        ));
        let log = parse_game_log(&raw, chrono_tz::UTC).expect("log");
        assert_eq!(log.entries[0].timestamp_ms, ms("2026-05-26 10:00:00"));
    }

    #[test]
    fn a_wall_clock_time_in_a_dst_gap_moves_an_hour_on() {
        // 2026-03-08 02:30 does not exist in Los Angeles (clocks jump 02:00 to 03:00).
        let raw = file(&[("2026-03-08 02:30:00", "@PQalpha rolled a 4.")]);
        let log = parse_game_log(&raw, chrono_tz::America::Los_Angeles).expect("log");
        // 03:30 PDT is 10:30 UTC.
        assert_eq!(log.entries[0].timestamp_ms, ms("2026-03-08 10:30:00"));
    }

    #[test]
    fn a_dst_gap_of_half_an_hour_moves_by_half_an_hour() {
        // Lord Howe jumps 02:00 to 02:30 on 2026-10-04 (+10:30 to +11:00).
        let raw = file(&[("2026-10-04 02:15:00", "@PQalpha rolled a 4.")]);
        let log = parse_game_log(&raw, chrono_tz::Australia::Lord_Howe).expect("log");
        // 02:45 at +11:00 is 15:45 UTC the day before.
        assert_eq!(log.entries[0].timestamp_ms, ms("2026-10-03 15:45:00"));
    }
}
