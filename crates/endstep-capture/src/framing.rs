//! Native messaging framing: a 4-byte length in native byte order, then the body.

use std::io::{Read, Write};

/// The largest body the helper accepts.
pub const MAX_MESSAGE_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FramingError {
    TooLarge(usize),
    Truncated,
    Io(String),
}

impl std::fmt::Display for FramingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FramingError::TooLarge(len) => {
                write!(
                    f,
                    "message of {len} bytes is over the {MAX_MESSAGE_BYTES} byte limit"
                )
            }
            FramingError::Truncated => write!(f, "input ended in the middle of a message"),
            FramingError::Io(message) => write!(f, "io error: {message}"),
        }
    }
}

impl std::error::Error for FramingError {}

/// Fills `buf` completely. Returns how many bytes were read (less than
/// `buf.len()` only at end of input).
fn read_up_to(input: &mut impl Read, buf: &mut [u8]) -> Result<usize, FramingError> {
    let mut filled = 0;
    while filled < buf.len() {
        match input.read(&mut buf[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(FramingError::Io(e.to_string())),
        }
    }
    Ok(filled)
}

/// Reads one message. `Ok(None)` means the input ended cleanly before a frame.
pub fn read_message(input: &mut impl Read) -> Result<Option<Vec<u8>>, FramingError> {
    let mut prefix = [0u8; 4];
    let got = read_up_to(input, &mut prefix)?;
    if got == 0 {
        return Ok(None);
    }
    if got < 4 {
        return Err(FramingError::Truncated);
    }
    let len = u32::from_ne_bytes(prefix) as usize;
    if len > MAX_MESSAGE_BYTES {
        return Err(FramingError::TooLarge(len));
    }
    let mut body = vec![0u8; len];
    let got = read_up_to(input, &mut body)?;
    if got < len {
        return Err(FramingError::Truncated);
    }
    Ok(Some(body))
}

/// Writes one message and flushes.
pub fn write_message(output: &mut impl Write, body: &[u8]) -> Result<(), FramingError> {
    if body.len() > MAX_MESSAGE_BYTES {
        return Err(FramingError::TooLarge(body.len()));
    }
    let prefix = (body.len() as u32).to_ne_bytes();
    output
        .write_all(&prefix)
        .and_then(|_| output.write_all(body))
        .and_then(|_| output.flush())
        .map_err(|e| FramingError::Io(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_message_round_trips() {
        let mut wire = Vec::new();
        write_message(&mut wire, b"hello").unwrap();
        let mut input = wire.as_slice();
        assert_eq!(read_message(&mut input), Ok(Some(b"hello".to_vec())));
        assert_eq!(read_message(&mut input), Ok(None));
    }

    #[test]
    fn a_frame_over_the_limit_is_refused() {
        let wire = ((MAX_MESSAGE_BYTES + 1) as u32).to_ne_bytes().to_vec();
        let mut input = wire.as_slice();
        assert!(matches!(
            read_message(&mut input),
            Err(FramingError::TooLarge(_))
        ));
    }

    #[test]
    fn a_short_frame_is_truncated() {
        let mut wire = 10u32.to_ne_bytes().to_vec();
        wire.extend_from_slice(b"abcd");
        let mut input = wire.as_slice();
        assert_eq!(read_message(&mut input), Err(FramingError::Truncated));
    }
}
