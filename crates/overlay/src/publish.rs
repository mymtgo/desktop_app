//! The hosted OBS overlay (overlay spec section 10, 0.x `PublishOverlayState`
//! and the overlays settings page): the payload sent to `PUT /api/overlay`,
//! the reason a push failed, the OBS browser source size and the public URL.

use serde_json::Value;

use crate::league::{LeagueOverlayState, LeagueRecord, OverlayArt, TournamentBlock};
use crate::options::{LeagueTheme, OverlaySize, PublishError};

/// The room around the card in the OBS source (0.x
/// `OpenOverlayWindow::PADDING` at the `release_0.46.0` tip): the hosted
/// page pads the card 16 px on every side. The desktop window has no
/// padding. The card sizes live in `size::card_size`.
pub const CARD_PADDING: u32 = 16;

/// the API `PublishOverlayStateRequest` limits.
const MAX_NAME: usize = 120;
const MAX_FORMAT: usize = 60;
const MAX_LABEL: usize = 40;
const MAX_COUNT: u32 = 99;
const MAX_GAMES: usize = 9;
const MAX_GAMES_WON: u32 = 9;
const MAX_GAME_RECORD: u32 = 999;
const MIN_LIFE: i32 = -99;
const MAX_LIFE: i32 = 999;
/// The tournament block's and the strip's limits (event overlay spec 7.2).
const MAX_RANK: u32 = 9_999;
const MAX_MATCHES_LEFT: u32 = 999;
const MAX_GAME: u32 = 9;

/// The state the hosted page receives (0.x `PublishOverlayState::payload`):
/// the card with `public_art` (a Scryfall art crop or the uploaded
/// background, never a local file or scheme URL) in place of the window's
/// art, every field clamped to the API's validation so a valid card is never
/// refused. A `public_art` that is not `https` is dropped; `thumbnail` is
/// whether the artwork setting is the side thumbnail.
pub fn publish_state(
    state: &LeagueOverlayState,
    public_art: Option<String>,
    thumbnail: bool,
) -> Value {
    let mut card = state.clone();
    card.art = public_art
        .filter(|url| url.starts_with("https://"))
        .map(|url| OverlayArt { url, thumbnail });
    clamp(&mut card);
    serde_json::to_value(&card).expect("the league card always serialises")
}

/// Cuts every field to the API's `PublishOverlayStateRequest` limits.
fn clamp(card: &mut LeagueOverlayState) {
    if let Some(event) = card.event.as_mut() {
        event.name = cut(&event.name, MAX_NAME);
        event.format = cut(&event.format, MAX_FORMAT);
    }
    if let Some(record) = card.record.as_mut() {
        clamp_record(record);
    }
    if let Some(progress) = card.progress.as_mut() {
        progress.played = progress.played.min(MAX_COUNT);
        progress.of = progress.of.clamp(1, MAX_COUNT);
    }
    if let Some(live) = card.live_match.as_mut() {
        live.number = live.number.clamp(1, MAX_COUNT);
        if let Some(life) = live.life.as_mut() {
            life.you = life.you.clamp(MIN_LIFE, MAX_LIFE);
            life.opponent = life.opponent.clamp(MIN_LIFE, MAX_LIFE);
            life.start_you = life.start_you.clamp(1, MAX_LIFE);
            life.start_opponent = life.start_opponent.clamp(1, MAX_LIFE);
        }
        // Keep the newest games: the live game's pip is the last one.
        if live.games.len() > MAX_GAMES {
            let extra = live.games.len() - MAX_GAMES;
            live.games.drain(..extra);
        }
        live.games_won = live.games_won.min(MAX_GAMES_WON);
        live.games_lost = live.games_lost.min(MAX_GAMES_WON);
        live.opponent_archetype = live
            .opponent_archetype
            .as_deref()
            .map(|name| cut(name, MAX_NAME));
    }
    if let Some(record) = card.game_record.as_mut() {
        record.won = record.won.min(MAX_GAME_RECORD);
        record.lost = record.lost.min(MAX_GAME_RECORD);
    }
    if let Some(deck) = card.deck.as_mut() {
        deck.name = cut(&deck.name, MAX_NAME);
        deck.archetype = deck.archetype.as_deref().map(|name| cut(name, MAX_NAME));
        deck.label = deck.label.as_deref().map(|label| cut(label, MAX_LABEL));
    }
    if let Some(block) = card.tournament.as_mut() {
        clamp_tournament(block);
    }
    if let Some(other) = card.other.as_mut() {
        other.name = cut(&other.name, MAX_NAME);
        if let Some(record) = other.record.as_mut() {
            clamp_record(record);
        }
        if let Some(live) = other.live_match.as_mut() {
            live.game = live.game.clamp(1, MAX_GAME);
            live.games_won = live.games_won.min(MAX_GAMES_WON);
            live.games_lost = live.games_lost.min(MAX_GAMES_WON);
        }
        if let Some(block) = other.tournament.as_mut() {
            clamp_tournament(block);
        }
    }
}

fn clamp_record(record: &mut LeagueRecord) {
    record.wins = record.wins.min(MAX_COUNT);
    record.losses = record.losses.min(MAX_COUNT);
    record.draws = record.draws.map(|draws| draws.min(MAX_COUNT));
}

/// A tournament block's numbers inside the API's ranges. A number below a
/// range's floor (a rank or place of 0) is unknown, so it is sent as null,
/// never as a made-up 1.
fn clamp_tournament(block: &mut TournamentBlock) {
    block.round = block.round.clamp(1, MAX_COUNT);
    block.swiss_rounds = at_least_one(block.swiss_rounds).map(|n| n.min(MAX_COUNT));
    block.cut_size = at_least_one(block.cut_size).map(|n| n.min(MAX_COUNT));
    block.rank = at_least_one(block.rank).map(|n| n.min(MAX_RANK));
    block.players = block.players.map(|n| n.min(MAX_RANK));
    block.matches_left = block.matches_left.map(|n| n.min(MAX_MATCHES_LEFT));
    block.next_round_at_ms = block.next_round_at_ms.map(|at| at.max(0));
    block.starts_at_ms = block.starts_at_ms.map(|at| at.max(0));
    block.place = at_least_one(block.place).map(|n| n.min(MAX_RANK));
}

fn at_least_one(value: Option<u32>) -> Option<u32> {
    value.filter(|n| *n >= 1)
}

/// The first `max` characters (Laravel's `max` counts characters).
fn cut(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

/// Why a push failed, by HTTP status (`None`: no answer at all).
pub fn publish_error(status: Option<u16>) -> PublishError {
    match status {
        Some(409) => PublishError::NotClaimed,
        Some(422) => PublishError::Rejected,
        _ => PublishError::Unreachable,
    }
}

/// The OBS browser source size with room for the other event strip while
/// its switch is on (event overlay spec 8.2), else today's size. The hosted
/// page draws the strip only in a source this tall (spec 8.4).
pub fn obs_size_with_strip(theme: LeagueTheme, size: OverlaySize, strip: bool) -> (u32, u32) {
    let (width, height) = crate::size::card_and_strip_size(theme, size, strip);
    (width + 2 * CARD_PADDING, height + 2 * CARD_PADDING)
}

/// The OBS browser source URL: `{base}/overlays/{username}` with the name
/// encoded as PHP's `rawurlencode`, plus `?size=compact` for the compact
/// card (0.x `OverlaysController::publicUrl`, `StreamOverlayCard.vue`).
pub fn public_url(base: &str, username: &str, size: OverlaySize) -> String {
    let mut url = format!(
        "{}/overlays/{}",
        base.trim_end_matches('/'),
        raw_url_encode(username)
    );
    if size == OverlaySize::Compact {
        url.push_str("?size=compact");
    }
    url
}

/// PHP's `rawurlencode`: every byte but `A-Z a-z 0-9 - _ . ~` as `%XX`.
fn raw_url_encode(text: &str) -> String {
    let mut out = String::new();
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::art::{art_mode, window_art};
    use crate::league::{
        EventKind, GamePip, GameRecord, LeagueDeck, LeagueEvent, LeagueMatch, LeagueProgress,
        LeagueRecord, LeagueStatus, LiveLife,
    };
    use crate::options::{LeagueDisplay, LeagueGames, LeagueTheme, OverlayArtwork};

    const KARN: &str = "https://cards.scryfall.io/art_crop/karn.jpg";
    const LOCAL: &str = "card://localhost/1001_art.jpg";
    const UPLOADED: &str = "https://bucket.example/overlay-backgrounds/bg.png";

    fn state() -> LeagueOverlayState {
        LeagueOverlayState {
            status: LeagueStatus::Idle,
            event: None,
            record: None,
            progress: None,
            live_match: None,
            game_record: None,
            deck: Some(LeagueDeck {
                id: 7,
                name: "Mono Green Tron".to_string(),
                archetype: None,
                color_identity: vec!["G".to_string()],
                label: None,
            }),
            art: Some(OverlayArt {
                url: LOCAL.to_string(),
                thumbnail: false,
            }),
            size: OverlaySize::Full,
            display: LeagueDisplay::default(),
            theme: LeagueTheme::default(),
            tournament: None,
            other: None,
            public_deck_art: Some(KARN.to_string()),
        }
    }

    /// The public art as the publisher picks it: the mode's image from the
    /// deck's Scryfall art and the uploaded background.
    fn public_art(
        setting: Option<OverlayArtwork>,
        has_file: bool,
        uploaded: Option<&str>,
    ) -> Option<String> {
        let mode = art_mode(setting, has_file);
        window_art(mode, state().public_deck_art, uploaded.map(str::to_string))
    }

    // PublishOverlayStateTest

    #[test]
    fn sends_the_remote_deck_art_never_the_local_copy() {
        let payload = publish_state(&state(), public_art(None, false, None), false);

        assert_eq!(
            payload["art"],
            serde_json::json!({ "url": KARN, "thumbnail": false })
        );
        assert_eq!(payload["deck"]["name"], "Mono Green Tron");
    }

    #[test]
    fn sends_no_art_in_none_mode() {
        let payload = publish_state(
            &state(),
            public_art(Some(OverlayArtwork::NoArt), true, None),
            false,
        );

        assert_eq!(payload["art"], Value::Null);
    }

    #[test]
    fn falls_back_to_deck_art_when_custom_is_chosen_but_no_file_exists() {
        let payload = publish_state(
            &state(),
            public_art(Some(OverlayArtwork::Custom), false, None),
            false,
        );

        assert!(!payload.to_string().contains("card://"));
        assert_eq!(
            payload["art"],
            serde_json::json!({ "url": KARN, "thumbnail": false })
        );
    }

    #[test]
    fn sends_the_uploaded_background_in_custom_mode() {
        let payload = publish_state(
            &state(),
            public_art(Some(OverlayArtwork::Custom), true, Some(UPLOADED)),
            false,
        );

        assert_eq!(
            payload["art"],
            serde_json::json!({ "url": UPLOADED, "thumbnail": false })
        );
    }

    #[test]
    fn records_why_a_push_failed_so_settings_can_say() {
        assert_eq!(publish_error(Some(409)), PublishError::NotClaimed);
        assert_eq!(publish_error(Some(422)), PublishError::Rejected);
        assert_eq!(publish_error(Some(503)), PublishError::Unreachable);
        assert_eq!(publish_error(Some(500)), PublishError::Unreachable);
        assert_eq!(publish_error(None), PublishError::Unreachable);
    }

    // OverlayPublishControllerTest

    #[test]
    fn shares_the_obs_browser_source_size_for_each_card_size() {
        // With the strip switch off, the branch tip's sizes (owner ruling 1):
        // 0.x's test still names the pre-revert 332 x 132.
        let off = |theme, size| obs_size_with_strip(theme, size, false);
        assert_eq!(off(LeagueTheme::Classic, OverlaySize::Full), (332, 112));
        assert_eq!(off(LeagueTheme::Classic, OverlaySize::Compact), (272, 90));
        assert_eq!(off(LeagueTheme::Versus, OverlaySize::Full), (612, 136));
        assert_eq!(off(LeagueTheme::Versus, OverlaySize::Compact), (412, 96));
    }

    #[test]
    fn the_obs_source_with_the_strip_switch_on_fits_card_and_strip() {
        // Event overlay spec 8.2: card + strip + 16 px on every side.
        assert_eq!(
            obs_size_with_strip(LeagueTheme::Classic, OverlaySize::Full, true),
            (332, 128)
        );
        assert_eq!(
            obs_size_with_strip(LeagueTheme::Classic, OverlaySize::Compact, true),
            (272, 106)
        );
        assert_eq!(
            obs_size_with_strip(LeagueTheme::Versus, OverlaySize::Full, true),
            (612, 156)
        );
        assert_eq!(
            obs_size_with_strip(LeagueTheme::Versus, OverlaySize::Compact, true),
            (412, 112)
        );
    }

    // v1

    #[test]
    fn the_published_art_is_never_local() {
        for local in [
            LOCAL,
            "http://card.localhost/1001_art.jpg",
            "overlay://localhost/background",
            "http://127.0.0.1:8100/storage/karn.jpg",
            "C:\\Users\\Q\\karn.jpg",
        ] {
            let payload = publish_state(&state(), Some(local.to_string()), false);
            assert_eq!(payload["art"], Value::Null, "{local}");
        }
        // The window's local art never leaks when no public art is given.
        assert_eq!(publish_state(&state(), None, false)["art"], Value::Null);
        assert!(
            !publish_state(&state(), None, false)
                .to_string()
                .contains("card://")
        );
    }

    #[test]
    fn an_unknown_opponent_archetype_is_left_out_of_the_payload() {
        let mut live = state();
        live.status = LeagueStatus::InGame;
        live.live_match = Some(LeagueMatch {
            number: 1,
            games: Vec::new(),
            games_won: 0,
            games_lost: 0,
            life: None,
            opponent_archetype: None,
        });
        let payload = publish_state(&live, None, false);
        assert!(payload["match"].get("opponentArchetype").is_none());

        if let Some(found) = live.live_match.as_mut() {
            found.opponent_archetype = Some("Dimir Tempo".to_string());
        }
        let payload = publish_state(&live, None, false);
        assert_eq!(payload["match"]["opponentArchetype"], "Dimir Tempo");
    }

    #[test]
    fn the_payload_respects_every_api_limit() {
        let mut big = state();
        big.status = LeagueStatus::InGame;
        big.event = Some(LeagueEvent {
            kind: EventKind::League,
            name: "Q".repeat(130),
            format: "F".repeat(70),
        });
        big.record = Some(LeagueRecord {
            wins: 150,
            losses: 100,
            draws: None,
        });
        big.progress = Some(LeagueProgress { played: 120, of: 0 });
        big.live_match = Some(LeagueMatch {
            number: 140,
            games: (0..10)
                .map(|i| GamePip {
                    won: if i == 9 { None } else { Some(i % 2 == 0) },
                })
                .collect(),
            games_won: 12,
            games_lost: 10,
            life: Some(LiveLife {
                you: -500,
                opponent: 5000,
                start_you: 0,
                start_opponent: 5000,
            }),
            opponent_archetype: Some("O".repeat(130)),
        });
        big.game_record = Some(GameRecord {
            won: 1200,
            lost: 1000,
        });
        if let Some(deck) = big.deck.as_mut() {
            deck.name = "D".repeat(130);
            deck.archetype = Some("A".repeat(130));
            deck.label = Some("é".repeat(41));
        }

        let payload = publish_state(&big, Some(KARN.to_string()), false);

        let chars = |value: &Value| value.as_str().expect("text").chars().count();
        assert_eq!(chars(&payload["event"]["name"]), 120);
        assert_eq!(chars(&payload["event"]["format"]), 60);
        assert_eq!(payload["record"]["wins"], 99);
        assert_eq!(payload["record"]["losses"], 99);
        assert_eq!(payload["progress"]["played"], 99);
        assert_eq!(payload["progress"]["of"], 1);
        assert_eq!(payload["match"]["number"], 99);
        assert_eq!(payload["match"]["life"]["you"], -99);
        assert_eq!(payload["match"]["life"]["opponent"], 999);
        assert_eq!(payload["match"]["life"]["startYou"], 1);
        assert_eq!(payload["match"]["life"]["startOpponent"], 999);
        let games = payload["match"]["games"].as_array().expect("games");
        assert_eq!(games.len(), 9);
        // The newest games are kept, so the live game's pip stays.
        assert_eq!(games[8]["won"], Value::Null);
        assert_eq!(payload["match"]["gamesWon"], 9);
        assert_eq!(payload["match"]["gamesLost"], 9);
        assert_eq!(payload["gameRecord"]["won"], 999);
        assert_eq!(payload["gameRecord"]["lost"], 999);
        assert_eq!(chars(&payload["match"]["opponentArchetype"]), 120);
        assert_eq!(chars(&payload["deck"]["name"]), 120);
        assert_eq!(chars(&payload["deck"]["archetype"]), 120);
        assert_eq!(chars(&payload["deck"]["label"]), 40);
        assert_eq!(payload["status"], "in_game");
        assert_eq!(payload["size"], "full");
    }

    #[test]
    fn a_small_card_is_sent_unchanged_but_for_the_art() {
        let mut expected = serde_json::to_value(state()).expect("json");
        expected["art"] = serde_json::json!({ "url": KARN, "thumbnail": false });

        assert_eq!(
            publish_state(&state(), Some(KARN.to_string()), false),
            expected
        );
    }

    #[test]
    fn the_public_url_encodes_the_username() {
        assert_eq!(
            public_url("https://mymtgo.com", "QStreamPlayer", OverlaySize::Full),
            "https://mymtgo.com/overlays/QStreamPlayer"
        );
        assert_eq!(
            public_url("https://mymtgo.com/", "Q Player/é~_.-", OverlaySize::Full),
            "https://mymtgo.com/overlays/Q%20Player%2F%C3%A9~_.-"
        );
        assert_eq!(
            public_url("https://mymtgo.test", "Q+Player", OverlaySize::Compact),
            "https://mymtgo.test/overlays/Q%2BPlayer?size=compact"
        );
    }

    #[test]
    fn the_payload_keeps_the_display_and_the_thumbnail() {
        let mut card = state();
        card.display.games = LeagueGames::Letters;
        card.art = Some(OverlayArt {
            url: LOCAL.to_string(),
            thumbnail: true,
        });
        let json = publish_state(&card, Some(KARN.to_string()), true);
        assert_eq!(json["display"]["games"], "letters");
        assert_eq!(json["art"]["url"], KARN);
        assert_eq!(json["art"]["thumbnail"], true);
    }

    #[test]
    fn the_thumbnail_flag_follows_the_artwork_not_the_window_art() {
        // No window art (no local or remote crop for the window), but a public
        // crop for OBS: the thumbnail layout still applies.
        let mut card = state();
        card.art = None;
        let json = publish_state(&card, Some(KARN.to_string()), true);
        assert_eq!(json["art"]["thumbnail"], true);
        let background = publish_state(&card, Some(KARN.to_string()), false);
        assert_eq!(background["art"]["thumbnail"], false);
    }

    fn block() -> TournamentBlock {
        TournamentBlock {
            series: crate::league::TournamentSeries::Challenge,
            round: 4,
            stage: crate::league::TournamentStage::Swiss,
            swiss_rounds: Some(6),
            cut_size: Some(8),
            rank: Some(12),
            players: Some(64),
            matches_left: Some(12),
            next_round_at_ms: Some(1_800_000_000_000),
            starts_at_ms: None,
            place: None,
        }
    }

    #[test]
    fn a_tournament_block_is_clamped_to_the_api_ranges() {
        let mut big = state();
        big.record = Some(LeagueRecord {
            wins: 150,
            losses: 0,
            draws: Some(120),
        });
        big.tournament = Some(TournamentBlock {
            round: 0,
            swiss_rounds: Some(0),
            cut_size: Some(300),
            rank: Some(0),
            players: Some(20_000),
            matches_left: Some(5_000),
            next_round_at_ms: Some(-5),
            starts_at_ms: Some(-1),
            place: Some(12_000),
            ..block()
        });
        let json = publish_state(&big, None, false);
        assert_eq!(json["record"]["draws"], 99);
        let sent = &json["tournament"];
        assert_eq!(sent["round"], 1);
        assert_eq!(sent["swissRounds"], Value::Null, "0 is unknown");
        assert_eq!(sent["cutSize"], 99);
        assert_eq!(sent["rank"], Value::Null, "0 is unknown");
        assert_eq!(sent["players"], 9_999);
        assert_eq!(sent["matchesLeft"], 999);
        assert_eq!(sent["nextRoundAtMs"], 0);
        assert_eq!(sent["startsAtMs"], 0);
        assert_eq!(sent["place"], 9_999);
        // In range: unchanged.
        let mut fine = state();
        fine.tournament = Some(block());
        assert_eq!(
            publish_state(&fine, None, false)["tournament"],
            serde_json::to_value(block()).expect("json")
        );
    }

    #[test]
    fn the_strip_is_clamped_to_the_api_ranges() {
        let mut big = state();
        big.other = Some(crate::league::OtherEvent {
            kind: EventKind::Tournament,
            name: "Q".repeat(130),
            status: LeagueStatus::InGame,
            record: Some(LeagueRecord {
                wins: 120,
                losses: 3,
                draws: Some(200),
            }),
            live_match: Some(crate::league::OtherMatch {
                game: 12,
                games_won: 11,
                games_lost: 10,
            }),
            tournament: Some(TournamentBlock {
                round: 150,
                rank: Some(10_000),
                ..block()
            }),
        });
        let json = publish_state(&big, None, false);
        let other = &json["other"];
        assert_eq!(
            other["name"].as_str().map(|name| name.chars().count()),
            Some(120)
        );
        assert_eq!(other["record"]["wins"], 99);
        assert_eq!(other["record"]["draws"], 99);
        assert_eq!(other["match"]["game"], 9);
        assert_eq!(other["match"]["gamesWon"], 9);
        assert_eq!(other["match"]["gamesLost"], 9);
        assert_eq!(other["tournament"]["round"], 99);
        assert_eq!(other["tournament"]["rank"], 9_999);
        let mut zero = state();
        zero.other = big.other.clone();
        if let Some(live) = zero
            .other
            .as_mut()
            .and_then(|other| other.live_match.as_mut())
        {
            live.game = 0;
        }
        assert_eq!(
            publish_state(&zero, None, false)["other"]["match"]["game"],
            1
        );
    }
}
