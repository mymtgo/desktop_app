//! The snapshot builder (package `BuildReplaySnapshot`, fed as 0.x's
//! `BuildReplaySnapshot` feeds it).

use std::collections::BTreeMap;

use crate::format::format_key;
use crate::frames::{catalog_id, decorate_content, https_only};
use crate::repair::{repair_active_player, repair_midnight_wrap};
use crate::snapshot::{
    CardInfo, ImageMode, LogEntry, Meta, ReplayFrame, SideboardEntry, Snapshot, SnapshotGame,
    VERSION,
};

/// One match as the builder reads it, from stored rows.
#[derive(Debug, Clone, PartialEq)]
pub struct SnapshotInput {
    /// MTGO's format code (`CMODERN`), reduced by `format_key`.
    pub format: Option<String>,
    /// The first game's start, else the match's, as UTC unix milliseconds.
    pub played_at_ms: i64,
    pub local_archetype: Option<String>,
    pub opponent_archetype: Option<String>,
    /// Every game of the match in play order, frameless games included.
    pub games: Vec<GameInput>,
}

/// One game of the match.
#[derive(Debug, Clone, PartialEq)]
pub struct GameInput {
    pub won: Option<bool>,
    pub local_username: String,
    pub frames: Vec<ReplayFrame>,
    pub log: Vec<LogEntry>,
    /// Your sideboard as the game began: `(catalog id, quantity)`.
    pub sideboard: Vec<(u32, u32)>,
}

/// Package `BuildReplaySnapshot::run`: one match as a self-contained replay,
/// unredacted (`prepare_shared_snapshot` removes the opponent's name).
///
/// - A game without frames is skipped but keeps its number
///   (`game_number` = its position among all games), so gaps stay.
/// - Each game's frames go through `repair_midnight_wrap`, then
///   `repair_active_player`, then card and player decoration with
///   `https://` images only.
/// - `cards` is asked once per game, with each catalog id of the frames and
///   the sideboard once in first-seen order, and only when there is one.
/// - `sideboard` is written only when the game has one.
/// - `meta.played_at` is Carbon's `toIso8601String()` in UTC;
///   `games_in_match` counts every game, at least 1.
pub fn build_snapshot(
    input: SnapshotInput,
    cards: &dyn Fn(&[u32]) -> BTreeMap<u32, CardInfo>,
) -> Snapshot {
    let games_in_match = u32::try_from(input.games.len().max(1)).unwrap_or(u32::MAX);
    let mut games = Vec::new();

    for (index, game) in input.games.into_iter().enumerate() {
        if game.frames.is_empty() {
            continue;
        }

        let frames = repair_active_player(repair_midnight_wrap(game.frames));
        let resolved = resolve(&frames, &game.sideboard, cards);

        let mut decorated = frames;
        for frame in &mut decorated {
            decorate_content(
                &mut frame.content,
                Some(&game.local_username),
                &resolved,
                ImageMode::Portable,
            );
        }

        let sideboard = if game.sideboard.is_empty() {
            None
        } else {
            Some(sideboard_entries(&game.sideboard, &resolved))
        };

        games.push(SnapshotGame {
            game_number: u32::try_from(index + 1).unwrap_or(u32::MAX),
            won: game.won,
            frames: decorated,
            log: game.log,
            sideboard,
        });
    }

    Snapshot {
        version: VERSION,
        meta: Meta {
            format: format_key(input.format.as_deref()),
            played_at: iso8601_utc(input.played_at_ms),
            games_in_match,
            local_archetype: input.local_archetype,
            opponent_archetype: input.opponent_archetype,
        },
        games,
    }
}

/// Package `resolve`: one catalog call for every card a game mentions,
/// frames and sideboard alike.
fn resolve(
    frames: &[ReplayFrame],
    sideboard: &[(u32, u32)],
    cards: &dyn Fn(&[u32]) -> BTreeMap<u32, CardInfo>,
) -> BTreeMap<u32, CardInfo> {
    let mut ids: Vec<u32> = Vec::new();
    let mut add = |id: u32| {
        if !ids.contains(&id) {
            ids.push(id);
        }
    };

    for frame in frames {
        if let Some(list) = frame
            .content
            .get("Cards")
            .and_then(|cards| cards.as_array())
        {
            for card in list {
                if let Some(id) = catalog_id(card) {
                    add(id);
                }
            }
        }
    }
    for (id, _quantity) in sideboard {
        add(*id);
    }

    if ids.is_empty() {
        BTreeMap::new()
    } else {
        cards(&ids)
    }
}

/// Package `sideboard`: each entry with the card's name, type and image,
/// `https://` images only.
fn sideboard_entries(
    sideboard: &[(u32, u32)],
    resolved: &BTreeMap<u32, CardInfo>,
) -> Vec<SideboardEntry> {
    let mut entries = Vec::new();
    for (catalog_id, quantity) in sideboard {
        let known = resolved.get(catalog_id);
        entries.push(SideboardEntry {
            catalog_id: *catalog_id,
            quantity: *quantity,
            name: known.and_then(|known| known.name.clone()),
            type_line: known.and_then(|known| known.type_line.clone()),
            image: known.and_then(|known| https_only(known.image.as_deref())),
        });
    }
    entries
}

/// Carbon's `toIso8601String()` for a UTC unix millisecond time:
/// `YYYY-MM-DDTHH:MM:SS+00:00`, milliseconds dropped. A time outside
/// chrono's range (hundreds of thousands of years away) renders as the
/// epoch. The one copy: `mymtgo_api::php::iso8601_utc` calls this
/// (`mymtgo-api` depends on `replay` for the share request, so not the
/// other way round). Pinned by `fixtures/php/iso8601_utc.json`.
pub fn iso8601_utc(ms: i64) -> String {
    let time = chrono::DateTime::from_timestamp_millis(ms).unwrap_or_default();
    time.format("%Y-%m-%dT%H:%M:%S+00:00").to_string()
}

#[cfg(test)]
mod tests {
    use super::iso8601_utc;

    /// The shared table, which also pins `mymtgo_api::php::iso8601_utc`
    /// (a call to this one).
    #[test]
    fn played_at_is_carbons_iso_8601_in_utc() {
        let cases: Vec<(i64, String)> =
            serde_json::from_str(include_str!("../../../fixtures/php/iso8601_utc.json"))
                .expect("the shared iso8601_utc table is JSON");

        assert!(!cases.is_empty());
        for (ms, expected) in cases {
            assert_eq!(iso8601_utc(ms), expected, "{ms}");
        }
    }
}
