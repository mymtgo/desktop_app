//! Reads new bytes from `Player.log` and splits them into whole entries. An
//! entry still being written (a half written line or JSON block) stays in the
//! splitter and comes out whole once the rest arrives; the cursor to keep is
//! the splitter's resume offset, never the bytes read.

use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};

use arena_protocol::{ArenaSplitter, RawEntry};

/// Largest single read; a catch-up continues on the next poll (as MTGO's).
pub const MAX_READ: u64 = 8 * 1024 * 1024;
/// How much of the start of a file the header date order is detected from.
pub const DAY_ORDER_BYTES: u64 = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArenaRead {
    /// Complete entries, in file order.
    pub entries: Vec<RawEntry>,
    /// Where a restart must start reading to see every entry not returned
    /// yet: store this with the entries.
    pub cursor: u64,
    /// Everything up to the given size has been read.
    pub at_eof: bool,
}

#[derive(Debug)]
pub struct ArenaTailer {
    path: PathBuf,
    splitter: ArenaSplitter,
    read_pos: u64,
}

impl ArenaTailer {
    /// Start reading `path` at `cursor`.
    pub fn new(path: PathBuf, cursor: u64) -> Self {
        Self {
            path,
            splitter: ArenaSplitter::new(cursor),
            read_pos: cursor,
        }
    }

    /// The resume offset (see [`ArenaRead::cursor`]).
    pub fn cursor(&self) -> u64 {
        self.splitter.resume_offset()
    }

    /// Read what was appended up to `size` (at most `MAX_READ`).
    pub fn poll(&mut self, size: u64) -> io::Result<ArenaRead> {
        let mut entries = Vec::new();
        if size > self.read_pos {
            let bytes = read_range(
                &self.path,
                self.read_pos,
                (size - self.read_pos).min(MAX_READ),
            )?;
            self.read_pos += bytes.len() as u64;
            entries = self.splitter.push(&bytes);
        }
        Ok(ArenaRead {
            entries,
            cursor: self.cursor(),
            at_eof: self.read_pos >= size,
        })
    }
}

/// Up to the first `len` bytes of the file (the header date order is
/// detected from them).
pub fn read_start(path: &Path, len: u64) -> io::Result<Vec<u8>> {
    read_range(path, 0, len)
}

/// Open `path` and read up to `len` bytes from `offset` (fewer at the end of
/// the file).
fn read_range(path: &Path, offset: u64, len: u64) -> io::Result<Vec<u8>> {
    let mut file = File::open(path)?;
    mtgo_log::identity::read_range(&mut file, offset, len)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fixture_tools::TestDir;

    const LOGIN: &str = "[Accounts - Login] Logged in successfully. Display Name: Player#00001\n";
    const RANK: &str = "[UnityCrossThreadLogger]03/11/2026 12:33:02\n<== RankGetCombinedRankInfo(abc)\n{\"constructedSeasonOrdinal\":95}\n";
    const BLOCK_START: &str = "[UnityCrossThreadLogger]ClientToMatchServiceMessageType_ClientToGREMessage\n{\n  \"a\": 1,\n";
    const BLOCK_END: &str = "  \"b\": 2\n}\n";

    fn size(path: &Path) -> u64 {
        std::fs::metadata(path).expect("metadata").len()
    }

    #[test]
    fn tailer_resumes_at_its_cursor() {
        let dir = TestDir::new("arena-tail-resume");
        let path = dir.write("Player.log", LOGIN.as_bytes());
        let mut tailer = ArenaTailer::new(path.clone(), 0);
        let first = tailer.poll(size(&path)).expect("poll");
        assert_eq!(first.entries.len(), 1);
        assert_eq!(first.cursor, LOGIN.len() as u64);
        assert!(first.at_eof);

        dir.append("Player.log", RANK.as_bytes());
        let second = tailer.poll(size(&path)).expect("poll");
        assert_eq!(second.entries.len(), 1, "only the new entry");
        assert_eq!(second.entries[0].offset, LOGIN.len() as u64);

        // A new tailer at the stored cursor sees nothing again.
        let mut resumed = ArenaTailer::new(path.clone(), second.cursor);
        assert!(resumed.poll(size(&path)).expect("poll").entries.is_empty());
    }

    #[test]
    fn a_half_written_block_waits_and_the_cursor_stays_at_its_start() {
        let dir = TestDir::new("arena-tail-half");
        let path = dir.write("Player.log", format!("{LOGIN}{BLOCK_START}").as_bytes());
        let mut tailer = ArenaTailer::new(path.clone(), 0);
        let read = tailer.poll(size(&path)).expect("poll");
        assert_eq!(read.entries.len(), 1);
        assert_eq!(read.cursor, LOGIN.len() as u64, "the block is not read yet");

        dir.append("Player.log", BLOCK_END.as_bytes());
        let read = tailer.poll(size(&path)).expect("poll");
        assert_eq!(read.entries.len(), 1);
        assert_eq!(read.entries[0].offset, LOGIN.len() as u64);
        assert_eq!(read.cursor, size(&path));
    }

    #[test]
    fn a_large_backlog_is_read_in_steps() {
        let dir = TestDir::new("arena-tail-steps");
        let line = LOGIN.repeat((MAX_READ as usize / LOGIN.len()) + 10);
        let path = dir.write("Player.log", line.as_bytes());
        let mut tailer = ArenaTailer::new(path.clone(), 0);
        let first = tailer.poll(size(&path)).expect("poll");
        assert!(!first.at_eof);
        let second = tailer.poll(size(&path)).expect("poll");
        assert!(second.at_eof);
        assert_eq!(second.cursor, size(&path));
    }

    #[test]
    fn read_start_reads_at_most_the_asked_bytes() {
        let dir = TestDir::new("arena-tail-start");
        let path = dir.write("Player.log", LOGIN.as_bytes());
        assert_eq!(
            read_start(&path, 5).expect("read"),
            LOGIN.as_bytes()[..5].to_vec()
        );
        assert_eq!(read_start(&path, 1_000).expect("read").len(), LOGIN.len());
    }
}
