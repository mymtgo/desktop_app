//! Catalog id plausibility (0.x `CreateMissingCards::isPlausibleCatalogId`).

/// Highest value accepted as an MTGO catalog id.
///
/// Catalog ids are five- or six-digit and climb slowly, so this sits well
/// above anything MTGO has issued. It catches values that are not catalog ids
/// at all: a misread log line or a bad decode once put numbers like
/// 89215000000 into 0.x's card table, and a stub for one can never resolve,
/// so it would count as missing card data forever.
pub const MAX_CATALOG_ID: u32 = 1_000_000;

/// Whether `id` could be an MTGO catalog id: a whole number from 0 to
/// `MAX_CATALOG_ID`.
pub fn plausible(id: i64) -> bool {
    id >= 0 && id <= i64::from(MAX_CATALOG_ID)
}

/// Arena cards are stored at `catalog_id = ARENA_ID_OFFSET + grpId`, so they
/// never collide with MTGO catalog ids (which stay below `MAX_CATALOG_ID`).
pub const ARENA_ID_OFFSET: u32 = 100_000_000;

/// An Arena card's own id (`grpId` in Arena's log).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GrpId(pub u32);

/// The local catalog id of an Arena card.
pub fn arena_catalog_id(grp: GrpId) -> u32 {
    ARENA_ID_OFFSET + grp.0
}

/// The catalog id of an Arena card, or `None` when `grp` is too large to be
/// an Arena id (a hostile log), so the result could not be read back by
/// `grp_id_of`.
pub fn checked_arena_catalog_id(grp: u32) -> Option<u32> {
    if grp < ARENA_ID_OFFSET {
        Some(arena_catalog_id(GrpId(grp)))
    } else {
        None
    }
}

/// The Arena id behind a catalog id: `Some` only from the offset up to
/// twice it, so an Endstep id (at `ENDSTEP_ID_OFFSET` and above) is never
/// read as an Arena card.
pub fn grp_id_of(catalog_id: i64) -> Option<GrpId> {
    let grp = catalog_id.checked_sub(i64::from(ARENA_ID_OFFSET))?;
    let grp = u32::try_from(grp).ok()?;
    if grp < ARENA_ID_OFFSET {
        Some(GrpId(grp))
    } else {
        None
    }
}

/// Endstep cards are stored at `catalog_id = ENDSTEP_ID_OFFSET + row id` of
/// their `endstep_cards` row (Endstep has no card ids of its own), above
/// Arena's range, so they collide with neither MTGO nor Arena ids.
pub const ENDSTEP_ID_OFFSET: u32 = 200_000_000;

/// The local catalog id of the Endstep card in `endstep_cards` row `row`.
pub fn endstep_catalog_id(row: u32) -> u32 {
    ENDSTEP_ID_OFFSET + row
}

/// The `endstep_cards` row behind a catalog id: `Some` only from
/// `ENDSTEP_ID_OFFSET` up to twice it.
pub fn endstep_row_of(catalog_id: i64) -> Option<u32> {
    let row = catalog_id.checked_sub(i64::from(ENDSTEP_ID_OFFSET))?;
    let row = u32::try_from(row).ok()?;
    if row < ENDSTEP_ID_OFFSET {
        Some(row)
    } else {
        None
    }
}

/// Whether a `cards` row may be stored for `id`: a plausible MTGO id, an
/// Arena id or an Endstep id (each range is bounded, so a misread number
/// like 590,000,000 is still refused).
pub fn storable(id: i64) -> bool {
    plausible(id) || grp_id_of(id).is_some() || endstep_row_of(id).is_some()
}

/// The plausible ids (as catalog ids) and the refused ones, each in input
/// order. Duplicates are kept: callers decide what a repeat means.
pub fn split_plausible(ids: &[i64]) -> (Vec<u32>, Vec<i64>) {
    let mut accepted = Vec::new();
    let mut refused = Vec::new();
    for &id in ids {
        match u32::try_from(id) {
            Ok(catalog_id) if plausible(id) => accepted.push(catalog_id),
            _ => refused.push(id),
        }
    }
    (accepted, refused)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plausible_id_is_zero_to_the_maximum() {
        assert!(plausible(0));
        assert!(plausible(126_519));
        assert!(plausible(1_000_000));
        assert!(!plausible(1_000_001));
        assert!(!plausible(-5));
        assert!(!plausible(89_215_000_000));
    }

    #[test]
    fn arena_catalog_id_adds_the_offset() {
        assert_eq!(arena_catalog_id(GrpId(91_538)), 100_091_538);
    }

    #[test]
    fn grp_id_of_reverses_it() {
        assert_eq!(grp_id_of(100_091_538), Some(GrpId(91_538)));
        assert_eq!(grp_id_of(i64::from(ARENA_ID_OFFSET)), Some(GrpId(0)));
    }

    #[test]
    fn grp_id_of_is_none_below_the_offset() {
        assert_eq!(grp_id_of(126_519), None);
        assert_eq!(grp_id_of(99_999_999), None);
        assert_eq!(grp_id_of(-1), None);
    }

    #[test]
    fn storable_accepts_arena_ids_and_mtgo_ids_only() {
        assert!(storable(5));
        assert!(storable(100_091_538));
        assert!(!storable(-1));
        assert!(!storable(1_000_001));
        assert!(!storable(99_999_999));
        assert!(!storable(590_000_000));
        assert!(!storable(i64::from(u32::MAX) + 1));
    }

    #[test]
    fn an_endstep_id_is_never_arena_or_mtgo() {
        assert_eq!(grp_id_of(200_000_001), None);
        assert_eq!(endstep_row_of(200_000_001), Some(1));
        assert!(!plausible(200_000_001));
        assert!(storable(200_000_001));
        assert_eq!(endstep_row_of(100_000_001), None);
        assert_eq!(endstep_row_of(400_000_000), None);
        assert_eq!(grp_id_of(100_000_001), Some(GrpId(1)));
        assert_eq!(endstep_catalog_id(1), 200_000_001);
    }

    #[test]
    fn split_keeps_input_order_on_both_sides() {
        let (accepted, refused) = split_plausible(&[89_215_000_000, 126_519, 590_000_000, 7, -5]);
        assert_eq!(accepted, vec![126_519, 7]);
        assert_eq!(refused, vec![89_215_000_000, 590_000_000, -5]);
    }
}
