//! Manual archetypes (survey section 4): the uuid, variant duplicates and
//! the merge, unmerge and reassign guards with 0.x's messages.

use crate::ListCard;

/// 0.x `DuplicateVariantException`'s message.
pub const DUPLICATE_VARIANT: &str = "This variant matches an existing one.";

/// What the guards read of an archetype row.
#[derive(Debug, Clone, PartialEq)]
pub struct GuardArchetype {
    pub id: i64,
    pub format: Option<String>,
    pub fallback: bool,
    pub merged_into_id: Option<i64>,
}

/// What a reassign does besides moving the variant.
#[derive(Debug, Clone, PartialEq)]
pub struct ReassignOutcome {
    /// The source has no variant left and is merged into the target (0.x).
    pub merge_source: bool,
}

/// A manual archetype's uuid: the first 8 characters of the device id
/// (`00000000` without one), a dash, then a version 4 uuid made from the
/// given random bytes (0.x `StoreManualArchetype`).
pub fn manual_uuid(device_id: Option<&str>, random: [u8; 16]) -> String {
    let prefix: String = device_id.unwrap_or("00000000").chars().take(8).collect();
    let mut bytes = random;
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: Vec<String> = bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!(
        "{prefix}-{}-{}-{}-{}-{}",
        hex[0..4].concat(),
        hex[4..6].concat(),
        hex[6..8].concat(),
        hex[8..10].concat(),
        hex[10..16].concat()
    )
}

/// A list's identity for duplicate checks: its `catalog:board:quantity`
/// rows sorted and joined with `|` (0.x `AddArchetypeVariant::hashPivot`,
/// by catalog id instead of a card row id). A card listed twice on the same
/// board keeps its last row, as 0.x.
pub fn variant_hash(cards: &[ListCard]) -> String {
    let mut rows: Vec<(i64, bool, u32)> = Vec::new();
    for card in cards {
        rows.retain(|(catalog_id, sideboard, _)| {
            !(*catalog_id == card.catalog_id && *sideboard == card.sideboard)
        });
        rows.push((card.catalog_id, card.sideboard, card.quantity));
    }
    let mut keys: Vec<String> = rows
        .iter()
        .map(|(catalog_id, sideboard, quantity)| {
            format!("{catalog_id}:{}:{quantity}", u8::from(*sideboard))
        })
        .collect();
    keys.sort();
    keys.join("|")
}

/// Refuses a new variant identical to an existing one.
pub fn check_new_variant(existing: &[Vec<ListCard>], new: &[ListCard]) -> Result<(), String> {
    let new_hash = variant_hash(new);
    for list in existing {
        if variant_hash(list) == new_hash {
            return Err(DUPLICATE_VARIANT.to_string());
        }
    }
    Ok(())
}

/// 0.x `MergeArchetype`'s guards, in its order, with the words of 0.x's
/// `MergeArchetypeRequest`, which the player saw (#300 F9).
pub fn check_merge(source: &GuardArchetype, parent: &GuardArchetype) -> Result<(), String> {
    if source.id == parent.id {
        return Err("Cannot merge an archetype into itself.".to_string());
    }
    if source.format != parent.format {
        return Err("Parent must share the same format.".to_string());
    }
    if source.fallback || parent.fallback {
        return Err("Fallback archetypes cannot be merged.".to_string());
    }
    if source.merged_into_id.is_some() {
        return Err("This archetype is already merged.".to_string());
    }
    if parent.merged_into_id.is_some() {
        return Err("Parent archetype is already merged.".to_string());
    }
    Ok(())
}

/// 0.x `UnmergeArchetype`'s guard.
pub fn check_unmerge(source: &GuardArchetype) -> Result<(), String> {
    if source.merged_into_id.is_none() {
        return Err("Archetype is not merged.".to_string());
    }
    Ok(())
}

/// 0.x `ReassignArchetypeVariant`'s guards, in its order, with the words of
/// 0.x's `ReassignArchetypeVariantRequest` (#300 F9). `source_lists` is
/// the source's variant count before the move; moving its last one merges
/// the source into the target.
pub fn check_reassign(
    source: &GuardArchetype,
    target: &GuardArchetype,
    source_lists: usize,
) -> Result<ReassignOutcome, String> {
    if source.id == target.id {
        return Err("Cannot reassign to the same archetype.".to_string());
    }
    if source.format != target.format {
        return Err("Target must share the same format.".to_string());
    }
    if target.fallback {
        return Err("Cannot reassign to a fallback archetype.".to_string());
    }
    if target.merged_into_id.is_some() {
        return Err("Target archetype is already merged.".to_string());
    }
    if source.merged_into_id.is_some() {
        return Err("Source archetype is already merged.".to_string());
    }
    Ok(ReassignOutcome {
        merge_source: source_lists <= 1,
    })
}
