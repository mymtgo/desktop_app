//! The overlay's setting values that the pure rules read (overlay spec
//! section 4): the league card's artwork mode and size, and why the last
//! hosted overlay push failed. The app stores them as JSON strings, as 0.x
//! wrote them to settings.json.

use serde::{Deserialize, Serialize};

/// The league card's artwork (0.x `overlay_artwork`). The setting is
/// optional: unset means the read-time default (spec section 9.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "lowercase")]
pub enum OverlayArtwork {
    /// The deck cover's art crop.
    Deck,
    /// No art (0.x `none`).
    #[serde(rename = "none")]
    NoArt,
    /// The chosen background image.
    Custom,
    /// The deck cover as a full-height square on the card's left, the text
    /// beside it on the plain card (owner, 2026-10-12).
    Thumbnail,
}

/// The league card's size (0.x `overlay_size`: anything but `compact` is
/// full).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "lowercase")]
pub enum OverlaySize {
    #[default]
    Full,
    Compact,
}

/// The league card's look (setting `overlay_league_theme`, league overlay
/// themes spec 4.1). Unset or unknown reads as `Classic`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "lowercase")]
pub enum LeagueTheme {
    /// The card as it was before themes.
    #[default]
    Classic,
    /// A fighting game versus screen with live life bars.
    Versus,
}

/// How the league card shows the live match's games.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "lowercase")]
pub enum LeagueGames {
    /// Coloured pips (0.x).
    #[default]
    Dots,
    /// `W / L / -`.
    Letters,
}

/// What the league card shows (setting `overlay_league_display`, league
/// card options spec section 3). Every field is required on the wire (so the
/// generated TypeScript type has no optional fields); a stored row is read
/// with `from_stored`, which fills missing fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct LeagueDisplay {
    pub games: LeagueGames,
    /// The run name in the top-left chip.
    pub event: bool,
    /// The format next to the run name.
    pub format: bool,
    /// The title row: the label, archetype or deck name.
    pub title: bool,
    /// "Match 3 of 5" and "Next: match 4".
    pub progress: bool,
    /// The state chips and the state text.
    pub status: bool,
    /// The run's match record.
    pub record: bool,
    /// The account's rank between rounds (tournaments only, event overlay
    /// spec 9). Off by default: the rank counts dropped players.
    pub standing: bool,
}

impl Default for LeagueDisplay {
    /// Every part on, games as dots: the card as it was before the choices.
    /// Written by hand: a derived default would hide everything. Standing
    /// is the one part off by default (event overlay spec 9).
    fn default() -> Self {
        Self {
            games: LeagueGames::Dots,
            event: true,
            format: true,
            title: true,
            progress: true,
            status: true,
            record: true,
            standing: false,
        }
    }
}

impl LeagueDisplay {
    /// A stored row: each missing or unreadable field is its default, the
    /// others kept, so an older, partial or newer object never hides a part
    /// by accident or loses the other choices; anything that is not an
    /// object gives the whole default.
    pub fn from_stored(text: &str) -> Self {
        let Ok(serde_json::Value::Object(stored)) = serde_json::from_str::<serde_json::Value>(text)
        else {
            return Self::default();
        };
        let mut merged = match serde_json::to_value(Self::default()) {
            Ok(serde_json::Value::Object(defaults)) => defaults,
            _ => return Self::default(),
        };
        // One field at a time: a field is taken only if the whole still reads.
        for (key, value) in stored {
            let mut candidate = merged.clone();
            candidate.insert(key, value);
            let readable =
                serde_json::from_value::<Self>(serde_json::Value::Object(candidate.clone()))
                    .is_ok();
            if readable {
                merged = candidate;
            }
        }
        serde_json::from_value(serde_json::Value::Object(merged)).unwrap_or_default()
    }
}

/// Why the last hosted overlay push failed (0.x `overlay_publish_error`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum PublishError {
    /// No login event names an account with a confirmed login id.
    NoLoginId,
    /// 409: the account does not govern that MTGO player.
    NotClaimed,
    /// 422: the API refused the state.
    Rejected,
    /// Anything else, the network included.
    Unreachable,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_theme_is_stored_lowercase_and_defaults_to_classic() {
        assert_eq!(LeagueTheme::default(), LeagueTheme::Classic);
        assert_eq!(
            serde_json::to_string(&LeagueTheme::Versus).expect("json"),
            "\"versus\""
        );
        assert_eq!(
            serde_json::from_str::<LeagueTheme>("\"classic\"").expect("read"),
            LeagueTheme::Classic
        );
        assert!(serde_json::from_str::<LeagueTheme>("\"pixel\"").is_err());
    }

    #[test]
    fn the_values_are_stored_as_0x_wrote_them() {
        for (artwork, stored) in [
            (OverlayArtwork::Deck, "\"deck\""),
            (OverlayArtwork::NoArt, "\"none\""),
            (OverlayArtwork::Custom, "\"custom\""),
        ] {
            assert_eq!(serde_json::to_string(&artwork).expect("json"), stored);
            assert_eq!(
                serde_json::from_str::<OverlayArtwork>(stored).expect("read"),
                artwork
            );
        }
        assert_eq!(
            serde_json::to_string(&OverlaySize::Compact).expect("json"),
            "\"compact\""
        );
        assert_eq!(OverlaySize::default(), OverlaySize::Full);
        for (error, stored) in [
            (PublishError::NoLoginId, "\"no_login_id\""),
            (PublishError::NotClaimed, "\"not_claimed\""),
            (PublishError::Rejected, "\"rejected\""),
            (PublishError::Unreachable, "\"unreachable\""),
        ] {
            assert_eq!(serde_json::to_string(&error).expect("json"), stored);
            assert_eq!(
                serde_json::from_str::<PublishError>(stored).expect("read"),
                error
            );
        }
    }

    #[test]
    fn the_display_defaults_to_everything_on_with_dots() {
        let display = LeagueDisplay::default();
        assert_eq!(display.games, LeagueGames::Dots);
        assert!(display.event && display.format && display.title);
        assert!(display.progress && display.status && display.record);
    }

    #[test]
    fn a_stored_display_fills_missing_fields_with_defaults() {
        let read = LeagueDisplay::from_stored(r#"{"games":"letters","record":false}"#);
        assert_eq!(read.games, LeagueGames::Letters);
        assert!(!read.record);
        assert!(read.event && read.format && read.title && read.progress && read.status);
        assert_eq!(
            LeagueDisplay::from_stored("\"nope\""),
            LeagueDisplay::default()
        );
        // One bad field falls back alone; the other choices stay.
        let mixed = LeagueDisplay::from_stored(r#"{"record":"no","title":false,"games":"icons"}"#);
        assert!(mixed.record);
        assert!(!mixed.title);
        assert_eq!(mixed.games, LeagueGames::Dots);
        let json = serde_json::to_value(LeagueDisplay::default()).expect("json");
        assert_eq!(
            json,
            serde_json::json!({
                "games": "dots", "event": true, "format": true, "title": true,
                "progress": true, "status": true, "record": true,
                "standing": false
            })
        );
    }

    #[test]
    fn standing_is_off_unless_stored_on() {
        assert!(!LeagueDisplay::default().standing);
        // A row stored before the switch existed reads it as off.
        let older = LeagueDisplay::from_stored(r#"{"games":"letters","record":false}"#);
        assert!(!older.standing);
        assert!(!older.record);
        let on = LeagueDisplay::from_stored(r#"{"standing":true}"#);
        assert!(on.standing);
        assert!(on.record);
        assert_eq!(
            LeagueDisplay::from_stored(r#"{"standing":"yes"}"#),
            LeagueDisplay::default()
        );
    }

    #[test]
    fn the_thumbnail_artwork_is_stored_as_thumbnail() {
        assert_eq!(
            serde_json::to_string(&OverlayArtwork::Thumbnail).expect("json"),
            "\"thumbnail\""
        );
        assert_eq!(
            serde_json::from_str::<OverlayArtwork>("\"thumbnail\"").expect("read"),
            OverlayArtwork::Thumbnail
        );
    }
}
