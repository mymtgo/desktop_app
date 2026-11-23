//! MTGO deck files (0.x `GetDeckFiles` and `SyncDecks`).
//!
//! MTGO keeps one `grouping <uuid>.xml` per deck in the account's data folder:
//!
//! ```xml
//! <Grouping Name="..." NetDeckId="..." GroupingType="Deck" Timestamp="2026-04-29T10:00:00" FormatCode="CMODERN">
//!   <Item CatId="1001" Quantity="4" IsSideboard="False" />
//! </Grouping>
//! ```
//!
//! The XML is machine-written and flat, so a small tag and attribute scanner
//! reads it (no XML dependency).

use chrono::{DateTime, NaiveDateTime, Timelike};
use chrono_tz::Tz;

use crate::{DeckCard, php_bool};

/// The largest deck file or `.dek` text the scanner reads, in bytes. Real
/// ones are a few kilobytes; larger text does not parse. Callers that read
/// the file should refuse a bigger one before reading it all.
pub const MAX_DEK_BYTES: usize = 2 * 1024 * 1024;

/// The most attributes one tag may have. MTGO writes at most six.
const MAX_ATTRIBUTES: usize = 64;

/// A parsed deck file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeckFile {
    pub name: String,
    /// `NetDeckId`: the deck's MTGO id (a uuid), stable across edits.
    pub net_deck_id: String,
    /// `FormatCode` as written (`CMODERN`, `Standard`, `Limited`, ...).
    pub format: String,
    /// `Timestamp`, truncated to the second, as UTC unix milliseconds.
    pub timestamp_ms: i64,
    pub cards: Vec<DeckCard>,
}

/// Reads a deck file. `None` when the XML does not parse, the root element's
/// `GroupingType` is not `Deck`, or a required attribute is missing (0.x
/// stops on those too).
///
/// `Timestamp` carries no zone (`2026-04-29T10:00:00`) and is read in `tz`;
/// an explicit offset in the text wins. 0.x parses it with `Carbon::parse` in
/// the app timezone, which is UTC, so pass `chrono_tz::UTC` to match 0.x.
pub fn parse_deck_file(xml: &str, tz: Tz) -> Option<DeckFile> {
    let tags = scan_tags(xml)?;
    let root = tags.first()?;

    // 0.x reads only the root's attributes, whatever the root is called.
    if attribute(root, "GroupingType")? != "Deck" {
        return None;
    }
    let name = attribute(root, "Name")?.to_string();
    let net_deck_id = attribute(root, "NetDeckId")?.to_string();
    let format = attribute(root, "FormatCode")?.to_string();
    let timestamp_ms = parse_timestamp(attribute(root, "Timestamp")?, tz)?;

    let mut cards = Vec::new();
    for tag in &tags {
        if tag.depth == 1 && tag.name == "Item" {
            cards.push(parse_item(tag)?);
        }
    }

    Some(DeckFile {
        name,
        net_deck_id,
        format,
        timestamp_ms,
        cards,
    })
}

/// One `<Item CatId Quantity IsSideboard/>`; `None` when any is missing, the
/// catalog id does not parse, or the quantity is negative or too big.
fn parse_item(tag: &Tag) -> Option<DeckCard> {
    let catalog_id = attribute(tag, "CatId")?.trim().parse::<u32>().ok()?;
    // 0.x reads it with `(int)`: text that is not a number is 0.
    let quantity = crate::clamp_quantity(crate::php_int(attribute(tag, "Quantity")?)?);
    let sideboard = php_bool(attribute(tag, "IsSideboard")?);
    Some(DeckCard {
        catalog_id,
        quantity,
        sideboard,
    })
}

/// The value of a tag's attribute, by exact name.
fn attribute<'a>(tag: &'a Tag, name: &str) -> Option<&'a str> {
    tag.attributes
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.as_str())
}

/// A deck file `Timestamp` as UTC unix milliseconds, truncated to the second
/// (0.x `Carbon::parse(...)->startOfSecond()`). Text with an offset or `Z`
/// keeps it; text without one is wall-clock time in `tz`, a time the clocks
/// skip moved on by the gap as PHP does.
fn parse_timestamp(text: &str, tz: Tz) -> Option<i64> {
    let text = text.trim();

    if let Ok(with_offset) = DateTime::parse_from_rfc3339(text) {
        return Some(with_offset.timestamp() * 1000);
    }

    let mut local = None;
    for pattern in ["%Y-%m-%dT%H:%M:%S%.f", "%Y-%m-%d %H:%M:%S%.f"] {
        if let Ok(parsed) = NaiveDateTime::parse_from_str(text, pattern) {
            local = Some(parsed);
            break;
        }
    }
    let local = local?.with_nanosecond(0)?;
    mtgo_protocol::local_to_utc_ms(local, tz)
}

/// Whether a file name is an MTGO deck file: `grouping <36 hex digits or
/// dashes>.xml`, any case (0.x `/^grouping ([0-9a-f-]{36})\.xml$/i`).
pub fn is_deck_file_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    let Some(rest) = lower.strip_prefix("grouping ") else {
        return false;
    };
    let Some(id) = rest.strip_suffix(".xml") else {
        return false;
    };
    id.len() == 36
        && id
            .chars()
            .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c) || c == '-')
}

/// One element tag found by [`scan_tags`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Tag {
    pub(crate) name: String,
    /// Attribute names and decoded values, in document order.
    pub(crate) attributes: Vec<(String, String)>,
    /// How many elements enclose this one (the root is 0).
    pub(crate) depth: usize,
}

/// Every element of a small, flat XML document in document order, or `None`
/// when it is not well formed or is longer than [`MAX_DEK_BYTES`].
pub(crate) fn scan_tags(xml: &str) -> Option<Vec<Tag>> {
    if xml.len() > MAX_DEK_BYTES {
        return None;
    }
    // XML reads every line ending as a line feed.
    let text = xml.replace("\r\n", "\n").replace('\r', "\n");
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);

    let mut tags = Vec::new();
    // Names of the elements still open, innermost last.
    let mut open: Vec<String> = Vec::new();
    let mut root_closed = false;
    let mut rest = text;

    while !rest.is_empty() {
        let Some(start) = rest.find('<') else {
            check_text(rest, open.is_empty())?;
            break;
        };
        check_text(&rest[..start], open.is_empty())?;
        rest = &rest[start..];

        if rest.starts_with("<?") {
            rest = skip_past(rest, "?>")?;
        } else if rest.starts_with("<!--") {
            rest = skip_past(rest, "-->")?;
        } else if rest.starts_with("<![CDATA[") {
            if open.is_empty() {
                return None;
            }
            rest = skip_past(rest, "]]>")?;
        } else if rest.starts_with("<!") {
            // A DOCTYPE before the root; MTGO does not write one with a subset.
            if !tags.is_empty() {
                return None;
            }
            rest = skip_past(rest, ">")?;
        } else if let Some(after) = rest.strip_prefix("</") {
            let end = after.find('>')?;
            let name = after[..end].trim_end();
            if open.pop()? != name {
                return None;
            }
            if open.is_empty() {
                root_closed = true;
            }
            rest = &after[end + 1..];
        } else {
            if root_closed {
                // A second root element.
                return None;
            }
            let start_tag = read_start_tag(&rest[1..])?;
            tags.push(Tag {
                name: start_tag.name.clone(),
                attributes: start_tag.attributes,
                depth: open.len(),
            });
            if !start_tag.self_closing {
                open.push(start_tag.name);
            } else if open.is_empty() {
                root_closed = true;
            }
            rest = start_tag.rest;
        }
    }

    if !open.is_empty() || !root_closed {
        return None;
    }
    Some(tags)
}

/// Text between tags: only whitespace outside the root, and no bad entities
/// inside it.
fn check_text(text: &str, outside_root: bool) -> Option<()> {
    if outside_root {
        if text.trim().is_empty() {
            Some(())
        } else {
            None
        }
    } else {
        decode_entities(text).map(|_| ())
    }
}

/// The text after the first `marker`.
fn skip_past<'a>(text: &'a str, marker: &str) -> Option<&'a str> {
    let at = text.find(marker)?;
    Some(&text[at + marker.len()..])
}

/// A start tag read by [`read_start_tag`].
struct StartTag<'a> {
    name: String,
    attributes: Vec<(String, String)>,
    /// Ends with `/>`, so it has no children and no end tag.
    self_closing: bool,
    /// The document text after the tag.
    rest: &'a str,
}

/// A start tag after its `<`.
fn read_start_tag(text: &str) -> Option<StartTag<'_>> {
    let name_end = text.find(|c: char| c.is_whitespace() || c == '/' || c == '>')?;
    let name = &text[..name_end];
    if !is_name(name) {
        return None;
    }

    let mut attributes: Vec<(String, String)> = Vec::new();
    let mut rest = &text[name_end..];
    loop {
        let trimmed = rest.trim_start();
        let had_space = trimmed.len() < rest.len();
        rest = trimmed;

        let ending = if let Some(after) = rest.strip_prefix("/>") {
            Some((true, after))
        } else {
            rest.strip_prefix('>').map(|after| (false, after))
        };
        if let Some((self_closing, after)) = ending {
            return Some(StartTag {
                name: name.to_string(),
                attributes,
                self_closing,
                rest: after,
            });
        }
        // Attributes are separated from the name and each other by whitespace.
        if !had_space {
            return None;
        }

        if attributes.len() >= MAX_ATTRIBUTES {
            return None;
        }
        let equals = rest.find('=')?;
        let key = rest[..equals].trim_end();
        if !is_name(key) || attributes.iter().any(|(existing, _)| existing == key) {
            return None;
        }
        rest = rest[equals + 1..].trim_start();

        let quote = rest.chars().next()?;
        if quote != '"' && quote != '\'' {
            return None;
        }
        let close = rest[1..].find(quote)? + 1;
        let raw = &rest[1..close];
        if raw.contains('<') {
            return None;
        }
        // Attribute values read literal tabs and line feeds as spaces.
        let spaced = raw.replace(['\t', '\n'], " ");
        attributes.push((key.to_string(), decode_entities(&spaced)?));
        rest = &rest[close + 1..];
    }
}

/// Whether text is a usable element or attribute name.
fn is_name(text: &str) -> bool {
    let mut chars = text.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_alphabetic() || first == '_' || first == ':')
        && chars.all(|c| c.is_alphanumeric() || matches!(c, '_' | ':' | '-' | '.'))
}

/// Decodes `&amp; &lt; &gt; &quot; &apos;` and `&#N;` / `&#xN;`. `None` for
/// a bare `&` or any other entity, which XML parsers reject.
fn decode_entities(text: &str) -> Option<String> {
    let mut out = String::new();
    let mut rest = text;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        let after = &rest[at + 1..];
        let end = after.find(';')?;
        let entity = &after[..end];
        let decoded = match entity {
            "amp" => '&',
            "lt" => '<',
            "gt" => '>',
            "quot" => '"',
            "apos" => '\'',
            _ => {
                let number = entity.strip_prefix('#')?;
                let code = match number.strip_prefix('x') {
                    Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                    None => number.parse::<u32>().ok()?,
                };
                char::from_u32(code)?
            }
        };
        out.push(decoded);
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DECK_ID: &str = "caab1001-0000-0000-0000-000000002001";

    fn card(catalog_id: u32, quantity: u32, sideboard: bool) -> DeckCard {
        DeckCard {
            catalog_id,
            quantity,
            sideboard,
        }
    }

    /// The shape of 0.x's SyncDecksTest file (no player names in it).
    fn caps_test_xml() -> String {
        format!(
            "<Grouping Name=\"Caps Test\" NetDeckId=\"{DECK_ID}\" GroupingType=\"Deck\" \
             Timestamp=\"2026-04-29T10:00:00\" FormatCode=\"Standard\">\n\
             \x20 <Item CatId=\"1001\" Quantity=\"4\" IsSideboard=\"False\" />\n\
             \x20 <Item CatId=\"2001\" Quantity=\"3\" IsSideboard=\"True\" />\n\
             </Grouping>"
        )
    }

    #[test]
    fn parses_a_deck_file() {
        let deck = parse_deck_file(&caps_test_xml(), chrono_tz::UTC).unwrap();
        assert_eq!(
            deck,
            DeckFile {
                name: "Caps Test".to_string(),
                net_deck_id: DECK_ID.to_string(),
                format: "Standard".to_string(),
                timestamp_ms: 1_777_456_800_000,
                cards: vec![card(1001, 4, false), card(2001, 3, true)],
            }
        );
    }

    #[test]
    fn sideboard_capitalisation_gives_the_0x_signature() {
        // SyncDecksTest: IsSideboard="False"/"True" still sign as false/true.
        let deck = parse_deck_file(&caps_test_xml(), chrono_tz::UTC).unwrap();
        assert_eq!(
            crate::signature(&deck.cards),
            "MTAwMTo0OmZhbHNlfDIwMDE6Mzp0cnVl"
        );
    }

    #[test]
    fn reads_the_timestamp_in_the_given_timezone() {
        let deck = parse_deck_file(&caps_test_xml(), chrono_tz::Europe::London).unwrap();
        // 10:00 BST is 09:00 UTC.
        assert_eq!(deck.timestamp_ms, 1_777_453_200_000);
    }

    #[test]
    fn timestamp_drops_fractions_and_honours_an_explicit_offset() {
        let with = |stamp: &str| caps_test_xml().replace("2026-04-29T10:00:00", stamp);
        let utc = chrono_tz::UTC;
        assert_eq!(
            parse_deck_file(&with("2026-04-29T10:00:00.9876543"), utc)
                .unwrap()
                .timestamp_ms,
            1_777_456_800_000
        );
        assert_eq!(
            parse_deck_file(&with("2026-04-29T10:00:00+02:00"), utc)
                .unwrap()
                .timestamp_ms,
            1_777_449_600_000
        );
        assert_eq!(
            parse_deck_file(&with("2026-04-29T10:00:00Z"), chrono_tz::Europe::London)
                .unwrap()
                .timestamp_ms,
            1_777_456_800_000
        );
        assert_eq!(
            parse_deck_file(&with("2026-04-29 10:00:00"), utc)
                .unwrap()
                .timestamp_ms,
            1_777_456_800_000
        );
        assert_eq!(parse_deck_file(&with("yesterday-ish"), utc), None);
    }

    #[test]
    fn a_time_inside_a_dst_gap_moves_forward() {
        let xml = caps_test_xml().replace("2026-04-29T10:00:00", "2026-03-29T01:30:00");
        let deck = parse_deck_file(&xml, chrono_tz::Europe::London).unwrap();
        // 01:30 does not exist in London that night; 02:30 BST is 01:30 UTC.
        assert_eq!(deck.timestamp_ms, 1_774_747_800_000);
        // Lord Howe's gap is half an hour: 02:15 (+10:30) reads as 02:45 (+11:00).
        let xml = caps_test_xml().replace("2026-04-29T10:00:00", "2026-10-04T02:15:00");
        let deck = parse_deck_file(&xml, chrono_tz::Australia::Lord_Howe).unwrap();
        assert_eq!(deck.timestamp_ms, 1_791_042_300_000);
    }

    #[test]
    fn reads_a_dek_shaped_file_with_a_declaration() {
        // 0.x tests/Fixtures/sample_deck.dek: the root is <Deck>, which 0.x
        // accepts because it only reads the root's attributes.
        let xml = "<?xml version=\"1.0\" encoding=\"utf-8\"?>\r\n\
            <Deck xmlns:xsd=\"http://www.w3.org/2001/XMLSchema\" \
            xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\" GroupingType=\"Deck\" \
            NetDeckId=\"12345\" Name=\"Test Deck\" FormatCode=\"Standard\" \
            Timestamp=\"2026-01-20T10:00:00\">\r\n\
            \x20 <Item CatId=\"1001\" Quantity=\"4\" IsSideboard=\"false\" />\r\n\
            \x20 <Item CatId=\"1002\" Quantity=\"4\" IsSideboard=\"false\" />\r\n\
            \x20 <Item CatId=\"1003\" Quantity=\"4\" IsSideboard=\"false\" />\r\n\
            \x20 <Item CatId=\"2001\" Quantity=\"3\" IsSideboard=\"true\" />\r\n\
            </Deck>\r\n";
        let deck = parse_deck_file(xml, chrono_tz::UTC).unwrap();
        assert_eq!(deck.name, "Test Deck");
        assert_eq!(deck.net_deck_id, "12345");
        assert_eq!(deck.cards.len(), 4);
        assert_eq!(deck.cards[3], card(2001, 3, true));
    }

    #[test]
    fn accepts_a_byte_order_mark_and_comments() {
        let xml = format!("\u{feff}<!-- saved by MTGO -->\n{}", caps_test_xml());
        assert!(parse_deck_file(&xml, chrono_tz::UTC).is_some());
    }

    #[test]
    fn decodes_entities_in_attributes() {
        let xml = caps_test_xml().replace(
            "Name=\"Caps Test\"",
            "Name=\"R&amp;B &lt;Burn&gt; &quot;Q&quot; &apos;s &#65;&#x42; caf&#xe9;\"",
        );
        let deck = parse_deck_file(&xml, chrono_tz::UTC).unwrap();
        assert_eq!(deck.name, "R&B <Burn> \"Q\" 's AB caf\u{e9}");
    }

    #[test]
    fn single_quoted_attributes_and_spaces_around_equals_parse() {
        let xml = caps_test_xml().replace("Name=\"Caps Test\"", "Name = 'Caps \"Q\" Test'");
        let deck = parse_deck_file(&xml, chrono_tz::UTC).unwrap();
        assert_eq!(deck.name, "Caps \"Q\" Test");
    }

    #[test]
    fn a_deck_with_no_items_has_no_cards() {
        let xml = format!(
            "<Grouping Name=\"Empty\" NetDeckId=\"{DECK_ID}\" GroupingType=\"Deck\" \
             Timestamp=\"2026-04-29T10:00:00\" FormatCode=\"CMODERN\" />"
        );
        let deck = parse_deck_file(&xml, chrono_tz::UTC).unwrap();
        assert_eq!(deck.cards, vec![]);
        assert_eq!(deck.format, "CMODERN");
    }

    #[test]
    fn only_direct_item_children_are_cards() {
        let xml = caps_test_xml().replace(
            "</Grouping>",
            "<Other><Item CatId=\"5\" Quantity=\"1\" IsSideboard=\"false\"/></Other></Grouping>",
        );
        let deck = parse_deck_file(&xml, chrono_tz::UTC).unwrap();
        assert_eq!(deck.cards.len(), 2);
    }

    #[test]
    fn a_grouping_that_is_not_a_deck_is_skipped() {
        let xml = caps_test_xml().replace("GroupingType=\"Deck\"", "GroupingType=\"Binder\"");
        assert_eq!(parse_deck_file(&xml, chrono_tz::UTC), None);
        let xml = caps_test_xml().replace("GroupingType=\"Deck\"", "GroupingType=\"deck\"");
        assert_eq!(parse_deck_file(&xml, chrono_tz::UTC), None);
        let xml = caps_test_xml().replace("GroupingType=\"Deck\" ", "");
        assert_eq!(parse_deck_file(&xml, chrono_tz::UTC), None);
    }

    #[test]
    fn missing_required_attributes_are_skipped() {
        for attribute in [
            "Name=\"Caps Test\" ",
            "Timestamp=\"2026-04-29T10:00:00\" ",
            "FormatCode=\"Standard\"",
        ] {
            let xml = caps_test_xml().replace(attribute, "");
            assert_eq!(parse_deck_file(&xml, chrono_tz::UTC), None, "{attribute}");
        }
        let xml = caps_test_xml().replace(&format!("NetDeckId=\"{DECK_ID}\" "), "");
        assert_eq!(parse_deck_file(&xml, chrono_tz::UTC), None);
    }

    #[test]
    fn a_bad_item_is_skipped_with_its_file() {
        for (from, to) in [
            ("CatId=\"1001\"", "CatId=\"abc\""),
            ("Quantity=\"4\"", "Quantity=\"-1\""),
            ("IsSideboard=\"False\" ", ""),
        ] {
            let xml = caps_test_xml().replacen(from, to, 1);
            assert_eq!(parse_deck_file(&xml, chrono_tz::UTC), None, "{to}");
        }
    }

    #[test]
    fn a_quantity_is_read_like_a_php_int_cast() {
        for (quantity, expected) in [("4.9", 4), ("x", 0), ("", 0), ("2 ", 2)] {
            let xml =
                caps_test_xml().replacen("Quantity=\"4\"", &format!("Quantity=\"{quantity}\""), 1);
            let file = parse_deck_file(&xml, chrono_tz::UTC).expect("parses");
            assert_eq!(file.cards[0].quantity, expected, "{quantity:?}");
        }
    }

    #[test]
    fn a_quantity_is_clamped_to_the_card_quantity_cap() {
        let xml = caps_test_xml().replacen("Quantity=\"4\"", "Quantity=\"4000000000\"", 1);
        let file = parse_deck_file(&xml, chrono_tz::UTC).expect("parses");
        assert_eq!(file.cards[0].quantity, crate::MAX_CARD_QUANTITY);
    }

    #[test]
    fn a_tag_with_too_many_attributes_does_not_parse_and_stays_fast() {
        let at_cap: String = (0..MAX_ATTRIBUTES)
            .map(|n| format!(" a{n}=\"x\""))
            .collect();
        let xml = caps_test_xml().replacen("<Item ", &format!("<Item{at_cap} "), 1);
        assert!(
            scan_tags(&xml).is_none(),
            "the Item's own three push it over"
        );
        let xml = format!("<Deck{at_cap}/>");
        assert!(scan_tags(&xml).is_some(), "the cap itself is allowed");

        let many: String = (0..100_000).map(|n| format!(" a{n}=\"x\"")).collect();
        let started = std::time::Instant::now();
        assert!(scan_tags(&format!("<Deck{many}/>")).is_none());
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
    }

    #[test]
    fn a_document_over_the_size_cap_does_not_parse() {
        let padding = " ".repeat(MAX_DEK_BYTES);
        let xml = format!("{}{padding}", caps_test_xml());
        assert_eq!(parse_deck_file(&xml, chrono_tz::UTC), None);
        assert!(crate::parse_dek(&format!("<Deck/>{padding}")).is_err());
    }

    #[test]
    fn malformed_xml_does_not_parse() {
        let xml = caps_test_xml();
        let broken = [
            "",
            "not xml at all",
            &xml[..xml.len() - 3],
            &xml.replace("</Grouping>", "</Group>"),
            &xml.replace("Name=\"Caps Test\"", "Name=Caps"),
            &xml.replace("Name=\"Caps Test\"", "Name=\"Caps &nbsp; Test\""),
            &xml.replace("Name=\"Caps Test\"", "Name=\"Caps & Test\""),
            &xml.replace("Name=\"Caps Test\"", "Name=\"Caps <Test\""),
            &xml.replace("Name=\"Caps Test\"", "Name=\"Caps Test\" Name=\"Again\""),
            &format!("{xml}<Grouping/>"),
        ];
        for text in broken {
            assert_eq!(parse_deck_file(text, chrono_tz::UTC), None, "{text:?}");
        }
    }

    #[test]
    fn deck_file_names() {
        assert!(is_deck_file_name(&format!("grouping {DECK_ID}.xml")));
        assert!(is_deck_file_name(&format!(
            "Grouping {}.XML",
            DECK_ID.to_uppercase()
        )));
        assert!(is_deck_file_name(
            "grouping ------------------------------------.xml"
        ));
        assert!(!is_deck_file_name(&format!("grouping {DECK_ID}.xml.bak")));
        assert!(!is_deck_file_name(&format!("grouping  {DECK_ID}.xml")));
        assert!(!is_deck_file_name(&format!(
            "grouping {}.xml",
            &DECK_ID[1..]
        )));
        assert!(!is_deck_file_name(&format!("grouping {DECK_ID}0.xml")));
        assert!(!is_deck_file_name(
            "grouping caab1001-0000-0000-0000-00000000200g.xml"
        ));
        assert!(!is_deck_file_name("user_settings"));
        assert!(!is_deck_file_name(""));
    }
}
