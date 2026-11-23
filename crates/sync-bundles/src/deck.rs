//! The deck bundle (0.x `DeckBundleBuilder`, survey section 12), built
//! from v1's `decks`, `deck_versions`, `sideboard_guides`,
//! `sideboard_guide_cards` and `deck_archetype_notes` rows (archetypes spec
//! sections 14 and 21.5).

use std::fmt;

use serde_json::{Map, Value, json};

use crate::carry::{Carry, DECK_CARRY};
use crate::datetime::{bundle_datetime, parse_bundle_datetime};
use crate::endstep_cards::EndstepCardRef;
use crate::match_bundle::php_compare;
use crate::platform::{BundlePlatform, add_platform, copy_platform};

/// One card of a sideboard plan (`sideboard_guide_cards`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuideCardInput {
    pub oracle_id: String,
    /// `in` or `out`.
    pub direction: String,
    pub quantity: i64,
}

/// One sideboard guide: the opponent archetype's uuid and its plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuideInput {
    pub archetype_uuid: String,
    pub cards: Vec<GuideCardInput>,
}

/// One matchup note. `created_at_ms` is `None` only in a pulled bundle that
/// had none (the pull stamps it with its own time, 0.x).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoteInput {
    pub archetype_uuid: String,
    pub body: String,
    pub created_at_ms: Option<i64>,
}

/// What a deck bundle is built from (`decks`, `deck_versions`, the deck's
/// archetype, guides and notes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeckBundleInput {
    /// The game the deck is from (MTGO writes no `platform` key).
    pub platform: BundlePlatform,
    pub mtgo_id: String,
    pub name: String,
    pub original_name: Option<String>,
    pub format: Option<String>,
    pub color_identity: Option<String>,
    /// `decks.cover_catalog_id` (0.x the cover card's `mtgo_id`). Never
    /// written by an Endstep bundle: its local id means nothing elsewhere.
    pub cover_catalog_id: Option<u32>,
    /// Endstep: the cover card by ref (spec section 5.3), written as
    /// `cover_mtgo_id` in place of the catalog id. Ignored on MTGO and Arena.
    pub endstep_cover: Option<EndstepCardRef>,
    /// `(signature, modified_at)` of every version, any order.
    pub versions: Vec<(String, i64)>,
    /// The uuid of `decks.archetype_id`.
    pub archetype_uuid: Option<String>,
    /// Any order: the bundle sorts them as 0.x does.
    pub guides: Vec<GuideInput>,
    pub notes: Vec<NoteInput>,
}

/// Which optional parts a pulled deck bundle speaks for. A part it does
/// not have is "unknown", not "none" (0.x `DeckBundleImporter`): the pull
/// leaves the deck's rows for it alone.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DeckParts {
    /// `deck.archetype_uuid` is there (canonical version 4 on).
    pub has_archetype: bool,
    /// Both `guides` and `notes` are there (canonical version 3 on).
    pub has_guides_and_notes: bool,
}

/// Why a pulled bundle could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BundleError {
    /// A part the bundle must have is missing.
    Missing(String),
    /// A part is there but not readable.
    Invalid(String),
}

impl fmt::Display for BundleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BundleError::Missing(part) => write!(f, "the bundle has no {part}"),
            BundleError::Invalid(part) => write!(f, "the bundle's {part} is not readable"),
        }
    }
}

impl std::error::Error for BundleError {}

/// The guides in 0.x's order: by archetype uuid; each plan by `(direction,
/// oracle_id)`. Also the stored form of a deck's unplaceable guides
/// (`UNPLACED_GUIDES`), read back with `read_guides`.
pub fn guides_section(guides: &[GuideInput]) -> Value {
    let mut sorted: Vec<&GuideInput> = guides.iter().collect();
    sorted.sort_by(|a, b| php_compare(&a.archetype_uuid, &b.archetype_uuid));
    let mut out = Vec::new();
    for guide in sorted {
        let mut cards: Vec<&GuideCardInput> = guide.cards.iter().collect();
        cards.sort_by(|a, b| {
            php_compare(&a.direction, &b.direction)
                .then_with(|| php_compare(&a.oracle_id, &b.oracle_id))
        });
        let cards: Vec<Value> = cards
            .into_iter()
            .map(|card| {
                json!({
                    "oracle_id": card.oracle_id,
                    "direction": card.direction,
                    "quantity": card.quantity,
                })
            })
            .collect();
        out.push(json!({"archetype_uuid": guide.archetype_uuid, "cards": cards}));
    }
    Value::Array(out)
}

/// A note's sort key time: whole seconds (0.x `getTimestamp()`).
fn note_seconds(note: &NoteInput) -> Option<i64> {
    note.created_at_ms.map(|at| at.div_euclid(1_000))
}

/// The notes in 0.x's order: by `(archetype uuid, created_at in whole
/// seconds, body)`, `created_at` without milliseconds. Also the stored form
/// of a deck's unplaceable notes (`UNPLACED_NOTES`), read back with
/// `read_notes`.
pub fn notes_section(notes: &[NoteInput]) -> Value {
    let mut sorted: Vec<&NoteInput> = notes.iter().collect();
    sorted.sort_by(|a, b| {
        php_compare(&a.archetype_uuid, &b.archetype_uuid)
            .then_with(|| note_seconds(a).cmp(&note_seconds(b)))
            .then_with(|| php_compare(&a.body, &b.body))
    });
    let out: Vec<Value> = sorted
        .into_iter()
        .map(|note| {
            json!({
                "archetype_uuid": note.archetype_uuid,
                "body": note.body,
                "created_at": note.created_at_ms.map(bundle_datetime),
            })
        })
        .collect();
    Value::Array(out)
}

/// The deck bundle: `deck`, `versions` sorted by `(modified_at` in whole
/// seconds`, signature)` (0.x compares `getTimestamp()`), `guides` and
/// `notes`, all from the rows. A carry (now empty for decks) still applies,
/// so a stored carry is put back as it was.
pub fn deck_bundle(input: &DeckBundleInput, carry: Option<&Carry>) -> Value {
    let mut versions: Vec<&(String, i64)> = input.versions.iter().collect();
    versions.sort_by(|(a_sig, a_at), (b_sig, b_at)| {
        (a_at.div_euclid(1_000), a_sig.as_bytes()).cmp(&(b_at.div_euclid(1_000), b_sig.as_bytes()))
    });
    let versions: Vec<Value> = versions
        .into_iter()
        .map(|(signature, modified_at)| {
            json!({ "signature": signature, "modified_at": bundle_datetime(*modified_at) })
        })
        .collect();
    let mut bundle = json!({
        "deck": {
            "mtgo_id": input.mtgo_id,
            "name": input.name,
            "original_name": input.original_name,
            "format": input.format,
            "color_identity": input.color_identity,
            "cover_mtgo_id": cover_value(input),
            "archetype_uuid": input.archetype_uuid,
        },
        "versions": versions,
        "guides": guides_section(&input.guides),
        "notes": notes_section(&input.notes),
    });
    add_platform(&mut bundle, input.platform);
    if let Some(carry) = carry {
        carry.apply(&mut bundle);
    }
    bundle
}

/// A deck's `cover_mtgo_id`: the catalog id as text (0.x), or on Endstep
/// the cover's ref (null without one, never the local number).
fn cover_value(input: &DeckBundleInput) -> Value {
    if input.platform == BundlePlatform::Endstep {
        return input
            .endstep_cover
            .as_ref()
            .map_or(Value::Null, EndstepCardRef::to_value);
    }
    match input.cover_catalog_id {
        Some(id) => Value::String(id.to_string()),
        None => Value::Null,
    }
}

/// A pulled `cover_mtgo_id` as `(catalog id, Endstep ref)`. An Endstep
/// bundle names its cover by ref only: anything else there is no cover.
fn read_cover(
    value: &Value,
    platform: BundlePlatform,
) -> Result<(Option<u32>, Option<EndstepCardRef>), BundleError> {
    if platform == BundlePlatform::Endstep {
        return Ok((None, EndstepCardRef::from_value(value)));
    }
    let invalid = || BundleError::Invalid("deck.cover_mtgo_id".to_string());
    let id = match value {
        Value::Null => None,
        Value::String(id) => Some(id.parse::<u32>().map_err(|_| invalid())?),
        Value::Number(id) => Some(
            id.as_u64()
                .and_then(|id| u32::try_from(id).ok())
                .ok_or_else(invalid)?,
        ),
        _ => return Err(invalid()),
    };
    Ok((id, None))
}

/// The sidecar sent beside the blob (0.x `DeckBundleBuilder::sidecar`):
/// `name`, `format`, `color_identity` from the bundle, nulls dropped.
pub fn deck_sidecar(bundle: &Value) -> Value {
    let mut sidecar = Map::new();
    for key in ["name", "format", "color_identity"] {
        let value = &bundle["deck"][key];
        if !value.is_null() {
            sidecar.insert(key.to_string(), value.clone());
        }
    }
    let mut sidecar = Value::Object(sidecar);
    copy_platform(bundle, &mut sidecar);
    sidecar
}

/// A pulled guide (a guide without an archetype uuid is left out, as is a
/// card that is not `{oracle_id: text, direction: text, quantity: integer}`;
/// the store skips the rows that break a CHECK).
pub fn read_guides(listed: &Value) -> Vec<GuideInput> {
    let mut guides = Vec::new();
    for guide in listed.as_array().map(Vec::as_slice).unwrap_or(&[]) {
        let Some(uuid) = guide["archetype_uuid"].as_str() else {
            continue;
        };
        let mut cards = Vec::new();
        for card in guide["cards"].as_array().map(Vec::as_slice).unwrap_or(&[]) {
            let (Some(oracle_id), Some(direction), Some(quantity)) = (
                card["oracle_id"].as_str(),
                card["direction"].as_str(),
                card["quantity"].as_i64(),
            ) else {
                continue;
            };
            cards.push(GuideCardInput {
                oracle_id: oracle_id.to_string(),
                direction: direction.to_string(),
                quantity,
            });
        }
        guides.push(GuideInput {
            archetype_uuid: uuid.to_string(),
            cards,
        });
    }
    guides
}

/// Pulled notes (one without an archetype uuid or a text body is left out;
/// an unreadable time is none).
pub fn read_notes(listed: &Value) -> Vec<NoteInput> {
    let mut notes = Vec::new();
    for note in listed.as_array().map(Vec::as_slice).unwrap_or(&[]) {
        let (Some(uuid), Some(body)) = (note["archetype_uuid"].as_str(), note["body"].as_str())
        else {
            continue;
        };
        notes.push(NoteInput {
            archetype_uuid: uuid.to_string(),
            body: body.to_string(),
            created_at_ms: note["created_at"].as_str().and_then(parse_bundle_datetime),
        });
    }
    notes
}

/// A pulled deck bundle as the columns and rows v1 stores, which optional
/// parts it speaks for, and the carry of the rest (empty for decks now).
pub fn read_deck_bundle(
    bundle: &Value,
) -> Result<(DeckBundleInput, DeckParts, Carry), BundleError> {
    let deck = bundle
        .get("deck")
        .filter(|deck| deck.is_object())
        .ok_or_else(|| BundleError::Missing("deck".to_string()))?;
    let text = |key: &str| deck[key].as_str().map(str::to_string);
    let mtgo_id = match &deck["mtgo_id"] {
        Value::String(id) => id.clone(),
        Value::Number(id) => id.to_string(),
        _ => return Err(BundleError::Missing("deck.mtgo_id".to_string())),
    };
    let name = text("name").ok_or_else(|| BundleError::Missing("deck.name".to_string()))?;
    let platform = BundlePlatform::of_bundle(bundle)?;
    let (cover_catalog_id, endstep_cover) = read_cover(&deck["cover_mtgo_id"], platform)?;
    let mut versions = Vec::new();
    if let Some(listed) = bundle.get("versions") {
        let listed = listed
            .as_array()
            .ok_or_else(|| BundleError::Invalid("versions".to_string()))?;
        for version in listed {
            let signature = version["signature"]
                .as_str()
                .ok_or_else(|| BundleError::Invalid("versions.signature".to_string()))?;
            let modified_at = version["modified_at"]
                .as_str()
                .and_then(parse_bundle_datetime)
                .ok_or_else(|| BundleError::Invalid("versions.modified_at".to_string()))?;
            versions.push((signature.to_string(), modified_at));
        }
    }
    let parts = DeckParts {
        has_archetype: deck.get("archetype_uuid").is_some(),
        has_guides_and_notes: bundle.get("guides").is_some() && bundle.get("notes").is_some(),
    };
    let input = DeckBundleInput {
        platform,
        mtgo_id,
        name,
        original_name: text("original_name"),
        format: text("format"),
        color_identity: text("color_identity"),
        cover_catalog_id,
        endstep_cover,
        versions,
        archetype_uuid: text("archetype_uuid"),
        guides: read_guides(&bundle["guides"]),
        notes: read_notes(&bundle["notes"]),
    };
    Ok((input, parts, Carry::from_bundle(bundle, DECK_CARRY)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canonical::{bundle_hash, canonical_json};
    use serde_json::{Value, json};

    fn golden() -> Vec<Value> {
        let golden: Value = serde_json::from_str(include_str!(
            "../../../fixtures/parity/sync_deck_bundles.golden.json"
        ))
        .expect("the deck golden is JSON");
        golden["bundles"].as_array().expect("bundles").clone()
    }

    /// A golden's `input`: the v1 rows the 0.x export seeded (guides,
    /// guide cards and notes in 0.x id order).
    fn input_of(case: &Value) -> DeckBundleInput {
        let input = &case["input"];
        let text = |value: &Value| value.as_str().map(str::to_string);
        let guides = input["guides"]
            .as_array()
            .expect("guides")
            .iter()
            .map(|guide| GuideInput {
                archetype_uuid: text(&guide["archetype_uuid"]).expect("uuid"),
                cards: guide["cards"]
                    .as_array()
                    .expect("cards")
                    .iter()
                    .map(|card| GuideCardInput {
                        oracle_id: text(&card["oracle_id"]).expect("oracle"),
                        direction: text(&card["direction"]).expect("direction"),
                        quantity: card["quantity"].as_i64().expect("quantity"),
                    })
                    .collect(),
            })
            .collect();
        let notes = input["notes"]
            .as_array()
            .expect("notes")
            .iter()
            .map(|note| NoteInput {
                archetype_uuid: text(&note["archetype_uuid"]).expect("uuid"),
                body: text(&note["body"]).expect("body"),
                created_at_ms: note["created_at_ms"].as_i64(),
            })
            .collect();
        DeckBundleInput {
            platform: BundlePlatform::Mtgo,
            mtgo_id: text(&input["mtgo_id"]).expect("mtgo id"),
            name: text(&input["name"]).expect("name"),
            original_name: text(&input["original_name"]),
            format: text(&input["format"]),
            color_identity: text(&input["color_identity"]),
            cover_catalog_id: input["cover_catalog_id"].as_u64().map(|id| id as u32),
            endstep_cover: None,
            versions: input["versions"]
                .as_array()
                .expect("versions")
                .iter()
                .map(|version| {
                    (
                        version["signature"]
                            .as_str()
                            .expect("signature")
                            .to_string(),
                        version["modified_at_ms"].as_i64().expect("ms"),
                    )
                })
                .collect(),
            archetype_uuid: text(&input["archetype_uuid"]),
            guides,
            notes,
        }
    }

    fn deck(versions: Vec<(&str, i64)>) -> DeckBundleInput {
        DeckBundleInput {
            platform: BundlePlatform::Mtgo,
            mtgo_id: "12345".to_string(),
            name: "Qdeck".to_string(),
            original_name: Some("Original Name".to_string()),
            format: Some("Modern".to_string()),
            color_identity: Some("R".to_string()),
            cover_catalog_id: None,
            endstep_cover: None,
            versions: versions
                .into_iter()
                .map(|(signature, at)| (signature.to_string(), at))
                .collect(),
            archetype_uuid: None,
            guides: Vec::new(),
            notes: Vec::new(),
        }
    }

    // Every deck 0.x's DeckBundleBuilder built: the same bytes, hash, client
    // id and sidecar from v1's stored rows alone (archetypes spec section
    // 14: no carry).
    #[test]
    fn every_golden_deck_bundle_matches_0x_byte_for_byte() {
        for case in golden() {
            let mtgo_id = case["mtgo_id"].as_str().expect("mtgo id");
            let expected = case["canonical"].as_str().expect("canonical");
            let input = input_of(&case);
            let bundle = deck_bundle(&input, None);
            let encoded = canonical_json(&bundle);
            assert_eq!(encoded, expected, "{mtgo_id}");
            assert_eq!(
                bundle_hash(&encoded),
                case["hash"].as_str().expect("hash"),
                "{mtgo_id}"
            );
            assert_eq!(deck_sidecar(&bundle), case["sidecar"], "{mtgo_id}");
            assert_eq!(
                crate::client_id::deck_client_id(&input.mtgo_id),
                case["client_id"].as_str().expect("client id")
            );
        }
    }

    // Review Focus 6 (spec section 21.5): the golden deck's guides and notes
    // (two notes in one second, the later body stored first) come out in
    // 0.x's order whatever order v1 holds them in, milliseconds dropped from
    // `created_at` and ignored by the sort; reading the bytes back and
    // building again gives the same bytes.
    #[test]
    fn guides_and_notes_round_trip_byte_identical() {
        let case = golden()
            .into_iter()
            .find(|case| case["mtgo_id"] == "12345")
            .expect("the full deck");
        let expected = case["canonical"].as_str().expect("canonical");
        let mut input = input_of(&case);
        assert_eq!(input.notes.len(), 3);
        // Milliseconds in the same second: "same second" earlier by the
        // clock, still after "keep" by the body.
        for note in &mut input.notes {
            let extra = if note.body.starts_with("Qnote: keep") {
                900
            } else if note.body.starts_with("Qnote: same") {
                100
            } else {
                0
            };
            note.created_at_ms = note.created_at_ms.map(|at| at + extra);
        }
        input.guides.reverse();
        input.notes.reverse();
        for guide in &mut input.guides {
            guide.cards.reverse();
        }
        assert_eq!(canonical_json(&deck_bundle(&input, None)), expected);
        let pulled: Value = serde_json::from_str(expected).expect("bundle");
        let (read, parts, carry) = read_deck_bundle(&pulled).expect("readable");
        assert!(parts.has_archetype && parts.has_guides_and_notes);
        assert!(carry.is_empty(), "nothing rides in the carry any more");
        assert_eq!(
            read.archetype_uuid.as_deref(),
            Some("cccccccc-1111-4111-8111-111111111111")
        );
        assert_eq!(read.guides.len(), 2);
        assert_eq!(read.notes[1].created_at_ms, Some(1_787_216_400_000));
        assert_eq!(canonical_json(&deck_bundle(&read, None)), expected);
    }

    // A pulled bundle read into stored columns and rows, then rebuilt, gives
    // the same bytes: v1 never uploads a different hash for a row it did not
    // change.
    #[test]
    fn a_read_then_rebuild_of_every_golden_bundle_gives_the_same_bytes() {
        for case in golden() {
            let expected = case["canonical"].as_str().expect("canonical");
            let pulled: Value = serde_json::from_str(expected).expect("bundle");
            let (input, _, _) = read_deck_bundle(&pulled).expect("readable");
            assert_eq!(canonical_json(&deck_bundle(&input, None)), expected);
        }
    }

    // 0.x "builds a deck bundle whose versions carry signature and
    // modified_at only".
    #[test]
    fn builds_a_deck_bundle_whose_versions_carry_signature_and_modified_at_only() {
        let bundle = deck_bundle(&deck(vec![("q-sig", 1_787_133_605_000)]), None);
        assert_eq!(
            bundle["versions"],
            json!([{"signature": "q-sig", "modified_at": "2026-08-19T10:00:05Z"}])
        );
        assert_eq!(bundle["deck"]["mtgo_id"], json!("12345"));
        assert_eq!(bundle["deck"]["archetype_uuid"], Value::Null);
        assert_eq!(bundle["guides"], json!([]));
        assert_eq!(bundle["notes"], json!([]));
    }

    // 0.x "orders deck versions by signature when modified_at ties" (0.x
    // compares whole seconds).
    #[test]
    fn orders_deck_versions_by_signature_when_modified_at_ties() {
        let bundle = deck_bundle(
            &deck(vec![
                ("zzz-signature", 1_787_133_605_900),
                ("aaa-signature", 1_787_133_605_100),
                ("mmm-signature", 1_787_133_604_000),
            ]),
            None,
        );
        let order: Vec<&str> = bundle["versions"]
            .as_array()
            .expect("versions")
            .iter()
            .map(|version| version["signature"].as_str().expect("signature"))
            .collect();
        assert_eq!(
            order,
            vec!["mmm-signature", "aaa-signature", "zzz-signature"]
        );
    }

    fn card(oracle_id: &str, direction: &str, quantity: i64) -> GuideCardInput {
        GuideCardInput {
            oracle_id: oracle_id.to_string(),
            direction: direction.to_string(),
            quantity,
        }
    }

    // 0.x "carries sideboard guides and matchup notes on the deck bundle,
    // ordered by archetype uuid and free of local ids".
    #[test]
    fn carries_sideboard_guides_and_matchup_notes_ordered_by_archetype_uuid_and_free_of_local_ids()
    {
        let mut input = deck(Vec::new());
        input.guides = vec![
            GuideInput {
                archetype_uuid: "bbbbbbbb-0000-0000-0000-000000000000".to_string(),
                cards: vec![
                    card("o-zzz", "in", 2),
                    card("o-aaa", "out", 3),
                    card("o-bbb", "in", 1),
                ],
            },
            GuideInput {
                archetype_uuid: "aaaaaaaa-0000-0000-0000-000000000000".to_string(),
                cards: Vec::new(),
            },
        ];
        input.notes = vec![NoteInput {
            archetype_uuid: "aaaaaaaa-0000-0000-0000-000000000000".to_string(),
            body: "Keep the counters in".to_string(),
            created_at_ms: Some(1_787_216_400_250),
        }];
        let bundle = deck_bundle(&input, None);
        assert_eq!(
            bundle["guides"],
            json!([
                {"archetype_uuid": "aaaaaaaa-0000-0000-0000-000000000000", "cards": []},
                {"archetype_uuid": "bbbbbbbb-0000-0000-0000-000000000000", "cards": [
                    {"oracle_id": "o-bbb", "direction": "in", "quantity": 1},
                    {"oracle_id": "o-zzz", "direction": "in", "quantity": 2},
                    {"oracle_id": "o-aaa", "direction": "out", "quantity": 3},
                ]},
            ])
        );
        assert_eq!(
            bundle["notes"],
            json!([{"archetype_uuid": "aaaaaaaa-0000-0000-0000-000000000000",
                    "body": "Keep the counters in", "created_at": "2026-08-20T09:00:00Z"}])
        );
        let encoded = canonical_json(&bundle);
        assert!(!encoded.contains("\"id\""), "{encoded}");
        assert!(!encoded.contains("guide_id"), "{encoded}");
    }

    // 0.x "carries the cover card and deck archetype on the deck bundle as
    // cross-device ids".
    #[test]
    fn carries_the_cover_card_and_deck_archetype_as_cross_device_ids() {
        let mut with_cover = deck(Vec::new());
        with_cover.cover_catalog_id = Some(98_765);
        with_cover.archetype_uuid = Some("q-archetype-uuid".to_string());
        let bundle = deck_bundle(&with_cover, None);
        assert_eq!(bundle["deck"]["cover_mtgo_id"], json!("98765"));
        assert_eq!(bundle["deck"]["archetype_uuid"], json!("q-archetype-uuid"));
        let keys: Vec<&String> = bundle["deck"].as_object().expect("deck").keys().collect();
        assert_eq!(keys.len(), 7, "{keys:?}");
    }

    // 0.x "leaves local guides and notes alone when a pre-version-3 deck
    // bundle carries no guide keys": the read says the keys are absent, so
    // the pull leaves the deck's rows alone (`store::sync`); one key alone
    // is not enough either (0.x needs both).
    #[test]
    fn a_bundle_without_guide_keys_leaves_local_guides_alone() {
        let old = json!({
            "deck": {"mtgo_id": "1", "name": "Q"},
            "versions": [],
        });
        let (input, parts, _) = read_deck_bundle(&old).expect("readable");
        assert!(!parts.has_guides_and_notes);
        assert!(!parts.has_archetype, "no archetype key: unknown, not none");
        assert!(input.guides.is_empty() && input.notes.is_empty());
        let half = json!({
            "deck": {"mtgo_id": "1", "name": "Q", "archetype_uuid": null},
            "versions": [],
            "guides": [],
        });
        let (_, parts, _) = read_deck_bundle(&half).expect("readable");
        assert!(!parts.has_guides_and_notes);
        assert!(parts.has_archetype);
    }

    // Pulled guides and notes are read loosely: a row that is not readable
    // is left out, the rest is kept (the store skips CHECK breakers).
    #[test]
    fn unreadable_guide_rows_are_left_out_of_a_pull() {
        let pulled = json!({
            "deck": {"mtgo_id": "1", "name": "Q"},
            "guides": [
                {"archetype_uuid": "q-a", "cards": [
                    {"oracle_id": "o-1", "direction": "in", "quantity": 2},
                    {"oracle_id": 7, "direction": "in", "quantity": 1},
                    {"oracle_id": "o-2", "direction": "out", "quantity": "x"},
                ]},
                {"cards": []},
            ],
            "notes": [
                {"archetype_uuid": "q-a", "body": "Qnote", "created_at": null},
                {"archetype_uuid": "q-a", "body": "Qtimed", "created_at": "2026-08-20T09:00:00Z"},
                {"body": "Qno archetype"},
            ],
        });
        let (input, parts, _) = read_deck_bundle(&pulled).expect("readable");
        assert!(parts.has_guides_and_notes);
        assert_eq!(input.guides.len(), 1);
        assert_eq!(input.guides[0].cards, vec![card("o-1", "in", 2)]);
        assert_eq!(input.notes.len(), 2);
        assert_eq!(input.notes[0].created_at_ms, None);
        assert_eq!(input.notes[1].created_at_ms, Some(1_787_216_400_000));
    }

    #[test]
    fn mtgo_deck_has_no_platform_key_and_an_arena_one_carries_it() {
        let bundle = deck_bundle(&deck(Vec::new()), None);
        assert!(bundle.get("platform").is_none());
        assert!(deck_sidecar(&bundle).get("platform").is_none());
        let mut arena = deck(Vec::new());
        arena.platform = BundlePlatform::Arena;
        let bundle = deck_bundle(&arena, None);
        assert_eq!(bundle["platform"], json!("arena"));
        assert_eq!(deck_sidecar(&bundle)["platform"], json!("arena"));
        let parts = read_deck_bundle(&bundle).expect("reads");
        assert_eq!(parts.0.platform, BundlePlatform::Arena);
    }

    fn bolt() -> crate::EndstepCardRef {
        crate::EndstepCardRef {
            name: "Qbolt".to_string(),
            set: "m10".to_string(),
            number: "146".to_string(),
        }
    }

    #[test]
    fn an_endstep_deck_names_its_cover_by_ref() {
        let mut endstep = deck(Vec::new());
        endstep.platform = BundlePlatform::Endstep;
        endstep.endstep_cover = Some(bolt());
        let bundle = deck_bundle(&endstep, None);
        assert_eq!(
            bundle["deck"]["cover_mtgo_id"],
            json!({"name": "Qbolt", "number": "146", "set": "m10"})
        );
        assert_eq!(deck_sidecar(&bundle)["platform"], json!("endstep"));
        let (read, _, _) = read_deck_bundle(&bundle).expect("reads");
        assert_eq!(read.endstep_cover, Some(bolt()));
        assert_eq!(read.cover_catalog_id, None);
        assert_eq!(read, endstep, "the pull reads back what was built");
    }

    #[test]
    fn an_endstep_deck_never_writes_or_reads_a_number_cover() {
        let mut endstep = deck(Vec::new());
        endstep.platform = BundlePlatform::Endstep;
        endstep.cover_catalog_id = Some(200_000_001);
        let bundle = deck_bundle(&endstep, None);
        assert_eq!(bundle["deck"]["cover_mtgo_id"], Value::Null);
        let mut numbered = bundle.clone();
        numbered["deck"]["cover_mtgo_id"] = json!("200000001");
        let (read, _, _) = read_deck_bundle(&numbered).expect("reads");
        assert_eq!(read.cover_catalog_id, None);
        assert_eq!(read.endstep_cover, None);
    }

    #[test]
    fn an_mtgo_or_arena_deck_ignores_an_endstep_cover() {
        let mut arena = deck(Vec::new());
        arena.platform = BundlePlatform::Arena;
        arena.cover_catalog_id = Some(100_123_456);
        arena.endstep_cover = Some(bolt());
        let bundle = deck_bundle(&arena, None);
        assert_eq!(bundle["deck"]["cover_mtgo_id"], json!("100123456"));
        let (read, _, _) = read_deck_bundle(&bundle).expect("reads");
        assert_eq!(read.cover_catalog_id, Some(100_123_456));
        assert_eq!(read.endstep_cover, None);
    }

    #[test]
    fn the_sidecar_drops_null_fields() {
        let mut bare = deck(Vec::new());
        bare.color_identity = None;
        bare.format = None;
        assert_eq!(
            deck_sidecar(&deck_bundle(&bare, None)),
            json!({"name": "Qdeck"})
        );
    }

    #[test]
    fn a_bundle_without_a_deck_or_with_a_bad_version_is_refused() {
        assert!(matches!(
            read_deck_bundle(&json!({})),
            Err(BundleError::Missing(_))
        ));
        let bad = json!({
            "deck": {"mtgo_id": "1", "name": "Q"},
            "versions": [{"signature": "q", "modified_at": "yesterday"}],
        });
        assert!(matches!(
            read_deck_bundle(&bad),
            Err(BundleError::Invalid(_))
        ));
    }
}
