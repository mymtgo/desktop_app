//! Card rules ported from the 0.x app (cards spec section 3).
//!
//! `catalog` decides which numbers are plausible MTGO catalog ids, so only
//! those become card stubs (0.x `CreateMissingCards`). `images` names the
//! local image files v1 owns, formats their size, checks the image host
//! and validates a move of the images folder. `colors` is the one canonical
//! colour identity form (WUBRG order) every page shows.
//!
//! Pure: no filesystem, clock, network or database.

pub mod catalog;
pub mod colors;
pub mod images;
pub mod standout;
pub mod stats;
pub mod tokens;

pub use catalog::{
    ARENA_ID_OFFSET, ENDSTEP_ID_OFFSET, GrpId, MAX_CATALOG_ID, arena_catalog_id,
    checked_arena_catalog_id, endstep_catalog_id, endstep_row_of, grp_id_of, plausible,
    split_plausible, storable,
};
pub use colors::normalize_colors;
