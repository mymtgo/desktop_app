//! The archetype format key of an MTGO format code.

/// Package `BuildReplaySnapshot::formatKey`: `CMODERN`, `CModern` and
/// `Modern` all give `modern`. Only a `C` followed by another capital is a
/// prefix, so `Commander` stays a name. `None` when there is no format or the
/// key is empty (the snapshot writes a null format then).
pub fn format_key(code: Option<&str>) -> Option<String> {
    let code = code?;
    let bytes = code.as_bytes();
    let has_prefix = bytes.len() >= 2 && bytes[0] == b'C' && bytes[1].is_ascii_uppercase();
    // `C` is one byte, so slicing after it stays on a character boundary.
    let raw = if has_prefix { &code[1..] } else { code };
    let key = raw.to_ascii_lowercase();

    if key.is_empty() { None } else { Some(key) }
}
