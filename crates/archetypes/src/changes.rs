//! The definitions check's answer and what v1 does with it (archetypes
//! spec section 5): which definitions replace their rows, which held uuids
//! move to another archetype, which retire, and which matches are asked
//! for detection again.
//!
//! Pure: the store applies the plan (`ArchetypeEdit::ApplyDefinitions`).

use std::collections::{BTreeMap, BTreeSet};

use crate::definition::Definition;

/// The check's answer (spec section 16.3). Unchanged uuids are in none of
/// the lists.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DefinitionsCheck {
    /// New or changed definitions, including the targets of `moved`
    /// entries that are still active.
    pub changed: Vec<Definition>,
    /// Merged or renamed on the server: `(old uuid, new uuid)`.
    pub moved: Vec<(String, String)>,
    /// Inactive or unknown on the server.
    pub retired: Vec<String>,
}

/// One downloaded archetype v1 holds, as the check sees it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct HeldDefinition {
    pub uuid: String,
    /// The archetypes format key (`modern`).
    pub format: Option<String>,
    /// `None` before its definition is fetched.
    pub version: Option<String>,
    pub retired: bool,
}

/// What to re-detect after a check: matches with an auto row pointing at
/// one of `uuids`, and matches of one of `formats` (format keys) with an
/// auto row below confidence 1.0. Hand-set rows are never re-detected.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Redetect {
    pub uuids: Vec<String>,
    pub formats: Vec<String>,
}

impl Redetect {
    pub fn is_empty(&self) -> bool {
        self.uuids.is_empty() && self.formats.is_empty()
    }
}

/// What one check changes, in the order the store applies it: `replace`
/// first (so a move target is held), then `moves`, `retire` and
/// `redetect`. `plan_slices` splits it into writer-sized edits.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ChangePlan {
    /// Definitions that replace their rows (or add new ones).
    pub replace: Vec<Definition>,
    /// Held uuid to the archetype it now is, chains followed to the end.
    pub moves: Vec<(String, String)>,
    /// Held uuids the server retired (matches keep them).
    pub retire: Vec<String>,
    pub redetect: Redetect,
}

impl ChangePlan {
    /// Nothing to apply.
    pub fn is_empty(&self) -> bool {
        self.replace.is_empty()
            && self.moves.is_empty()
            && self.retire.is_empty()
            && self.redetect.is_empty()
    }
}

/// The plan for a check's answer over the held downloaded archetypes:
/// - a changed definition replaces its row unless the held row already has
///   that version and is not retired (so applying an answer twice changes
///   nothing the second time);
/// - a move of a held uuid follows chains (`a` to `b`, `b` to `c` is `a`
///   to `c`); a move to itself or round a loop is dropped; a move whose
///   target is neither held nor in the answer cannot keep its matches
///   anywhere else, so the old archetype retires instead;
/// - a retired uuid that v1 holds, not yet retired and not moved, retires;
/// - re-detection covers the replaced archetypes v1 already held and the
///   formats of every replaced definition.
pub fn plan_changes(held: &[HeldDefinition], check: &DefinitionsCheck) -> ChangePlan {
    let held_by_uuid: BTreeMap<&str, &HeldDefinition> =
        held.iter().map(|row| (row.uuid.as_str(), row)).collect();
    let mut plan = ChangePlan::default();

    let mut answered: BTreeSet<&str> = BTreeSet::new();
    for definition in &check.changed {
        if definition.uuid.is_empty() || !answered.insert(definition.uuid.as_str()) {
            continue;
        }
        if let Some(row) = held_by_uuid.get(definition.uuid.as_str())
            && !row.retired
            && row.version.is_some()
            && row.version == definition.version
        {
            continue;
        }
        plan.replace.push(definition.clone());
    }

    let moved_to: BTreeMap<&str, &str> = check
        .moved
        .iter()
        .map(|(from, to)| (from.as_str(), to.as_str()))
        .collect();
    let mut moved_from: BTreeSet<String> = BTreeSet::new();
    let mut retire: BTreeSet<String> = BTreeSet::new();
    for (from, _) in &check.moved {
        if !held_by_uuid.contains_key(from.as_str()) || moved_from.contains(from) {
            continue;
        }
        let Some(target) = final_target(from, &moved_to) else {
            continue;
        };
        moved_from.insert(from.clone());
        let target_known = held_by_uuid.contains_key(target) || answered.contains(target);
        if target_known {
            plan.moves.push((from.clone(), target.to_string()));
        } else if !held_by_uuid[from.as_str()].retired {
            retire.insert(from.clone());
        }
    }

    for uuid in &check.retired {
        let Some(row) = held_by_uuid.get(uuid.as_str()) else {
            continue;
        };
        if !row.retired && !moved_from.contains(uuid) {
            retire.insert(uuid.clone());
        }
    }
    plan.retire = retire.into_iter().collect();

    plan.redetect = redetect_for(&plan.replace, &held_by_uuid);
    plan
}

/// The plan as edits small enough for one writer transaction each:
/// `replace` in chunks of `per_slice` definitions, each with the
/// re-detection its own definitions cause (so a definition and its
/// re-detection commit together, and a later slice failing loses nothing
/// that already committed), then one last slice with the moves and
/// retirements. Always at least one slice (an empty answer still records
/// that the check ran).
pub fn plan_slices(
    held: &[HeldDefinition],
    check: &DefinitionsCheck,
    per_slice: usize,
) -> Vec<ChangePlan> {
    let held_by_uuid: BTreeMap<&str, &HeldDefinition> =
        held.iter().map(|row| (row.uuid.as_str(), row)).collect();
    let plan = plan_changes(held, check);
    let mut slices = Vec::new();
    for chunk in plan.replace.chunks(per_slice.max(1)) {
        slices.push(ChangePlan {
            replace: chunk.to_vec(),
            redetect: redetect_for(chunk, &held_by_uuid),
            ..ChangePlan::default()
        });
    }
    slices.push(ChangePlan {
        moves: plan.moves,
        retire: plan.retire,
        ..ChangePlan::default()
    });
    slices
}

/// What replacing these definitions asks for detection again: the ones
/// v1 already held, and every definition's format (the held row's when the
/// definition names none).
fn redetect_for(
    replace: &[Definition],
    held_by_uuid: &BTreeMap<&str, &HeldDefinition>,
) -> Redetect {
    let mut uuids = BTreeSet::new();
    let mut formats = BTreeSet::new();
    for definition in replace {
        let held_row = held_by_uuid.get(definition.uuid.as_str());
        if held_row.is_some() {
            uuids.insert(definition.uuid.clone());
        }
        let format = definition
            .format
            .as_ref()
            .map(|format| format.to_lowercase())
            .or_else(|| held_row.and_then(|row| row.format.clone()));
        if let Some(format) = format {
            formats.insert(format);
        }
    }
    Redetect {
        uuids: uuids.into_iter().collect(),
        formats: formats.into_iter().collect(),
    }
}

/// Where a moved uuid ends up, following moves of the target. `None` for
/// a move to itself or round a loop.
fn final_target<'a>(from: &'a str, moved_to: &BTreeMap<&'a str, &'a str>) -> Option<&'a str> {
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    seen.insert(from);
    let mut current = *moved_to.get(from)?;
    while let Some(next) = moved_to.get(current) {
        if !seen.insert(current) {
            return None;
        }
        current = next;
    }
    if seen.contains(current) {
        return None;
    }
    Some(current)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn held(uuid: &str, version: Option<&str>) -> HeldDefinition {
        HeldDefinition {
            uuid: uuid.to_string(),
            format: Some("modern".to_string()),
            version: version.map(str::to_string),
            retired: false,
        }
    }

    fn definition(uuid: &str, version: &str, format: &str) -> Definition {
        Definition {
            uuid: uuid.to_string(),
            name: format!("Q{uuid}"),
            format: Some(format.to_string()),
            version: Some(version.to_string()),
            ..Definition::default()
        }
    }

    fn moved(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(from, to)| (from.to_string(), to.to_string()))
            .collect()
    }

    fn uuids(plan: &[Definition]) -> Vec<&str> {
        plan.iter()
            .map(|definition| definition.uuid.as_str())
            .collect()
    }

    #[test]
    fn changed_definitions_replace_and_ask_their_rows_and_formats_again() {
        let held = [held("q-a", Some("v1")), held("q-b", None)];
        let check = DefinitionsCheck {
            changed: vec![
                definition("q-a", "v2", "Modern"),
                definition("q-b", "v1", "modern"),
                definition("q-new", "v1", "pauper"),
            ],
            ..DefinitionsCheck::default()
        };
        let plan = plan_changes(&held, &check);
        assert_eq!(uuids(&plan.replace), ["q-a", "q-b", "q-new"]);
        assert_eq!(plan.redetect.uuids, ["q-a", "q-b"], "held ones only");
        assert_eq!(plan.redetect.formats, ["modern", "pauper"]);
        assert!(plan.moves.is_empty() && plan.retire.is_empty());
    }

    // 0.x ApplyArchetypeRefreshTest "is idempotent": the same answer
    // applied twice changes nothing the second time.
    #[test]
    fn an_answer_already_applied_plans_nothing() {
        let check = DefinitionsCheck {
            changed: vec![definition("q-a", "v2", "modern")],
            moved: moved(&[("q-gone", "q-a")]),
            retired: vec!["q-old".to_string()],
        };
        let mut retired = held("q-old", Some("v1"));
        retired.retired = true;
        let after = [held("q-a", Some("v2")), retired];
        let plan = plan_changes(&after, &check);
        assert!(plan.is_empty(), "{plan:?}");
    }

    #[test]
    fn a_retired_row_comes_back_with_its_old_version() {
        let mut row = held("q-a", Some("v1"));
        row.retired = true;
        let check = DefinitionsCheck {
            changed: vec![definition("q-a", "v1", "modern")],
            ..DefinitionsCheck::default()
        };
        assert_eq!(uuids(&plan_changes(&[row], &check).replace), ["q-a"]);
    }

    #[test]
    fn moves_follow_chains_and_skip_loops_and_unheld_uuids() {
        let held = [
            held("q-a", Some("v1")),
            held("q-b", Some("v1")),
            held("q-x", Some("v1")),
            held("q-y", Some("v1")),
        ];
        let check = DefinitionsCheck {
            changed: vec![definition("q-c", "v1", "modern")],
            moved: moved(&[
                ("q-a", "q-b"),
                ("q-b", "q-c"),
                ("q-x", "q-y"),
                ("q-y", "q-x"),
                ("q-unheld", "q-c"),
            ]),
            retired: Vec::new(),
        };
        let plan = plan_changes(&held, &check);
        assert_eq!(
            plan.moves,
            moved(&[("q-a", "q-c"), ("q-b", "q-c")]),
            "chains resolved, the loop and the unheld uuid dropped"
        );
    }

    // 0.x "ignores mappings whose successor does not survive the refresh":
    // with nowhere to move to, the archetype retires and keeps its matches.
    #[test]
    fn a_move_to_an_unknown_target_retires_the_archetype() {
        let check = DefinitionsCheck {
            moved: moved(&[("q-a", "q-nowhere")]),
            retired: vec!["q-nowhere".to_string()],
            ..DefinitionsCheck::default()
        };
        let plan = plan_changes(&[held("q-a", Some("v1"))], &check);
        assert!(plan.moves.is_empty());
        assert_eq!(plan.retire, ["q-a"]);
    }

    #[test]
    fn a_move_to_a_held_target_needs_no_definition() {
        let check = DefinitionsCheck {
            moved: moved(&[("q-a", "q-b")]),
            ..DefinitionsCheck::default()
        };
        let plan = plan_changes(&[held("q-a", None), held("q-b", Some("v1"))], &check);
        assert_eq!(plan.moves, moved(&[("q-a", "q-b")]));
        assert!(plan.redetect.is_empty(), "a move is not a re-detection");
    }

    #[test]
    fn retired_uuids_v1_holds_retire_once() {
        let check = DefinitionsCheck {
            retired: vec!["q-a".to_string(), "q-a".to_string(), "q-unheld".to_string()],
            ..DefinitionsCheck::default()
        };
        let plan = plan_changes(&[held("q-a", Some("v1"))], &check);
        assert_eq!(plan.retire, ["q-a"]);
        assert!(
            plan.redetect.is_empty(),
            "retired archetypes keep their rows"
        );
    }

    // T5 review I1: each slice carries the re-detection its own
    // definitions cause, so a definition and its re-detection commit
    // together and a later slice failing loses nothing.
    #[test]
    fn each_slice_carries_its_own_redetection() {
        let mut no_format = definition("q-2", "v2", "modern");
        no_format.format = None;
        let held = [
            held("q-1", Some("v1")),
            HeldDefinition {
                format: Some("pauper".to_string()),
                ..held("q-2", Some("v1"))
            },
            held("q-a", Some("v1")),
            held("q-b", Some("v1")),
        ];
        let check = DefinitionsCheck {
            changed: vec![
                definition("q-1", "v2", "modern"),
                no_format,
                definition("q-3", "v1", "legacy"),
            ],
            moved: moved(&[("q-a", "q-1")]),
            retired: vec!["q-b".to_string()],
        };
        let slices = plan_slices(&held, &check, 2);
        assert_eq!(slices.len(), 3);
        assert_eq!(uuids(&slices[0].replace), ["q-1", "q-2"]);
        assert_eq!(
            slices[0].redetect,
            Redetect {
                uuids: vec!["q-1".to_string(), "q-2".to_string()],
                formats: vec!["modern".to_string(), "pauper".to_string()],
            },
            "a definition without a format uses the held row's"
        );
        assert_eq!(uuids(&slices[1].replace), ["q-3"]);
        assert_eq!(
            slices[1].redetect,
            Redetect {
                uuids: Vec::new(),
                formats: vec!["legacy".to_string()],
            }
        );
        assert!(slices[0].moves.is_empty() && slices[1].retire.is_empty());
        assert_eq!(slices[2].moves, moved(&[("q-a", "q-1")]));
        assert_eq!(slices[2].retire, ["q-b"]);
        assert!(slices[2].replace.is_empty() && slices[2].redetect.is_empty());
        let whole = plan_changes(&held, &check);
        assert_eq!(
            whole.redetect.formats,
            ["legacy", "modern", "pauper"],
            "the whole plan names every format"
        );
        assert_eq!(
            plan_slices(&[], &DefinitionsCheck::default(), 50).len(),
            1,
            "an empty answer still records the check"
        );
    }
}
