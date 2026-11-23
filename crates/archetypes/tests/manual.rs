//! Manual archetypes: 0.x `MergeArchetypeTest` (seven), `UnmergeArchetypeTest`
//! (the guard), `ReassignArchetypeVariantTest` (the guards and the merge of an
//! emptied source), `AddArchetypeVariantTest` (the three duplicate cases),
//! `ResolveMergedArchetypeTest` (the one-hop cases) and the manual uuid.

use archetypes::{
    DUPLICATE_VARIANT, GuardArchetype, ListCard, check_merge, check_new_variant, check_reassign,
    check_unmerge, manual_uuid, merge_hop, variant_hash,
};

fn archetype(id: i64, format: &str) -> GuardArchetype {
    GuardArchetype {
        id,
        format: Some(format.to_string()),
        fallback: false,
        merged_into_id: None,
    }
}

fn fallback(id: i64) -> GuardArchetype {
    GuardArchetype {
        id,
        format: None,
        fallback: true,
        merged_into_id: None,
    }
}

fn merged(id: i64, format: &str, into: i64) -> GuardArchetype {
    GuardArchetype {
        merged_into_id: Some(into),
        ..archetype(id, format)
    }
}

#[test]
fn sets_merged_into_id_on_source() {
    assert_eq!(
        check_merge(&archetype(2, "modern"), &archetype(1, "modern")),
        Ok(())
    );
}

#[test]
fn refuses_cross_format_merge() {
    assert_eq!(
        check_merge(&archetype(2, "pioneer"), &archetype(1, "modern")),
        Err("Parent must share the same format.".to_string())
    );
}

#[test]
fn refuses_merge_with_fallback_archetype_as_source() {
    // 0.x's fallback had a format in this test; v1's fallbacks have none, so
    // the format check would answer first. Both refuse; 0.x's message order
    // is kept for a fallback that shares the format.
    let mut source = fallback(2);
    source.format = Some("modern".to_string());
    assert_eq!(
        check_merge(&source, &archetype(1, "modern")),
        Err("Fallback archetypes cannot be merged.".to_string())
    );
}

#[test]
fn refuses_merge_with_fallback_archetype_as_parent() {
    let mut parent = fallback(1);
    parent.format = Some("modern".to_string());
    assert_eq!(
        check_merge(&archetype(2, "modern"), &parent),
        Err("Fallback archetypes cannot be merged.".to_string())
    );
}

#[test]
fn refuses_merge_if_source_already_merged() {
    assert_eq!(
        check_merge(&merged(3, "modern", 1), &archetype(2, "modern")),
        Err("This archetype is already merged.".to_string())
    );
}

#[test]
fn refuses_merge_if_parent_already_merged() {
    assert_eq!(
        check_merge(&archetype(3, "modern"), &merged(2, "modern", 1)),
        Err("Parent archetype is already merged.".to_string())
    );
}

#[test]
fn refuses_self_merge() {
    let one = archetype(1, "modern");
    assert_eq!(
        check_merge(&one, &one),
        Err("Cannot merge an archetype into itself.".to_string())
    );
}

#[test]
fn unmerge_refuses_a_standalone_archetype() {
    assert_eq!(check_unmerge(&merged(2, "modern", 1)), Ok(()));
    assert_eq!(
        check_unmerge(&archetype(2, "modern")),
        Err("Archetype is not merged.".to_string())
    );
}

#[test]
fn reassign_moves_a_variant_and_keeps_a_source_that_still_has_variants() {
    assert_eq!(
        check_reassign(&archetype(1, "modern"), &archetype(2, "modern"), 2),
        Ok(archetypes::ReassignOutcome {
            merge_source: false
        })
    );
}

#[test]
fn reassign_flags_source_as_merged_into_target_when_source_has_no_variants_left() {
    assert_eq!(
        check_reassign(&archetype(1, "modern"), &archetype(2, "modern"), 1),
        Ok(archetypes::ReassignOutcome { merge_source: true })
    );
}

#[test]
fn reassign_refuses_cross_format() {
    assert_eq!(
        check_reassign(&archetype(1, "modern"), &archetype(2, "pioneer"), 2),
        Err("Target must share the same format.".to_string())
    );
}

#[test]
fn reassign_refuses_a_fallback_target() {
    let mut target = fallback(2);
    target.format = Some("modern".to_string());
    assert_eq!(
        check_reassign(&archetype(1, "modern"), &target, 2),
        Err("Cannot reassign to a fallback archetype.".to_string())
    );
}

#[test]
fn reassign_refuses_a_merged_target() {
    assert_eq!(
        check_reassign(&archetype(1, "modern"), &merged(2, "modern", 3), 2),
        Err("Target archetype is already merged.".to_string())
    );
}

#[test]
fn reassign_refuses_when_source_is_already_merged() {
    assert_eq!(
        check_reassign(&merged(1, "modern", 3), &archetype(2, "modern"), 2),
        Err("Source archetype is already merged.".to_string())
    );
}

#[test]
fn reassign_refuses_self() {
    let one = archetype(1, "modern");
    assert_eq!(
        check_reassign(&one, &one, 2),
        Err("Cannot reassign to the same archetype.".to_string())
    );
}

fn list_card(catalog_id: i64, quantity: u32, sideboard: bool) -> ListCard {
    ListCard {
        catalog_id,
        oracle_id: Some(format!("q-oracle-{catalog_id}")),
        card_type: None,
        quantity,
        sideboard,
    }
}

#[test]
fn throws_duplicate_variant_when_an_identical_variant_exists() {
    let existing = vec![vec![list_card(1, 4, false), list_card(2, 2, true)]];
    let same_in_another_order = vec![list_card(2, 2, true), list_card(1, 4, false)];

    assert_eq!(
        check_new_variant(&existing, &same_in_another_order),
        Err(DUPLICATE_VARIANT.to_string())
    );
}

#[test]
fn allows_two_variants_that_differ_only_in_quantity() {
    let existing = vec![vec![list_card(1, 4, false)]];

    assert_eq!(
        check_new_variant(&existing, &[list_card(1, 3, false)]),
        Ok(())
    );
}

#[test]
fn treats_sideboard_differences_as_distinct_variants() {
    let existing = vec![vec![list_card(1, 4, false)]];

    assert_eq!(
        check_new_variant(&existing, &[list_card(1, 4, true)]),
        Ok(())
    );
}

#[test]
fn a_variant_hash_is_the_sorted_catalog_board_quantity_rows() {
    let cards = vec![
        list_card(20, 1, true),
        list_card(3, 4, false),
        list_card(20, 2, false),
    ];

    assert_eq!(variant_hash(&cards), "20:0:2|20:1:1|3:0:4");
}

#[test]
fn manual_uuid_has_the_device_prefix() {
    let random = [0xff; 16];

    let uuid = manual_uuid(Some("abcdef1234567890"), random);
    assert_eq!(uuid, "abcdef12-ffffffff-ffff-4fff-bfff-ffffffffffff");

    let no_device = manual_uuid(None, [0; 16]);
    assert_eq!(no_device, "00000000-00000000-0000-4000-8000-000000000000");

    let short_device = manual_uuid(Some("q12"), [0; 16]);
    assert!(short_device.starts_with("q12-"));
}

fn merged_into(id: i64) -> Option<i64> {
    match id {
        2 => Some(1), // middle merged into top
        3 => Some(2), // bottom merged into middle
        _ => None,
    }
}

#[test]
fn passes_through_when_archetype_has_no_merged_into_id() {
    assert_eq!(merge_hop(1, merged_into), 1);
}

#[test]
fn passes_through_when_archetype_does_not_exist() {
    assert_eq!(merge_hop(999_999, merged_into), 999_999);
}

#[test]
fn redirects_to_parent() {
    assert_eq!(merge_hop(2, merged_into), 1);
}

#[test]
fn does_not_chase_chains_beyond_one_hop() {
    assert_eq!(merge_hop(3, merged_into), 2);
}
