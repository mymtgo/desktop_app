//! The server's definition of one archetype (spec section 16.3): identity,
//! version, the rules (`conditions`, `rank`), the fallback data and the key
//! list (`cards`).
//!
//! Every key may be missing: a definition built from the `/decklist`
//! fallback has only `cards` (spec section 5). The store keeps the
//! definition as `serde_json::to_string(&definition)`.

use serde::{Deserialize, Serialize};

/// The API's default for `fallback_min_overlap` (survey section 8).
const DEFAULT_MIN_OVERLAP: f64 = 0.1;

/// The twelve condition types of the API's `ConditionEvaluator`. Type names
/// are read case-insensitively; any other name is kept as `Unknown` and
/// never passes (the API `default => false`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "String", into = "String")]
pub enum ConditionKind {
    InMainboard,
    InSideboard,
    InMainOrSideboard,
    OneOrMoreInMainboard,
    OneOrMoreInSideboard,
    OneOrMoreInMainOrSideboard,
    TwoOrMoreInMainboard,
    TwoOrMoreInSideboard,
    TwoOrMoreInMainOrSideboard,
    DoesNotContain,
    DoesNotContainMainboard,
    DoesNotContainSideboard,
    Unknown(String),
}

impl Default for ConditionKind {
    fn default() -> Self {
        ConditionKind::Unknown(String::new())
    }
}

const KNOWN_KINDS: [ConditionKind; 12] = [
    ConditionKind::InMainboard,
    ConditionKind::InSideboard,
    ConditionKind::InMainOrSideboard,
    ConditionKind::OneOrMoreInMainboard,
    ConditionKind::OneOrMoreInSideboard,
    ConditionKind::OneOrMoreInMainOrSideboard,
    ConditionKind::TwoOrMoreInMainboard,
    ConditionKind::TwoOrMoreInSideboard,
    ConditionKind::TwoOrMoreInMainOrSideboard,
    ConditionKind::DoesNotContain,
    ConditionKind::DoesNotContainMainboard,
    ConditionKind::DoesNotContainSideboard,
];

impl ConditionKind {
    /// The type name as the server writes it.
    pub fn name(&self) -> String {
        let name = match self {
            ConditionKind::InMainboard => "InMainboard",
            ConditionKind::InSideboard => "InSideboard",
            ConditionKind::InMainOrSideboard => "InMainOrSideboard",
            ConditionKind::OneOrMoreInMainboard => "OneOrMoreInMainboard",
            ConditionKind::OneOrMoreInSideboard => "OneOrMoreInSideboard",
            ConditionKind::OneOrMoreInMainOrSideboard => "OneOrMoreInMainOrSideboard",
            ConditionKind::TwoOrMoreInMainboard => "TwoOrMoreInMainboard",
            ConditionKind::TwoOrMoreInSideboard => "TwoOrMoreInSideboard",
            ConditionKind::TwoOrMoreInMainOrSideboard => "TwoOrMoreInMainOrSideboard",
            ConditionKind::DoesNotContain => "DoesNotContain",
            ConditionKind::DoesNotContainMainboard => "DoesNotContainMainboard",
            ConditionKind::DoesNotContainSideboard => "DoesNotContainSideboard",
            ConditionKind::Unknown(text) => text.as_str(),
        };
        name.to_string()
    }
}

impl From<String> for ConditionKind {
    fn from(text: String) -> ConditionKind {
        let wanted = text.to_lowercase();
        for kind in KNOWN_KINDS {
            if kind.name().to_lowercase() == wanted {
                return kind;
            }
        }
        ConditionKind::Unknown(text)
    }
}

impl From<ConditionKind> for String {
    fn from(kind: ConditionKind) -> String {
        kind.name()
    }
}

/// One rule: a type and the card names it reads, as written by the server.
/// A condition without a type is `Unknown("")` and fails on its own (the API
/// `$condition['Type'] ?? ''`), so the rest of the definition still loads.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Condition {
    #[serde(rename = "type", default)]
    pub kind: ConditionKind,
    #[serde(default)]
    pub cards: Vec<String>,
}

/// One card of the definition's key list (the latest deck version).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DefinitionCard {
    pub mtgo_id: i64,
    /// The MTG Arena id (`grpId`) of a card MTGO does not have (`mtgo_id`
    /// is 0 then). Absent in answers that predate Arena formats.
    pub arena_id: Option<i64>,
    pub oracle_id: Option<String>,
    pub name: Option<String>,
    pub quantity: u32,
    pub sideboard: bool,
}

/// One archetype's definition (spec section 16.3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Definition {
    pub uuid: String,
    pub name: String,
    pub format: Option<String>,
    pub color_identity: Option<String>,
    pub version: Option<String>,
    /// Compared as a list, higher wins; never used for arithmetic.
    pub rank: Vec<f64>,
    pub conditions: Vec<Condition>,
    pub fallback: bool,
    pub common_cards: Vec<String>,
    pub min_overlap: f64,
    /// Shared by archetypes that differ only by colour.
    pub colour_fork: Option<String>,
    pub cards: Vec<DefinitionCard>,
}

impl Default for Definition {
    fn default() -> Definition {
        Definition {
            uuid: String::new(),
            name: String::new(),
            format: None,
            color_identity: None,
            version: None,
            rank: Vec::new(),
            conditions: Vec::new(),
            fallback: false,
            common_cards: Vec::new(),
            min_overlap: DEFAULT_MIN_OVERLAP,
            colour_fork: None,
            cards: Vec::new(),
        }
    }
}

/// Reads one definition from the server's JSON (or from the stored copy).
pub fn parse_definition(json: &str) -> Result<Definition, String> {
    serde_json::from_str(json).map_err(|error| format!("invalid archetype definition: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_definition_card_reads_an_arena_id() {
        let json = r#"{"mtgo_id": 0, "arena_id": 91538, "oracle_id": "o", "name": "A-Card", "quantity": 4, "sideboard": false}"#;
        let card: DefinitionCard = serde_json::from_str(json).expect("parses");
        assert_eq!(card.arena_id, Some(91538));
        assert_eq!(card.mtgo_id, 0);

        let old = r#"{"mtgo_id": 12345, "oracle_id": "o", "name": "Card", "quantity": 4, "sideboard": false}"#;
        let card: DefinitionCard = serde_json::from_str(old).expect("parses");
        assert_eq!(card.arena_id, None);
    }
}
