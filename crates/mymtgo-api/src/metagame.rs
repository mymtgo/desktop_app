//! The field's archetype shares for one format and window (archetypes spec
//! sections 9 and 16.4): `GET /api/metagame/{slug}?days=N`, the numbers
//! mymtgo.com/metagame/{slug}?days=N shows. Class `Lookup`: never offline.

use serde::Serialize;
use serde_json::Value;

use crate::error::ApiError;
use crate::gate::RequestClass;
use crate::loose;
use crate::php::urlencode;
use crate::request::{ApiRequest, ApiResponse, AuthMode, Method};

/// The windows the desktop asks for (the API `GetMetagameShares::WINDOWS`):
/// "Last 7 days", "Last 30 days" and "All time" (spec 9, ruling 7).
pub const METAGAME_WINDOWS: [u32; 3] = [7, 30, 90];

/// The metagame call's timeout (spec 9).
pub const METAGAME_TIMEOUT_MS: u64 = 15_000;

/// The public format slugs (the API `App\Support\Formats::SLUGS`).
const PUBLIC_SLUGS: [&str; 7] = [
    "modern",
    "legacy",
    "pauper",
    "vintage",
    "premodern",
    "standard",
    "pioneer",
];

/// The API's slug for an archetypes format key or an MTGO format code
/// (`modern`, `CMODERN`); `None` for any format the website does not
/// publish (its endpoint answers those 404).
pub fn format_slug(key: &str) -> Option<&'static str> {
    let key = archetypes::format_key(key)?;
    PUBLIC_SLUGS.iter().copied().find(|slug| *slug == key)
}

/// `GET /api/metagame/{slug}?days=N`.
pub fn metagame_request(format_slug: &str, days: u32) -> ApiRequest {
    let path = format!("/api/metagame/{}", urlencode(format_slug));
    let mut request = ApiRequest::new(
        Method::Get,
        &path,
        RequestClass::Lookup,
        AuthMode::DeviceOrBearer,
    );
    request.query = vec![("days".to_string(), days.to_string())];
    request.timeout_ms = METAGAME_TIMEOUT_MS;
    request
}

/// One format's field over a window (spec 16.4).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MetagameShares {
    pub format: String,
    pub days: u32,
    /// `Y-m-d`, the window's first day (the website's rolling start).
    pub window_start: String,
    /// `Y-m-d`, the server's today.
    pub window_end: String,
    pub matches: u64,
    /// Sorted by share, then name, as the server sends them.
    pub archetypes: Vec<GlobalShare>,
    /// Seats of archetypes not listed, percent; `None` for an empty window.
    pub other_share: Option<f64>,
    /// Seats no archetype could name, percent; `None` for an empty window.
    pub unnameable_share: Option<f64>,
}

/// One archetype's share of the field.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct GlobalShare {
    pub uuid: String,
    pub name: String,
    pub color_identity: Option<String>,
    /// Percent of all seats, one decimal; `None` when the server has none.
    pub share: Option<f64>,
    /// Seats played in the window.
    pub matches: u64,
    /// The key cards the website's tile fans out for it, in its order.
    pub cards: Vec<GlobalCard>,
}

/// One card on an archetype's website tile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GlobalCard {
    pub name: String,
    /// The full card image URL.
    pub image: String,
    /// The art crop URL.
    pub art: Option<String>,
}

/// Reads a 2xx metagame answer. A non-2xx is `Status` with the body (404
/// for a format the server does not know); an answer that is not an object,
/// or whose `archetypes` is not a list, is `Malformed`. A row without a
/// uuid is dropped.
pub fn parse_metagame(response: &ApiResponse) -> Result<MetagameShares, ApiError> {
    if !response.is_success() {
        return Err(response.status_error());
    }
    let body: Value = response.json()?;
    let Value::Object(_) = body else {
        return Err(ApiError::Malformed(
            "the metagame answer is not an object".to_string(),
        ));
    };
    let rows: &[Value] = match body.get("archetypes") {
        None | Some(Value::Null) => &[],
        Some(Value::Array(rows)) => rows,
        Some(_) => {
            return Err(ApiError::Malformed(
                "the metagame archetypes are not a list".to_string(),
            ));
        }
    };
    let mut archetypes = Vec::new();
    for row in rows {
        let Some(uuid) = loose::text(row, "uuid").filter(|uuid| !uuid.is_empty()) else {
            continue;
        };
        archetypes.push(GlobalShare {
            uuid,
            name: loose::text(row, "name").unwrap_or_default(),
            color_identity: loose::text(row, "color_identity"),
            share: loose::number(row, "share"),
            matches: count(row, "matches"),
            cards: cards(row),
        });
    }
    Ok(MetagameShares {
        format: loose::text(&body, "format").unwrap_or_default(),
        days: u32::try_from(count(&body, "days")).unwrap_or(0),
        window_start: loose::text(&body, "window_start").unwrap_or_default(),
        window_end: loose::text(&body, "window_end").unwrap_or_default(),
        matches: count(&body, "matches"),
        archetypes,
        other_share: loose::number(&body, "other_share"),
        unnameable_share: loose::number(&body, "unnameable_share"),
    })
}

/// A row's tile cards; a card without an image is dropped, and `cards`
/// that is not a list reads as none.
fn cards(row: &Value) -> Vec<GlobalCard> {
    let Some(Value::Array(items)) = row.get("cards") else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for item in items {
        let Some(image) = loose::text(item, "image").filter(|image| !image.is_empty()) else {
            continue;
        };
        out.push(GlobalCard {
            name: loose::text(item, "name").unwrap_or_default(),
            image,
            art: loose::text(item, "art").filter(|art| !art.is_empty()),
        });
    }
    out
}

/// A count field: a whole number at least 0, else 0.
fn count(object: &Value, key: &str) -> u64 {
    loose::whole(object, key)
        .and_then(|value| u64::try_from(value).ok())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gate::RequestClass;
    use crate::request::{AuthMode, Method};
    use crate::test_support::response;

    #[test]
    fn the_metagame_request_gets_the_format_and_window() {
        let request = metagame_request("modern", 30);
        assert_eq!(request.method, Method::Get);
        assert_eq!(request.path, "/api/metagame/modern");
        assert_eq!(request.query, vec![("days".to_string(), "30".to_string())]);
        assert_eq!(request.class, RequestClass::Lookup);
        assert_eq!(request.auth, AuthMode::DeviceOrBearer);
        assert_eq!(request.timeout_ms, 15_000);
        assert!(request.body.bytes().is_empty());
        let odd = metagame_request("q/../x", 7);
        assert_eq!(odd.path, "/api/metagame/q%2F..%2Fx");
        assert_eq!(METAGAME_WINDOWS, [7, 30, 90]);
    }

    // the API `App\Support\Formats::SLUGS`: the seven public formats.
    #[test]
    fn format_slug_names_the_seven_public_formats() {
        let expected = [
            ("modern", "modern"),
            ("legacy", "legacy"),
            ("pauper", "pauper"),
            ("vintage", "vintage"),
            ("premodern", "premodern"),
            ("standard", "standard"),
            ("pioneer", "pioneer"),
        ];
        for (key, slug) in expected {
            assert_eq!(format_slug(key), Some(slug), "{key}");
        }
        // A format code reads as its key (0.x `MtgoFormat::key`).
        assert_eq!(format_slug("CMODERN"), Some("modern"));
        assert_eq!(format_slug("CPremodern"), Some("premodern"));
        assert_eq!(format_slug("Pioneer"), Some("pioneer"));
        for other in [
            "",
            "commander",
            "CCOMMANDER",
            "historic",
            "DHOBHOBHOB",
            "cube",
        ] {
            assert_eq!(format_slug(other), None, "{other}");
        }
    }

    // Spec 16.4 and the API `MetagameSharesTest` "sorts by share then name".
    #[test]
    fn parse_metagame_reads_the_spec_shape() {
        let body = r#"{
            "format": "modern",
            "days": 30,
            "window_start": "2026-09-10",
            "window_end": "2026-10-09",
            "matches": 10011,
            "archetypes": [
                {"uuid": "6f1c2e4a-0000-4000-8000-000000000001", "name": "Ruby Storm",
                 "color_identity": "R", "share": 6.2, "matches": 1241},
                {"uuid": "6f1c2e4a-0000-4000-8000-000000000002", "name": "Eldrazi Tron",
                 "color_identity": null, "share": 44, "matches": 40}
            ],
            "other_share": 7.4,
            "unnameable_share": 3.1
        }"#;
        assert_eq!(
            parse_metagame(&response(200, body)),
            Ok(MetagameShares {
                format: "modern".to_string(),
                days: 30,
                window_start: "2026-09-10".to_string(),
                window_end: "2026-10-09".to_string(),
                matches: 10_011,
                archetypes: vec![
                    GlobalShare {
                        uuid: "6f1c2e4a-0000-4000-8000-000000000001".to_string(),
                        name: "Ruby Storm".to_string(),
                        color_identity: Some("R".to_string()),
                        share: Some(6.2),
                        matches: 1_241,
                        cards: Vec::new(),
                    },
                    GlobalShare {
                        uuid: "6f1c2e4a-0000-4000-8000-000000000002".to_string(),
                        name: "Eldrazi Tron".to_string(),
                        color_identity: None,
                        share: Some(44.0),
                        matches: 40,
                        cards: Vec::new(),
                    },
                ],
                other_share: Some(7.4),
                unnameable_share: Some(3.1),
            })
        );
    }

    // the API "answers an empty window with zero matches and null shares".
    #[test]
    fn an_empty_window_has_no_shares() {
        let body = r#"{"format": "legacy", "days": 30, "window_start": "2026-09-10",
            "window_end": "2026-10-09", "matches": 0, "archetypes": [],
            "other_share": null, "unnameable_share": null}"#;
        let shares = parse_metagame(&response(200, body)).expect("parsed");
        assert_eq!(shares.format, "legacy");
        assert_eq!(shares.matches, 0);
        assert!(shares.archetypes.is_empty());
        assert_eq!(shares.other_share, None);
        assert_eq!(shares.unnameable_share, None);
    }

    // A row without a uuid cannot be matched to a held archetype and is
    // dropped; a null share stays unknown.
    #[test]
    fn rows_without_a_uuid_are_dropped() {
        let body = r#"{"format": "modern", "days": 7, "window_start": "2026-10-03",
            "window_end": "2026-10-09", "matches": "45", "archetypes": [
                {"name": "Qnameless", "share": 1.0, "matches": 1},
                {"uuid": "q-arch", "name": "Qarch", "share": null, "matches": "3"},
                "q"
            ]}"#;
        let shares = parse_metagame(&response(200, body)).expect("parsed");
        assert_eq!(shares.matches, 45);
        assert_eq!(
            shares.archetypes,
            vec![GlobalShare {
                uuid: "q-arch".to_string(),
                name: "Qarch".to_string(),
                color_identity: None,
                share: None,
                matches: 3,
                cards: Vec::new(),
            }]
        );
        assert_eq!(shares.other_share, None);
    }

    // the API "sends each archetype's key cards": the website tile's cards
    // in its order. A card without an image cannot be drawn and is dropped;
    // `cards` that is not a list reads as none.
    #[test]
    fn rows_carry_their_tile_cards() {
        let body = r#"{"format": "modern", "days": 7, "archetypes": [
                {"uuid": "q-one", "name": "Qone", "share": 1.0, "matches": 1, "cards": [
                    {"name": "Qbolt", "image": "https://cards.scryfall.io/normal/b.jpg",
                     "art": "https://cards.scryfall.io/art_crop/b.jpg"},
                    {"name": "Qnoimage", "image": null, "art": "x"},
                    {"name": "Qblank", "image": ""},
                    "q",
                    {"name": "Qvial", "image": "https://cards.scryfall.io/normal/v.jpg", "art": null}
                ]},
                {"uuid": "q-two", "name": "Qtwo", "share": 1.0, "matches": 1, "cards": {}}
            ]}"#;
        let shares = parse_metagame(&response(200, body)).expect("parsed");
        assert_eq!(
            shares.archetypes[0].cards,
            vec![
                GlobalCard {
                    name: "Qbolt".to_string(),
                    image: "https://cards.scryfall.io/normal/b.jpg".to_string(),
                    art: Some("https://cards.scryfall.io/art_crop/b.jpg".to_string()),
                },
                GlobalCard {
                    name: "Qvial".to_string(),
                    image: "https://cards.scryfall.io/normal/v.jpg".to_string(),
                    art: None,
                },
            ]
        );
        assert!(shares.archetypes[1].cards.is_empty());
    }

    #[test]
    fn a_failed_or_malformed_answer_is_a_typed_error() {
        assert_eq!(
            parse_metagame(&response(404, r#"{"message":"Unknown format."}"#)),
            Err(ApiError::Status(
                404,
                r#"{"message":"Unknown format."}"#.to_string()
            ))
        );
        assert!(matches!(
            parse_metagame(&response(422, "{}")),
            Err(ApiError::Status(422, _))
        ));
        for body in ["[1", "", "[]", r#"{"archetypes": {}}"#] {
            assert!(
                matches!(
                    parse_metagame(&response(200, body)),
                    Err(ApiError::Malformed(_))
                ),
                "{body:?}"
            );
        }
    }
}
