//! Limited rules ported from the 0.x app (limited spec section 4).
//!
//! `draft` folds one account's draft events, in log order, into drafts: the
//! picks with their pack contents, reservations and timings, and each draft's
//! state (0.x `ApplyDraftEvent`, `ReconcileDraftFromEnded` and
//! `AbandonStaleDrafts`). Drafts are derived, never stored: the same events
//! always give the same drafts, whether they are folded one by one or all at
//! once.
//!
//! `payload` reads the JSON payloads of draft lines, tolerating MTGO's missing
//! closing brackets as 0.x's `RepairJson` does.
//!
//! Pure: no filesystem, clock, network or database. Callers pass in the
//! events read from the database, the OS timezone and `now`.

pub mod draft;
pub mod payload;

pub use draft::{
    ABANDON_AFTER_MS, Draft, DraftEventInput, DraftFold, DraftState, Pick, Reservation,
    fold_drafts, pool_counts,
};

// Draft and pool analytics (limited spec sections 5 and 6): pure functions
// over drafts, registered decks and a card facts map keyed by catalog id.
pub mod cards;
pub mod colours;
pub mod evolution;
pub mod index;
pub mod pool;
pub mod set_code;
pub mod sets;
pub mod signals;
pub mod timings;
pub mod wheel;

pub use cards::{
    COLORS, CardFacts, CardFactsMap, CardRowsInput, CardRowsSummary, CastStats, CrossDraftStats,
    LimitedCardRow, LimitedCardRows, OtherDraft, card_name, cross_draft_card_stats, is_basic,
    limited_card_rows,
};
pub use colours::{colour_combination_name, run_deck_colours};
pub use evolution::{
    CardQuantity, CardStatus, CompletedMatch, DeckEvolution, EvolutionInput, EvolutionSummary,
    GameChange, MatchChanges, PoolCard, PoolGroup, RegisteredVersion, ZoneDiff, deck_evolution,
    pool_statuses,
};
pub use index::{IndexRun, LimitedIndexKpis, limited_index_kpis};
pub use pool::{
    MIN_COVERAGE, MIN_POOL_OVERLAP, RegisteredCard, RegisteredDeck, RunPool, deck_fits_pool,
    main_deck, registered_cards_from_json, registered_deck_matches_pool, run_pool,
    sealed_pack_count, sealed_pool,
};
pub use set_code::{
    format_name, panel_play_format, resolve_set_code, set_code_from_picks,
    set_code_from_play_format,
};
pub use sets::{SetCount, SetMatch, matches_by_set};
pub use signals::{ColorSignal, draft_signals};
pub use timings::{
    DraftPickStats, INDECISIVE_RESERVATIONS, PickTiming, TimingSummary, draft_pick_stats,
    pick_timing, timing_summary,
};
pub use wheel::{SeenFacts, WheelView, seats, seen_wheel, wheel_for_pick};
