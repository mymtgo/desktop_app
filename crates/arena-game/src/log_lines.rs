//! Plain game log lines from zone transfers: the local player's draws and
//! both players' land plays and casts, for the match detail game log panel.

use crate::{CARD, Fold, catalog_id};

/// What a log line says a player did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogVerb {
    Drew,
    Played,
    Cast,
}

/// One game log line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogLine {
    /// When the transfer was logged (UTC unix ms).
    pub at_ms: i64,
    /// The local player's card (else the opponent's).
    pub local: bool,
    pub verb: LogVerb,
    pub catalog_id: u32,
}

/// The game's log lines, in order. Draws are the local player's only (the
/// opponent's are hidden); only cards count, never tokens or abilities. A
/// fold that never saw the game start has none.
pub fn log_lines(fold: &Fold) -> Vec<LogLine> {
    let mut lines = Vec::new();
    if !fold.started {
        return lines;
    }
    for transfer in &fold.transfers {
        // An owner nothing told is neither player's.
        if transfer.kind != CARD || transfer.owner_seat == 0 {
            continue;
        }
        let Some(catalog_id) = catalog_id(transfer.grp_id) else {
            continue;
        };
        let local = Some(transfer.owner_seat) == fold.local_seat;
        let verb = match transfer.category.as_str() {
            "Draw" if local => LogVerb::Drew,
            "PlayLand" => LogVerb::Played,
            "CastSpell" => LogVerb::Cast,
            _ => continue,
        };
        lines.push(LogLine {
            at_ms: transfer.logged_at,
            local,
            verb,
            catalog_id,
        });
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{GameInput, Source, fold_game};

    fn gre(json: &str, logged_at: i64) -> GameInput {
        GameInput {
            source: Source::Gre,
            logged_at,
            json: json.to_string(),
        }
    }

    const FULL: &str = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[1],
      "gameStateMessage":{"type":"GameStateType_Full","gameStateId":1,
        "zones":[
          {"zoneId":31,"type":"ZoneType_Hand","ownerSeatId":1,"objectInstanceIds":[]},
          {"zoneId":32,"type":"ZoneType_Library","ownerSeatId":1,"objectInstanceIds":[161]},
          {"zoneId":35,"type":"ZoneType_Hand","ownerSeatId":2,"objectInstanceIds":[]},
          {"zoneId":36,"type":"ZoneType_Library","ownerSeatId":2,"objectInstanceIds":[700,701]},
          {"zoneId":28,"type":"ZoneType_Battlefield","objectInstanceIds":[]}],
        "gameObjects":[
          {"instanceId":161,"grpId":5,"type":"GameObjectType_Card","zoneId":32,"visibility":"Visibility_Private","ownerSeatId":1},
          {"instanceId":700,"grpId":9,"type":"GameObjectType_Card","zoneId":36,"visibility":"Visibility_Private","ownerSeatId":2},
          {"instanceId":701,"grpId":10,"type":"GameObjectType_Card","zoneId":36,"visibility":"Visibility_Private","ownerSeatId":2}]}}"#;

    /// One zone transfer of `id` with this category.
    fn moved(state_id: u32, id: u32, from: u32, to: u32, category: &str) -> String {
        format!(
            r#"{{"type":"GREMessageType_GameStateMessage","systemSeatIds":[1],
              "gameStateMessage":{{"type":"GameStateType_Diff","gameStateId":{state_id},
                "annotations":[{{"id":{state_id},"affectedIds":[{id}],"type":["AnnotationType_ZoneTransfer"],
                  "details":[{{"key":"zone_src","valueInt32":[{from}]}},{{"key":"zone_dest","valueInt32":[{to}]}},
                             {{"key":"category","valueString":["{category}"]}}]}}]}}}}"#
        )
    }

    #[test]
    fn log_lines_hide_opponent_draws() {
        let fold = fold_game(&[
            gre(FULL, 1000),
            gre(&moved(2, 161, 32, 31, "Draw"), 2000),
            gre(&moved(3, 700, 36, 35, "Draw"), 3000),
            gre(&moved(4, 701, 35, 28, "PlayLand"), 4000),
            gre(&moved(5, 161, 31, 28, "CastSpell"), 5000),
        ]);
        let id = |grp| cards::arena_catalog_id(cards::GrpId(grp));
        assert_eq!(
            log_lines(&fold),
            vec![
                LogLine {
                    at_ms: 2000,
                    local: true,
                    verb: LogVerb::Drew,
                    catalog_id: id(5),
                },
                LogLine {
                    at_ms: 4000,
                    local: false,
                    verb: LogVerb::Played,
                    catalog_id: id(10),
                },
                LogLine {
                    at_ms: 5000,
                    local: true,
                    verb: LogVerb::Cast,
                    catalog_id: id(5),
                },
            ]
        );
    }

    #[test]
    fn log_lines_skip_cards_with_no_known_owner() {
        let unowned = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[1],
          "gameStateMessage":{"type":"GameStateType_Diff","gameStateId":2,
            "gameObjects":[{"instanceId":800,"grpId":11,"type":"GameObjectType_Card","zoneId":99,"visibility":"Visibility_Public"}]}}"#;
        let fold = fold_game(&[
            gre(FULL, 1000),
            gre(unowned, 1500),
            gre(&moved(3, 800, 98, 99, "CastSpell"), 2000),
        ]);
        assert_eq!(fold.transfers[0].owner_seat, 0);
        assert_eq!(log_lines(&fold), Vec::new());
    }

    #[test]
    fn log_lines_skip_tokens() {
        let token = r#"{"type":"GREMessageType_GameStateMessage","systemSeatIds":[1],
          "gameStateMessage":{"type":"GameStateType_Diff","gameStateId":2,
            "gameObjects":[{"instanceId":161,"grpId":5,"type":"GameObjectType_Token","zoneId":28,"visibility":"Visibility_Public","ownerSeatId":1}]}}"#;
        let fold = fold_game(&[
            gre(FULL, 1000),
            gre(token, 1500),
            gre(&moved(3, 161, 31, 28, "CastSpell"), 2000),
        ]);
        assert_eq!(log_lines(&fold), Vec::new());
    }
}
