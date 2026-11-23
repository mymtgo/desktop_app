//! Player names from a `game_state_update` event's JSON.

use serde::Deserialize;

#[derive(Deserialize)]
struct StateLite {
    #[serde(rename = "Players", default)]
    players: Vec<PlayerLite>,
}

#[derive(Deserialize)]
struct PlayerLite {
    #[serde(rename = "Name")]
    name: String,
}

/// Names from the `Players` array, in MTGO's order. `None` when the JSON is unreadable.
pub fn parse_players(text: &str) -> Option<Vec<String>> {
    let start = text.find("{\"Players\"")?;
    let mut stream = serde_json::Deserializer::from_str(&text[start..]).into_iter::<StateLite>();
    let state = stream.next()?.ok()?;
    Some(
        state
            .players
            .into_iter()
            .map(|player| player.name)
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_player_names_and_ignores_cards() {
        let text = "12:25:50 [INF] (Twitch Info|Game Play Status Update for Game ID: 1, Match ID: 2, Event ID: 2) {\"Players\":[{\"Id\":0,\"Name\":\"Steve O\",\"Life\":20},{\"Id\":1,\"Name\":\"abc\",\"Life\":20}],\"Cards\":[{\"Id\":333,\"CatalogID\":62561}]}";
        assert_eq!(
            parse_players(text),
            Some(vec!["Steve O".to_string(), "abc".to_string()])
        );
    }

    #[test]
    fn broken_json_gives_none() {
        assert_eq!(parse_players("x {\"Players\":[{\"Id\":0,"), None);
    }
}
