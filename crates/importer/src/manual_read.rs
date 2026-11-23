//! What the player recorded on a 0.x manual match's games (Plan L): the hand,
//! the sideboarding and the opponent's revealed cards. 0.x kept them on the
//! game's player pivots (`game_player`):
//! - the local seat's `opening_hand_json`,
//!   `{"kept":[ids],"bottomed":[ids],"mulligans":[[ids],...]}` (0.x
//!   `UpdateManualGameHand`), NULL when no hand is recorded;
//! - the local seat's `deck_json`, the whole postboard list
//!   (`[{"mtgo_id","quantity","sideboard"}]`, 0.x
//!   `UpdateManualGameSideboard`), `[]` when the game used the registered
//!   deck;
//! - the opponent seat's `deck_json`, the revealed cards
//!   (`[{"mtgo_id","quantity"}]`, 0.x `UpdateManualGameReveals`).
//!
//! Values are read loosely, as in `read`: a value v1 cannot read counts as
//! not recorded.

use std::collections::{BTreeMap, HashMap};

use serde_json::Value;
use store::manual::{CardCount, ManualGameDetail};

use crate::deck_read::ImportedVersions;
use crate::parity_rules::main_copies;

/// The registered maindeck of each imported 0.x deck version, by its 0.x id:
/// copies by catalog id.
pub(crate) type RegisteredMains = HashMap<i64, BTreeMap<u32, u32>>;

/// The maindeck copies of each imported deck version.
pub(crate) fn registered_mains(versions: &ImportedVersions) -> RegisteredMains {
    let mut mains = HashMap::new();
    for (version_id, deck) in versions {
        mains.insert(*version_id, main_copies(&deck.signature));
    }
    mains
}

/// One game's recorded detail, or `None` when nothing is recorded.
/// `registered` is the match's registered maindeck, when it is known: a
/// postboard list equal to it is no sideboarding. Game 1 is never
/// sideboarded (0.x refused it).
pub(crate) fn game_detail(
    game_number: u32,
    local_hand: Option<&str>,
    local_deck: Option<&str>,
    opponent_deck: Option<&str>,
    registered: Option<&BTreeMap<u32, u32>>,
) -> Option<ManualGameDetail> {
    let hand = local_hand.and_then(read_hand);
    let postboard_main = if game_number > 1 {
        local_deck.and_then(|text| postboard_main(text, registered))
    } else {
        None
    };
    let reveals = opponent_deck.map(read_reveals).unwrap_or_default();
    if hand.is_none() && postboard_main.is_none() && reveals.is_empty() {
        return None;
    }
    let (starting_hand, bottomed, mulligans) = match hand {
        Some(hand) => (Some(hand.kept), hand.bottomed, hand.mulligans),
        None => (None, Vec::new(), Vec::new()),
    };
    Some(ManualGameDetail {
        game_number,
        starting_hand,
        bottomed,
        mulligans,
        postboard_main,
        reveals,
    })
}

/// A recorded hand: `kept` is 0.x's name for the starting hand.
pub(crate) struct Hand {
    pub(crate) kept: Vec<u32>,
    pub(crate) bottomed: Vec<u32>,
    pub(crate) mulligans: Vec<Vec<u32>>,
}

/// JSON text as a value. A JSON string holding JSON (text written twice) is
/// read through.
pub(crate) fn parse(text: &str) -> Option<Value> {
    let value: Value = serde_json::from_str(text).ok()?;
    match value {
        Value::String(inner) => serde_json::from_str(&inner).ok(),
        other => Some(other),
    }
}

/// The hand of an `opening_hand_json`; `None` for NULL, or when any part of
/// it is unreadable. A missing list is empty, as 0.x read it (`?? []`).
pub(crate) fn read_hand(text: &str) -> Option<Hand> {
    let value = parse(text)?;
    let object = value.as_object()?;
    let kept = match object.get("kept") {
        Some(list) => ids(list)?,
        None => Vec::new(),
    };
    let bottomed = match object.get("bottomed") {
        Some(list) => ids(list)?,
        None => Vec::new(),
    };
    let mut mulligans = Vec::new();
    if let Some(hands) = object.get("mulligans") {
        for hand in hands.as_array()? {
            mulligans.push(ids(hand)?);
        }
    }
    Some(Hand {
        kept,
        bottomed,
        mulligans,
    })
}

/// A list of catalog ids (numbers, or text that is one).
fn ids(value: &Value) -> Option<Vec<u32>> {
    let mut ids = Vec::new();
    for item in value.as_array()? {
        ids.push(catalog_id(item)?);
    }
    Some(ids)
}

fn catalog_id(value: &Value) -> Option<u32> {
    let number = whole_number(value)?;
    u32::try_from(number).ok()
}

/// A whole number, or text that is one.
fn whole_number(value: &Value) -> Option<i64> {
    match value {
        Value::Number(number) => number.as_i64(),
        Value::String(text) => text.trim().parse().ok(),
        _ => None,
    }
}

/// 0.x's sideboard flag: a boolean, or the text `true` (0.x
/// `ResolveManualGameMaindeck::isSideboard`).
fn is_sideboard(value: Option<&Value>) -> bool {
    match value {
        Some(Value::Bool(flag)) => *flag,
        Some(Value::String(text)) => text.trim().eq_ignore_ascii_case("true"),
        _ => false,
    }
}

/// The `deck_json` rows v1 can read: a catalog id, the copies, and whether
/// they are in the sideboard. Rows without a readable id are skipped, as 0.x
/// skipped rows without an `mtgo_id`; a row without a readable quantity has
/// `missing_quantity` copies.
pub(crate) fn deck_rows(text: &str, missing_quantity: i64) -> Vec<(u32, i64, bool)> {
    let Some(value) = parse(text) else {
        return Vec::new();
    };
    let Some(rows) = value.as_array() else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for row in rows {
        let Some(object) = row.as_object() else {
            continue;
        };
        let Some(id) = object.get("mtgo_id").and_then(catalog_id) else {
            continue;
        };
        let quantity = object
            .get("quantity")
            .and_then(whole_number)
            .unwrap_or(missing_quantity);
        found.push((id, quantity, is_sideboard(object.get("sideboard"))));
    }
    found
}

/// The maindeck of a postboard list, by catalog id with copies above 0 (as
/// `manual_rules::postboard_main` stores it). `None` for no list (`[]`), or
/// one equal to the registered maindeck.
fn postboard_main(text: &str, registered: Option<&BTreeMap<u32, u32>>) -> Option<Vec<CardCount>> {
    let mut mains: BTreeMap<u32, i64> = BTreeMap::new();
    for (id, quantity, sideboard) in deck_rows(text, 0) {
        if !sideboard {
            *mains.entry(id).or_insert(0) += quantity;
        }
    }
    let mut copies: BTreeMap<u32, u32> = BTreeMap::new();
    for (id, quantity) in mains {
        if let Ok(quantity) = u32::try_from(quantity)
            && quantity > 0
        {
            copies.insert(id, quantity);
        }
    }
    if copies.is_empty() || registered == Some(&copies) {
        return None;
    }
    let mut main = Vec::new();
    for (catalog_id, quantity) in copies {
        main.push(CardCount {
            catalog_id,
            quantity,
        });
    }
    Some(main)
}

/// The opponent's revealed cards, in the order entered. Rows without at
/// least one copy are left out.
fn read_reveals(text: &str) -> Vec<CardCount> {
    let mut reveals = Vec::new();
    for (catalog_id, quantity, _) in deck_rows(text, 0) {
        if let Ok(quantity) = u32::try_from(quantity)
            && quantity > 0
        {
            reveals.push(CardCount {
                catalog_id,
                quantity,
            });
        }
    }
    reveals
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hand_reads_its_three_lists_and_text_ids() {
        let hand =
            read_hand(r#"{"kept":[1,"2"],"bottomed":[3],"mulligans":[[4,5],[]]}"#).expect("a hand");
        assert_eq!(hand.kept, vec![1, 2]);
        assert_eq!(hand.bottomed, vec![3]);
        assert_eq!(hand.mulligans, vec![vec![4, 5], Vec::new()]);
    }

    #[test]
    fn a_hand_written_twice_is_read_through() {
        let hand = read_hand(r#""{\"kept\":[7]}""#).expect("a hand");
        assert_eq!(hand.kept, vec![7]);
        assert!(hand.bottomed.is_empty());
        assert!(hand.mulligans.is_empty());
    }

    #[test]
    fn an_unreadable_hand_is_no_hand() {
        assert!(read_hand("null").is_none());
        assert!(read_hand("not json").is_none());
        assert!(read_hand(r#"{"kept":[1,"x"]}"#).is_none());
        assert!(read_hand(r#"{"kept":[-1]}"#).is_none());
    }

    #[test]
    fn a_postboard_list_keeps_its_maindeck_and_drops_one_equal_to_the_registered() {
        let text = r#"[{"mtgo_id":2,"quantity":3,"sideboard":"false"},
            {"mtgo_id":1,"quantity":4,"sideboard":false},
            {"mtgo_id":2,"quantity":1,"sideboard":"True"},
            {"mtgo_id":5,"quantity":0,"sideboard":false},
            {"quantity":9,"sideboard":false}]"#;
        let main = postboard_main(text, None).expect("a postboard list");
        assert_eq!(
            main,
            vec![
                CardCount {
                    catalog_id: 1,
                    quantity: 4
                },
                CardCount {
                    catalog_id: 2,
                    quantity: 3
                },
            ]
        );
        let registered = BTreeMap::from([(1, 4), (2, 3)]);
        assert_eq!(postboard_main(text, Some(&registered)), None);
        assert_eq!(postboard_main("[]", None), None);
    }

    #[test]
    fn game_one_is_never_sideboarded_and_an_empty_game_has_no_detail() {
        let list = r#"[{"mtgo_id":1,"quantity":4,"sideboard":false}]"#;
        assert_eq!(game_detail(1, None, Some(list), Some("[]"), None), None);
        let detail = game_detail(2, None, Some(list), None, None).expect("sideboarded");
        assert_eq!(
            detail.postboard_main,
            Some(vec![CardCount {
                catalog_id: 1,
                quantity: 4
            }])
        );
    }
}
