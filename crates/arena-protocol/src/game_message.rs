//! Raw game messages: the parts of an Arena log entry that describe a game
//! (the GRE's messages to the client and the client's messages to the GRE),
//! cut out one by one and kept as compact JSON for later folding.

use serde_json::Value;

use crate::clock::timestamp_to_ms;
use crate::split::RawEntry;

/// Where a game message came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageSource {
    Gre,
    Client,
}

/// One game message of a log entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameMessage {
    pub source: MessageSource,
    /// The message's own JSON (one element of greToClientMessages, or the
    /// client payload), compact, not the whole entry.
    pub json: String,
    /// `gameInfo.matchID` when this message carries one.
    pub match_id: Option<String>,
    /// `gameInfo.gameNumber` when this message carries one.
    pub game_number: Option<u32>,
    /// `gameInfo.matchWinCondition == "MatchWinCondition_Best2of3"` when present.
    pub best_of_three: Option<bool>,
    /// The entry's `timestamp` field (unix ms) when present.
    pub at_ms: Option<i64>,
}

const GRE_NOISE: [&str; 2] = [
    "GREMessageType_UIMessage",
    "GREMessageType_TimerStateMessage",
];
const CLIENT_TO_GRE: &str = "ClientToMatchServiceMessageType_ClientToGREMessage";
const CLIENT_UI: &str = "ClientMessageType_UIMessage";

/// The GRE message type that ends a game (an intermission).
pub const INTERMISSION_TYPE: &str = "GREMessageType_IntermissionReq";

/// Whether a GRE message's JSON is an intermission: its top-level `type` is
/// `INTERMISSION_TYPE`.
pub fn is_intermission_json(json: &str) -> bool {
    if !json.contains(INTERMISSION_TYPE) {
        return false;
    }
    serde_json::from_str::<Value>(json)
        .ok()
        .and_then(|value| {
            value
                .get("type")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .is_some_and(|kind| kind == INTERMISSION_TYPE)
}

impl GameMessage {
    /// Whether this is the GRE's intermission (a game is over).
    pub fn is_intermission(&self) -> bool {
        self.source == MessageSource::Gre && is_intermission_json(&self.json)
    }
}

/// The game messages of one entry, in array order (none for an entry that
/// carries no game message).
pub fn game_messages(entry: &RawEntry) -> Vec<GameMessage> {
    let Some(text) = entry.json.as_deref() else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_str::<Value>(text) else {
        return Vec::new();
    };
    let at_ms = entry_time(&value);
    if let Some(gre) = value.get("greToClientEvent") {
        return gre_messages(gre, at_ms);
    }
    client_message(&value, at_ms).into_iter().collect()
}

fn gre_messages(gre: &Value, at_ms: Option<i64>) -> Vec<GameMessage> {
    let list = gre.get("greToClientMessages").and_then(Value::as_array);
    let mut messages = Vec::new();
    for message in list.into_iter().flatten() {
        let kind = message.get("type").and_then(Value::as_str).unwrap_or("");
        if GRE_NOISE.contains(&kind) {
            continue;
        }
        let info = message
            .get("gameStateMessage")
            .and_then(|state| state.get("gameInfo"))
            .filter(|info| info.is_object());
        messages.push(GameMessage {
            source: MessageSource::Gre,
            json: message.to_string(),
            match_id: info
                .and_then(|i| i.get("matchID"))
                .and_then(Value::as_str)
                .map(str::to_string),
            game_number: info
                .and_then(|i| i.get("gameNumber"))
                .and_then(Value::as_u64)
                .and_then(|n| u32::try_from(n).ok()),
            best_of_three: info
                .and_then(|i| i.get("matchWinCondition"))
                .and_then(Value::as_str)
                .map(|w| w == "MatchWinCondition_Best2of3"),
            at_ms,
        });
    }
    messages
}

fn client_message(value: &Value, at_ms: Option<i64>) -> Option<GameMessage> {
    let kind = value.get("clientToMatchServiceMessageType")?.as_str()?;
    if kind != CLIENT_TO_GRE {
        return None;
    }
    let payload = value.get("payload")?;
    if payload.get("type").and_then(Value::as_str) == Some(CLIENT_UI) {
        return None;
    }
    Some(GameMessage {
        source: MessageSource::Client,
        json: payload.to_string(),
        match_id: None,
        game_number: None,
        best_of_three: None,
        at_ms,
    })
}

/// The entry's `timestamp` (unix ms or .NET ticks, string or number) in unix ms.
fn entry_time(value: &Value) -> Option<i64> {
    let raw = match value.get("timestamp")? {
        Value::String(text) => text.parse().ok()?,
        Value::Number(number) => number.as_i64()?,
        _ => return None,
    };
    Some(timestamp_to_ms(raw))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::split::split_complete;

    fn entry(json: &str) -> RawEntry {
        RawEntry {
            offset: 0,
            len: json.len() as u64,
            head: String::new(),
            json: Some(json.to_string()),
        }
    }

    #[test]
    fn keeps_gre_state_messages_and_drops_ui_and_timer() {
        let json = r#"{"timestamp":"1793709133013","greToClientEvent":{"greToClientMessages":[
            {"type":"GREMessageType_GameStateMessage","gameStateMessage":{"gameStateId":1}},
            {"type":"GREMessageType_UIMessage","uiMessage":{}},
            {"type":"GREMessageType_TimerStateMessage","timerStateMessage":{}}]}}"#;
        let messages = game_messages(&entry(json));
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].source, MessageSource::Gre);
        assert_eq!(messages[0].at_ms, Some(1793709133013));
        assert!(messages[0].json.contains("GREMessageType_GameStateMessage"));
        assert!(!messages[0].json.contains('\n'));
    }

    #[test]
    fn an_intermission_is_a_gre_message_of_that_type() {
        assert!(is_intermission_json(
            r#"{"type":"GREMessageType_IntermissionReq"}"#
        ));
        // The words inside another field do not count.
        assert!(!is_intermission_json(
            r#"{"type":"GREMessageType_GameStateMessage","note":"GREMessageType_IntermissionReq"}"#
        ));
        assert!(!is_intermission_json(
            "not json GREMessageType_IntermissionReq"
        ));
    }

    #[test]
    fn reads_game_info_fields() {
        let json = r#"{"greToClientEvent":{"greToClientMessages":[
            {"type":"GREMessageType_GameStateMessage","gameStateMessage":{"gameInfo":
            {"matchID":"m","gameNumber":2,"matchWinCondition":"MatchWinCondition_Best2of3"}}}]}}"#;
        let messages = game_messages(&entry(json));
        assert_eq!(messages[0].match_id.as_deref(), Some("m"));
        assert_eq!(messages[0].game_number, Some(2));
        assert_eq!(messages[0].best_of_three, Some(true));
        assert_eq!(messages[0].at_ms, None);
    }

    #[test]
    fn keeps_client_to_gre_payload_not_ui() {
        let log = "[UnityCrossThreadLogger]03/11/2026 13:03:33: P to Match: ClientToGremessage\n{\n  \"requestId\": 3,\n  \"clientToMatchServiceMessageType\": \"ClientToMatchServiceMessageType_ClientToGREMessage\",\n  \"timestamp\": \"639293078135768350\",\n  \"payload\": {\n    \"type\": \"ClientMessageType_MulliganResp\",\n    \"mulliganResp\": {\"decision\": \"MulliganOption_AcceptHand\"}\n  }\n}\nUnityEngine.DebugLogHandler:Internal_Log(LogType, LogOption, String, Object)\n\n[UnityCrossThreadLogger]03/11/2026 13:03:34: P to Match: ClientToGreuimessage\n{\n  \"clientToMatchServiceMessageType\": \"ClientToMatchServiceMessageType_ClientToGREUIMessage\",\n  \"payload\": {\"type\": \"ClientMessageType_UIMessage\"}\n}\n";
        let entries = split_complete(log.as_bytes());
        let messages: Vec<GameMessage> = entries.iter().flat_map(game_messages).collect();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].source, MessageSource::Client);
        let payload: serde_json::Value = serde_json::from_str(&messages[0].json).unwrap();
        assert_eq!(payload["type"], "ClientMessageType_MulliganResp");
        assert!(messages[0].at_ms.is_some());
    }

    #[test]
    fn fixture_counts() {
        for (name, expected_clients) in [
            ("bo3-loss-and-match-concede", 146),
            ("bo3-three-games", 156),
        ] {
            let entries = split_complete(&fixture_tools::arena_fixture(name).bytes);
            let wanted = entries
                .iter()
                .filter(|e| {
                    e.json.as_deref().is_some_and(|j| {
                        j.contains("\"ClientToMatchServiceMessageType_ClientToGREMessage\"")
                    })
                })
                .count();
            let messages: Vec<GameMessage> = entries.iter().flat_map(game_messages).collect();
            let clients = messages
                .iter()
                .filter(|m| m.source == MessageSource::Client)
                .count();
            assert_eq!(wanted, expected_clients, "{name}");
            assert_eq!(clients, wanted, "{name}");
            assert!(
                messages
                    .iter()
                    .all(|m| !m.json.contains("\"GREMessageType_UIMessage\""))
            );
            assert!(messages.iter().any(|m| m.source == MessageSource::Gre));
        }
    }
}
