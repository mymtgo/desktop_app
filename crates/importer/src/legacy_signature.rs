//! Converting legacy 0.x deck signatures. Older 0.x versions wrote each card
//! row as `oracle_id:quantity:sideboard` (base64 of the rows joined with `|`);
//! the current form uses the MTGO catalog id first (0.x `DeckVersion`
//! `getCardsAttribute`). v1 reads only the current form, so a legacy signature
//! is rebuilt from the imported cards: each oracle id becomes the lowest
//! catalog id with that oracle id.

use std::collections::HashMap;

use decks::DeckCard;

/// The current-form signature of a legacy signature, or `None` when it is not
/// a legacy signature v1 can convert: it does not decode, has no card rows,
/// starts with a catalog id (not legacy), has a quantity PHP would read as negative or huge,
/// or names an oracle id no imported card has. Rows with an empty oracle id
/// are left out (0.x could not resolve them either); a signature with no
/// other rows gives `None`.
pub(crate) fn convert_legacy(
    signature: &str,
    catalog_by_oracle: &HashMap<String, i64>,
) -> Option<String> {
    let bytes = decks::base64_decode(signature)?;
    let text = String::from_utf8(bytes).ok()?;
    // Rows with at least three `:` parts, as 0.x reads them.
    let rows: Vec<Vec<&str>> = text
        .split('|')
        .filter(|row| !row.is_empty())
        .map(|row| row.split(':').collect::<Vec<&str>>())
        .filter(|parts| parts.len() >= 3)
        .collect();
    let first = rows.first()?;
    if decks::php_is_numeric(first[0]) {
        return None;
    }
    let mut cards = Vec::new();
    for parts in rows {
        let oracle_id = parts[0].trim();
        // 0.x wrote an empty oracle id for a card it had no data for; it could
        // not show that card either, so the row is left out.
        if oracle_id.is_empty() {
            continue;
        }
        let catalog_id = catalog_by_oracle.get(oracle_id)?;
        cards.push(DeckCard {
            catalog_id: u32::try_from(*catalog_id).ok()?,
            quantity: decks::php_int(parts[1])?,
            sideboard: php_bool(parts[2]),
        });
    }
    if cards.is_empty() {
        return None;
    }
    Some(decks::signature(&cards))
}

/// PHP's `filter_var($value, FILTER_VALIDATE_BOOL)`, as 0.x reads the
/// sideboard flag: `true`, `1`, `yes` and `on` are true.
fn php_bool(value: &str) -> bool {
    let lower = value.trim().to_ascii_lowercase();
    lower == "true" || lower == "1" || lower == "yes" || lower == "on"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_standard_base64_with_and_without_padding() {
        assert_eq!(decks::base64_decode("TWFu"), Some(b"Man".to_vec()));
        assert_eq!(decks::base64_decode("TWE="), Some(b"Ma".to_vec()));
        assert_eq!(decks::base64_decode("TQ=="), Some(b"M".to_vec()));
        assert_eq!(decks::base64_decode("TQ"), Some(b"M".to_vec()));
        assert_eq!(decks::base64_decode(""), Some(Vec::new()));
        assert_eq!(decks::base64_decode("not base64!"), None);
    }

    fn oracles() -> HashMap<String, i64> {
        let mut map = HashMap::new();
        map.insert("q-oracle-a".to_string(), 12);
        map.insert("q-oracle-b".to_string(), 34);
        map
    }

    #[test]
    fn a_legacy_signature_becomes_the_current_form() {
        // base64 of "q-oracle-b:2:true|q-oracle-a:4:false"
        let legacy = "cS1vcmFjbGUtYjoyOnRydWV8cS1vcmFjbGUtYTo0OmZhbHNl";
        let expected = decks::signature(&[
            DeckCard {
                catalog_id: 12,
                quantity: 4,
                sideboard: false,
            },
            DeckCard {
                catalog_id: 34,
                quantity: 2,
                sideboard: true,
            },
        ]);
        assert_eq!(convert_legacy(legacy, &oracles()), Some(expected));
    }

    #[test]
    fn a_row_without_an_oracle_id_is_left_out() {
        // base64 of "q-oracle-a:4:false|:2:true": 0.x wrote an empty oracle id
        // for a card it had no data for, and could not show that card either.
        let legacy = "cS1vcmFjbGUtYTo0OmZhbHNlfDoyOnRydWU=";
        let expected = decks::signature(&[DeckCard {
            catalog_id: 12,
            quantity: 4,
            sideboard: false,
        }]);
        assert_eq!(convert_legacy(legacy, &oracles()), Some(expected));
        // base64 of ":2:true": nothing left.
        assert_eq!(convert_legacy("OjI6dHJ1ZQ==", &oracles()), None);
    }

    #[test]
    fn a_non_numeric_quantity_reads_as_zero_like_php() {
        // base64 of "q-oracle-a:x:false"
        let legacy = "cS1vcmFjbGUtYTp4OmZhbHNl";
        let expected = decks::signature(&[DeckCard {
            catalog_id: 12,
            quantity: 0,
            sideboard: false,
        }]);
        assert_eq!(convert_legacy(legacy, &oracles()), Some(expected));
    }

    #[test]
    fn unknown_oracle_ids_modern_and_bad_signatures_are_not_converted() {
        // base64 of "q-oracle-z:1:false"
        assert_eq!(convert_legacy("cS1vcmFjbGUtejoxOmZhbHNl", &oracles()), None);
        // base64 of "12:4:false" (already the current form)
        assert_eq!(convert_legacy("MTI6NDpmYWxzZQ==", &oracles()), None);
        assert_eq!(convert_legacy("not base64!", &oracles()), None);
        assert_eq!(convert_legacy("", &oracles()), None);
    }
}
