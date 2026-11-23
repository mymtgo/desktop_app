//! Reads new bytes from a log file and turns them into whole events (spec 5.2).
//! The last event may still be being written: it is offered as provisional and
//! committed only when the next header arrives or the file has been quiet for
//! `QUIET_COMMIT_MS`.

use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};

use mtgo_protocol::{EventSplitter, RawEvent};

use crate::identity::read_range;

/// Largest single read; a catch-up continues on the next poll (0.x's proven cap).
pub const MAX_READ: u64 = 8 * 1024 * 1024;
/// Quiet time after which the last event is committed (spec 5.2).
pub const QUIET_COMMIT_MS: i64 = 300;
/// Bytes appended within this long of a poll that reached the end of the file were
/// written moments before they were read, so their read time can date them.
pub const LIVE_WINDOW_MS: i64 = 2 * 60 * 1000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TailRead {
    /// Complete events, safe to commit.
    pub events: Vec<RawEvent>,
    /// The last event, which may still grow. Show it, never commit it.
    pub provisional: Option<RawEvent>,
    /// Committed offset after `events`: the next restart resumes here.
    pub cursor: u64,
    /// Everything up to the probed size has been read.
    pub at_eof: bool,
    /// The read time, when these events were appended while the file was being
    /// followed live (spec 5.4). `None` for catch-up reads.
    pub live_read_at_ms: Option<i64>,
    /// New bytes were read.
    pub grew: bool,
}

#[derive(Debug)]
pub struct Tailer {
    path: PathBuf,
    splitter: EventSplitter,
    read_pos: u64,
    last_growth_ms: Option<i64>,
    last_live_growth_ms: Option<i64>,
    last_eof_ms: Option<i64>,
    /// Where live following began: bytes before this were already in the file
    /// (a catch-up), so events starting before it must not carry a read time.
    live_from: Option<u64>,
}

impl Tailer {
    /// Start reading `path` at the committed `cursor`.
    pub fn new(path: PathBuf, cursor: u64) -> Self {
        Self {
            path,
            splitter: EventSplitter::new(cursor),
            read_pos: cursor,
            last_growth_ms: None,
            last_live_growth_ms: None,
            last_eof_ms: None,
            live_from: None,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn cursor(&self) -> u64 {
        self.splitter.committed_offset()
    }

    pub fn last_live_growth_ms(&self) -> Option<i64> {
        self.last_live_growth_ms
    }

    /// Continuation lines dropped because no header came before them.
    pub fn dropped_orphan_lines(&self) -> u64 {
        self.splitter.dropped_orphan_lines()
    }

    /// Read what was appended up to `size` (at most `MAX_READ`), as of `now_ms`.
    pub fn poll(&mut self, size: u64, now_ms: i64) -> io::Result<TailRead> {
        let following = self
            .last_eof_ms
            .is_some_and(|at| now_ms - at <= LIVE_WINDOW_MS);

        if !following {
            self.live_from = None;
        } else if self.live_from.is_none() {
            self.live_from = Some(self.read_pos);
        }

        let mut events = Vec::new();
        let mut grew = false;
        if size > self.read_pos {
            let bytes = self.read_new_bytes((size - self.read_pos).min(MAX_READ))?;
            if !bytes.is_empty() {
                grew = true;
                self.read_pos += bytes.len() as u64;
                self.last_growth_ms = Some(now_ms);
                if following {
                    self.last_live_growth_ms = Some(now_ms);
                }
                events = self.splitter.push(&bytes);
            }
        }

        let at_eof = self.read_pos >= size;
        let quiet = self
            .last_growth_ms
            .is_none_or(|at| now_ms - at >= QUIET_COMMIT_MS);
        if at_eof
            && quiet
            && let Some(last) = self.splitter.take_pending()
        {
            events.push(last);
        }
        let provisional = if at_eof {
            self.splitter.pending()
        } else {
            None
        };
        if at_eof {
            self.last_eof_ms = Some(now_ms);
        }

        let live_from = self.live_from;
        let all_live = events
            .iter()
            .all(|event| live_from.is_some_and(|from| event.offset >= from));
        let live_read_at_ms = (following && all_live).then_some(now_ms);

        Ok(TailRead {
            events,
            provisional,
            cursor: self.cursor(),
            at_eof,
            live_read_at_ms,
            grew,
        })
    }

    /// Open the file and read `len` bytes at the current read position.
    fn read_new_bytes(&self, len: u64) -> io::Result<Vec<u8>> {
        let mut file = File::open(&self.path)?;
        read_range(&mut file, self.read_pos, len)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fixture_tools::TestDir;
    use mtgo_protocol::split_complete;

    const A: &str = "12:00:00 [INF] (UI|first)\ncontinued\n";
    const B: &str = "12:00:01 [INF] (UI|second)\n";
    const C: &str = "12:00:02 [INF] (UI|third)\n";

    fn size(path: &std::path::Path) -> u64 {
        std::fs::metadata(path).expect("metadata").len()
    }

    #[test]
    fn events_commit_when_the_next_header_arrives() {
        let dir = TestDir::new("tail-next");
        let path = dir.write("mtgo.log", format!("{A}{B}").as_bytes());
        let mut tailer = Tailer::new(path.clone(), 0);
        let read = tailer.poll(size(&path), 0).expect("poll");
        assert_eq!(read.events.len(), 1);
        assert_eq!(read.events[0].offset, 0);
        assert_eq!(read.cursor, A.len() as u64);
        assert_eq!(
            read.provisional.as_ref().map(|e| e.text.as_str()),
            Some("12:00:01 [INF] (UI|second)")
        );
        assert!(read.at_eof);
    }

    #[test]
    fn the_last_event_commits_once_the_file_is_quiet() {
        let dir = TestDir::new("tail-quiet");
        let path = dir.write("mtgo.log", format!("{A}{B}").as_bytes());
        let mut tailer = Tailer::new(path.clone(), 0);
        tailer.poll(size(&path), 0).expect("poll");
        let early = tailer.poll(size(&path), QUIET_COMMIT_MS - 1).expect("poll");
        assert!(early.events.is_empty());
        let quiet = tailer.poll(size(&path), QUIET_COMMIT_MS).expect("poll");
        assert_eq!(quiet.events.len(), 1);
        assert_eq!(quiet.cursor, size(&path));
        assert_eq!(quiet.provisional, None);
    }

    #[test]
    fn a_half_written_line_waits_for_its_newline() {
        let dir = TestDir::new("tail-half");
        let path = dir.write("mtgo.log", format!("{A}12:00:01 [INF] (UI|sec").as_bytes());
        let mut tailer = Tailer::new(path.clone(), 0);
        tailer.poll(size(&path), 0).expect("poll");
        let waiting = tailer.poll(size(&path), 10_000).expect("poll");
        assert_eq!(
            waiting.cursor, 0,
            "the first event waits for a whole next header"
        );
        dir.append("mtgo.log", b"ond)\n");
        let read = tailer.poll(size(&path), 10_001).expect("poll");
        assert_eq!(read.events.len(), 1);
        assert_eq!(read.cursor, A.len() as u64);
    }

    #[test]
    fn catch_up_reads_stop_at_eight_megabytes() {
        let dir = TestDir::new("tail-chunk");
        let line = "12:00:00 [INF] (UI|filler line for the chunk limit test)\n";
        let count = (MAX_READ as usize / line.len()) + 1000;
        let path = dir.write("mtgo.log", line.repeat(count).as_bytes());
        let total = size(&path);
        let mut tailer = Tailer::new(path.clone(), 0);
        let first = tailer.poll(total, 0).expect("poll");
        assert!(!first.at_eof);
        assert!(first.cursor <= MAX_READ);
        let second = tailer.poll(total, 1).expect("poll");
        assert!(second.at_eof);
        assert_eq!(first.events.len() + second.events.len(), count - 1);
    }

    #[test]
    fn bytes_read_while_following_carry_their_read_time() {
        let dir = TestDir::new("tail-live");
        let path = dir.write("mtgo.log", A.as_bytes());
        let mut tailer = Tailer::new(path.clone(), 0);
        let catch_up = tailer.poll(size(&path), 1_000).expect("poll");
        assert_eq!(
            catch_up.live_read_at_ms, None,
            "the first read is a catch-up"
        );

        dir.append("mtgo.log", B.as_bytes());
        let first_live = tailer.poll(size(&path), 2_000).expect("poll");
        assert_eq!(first_live.events.len(), 1, "A commits when B arrives");
        assert_eq!(
            first_live.live_read_at_ms, None,
            "A was already in the file before following began"
        );

        dir.append("mtgo.log", C.as_bytes());
        let live = tailer.poll(size(&path), 2_100).expect("poll");
        assert_eq!(live.events.len(), 1, "B commits when C arrives");
        assert_eq!(live.live_read_at_ms, Some(2_100));
        assert_eq!(tailer.last_live_growth_ms(), Some(2_100));

        dir.append("mtgo.log", b"12:00:03 [INF] (UI|fourth)\n");
        let late = tailer
            .poll(size(&path), 2_100 + LIVE_WINDOW_MS + 1)
            .expect("poll");
        assert_eq!(late.live_read_at_ms, None, "too long since the last poll");
    }

    #[test]
    fn the_catch_up_tail_is_never_dated_as_a_live_read() {
        let dir = TestDir::new("tail-catchup-tail");
        let path = dir.write("mtgo.log", format!("{A}{B}").as_bytes());
        let mut tailer = Tailer::new(path.clone(), 0);
        let catch_up = tailer.poll(size(&path), 0).expect("poll");
        assert_eq!(catch_up.live_read_at_ms, None);
        assert!(catch_up.provisional.is_some());

        let quiet = tailer.poll(size(&path), QUIET_COMMIT_MS).expect("poll");
        assert_eq!(quiet.events.len(), 1, "the catch-up tail commits");
        assert_eq!(quiet.live_read_at_ms, None);
    }

    #[test]
    fn orphan_lines_before_the_first_header_are_counted() {
        let dir = TestDir::new("tail-orphans");
        let path = dir.write("mtgo.log", format!("half of an event\n{A}{B}").as_bytes());
        let mut tailer = Tailer::new(path.clone(), 0);
        assert_eq!(tailer.dropped_orphan_lines(), 0);
        tailer.poll(size(&path), 0).expect("poll");
        assert_eq!(tailer.dropped_orphan_lines(), 1);
        dir.append("mtgo.log", C.as_bytes());
        tailer.poll(size(&path), 1).expect("poll");
        assert_eq!(tailer.dropped_orphan_lines(), 1);
    }

    #[test]
    fn crlf_files_resume_at_exact_offsets() {
        let dir = TestDir::new("tail-crlf");
        let crlf = format!("{A}{B}{C}").replace('\n', "\r\n");
        let path = dir.write("mtgo.log", crlf.as_bytes());
        let mut tailer = Tailer::new(path.clone(), 0);
        let read = tailer.poll(size(&path), 0).expect("poll");
        assert_eq!(read.events.len(), 2);
        let a_len = A.replace('\n', "\r\n").len() as u64;
        let b_len = B.replace('\n', "\r\n").len() as u64;
        assert_eq!(read.cursor, a_len + b_len);

        let mut resumed = Tailer::new(path.clone(), read.cursor);
        dir.append("mtgo.log", b"12:00:03 [INF] (UI|fourth)\r\n");
        let after = resumed.poll(size(&path), 0).expect("poll");
        assert_eq!(after.events.len(), 1);
        assert_eq!(after.events[0].offset, a_len + b_len);
        assert_eq!(after.events[0].text, "12:00:02 [INF] (UI|third)");
    }

    /// Poll until the file is read to the end and the last event commits. Returns
    /// each read that produced events, in order.
    fn read_all(tailer: &mut Tailer, total: u64) -> Vec<TailRead> {
        let mut reads = Vec::new();
        let mut now = 0;
        loop {
            let read = tailer.poll(total, now).expect("poll");
            now += 1_000;
            let finished = read.at_eof && read.events.is_empty() && !read.grew;
            if !read.events.is_empty() {
                reads.push(read);
            }
            if finished {
                return reads;
            }
        }
    }

    fn assert_joined_without_gaps(reads: &[TailRead], total: u64, expected: &[RawEvent]) {
        for pair in reads.windows(2) {
            assert_eq!(
                pair[1].events[0].offset, pair[0].cursor,
                "a read starts where the previous one committed"
            );
        }
        let all: Vec<RawEvent> = reads.iter().flat_map(|r| r.events.clone()).collect();
        for pair in all.windows(2) {
            assert_eq!(pair[0].offset + pair[0].len, pair[1].offset, "no gap");
        }
        assert_eq!(all.iter().map(|e| e.len).sum::<u64>(), total);
        assert_eq!(all, expected, "the same events as one whole-file split");
    }

    #[test]
    fn consecutive_capped_reads_join_with_no_gap() {
        let dir = TestDir::new("tail-two-chunks");
        // A line length that does not divide the cap, so each cap lands mid-line.
        let line = "12:00:00 [INF] (UI|filler line for the two chunk test)\n";
        assert_ne!(MAX_READ % line.len() as u64, 0);
        let count = (2 * MAX_READ as usize / line.len()) + 500;
        let bytes = line.repeat(count).into_bytes();
        let path = dir.write("mtgo.log", &bytes);
        let total = size(&path);
        assert!(total > 2 * MAX_READ);

        let mut tailer = Tailer::new(path, 0);
        let reads = read_all(&mut tailer, total);
        assert!(reads.len() >= 3, "{} reads", reads.len());
        assert_joined_without_gaps(&reads, total, &split_complete(&bytes));
    }

    #[test]
    fn a_read_cap_between_cr_and_lf_loses_nothing() {
        let dir = TestDir::new("tail-cap-crlf");
        let cap = MAX_READ as usize;
        let line = format!("12:00:00 [INF] (UI|{})\r\n", "x".repeat(37));
        let mut bytes = Vec::new();
        while cap + 1 - bytes.len() >= line.len() + 30 {
            bytes.extend_from_slice(line.as_bytes());
        }
        // The last line of the first read ends exactly in its `\r`.
        let room = cap + 1 - bytes.len();
        let split_line = format!("12:00:00 [INF] (UI|{})", "y".repeat(room - 19 - 1 - 2));
        bytes.extend_from_slice(split_line.as_bytes());
        bytes.extend_from_slice(b"\r\n");
        assert_eq!(bytes.len(), cap + 1);
        assert_eq!(bytes[cap - 1], b'\r');
        assert_eq!(bytes[cap], b'\n');
        for n in 0..5 {
            bytes.extend_from_slice(format!("12:00:01 [INF] (UI|after {n})\r\n").as_bytes());
        }
        let path = dir.write("mtgo.log", &bytes);
        let total = size(&path);

        let mut tailer = Tailer::new(path, 0);
        let first = tailer.poll(total, 0).expect("poll");
        assert!(!first.at_eof, "the cap stopped the first read");
        assert!(
            first.events.iter().all(|e| !e.text.contains('\r')),
            "no stray carriage return"
        );
        let split_at = (cap + 1 - split_line.len() - 2) as u64;
        // The line before the split one is still pending (its successor's header is
        // not whole yet), so the cursor sits at its start, not past the `\r`.
        assert_eq!(first.cursor, split_at - line.len() as u64);

        let mut reads = vec![first];
        reads.extend(read_all(&mut tailer, total));
        let events: Vec<RawEvent> = reads.iter().flat_map(|r| r.events.clone()).collect();
        let joined = events
            .iter()
            .find(|event| event.offset == split_at)
            .expect("the split line is delivered whole");
        assert_eq!(joined.text, split_line);
        assert_eq!(joined.len, split_line.len() as u64 + 2);
        assert_joined_without_gaps(&reads, total, &split_complete(&bytes));
    }
}
