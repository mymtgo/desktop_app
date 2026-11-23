//! The game overlay's view against a temporary database (0.x
//! `GameOverlayControllerTest`, `ResolveOverlayOpponentTest`), with the
//! lookups recorded by a fake port. Fake names start with Q.

use fixture_tools::TestDir;
use mymtgo_api::{ArchetypeDeck, DecklistCard, ScoutedArchetype, Scouting};
use serde_json::Value;
use store::params;

use super::*;
use crate::overlay::game_net::testing::FakePort;
use crate::overlay::game_net::{GameAnswer, GameNet, GameRequest};

/// 2026-07-01T10:00:00Z.
pub(crate) const NOW: i64 = 1_782_900_000_000;
pub(crate) const LIVE: &str = "q-live";
/// MTGO's game id of the live match's game 1.
pub(crate) const GAME_ONE: i64 = 9001;

pub(crate) fn db(label: &str) -> (TestDir, Connection) {
    let dir = TestDir::new(label);
    let conn = store::open_writer(&dir.join("mymtgo.sqlite")).expect("open");
    seed(&conn);
    conn.execute(
        "INSERT INTO log_files (id, path, identity_hash, head_hash, head_len, day_anchor,
             timezone, clock_date, first_seen_at, last_seen_at)
         VALUES (1, 'Q/mtgo.log', 'q', 'q', 0, '2026-07-01', 'UTC', '2026-07-01', 0, 0)",
        [],
    )
    .expect("log file");
    (dir, conn)
}

/// The account, the opponent, the cards and the deck of `liveOverlayMatch`
/// (0.x): Mountain x20 and Lightning Bolt x4.
pub(crate) fn seed(conn: &Connection) {
    conn.execute_batch(
        "INSERT INTO accounts (id, username, first_seen_at) VALUES (1, 'Qlocal', 0);
         INSERT INTO opponents (id, username, first_seen_at) VALUES (1, 'Qrival', 0), (2, 'Qother', 0);",
    )
    .expect("seed");
    card(conn, 101, "o-mountain", "Mountain", "Basic Land");
    card(conn, 102, "o-bolt", "Lightning Bolt", "Instant");
    deck(conn, 1, "Qburn", &[(101, 20, false), (102, 4, false)]);
}

pub(crate) fn card(conn: &Connection, catalog_id: u32, oracle: &str, name: &str, kind: &str) {
    conn.execute(
        "INSERT INTO cards (catalog_id, oracle_id, name, type, image, art_crop)
         VALUES (?1, ?2, ?3, ?4, 'https://img/' || ?1 || '.jpg', 'https://art/' || ?1 || '.jpg')",
        params![catalog_id, oracle, name, kind],
    )
    .expect("card");
}

/// A deck and its version `id * 10`.
pub(crate) fn deck(conn: &Connection, id: i64, name: &str, cards: &[(u32, u32, bool)]) {
    conn.execute(
        "INSERT INTO decks (id, mtgo_id, name, format, created_at, updated_at, source)
         VALUES (?1, 'q-deck-' || ?1, ?2, 'CMODERN', 1, 1, 'file')",
        params![id, name],
    )
    .expect("deck");
    conn.execute(
        "INSERT INTO deck_versions (id, deck_id, signature, modified_at) VALUES (?1 * 10, ?1, ?2, 1)",
        params![id, signature(cards)],
    )
    .expect("version");
}

pub(crate) fn signature(cards: &[(u32, u32, bool)]) -> String {
    let rows: Vec<DeckCard> = cards
        .iter()
        .map(|&(catalog_id, quantity, sideboard)| DeckCard {
            catalog_id,
            quantity,
            sideboard,
        })
        .collect();
    decks::signature(&rows)
}

pub(crate) fn set_version(conn: &Connection, deck_id: i64, cards: &[(u32, u32, bool)]) {
    conn.execute(
        "UPDATE deck_versions SET signature = ?2 WHERE id = ?1 * 10",
        params![deck_id, signature(cards)],
    )
    .expect("version");
}

/// A log match of the local account against `opponent` (1 is Qrival).
#[allow(clippy::too_many_arguments)]
pub(crate) fn a_match(
    conn: &Connection,
    id: i64,
    token: &str,
    opponent: Option<i64>,
    format: &str,
    state: &str,
    outcome: &str,
    started_at: i64,
    deck: Option<i64>,
) {
    conn.execute(
        "INSERT INTO matches (id, token, account_id, opponent_id, source, format, state,
             outcome, started_at, last_activity_at, revision, derived_version, deck_version_id)
         VALUES (?1, ?2, 1, ?3, 'log', ?4, ?5, ?6, ?7, ?7, 1, 1, ?8 * 10)",
        params![
            id, token, opponent, format, state, outcome, started_at, deck
        ],
    )
    .expect("match");
}

/// The live Modern match against Qrival with game 1 under way and deck 1.
pub(crate) fn live_match(conn: &Connection) {
    a_match(
        conn,
        1,
        LIVE,
        Some(1),
        "CModern",
        "in_progress",
        "unknown",
        NOW - 60_000,
        Some(1),
    );
    // Under way: the play or draw is chosen and both hands kept.
    conn.execute(
        "INSERT INTO games (match_id, game_number, mtgo_game_id, result, end_reason, on_play,
             local_mulligans, opp_mulligans)
         VALUES (1, 1, ?1, 'unknown', 'unknown', 1, 0, 0)",
        [GAME_ONE],
    )
    .expect("game");
}

/// A state frame of `game_id`: the local seat (1) with `library` cards
/// left and `local` cards in their zones, the opponent's (2) `opponent`
/// cards on the battlefield.
pub(crate) fn frame(
    conn: &Connection,
    offset: i64,
    game_id: i64,
    library: i64,
    local: &[(u32, &str)],
    opponent: &[u32],
) {
    let mut cards: Vec<String> = Vec::new();
    for (index, (catalog_id, zone)) in local.iter().enumerate() {
        cards.push(format!(
            r#"{{"Id":{},"CatalogID":{catalog_id},"Zone":"{zone}","ActualZone":"{zone}","Owner":1,"Controller":1}}"#,
            100 + index
        ));
    }
    for (index, catalog_id) in opponent.iter().enumerate() {
        cards.push(format!(
            r#"{{"Id":{},"CatalogID":{catalog_id},"Zone":"Battlefield","ActualZone":"Battlefield","Owner":2,"Controller":2}}"#,
            500 + index
        ));
    }
    let text = format!(
        r#"12:00:00 [INF] (Twitch Info|Game Play Status Update for Game ID: {game_id}, Match ID: 99, Event ID: 99) {{"Players":[{{"Id":1,"Name":"Qlocal","LibraryCount":{library},"HandCount":7,"Life":20}},{{"Id":2,"Name":"Qrival","LibraryCount":50,"HandCount":7,"Life":20}}],"Cards":[{}]}}"#,
        cards.join(",")
    );
    conn.execute(
        "INSERT INTO log_events (log_file_id, byte_offset, kind, game_id, logged_at, raw)
         VALUES (1, ?1, 'game_state_update', ?2, ?3, CAST(?4 AS BLOB))",
        params![offset, game_id, NOW, text],
    )
    .expect("frame");
}

pub(crate) fn archetype(conn: &Connection, id: i64, uuid: &str, name: &str, source: &str) {
    conn.execute(
        "INSERT INTO archetypes (id, uuid, name, format, color_identity, source, created_at, updated_at)
         VALUES (?1, ?2, ?3, 'modern', 'W,U,B', ?4, 1, 1)",
        params![id, uuid, name, source],
    )
    .expect("archetype");
}

/// The opponent seat of match `match_id`.
pub(crate) fn opponent_seat(conn: &Connection, match_id: i64, archetype_id: i64, manual: bool) {
    conn.execute(
        "INSERT INTO match_archetypes (match_id, is_opponent, archetype_id, confidence, manual)
         VALUES (?1, 1, ?2, ?3, ?4)
         ON CONFLICT (match_id, is_opponent) DO UPDATE SET archetype_id = excluded.archetype_id",
        params![
            match_id,
            archetype_id,
            if manual { 1.0 } else { 0.9 },
            manual
        ],
    )
    .expect("seat");
}

pub(crate) fn note(conn: &Connection, deck_id: i64, archetype_id: i64, body: &str, at: i64) {
    conn.execute(
        "INSERT INTO deck_archetype_notes (deck_id, archetype_id, body, created_at)
         VALUES (?1, ?2, ?3, ?4)",
        params![deck_id, archetype_id, body, at],
    )
    .expect("note");
}

fn guide(conn: &Connection, id: i64, deck_id: i64, archetype_id: i64, cards: &[(&str, &str, u32)]) {
    conn.execute(
        "INSERT INTO sideboard_guides (id, deck_id, archetype_id, created_at, updated_at)
         VALUES (?1, ?2, ?3, 1, 1)",
        params![id, deck_id, archetype_id],
    )
    .expect("guide");
    for (oracle, direction, quantity) in cards {
        conn.execute(
            "INSERT INTO sideboard_guide_cards (guide_id, oracle_id, direction, quantity)
             VALUES (?1, ?2, ?3, ?4)",
            params![id, oracle, direction, quantity],
        )
        .expect("guide card");
    }
}

/// Every section on (0.x's defaults).
pub(crate) fn settings() -> Settings {
    Settings {
        game_overlay: true,
        ..Settings::default()
    }
}

/// The view of `token`, its lookups through `port`.
fn view_with(
    conn: &Connection,
    token: Option<&str>,
    settings: &Settings,
    net: &mut GameNet,
) -> GameOverlayView {
    view_at(conn, token, settings, net, NOW)
}

fn view_at(
    conn: &Connection,
    token: Option<&str>,
    settings: &Settings,
    net: &mut GameNet,
    now_ms: i64,
) -> GameOverlayView {
    let inputs = GameInputs {
        settings,
        collapsed: false,
        now_ms,
        previous: None,
    };
    game_view(conn, token, &inputs, net).expect("view").0
}

fn view(conn: &Connection, settings: &Settings) -> (GameOverlayView, FakePort) {
    let port = FakePort::online();
    let mut net = port.net();
    (view_with(conn, Some(LIVE), settings, &mut net), port)
}

fn json(view: &GameOverlayView) -> Value {
    serde_json::to_value(view).expect("json")
}

fn opponent(view: &GameOverlayView) -> &OverlayOpponent {
    view.opponent.as_ref().expect("an opponent")
}

/// 0.x "renders draw odds for the active match deck".
#[test]
fn renders_draw_odds_for_the_active_match_deck() {
    let (_dir, conn) = db("game-overlay-draw-odds");
    live_match(&conn);
    let (view, _) = view(&conn, &settings());
    let odds = view.draw_odds.expect("draw odds");
    assert_eq!(odds.library_size, 24);
    assert_eq!(odds.cards.len(), 2);
}

/// The dealt hand is in the frames while the play or draw is still being
/// chosen: the odds count the whole deck until the choice is logged.
#[test]
fn draw_odds_do_not_give_the_hand_away_before_the_play_draw_choice() {
    let (_dir, conn) = db("game-overlay-before-choice");
    live_match(&conn);
    conn.execute(
        "UPDATE games SET on_play = NULL, local_mulligans = NULL, opp_mulligans = NULL",
        [],
    )
    .expect("choosing");
    frame(
        &conn,
        10,
        GAME_ONE,
        17,
        &[(102, "Hand"), (102, "Hand")],
        &[],
    );

    let (choosing, _) = view(&conn, &settings());
    let odds = choosing.draw_odds.expect("draw odds");
    assert_eq!(odds.library_size, 24);
    assert_eq!(odds.live_library_count, 0);

    conn.execute("UPDATE games SET on_play = 0", [])
        .expect("chosen");
    let (chosen, _) = view(&conn, &settings());
    assert_eq!(chosen.draw_odds.expect("draw odds").library_size, 22);
}

/// The strip above the opponent's name: play or draw and the kept hands.
#[test]
fn the_header_shows_the_current_games_play_draw_and_hands() {
    let (_dir, conn) = db("game-overlay-current-game");
    live_match(&conn);
    conn.execute(
        "UPDATE games SET on_play = 0, local_mulligans = 1, opp_mulligans = NULL",
        [],
    )
    .expect("game");

    let (shown, _) = view(&conn, &settings());
    let game = shown.game.expect("the current game");
    assert_eq!(game.on_play, Some(false));
    assert_eq!((game.local_hand, game.opponent_hand), (Some(6), None));

    let header_off = Settings {
        overlay_show_opponent: false,
        ..settings()
    };
    let (hidden, _) = view(&conn, &header_off);
    assert_eq!(hidden.game, None);
}

/// After sideboarding, MTGO's `deck_used` for game 2 can repeat game 1's
/// list; the first frame's Sideboard zone says what was sided out, as 0.x
/// `CreateGames::buildLocalDeck` reads it (#348).
#[test]
fn draw_odds_count_the_deck_as_sided_for_the_current_game() {
    let (_dir, conn) = db("game-overlay-sided-deck");
    live_match(&conn);
    card(&conn, 103, "o-smash", "Smash to Smithereens", "Instant");
    conn.execute(
        "INSERT INTO games (match_id, game_number, mtgo_game_id, result, end_reason)
         VALUES (1, 2, 9002, 'unknown', 'unknown')",
        [],
    )
    .expect("game two");
    // Game 1's list again: two Bolts are sided out for two Smash.
    let list = r#"[{"CatalogId":101,"Quantity":20,"Annotation":"NotSet","InSideboard":false},{"CatalogId":102,"Quantity":4,"Annotation":"NotSet","InSideboard":false},{"CatalogId":103,"Quantity":2,"Annotation":"NotSet","InSideboard":true}]"#;
    conn.execute(
        "INSERT INTO log_events (log_file_id, byte_offset, kind, game_id, logged_at, raw)
         VALUES (1, 10, 'deck_used', 9002, ?1, CAST(?2 AS BLOB))",
        params![
            NOW,
            format!(
                "12:00:00 [INF] (Twitch Info|Username: Qlocal Deck Used in Game ID: 9002) {list}"
            )
        ],
    )
    .expect("deck used");
    frame(
        &conn,
        20,
        9002,
        24,
        &[(102, "Sideboard"), (102, "Sideboard")],
        &[],
    );

    let (view, _) = view(&conn, &settings());
    let odds = view.draw_odds.expect("draw odds");
    let total = |catalog_id: u32| {
        odds.cards
            .iter()
            .find(|card| card.catalog_id == catalog_id)
            .map(|card| card.total)
    };
    assert_eq!(total(102), Some(2));
    assert_eq!(total(103), Some(2));
    assert_eq!(odds.library_size, 24);
}

/// 0.x "renders null draw odds and a null opponent when no match is live".
#[test]
fn renders_null_draw_odds_and_a_null_opponent_when_no_match_is_live() {
    let (_dir, conn) = db("game-overlay-no-match");
    let port = FakePort::online();
    let view = view_with(&conn, None, &settings(), &mut port.net());
    assert_eq!(view.draw_odds, None);
    assert_eq!(view.opponent, None);
    assert_eq!(view.sideboard, None);
    assert!(!view.has_match);
    assert!(port.requests().is_empty());
}

/// 0.x "sends the humanised format so the archetype dropdown can match on
/// it" and "sends a null format when no match is live".
#[test]
fn sends_the_humanised_format_or_none_without_a_match() {
    let (_dir, conn) = db("game-overlay-format");
    let port = FakePort::online();
    assert_eq!(
        view_with(&conn, None, &settings(), &mut port.net()).format,
        None
    );
    live_match(&conn);
    let (view, _) = view(&conn, &settings());
    assert_eq!(view.format.as_deref(), Some("Modern"));
}

/// 0.x "renders the opponent with a manual archetype and its sideboard
/// guide" and "makes no HTTP request while building the payload for a
/// manual pick".
#[test]
fn renders_the_opponent_with_a_manual_archetype_and_its_sideboard_guide() {
    let (_dir, conn) = db("game-overlay-manual");
    live_match(&conn);
    archetype(&conn, 101, "q-blink", "Esper Blink", "downloaded");
    opponent_seat(&conn, 1, 101, true);
    note(
        &conn,
        1,
        101,
        "Bring in the extra removal, they run few threats worth countering.",
        NOW - 1_000,
    );
    let (view, port) = view(&conn, &settings());
    let opponent = opponent(&view);
    assert_eq!(opponent.username, "Qrival");
    assert_eq!(opponent.archetype_name.as_deref(), Some("Esper Blink"));
    assert_eq!(opponent.archetype_colors, vec!["W", "U", "B"]);
    assert_eq!(opponent.source, Source::Manual);
    assert!(opponent.manual);
    assert_eq!(opponent.source_label, None);
    assert_eq!(view.sideboard.as_ref().expect("guide").postboard_games, 0);
    assert_eq!(view.notes.current.len(), 1);
    assert_eq!(
        view.notes.current[0].body,
        "Bring in the extra removal, they run few threats worth countering."
    );
    // No opponent lookup for a manual pick (the archetype's live decklists
    // for the could-still-have cards are spec 7.5's, on Plan T's worker).
    assert_eq!(
        port.requests(),
        vec![GameRequest::Decklists {
            uuid: "q-blink".to_string()
        }]
    );
}

/// 0.x "renders the opponent with no archetype when nothing resolves".
#[test]
fn renders_the_opponent_with_no_archetype_when_nothing_resolves() {
    let (_dir, conn) = db("game-overlay-nothing");
    live_match(&conn);
    let (view, port) = view(&conn, &settings());
    let opponent = opponent(&view);
    assert_eq!(opponent.username, "Qrival");
    assert_eq!(opponent.archetype_id, None);
    assert_eq!(opponent.source, Source::None);
    assert_eq!(view.sideboard, None);
    assert!(view.notes.current.is_empty());
    // Online, the opponent is scouted (nothing revealed: no estimate).
    assert_eq!(
        port.requests(),
        vec![GameRequest::Scout {
            username: "Qrival".to_string(),
            format_key: "modern".to_string(),
        }]
    );
}

/// 0.x ResolveOverlayOpponentTest "returns null when no opponent player
/// exists yet" (U2 carry): "Waiting for opponent…".
#[test]
fn shows_no_opponent_before_one_is_known() {
    let (_dir, conn) = db("game-overlay-no-opponent");
    live_match(&conn);
    conn.execute("UPDATE matches SET opponent_id = NULL", [])
        .expect("no opponent");
    let (view, port) = view(&conn, &settings());
    assert!(view.has_match);
    assert_eq!(view.opponent, None);
    assert!(!view.has_archetype);
    assert!(port.requests().is_empty());
}

/// 0.x "reports hasMatch and hasDeck from the match itself, independent of
/// section toggles".
#[test]
fn reports_has_match_and_has_deck_from_the_match_itself() {
    let (_dir, conn) = db("game-overlay-facts");
    live_match(&conn);
    let settings = Settings {
        overlay_show_draw_odds: false,
        ..settings()
    };
    let (view, _) = view(&conn, &settings);
    assert!(view.sections.opponent);
    assert!(!view.sections.draw_odds);
    assert!(view.sections.sideboard);
    assert_eq!(view.draw_odds, None);
    assert_eq!(view.sideboard, None);
    assert!(view.has_match);
    assert!(view.has_deck);
    assert!(!view.has_archetype);
}

/// 0.x "omits disabled sections from the payload" and "omits reveals when
/// the section is disabled".
#[test]
fn omits_disabled_sections_from_the_payload() {
    let (_dir, conn) = db("game-overlay-sections");
    live_match(&conn);
    frame(&conn, 10, GAME_ONE, 50, &[], &[102]);
    let settings = Settings {
        overlay_show_draw_odds: false,
        overlay_show_sideboard: false,
        overlay_show_reveals: false,
        ..settings()
    };
    let (view, _) = view(&conn, &settings);
    assert_eq!(view.draw_odds, None);
    assert_eq!(view.sideboard, None);
    assert_eq!(view.reveals, None);
    assert_eq!(view.potential, None);
    assert!(!view.is_sideboarding);
    assert!(!view.sections.draw_odds);
    assert!(!view.sections.sideboard);
    assert!(view.sections.opponent);
    assert!(view.opponent.is_some());
}

/// 0.x "keeps the sideboard guide and notes when the opponent header is
/// switched off".
#[test]
fn keeps_the_sideboard_guide_and_notes_when_the_opponent_header_is_switched_off() {
    let (_dir, conn) = db("game-overlay-header-off");
    live_match(&conn);
    archetype(&conn, 101, "q-blink", "Esper Blink", "downloaded");
    opponent_seat(&conn, 1, 101, true);
    note(&conn, 1, 101, "Keep the graveyard hate, cut a land.", NOW);
    let settings = Settings {
        overlay_show_opponent: false,
        ..settings()
    };
    let (view, _) = view(&conn, &settings);
    assert!(!view.sections.opponent);
    assert_eq!(view.opponent, None);
    assert!(view.has_archetype);
    let guide = view.sideboard.expect("guide");
    assert_eq!(guide.postboard_games, 0);
    assert_eq!(view.notes.current.len(), 1);
    assert_eq!(
        view.notes.current[0].body,
        "Keep the graveyard hate, cut a land."
    );
}

/// 0.x "still responds when a pre-upgrade league cache entry is missing its
/// uuid": a scouting answer whose source has no uuid names nothing.
#[test]
fn still_responds_when_a_scouting_source_has_no_uuid() {
    let (_dir, conn) = db("game-overlay-no-uuid");
    live_match(&conn);
    let response = mymtgo_api::ApiResponse {
        status: 200,
        headers: Vec::new(),
        body: br#"{"data":{"league_result":{"archetype":{"name":"Esper Blink"}}}}"#.to_vec(),
    };
    let port = FakePort::online();
    let mut net = port.net();
    net.answered(
        GameAnswer::Scouted {
            username: "Qrival".to_string(),
            format_key: "modern".to_string(),
            answer: mymtgo_api::parse_players(&response),
            failed: false,
        },
        NOW,
    );
    let view = view_with(&conn, Some(LIVE), &settings(), &mut net);
    assert_eq!(opponent(&view).username, "Qrival");
    assert_eq!(opponent(&view).archetype_id, None);
    assert!(!view.has_archetype);
}

/// 0.x "annotates the sideboard guide with community rates for a classified
/// deck" and "renders the sideboard guide without community rates when
/// stats sharing is off" (spec ruling 6: no Field rates at all).
#[test]
fn the_guide_never_shows_field_rates() {
    let (_dir, conn) = db("game-overlay-no-field");
    live_match(&conn);
    set_version(&conn, 1, &[(101, 20, false), (102, 4, true)]);
    archetype(&conn, 101, "q-blink", "Esper Blink", "downloaded");
    opponent_seat(&conn, 1, 101, true);
    let (view, port) = view(&conn, &settings());
    let guide = view.sideboard.as_ref().expect("guide");
    assert_eq!(guide.sided_in[0].oracle_id, "o-bolt");
    let text = json(&view).to_string();
    for community in ["community", "Community", "field"] {
        assert!(!text.contains(community), "{community} in {text}");
    }
    assert!(
        port.requests()
            .iter()
            .all(|request| matches!(request, GameRequest::Decklists { .. })),
        "no card stats request: {:?}",
        port.requests()
    );
}

/// 0.x "swaps the sideboard guide when a new archetype is pinned".
#[test]
fn swaps_the_sideboard_guide_when_a_new_archetype_is_pinned() {
    let (_dir, conn) = db("game-overlay-swap");
    live_match(&conn);
    card(&conn, 103, "o-rip", "Rest in Peace", "Enchantment");
    set_version(
        &conn,
        1,
        &[(101, 20, false), (102, 4, false), (103, 2, true)],
    );
    archetype(&conn, 101, "q-first", "Esper Blink", "downloaded");
    archetype(&conn, 102, "q-second", "Ruby Storm", "downloaded");
    guide(&conn, 1, 1, 101, &[("o-rip", "in", 1)]);
    guide(&conn, 2, 1, 102, &[("o-rip", "in", 2)]);
    opponent_seat(&conn, 1, 101, true);
    let (first, _) = view(&conn, &settings());
    assert_eq!(first.sideboard.expect("guide").sided_in[0].quantity, 1);
    opponent_seat(&conn, 1, 102, true);
    let (second, _) = view(&conn, &settings());
    assert_eq!(
        opponent(&second).archetype_name.as_deref(),
        Some("Ruby Storm")
    );
    assert_eq!(second.sideboard.expect("guide").sided_in[0].quantity, 2);
}

/// 0.x "renders revealed opponent cards for the active match".
#[test]
fn renders_revealed_opponent_cards_for_the_active_match() {
    let (_dir, conn) = db("game-overlay-reveals");
    live_match(&conn);
    frame(&conn, 10, GAME_ONE, 50, &[], &[102, 102]);
    let (view, _) = view(&conn, &settings());
    assert!(view.sections.reveals);
    let reveals = view.reveals.expect("reveals");
    assert_eq!(reveals[0].name, "Lightning Bolt");
    assert_eq!(reveals[0].quantity, 2);
}

/// 0.x "shows the authored plan instead of history when a guide with cards
/// exists for the matchup".
#[test]
fn shows_the_authored_plan_instead_of_history_when_a_guide_with_cards_exists() {
    let (_dir, conn) = db("game-overlay-plan");
    live_match(&conn);
    card(&conn, 103, "o-rip", "Rest in Peace", "Enchantment");
    set_version(
        &conn,
        1,
        &[(101, 20, false), (102, 4, false), (103, 2, true)],
    );
    archetype(&conn, 101, "q-blink", "Esper Blink", "downloaded");
    opponent_seat(&conn, 1, 101, true);
    guide(
        &conn,
        1,
        1,
        101,
        &[("o-rip", "in", 1), ("o-bolt", "out", 1)],
    );
    let (view, _) = view(&conn, &settings());
    let guide = view.sideboard.expect("guide");
    assert!(guide.has_plan);
    assert_eq!(guide.sided_in.len(), 1);
    assert_eq!(guide.sided_in[0].oracle_id, "o-rip");
    assert_eq!(guide.sided_in[0].quantity, 1);
    assert_eq!(guide.sided_out.len(), 1);
    assert_eq!(guide.sided_out[0].oracle_id, "o-bolt");
}

/// 0.x "falls back to history when the guide for the matchup has no cards".
#[test]
fn falls_back_to_history_when_the_guide_for_the_matchup_has_no_cards() {
    let (_dir, conn) = db("game-overlay-history");
    live_match(&conn);
    card(&conn, 103, "o-rip", "Rest in Peace", "Enchantment");
    set_version(
        &conn,
        1,
        &[(101, 20, false), (102, 4, false), (103, 2, true)],
    );
    archetype(&conn, 101, "q-blink", "Esper Blink", "downloaded");
    opponent_seat(&conn, 1, 101, true);
    guide(&conn, 1, 1, 101, &[]);
    let (view, _) = view(&conn, &settings());
    let guide = view.sideboard.expect("guide");
    assert!(!guide.has_plan);
    assert_eq!(guide.sided_in.len(), 1);
    assert_eq!(guide.sided_in[0].oracle_id, "o-rip");
    assert_eq!(guide.sided_in[0].quantity, 2);
}

/// U2 review: a planned card the version no longer holds is named from
/// another printing of it (0.x `$meta ?? $row`), not "Unknown card".
#[test]
fn a_planned_card_missing_from_the_version_is_named_from_another_printing() {
    let (_dir, conn) = db("game-overlay-stale-name");
    live_match(&conn);
    card(&conn, 103, "o-rip", "Rest in Peace", "Enchantment");
    archetype(&conn, 101, "q-blink", "Esper Blink", "downloaded");
    opponent_seat(&conn, 1, 101, true);
    guide(&conn, 1, 1, 101, &[("o-rip", "in", 1)]);
    let (view, _) = view(&conn, &settings());
    let guide = view.sideboard.expect("guide");
    assert!(guide.sided_in[0].stale);
    assert_eq!(guide.sided_in[0].name, "Rest in Peace");
}

/// 0.x "tells the page whether the overlay is collapsed".
#[test]
fn tells_the_page_whether_the_overlay_is_collapsed() {
    let (_dir, conn) = db("game-overlay-collapsed");
    let port = FakePort::online();
    let settings = settings();
    let mut inputs = GameInputs {
        settings: &settings,
        collapsed: false,
        now_ms: NOW,
        previous: None,
    };
    let mut net = port.net();
    assert!(
        !game_view(&conn, None, &inputs, &mut net)
            .expect("view")
            .0
            .collapsed
    );
    inputs.collapsed = true;
    assert!(
        game_view(&conn, None, &inputs, &mut net)
            .expect("view")
            .0
            .collapsed
    );
}

fn murktide_lists(conn: &Connection) {
    card(conn, 103, "o-murktide", "Murktide Regent", "Creature");
    archetype(conn, 101, "q-murktide", "Murktide", "downloaded");
    conn.execute_batch(
        "INSERT INTO archetype_cards (archetype_id, list, catalog_id, oracle_id, quantity, sideboard)
             VALUES (101, 1, 102, 'o-bolt', 4, 0), (101, 1, 103, 'o-murktide', 4, 0);",
    )
    .expect("list");
}

/// 0.x "suggests cards the picked archetype could still have alongside the
/// reveals" (offline: the held key list).
#[test]
fn suggests_cards_the_picked_archetype_could_still_have_alongside_the_reveals() {
    let (_dir, conn) = db("game-overlay-potential");
    live_match(&conn);
    murktide_lists(&conn);
    opponent_seat(&conn, 1, 101, true);
    frame(&conn, 10, GAME_ONE, 50, &[], &[102, 102]);
    let port = FakePort::default();
    let view = view_with(&conn, Some(LIVE), &settings(), &mut port.net());
    let potential = view.potential.expect("potential");
    let rows: Vec<(&str, u32)> = potential
        .maindeck
        .iter()
        .map(|card| (card.name.as_str(), card.quantity))
        .collect();
    assert_eq!(rows, vec![("Lightning Bolt", 2), ("Murktide Regent", 4)]);
    assert!(potential.sideboard.is_empty());
}

/// Spec 7.5: online, the archetype's live decklists come before its key
/// list (asked once, then read from Plan T's cache).
#[test]
fn the_live_decklists_come_before_the_key_list_when_online() {
    let (_dir, conn) = db("game-overlay-potential-live");
    live_match(&conn);
    murktide_lists(&conn);
    opponent_seat(&conn, 1, 101, true);
    let port = FakePort::online();
    let mut net = port.net();
    let first = view_with(&conn, Some(LIVE), &settings(), &mut net);
    assert_eq!(
        first.potential.expect("key list").maindeck.len(),
        2,
        "the key list while the decklists are asked"
    );
    assert!(port.requests().contains(&GameRequest::Decklists {
        uuid: "q-murktide".to_string()
    }));
    crate::bridge::lock(&port.0).decklists.insert(
        "q-murktide".to_string(),
        vec![ArchetypeDeck {
            uuid: "q-list".to_string(),
            seen_count: 3,
            cards: vec![DecklistCard {
                mtgo_id: 104,
                oracle_id: "o-ragavan".to_string(),
                name: Some("Ragavan, Nimble Pilferer".to_string()),
                card_type: Some("Legendary Creature".to_string()),
                color_identity: Some("R".to_string()),
                image: None,
                quantity: 4,
                sideboard: false,
            }],
        }],
    );
    let second = view_with(&conn, Some(LIVE), &settings(), &mut net);
    let names: Vec<String> = second
        .potential
        .expect("live lists")
        .maindeck
        .into_iter()
        .map(|card| card.name)
        .collect();
    assert_eq!(names, vec!["Ragavan, Nimble Pilferer"]);
    let asked = port
        .requests()
        .iter()
        .filter(|request| matches!(request, GameRequest::Decklists { .. }))
        .count();
    assert_eq!(asked, 1);
}

/// 0.x "suggests nothing until an archetype is known".
#[test]
fn suggests_nothing_until_an_archetype_is_known() {
    let (_dir, conn) = db("game-overlay-no-potential");
    live_match(&conn);
    let (view, _) = view(&conn, &settings());
    assert_eq!(view.potential, None);
}

/// 0.x "defers draw odds so a poll that excludes it never computes it":
/// v1 pushes, so a frame recomputes the frame parts only and keeps the
/// match parts (here a note added without its `GuidesChanged`).
#[test]
fn frames_recompute_only_the_frame_parts() {
    let (_dir, conn) = db("game-overlay-frames-only");
    live_match(&conn);
    archetype(&conn, 101, "q-blink", "Esper Blink", "downloaded");
    opponent_seat(&conn, 1, 101, true);
    frame(&conn, 10, GAME_ONE, 50, &[], &[]);
    let settings = settings();
    let port = FakePort::online();
    let mut net = port.net();
    let full = GameInputs {
        settings: &settings,
        collapsed: false,
        now_ms: NOW,
        previous: None,
    };
    let (first, memo) = game_view(&conn, Some(LIVE), &full, &mut net).expect("view");
    let memo = memo.expect("memo");
    assert_eq!(first.draw_odds.as_ref().expect("odds").library_size, 24);

    note(&conn, 1, 101, "Qnot pushed yet", NOW);
    frame(&conn, 20, GAME_ONE, 49, &[(102, "Hand")], &[]);
    let frames_only = GameInputs {
        previous: Some(&memo),
        ..full
    };
    let (second, _) = game_view(&conn, Some(LIVE), &frames_only, &mut net).expect("view");
    assert_eq!(
        second.draw_odds.as_ref().expect("odds").library_size,
        23,
        "the frame part follows the frame"
    );
    assert!(second.notes.current.is_empty(), "the match parts are kept");

    let (third, _) = game_view(&conn, Some(LIVE), &full, &mut net).expect("view");
    assert_eq!(third.notes.current.len(), 1, "a full compute reads them");

    // New reveals are a full compute even on a frame-only wake (the live
    // read and the potential cards follow them).
    frame(&conn, 30, GAME_ONE, 49, &[(102, "Hand")], &[102]);
    let (fourth, _) = game_view(&conn, Some(LIVE), &frames_only, &mut net).expect("view");
    assert_eq!(fourth.notes.current.len(), 1);
}

/// 0.x ResolveOverlayOpponentTest "resolves the league archetype by uuid,
/// not by a same-named archetype in another format" (U2 carry).
#[test]
fn resolves_the_league_archetype_by_uuid_not_by_name() {
    let (_dir, conn) = db("game-overlay-league-uuid");
    live_match(&conn);
    conn.execute(
        "INSERT INTO archetypes (id, uuid, name, format, source, created_at, updated_at)
         VALUES (101, 'q-pauper-burn', 'Burn', 'pauper', 'downloaded', 1, 1),
                (102, 'q-modern-burn', 'Burn', 'modern', 'downloaded', 1, 1)",
        [],
    )
    .expect("archetypes");
    let port = FakePort::online();
    let mut net = port.net();
    net.answered(
        GameAnswer::Scouted {
            username: "Qrival".to_string(),
            format_key: "modern".to_string(),
            answer: Some(Scouting {
                league: Some(ScoutedArchetype {
                    uuid: "q-modern-burn".to_string(),
                    name: "Burn".to_string(),
                }),
                tracked: None,
                observed: None,
            }),
            failed: false,
        },
        NOW,
    );
    let view = view_with(&conn, Some(LIVE), &settings(), &mut net);
    let opponent = opponent(&view);
    assert_eq!(opponent.archetype_id, Some(102));
    assert_eq!(opponent.source, Source::League);
    assert_eq!(opponent.source_label.as_deref(), Some("5-0 list"));
}

/// Spec 7.3: a uuid v1 does not hold is shown by the API's name at once
/// and handed to Plan T as a stub, once.
#[test]
fn an_unheld_scouted_archetype_is_shown_by_its_name_and_becomes_a_stub() {
    let (_dir, conn) = db("game-overlay-unheld");
    live_match(&conn);
    let port = FakePort::online();
    let mut net = port.net();
    net.answered(
        GameAnswer::Scouted {
            username: "Qrival".to_string(),
            format_key: "modern".to_string(),
            answer: Some(Scouting {
                league: None,
                tracked: Some(ScoutedArchetype {
                    uuid: "q-unmet".to_string(),
                    name: "Qunmet Combo".to_string(),
                }),
                observed: None,
            }),
            failed: false,
        },
        NOW,
    );
    let view = view_with(&conn, Some(LIVE), &settings(), &mut net);
    let opponent = opponent(&view);
    assert_eq!(opponent.archetype_id, None);
    assert_eq!(opponent.archetype_uuid.as_deref(), Some("q-unmet"));
    assert_eq!(opponent.archetype_name.as_deref(), Some("Qunmet Combo"));
    assert_eq!(opponent.source_label.as_deref(), Some("last played"));
    assert!(
        !view.has_archetype,
        "no guide or notes before the row exists"
    );
    view_with(&conn, Some(LIVE), &settings(), &mut net);
    let stubs: Vec<GameRequest> = port
        .requests()
        .into_iter()
        .filter(|request| matches!(request, GameRequest::Stub { .. }))
        .collect();
    assert_eq!(
        stubs,
        vec![GameRequest::Stub {
            uuid: "q-unmet".to_string(),
            name: "Qunmet Combo".to_string(),
            format_key: "modern".to_string(),
            color_identity: None,
        }]
    );
}

/// F2: an estimate uuid with no metagame name asks for the format's list
/// when it is not held (online), once, as the picker does.
#[test]
fn a_missing_metagame_list_is_asked_for_once() {
    let port = FakePort::online();
    let mut net = port.net();
    assert_eq!(net.metagame_name("modern", "q-x", NOW), None);
    assert_eq!(net.metagame_name("modern", "q-y", NOW + 1), None);
    assert_eq!(
        port.requests(),
        vec![GameRequest::Metagame {
            format_key: "modern".to_string()
        }]
    );
    let offline = FakePort::default();
    let mut net = offline.net();
    assert_eq!(net.metagame_name("modern", "q-x", NOW), None);
    assert!(offline.requests().is_empty(), "never offline");
}

/// 0.x "ignores what we faced them on in another format" and "counts the
/// head-to-head record in this format only" (U2 carry, end to end).
#[test]
fn last_faced_and_head_to_head_count_this_format_only() {
    let (_dir, conn) = db("game-overlay-this-format");
    live_match(&conn);
    archetype(&conn, 101, "q-pauper", "Qpauper Faeries", "downloaded");
    archetype(&conn, 102, "q-modern", "Qmodern Faeries", "downloaded");
    a_match(
        &conn,
        2,
        "q-pauper-1",
        Some(1),
        "CPauper",
        "complete",
        "loss",
        NOW - 9_000,
        None,
    );
    a_match(
        &conn,
        3,
        "q-pauper-2",
        Some(1),
        "CPauper",
        "complete",
        "loss",
        NOW - 8_000,
        None,
    );
    a_match(
        &conn,
        4,
        "q-modern-1",
        Some(1),
        "CModern",
        "complete",
        "win",
        NOW - 7_000,
        None,
    );
    opponent_seat(&conn, 2, 101, false);
    let (first, _) = view(&conn, &settings());
    let shown = opponent(&first);
    assert_eq!(shown.previous_matches, 1);
    assert_eq!((shown.wins, shown.losses), (1, 0));
    assert_eq!(shown.head_to_head.as_deref(), Some("Met 1× · won 1"));
    assert_eq!(
        shown.archetype_id, None,
        "the Pauper read is not this format's"
    );

    opponent_seat(&conn, 4, 102, false);
    let (second, _) = view(&conn, &settings());
    assert_eq!(opponent(&second).archetype_id, Some(102));
    assert_eq!(opponent(&second).source_label.as_deref(), Some("you faced"));
}

/// 0.x "does not count an automatic guess on the live match as history".
#[test]
fn does_not_count_an_automatic_guess_on_the_live_match_as_history() {
    let (_dir, conn) = db("game-overlay-live-guess");
    live_match(&conn);
    archetype(&conn, 101, "q-guess", "Qguess", "downloaded");
    opponent_seat(&conn, 1, 101, false);
    let (view, _) = view(&conn, &settings());
    assert_eq!(opponent(&view).archetype_id, None);
    assert_eq!(opponent(&view).source, Source::None);
}

/// 0.x "never calls the API estimator when stats sharing is switched off"
/// (v1: offline, the `Lookup` gate).
#[test]
fn never_calls_the_api_estimator_when_offline() {
    let (_dir, conn) = db("game-overlay-offline");
    live_match(&conn);
    frame(&conn, 10, GAME_ONE, 50, &[], &[102]);
    let port = FakePort::default();
    let view = view_with(&conn, Some(LIVE), &settings(), &mut port.net());
    assert_eq!(opponent(&view).source, Source::None);
    assert!(port.requests().is_empty(), "{:?}", port.requests());
}

/// Review Focus 4: the estimate is asked at most once per distinct reveal
/// set, however many frames arrive meanwhile and after its answer.
#[test]
fn the_estimate_is_asked_once_per_reveal_set() {
    let (_dir, conn) = db("game-overlay-estimate-once");
    live_match(&conn);
    frame(&conn, 10, GAME_ONE, 50, &[], &[102]);
    let port = FakePort::online();
    let mut net = port.net();
    let estimates = |port: &FakePort| {
        port.requests()
            .into_iter()
            .filter_map(|request| match request {
                GameRequest::Estimate { key, cards, .. } => Some((key, cards)),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    view_with(&conn, Some(LIVE), &settings(), &mut net);
    view_with(&conn, Some(LIVE), &settings(), &mut net);
    assert_eq!(
        estimates(&port),
        vec![("1:102:1".to_string(), vec![(102, 1)])]
    );
    net.answered(
        GameAnswer::Estimated {
            key: "1:102:1".to_string(),
            answer: None,
            failed: false,
        },
        NOW,
    );
    view_with(&conn, Some(LIVE), &settings(), &mut net);
    assert_eq!(estimates(&port).len(), 1, "no answer is an answer");
    // #502: one new card waits 20 seconds; two ask at once.
    frame(&conn, 20, GAME_ONE, 50, &[], &[102, 102]);
    view_with(&conn, Some(LIVE), &settings(), &mut net);
    assert_eq!(estimates(&port).len(), 1, "one new card waits");
    view_at(&conn, Some(LIVE), &settings(), &mut net, NOW + 19_999);
    assert_eq!(estimates(&port).len(), 1);
    view_at(&conn, Some(LIVE), &settings(), &mut net, NOW + 20_000);
    assert_eq!(
        estimates(&port).len(),
        2,
        "a new reveal set asks after the wait"
    );
    frame(&conn, 30, GAME_ONE, 50, &[], &[102, 102, 104, 105]);
    view_at(&conn, Some(LIVE), &settings(), &mut net, NOW + 20_001);
    assert_eq!(estimates(&port).len(), 3, "two new cards ask at once");
}

// #502: while a newer set waits (debounced or asked), the overlay keeps the
// last answer it had for the match rather than dropping the guess.
#[test]
fn a_waiting_reveal_set_keeps_the_last_estimate() {
    let (_dir, conn) = db("game-overlay-estimate-keeps");
    live_match(&conn);
    archetype(&conn, 101, "q-estimated", "Qestimated Tron", "downloaded");
    frame(&conn, 10, GAME_ONE, 50, &[], &[102]);
    let port = FakePort::online();
    let mut net = port.net();
    view_with(&conn, Some(LIVE), &settings(), &mut net);
    net.answered(
        GameAnswer::Estimated {
            key: "1:102:1".to_string(),
            answer: Some(mymtgo_api::ArchetypeEstimate {
                uuid: "q-estimated".to_string(),
                confidence: 0.5,
                deck_version_uuid: None,
            }),
            failed: false,
        },
        NOW,
    );
    assert_eq!(
        opponent(&view_with(&conn, Some(LIVE), &settings(), &mut net)).archetype_id,
        Some(101)
    );
    frame(&conn, 20, GAME_ONE, 50, &[], &[102, 104]);
    let debounced = view_with(&conn, Some(LIVE), &settings(), &mut net);
    assert_eq!(opponent(&debounced).archetype_id, Some(101), "debounced");
    let asked = view_at(&conn, Some(LIVE), &settings(), &mut net, NOW + 20_000);
    assert_eq!(opponent(&asked).archetype_id, Some(101), "asked, waiting");
}

/// 0.x "falls back to an API estimate when the local guess is thin and no
/// league list exists", against the stub server.
#[test]
fn falls_back_to_an_api_estimate_when_the_local_guess_is_thin_and_no_league_list_exists() {
    use crate::api::stub::{Reply, Stub};
    let (_dir, conn) = db("game-overlay-api-estimate");
    live_match(&conn);
    archetype(&conn, 101, "q-estimated", "Qestimated Tron", "downloaded");
    frame(&conn, 10, GAME_ONE, 50, &[], &[102]);
    let stub = Stub::start(|request| {
        if request.target == mymtgo_api::archetypes::ESTIMATE_PATH {
            Reply::json(200, r#"[{"uuid":"q-estimated","confidence":0.5}]"#)
        } else {
            Reply::json(404, "{}")
        }
    });
    let api = crate::api::session::testing::device_api(&stub.base_url, false);
    let port = FakePort::online();
    let mut net = port.net();
    view_with(&conn, Some(LIVE), &settings(), &mut net);
    // The `overlay-net` thread's work, run here against the stub.
    for request in port.requests() {
        let answer = match request {
            GameRequest::Scout {
                username,
                format_key,
            } => crate::overlay::game_net::scout(&api.state, &username, &format_key, NOW),
            GameRequest::Estimate { key, format, cards } => {
                crate::overlay::game_net::estimate(&api.state, &key, &format, &cards, NOW)
            }
            other => panic!("unexpected {other:?}"),
        };
        net.answered(answer, NOW);
    }
    let view = view_with(&conn, Some(LIVE), &settings(), &mut net);
    let opponent = opponent(&view);
    assert_eq!(opponent.archetype_id, Some(101));
    assert_eq!(opponent.source, Source::Api);
    assert_eq!(opponent.source_label.as_deref(), Some("guess"));
    let paths: Vec<String> = stub
        .requests()
        .into_iter()
        .map(|seen| seen.target)
        .collect();
    assert_eq!(
        paths,
        vec![
            mymtgo_api::scouting::PLAYERS_PATH.to_string(),
            mymtgo_api::archetypes::ESTIMATE_PATH.to_string()
        ]
    );
}

/// 0.x OverlayDegradationTest "renders the overlay without contacting the
/// api when no match is live".
#[test]
fn renders_the_overlay_without_contacting_the_api_when_no_match_is_live() {
    let (_dir, conn) = db("game-overlay-no-match-offline");
    let port = FakePort::default();
    let view = view_with(&conn, None, &settings(), &mut port.net());
    assert!(!view.has_match);
    assert!(port.requests().is_empty());
    // Online too: no match, nothing to ask.
    let port = FakePort::online();
    view_with(&conn, None, &settings(), &mut port.net());
    assert!(port.requests().is_empty());
}

/// A confident live read wins over scouting; the archetype's colours come
/// from its row (0.x "prefers a confident live estimate over a faked
/// league hit").
#[test]
fn a_confident_live_read_of_the_reveals_names_the_archetype() {
    let (_dir, conn) = db("game-overlay-live-read");
    live_match(&conn);
    card(&conn, 103, "o-murktide", "Murktide Regent", "Creature");
    murktide_definition(&conn);
    frame(&conn, 10, GAME_ONE, 50, &[], &[103]);
    let (view, port) = view(&conn, &settings());
    let opponent = opponent(&view);
    assert_eq!(opponent.archetype_id, Some(101));
    assert_eq!(opponent.source, Source::Live);
    assert_eq!(opponent.source_label, None);
    assert!(
        !port
            .requests()
            .iter()
            .any(|request| matches!(request, GameRequest::Estimate { .. })),
        "a confident read never costs an estimate"
    );
}

fn murktide_definition(conn: &Connection) {
    let definition = r#"{"uuid":"q-murktide","name":"Murktide","format":"modern",
        "conditions":[{"type":"InMainboard","cards":["Murktide Regent"]}],
        "cards":[{"mtgo_id":103,"oracle_id":"o-murktide","name":"Murktide Regent","quantity":4,"sideboard":false}]}"#;
    conn.execute(
        "INSERT INTO archetypes (id, uuid, name, format, source, definition, created_at, updated_at)
         VALUES (101, 'q-murktide', 'Murktide', 'modern', 'downloaded', ?1, 1, 1)",
        [definition],
    )
    .expect("archetype");
    conn.execute(
        "INSERT INTO archetype_cards (archetype_id, list, catalog_id, oracle_id, quantity, sideboard)
         VALUES (101, 1, 103, 'o-murktide', 4, 0)",
        [],
    )
    .expect("list");
}

/// The live read is computed once per reveal set (cached 30 minutes).
#[test]
fn the_live_read_is_kept_per_reveal_set() {
    let mut net = GameNet::default();
    let mut computed = 0;
    for _ in 0..3 {
        net.live_read("1:103:1", NOW, &mut || {
            computed += 1;
            None
        });
    }
    assert_eq!(computed, 1);
    net.live_read(
        "1:103:1",
        NOW + crate::overlay::game_net::LIVE_TTL_MS,
        &mut || {
            computed += 1;
            None
        },
    );
    assert_eq!(computed, 2, "after 30 minutes it is read again");
}

/// Spec 14 (game view budget): the format's held candidates are read once
/// and kept while their stamp stays; a moved stamp, another format or
/// `ArchetypesChanged` (`forget_candidates`) reads them again.
#[test]
fn the_held_candidates_are_kept_until_the_archetypes_change() {
    let loads = std::cell::Cell::new(0);
    let mut load = || {
        loads.set(loads.get() + 1);
        Ok(Vec::new())
    };
    let mut net = GameNet::default();
    for _ in 0..3 {
        net.held_candidates("modern", (3, 50), &mut load)
            .expect("candidates");
    }
    assert_eq!(loads.get(), 1);
    net.held_candidates("modern", (3, 51), &mut load)
        .expect("candidates");
    assert_eq!(loads.get(), 2, "an edit moved the stamp");
    net.held_candidates("legacy", (3, 51), &mut load)
        .expect("candidates");
    assert_eq!(loads.get(), 3, "another format");
    net.forget_candidates();
    net.held_candidates("legacy", (3, 51), &mut load)
        .expect("candidates");
    assert_eq!(loads.get(), 4, "archetypes changed");
}

/// Spec ruling 15: a failed scouting lookup is asked again after 5
/// minutes, an answer after an hour.
#[test]
fn scouting_failures_are_retried_after_five_minutes_and_answers_kept_an_hour() {
    use crate::overlay::game_net::{SCOUTING_FAILURE_TTL_MS, SCOUTING_TTL_MS};
    let port = FakePort::online();
    let mut net = port.net();
    let scouts = |port: &FakePort| {
        port.requests()
            .iter()
            .filter(|request| matches!(request, GameRequest::Scout { .. }))
            .count()
    };
    assert_eq!(net.scouting("Qrival", "modern", NOW), None);
    assert_eq!(net.scouting("Qrival", "modern", NOW + 1), None);
    assert_eq!(scouts(&port), 1, "one in flight");
    let failed = |at| GameAnswer::Scouted {
        username: "Qrival".to_string(),
        format_key: "modern".to_string(),
        answer: None,
        failed: at,
    };
    net.answered(failed(true), NOW);
    net.scouting("Qrival", "modern", NOW + SCOUTING_FAILURE_TTL_MS - 1);
    assert_eq!(scouts(&port), 1);
    net.scouting("Qrival", "modern", NOW + SCOUTING_FAILURE_TTL_MS);
    assert_eq!(scouts(&port), 2);
    net.answered(failed(false), NOW + SCOUTING_FAILURE_TTL_MS);
    net.scouting(
        "Qrival",
        "modern",
        NOW + SCOUTING_FAILURE_TTL_MS + SCOUTING_TTL_MS - 1,
    );
    assert_eq!(scouts(&port), 2, "an answer is kept an hour");
}

// ---- The worker: frames, fit and collapse -------------------------------------

mod worker {
    use std::sync::{Arc, Mutex};
    use std::time::Instant;

    use overlay::{LiveMatch, Rect};

    use super::*;
    use crate::commands::CommandError;
    use crate::overlay::GAME_OVERLAY;
    use crate::overlay::commands::{collapse_message, fit_message};
    use crate::overlay::windows::testing::{Call, FakeWindows};
    use crate::overlay::worker::{OverlayMsg, Worker};

    type Saved = Arc<Mutex<Vec<Vec<(String, String)>>>>;

    fn worker(settings: Settings, collapsed: bool, expanded: Option<u32>) -> (Worker, Saved) {
        let saved: Saved = Arc::new(Mutex::new(Vec::new()));
        let rows = Arc::clone(&saved);
        let worker = Worker::new(settings, collapsed, true)
            .with_expanded_height(expanded)
            .with_save_rows(Box::new(move |batch| {
                crate::bridge::lock(&rows).push(batch);
                Ok(())
            }));
        (worker, saved)
    }

    fn open_at(width: u32, height: u32) -> FakeWindows {
        let mut windows = FakeWindows::default();
        windows.open.insert(
            GAME_OVERLAY.to_string(),
            Rect {
                x: 10,
                y: 10,
                width,
                height,
            },
        );
        windows
    }

    fn wake(
        worker: &mut Worker,
        conn: &Connection,
        windows: &mut FakeWindows,
        messages: Vec<OverlayMsg>,
    ) {
        for message in messages {
            worker.handle(conn, message);
        }
        worker.update(conn, windows, NOW);
    }

    fn resizes(windows: &FakeWindows) -> Vec<(f64, f64)> {
        windows
            .calls
            .iter()
            .filter_map(|call| match call {
                Call::Resized(label, width, height) if label == GAME_OVERLAY => {
                    Some((*width, *height))
                }
                _ => None,
            })
            .collect()
    }

    fn only(opponent: bool, draw_odds: bool, reveals: bool, sideboard: bool) -> Settings {
        Settings {
            overlay_show_opponent: opponent,
            overlay_show_draw_odds: draw_odds,
            overlay_show_reveals: reveals,
            overlay_show_sideboard: sideboard,
            ..settings()
        }
    }

    fn collapse(collapsed: bool, fixed: i32, bar: i32) -> OverlayMsg {
        collapse_message(collapsed, fixed, bar).expect("valid")
    }

    fn fit(fixed: i32, bar: Option<i32>) -> OverlayMsg {
        fit_message(fixed, bar).expect("valid")
    }

    fn rows(saved: &Saved) -> Vec<(String, String)> {
        crate::bridge::lock(saved)
            .last()
            .cloned()
            .unwrap_or_default()
    }

    /// A live match the overlay is open for, then `op`.
    fn playing(op: OverlayMsg) -> Vec<OverlayMsg> {
        vec![
            OverlayMsg::MatchUpdated {
                live: Box::new(live()),
                league: false,
            },
            frames_stored(),
            op,
        ]
    }

    // ---- 0.x CollapseGameOverlayWindowControllerTest ----

    #[test]
    fn is_expanded_by_default() {
        assert!(!crate::settings::overlay_collapsed(&[]));
        assert_eq!(crate::settings::overlay_expanded_height(&[]), None);
        let (worker, _) = worker(settings(), false, None);
        assert_eq!(worker.collapse_state(), (false, None));
    }

    #[test]
    fn collapses_the_overlay_down_to_its_header_and_tab_bar() {
        let (_dir, conn) = db("game-collapse-down");
        let (mut worker, saved) = worker(settings(), false, None);
        live_match(&conn);
        let mut windows = open_at(320, 700);
        wake(
            &mut worker,
            &conn,
            &mut windows,
            playing(collapse(true, 120, 42)),
        );
        assert_eq!(resizes(&windows), vec![(320.0, 162.0)]);
        assert_eq!(worker.collapse_state(), (true, Some(700)));
        assert_eq!(
            rows(&saved),
            vec![
                crate::settings::overlay_collapsed_row(true),
                crate::settings::overlay_expanded_height_row(Some(700)),
            ]
        );
        let pushed: Vec<_> = windows
            .emitted()
            .into_iter()
            .filter(|(label, _)| label == GAME_OVERLAY)
            .collect();
        let last = &pushed.last().expect("a push").1;
        assert_eq!(last.payload["view"]["collapsed"], true);
    }

    #[test]
    fn restores_the_height_the_overlay_had_before_it_was_collapsed() {
        let (_dir, conn) = db("game-collapse-restore");
        let (mut worker, saved) = worker(settings(), true, Some(700));
        live_match(&conn);
        let mut windows = open_at(320, 162);
        wake(
            &mut worker,
            &conn,
            &mut windows,
            playing(collapse(false, 120, 42)),
        );
        assert_eq!(resizes(&windows), vec![(320.0, 700.0)]);
        assert_eq!(worker.collapse_state(), (false, None));
        assert_eq!(
            rows(&saved),
            vec![
                crate::settings::overlay_collapsed_row(false),
                crate::settings::overlay_expanded_height_row(None),
            ]
        );
    }

    #[test]
    fn falls_back_to_the_default_height_when_expanding_with_nothing_remembered() {
        let (_dir, conn) = db("game-collapse-default");
        let (mut worker, _) = worker(only(true, true, false, false), true, None);
        live_match(&conn);
        let mut windows = open_at(320, 162);
        wake(
            &mut worker,
            &conn,
            &mut windows,
            playing(collapse(false, 120, 42)),
        );
        assert_eq!(resizes(&windows), vec![(320.0, 120.0 + 520.0)]);
    }

    #[test]
    fn keeps_the_remembered_height_when_collapse_is_sent_twice() {
        let (_dir, conn) = db("game-collapse-twice");
        let (mut worker, _) = worker(settings(), false, None);
        live_match(&conn);
        let mut windows = open_at(320, 700);
        wake(
            &mut worker,
            &conn,
            &mut windows,
            playing(collapse(true, 120, 42)),
        );
        // The window is now the collapsed strip; a repeat keeps 700.
        wake(
            &mut worker,
            &conn,
            &mut windows,
            vec![collapse(true, 120, 42)],
        );
        assert_eq!(resizes(&windows).len(), 2);
        assert_eq!(worker.collapse_state(), (true, Some(700)));
    }

    #[test]
    fn still_records_the_choice_when_the_overlay_window_is_not_open() {
        let (_dir, conn) = db("game-collapse-closed");
        let (mut worker, saved) = worker(settings(), false, None);
        let mut windows = FakeWindows::default();
        wake(
            &mut worker,
            &conn,
            &mut windows,
            vec![collapse(true, 120, 42)],
        );
        assert!(resizes(&windows).is_empty());
        assert_eq!(worker.collapse_state(), (true, None));
        assert_eq!(
            rows(&saved)[0],
            crate::settings::overlay_collapsed_row(true)
        );
    }

    #[test]
    fn rejects_missing_or_absurd_values() {
        assert_eq!(
            collapse_message(true, 120, 5000),
            Err(CommandError::Refused(
                "The bar height field must not be greater than 200.".to_string()
            ))
        );
        assert_eq!(
            collapse_message(true, -1, 42),
            Err(CommandError::Refused(
                "The fixed height field must be at least 0.".to_string()
            ))
        );
        assert!(collapse_message(false, 2000, 200).is_ok());
    }

    // ---- 0.x FitGameOverlayWindowControllerTest ----

    #[test]
    fn fits_the_overlay_window_to_the_height_the_page_measured() {
        let (_dir, conn) = db("game-fit");
        let (mut worker, _) = worker(only(true, false, false, false), false, None);
        live_match(&conn);
        let mut windows = open_at(320, 640);
        wake(&mut worker, &conn, &mut windows, playing(fit(142, None)));
        assert_eq!(resizes(&windows), vec![(320.0, 142.0)]);
    }

    #[test]
    fn rejects_a_missing_or_absurd_height() {
        assert_eq!(
            fit_message(5000, None),
            Err(CommandError::Refused(
                "The fixed height field must not be greater than 2000.".to_string()
            ))
        );
        assert_eq!(
            fit_message(100, Some(201)),
            Err(CommandError::Refused(
                "The bar height field must not be greater than 200.".to_string()
            ))
        );
    }

    #[test]
    fn keeps_a_collapsed_overlay_collapsed_when_it_refits() {
        let (_dir, conn) = db("game-fit-collapsed");
        let (mut worker, _) = worker(only(true, true, false, false), true, None);
        live_match(&conn);
        let mut windows = open_at(320, 162);
        // The opponent header grew once an archetype resolved.
        wake(
            &mut worker,
            &conn,
            &mut windows,
            playing(fit(138, Some(42))),
        );
        assert_eq!(resizes(&windows), vec![(320.0, 180.0)]);
    }

    // ---- 0.x FitGameOverlayWindowTest ----

    #[test]
    fn resizes_the_open_overlay_to_the_measured_fixed_content_keeping_its_current_width() {
        let (_dir, conn) = db("game-fit-width");
        let (mut worker, _) = worker(only(true, false, false, false), false, None);
        live_match(&conn);
        let mut windows = open_at(352, 640);
        wake(&mut worker, &conn, &mut windows, playing(fit(137, None)));
        assert_eq!(resizes(&windows), vec![(352.0, 137.0)]);
    }

    #[test]
    fn adds_the_tab_area_when_a_tab_section_is_enabled() {
        let (_dir, conn) = db("game-fit-tabs");
        let (mut worker, _) = worker(only(true, false, true, false), false, None);
        live_match(&conn);
        let mut windows = open_at(320, 200);
        wake(&mut worker, &conn, &mut windows, playing(fit(100, None)));
        assert_eq!(
            resizes(&windows),
            vec![(320.0, f64::from(100 + overlay::height::TAB_AREA))]
        );
    }

    // The restored height survives the page's first fit (#349).
    #[test]
    fn keeps_the_players_height_when_a_tab_section_is_enabled() {
        let (_dir, conn) = db("game-fit-keeps-height");
        let (mut worker, _) = worker(only(true, true, false, false), false, None);
        live_match(&conn);
        let mut windows = open_at(320, 900);
        wake(&mut worker, &conn, &mut windows, playing(fit(100, None)));
        assert!(resizes(&windows).is_empty());
    }

    #[test]
    fn does_nothing_when_the_overlay_window_is_not_open() {
        let (_dir, conn) = db("game-fit-closed");
        let (mut worker, _) = worker(settings(), false, None);
        let mut windows = FakeWindows::default();
        windows.open.insert(
            "main".to_string(),
            Rect {
                x: 0,
                y: 0,
                width: 800,
                height: 600,
            },
        );
        wake(&mut worker, &conn, &mut windows, vec![fit(100, None)]);
        assert!(resizes(&windows).is_empty());
    }

    // ---- Review Focus 1 ----

    fn live() -> LiveMatch {
        LiveMatch {
            token: LIVE.to_string(),
            source: "log".to_string(),
            state: "in_progress".to_string(),
            game_count: 1,
            format: Some("CModern".to_string()),
            hidden: false,
            needs_attention: false,
            started_at: NOW - 60_000,
        }
    }

    fn frames_stored() -> OverlayMsg {
        OverlayMsg::LiveEventsStored {
            tokens: vec![LIVE.to_string()],
            detected_at: Instant::now(),
        }
    }

    fn pushed_library_sizes(windows: &FakeWindows) -> Vec<u64> {
        windows
            .emitted()
            .into_iter()
            .filter(|(label, _)| label == GAME_OVERLAY)
            .filter_map(|(_, event)| event.payload["view"]["drawOdds"]["librarySize"].as_u64())
            .collect()
    }

    /// Every committed frame of the live match reaches the overlay's draw
    /// odds, in order.
    #[test]
    fn the_draw_odds_follow_every_committed_frame() {
        let (_dir, conn) = db("game-frames-follow");
        live_match(&conn);
        frame(&conn, 10, GAME_ONE, 50, &[], &[]);
        let (mut worker, _) = worker(settings(), false, None);
        let mut windows = open_at(320, 640);
        wake(
            &mut worker,
            &conn,
            &mut windows,
            vec![
                OverlayMsg::MatchUpdated {
                    live: Box::new(live()),
                    league: false,
                },
                frames_stored(),
            ],
        );
        let mut drawn: Vec<(u32, &str)> = Vec::new();
        for (offset, zone) in [(20, "Hand"), (30, "Battlefield"), (40, "Graveyard")] {
            drawn.push((102, zone));
            frame(&conn, offset, GAME_ONE, 50, &drawn, &[]);
            wake(&mut worker, &conn, &mut windows, vec![frames_stored()]);
        }
        assert_eq!(pushed_library_sizes(&windows), vec![24, 23, 22, 21]);
        let revisions: Vec<u64> = windows
            .emitted()
            .into_iter()
            .filter(|(label, _)| label == GAME_OVERLAY)
            .filter_map(|(_, event)| event.payload["revision"].as_u64())
            .collect();
        assert!(
            revisions.windows(2).all(|pair| pair[0] < pair[1]),
            "{revisions:?}"
        );
    }
    /// Ruling C6: Plan T's archetype events reach the overlay worker, and
    /// a guide or note change (an app event) recomputes the match parts.
    /// Spec 14 (U12 review I1, I2): the kept candidates are read again
    /// after any archetype edit (`ArchetypesChanged`, sent for every manual
    /// edit, removing a variant included) and after card rows were filled
    /// in (`CardsChanged`), which the stamp does not see.
    #[test]
    fn archetype_and_card_changes_make_the_live_read_reread_its_candidates() {
        use crate::overlay::worker::from_tracker;
        assert_eq!(
            from_tracker(&tracker::TrackerEvent::CardsChanged),
            Some(OverlayMsg::CardsChanged)
        );
        let (_dir, conn) = db("game-candidates-forgotten");
        live_match(&conn);
        card(&conn, 103, "o-murktide", "Murktide Regent", "Creature");
        murktide_definition(&conn);
        frame(&conn, 10, GAME_ONE, 50, &[], &[103]);
        let (mut worker, _) = worker(settings(), false, None);
        let mut windows = open_at(320, 640);
        let full = || OverlayMsg::MatchUpdated {
            live: Box::new(live()),
            league: false,
        };
        wake(
            &mut worker,
            &conn,
            &mut windows,
            vec![full(), frames_stored()],
        );
        assert_eq!(worker.game_net().candidate_reads(), 1);
        wake(&mut worker, &conn, &mut windows, vec![full()]);
        assert_eq!(worker.game_net().candidate_reads(), 1, "kept");
        wake(
            &mut worker,
            &conn,
            &mut windows,
            vec![OverlayMsg::ArchetypesChanged],
        );
        assert_eq!(worker.game_net().candidate_reads(), 2, "an archetype edit");
        wake(
            &mut worker,
            &conn,
            &mut windows,
            vec![OverlayMsg::CardsChanged],
        );
        wake(&mut worker, &conn, &mut windows, vec![full()]);
        assert_eq!(
            worker.game_net().candidate_reads(),
            3,
            "card rows filled in"
        );
    }

    #[test]
    fn archetype_and_guide_events_recompute_the_match_parts() {
        use crate::overlay::worker::from_tracker;
        assert_eq!(
            from_tracker(&tracker::TrackerEvent::ArchetypesChanged),
            Some(OverlayMsg::ArchetypesChanged)
        );
        assert_eq!(
            from_tracker(&tracker::TrackerEvent::MatchArchetypesChanged { match_ids: vec![1] }),
            Some(OverlayMsg::MatchArchetypesChanged { match_ids: vec![1] })
        );

        let (_dir, conn) = db("game-guides-changed");
        live_match(&conn);
        archetype(&conn, 101, "q-blink", "Esper Blink", "downloaded");
        opponent_seat(&conn, 1, 101, true);
        let (mut worker, _) = worker(settings(), false, None);
        let mut windows = open_at(320, 640);
        wake(
            &mut worker,
            &conn,
            &mut windows,
            vec![
                OverlayMsg::MatchUpdated {
                    live: Box::new(live()),
                    league: false,
                },
                frames_stored(),
            ],
        );
        note(&conn, 1, 101, "Qnew note", NOW);
        wake(&mut worker, &conn, &mut windows, vec![frames_stored()]);
        let notes = |windows: &FakeWindows| {
            windows
                .emitted()
                .into_iter()
                .rfind(|(label, _)| label == GAME_OVERLAY)
                .map(|(_, event)| event.payload["view"]["notes"]["current"].clone())
        };
        assert_eq!(notes(&windows), Some(serde_json::json!([])));
        wake(
            &mut worker,
            &conn,
            &mut windows,
            vec![OverlayMsg::GuidesChanged {
                deck_id: 1,
                archetype_id: 101,
            }],
        );
        assert_eq!(
            notes(&windows).expect("pushed")[0]["body"],
            serde_json::json!("Qnew note")
        );
    }
}

// #503: when the lookups could not reach MyMTGO and nothing names the
// archetype, the opponent says so instead of "Unknown archetype"; the next
// answer that arrives clears it.
#[test]
fn an_unreachable_api_is_said_instead_of_unknown_archetype() {
    let (_dir, conn) = db("game-overlay-unreachable");
    live_match(&conn);
    frame(&conn, 10, GAME_ONE, 50, &[], &[102]);
    let port = FakePort::online();
    let mut net = port.net();
    let first = view_with(&conn, Some(LIVE), &settings(), &mut net);
    assert!(!opponent(&first).unreachable, "nothing failed yet");
    net.answered(
        GameAnswer::Estimated {
            key: "1:102:1".to_string(),
            answer: None,
            failed: true,
        },
        NOW,
    );
    let failed = view_with(&conn, Some(LIVE), &settings(), &mut net);
    assert_eq!(opponent(&failed).archetype_name, None);
    assert!(opponent(&failed).unreachable);
    net.answered(
        GameAnswer::Estimated {
            key: "1:102:1".to_string(),
            answer: None,
            failed: false,
        },
        NOW,
    );
    let answered = view_with(&conn, Some(LIVE), &settings(), &mut net);
    assert!(!opponent(&answered).unreachable, "MyMTGO answered");
}

// Review of #502: a set held back for one new card asks for a wake at the
// end of the wait, so the overlay recomputes and asks even when nothing
// else changes; the wake is taken once.
#[test]
fn a_held_back_reveal_set_asks_for_a_wake() {
    let (_dir, conn) = db("game-overlay-estimate-wake");
    live_match(&conn);
    frame(&conn, 10, GAME_ONE, 50, &[], &[102]);
    let port = FakePort::online();
    let mut net = port.net();
    view_with(&conn, Some(LIVE), &settings(), &mut net);
    assert_eq!(net.estimate_wake_at(), None, "asked at once");
    frame(&conn, 20, GAME_ONE, 50, &[], &[102, 102]);
    view_with(&conn, Some(LIVE), &settings(), &mut net);
    let wake = NOW + overlay::ESTIMATE_GAP_MS;
    assert_eq!(net.estimate_wake_at(), Some(wake));
    assert!(!net.take_due_estimates(wake - 1));
    assert!(net.take_due_estimates(wake));
    assert_eq!(net.estimate_wake_at(), None, "taken once");
}

// Review of #502: a match's last ask is forgotten once its answer has
// expired, so the asks kept do not grow by one a match for the app's life.
#[test]
fn a_finished_matchs_estimate_ask_is_forgotten() {
    use crate::overlay::game_net::ESTIMATE_TTL_MS;
    let port = FakePort::online();
    let mut net = port.net();
    net.estimate(1, "1:102:1", "modern", &[(102, 1)], NOW);
    assert_eq!(net.estimate_matches(), vec![1]);
    let later = NOW + ESTIMATE_TTL_MS;
    net.estimate(2, "2:103:1", "modern", &[(103, 1)], later);
    assert_eq!(net.estimate_matches(), vec![2], "match 1's ask expired");
}

// ---- The league card's opponent name ----

fn card_name(
    conn: &Connection,
    known: Option<&KnownOpponent>,
    net: &mut GameNet,
) -> Option<String> {
    card_opponent_archetype(conn, LIVE, known, NOW, net).expect("the card's name")
}

#[test]
fn the_card_names_a_manual_pick_without_asking_the_network() {
    let (_dir, conn) = db("card-opponent-manual");
    live_match(&conn);
    archetype(&conn, 101, "q-blink", "Esper Blink", "downloaded");
    opponent_seat(&conn, 1, 101, true);
    let port = FakePort::online();
    assert_eq!(
        card_name(&conn, None, &mut port.net()).as_deref(),
        Some("Esper Blink")
    );
    assert!(port.requests().is_empty(), "{:?}", port.requests());
}

#[test]
fn the_card_names_the_opponent_last_faced() {
    let (_dir, conn) = db("card-opponent-last-faced");
    live_match(&conn);
    archetype(&conn, 101, "q-faeries", "Qmodern Faeries", "downloaded");
    a_match(
        &conn,
        2,
        "q-modern-1",
        Some(1),
        "CModern",
        "complete",
        "win",
        NOW - 7_000,
        None,
    );
    opponent_seat(&conn, 2, 101, false);
    let port = FakePort::online();
    assert_eq!(
        card_name(&conn, None, &mut port.net()).as_deref(),
        Some("Qmodern Faeries")
    );
    assert!(port.requests().is_empty(), "{:?}", port.requests());
}

#[test]
fn the_card_reads_the_reveals_locally_and_never_asks_for_an_estimate() {
    let (_dir, conn) = db("card-opponent-live-read");
    live_match(&conn);
    card(&conn, 103, "o-murktide", "Murktide Regent", "Creature");
    murktide_definition(&conn);
    frame(&conn, 10, GAME_ONE, 50, &[], &[103]);
    let port = FakePort::online();
    assert_eq!(
        card_name(&conn, None, &mut port.net()).as_deref(),
        Some("Murktide")
    );
    assert!(port.requests().is_empty(), "{:?}", port.requests());
}

#[test]
fn the_card_has_no_name_and_asks_for_nothing_when_nothing_local_answers() {
    let (_dir, conn) = db("card-opponent-nothing");
    live_match(&conn);
    frame(&conn, 10, GAME_ONE, 50, &[], &[102]);
    let port = FakePort::online();
    assert_eq!(card_name(&conn, None, &mut port.net()), None);
    assert!(
        port.requests().is_empty(),
        "no scouting and no estimate for the card: {:?}",
        port.requests()
    );
}

#[test]
fn the_card_has_no_name_before_the_opponent_is_known() {
    let (_dir, conn) = db("card-opponent-waiting");
    a_match(
        &conn,
        1,
        LIVE,
        None,
        "CModern",
        "in_progress",
        "unknown",
        NOW - 60_000,
        Some(1),
    );
    assert_eq!(card_name(&conn, None, &mut GameNet::default()), None);
}

#[test]
fn the_game_overlays_answer_is_used_for_its_own_match_only() {
    let (_dir, conn) = db("card-opponent-known");
    live_match(&conn);
    let known = KnownOpponent {
        token: LIVE.to_string(),
        archetype: "Dimir Tempo".to_string(),
    };
    let net = &mut GameNet::default();
    assert_eq!(
        card_name(&conn, Some(&known), net).as_deref(),
        Some("Dimir Tempo")
    );
    let other = KnownOpponent {
        token: "q-another".to_string(),
        archetype: "Dimir Tempo".to_string(),
    };
    assert_eq!(card_name(&conn, Some(&other), net), None);
}

#[test]
fn a_memo_names_its_match_with_the_name_the_header_shows() {
    let (_dir, conn) = db("card-opponent-memo");
    live_match(&conn);
    archetype(&conn, 101, "q-blink", "Esper Blink", "downloaded");
    opponent_seat(&conn, 1, 101, true);
    let port = FakePort::online();
    let mut net = port.net();
    let settings = settings();
    let inputs = GameInputs {
        settings: &settings,
        collapsed: false,
        now_ms: NOW,
        previous: None,
    };
    let (_, memo) = game_view(&conn, Some(LIVE), &inputs, &mut net).expect("view");
    assert_eq!(
        memo.expect("a memo").known_opponent(),
        Some(KnownOpponent {
            token: LIVE.to_string(),
            archetype: "Esper Blink".to_string(),
        })
    );

    let quiet = Settings {
        overlay_show_opponent: false,
        overlay_show_reveals: false,
        overlay_show_sideboard: false,
        ..settings
    };
    let inputs = GameInputs {
        settings: &quiet,
        collapsed: false,
        now_ms: NOW,
        previous: None,
    };
    let (_, memo) = game_view(&conn, Some(LIVE), &inputs, &mut net).expect("view");
    assert_eq!(
        memo.expect("a memo").known_opponent(),
        None,
        "nothing resolved with every section off: the card reads locally"
    );
}
