//! Is this still the same log file? Ported from 0.x `DetectLogRotation` and
//! `IngestLogInstance::observe` (spec 5.2). MTGO rotates and replaces `mtgo.log`;
//! rotation is expected, never an error.

use std::fs::File;
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::Path;
use std::time::UNIX_EPOCH;

use chrono::NaiveTime;
use mtgo_protocol::header_time;

use crate::hash::fnv1a64;

/// Bytes hashed from the start of the file. Until the file reaches this size the
/// head keeps growing with it; after that it is frozen (0.x behaviour).
pub const HEAD_LEN: u64 = 4096;
/// Where the anchor sits, and how many bytes it covers. Set once the committed
/// cursor has passed `ANCHOR_OFFSET + ANCHOR_LEN`.
pub const ANCHOR_OFFSET: u64 = 16 * 1024;
pub const ANCHOR_LEN: u64 = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Anchor {
    pub offset: u64,
    pub hash: u64,
}

/// What the file looks like now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Probe {
    pub size: u64,
    /// True creation time (`Metadata::created`). Linux `ctime` is never used: it
    /// changes on every write (spec 5.2).
    pub created_ms: Option<i64>,
    /// Last write time (`Metadata::modified`): when the file's last line was
    /// written. A new file's day anchor is worked back from it (issue #48).
    pub modified_ms: Option<i64>,
    pub head_hash: u64,
    pub head_len: u64,
    pub anchor_hash: Option<u64>,
}

/// What was recorded about the file when it was last read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recorded {
    pub last_observed_size: u64,
    pub created_ms: Option<i64>,
    pub head_hash: u64,
    pub head_len: u64,
    pub anchor: Option<Anchor>,
}

impl Recorded {
    /// The record for a file seen for the first time.
    pub fn first(probe: &Probe) -> Self {
        Self {
            last_observed_size: 0,
            created_ms: probe.created_ms,
            head_hash: probe.head_hash,
            head_len: probe.head_len,
            anchor: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SealReason {
    Truncated,
    CreatedForward,
    HeadChanged,
    AnchorChanged,
}

impl SealReason {
    /// The `log_files.seal_reason` value.
    pub fn as_str(self) -> &'static str {
        match self {
            SealReason::Truncated => "truncated",
            SealReason::CreatedForward => "created_forward",
            SealReason::HeadChanged => "head_changed",
            SealReason::AnchorChanged => "anchor_changed",
        }
    }
}

/// Read size, creation time, head hash, and (when `anchor_offset` is given and the
/// file is long enough) the anchor hash.
pub fn probe(path: &Path, anchor_offset: Option<u64>) -> io::Result<Probe> {
    let mut file = File::open(path)?;
    let meta = file.metadata()?;
    let size = meta.len();
    let head = read_range(&mut file, 0, HEAD_LEN)?;
    // A corrupt stored offset so large that the anchor's end overflows reads no
    // anchor, like a file too short to have one.
    let anchor_hash = match anchor_offset {
        Some(offset)
            if offset
                .checked_add(ANCHOR_LEN)
                .is_some_and(|end| size >= end) =>
        {
            Some(fnv1a64(&read_range(&mut file, offset, ANCHOR_LEN)?))
        }
        _ => None,
    };
    Ok(Probe {
        size,
        created_ms: created_ms(&meta),
        modified_ms: modified_ms(&meta),
        head_hash: fnv1a64(&head),
        head_len: head.len() as u64,
        anchor_hash,
    })
}

/// The anchor to record, once the file is long enough to have one.
pub fn read_anchor(path: &Path) -> io::Result<Option<Anchor>> {
    let mut file = File::open(path)?;
    if file.metadata()?.len() < ANCHOR_OFFSET + ANCHOR_LEN {
        return Ok(None);
    }
    let bytes = read_range(&mut file, ANCHOR_OFFSET, ANCHOR_LEN)?;
    Ok(Some(Anchor {
        offset: ANCHOR_OFFSET,
        hash: fnv1a64(&bytes),
    }))
}

/// The first rule that says the file was replaced, in 0.x order.
pub fn detect_rotation(recorded: &Recorded, probe: &Probe) -> Option<SealReason> {
    if recorded.last_observed_size > 0 && probe.size < recorded.last_observed_size {
        return Some(SealReason::Truncated);
    }
    if let (Some(before), Some(now)) = (recorded.created_ms, probe.created_ms)
        && now > before
    {
        return Some(SealReason::CreatedForward);
    }
    if recorded.head_len >= HEAD_LEN && probe.head_hash != recorded.head_hash {
        return Some(SealReason::HeadChanged);
    }
    if let (Some(anchor), Some(hash)) = (recorded.anchor, probe.anchor_hash)
        && hash != anchor.hash
    {
        return Some(SealReason::AnchorChanged);
    }
    None
}

/// Accept the probe as the new normal: the observed size, and the head while it
/// is still growing towards `HEAD_LEN`.
pub fn observe(recorded: &mut Recorded, probe: &Probe) {
    recorded.last_observed_size = probe.size;
    if recorded.head_len < HEAD_LEN {
        recorded.head_hash = probe.head_hash;
        recorded.head_len = probe.head_len;
    }
}

/// `log_files.identity_hash`: head, creation time and size when first seen.
pub fn identity_hash(probe: &Probe) -> u64 {
    let text = format!(
        "{}:{}:{}",
        probe.head_hash,
        probe.created_ms.unwrap_or(0),
        probe.size
    );
    fnv1a64(text.as_bytes())
}

fn created_ms(meta: &std::fs::Metadata) -> Option<i64> {
    let created = meta.created().ok()?;
    let since = created.duration_since(UNIX_EPOCH).ok()?;
    i64::try_from(since.as_millis()).ok()
}

fn modified_ms(meta: &std::fs::Metadata) -> Option<i64> {
    let modified = meta.modified().ok()?;
    let since = modified.duration_since(UNIX_EPOCH).ok()?;
    i64::try_from(since.as_millis()).ok()
}

/// The time of day of every event header in the first `limit` bytes of the
/// file, in byte order (issue #48: a new file's day anchor is worked back from
/// them and its last write). Pass the probed size, so a line appended after the
/// probe took the last write is not read.
pub fn header_times(path: &Path, limit: u64) -> io::Result<Vec<NaiveTime>> {
    let mut reader = BufReader::new(File::open(path)?.take(limit));
    let mut times = Vec::new();
    let mut line = Vec::new();
    while reader.read_until(b'\n', &mut line)? > 0 {
        if let Some(time) = header_time(&String::from_utf8_lossy(&line)) {
            times.push(time);
        }
        line.clear();
    }
    Ok(times)
}

/// Seek to `offset` and read up to `len` bytes (fewer at the end of the file,
/// none past it). Shared with the MTGO and Arena tailers.
pub fn read_range(file: &mut File, offset: u64, len: u64) -> io::Result<Vec<u8>> {
    file.seek(SeekFrom::Start(offset))?;
    let mut bytes = Vec::new();
    file.by_ref().take(len).read_to_end(&mut bytes)?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fixture_tools::TestDir;

    fn recorded_after(path: &std::path::Path) -> Recorded {
        let first = probe(path, None).expect("probe");
        let mut recorded = Recorded::first(&first);
        observe(&mut recorded, &first);
        recorded
    }

    #[test]
    fn a_growing_file_is_the_same_file_and_its_head_follows_until_frozen() {
        let dir = TestDir::new("identity-grow");
        let path = dir.write("mtgo.log", b"12:00:00 [INF] (UI|a)\n");
        let mut recorded = recorded_after(&path);
        dir.append("mtgo.log", b"12:00:01 [INF] (UI|b)\n");
        let now = probe(&path, None).expect("probe");
        assert_eq!(detect_rotation(&recorded, &now), None);
        assert_ne!(
            recorded.head_hash, now.head_hash,
            "the file grew, so its head hash moved"
        );
        observe(&mut recorded, &now);
        assert_eq!(recorded.head_hash, now.head_hash);
        assert_eq!(recorded.last_observed_size, now.size);
    }

    #[test]
    fn a_shrunk_file_was_truncated() {
        let dir = TestDir::new("identity-truncate");
        let path = dir.write("mtgo.log", &[b'a'; 100]);
        let recorded = recorded_after(&path);
        dir.write("mtgo.log", &[b'a'; 10]);
        let now = probe(&path, None).expect("probe");
        assert_eq!(
            detect_rotation(&recorded, &now),
            Some(SealReason::Truncated)
        );
    }

    #[test]
    fn a_rewritten_head_after_it_froze_is_a_new_file() {
        let dir = TestDir::new("identity-head");
        let path = dir.write("mtgo.log", &[b'a'; 5000]);
        let recorded = recorded_after(&path);
        assert_eq!(recorded.head_len, HEAD_LEN);
        dir.write("mtgo.log", &[b'b'; 5000]);
        let now = probe(&path, None).expect("probe");
        assert_eq!(
            detect_rotation(&recorded, &now),
            Some(SealReason::HeadChanged)
        );
    }

    #[test]
    fn a_rewritten_anchor_is_a_new_file() {
        let dir = TestDir::new("identity-anchor");
        let size = (ANCHOR_OFFSET + ANCHOR_LEN) as usize + 10;
        let path = dir.write("mtgo.log", &vec![b'a'; size]);
        let mut recorded = recorded_after(&path);
        recorded.anchor = read_anchor(&path).expect("read anchor");
        assert!(recorded.anchor.is_some());

        let mut bytes = vec![b'a'; size];
        bytes[ANCHOR_OFFSET as usize + 5] = b'z';
        dir.write("mtgo.log", &bytes);
        let now = probe(&path, Some(ANCHOR_OFFSET)).expect("probe");
        assert_eq!(
            detect_rotation(&recorded, &now),
            Some(SealReason::AnchorChanged)
        );
    }

    #[test]
    fn a_later_creation_time_is_a_new_file() {
        let recorded = Recorded {
            last_observed_size: 10,
            created_ms: Some(1_000),
            head_hash: 1,
            head_len: 10,
            anchor: None,
        };
        let now = Probe {
            size: 20,
            created_ms: Some(2_000),
            modified_ms: None,
            head_hash: 1,
            head_len: 10,
            anchor_hash: None,
        };
        assert_eq!(
            detect_rotation(&recorded, &now),
            Some(SealReason::CreatedForward)
        );
    }

    #[test]
    fn a_corrupt_stored_anchor_offset_reads_no_anchor() {
        let dir = TestDir::new("identity-corrupt-anchor");
        let path = dir.write("mtgo.log", &[b'a'; 100]);
        let now = probe(&path, Some(u64::MAX - 10)).expect("probe");
        assert_eq!(now.anchor_hash, None);
        assert_eq!(now.size, 100);
    }

    #[test]
    fn the_anchor_needs_enough_bytes() {
        let dir = TestDir::new("identity-short");
        let path = dir.write("mtgo.log", &[b'a'; 100]);
        assert_eq!(read_anchor(&path).expect("read"), None);
    }

    #[test]
    fn seal_reasons_have_their_stored_names() {
        assert_eq!(SealReason::Truncated.as_str(), "truncated");
        assert_eq!(SealReason::CreatedForward.as_str(), "created_forward");
        assert_eq!(SealReason::HeadChanged.as_str(), "head_changed");
        assert_eq!(SealReason::AnchorChanged.as_str(), "anchor_changed");
    }

    #[test]
    fn read_range_returns_up_to_len_bytes_from_the_offset() {
        let dir = TestDir::new("identity-range");
        let path = dir.write("mtgo.log", b"0123456789");
        let mut file = std::fs::File::open(&path).expect("open");
        assert_eq!(read_range(&mut file, 3, 4).expect("read"), b"3456");
        assert_eq!(read_range(&mut file, 8, 10).expect("read"), b"89");
        assert!(read_range(&mut file, 50, 10).expect("read").is_empty());
    }

    fn recorded_with(created_ms: Option<i64>, head_hash: u64, head_len: u64) -> Recorded {
        Recorded {
            last_observed_size: 10,
            created_ms,
            head_hash,
            head_len,
            anchor: None,
        }
    }

    fn probe_with(created_ms: Option<i64>, head_hash: u64, head_len: u64) -> Probe {
        Probe {
            size: 20,
            created_ms,
            modified_ms: None,
            head_hash,
            head_len,
            anchor_hash: None,
        }
    }

    #[test]
    fn header_times_and_the_last_write_are_read() {
        let dir = TestDir::new("identity-header-times");
        let bytes = b"16:17:47 [INF] (UI|a)\ncontinuation line\n16:18:50 [INF] (UI|b)\n";
        let path = dir.write("mtgo.log", bytes);
        let times = header_times(&path, bytes.len() as u64).expect("times");
        assert_eq!(
            times,
            vec![
                chrono::NaiveTime::from_hms_opt(16, 17, 47).expect("time"),
                chrono::NaiveTime::from_hms_opt(16, 18, 50).expect("time"),
            ]
        );
        let written = std::time::UNIX_EPOCH + std::time::Duration::from_millis(1_781_000_000_000);
        std::fs::File::options()
            .write(true)
            .open(&path)
            .expect("open")
            .set_modified(written)
            .expect("set the last write");
        assert_eq!(
            probe(&path, None).expect("probe").modified_ms,
            Some(1_781_000_000_000)
        );
    }

    #[test]
    fn header_times_stop_at_the_limit() {
        // A line MTGO appends after the probe took the size and last write is
        // not read, so it cannot move the day anchor.
        let dir = TestDir::new("identity-header-times-limit");
        let first = b"23:59:58 [INF] (UI|a)\n";
        let path = dir.write("mtgo.log", first);
        dir.append("mtgo.log", b"00:00:01 [INF] (UI|b)\n");
        let times = header_times(&path, first.len() as u64).expect("times");
        assert_eq!(
            times,
            vec![chrono::NaiveTime::from_hms_opt(23, 59, 58).expect("time")]
        );
    }

    #[test]
    fn observe_keeps_a_frozen_head() {
        let mut recorded = recorded_with(None, 1, HEAD_LEN);
        observe(&mut recorded, &probe_with(None, 2, HEAD_LEN));
        assert_eq!((recorded.head_hash, recorded.head_len), (1, HEAD_LEN));
        assert_eq!(recorded.last_observed_size, 20, "the size is still taken");
    }

    #[test]
    fn observe_follows_a_head_that_is_still_growing() {
        let mut recorded = recorded_with(None, 1, HEAD_LEN - 1);
        observe(&mut recorded, &probe_with(None, 2, HEAD_LEN));
        assert_eq!((recorded.head_hash, recorded.head_len), (2, HEAD_LEN));
    }

    #[test]
    fn a_missing_creation_time_on_either_side_disables_created_forward() {
        let both = [(None, Some(2_000)), (Some(1_000), None), (None, None)];
        for (before, now) in both {
            assert_eq!(
                detect_rotation(&recorded_with(before, 1, 10), &probe_with(now, 1, 10)),
                None,
                "recorded {before:?}, now {now:?}"
            );
        }
    }

    #[test]
    fn an_earlier_or_equal_creation_time_does_not_rotate() {
        for now in [1_000, 500] {
            assert_eq!(
                detect_rotation(
                    &recorded_with(Some(1_000), 1, 10),
                    &probe_with(Some(now), 1, 10)
                ),
                None,
                "created {now}"
            );
        }
    }

    #[test]
    fn a_head_change_while_the_head_is_under_4kb_is_not_a_rotation() {
        assert_eq!(
            detect_rotation(
                &recorded_with(None, 1, HEAD_LEN - 1),
                &probe_with(None, 2, HEAD_LEN)
            ),
            None
        );
    }
}
