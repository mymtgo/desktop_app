//! Archetype rules ported from the 0.x app and the API server (archetypes
//! spec section 3).
//!
//! `format` gives the archetypes format key of an MTGO format code,
//! `colors` the canonical colour identity, `names` the card name keys the
//! server's rules read. `definition` parses the server's definition JSON,
//! `rules` evaluates its conditions, `score` is 0.x's local estimate and
//! `detect` the decision that puts them together. `manual` holds the manual
//! archetype rules: uuids, variant hashes and the merge, unmerge and
//! reassign guards. `guides` holds the sideboard guide and matchup note
//! rules. `metagame` is the Metagame page's tile arithmetic and the
//! archetype detail's cards seen.
//!
//! Pure: no filesystem, clock, network or database.

pub mod changes;
pub mod colors;
pub mod commander;
pub mod definition;
pub mod detect;
pub mod format;
pub mod guides;
pub mod manual;
pub mod metagame;
pub mod names;
pub mod offline;
pub mod rules;
pub mod score;

pub use changes::{
    ChangePlan, DefinitionsCheck, HeldDefinition, Redetect, plan_changes, plan_slices,
};
pub use colors::{colors_of, deck_identity, normalize_colors};
pub use commander::{
    COMMANDER_NO_COLOURS, COMMANDER_NO_DELETE, commander_colors, commander_name, commander_uuid,
};
pub use definition::{Condition, ConditionKind, Definition, DefinitionCard, parse_definition};
pub use detect::{
    Decision, FALLBACK_CONFIDENCE, HOMEBREW_UUID, LOCAL_CONFIDENCE_THRESHOLD, LocalAnswers,
    ROGUE_UUID, complete_maindeck, decide, decide_after_empty_estimate, local_answers, merge_hop,
};
pub use format::{format_key, format_label, is_limited};
pub use manual::{
    DUPLICATE_VARIANT, GuardArchetype, ReassignOutcome, check_merge, check_new_variant,
    check_reassign, check_unmerge, manual_uuid, variant_hash,
};
pub use names::{name_key, name_keys};
pub use offline::holdable_offline;
pub use rules::{fallback_match, rules_match, satisfies};
pub use score::{Estimate, score};

/// One card seen for a seat: a game's deck list or an opponent's reveals,
/// joined to the local `cards` row when there is one.
#[derive(Debug, Clone, PartialEq)]
pub struct ObservedCard {
    pub catalog_id: i64,
    pub quantity: u32,
    pub sideboard: bool,
    pub oracle_id: Option<String>,
    pub name: Option<String>,
    pub card_type: Option<String>,
    pub color_identity: Option<String>,
    /// The card's mana cost (`{1}{U}`); none for a land, and for a modal or
    /// transforming card, as the API stores them.
    pub mana_cost: Option<String>,
}

/// One card of an archetype's list (`archetype_cards`), with the card type
/// from the local `cards` row (the scorer drops basic lands by type).
#[derive(Debug, Clone, PartialEq)]
pub struct ListCard {
    pub catalog_id: i64,
    pub oracle_id: Option<String>,
    pub card_type: Option<String>,
    pub quantity: u32,
    pub sideboard: bool,
}

/// Where an archetype row comes from (`archetypes.source`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Downloaded,
    Manual,
    Fallback,
    Commander,
}

impl Source {
    /// The `archetypes.source` text.
    pub fn as_str(self) -> &'static str {
        match self {
            Source::Downloaded => "downloaded",
            Source::Manual => "manual",
            Source::Fallback => "fallback",
            Source::Commander => "commander",
        }
    }

    /// Reads `archetypes.source`; `None` for any other text.
    pub fn parse(text: &str) -> Option<Source> {
        match text {
            "downloaded" => Some(Source::Downloaded),
            "manual" => Some(Source::Manual),
            "fallback" => Some(Source::Fallback),
            "commander" => Some(Source::Commander),
            _ => None,
        }
    }
}

/// One held archetype detection may answer with: a downloaded one (its
/// definition and list 1) or a manual one (one list per variant).
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub archetype_id: i64,
    pub uuid: String,
    pub source: Source,
    pub color_identity: Option<String>,
    pub definition: Option<Definition>,
    /// The archetype's lists in list order (list 1 first).
    pub lists: Vec<Vec<ListCard>>,
}
