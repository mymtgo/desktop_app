//! MTGO format codes (0.x `App\Support\MtgoFormat`).
//!
//! MTGO prefixes constructed codes with C (`CMODERN`, `CModern`) and limited
//! codes with D (`DHOBHOBHOB`) or S and a digit. The archetypes format key
//! is the bare constructed name in lower case; limited codes have none.

/// Whether a format code is limited (0.x `MtgoMatch::isLimitedFormatCode`:
/// `^(D[A-Z]|S[0-9])`), the `decks` helper (#300 F10: one copy).
pub fn is_limited(code: &str) -> bool {
    decks::is_limited_match_format(code)
}

/// The `archetypes.format` key: `CSTANDARD`, `Standard` and `standard` all
/// give `standard` (0.x `MtgoFormat::key`). `None` for an empty code and
/// for a limited one.
pub fn format_key(code: &str) -> Option<String> {
    if code.is_empty() || is_limited(code) {
        return None;
    }
    Some(without_prefix(code).to_lowercase())
}

/// The label shown for a format code or key: `CSTANDARD` and `standard`
/// both give `Standard` (0.x `MtgoFormat::display`, `Str::title`), the
/// `decks` helper (#300 F10: one copy).
pub fn format_label(text: &str) -> String {
    decks::display_format(text)
}

/// Arena writes `ARENA_HISTORIC` (stripped, so it keys like the server's
/// `historic`). Only a C followed by another capital is an MTGO prefix:
/// "Commander" is a format name, "CModern" is a code.
fn without_prefix(code: &str) -> &str {
    if let Some(rest) = code.strip_prefix("ARENA_") {
        return rest;
    }
    let bytes = code.as_bytes();
    if bytes.len() >= 2 && bytes[0] == b'C' && bytes[1].is_ascii_uppercase() {
        &code[1..]
    } else {
        code
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_arena_brawl_codes_key_as_their_commander_archetype_formats() {
        assert_eq!(
            format_key("ARENA_HISTORIC_BRAWL").as_deref(),
            Some("historic_brawl")
        );
        assert_eq!(format_key("ARENA_BRAWL").as_deref(), Some("brawl"));
    }
}
