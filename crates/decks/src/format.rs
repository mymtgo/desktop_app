//! Format labels (0.x `MtgoFormat::display`) and the Limited check used to
//! leave draft decks out of the deck list.

use mtgo_match::Outcome;

use crate::stats::Record;

/// The human label of a format code. MTGO prefixes constructed codes with `C`
/// (`CMODERN`); a `C` followed by another capital is dropped, so `Commander`
/// stays whole. The rest is lowercased then title-cased like Laravel's
/// `Str::title` (`CMODERN` gives `Modern`, `pauper commander` gives
/// `Pauper Commander`). An underscore reads as a space, so the archetypes
/// key `historic_brawl` gives `Historic Brawl` like `ARENA_HISTORIC_BRAWL`.
pub fn display_format(code: &str) -> String {
    // Arena codes (`ARENA_HISTORIC_BRAWL`) read as words: `Historic Brawl`.
    if let Some(rest) = code.strip_prefix("ARENA_")
        && !rest.is_empty()
    {
        return title_case(&rest.replace('_', " ").to_ascii_lowercase());
    }
    let bytes = code.as_bytes();
    let raw = if bytes.len() >= 2 && bytes[0] == b'C' && bytes[1].is_ascii_uppercase() {
        &code[1..]
    } else {
        code
    };
    // Underscores separate words in archetypes keys (`historic_brawl`).
    title_case(&raw.replace('_', " ").to_ascii_lowercase())
}

/// Laravel's `Str::title` (PHP `mb_convert_case(..., MB_CASE_TITLE)`): a
/// letter is upper-cased when it does not follow another letter. Digits and
/// most punctuation start a new word; an apostrophe, `.`, `:`, `^` or `` ` ``
/// does not (`O'brien`, `3Rd`).
fn title_case(text: &str) -> String {
    let mut out = String::new();
    let mut in_word = false;
    for c in text.chars() {
        if c.is_lowercase() || c.is_uppercase() {
            if in_word {
                out.extend(c.to_lowercase());
            } else {
                out.extend(c.to_uppercase());
            }
            in_word = true;
        } else {
            out.push(c);
            if !matches!(c, '\'' | '.' | ':' | '^' | '`') {
                in_word = false;
            }
        }
    }
    out
}

/// Whether a deck's format is `Limited`, the format 0.x gives draft decks;
/// those are left out of the deck list. Only that exact label: a deck's
/// format code such as `DMU` or `S2` is a constructed deck for 0.x.
pub fn is_limited(format: &str) -> bool {
    format == "Limited"
}

/// Whether a match's MTGO format code is limited play (0.x
/// `isLimitedFormatCode`, `^(D[A-Z]|S[0-9])`: `DHOBHOBHOB` is a draft, `S6FRA`
/// a sealed pool). For match formats, not deck formats.
pub fn is_limited_match_format(code: &str) -> bool {
    let bytes = code.as_bytes();
    bytes.len() >= 2
        && ((bytes[0] == b'D' && bytes[1].is_ascii_uppercase())
            || (bytes[0] == b'S' && bytes[1].is_ascii_digit()))
}

/// One complete match for the Formats card (dashboard redesign spec 4.10).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormatMatch {
    /// The match's format code (`CMODERN`, `DHOBHOBHOB`).
    pub format: Option<String>,
    pub outcome: Outcome,
}

/// One Formats row: a label and its matches' record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormatShare {
    pub label: String,
    pub record: Record,
}

/// The row every limited match goes to.
pub const LIMITED_LABEL: &str = "Limited";
/// The row of a match without a format.
pub const UNKNOWN_FORMAT_LABEL: &str = "Unknown";

/// Matches by format label: codes with the same label are one row, every
/// limited match is the `Limited` row, a match without a format `Unknown`.
/// Most matches first, then by label.
pub fn format_breakdown(matches: &[FormatMatch]) -> Vec<FormatShare> {
    // (label, wins, losses, total), in first-seen order.
    let mut rows: Vec<(String, u32, u32, u32)> = Vec::new();
    for played in matches {
        let label = match played.format.as_deref() {
            Some(code) if is_limited_match_format(code) => LIMITED_LABEL.to_string(),
            Some(code) if !code.is_empty() => display_format(code),
            _ => UNKNOWN_FORMAT_LABEL.to_string(),
        };
        let index = match rows.iter().position(|row| row.0 == label) {
            Some(index) => index,
            None => {
                rows.push((label, 0, 0, 0));
                rows.len() - 1
            }
        };
        let row = &mut rows[index];
        match played.outcome {
            Outcome::Win => row.1 += 1,
            Outcome::Loss => row.2 += 1,
            Outcome::Draw | Outcome::Unknown => {}
        }
        row.3 += 1;
    }
    let mut shares = Vec::new();
    for (label, wins, losses, total) in rows {
        shares.push(FormatShare {
            label,
            record: Record::from_total(wins, losses, total),
        });
    }
    shares.sort_by(|a, b| {
        b.record
            .total
            .cmp(&a.record.total)
            .then_with(|| a.label.cmp(&b.label))
    });
    shares
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arena_codes_read_as_words() {
        for (code, label) in [
            ("ARENA_HISTORIC_BRAWL", "Historic Brawl"),
            ("ARENA_BRAWL", "Brawl"),
            ("ARENA_HISTORIC", "Historic"),
            ("CMODERN", "Modern"),
            // The archetypes key of an Arena code (the Metagame tabs).
            ("historic_brawl", "Historic Brawl"),
        ] {
            assert_eq!(display_format(code), label, "{code}");
        }
    }

    #[test]
    fn humanises_raw_codes() {
        for (code, label) in [
            ("CMODERN", "Modern"),
            ("CSTANDARD", "Standard"),
            ("CPIONEER", "Pioneer"),
            ("CPAUPER", "Pauper"),
            ("CLEGACY", "Legacy"),
            ("CVINTAGE", "Vintage"),
            ("CPREMODERN", "Premodern"),
            ("CModern", "Modern"),
        ] {
            assert_eq!(display_format(code), label, "{code}");
        }
    }

    #[test]
    fn leaves_humanised_labels_alone() {
        assert_eq!(display_format("Modern"), "Modern");
        assert_eq!(display_format("standard"), "Standard");
        assert_eq!(display_format("Limited"), "Limited");
        assert_eq!(display_format("Commander"), "Commander");
    }

    #[test]
    fn empty_stays_empty() {
        assert_eq!(display_format(""), "");
    }

    #[test]
    fn title_cases_like_laravel() {
        // Each checked against Str::title(strtolower(...)) under PHP.
        for (code, label) in [
            ("pauper commander", "Pauper Commander"),
            ("vintage-cube", "Vintage-Cube"),
            ("o'brien", "O'brien"),
            ("a.b", "A.b"),
            ("a:b", "A:b"),
            ("3rd ed", "3Rd Ed"),
            ("x2y z_w", "X2Y Z W"),
            ("x(y)", "X(Y)"),
            ("CPAUPER_X", "Pauper X"),
            ("DHOBHOBHOB", "Dhobhobhob"),
            ("C", "C"),
            ("CA", "A"),
            ("Cx", "Cx"),
        ] {
            assert_eq!(display_format(code), label, "{code}");
        }
    }

    #[test]
    fn only_the_limited_format_is_limited() {
        assert!(is_limited("Limited"));
        assert!(!is_limited("limited"));
        assert!(!is_limited("CMODERN"));
        assert!(!is_limited(""));
    }

    #[test]
    fn match_format_codes_of_drafts_and_sealed_are_limited() {
        for code in ["DHOBHOBHOB", "DAB", "S6FRA", "S3"] {
            assert!(is_limited_match_format(code), "{code}");
        }
        for code in [
            "D",
            "S",
            "Dx",
            "SX",
            "Standard",
            "CSTANDARD",
            "Sealed",
            "Limited",
        ] {
            assert!(!is_limited_match_format(code), "{code}");
        }
    }

    #[test]
    fn a_deck_format_code_is_not_limited_unless_it_is_the_label() {
        for code in ["DMU", "S2", "DHOBHOBHOB", "S6FRA"] {
            assert!(!is_limited(code), "{code}");
        }
    }

    #[test]
    fn the_breakdown_merges_codes_by_label_and_limited_into_one_row() {
        let played = |format: Option<&str>, outcome: Outcome| FormatMatch {
            format: format.map(str::to_string),
            outcome,
        };
        let rows = format_breakdown(&[
            played(Some("CMODERN"), Outcome::Win),
            played(Some("CModern"), Outcome::Loss),
            played(Some("CPAUPER"), Outcome::Win),
            played(Some("DQNWQNWQNW"), Outcome::Win),
            played(Some("S6QOD"), Outcome::Draw),
            played(None, Outcome::Win),
        ]);
        let summary: Vec<(&str, u32, u32, u32)> = rows
            .iter()
            .map(|row| {
                (
                    row.label.as_str(),
                    row.record.wins,
                    row.record.losses,
                    row.record.total,
                )
            })
            .collect();
        assert_eq!(
            summary,
            vec![
                ("Limited", 1, 0, 2),
                ("Modern", 1, 1, 2),
                ("Pauper", 1, 0, 1),
                ("Unknown", 1, 0, 1),
            ]
        );
        assert!(format_breakdown(&[]).is_empty());
    }
}
