//! Splits raw log bytes into whole events. An event is a header line plus every
//! following line up to the next header line. The last event in the buffer is
//! "pending" (provisional) until the next header arrives or the caller decides
//! the file has been quiet long enough to commit it.
//!
//! An event holds at most [`MAX_EVENT_BYTES`], so a crafted or corrupt log
//! cannot make the splitter buffer without bound (MTGO's largest real lines
//! are about 1 MB). The cap works on whole lines, so the result is the same
//! however the bytes arrive:
//!
//! - A continuation line that would take its event past the cap ends the
//!   event: the event is committed as it stands, and that line and the rest
//!   of the event's continuation lines are dropped as orphans.
//! - A header line longer than the cap is dropped, and the continuation lines
//!   after it are orphans.
//!
//! Each time the cap fires, [`EventSplitter::capped_events`] counts it, for
//! the caller to warn about. A line longer than the cap is not buffered: its
//! bytes are skipped as they arrive.

use crate::header::is_header_line;

/// The most bytes one event may hold, line endings included.
pub const MAX_EVENT_BYTES: usize = 16 * 1024 * 1024;

/// How many bytes of a line decide whether it is a header (the header
/// pattern needs at most 32, even with multi-byte digits).
const HEADER_PREFIX_BYTES: usize = 64;

/// One whole event as it appeared in the log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawEvent {
    /// Byte offset of the event's first byte in the log file.
    pub offset: u64,
    /// Number of bytes the event occupies in the file, line endings included.
    pub len: u64,
    /// Event text with `\r\n` normalised to `\n` and trailing whitespace trimmed.
    pub text: String,
}

/// Incremental splitter. Feed it bytes as they are read from the file.
#[derive(Debug, Default)]
pub struct EventSplitter {
    buf: Vec<u8>,
    buf_offset: u64,
    /// Continuation lines dropped because no header came before them.
    dropped_orphan_lines: u64,
    /// Times an event was cut or dropped at the cap.
    capped_events: u64,
    /// The buffer starts with an event's header line.
    in_event: bool,
    /// Where the first line not yet looked at starts in the buffer: every
    /// line before it is complete, and the bytes after it hold no newline.
    scan_pos: usize,
    /// Inside a line longer than the cap: bytes are skipped up to its newline.
    skipping_line: bool,
    /// The cap, when not [`MAX_EVENT_BYTES`] (tests use a small one).
    max_event_bytes: Option<usize>,
}

impl EventSplitter {
    /// `start_offset` is the file offset of the first byte that will be pushed.
    pub fn new(start_offset: u64) -> Self {
        Self {
            buf_offset: start_offset,
            ..Self::default()
        }
    }

    /// A splitter with a smaller cap than [`MAX_EVENT_BYTES`].
    #[cfg(test)]
    fn with_max_event_bytes(start_offset: u64, max_event_bytes: usize) -> Self {
        Self {
            buf_offset: start_offset,
            max_event_bytes: Some(max_event_bytes),
            ..Self::default()
        }
    }

    /// How many continuation lines were dropped because no header came before
    /// them (a resume mid-event, a truncated file, or the rest of an event cut
    /// at the cap). Each is counted once.
    pub fn dropped_orphan_lines(&self) -> u64 {
        self.dropped_orphan_lines
    }

    /// How many times an event was cut, or a header line dropped, because it
    /// would hold more than [`MAX_EVENT_BYTES`].
    pub fn capped_events(&self) -> u64 {
        self.capped_events
    }

    /// File offset of the first byte not yet emitted as part of an event.
    pub fn committed_offset(&self) -> u64 {
        self.buf_offset
    }

    /// Append bytes and return every event now known to be complete.
    pub fn push(&mut self, bytes: &[u8]) -> Vec<RawEvent> {
        let mut events = Vec::new();
        let mut bytes = bytes;
        if self.skipping_line {
            match bytes.iter().position(|&b| b == b'\n') {
                Some(newline) => {
                    self.buf_offset += (newline + 1) as u64;
                    bytes = &bytes[newline + 1..];
                    self.skipping_line = false;
                }
                None => {
                    self.buf_offset += bytes.len() as u64;
                    return events;
                }
            }
        }

        // The bytes already buffered after `scan_pos` hold no newline, so the
        // search for the next one starts at the new bytes.
        let mut search_from = self.buf.len();
        self.buf.extend_from_slice(bytes);

        let mut current_start = if self.in_event { Some(0) } else { None };
        let mut pos = self.scan_pos;

        while let Some(newline) = self.buf[search_from..].iter().position(|&b| b == b'\n') {
            let line_end = search_from + newline + 1;
            current_start = self.add_line(pos, line_end, current_start, &mut events);
            pos = line_end;
            search_from = line_end;
        }

        // A half-written line already as long as the cap is dropped now, so
        // it is never buffered: its newline is still to come, so the whole
        // line would be dropped however it ends.
        if self.buf.len() - pos >= self.max_event_bytes() {
            let line_end_at_least = self.buf.len() + 1;
            self.add_line(pos, line_end_at_least, current_start, &mut events);
            current_start = None;
            pos = self.buf.len();
            self.skipping_line = true;
        }

        // Lines before the first header are always drained here, so a line is
        // never looked at twice.
        let cut = current_start.unwrap_or(pos);
        self.buf.drain(..cut);
        self.buf_offset += cut as u64;
        self.in_event = current_start.is_some();
        self.scan_pos = pos - cut;
        events
    }

    /// Add the line at `pos..line_end` to the event beginning at
    /// `current_start`, emitting any event it completes, and return where the
    /// event it leaves open begins.
    fn add_line(
        &mut self,
        pos: usize,
        line_end: usize,
        current_start: Option<usize>,
        events: &mut Vec<RawEvent>,
    ) -> Option<usize> {
        let max = self.max_event_bytes();
        if self.is_header_at(pos, line_end) {
            if let Some(start) = current_start {
                events.push(self.build_event(start, pos));
            }
            if line_end - pos > max {
                self.capped_events += 1;
                return None;
            }
            return Some(pos);
        }
        match current_start {
            Some(start) if line_end - start > max => {
                events.push(self.build_event(start, pos));
                self.capped_events += 1;
                self.dropped_orphan_lines += 1;
                None
            }
            Some(start) => Some(start),
            None => {
                // A continuation line with no header before it: drop it.
                self.dropped_orphan_lines += 1;
                None
            }
        }
    }

    /// Whether the line starting at `start` is a header line. Only its first
    /// bytes are read, so a huge line is not copied.
    fn is_header_at(&self, start: usize, end: usize) -> bool {
        let prefix_end = end.min(start + HEADER_PREFIX_BYTES).min(self.buf.len());
        is_header_line(&String::from_utf8_lossy(&self.buf[start..prefix_end]))
    }

    fn max_event_bytes(&self) -> usize {
        self.max_event_bytes.unwrap_or(MAX_EVENT_BYTES)
    }

    /// The event still being written, made of complete lines only.
    pub fn pending(&self) -> Option<RawEvent> {
        if !self.in_event || self.scan_pos == 0 {
            return None;
        }
        Some(self.build_event(0, self.scan_pos))
    }

    /// Commit the pending event (quiet period elapsed, or end of a sealed file).
    /// Returns `None` while a line is still half written: committing then would
    /// strand the rest of that line, which the next push would drop as an orphan.
    pub fn take_pending(&mut self) -> Option<RawEvent> {
        if !self.buf.ends_with(b"\n") {
            return None;
        }
        let event = self.pending()?;
        let cut = event.len as usize;
        self.buf.drain(..cut);
        self.buf_offset += event.len;
        self.in_event = false;
        self.scan_pos = 0;
        Some(event)
    }

    fn build_event(&self, start: usize, end: usize) -> RawEvent {
        let raw = String::from_utf8_lossy(&self.buf[start..end]);
        let text = raw
            .lines()
            .collect::<Vec<_>>()
            .join("\n")
            .trim_end()
            .to_string();
        RawEvent {
            offset: self.buf_offset + start as u64,
            len: (end - start) as u64,
            text,
        }
    }
}

/// Split a whole file at once (fixtures, tests, sealed files).
pub fn split_complete(bytes: &[u8]) -> Vec<RawEvent> {
    let mut splitter = EventSplitter::new(0);
    let mut events = splitter.push(bytes);
    if !bytes.is_empty() && !bytes.ends_with(b"\n") {
        events.extend(splitter.push(b"\n"));
    }
    if let Some(last) = splitter.take_pending() {
        events.push(last);
    }
    events
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "12:00:00 [INF] (UI|first)\ncontinued line\n";
    const B: &str = "12:00:01 [INF] (UI|second)\n";

    #[test]
    fn emits_an_event_once_the_next_header_arrives() {
        let mut splitter = EventSplitter::new(0);
        assert!(splitter.push(A.as_bytes()).is_empty());
        let events = splitter.push(B.as_bytes());
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].offset, 0);
        assert_eq!(events[0].len, A.len() as u64);
        assert_eq!(events[0].text, "12:00:00 [INF] (UI|first)\ncontinued line");
        assert_eq!(splitter.committed_offset(), A.len() as u64);
    }

    #[test]
    fn keeps_a_partial_line_until_its_newline_arrives() {
        let mut splitter = EventSplitter::new(0);
        splitter.push(A.as_bytes());
        assert!(splitter.push(b"12:00:01 [INF] (UI|sec").is_empty());
        let events = splitter.push(b"ond)\n");
        assert_eq!(events.len(), 1);
        assert_eq!(
            splitter.pending().expect("pending").text,
            "12:00:01 [INF] (UI|second)"
        );
    }

    #[test]
    fn drops_orphan_continuation_lines_before_the_first_header() {
        let mut splitter = EventSplitter::new(100);
        let input = format!("orphan tail\nmore orphan\n{A}{B}");
        let events = splitter.push(input.as_bytes());
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0].offset,
            100 + "orphan tail\nmore orphan\n".len() as u64
        );
    }

    #[test]
    fn counts_dropped_orphan_lines() {
        let mut splitter = EventSplitter::new(0);
        assert_eq!(splitter.dropped_orphan_lines(), 0);
        splitter.push(b"orphan tail\nmore orphan\n");
        assert_eq!(splitter.dropped_orphan_lines(), 2);
        splitter.push(b"half an orph");
        assert_eq!(
            splitter.dropped_orphan_lines(),
            2,
            "a half line is not yet dropped"
        );
        splitter.push(format!("an\n{A}{B}").as_bytes());
        assert_eq!(splitter.dropped_orphan_lines(), 3);
        splitter.push(b"a continuation of B\n12:00:02 [INF] (UI|third)\n");
        assert_eq!(
            splitter.dropped_orphan_lines(),
            3,
            "continuations after a header are not orphans, even when rescanned"
        );
    }

    #[test]
    fn pending_is_provisional_until_taken() {
        let mut splitter = EventSplitter::new(0);
        splitter.push(A.as_bytes());
        let pending = splitter.pending().expect("pending");
        assert_eq!(splitter.committed_offset(), 0);
        let taken = splitter.take_pending().expect("taken");
        assert_eq!(pending, taken);
        assert_eq!(splitter.committed_offset(), A.len() as u64);
        assert_eq!(splitter.pending(), None);
    }

    #[test]
    fn crlf_splits_like_lf_and_strips_carriage_returns() {
        let crlf = format!("{A}{B}").replace('\n', "\r\n");
        let events = split_complete(crlf.as_bytes());
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].text, "12:00:00 [INF] (UI|first)\ncontinued line");
        assert!(!events[1].text.contains('\r'));
        assert_eq!(events[1].offset, events[0].len);
    }

    #[test]
    fn invalid_utf8_does_not_panic_and_offsets_stay_exact() {
        let mut bytes = b"12:00:00 [INF] (UI|bad \xff\xfe bytes)\n".to_vec();
        let first_len = bytes.len() as u64;
        bytes.extend_from_slice(B.as_bytes());
        let events = split_complete(&bytes);
        assert_eq!(events.len(), 2);
        assert_eq!(events[1].offset, first_len);
    }

    #[test]
    fn take_pending_refuses_while_a_line_is_half_written() {
        let mut splitter = EventSplitter::new(0);
        splitter.push(b"12:00:00 [INF] (UI|first)\ncont one\ncont tw");
        assert_eq!(splitter.take_pending(), None);
        assert_eq!(splitter.committed_offset(), 0);

        let events = splitter.push(b"o\n12:00:01 [INF] (UI|second)\n");
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0].text,
            "12:00:00 [INF] (UI|first)\ncont one\ncont two"
        );
    }

    /// A small cap, so the cap's rules can be checked byte by byte.
    const CAP: usize = 64;

    fn capped() -> EventSplitter {
        EventSplitter::with_max_event_bytes(0, CAP)
    }

    /// Everything a splitter says after `input` arrives in `chunk` byte
    /// pushes (0 is one push), then a quiet commit.
    fn run_in_chunks(
        mut splitter: EventSplitter,
        input: &[u8],
        chunk: usize,
    ) -> (Vec<RawEvent>, u64, u64, u64, Option<RawEvent>) {
        let mut events = Vec::new();
        if chunk == 0 {
            events.extend(splitter.push(input));
        } else {
            for piece in input.chunks(chunk) {
                events.extend(splitter.push(piece));
            }
        }
        let pending = splitter.pending();
        events.extend(splitter.take_pending());
        (
            events,
            splitter.dropped_orphan_lines(),
            splitter.capped_events(),
            splitter.committed_offset(),
            pending,
        )
    }

    #[test]
    fn an_event_that_grows_past_the_cap_is_committed_and_the_rest_dropped() {
        let line = format!("{}\n", "x".repeat(20));
        let input = format!("12:00:00 [INF] (UI|big)\n{line}{line}{line}{B}");
        let (events, orphans, capped, offset, _) = run_in_chunks(capped(), input.as_bytes(), 0);
        assert_eq!(events.len(), 2);
        assert_eq!(
            events[0].text,
            format!("12:00:00 [INF] (UI|big)\n{}", "x".repeat(20))
        );
        assert_eq!(events[0].len, 24 + 21);
        assert_eq!(events[1].text, "12:00:01 [INF] (UI|second)");
        assert_eq!(events[1].offset, (24 + 21 * 3) as u64);
        assert_eq!((orphans, capped), (2, 1));
        assert_eq!(offset, input.len() as u64);
    }

    #[test]
    fn a_header_line_longer_than_the_cap_is_dropped() {
        let long = format!("12:00:00 [INF] (UI|{})\n", "y".repeat(CAP));
        let input = format!("{A}{long}its continuation\n{B}");
        let (events, orphans, capped, _, _) = run_in_chunks(capped(), input.as_bytes(), 0);
        let texts: Vec<&str> = events.iter().map(|event| event.text.as_str()).collect();
        assert_eq!(
            texts,
            vec![
                "12:00:00 [INF] (UI|first)\ncontinued line",
                "12:00:01 [INF] (UI|second)"
            ]
        );
        assert_eq!((orphans, capped), (1, 1));
    }

    #[test]
    fn a_half_written_line_past_the_cap_is_not_buffered() {
        let mut splitter = capped();
        assert!(splitter.push(A.as_bytes()).is_empty());
        let events = splitter.push("z".repeat(CAP * 4).as_bytes());
        assert_eq!(events.len(), 1, "the event it would overflow is committed");
        assert!(splitter.buf.is_empty());
        assert_eq!(splitter.committed_offset(), (A.len() + CAP * 4) as u64);
        assert!(splitter.push("z".repeat(CAP * 4).as_bytes()).is_empty());
        assert!(splitter.buf.is_empty());
        let events = splitter.push(format!("z\n{B}").as_bytes());
        assert!(events.is_empty());
        assert_eq!(
            splitter.pending().expect("pending").text,
            "12:00:01 [INF] (UI|second)"
        );
        assert_eq!(splitter.dropped_orphan_lines(), 1);
        assert_eq!(splitter.capped_events(), 1);
    }

    #[test]
    fn the_cap_gives_the_same_result_however_the_bytes_arrive() {
        let at_cap = format!("12:00:05 [INF] (UI|x{})\n", "a".repeat(CAP - 22));
        assert_eq!(at_cap.len(), CAP);
        let over_cap = format!("12:00:06 [INF] (UI|x{})\n", "a".repeat(CAP - 21));
        let long_line = format!("{}\n", "c".repeat(CAP * 2));
        let almost = format!("{}\n", "d".repeat(CAP - 1));
        let input = format!(
            "orphan\n{long_line}{A}{long_line}more\n{at_cap}{B}{almost}{over_cap}cont\n\
             12:00:07 [INF] (UI|{})\n{A}line one\nline two\nline three\n{B}{long_line}{A}tail",
            "h".repeat(CAP * 3),
        );
        let whole = run_in_chunks(capped(), input.as_bytes(), 0);
        assert!(whole.2 >= 4, "the input hits the cap several times");
        for chunk in 1..=CAP + 3 {
            assert_eq!(
                run_in_chunks(capped(), input.as_bytes(), chunk),
                whole,
                "chunk {chunk}"
            );
        }
        let mut complete = split_complete_with_cap(input.as_bytes());
        let last = complete.pop();
        assert_eq!(
            last.map(|event| event.text),
            Some("12:00:00 [INF] (UI|first)\ncontinued line\ntail".to_string())
        );
        assert_eq!(complete, whole.0);
    }

    fn split_complete_with_cap(bytes: &[u8]) -> Vec<RawEvent> {
        let mut splitter = capped();
        let mut events = splitter.push(bytes);
        events.extend(splitter.push(b"\n"));
        events.extend(splitter.take_pending());
        events
    }

    #[test]
    fn the_real_cap_bounds_an_endless_event() {
        let mut splitter = EventSplitter::new(0);
        let mut events = splitter.push(A.as_bytes());
        let line = format!("{}\n", "e".repeat(1023));
        let chunk = line.repeat(1024);
        for _ in 0..(MAX_EVENT_BYTES / chunk.len() + 2) {
            events.extend(splitter.push(chunk.as_bytes()));
            assert!(splitter.buf.len() <= MAX_EVENT_BYTES);
        }
        assert_eq!(events.len(), 1);
        assert!(events[0].len <= MAX_EVENT_BYTES as u64);
        assert_eq!(splitter.capped_events(), 1);
    }

    #[test]
    fn a_long_event_arriving_in_small_pieces_is_split_in_linear_time() {
        let mut splitter = EventSplitter::new(0);
        splitter.push(A.as_bytes());
        let line = format!("{}\n", "f".repeat(4 * 1024 * 1024));
        let started = std::time::Instant::now();
        for piece in line.as_bytes().chunks(64) {
            assert!(splitter.push(piece).is_empty());
        }
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "took {:?}",
            started.elapsed()
        );
        assert_eq!(
            splitter.pending().expect("pending").len,
            (A.len() + line.len()) as u64
        );
    }

    #[test]
    fn split_complete_includes_a_final_line_without_newline() {
        let events = split_complete(b"12:00:00 [INF] (UI|only)");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].text, "12:00:00 [INF] (UI|only)");
    }
}
