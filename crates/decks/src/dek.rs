//! `.dek` files (0.x `GenerateDeckDekFile`, `ParseDekFile`): the deck file
//! format MTGO imports and exports.

use crate::DeckCard;
use crate::file::scan_tags;

/// 0.x `ParseDekFile`'s message for text that is not XML.
pub const INVALID_DEK: &str = "Invalid .dek file: could not parse XML.";

/// One `.dek` row. The caller resolves `name` from the card table and leaves
/// out cards with no card row, as 0.x does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DekCard {
    pub catalog_id: u32,
    pub quantity: u32,
    pub sideboard: bool,
    pub name: String,
}

/// The `.dek` XML for a card list, rows in the order given, lines joined with
/// `\n` and no trailing newline (byte-identical to 0.x for names without a
/// double quote).
///
/// Names escape `&`, `<` and `>` as 0.x does, and also `"`, which 0.x left
/// bare (its `htmlspecialchars(..., ENT_XML1)` call drops quote escaping),
/// breaking the attribute for a name such as `Kongming, "Sleeping Dragon"`.
pub fn dek_file(cards: &[DekCard]) -> String {
    let mut lines = vec![
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>".to_string(),
        "<Deck xmlns:xsd=\"http://www.w3.org/2001/XMLSchema\" xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\">".to_string(),
        "  <NetDeckID>0</NetDeckID>".to_string(),
        "  <PreconstructedDeckID>0</PreconstructedDeckID>".to_string(),
    ];

    for card in cards {
        lines.push(format!(
            "  <Cards CatID=\"{}\" Quantity=\"{}\" Sideboard=\"{}\" Name=\"{}\" Annotation=\"0\"/>",
            card.catalog_id,
            card.quantity,
            card.sideboard,
            escape_name(&card.name),
        ));
    }

    lines.push("</Deck>".to_string());
    lines.join("\n")
}

/// The cards of a `.dek` file (0.x `ParseDekFile`): every `<Cards>` element
/// directly under the root, in file order, with PHP's casts: `CatID` and
/// `Quantity` read as `(int)` ([`crate::php_int`], 0 when negative or past
/// `u32`) and `Sideboard` true
/// only for "true" in any case. Quantities are clamped to
/// [`crate::MAX_CARD_QUANTITY`]. Text that is not well-formed XML (or is
/// longer than [`crate::MAX_DEK_BYTES`]) is refused with 0.x's message.
pub fn parse_dek(xml: &str) -> Result<Vec<DeckCard>, String> {
    let Some(tags) = scan_tags(xml) else {
        return Err(INVALID_DEK.to_string());
    };
    let mut cards = Vec::new();
    for tag in &tags {
        if tag.depth != 1 || tag.name != "Cards" {
            continue;
        }
        let mut catalog_id = 0;
        let mut quantity = 0;
        let mut sideboard = false;
        for (key, value) in &tag.attributes {
            match key.as_str() {
                "CatID" => catalog_id = crate::php_int(value).unwrap_or(0),
                "Quantity" => {
                    quantity = crate::clamp_quantity(crate::php_int(value).unwrap_or(0));
                }
                "Sideboard" => sideboard = value.to_lowercase() == "true",
                _ => {}
            }
        }
        cards.push(DeckCard {
            catalog_id,
            quantity,
            sideboard,
        });
    }
    Ok(cards)
}

/// Escapes a card name for a double-quoted XML attribute.
fn escape_name(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dek(catalog_id: u32, quantity: u32, sideboard: bool, name: &str) -> DekCard {
        DekCard {
            catalog_id,
            quantity,
            sideboard,
            name: name.to_string(),
        }
    }

    #[test]
    fn writes_the_0x_layout() {
        let cards = [
            dek(100, 4, false, "Lightning Bolt"),
            dek(200, 20, false, "Mountain"),
            dek(100, 2, true, "Lightning Bolt"),
        ];
        let expected = [
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>",
            "<Deck xmlns:xsd=\"http://www.w3.org/2001/XMLSchema\" xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\">",
            "  <NetDeckID>0</NetDeckID>",
            "  <PreconstructedDeckID>0</PreconstructedDeckID>",
            "  <Cards CatID=\"100\" Quantity=\"4\" Sideboard=\"false\" Name=\"Lightning Bolt\" Annotation=\"0\"/>",
            "  <Cards CatID=\"200\" Quantity=\"20\" Sideboard=\"false\" Name=\"Mountain\" Annotation=\"0\"/>",
            "  <Cards CatID=\"100\" Quantity=\"2\" Sideboard=\"true\" Name=\"Lightning Bolt\" Annotation=\"0\"/>",
            "</Deck>",
        ]
        .join("\n");
        assert_eq!(dek_file(&cards), expected);
    }

    #[test]
    fn an_empty_list_still_has_the_wrapper() {
        let text = dek_file(&[]);
        assert!(text.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<Deck "));
        assert!(text.ends_with("  <PreconstructedDeckID>0</PreconstructedDeckID>\n</Deck>"));
    }

    #[test]
    fn escapes_names() {
        let text = dek_file(&[
            dek(1, 1, false, "Ach! Hans, Run!"),
            dek(2, 1, false, "R&D's <Secret> \"Lair\""),
        ]);
        assert!(text.contains("Name=\"Ach! Hans, Run!\""));
        assert!(text.contains("Name=\"R&amp;D's &lt;Secret&gt; &quot;Lair&quot;\""));
    }

    // 0.x ParseDekFileTest "parses a dek file into card entries".
    #[test]
    fn parses_a_dek_file_into_card_entries() {
        let xml = [
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>",
            "<Deck xmlns:xsd=\"http://www.w3.org/2001/XMLSchema\" xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\">",
            "  <NetDeckID>0</NetDeckID>",
            "  <PreconstructedDeckID>0</PreconstructedDeckID>",
            "  <Cards CatID=\"12345\" Quantity=\"4\" Sideboard=\"false\" Name=\"Qbolt\" Annotation=\"0\"/>",
            "  <Cards CatID=\"67890\" Quantity=\"2\" Sideboard=\"true\" Name=\"Qpath\" Annotation=\"0\"/>",
            "</Deck>",
        ]
        .join("\n");
        assert_eq!(
            parse_dek(&xml),
            Ok(vec![
                DeckCard {
                    catalog_id: 12345,
                    quantity: 4,
                    sideboard: false,
                },
                DeckCard {
                    catalog_id: 67890,
                    quantity: 2,
                    sideboard: true,
                },
            ])
        );
    }

    // 0.x ParseDekFileTest "handles empty deck file".
    #[test]
    fn an_empty_deck_file_has_no_cards() {
        assert_eq!(parse_dek(&dek_file(&[])), Ok(Vec::new()));
    }

    // 0.x ParseDekFileTest "throws on invalid xml".
    #[test]
    fn invalid_xml_is_refused_with_the_0x_message() {
        assert_eq!(
            parse_dek("not xml at all"),
            Err("Invalid .dek file: could not parse XML.".to_string())
        );
        assert_eq!(
            parse_dek("<Deck><Cards CatID=\"1\"></Deck>"),
            Err(INVALID_DEK.to_string())
        );
    }

    #[test]
    fn what_the_writer_writes_reads_back() {
        let cards = [
            dek(100, 4, false, "Qbolt"),
            dek(100, 2, true, "Qbolt"),
            dek(7, 1, false, "R&D's \"Lair\""),
        ];
        let read = parse_dek(&dek_file(&cards)).expect("parse");
        let back: Vec<(u32, u32, bool)> = read
            .iter()
            .map(|card| (card.catalog_id, card.quantity, card.sideboard))
            .collect();
        assert_eq!(back, vec![(100, 4, false), (100, 2, true), (7, 1, false)]);
    }

    #[test]
    fn a_quantity_is_clamped_to_the_card_quantity_cap() {
        let xml = "<Deck><Cards CatID=\"5\" Quantity=\"999999\" Sideboard=\"false\"/></Deck>";
        let cards = parse_dek(xml).expect("parses");
        assert_eq!(cards[0].quantity, crate::MAX_CARD_QUANTITY);
    }

    #[test]
    fn numbers_and_flags_read_as_php_casts() {
        // `(int)` of a missing or non-numeric attribute is 0; only "true"
        // (any case) is the sideboard.
        let xml = "<Deck><Cards CatID=\"12x\" Quantity=\"q\" Sideboard=\"TRUE\"/><Cards Quantity=\"3\" Sideboard=\"1\"/><Other CatID=\"5\"/></Deck>";
        assert_eq!(
            parse_dek(xml),
            Ok(vec![
                DeckCard {
                    catalog_id: 12,
                    quantity: 0,
                    sideboard: true,
                },
                DeckCard {
                    catalog_id: 0,
                    quantity: 3,
                    sideboard: false,
                },
            ])
        );
    }

    #[test]
    fn numbers_read_as_the_crate_php_int_cast() {
        // PHP 8.4: (int)"1e1" is 10, (int)" 3" is 3, (int)"4.9x" is 4,
        // (int)"1e3x" is 1000; a negative number is 0 here (no negative
        // printing or count).
        let xml = "<Deck><Cards CatID=\"1e1\" Quantity=\" 3\"/><Cards CatID=\"1e3x\" Quantity=\"4.9x\"/><Cards CatID=\"-2\" Quantity=\"\"/></Deck>";
        let cards = parse_dek(xml).expect("parses");
        let numbers: Vec<(u32, u32)> = cards
            .iter()
            .map(|card| (card.catalog_id, card.quantity))
            .collect();
        assert_eq!(numbers, vec![(10, 3), (1000, 4), (0, 0)]);
    }

    #[test]
    fn the_output_reads_back_as_a_deck_file_shape() {
        // The scanner that reads deck files accepts what we write.
        let text = dek_file(&[dek(2, 1, true, "Kongming, \"Sleeping Dragon\"")]);
        assert!(crate::file::scan_tags(&text).is_some());
    }
}
