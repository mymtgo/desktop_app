//! The game log rebuilt from `mtgo.log` MetaMessages, for a match whose `.dat`
//! file is missing (0.x `ExtractMetaMessageEntries`). Each MetaMessage byte
//! array is decoded with the phrase whitelist of 0.x `DecodeMetaMessageText`,
//! which v1 already has as `mtgo_protocol::decode_meta_message`.

use mtgo_protocol::{decode_meta_message, parse_game_management};

use crate::gamelog::GameLogEntry;

/// One stored `game_management_json` event of a match.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetaEvent {
    /// The event's UTC unix milliseconds.
    pub at_ms: i64,
    /// The event's full text.
    pub text: String,
}

/// The readable phrase in a MetaMessage byte array, or `None` when it carries
/// no recognised text (0.x `DecodeMetaMessageText`).
pub fn decode_meta_text(bytes: &[u8]) -> Option<String> {
    decode_meta_message(bytes)
}

/// Game log entries from a match's `game_management_json` events, in the
/// order given (callers pass them in `(file_id, byte_offset)` order). Events
/// with no MetaMessage or no recognised phrase are skipped.
pub fn meta_message_entries(events: &[MetaEvent]) -> Vec<GameLogEntry> {
    let mut entries = Vec::new();
    for event in events {
        let Some(bytes) = parse_game_management(&event.text).meta_message else {
            continue;
        };
        let Some(message) = decode_meta_text(&bytes) else {
            continue;
        };
        entries.push(GameLogEntry {
            timestamp_ms: event.at_ms,
            message,
        });
    }
    entries
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A chat MetaMessage as 0.x's tests build it: a 24-byte envelope (u32
    /// length, type 3, header bytes, u32 text length), then the text.
    fn chat(text: &str) -> Vec<u8> {
        let len = text.len() as u8;
        let mut bytes = vec![
            len + 24,
            0,
            0,
            0,
            3,
            17,
            186,
            129,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
        ];
        bytes.extend_from_slice(&[len, 0, 0, 0]);
        bytes.extend_from_slice(text.as_bytes());
        bytes
    }

    /// The real envelope bytes 0.x's first tests used, with a fake name.
    fn real_envelope(text: &str) -> Vec<u8> {
        let len = text.len() as u8;
        let mut bytes = vec![
            len + 24,
            0,
            0,
            0,
            3,
            17,
            186,
            129,
            94,
            148,
            91,
            56,
            103,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
        ];
        bytes.extend_from_slice(&[len, 0, 0, 0]);
        bytes.extend_from_slice(text.as_bytes());
        bytes
    }

    fn decodes_to_itself(text: &str) {
        assert_eq!(decode_meta_text(&chat(text)).as_deref(), Some(text));
    }

    #[test]
    fn extracts_player_roll_text() {
        let text = "@PQlocalxxxx rolled a 4.";
        assert_eq!(
            decode_meta_text(&real_envelope(text)).as_deref(),
            Some(text)
        );
    }

    #[test]
    fn extracts_player_joined_text_with_double_at_p_prefix() {
        let text = "@P@PQlocalxxxx joined the game.";
        assert_eq!(
            decode_meta_text(&real_envelope(text)).as_deref(),
            Some(text)
        );
    }

    #[test]
    fn extracts_chooses_to_play_text() {
        let text = "@PQopponentxxx chooses to play first.";
        assert_eq!(
            decode_meta_text(&real_envelope(text)).as_deref(),
            Some(text)
        );
    }

    #[test]
    fn extracts_begins_with_n_cards_text() {
        let text = "@PQopponentxxx begins the game with seven cards in hand.";
        assert_eq!(
            decode_meta_text(&real_envelope(text)).as_deref(),
            Some(text)
        );
    }

    #[test]
    fn extracts_wins_the_game_text() {
        let text = "@PQopponentxxx wins the game.";
        assert_eq!(
            decode_meta_text(&real_envelope(text)).as_deref(),
            Some(text)
        );
    }

    #[test]
    fn extracts_has_conceded_text() {
        let text = "@PQlocalxxxx has conceded from the game.";
        assert_eq!(
            decode_meta_text(&real_envelope(text)).as_deref(),
            Some(text)
        );
    }

    #[test]
    fn returns_none_for_a_state_update_binary_frame() {
        let bytes = [
            144, 7, 0, 0, 44, 18, 103, 197, 62, 7, 0, 0, 12, 0, 0, 0, 4, 0, 0, 0, 0, 0, 0, 0,
        ];
        assert_eq!(decode_meta_text(&bytes), None);
    }

    #[test]
    fn returns_none_for_empty_bytes() {
        assert_eq!(decode_meta_text(&[]), None);
    }

    #[test]
    fn returns_none_for_too_short_bytes() {
        assert_eq!(decode_meta_text(&[1, 2, 3]), None);
    }

    #[test]
    fn handles_player_names_with_digits_and_underscores() {
        decodes_to_itself("@PQx_y7777 rolled a 3.");
    }

    #[test]
    fn handles_player_names_with_dots() {
        for text in [
            "@PQa.bcd rolled a 4.",
            "@P@PQa.bcd joined the game.",
            "@PQa.bcd chooses to play first.",
            "@PQa.bcd begins the game with seven cards in hand.",
            "@PQa.bcd wins the game.",
            "@PQa.bcd has conceded from the game.",
            "@PQa.bcd wins the match 2-0",
        ] {
            decodes_to_itself(text);
        }
    }

    #[test]
    fn extracts_match_score_text() {
        decodes_to_itself("@PQopponentxxx wins the match 2-0");
    }

    #[test]
    fn extracts_disconnect_text() {
        decodes_to_itself("@PQopponentxxx has lost connection to the game.");
    }

    #[test]
    fn extracts_cast_text_with_card_markup() {
        decodes_to_itself("@PQopponentxxx casts @[Guide of Souls@:251350,424:@]");
    }

    #[test]
    fn extracts_plays_text_with_card_markup() {
        decodes_to_itself("@PQopponentxxx plays @[Plains@:55294,423:@]");
    }

    #[test]
    fn extracts_activates_ability_text() {
        decodes_to_itself(
            "@PQopponentxxx activates an ability of @[Windswept Heath@:108404,428:@]",
        );
    }

    #[test]
    fn extracts_discards_text() {
        decodes_to_itself("@PPlayer discards @[Lightning Bolt@:1234,42:@]");
    }

    #[test]
    fn extracts_turn_marker() {
        decodes_to_itself("@PTurn 5:");
    }

    #[test]
    fn extracts_mulligans_text() {
        decodes_to_itself("@PPlayer mulligans to six cards");
    }

    #[test]
    fn extracts_puts_onto_battlefield_text() {
        decodes_to_itself("@PPlayer puts @[Forest@:999,1:@] onto the battlefield");
    }

    #[test]
    fn extracts_opening_hand_reveal_text() {
        decodes_to_itself("@PPlayer reveals @[Card@:111,2:@] from their opening hand");
    }

    #[test]
    fn extracts_cast_text_for_usernames_containing_spaces() {
        decodes_to_itself(
            "@PQab Cd casts @[Behold the Multiverse@:174834,100:@] by paying its foretell cost.",
        );
    }

    #[test]
    fn extracts_roll_text_for_usernames_containing_spaces() {
        decodes_to_itself("@PQab Cd rolled a 3.");
    }

    fn event(at_ms: i64, bytes: &[u8]) -> MetaEvent {
        let list: Vec<String> = bytes.iter().map(|b| b.to_string()).collect();
        MetaEvent {
            at_ms,
            text: format!(
                "10:00:00 [INF] (Game Management|Processing) Message: {{\"MatchToken\":\"11111111-2222-4333-8444-555555555555\",\"MatchID\":1,\"GameID\":1,\"MetaMessage\":[{}]}}",
                list.join(",")
            ),
        }
    }

    #[test]
    fn meta_message_entries_keep_decodable_phrases_in_order() {
        let events = vec![
            event(1_000, &chat("@PQalpha rolled a 2.")),
            event(2_000, &[144, 7, 0, 0, 44, 18, 103, 197]),
            MetaEvent {
                at_ms: 2_500,
                text: "10:00:00 [INF] (Game Management|Processing) no message".to_string(),
            },
            event(3_000, &chat("@PQalpha wins the game.")),
        ];
        assert_eq!(
            meta_message_entries(&events),
            vec![
                GameLogEntry {
                    timestamp_ms: 1_000,
                    message: "@PQalpha rolled a 2.".to_string(),
                },
                GameLogEntry {
                    timestamp_ms: 3_000,
                    message: "@PQalpha wins the game.".to_string(),
                },
            ]
        );
    }
}
