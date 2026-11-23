//! The opponent's revealed cards with game-log-only casts merged in (0.x
//! `BuildMatchGameData::mergeLogOnlyCards`).
//!
//! Cards can leave every visible zone before the last frame (shuffled into
//! the library, bounced to hand), and multi-face casts are logged under the
//! face's catalog id while frames carry the parent printing. Both would
//! otherwise vanish from the reveals list.

use std::collections::HashSet;

/// One revealed card: the printing, its display name and how many were seen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevealedCard {
    pub catalog_id: u32,
    pub name: String,
    pub quantity: u32,
}

/// A card the opponent cast or played according to the game log. `name` is
/// the card's known name, or the log's name when the catalog id is unknown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogCard {
    pub catalog_id: u32,
    pub name: String,
}

/// `seen` (from the last frame, names already resolved) followed by each log
/// card none of whose faces is already shown, once each, with quantity 1.
/// Names compare by face (split on `" // "`), trimmed and lowercased.
pub fn merge_log_only_cards(seen: Vec<RevealedCard>, log_cards: &[LogCard]) -> Vec<RevealedCard> {
    let mut merged = seen;
    let mut known_faces: HashSet<String> = HashSet::new();
    for card in &merged {
        known_faces.extend(faces(&card.name));
    }

    for log_card in log_cards {
        let card_faces = faces(&log_card.name);
        if card_faces.iter().any(|face| known_faces.contains(face)) {
            continue;
        }
        known_faces.extend(card_faces);
        merged.push(RevealedCard {
            catalog_id: log_card.catalog_id,
            name: log_card.name.clone(),
            quantity: 1,
        });
    }
    merged
}

/// A card name's faces, lowercased and trimmed (`"Fire // Ice"` gives `fire`, `ice`).
fn faces(name: &str) -> Vec<String> {
    name.to_lowercase()
        .split(" // ")
        .map(|face| face.trim().to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Ports of the reveal cases in 0.x
    // tests/Feature/Actions/Matches/BuildMatchGameDataTest.php.

    fn revealed(catalog_id: u32, name: &str, quantity: u32) -> RevealedCard {
        RevealedCard {
            catalog_id,
            name: name.to_string(),
            quantity,
        }
    }

    fn log_card(catalog_id: u32, name: &str) -> LogCard {
        LogCard {
            catalog_id,
            name: name.to_string(),
        }
    }

    #[test]
    fn includes_opponent_cards_seen_only_in_the_game_log() {
        let seen = vec![revealed(8001, "Swamp", 2)];

        let merged = merge_log_only_cards(seen, &[log_card(8002, "Lembas")]);

        assert_eq!(
            merged,
            vec![revealed(8001, "Swamp", 2), revealed(8002, "Lembas", 1)]
        );
    }

    #[test]
    fn does_not_duplicate_a_log_card_already_visible_under_its_multi_face_parent_name() {
        let seen = vec![revealed(9001, "Brazen Borrower // Petty Theft", 1)];

        let merged = merge_log_only_cards(seen.clone(), &[log_card(9002, "Petty Theft")]);

        assert_eq!(merged, seen);
    }

    #[test]
    fn falls_back_to_the_log_provided_name_when_the_log_card_id_is_unknown() {
        // The caller passes the log's name for an unknown id.
        let merged = merge_log_only_cards(Vec::new(), &[log_card(7777, "Once Upon a Time")]);

        assert_eq!(merged, vec![revealed(7777, "Once Upon a Time", 1)]);
    }

    #[test]
    fn adds_each_log_only_card_once_and_compares_faces_case_insensitively() {
        let seen = vec![revealed(1, "LIGHTNING BOLT", 1)];
        let log = [
            log_card(2, "lightning bolt"),
            log_card(3, "Fire // Ice"),
            log_card(4, "Ice"),
            log_card(3, "Fire // Ice"),
            log_card(5, "Ébullition"),
            log_card(6, "ébullition"),
        ];

        let merged = merge_log_only_cards(seen, &log);

        assert_eq!(
            merged,
            vec![
                revealed(1, "LIGHTNING BOLT", 1),
                revealed(3, "Fire // Ice", 1),
                revealed(5, "Ébullition", 1),
            ]
        );
    }
}
