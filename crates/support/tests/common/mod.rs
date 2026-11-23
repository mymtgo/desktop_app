//! A small zip reader for the tests: the end of central directory record,
//! the central directory, each local header, the data and its data
//! descriptor, checked against each other and against the CRC-32.

#![allow(dead_code)]

use std::io::Read;

use flate2::Crc;
use flate2::read::DeflateDecoder;

const LOCAL: u32 = 0x0403_4b50;
const CENTRAL: u32 = 0x0201_4b50;
const DESCRIPTOR: u32 = 0x0807_4b50;
const END: u32 = 0x0605_4b50;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub method: u16,
    pub flags: u16,
    pub dos_time: u16,
    pub dos_date: u16,
    pub crc: u32,
    pub data: Vec<u8>,
}

fn u16_at(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([bytes[at], bytes[at + 1]])
}

fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

/// Every entry of `zip`, in central directory order. Panics on anything a
/// reader would refuse.
pub fn read_zip(zip: &[u8]) -> Vec<Entry> {
    assert!(zip.len() >= 22, "too short for an end record");
    let end = zip.len() - 22;
    assert_eq!(
        u32_at(zip, end),
        END,
        "the end record closes the file (no comment)"
    );
    let count = u16_at(zip, end + 10) as usize;
    assert_eq!(u16_at(zip, end + 8) as usize, count, "one disk");
    let directory_size = u32_at(zip, end + 12) as usize;
    let directory_at = u32_at(zip, end + 16) as usize;
    assert_eq!(
        directory_at + directory_size,
        end,
        "the directory sits before the end record"
    );

    let mut entries = Vec::new();
    let mut at = directory_at;
    for _ in 0..count {
        assert_eq!(u32_at(zip, at), CENTRAL);
        let flags = u16_at(zip, at + 8);
        let method = u16_at(zip, at + 10);
        let dos_time = u16_at(zip, at + 12);
        let dos_date = u16_at(zip, at + 14);
        let crc = u32_at(zip, at + 16);
        let compressed = u32_at(zip, at + 20) as usize;
        let size = u32_at(zip, at + 24) as usize;
        let name_len = u16_at(zip, at + 28) as usize;
        let extra_len = u16_at(zip, at + 30) as usize;
        let comment_len = u16_at(zip, at + 32) as usize;
        let local_at = u32_at(zip, at + 42) as usize;
        let name =
            String::from_utf8(zip[at + 46..at + 46 + name_len].to_vec()).expect("UTF-8 name");
        at += 46 + name_len + extra_len + comment_len;

        assert_eq!(u32_at(zip, local_at), LOCAL);
        assert_eq!(u16_at(zip, local_at + 6), flags, "local flags");
        assert_eq!(u16_at(zip, local_at + 8), method, "local method");
        let local_name_len = u16_at(zip, local_at + 26) as usize;
        let local_extra_len = u16_at(zip, local_at + 28) as usize;
        assert_eq!(
            &zip[local_at + 30..local_at + 30 + local_name_len],
            name.as_bytes()
        );
        let data_at = local_at + 30 + local_name_len + local_extra_len;
        let raw = &zip[data_at..data_at + compressed];
        let descriptor_at = data_at + compressed;
        assert_eq!(u32_at(zip, descriptor_at), DESCRIPTOR, "data descriptor");
        assert_eq!(u32_at(zip, descriptor_at + 4), crc);
        assert_eq!(u32_at(zip, descriptor_at + 8) as usize, compressed);
        assert_eq!(u32_at(zip, descriptor_at + 12) as usize, size);

        let data = match method {
            0 => raw.to_vec(),
            8 => {
                let mut out = Vec::new();
                DeflateDecoder::new(raw)
                    .read_to_end(&mut out)
                    .expect("inflate");
                out
            }
            other => panic!("unknown method {other}"),
        };
        assert_eq!(data.len(), size, "size of {name}");
        let mut check = Crc::new();
        check.update(&data);
        assert_eq!(check.sum(), crc, "CRC of {name}");
        entries.push(Entry {
            name,
            method,
            flags,
            dos_time,
            dos_date,
            crc,
            data,
        });
    }
    assert_eq!(at, directory_at + directory_size);
    entries
}

/// A reader of `len` bytes of generated MTGO-like log lines, never held in
/// memory: one 1 MiB block of varied lines, repeated (further apart than
/// deflate's 32 KiB window, so it still compresses as a real log does).
/// Records the largest buffer it was asked to fill.
pub struct GeneratedLog {
    pub remaining: u64,
    pub largest_read: usize,
    block: Vec<u8>,
    at: usize,
}

impl GeneratedLog {
    pub fn new(len: u64) -> Self {
        let mut block = Vec::with_capacity(1024 * 1024 + 128);
        let mut seed: u64 = 1;
        while block.len() < 1024 * 1024 {
            // A small linear congruential generator: varied, repeatable text.
            seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
            let n = seed >> 33;
            let line = format!(
                "10/10/2026 14:{:02}:{:02} [{}] Qplayer casts card {} targeting {}\n",
                n % 60,
                (n / 60) % 60,
                n % 7,
                n % 100_000,
                n % 13
            );
            block.extend_from_slice(line.as_bytes());
        }
        Self {
            remaining: len,
            largest_read: 0,
            block,
            at: 0,
        }
    }
}

impl Read for GeneratedLog {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.largest_read = self.largest_read.max(buf.len());
        let mut written = 0;
        while written < buf.len() && self.remaining > 0 {
            if self.at >= self.block.len() {
                self.at = 0;
            }
            let take = (self.block.len() - self.at)
                .min(buf.len() - written)
                .min(self.remaining as usize);
            buf[written..written + take].copy_from_slice(&self.block[self.at..self.at + take]);
            self.at += take;
            written += take;
            self.remaining -= take as u64;
        }
        Ok(written)
    }
}

/// A writer that keeps nothing: counts bytes and the largest single write.
#[derive(Default)]
pub struct CountingSink {
    pub total: u64,
    pub largest_write: usize,
}

impl std::io::Write for CountingSink {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.total += buf.len() as u64;
        self.largest_write = self.largest_write.max(buf.len());
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
