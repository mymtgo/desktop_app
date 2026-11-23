//! The raw source section of an Arena or Endstep match bundle (cross-device
//! sync spec section 5.4). MTGO bundles carry `timelines`; these carry
//! `sources`, the stored data the receiving device folds again.
//!
//! The section is plain JSON, so canonical form (sorted keys) gives the
//! same bytes for the same source on every device.

use serde_json::{Value, json};

use crate::deck::BundleError;
use crate::loose;
use crate::platform::BundlePlatform;

/// One stored Arena game message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArenaSourceMessage {
    pub source: String,
    pub logged_at_ms: i64,
    pub raw: String,
}

/// One game's messages, in their stored order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArenaGameSource {
    pub game_number: u32,
    pub messages: Vec<ArenaSourceMessage>,
}

/// An Arena match's source: the event id and every game's messages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArenaSources {
    pub event_id: Option<String>,
    pub games: Vec<ArenaGameSource>,
}

/// One Endstep capture unit trimmed to this match: the unit id and the
/// raw capture lines kept for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EndstepSourceUnit {
    pub unit_id: String,
    pub lines: Vec<String>,
}

/// An Endstep match's source: its game id and the trimmed units.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EndstepSources {
    pub forge_game_id: String,
    pub units: Vec<EndstepSourceUnit>,
}

/// A match's raw source, by platform.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MatchSources {
    Arena(ArenaSources),
    Endstep(EndstepSources),
}

/// The `sources` section of a bundle.
pub fn sources_section(sources: &MatchSources) -> Value {
    match sources {
        MatchSources::Arena(arena) => arena_section(arena),
        MatchSources::Endstep(endstep) => endstep_section(endstep),
    }
}

fn arena_section(arena: &ArenaSources) -> Value {
    let games: Vec<Value> = arena
        .games
        .iter()
        .map(|game| {
            let messages: Vec<Value> = game
                .messages
                .iter()
                .map(|message| {
                    json!({
                        "source": message.source,
                        "logged_at_ms": message.logged_at_ms,
                        "raw": message.raw,
                    })
                })
                .collect();
            json!({ "game_number": game.game_number, "messages": messages })
        })
        .collect();
    json!({ "event_id": arena.event_id, "games": games })
}

fn endstep_section(endstep: &EndstepSources) -> Value {
    let units: Vec<Value> = endstep
        .units
        .iter()
        .map(|unit| json!({ "unit_id": unit.unit_id, "lines": unit.lines }))
        .collect();
    json!({ "forge_game_id": endstep.forge_game_id, "units": units })
}

fn missing(part: &str) -> BundleError {
    BundleError::Missing(part.to_string())
}

fn invalid(part: &str) -> BundleError {
    BundleError::Invalid(part.to_string())
}

fn list<'a>(value: &'a Value, part: &str) -> Result<&'a Vec<Value>, BundleError> {
    value.as_array().ok_or_else(|| invalid(part))
}

fn required_text(value: &Value, part: &str) -> Result<String, BundleError> {
    match value {
        Value::Null => Err(missing(part)),
        Value::String(text) => Ok(text.clone()),
        _ => Err(invalid(part)),
    }
}

/// Reads a bundle's `sources` for its platform. No section (or null) is
/// `None`; MTGO has no sources, so a section on an MTGO bundle is ignored.
/// A section that does not read is an error.
pub fn read_sources(
    bundle: &Value,
    platform: BundlePlatform,
) -> Result<Option<MatchSources>, BundleError> {
    let section = match bundle.get("sources") {
        None | Some(Value::Null) => return Ok(None),
        Some(section) => section,
    };
    match platform {
        BundlePlatform::Mtgo => Ok(None),
        BundlePlatform::Arena => Ok(Some(MatchSources::Arena(read_arena(section)?))),
        BundlePlatform::Endstep => Ok(Some(MatchSources::Endstep(read_endstep(section)?))),
    }
}

fn read_arena(section: &Value) -> Result<ArenaSources, BundleError> {
    let mut games = Vec::new();
    for game in list(&section["games"], "sources.games")? {
        let number = loose::int(&game["game_number"])
            .and_then(|number| u32::try_from(number).ok())
            .ok_or_else(|| invalid("sources.games.game_number"))?;
        let mut messages = Vec::new();
        for message in list(&game["messages"], "sources.games.messages")? {
            messages.push(ArenaSourceMessage {
                source: required_text(&message["source"], "sources.games.messages.source")?,
                logged_at_ms: loose::int(&message["logged_at_ms"])
                    .ok_or_else(|| invalid("sources.games.messages.logged_at_ms"))?,
                raw: required_text(&message["raw"], "sources.games.messages.raw")?,
            });
        }
        games.push(ArenaGameSource {
            game_number: number,
            messages,
        });
    }
    Ok(ArenaSources {
        event_id: loose::text(&section["event_id"]),
        games,
    })
}

fn read_endstep(section: &Value) -> Result<EndstepSources, BundleError> {
    let forge_game_id = required_text(&section["forge_game_id"], "sources.forge_game_id")?;
    let mut units = Vec::new();
    for unit in list(&section["units"], "sources.units")? {
        let mut lines = Vec::new();
        for line in list(&unit["lines"], "sources.units.lines")? {
            lines.push(required_text(line, "sources.units.lines")?);
        }
        units.push(EndstepSourceUnit {
            unit_id: required_text(&unit["unit_id"], "sources.units.unit_id")?,
            lines,
        });
    }
    Ok(EndstepSources {
        forge_game_id,
        units,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canonical::canonical_json;

    fn arena() -> MatchSources {
        MatchSources::Arena(ArenaSources {
            event_id: Some("Play_Brawl_Historic".to_string()),
            games: vec![ArenaGameSource {
                game_number: 1,
                messages: vec![
                    ArenaSourceMessage {
                        source: "gre".to_string(),
                        logged_at_ms: 1_700_000_000_123,
                        raw: "{\"a\":1}".to_string(),
                    },
                    ArenaSourceMessage {
                        source: "client".to_string(),
                        logged_at_ms: 1_700_000_000_456,
                        raw: "x".to_string(),
                    },
                ],
            }],
        })
    }

    fn endstep() -> MatchSources {
        MatchSources::Endstep(EndstepSources {
            forge_game_id: "g-1".to_string(),
            units: vec![EndstepSourceUnit {
                unit_id: "u1".to_string(),
                lines: vec!["l1".to_string(), "l2".to_string()],
            }],
        })
    }

    #[test]
    fn arena_sources_round_trip() {
        let bundle = json!({ "sources": sources_section(&arena()) });
        assert_eq!(
            read_sources(&bundle, BundlePlatform::Arena),
            Ok(Some(arena()))
        );
    }

    #[test]
    fn endstep_sources_round_trip() {
        let bundle = json!({ "sources": sources_section(&endstep()) });
        assert_eq!(
            read_sources(&bundle, BundlePlatform::Endstep),
            Ok(Some(endstep()))
        );
    }

    #[test]
    fn no_section_reads_as_none_and_a_broken_one_is_an_error() {
        assert_eq!(read_sources(&json!({}), BundlePlatform::Arena), Ok(None));
        assert_eq!(
            read_sources(&json!({ "sources": null }), BundlePlatform::Endstep),
            Ok(None)
        );
        assert!(
            read_sources(&json!({ "sources": { "games": 3 } }), BundlePlatform::Arena).is_err()
        );
    }

    #[test]
    fn the_section_has_stable_canonical_bytes() {
        assert_eq!(
            canonical_json(&sources_section(&endstep())),
            r#"{"forge_game_id":"g-1","units":[{"lines":["l1","l2"],"unit_id":"u1"}]}"#
        );
    }
}
