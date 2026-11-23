//! A streaming zip writer over any `std::io::Write` (release spec section
//! 3 and 8.1). Each entry is stored or raw deflate (through `flate2`), its
//! CRC-32 comes from `flate2::Crc`, and its sizes and CRC follow the data in
//! a data descriptor (general purpose bit 3), so an entry is written as it is
//! read and a 200 MB log never sits in memory. Names are UTF-8 (bit 11).
//! There is no zip64: an entry, or the archive, past 4 GiB is an error.
//!
//! Format: PKWARE's APPNOTE.TXT, sections 4.3 (structure) and 4.4 (fields).

use std::fmt;
use std::io::{Read, Write};

use chrono::{Datelike, NaiveDateTime, Timelike};
use flate2::Compression;
use flate2::Crc;
use flate2::write::DeflateEncoder;

const LOCAL_HEADER: u32 = 0x0403_4b50;
const DATA_DESCRIPTOR: u32 = 0x0807_4b50;
const CENTRAL_HEADER: u32 = 0x0201_4b50;
const END_OF_DIRECTORY: u32 = 0x0605_4b50;

/// Version 2.0: deflate and data descriptors (APPNOTE 4.4.3).
const VERSION: u16 = 20;
/// Bit 3 (sizes and CRC in the data descriptor) and bit 11 (UTF-8 name).
const FLAGS: u16 = 0x0008 | 0x0800;
const METHOD_STORED: u16 = 0;
const METHOD_DEFLATED: u16 = 8;
/// Deflate level 6 (spec 8.1), zlib's default.
const DEFLATE_LEVEL: u32 = 6;
/// The read buffer: entries stream through this many bytes at a time.
const CHUNK: usize = 64 * 1024;
/// The largest size or offset a zip without zip64 can record.
const LIMIT: u64 = u32::MAX as u64;
/// The most entries a zip without zip64 can hold.
const MAX_ENTRIES: usize = u16::MAX as usize;

/// A local date and time in the MS-DOS form zip headers use: two-second
/// steps, years 1980 to 2107. The caller passes the local time in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DosTime {
    pub date: u16,
    pub time: u16,
}

impl DosTime {
    /// Pack a local time. Times before 1980 become 1980-01-01 00:00:00 and
    /// times after 2107 become 2107-12-31 23:59:58, the format's limits.
    pub fn from_local(local: NaiveDateTime) -> DosTime {
        let year = local.year();
        if year < 1980 {
            return DosTime {
                date: (1 << 5) | 1,
                time: 0,
            };
        }
        if year > 2107 {
            return DosTime {
                date: (127 << 9) | (12 << 5) | 31,
                time: (23 << 11) | (59 << 5) | 29,
            };
        }
        let date = ((year - 1980) as u16) << 9 | (local.month() as u16) << 5 | local.day() as u16;
        let time = (local.hour() as u16) << 11
            | (local.minute() as u16) << 5
            | (local.second().min(59) as u16 / 2);
        DosTime { date, time }
    }
}

/// Why an archive could not be written.
#[derive(Debug)]
pub enum ZipError {
    /// Reading an entry or writing the archive failed.
    Io(std::io::Error),
    /// This entry (or the archive up to it) is past 4 GiB, which needs zip64.
    TooLarge(String),
    /// More than 65,535 entries, which needs zip64.
    TooManyEntries,
}

impl fmt::Display for ZipError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ZipError::Io(error) => write!(f, "{error}"),
            ZipError::TooLarge(name) => {
                write!(f, "{name} is too large for a zip file (over 4 GiB)")
            }
            ZipError::TooManyEntries => write!(f, "too many files for a zip file"),
        }
    }
}

impl std::error::Error for ZipError {}

impl From<std::io::Error> for ZipError {
    fn from(error: std::io::Error) -> Self {
        ZipError::Io(error)
    }
}

/// What the central directory records of one written entry.
struct Written {
    name: String,
    method: u16,
    dos_time: DosTime,
    crc: u32,
    compressed: u32,
    size: u32,
    offset: u32,
}

/// Writes a zip archive entry by entry; `finish` writes the central
/// directory and hands the writer back.
pub struct ZipWriter<W: Write> {
    out: W,
    /// Bytes written so far: the next local header's offset.
    offset: u64,
    entries: Vec<Written>,
}

impl<W: Write> ZipWriter<W> {
    pub fn new(out: W) -> Self {
        Self {
            out,
            offset: 0,
            entries: Vec::new(),
        }
    }

    /// Add an entry whose bytes are copied as they are.
    pub fn add_stored<R: Read>(
        &mut self,
        name: &str,
        dos_time: DosTime,
        reader: R,
    ) -> Result<(), ZipError> {
        self.add(name, dos_time, reader, METHOD_STORED)
    }

    /// Add an entry compressed with raw deflate.
    pub fn add_deflated<R: Read>(
        &mut self,
        name: &str,
        dos_time: DosTime,
        reader: R,
    ) -> Result<(), ZipError> {
        self.add(name, dos_time, reader, METHOD_DEFLATED)
    }

    fn add<R: Read>(
        &mut self,
        name: &str,
        dos_time: DosTime,
        mut reader: R,
        method: u16,
    ) -> Result<(), ZipError> {
        if self.entries.len() >= MAX_ENTRIES {
            return Err(ZipError::TooManyEntries);
        }
        if self.offset > LIMIT || name.len() > usize::from(u16::MAX) {
            return Err(ZipError::TooLarge(name.to_string()));
        }
        let offset = self.offset as u32;

        let mut header = Vec::with_capacity(30 + name.len());
        put_u32(&mut header, LOCAL_HEADER);
        put_u16(&mut header, VERSION);
        put_u16(&mut header, FLAGS);
        put_u16(&mut header, method);
        put_u16(&mut header, dos_time.time);
        put_u16(&mut header, dos_time.date);
        // CRC and sizes: zero here, in the data descriptor after the data.
        put_u32(&mut header, 0);
        put_u32(&mut header, 0);
        put_u32(&mut header, 0);
        put_u16(&mut header, name.len() as u16);
        put_u16(&mut header, 0);
        header.extend_from_slice(name.as_bytes());
        self.out.write_all(&header)?;
        self.offset += header.len() as u64;

        let streamed = match method {
            METHOD_STORED => copy_stored(name, &mut reader, &mut self.out)?,
            _ => copy_deflated(name, &mut reader, &mut self.out)?,
        };
        self.offset += streamed.compressed;

        let mut descriptor = Vec::with_capacity(16);
        put_u32(&mut descriptor, DATA_DESCRIPTOR);
        put_u32(&mut descriptor, streamed.crc);
        put_u32(&mut descriptor, streamed.compressed as u32);
        put_u32(&mut descriptor, streamed.size as u32);
        self.out.write_all(&descriptor)?;
        self.offset += descriptor.len() as u64;

        self.entries.push(Written {
            name: name.to_string(),
            method,
            dos_time,
            crc: streamed.crc,
            compressed: streamed.compressed as u32,
            size: streamed.size as u32,
            offset,
        });
        Ok(())
    }

    /// Write the central directory and the end record; returns the writer,
    /// flushed.
    pub fn finish(mut self) -> Result<W, ZipError> {
        let directory_at = self.offset;
        let mut directory = Vec::new();
        for entry in &self.entries {
            put_u32(&mut directory, CENTRAL_HEADER);
            // Made by: MS-DOS attributes (upper byte 0), version 2.0.
            put_u16(&mut directory, VERSION);
            put_u16(&mut directory, VERSION);
            put_u16(&mut directory, FLAGS);
            put_u16(&mut directory, entry.method);
            put_u16(&mut directory, entry.dos_time.time);
            put_u16(&mut directory, entry.dos_time.date);
            put_u32(&mut directory, entry.crc);
            put_u32(&mut directory, entry.compressed);
            put_u32(&mut directory, entry.size);
            put_u16(&mut directory, entry.name.len() as u16);
            put_u16(&mut directory, 0); // extra field
            put_u16(&mut directory, 0); // comment
            put_u16(&mut directory, 0); // disk number
            put_u16(&mut directory, 0); // internal attributes
            put_u32(&mut directory, 0); // external attributes
            put_u32(&mut directory, entry.offset);
            directory.extend_from_slice(entry.name.as_bytes());
        }
        let directory_size = directory.len() as u64;
        if directory_at > LIMIT || directory_size > LIMIT || directory_at + directory_size > LIMIT {
            return Err(ZipError::TooLarge("the zip file".to_string()));
        }
        let count = self.entries.len() as u16;
        put_u32(&mut directory, END_OF_DIRECTORY);
        put_u16(&mut directory, 0); // this disk
        put_u16(&mut directory, 0); // disk with the directory
        put_u16(&mut directory, count);
        put_u16(&mut directory, count);
        put_u32(&mut directory, directory_size as u32);
        put_u32(&mut directory, directory_at as u32);
        put_u16(&mut directory, 0); // comment
        self.out.write_all(&directory)?;
        self.out.flush()?;
        Ok(self.out)
    }
}

/// One entry's data as written: CRC-32, bytes in, bytes out.
struct Streamed {
    crc: u32,
    size: u64,
    compressed: u64,
}

/// Read `reader` in chunks, updating the CRC and checking the size before
/// each chunk goes to `sink`.
fn pump<R: Read>(
    name: &str,
    reader: &mut R,
    mut sink: impl FnMut(&[u8]) -> std::io::Result<()>,
) -> Result<(u32, u64), ZipError> {
    let mut buffer = vec![0u8; CHUNK];
    let mut crc = Crc::new();
    let mut size: u64 = 0;
    loop {
        let read = match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => read,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(ZipError::Io(error)),
        };
        size += read as u64;
        if size > LIMIT {
            return Err(ZipError::TooLarge(name.to_string()));
        }
        crc.update(&buffer[..read]);
        sink(&buffer[..read])?;
    }
    Ok((crc.sum(), size))
}

fn copy_stored<R: Read, W: Write>(
    name: &str,
    reader: &mut R,
    out: &mut W,
) -> Result<Streamed, ZipError> {
    let (crc, size) = pump(name, reader, |chunk| out.write_all(chunk))?;
    Ok(Streamed {
        crc,
        size,
        compressed: size,
    })
}

fn copy_deflated<R: Read, W: Write>(
    name: &str,
    reader: &mut R,
    out: &mut W,
) -> Result<Streamed, ZipError> {
    let counter = Counting {
        inner: out,
        count: 0,
    };
    let mut encoder = DeflateEncoder::new(counter, Compression::new(DEFLATE_LEVEL));
    let (crc, size) = pump(name, reader, |chunk| encoder.write_all(chunk))?;
    let counter = encoder.finish()?;
    if counter.count > LIMIT {
        return Err(ZipError::TooLarge(name.to_string()));
    }
    Ok(Streamed {
        crc,
        size,
        compressed: counter.count,
    })
}

/// Passes bytes on and counts them (the compressed size).
struct Counting<'a, W: Write> {
    inner: &'a mut W,
    count: u64,
}

impl<W: Write> Write for Counting<'_, W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let written = self.inner.write(buf)?;
        self.count += written as u64;
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

fn put_u16(bytes: &mut Vec<u8>, value: u16) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

fn put_u32(bytes: &mut Vec<u8>, value: u32) {
    bytes.extend_from_slice(&value.to_le_bytes());
}
