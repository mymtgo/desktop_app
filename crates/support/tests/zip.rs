//! The zip writer (release spec section 8.1): stored and raw deflate
//! entries with data descriptors, read back by a small reader and by
//! `unzip -t` when it is installed.

mod common;

use std::io::{Read, Write};

use chrono::NaiveDate;
use common::{CountingSink, GeneratedLog, read_zip};
use support::{DosTime, ZipError, ZipWriter};

fn local(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> chrono::NaiveDateTime {
    NaiveDate::from_ymd_opt(y, mo, d)
        .unwrap()
        .and_hms_opt(h, mi, s)
        .unwrap()
}

fn noon() -> DosTime {
    DosTime::from_local(local(2026, 10, 10, 12, 0, 0))
}

#[test]
fn dos_time_packs_the_local_time() {
    let dos = DosTime::from_local(local(2026, 10, 10, 14, 23, 5));
    assert_eq!(dos.date, ((2026 - 1980) << 9) | (10 << 5) | 10);
    assert_eq!(dos.time, (14 << 11) | (23 << 5) | 2);
    // Zip times start in 1980 and end in 2107.
    let early = DosTime::from_local(local(1970, 1, 1, 0, 0, 0));
    assert_eq!(early.date, (1 << 5) | 1);
    assert_eq!(early.time, 0);
    let late = DosTime::from_local(local(2200, 1, 1, 0, 0, 0));
    assert_eq!(late.date, (127 << 9) | (12 << 5) | 31);
}

#[test]
fn zip_round_trips_through_a_reader() {
    let text = "10/10/2026 Qplayer joined the game.\n".repeat(200);
    let mut zip = ZipWriter::new(Vec::new());
    zip.add_stored("stored.txt", noon(), text.as_bytes())
        .unwrap();
    zip.add_deflated("mtgo.log", noon(), text.as_bytes())
        .unwrap();
    zip.add_deflated("empty.log", noon(), &b""[..]).unwrap();
    zip.add_stored("Qfoe é.txt", noon(), &b"accent"[..])
        .unwrap();
    let bytes = zip.finish().unwrap();

    let entries = read_zip(&bytes);
    let names: Vec<&str> = entries.iter().map(|entry| entry.name.as_str()).collect();
    assert_eq!(names, ["stored.txt", "mtgo.log", "empty.log", "Qfoe é.txt"]);
    assert_eq!(entries[0].method, 0);
    assert_eq!(entries[1].method, 8);
    assert_eq!(entries[0].data, text.as_bytes());
    assert_eq!(entries[1].data, text.as_bytes());
    assert!(entries[2].data.is_empty());
    assert_eq!(entries[3].data, b"accent");
    for entry in &entries {
        // Bit 3: sizes and CRC follow the data; bit 11: the name is UTF-8.
        assert_eq!(entry.flags, 0x0808, "{}", entry.name);
        assert_eq!(entry.dos_date, noon().date);
        assert_eq!(entry.dos_time, noon().time);
    }
    // Deflate shrinks a repetitive log.
    assert!(bytes.len() < text.len() * 2);

    unzip_test(&bytes);
}

/// `unzip -t` reads the archive, when it is installed (skipped otherwise).
fn unzip_test(bytes: &[u8]) {
    let probe = std::process::Command::new("unzip").arg("-v").output();
    if probe.is_err() {
        eprintln!("unzip is not installed: skipped");
        return;
    }
    let path = std::env::temp_dir().join(format!("support-zip-test-{}.zip", std::process::id()));
    std::fs::write(&path, bytes).unwrap();
    let output = std::process::Command::new("unzip")
        .arg("-t")
        .arg(&path)
        .output()
        .unwrap();
    std::fs::remove_file(&path).ok();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "unzip -t failed: {stdout}");
    assert!(stdout.contains("No errors detected"), "{stdout}");
}

#[test]
fn crc_matches_known_vectors() {
    let vectors: [(&[u8], u32); 3] = [
        (b"", 0),
        (b"123456789", 0xCBF4_3926),
        (b"The quick brown fox jumps over the lazy dog", 0x414F_A339),
    ];
    for (input, expected) in vectors {
        let mut zip = ZipWriter::new(Vec::new());
        zip.add_stored("a", noon(), input).unwrap();
        zip.add_deflated("b", noon(), input).unwrap();
        let entries = read_zip(&zip.finish().unwrap());
        assert_eq!(entries[0].crc, expected);
        assert_eq!(entries[1].crc, expected);
    }
}

#[test]
fn a_large_log_streams_in_constant_memory() {
    // 300 MB in release builds; 30 MB in debug builds, where unoptimised
    // deflate takes about 10 s per 100 MB. The bound checked is the same.
    const LEN: u64 = if cfg!(debug_assertions) {
        30 * 1024 * 1024
    } else {
        300 * 1024 * 1024
    };
    let mut log = GeneratedLog::new(LEN);
    let mut zip = ZipWriter::new(CountingSink::default());
    zip.add_deflated("mtgo.log", noon(), &mut log).unwrap();
    let sink = zip.finish().unwrap();
    assert_eq!(log.remaining, 0, "the whole log was read");
    // The log goes through in small pieces: never asked for in one read,
    // never written out in one write.
    assert!(
        log.largest_read <= 64 * 1024,
        "largest read {}",
        log.largest_read
    );
    assert!(
        sink.largest_write <= 128 * 1024,
        "largest write {}",
        sink.largest_write
    );
    assert!(sink.total > 0 && sink.total < LEN);
}

/// A reader of `len` bytes that never fills the buffer (zeros from the
/// writer's own buffer): cheap enough to pass 4 GiB through.
struct Blank {
    remaining: u64,
}

impl Read for Blank {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let take = (buf.len() as u64).min(self.remaining) as usize;
        self.remaining -= take as u64;
        Ok(take)
    }
}

#[test]
fn an_entry_over_4_gib_is_an_error() {
    let mut zip = ZipWriter::new(std::io::sink());
    let reader = Blank {
        remaining: u64::from(u32::MAX) + 1,
    };
    match zip.add_stored("huge.log", noon(), reader) {
        Err(ZipError::TooLarge(name)) => assert_eq!(name, "huge.log"),
        other => panic!("expected TooLarge, got {other:?}"),
    }
}

#[test]
fn a_failing_writer_is_an_io_error() {
    struct Broken;
    impl Write for Broken {
        fn write(&mut self, _buf: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("disk full"))
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut zip = ZipWriter::new(Broken);
    assert!(matches!(
        zip.add_deflated("mtgo.log", noon(), &b"text"[..]),
        Err(ZipError::Io(_))
    ));
}
