//! The offline archetype download (2026-10-27 offline download design): which
//! server definitions an offline player keeps as Custom archetypes.

use crate::{Definition, is_limited};

/// The definitions the download holds, in the order given, one per uuid
/// (the first wins). A fallback definition is dropped (as a Custom key list
/// it would compete with every specific archetype), and so is one without a
/// format or of a limited format (a Custom row needs a constructed format).
/// So is one without cards: a Custom row is scored by its cards only, so it
/// could never match a deck (a rules-only definition stays the server's).
pub fn holdable_offline(definitions: Vec<Definition>) -> Vec<Definition> {
    let mut kept: Vec<Definition> = Vec::new();
    for definition in definitions {
        if definition.fallback || definition.cards.is_empty() {
            continue;
        }
        let Some(format) = definition.format.as_deref().map(str::trim) else {
            continue;
        };
        if format.is_empty() || is_limited(format) {
            continue;
        }
        if kept.iter().any(|held| held.uuid == definition.uuid) {
            continue;
        }
        kept.push(definition);
    }
    kept
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DefinitionCard;

    fn named(uuid: &str, format: Option<&str>) -> Definition {
        Definition {
            uuid: uuid.to_string(),
            name: format!("Q{uuid}"),
            format: format.map(str::to_string),
            cards: vec![DefinitionCard {
                mtgo_id: 1001,
                quantity: 4,
                ..DefinitionCard::default()
            }],
            ..Definition::default()
        }
    }

    #[test]
    fn holdable_offline_drops_fallbacks_formatless_and_limited() {
        let fallback = Definition {
            fallback: true,
            ..named("q-fallback", Some("modern"))
        };
        let kept = holdable_offline(vec![
            fallback,
            named("q-none", None),
            named("q-blank", Some(" ")),
            named("q-draft", Some("DMKM")),
            named("q-one", Some("modern")),
            Definition {
                name: "Qsecond".to_string(),
                ..named("q-one", Some("modern"))
            },
        ]);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].uuid, "q-one");
        assert_eq!(kept[0].name, "Qq-one", "the first of a uuid wins");
    }

    #[test]
    fn holdable_offline_drops_definitions_without_cards() {
        let rules_only = Definition {
            cards: Vec::new(),
            conditions: vec![crate::Condition {
                kind: crate::ConditionKind::default(),
                cards: vec!["Qcard".to_string()],
            }],
            ..named("q-rules", Some("modern"))
        };
        let kept = holdable_offline(vec![rules_only, named("q-cards", Some("modern"))]);
        let uuids: Vec<&str> = kept.iter().map(|held| held.uuid.as_str()).collect();
        assert_eq!(uuids, vec!["q-cards"]);
    }
}
