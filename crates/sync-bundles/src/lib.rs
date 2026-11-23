//! Cross-device sync bundles without any IO (07 spec section 10, survey
//! section 12): PHP-identical canonical JSON and its hash, the client ids,
//! bundle datetimes, the deck, match and league bundles, and the carry (the
//! bundle parts v1 has no home for).
//!
//! Like 0.x's `CanonicalJson` and `DeckBundleBuilder`: two devices that
//! build the same row must produce the same bytes, so every byte here is
//! proven against goldens exported from 0.x (`fixtures/parity/sync_*`).

pub mod canonical;
pub mod carry;
pub mod client_id;
pub mod datetime;
pub mod deck;
pub mod endstep_cards;
pub mod league;
pub mod loose;
pub mod match_bundle;
pub mod platform;
pub mod sources;

pub use canonical::{CANONICAL_VERSION, bundle_hash, canonical_json};
pub use carry::{
    Carry, DECK_CARRY, LEAGUE_CARRY, MATCH_CARRY, UNPLACED_ARCHETYPE_UUID, UNPLACED_ARCHETYPES,
    UNPLACED_GUIDES, UNPLACED_NOTES,
};
pub use client_id::{deck_client_id, league_client_id};
pub use datetime::{bundle_datetime, parse_bundle_datetime};
pub use deck::{
    BundleError, DeckBundleInput, DeckParts, GuideCardInput, GuideInput, NoteInput, deck_bundle,
    deck_sidecar, guides_section, notes_section, read_deck_bundle, read_guides, read_notes,
};
pub use endstep_cards::EndstepCardRef;
pub use league::{PulledLeagueFields, USER_STATES, league_player_fields};
pub use match_bundle::{
    ArenaPlayerFields, EndstepRankedFields, PulledPlayerFields, match_endstep_ranked,
    match_game_structure, match_player_fields,
};
pub use platform::BundlePlatform;
pub use sources::{
    ArenaGameSource, ArenaSourceMessage, ArenaSources, EndstepSourceUnit, EndstepSources,
    MatchSources, read_sources, sources_section,
};
