//! Splits `Player.log` bytes into entries. The log is line based, but an
//! entry can span lines:
//!
//! - A `[UnityCrossThreadLogger]` line whose text ends a JSON object is one
//!   entry, its JSON taken from the first `{`.
//! - A `[UnityCrossThreadLogger]` line with no JSON (a GRE or client message
//!   header) takes the JSON block that starts on the next line as its body.
//! - A `<== Name(id)` line (an API response) takes the JSON block on the next
//!   line. The date-only header line just before it is kept in the head, as it
//!   holds the response's only time.
//! - A JSON block starts at a line whose first byte is `{` and ends when its
//!   braces balance outside strings. Client to server messages are pretty
//!   printed over many lines.
//! - `[Accounts - Login]` and `DETAILED LOGS:` lines are entries with no JSON.
//! - Everything else (Unity stack traces, engine noise) is skipped.
//!
//! Only whole lines are looked at, so an entry whose last line is still being
//! written is held back and comes out whole, at its first byte's offset, once
//! the rest arrives. [`ArenaSplitter::resume_offset`] says where a new
//! splitter must start to see the same entries after a restart.

/// The most bytes one entry may hold. A line or block that would pass it is
/// dropped, so a corrupt log cannot make the splitter buffer without bound
/// (the largest real line seen is about 1 MB).
pub const MAX_ENTRY_BYTES: usize = 16 * 1024 * 1024;

const UNITY_PREFIX: &str = "[UnityCrossThreadLogger]";
const LOGIN_PREFIX: &str = "[Accounts - Login]";
const DETAILED_LOGS_PREFIX: &str = "DETAILED LOGS:";
const RESPONSE_PREFIX: &str = "<== ";

/// One entry as it appeared in the log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawEntry {
    /// Byte offset of the entry's first byte in the log file.
    pub offset: u64,
    /// Bytes the entry occupies in the file, its last line ending included.
    pub len: u64,
    /// The entry's first line: a header (cut before its JSON when the JSON is
    /// on the same line), a `==>` request line, a login or detailed logs line,
    /// or empty for a bare JSON block. For an API response it is the date
    /// header line and the `<== Name(id)` line joined by `\n`.
    pub head: String,
    /// The JSON body, when the entry has one.
    pub json: Option<String>,
}

/// A head waiting for the JSON block that must start on the next line.
#[derive(Debug, Clone)]
struct Waiting {
    offset: u64,
    head: String,
    /// A `[UnityCrossThreadLogger]` line holding only a date and time.
    date_only: bool,
}

/// A JSON block whose closing brace has not been seen yet.
#[derive(Debug, Clone)]
struct OpenBlock {
    offset: u64,
    head: String,
    text: String,
    scan: BraceScan,
}

/// Brace depth outside JSON strings.
#[derive(Debug, Clone, Default)]
struct BraceScan {
    depth: u32,
    in_string: bool,
    escaped: bool,
}

impl BraceScan {
    /// Reads `text`; returns the index just past the brace that closes the
    /// outermost object, if `text` holds it.
    fn feed(&mut self, text: &str) -> Option<usize> {
        for (index, byte) in text.bytes().enumerate() {
            if self.in_string {
                if self.escaped {
                    self.escaped = false;
                } else if byte == b'\\' {
                    self.escaped = true;
                } else if byte == b'"' {
                    self.in_string = false;
                }
                continue;
            }
            match byte {
                b'"' => self.in_string = true,
                b'{' => self.depth += 1,
                b'}' if self.depth > 0 => {
                    self.depth -= 1;
                    if self.depth == 0 {
                        return Some(index + 1);
                    }
                }
                _ => {}
            }
        }
        None
    }
}

/// Incremental splitter. Feed it bytes as they are read from the file.
#[derive(Debug)]
pub struct ArenaSplitter {
    /// Bytes of the line not finished yet (no newline in them).
    buf: Vec<u8>,
    /// File offset of `buf[0]`.
    buf_offset: u64,
    /// Inside a line longer than [`MAX_ENTRY_BYTES`]: bytes are skipped up to
    /// its newline.
    skipping_line: bool,
    waiting: Option<Waiting>,
    block: Option<OpenBlock>,
}

impl ArenaSplitter {
    /// A splitter for bytes that start at `start_offset` in the file.
    pub fn new(start_offset: u64) -> Self {
        ArenaSplitter {
            buf: Vec::new(),
            buf_offset: start_offset,
            skipping_line: false,
            waiting: None,
            block: None,
        }
    }

    /// Reads more bytes and returns every entry they complete.
    pub fn push(&mut self, bytes: &[u8]) -> Vec<RawEntry> {
        let mut entries = Vec::new();
        let mut rest = bytes;
        while !rest.is_empty() {
            match rest.iter().position(|byte| *byte == b'\n') {
                Some(newline) => {
                    let (line, after) = rest.split_at(newline + 1);
                    self.end_line(line, &mut entries);
                    rest = after;
                }
                None => {
                    self.add_to_line(rest);
                    rest = &[];
                }
            }
        }
        entries
    }

    /// Where a new splitter must start to produce the entries this one has not
    /// produced yet: the start of an open block or waiting head, otherwise the
    /// start of the unfinished line.
    pub fn resume_offset(&self) -> u64 {
        if let Some(block) = &self.block {
            return block.offset;
        }
        if let Some(waiting) = &self.waiting {
            return waiting.offset;
        }
        self.buf_offset
    }

    /// Reads the last line when the file does not end with a newline. Used
    /// only when the whole file is known to be complete.
    fn finish(&mut self) -> Vec<RawEntry> {
        let mut entries = Vec::new();
        if !self.buf.is_empty() && !self.skipping_line {
            let line = std::mem::take(&mut self.buf);
            let offset = self.buf_offset;
            self.buf_offset += line.len() as u64;
            self.read_line(offset, &line, &mut entries);
        }
        entries
    }

    /// Adds bytes of a line whose newline has not arrived yet.
    fn add_to_line(&mut self, bytes: &[u8]) {
        if self.skipping_line {
            self.buf_offset += bytes.len() as u64;
            return;
        }
        self.buf.extend_from_slice(bytes);
        if self.buf.len() > MAX_ENTRY_BYTES {
            self.buf_offset += self.buf.len() as u64;
            self.buf.clear();
            self.skipping_line = true;
            self.waiting = None;
            self.block = None;
        }
    }

    /// Ends the current line with `bytes` (the newline included).
    fn end_line(&mut self, bytes: &[u8], entries: &mut Vec<RawEntry>) {
        if self.skipping_line {
            self.buf_offset += bytes.len() as u64;
            self.skipping_line = false;
            return;
        }
        let line = if self.buf.is_empty() {
            bytes.to_vec()
        } else {
            let mut line = std::mem::take(&mut self.buf);
            line.extend_from_slice(bytes);
            line
        };
        let offset = self.buf_offset;
        self.buf_offset += line.len() as u64;
        self.read_line(offset, &line, entries);
    }

    /// Reads one whole line that starts at `offset`.
    fn read_line(&mut self, offset: u64, line: &[u8], entries: &mut Vec<RawEntry>) {
        let end = offset + line.len() as u64;
        let text = String::from_utf8_lossy(line);
        let text = text.trim_end_matches(['\n', '\r']);

        // A block cut short (never closed) is dropped, and this line is read
        // as a new start.
        if let Some(block) = self.block.take()
            && continues_block(text)
        {
            self.continue_block(block, text, end, entries);
            return;
        }

        let waiting = self.waiting.take();
        if text.starts_with('{') {
            let (start, head) = match waiting {
                Some(waiting) => (waiting.offset, waiting.head),
                None => (offset, String::new()),
            };
            let block = OpenBlock {
                offset: start,
                head,
                text: String::new(),
                scan: BraceScan::default(),
            };
            self.continue_block(block, text, end, entries);
        } else if text.starts_with(RESPONSE_PREFIX) {
            let (start, head) = match waiting {
                Some(waiting) if waiting.date_only => {
                    (waiting.offset, format!("{}\n{}", waiting.head, text))
                }
                _ => (offset, text.to_string()),
            };
            self.waiting = Some(Waiting {
                offset: start,
                head,
                date_only: false,
            });
        } else if text.starts_with(LOGIN_PREFIX) || text.starts_with(DETAILED_LOGS_PREFIX) {
            entries.push(RawEntry {
                offset,
                len: end - offset,
                head: text.to_string(),
                json: None,
            });
        } else if text.starts_with(UNITY_PREFIX) {
            self.read_unity_line(offset, end, text, entries);
        }
        // Anything else is noise between entries.
    }

    /// A `[UnityCrossThreadLogger]` line: an entry when it ends a JSON object,
    /// otherwise a head waiting for the next line's block.
    fn read_unity_line(&mut self, offset: u64, end: u64, text: &str, entries: &mut Vec<RawEntry>) {
        if let Some(brace) = text.find('{') {
            let mut scan = BraceScan::default();
            if let Some(close) = scan.feed(&text[brace..]) {
                entries.push(RawEntry {
                    offset,
                    len: end - offset,
                    head: text[..brace].trim_end().to_string(),
                    json: Some(text[brace..brace + close].to_string()),
                });
                return;
            }
        }
        self.waiting = Some(Waiting {
            offset,
            head: text.to_string(),
            date_only: crate::clock::is_date_only_header(text),
        });
    }

    /// Adds `text` (one line) to an open block; emits the entry when the
    /// block closes on this line.
    fn continue_block(
        &mut self,
        mut block: OpenBlock,
        text: &str,
        end: u64,
        entries: &mut Vec<RawEntry>,
    ) {
        if let Some(close) = block.scan.feed(text) {
            block.text.push_str(&text[..close]);
            entries.push(RawEntry {
                offset: block.offset,
                len: end - block.offset,
                head: block.head,
                json: Some(block.text),
            });
            return;
        }
        block.text.push_str(text);
        block.text.push('\n');
        if block.text.len() <= MAX_ENTRY_BYTES {
            self.block = Some(block);
        }
    }
}

/// Whether a line can be inside a pretty printed JSON block: nested lines are
/// indented, and only the closing brace sits at the line start.
fn continues_block(text: &str) -> bool {
    text.starts_with([' ', '\t', '}', ']'])
}

/// Splits a whole, finished file.
pub fn split_complete(bytes: &[u8]) -> Vec<RawEntry> {
    let mut splitter = ArenaSplitter::new(0);
    let mut entries = splitter.push(bytes);
    entries.extend(splitter.finish());
    entries
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_bytes() -> Vec<u8> {
        fixture_tools::arena_fixture("bo3-loss-and-match-concede").bytes
    }

    #[test]
    fn splitter_pairs_a_response_with_its_json_line() {
        let entries = split_complete(b"<== RankGetCombinedRankInfo(x)\n{\"constructedLevel\":4}\n");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].head, "<== RankGetCombinedRankInfo(x)");
        assert_eq!(entries[0].json.as_deref(), Some("{\"constructedLevel\":4}"));
        assert_eq!(entries[0].offset, 0);
    }

    #[test]
    fn splitter_reads_a_multi_line_client_block() {
        let text = "noise\n{\n  \"payload\": {\n    \"type\": \"x\"\n  }\n}\n";
        let entries = split_complete(text.as_bytes());
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].head, "");
        assert_eq!(entries[0].offset, 6);
        assert_eq!(entries[0].len, text.len() as u64 - 6);
        let json: serde_json::Value =
            serde_json::from_str(entries[0].json.as_deref().unwrap()).unwrap();
        assert_eq!(json["payload"]["type"], "x");
    }

    #[test]
    fn splitter_skips_unity_stack_lines() {
        let text = "UnityEngine.DebugLogHandler:Internal_Log(LogType, LogOption, String, Object)\n\
                    MatchManager:Update()\nPAPA:Update()\n\n";
        assert!(split_complete(text.as_bytes()).is_empty());
    }

    #[test]
    fn splitter_holds_back_an_unfinished_json_block() {
        let first = "[UnityCrossThreadLogger]03/11/2026 12:32:44: P to Match: ClientToGremessage\n{\n  \"a\": {\n";
        let rest = "    \"b\": \"}\"\n  }\n}\nPAPA:Update()\n";
        let mut splitter = ArenaSplitter::new(100);
        assert!(splitter.push(first.as_bytes()).is_empty());
        assert_eq!(splitter.resume_offset(), 100);
        let entries = splitter.push(rest.as_bytes());
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].offset, 100);
        assert!(entries[0].head.ends_with("ClientToGremessage"));
        let json: serde_json::Value =
            serde_json::from_str(entries[0].json.as_deref().unwrap()).unwrap();
        assert_eq!(json["a"]["b"], "}");
        assert_eq!(
            entries[0].len,
            (first.len() + rest.len() - "PAPA:Update()\n".len()) as u64
        );
    }

    #[test]
    fn splitter_keeps_a_one_line_header_json() {
        let text = "[UnityCrossThreadLogger]==> EventJoin {\"id\":\"1\",\"request\":\"{}\"}\n";
        let entries = split_complete(text.as_bytes());
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].head, "[UnityCrossThreadLogger]==> EventJoin");
        assert_eq!(
            entries[0].json.as_deref(),
            Some("{\"id\":\"1\",\"request\":\"{}\"}")
        );
    }

    #[test]
    fn splitter_keeps_the_date_header_of_a_response() {
        let text =
            "[UnityCrossThreadLogger]03/11/2026 12:28:25\n<== RankGetCombinedRankInfo(x)\n{}\n";
        let entries = split_complete(text.as_bytes());
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].offset, 0);
        assert_eq!(
            entries[0].head,
            "[UnityCrossThreadLogger]03/11/2026 12:28:25\n<== RankGetCombinedRankInfo(x)"
        );
    }

    #[test]
    fn splitter_drops_a_block_cut_short_by_a_new_entry() {
        let text = "{\n  \"a\": 1,\n[Accounts - Login] Logged in successfully. Display Name: P#1\n";
        let entries = split_complete(text.as_bytes());
        assert_eq!(entries.len(), 1);
        assert!(entries[0].head.starts_with("[Accounts - Login]"));
        assert_eq!(entries[0].json, None);
    }

    #[test]
    fn splitter_reads_windows_line_endings() {
        let text = "<== Name(x)\r\n{\"a\":1}\r\n";
        let entries = split_complete(text.as_bytes());
        assert_eq!(entries[0].head, "<== Name(x)");
        assert_eq!(entries[0].json.as_deref(), Some("{\"a\":1}"));
        assert_eq!(entries[0].len, text.len() as u64);
    }

    #[test]
    fn split_complete_equals_pushing_byte_by_byte() {
        let bytes = fixture_bytes();
        let whole = split_complete(&bytes);
        let mut splitter = ArenaSplitter::new(0);
        let mut pieces = Vec::new();
        for byte in &bytes {
            pieces.extend(splitter.push(std::slice::from_ref(byte)));
        }
        assert_eq!(pieces, whole);
        assert!(whole.len() > 1000);
    }

    #[test]
    fn every_fixture_json_body_parses() {
        for entry in split_complete(&fixture_bytes()) {
            if let Some(json) = &entry.json {
                let parsed: Result<serde_json::Value, _> = serde_json::from_str(json);
                assert!(parsed.is_ok(), "entry at {} does not parse", entry.offset);
            }
        }
    }
}
