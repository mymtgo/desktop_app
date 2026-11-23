use super::*;
use std::sync::Arc;

use chrono::TimeZone;
use fixture_tools::{TestDir, load_fixture, load_game_log};
use game_detail::{detect_players, local_deck, parse_game_log, split_into_games};
use mtgo_match::{EndReason, GameResult, GameView, MatchState, MatchView, Outcome};
use mtgo_protocol::Subjects;
use store::game_sync::FrameText;
use store::import::{ImportedGame, ImportedMatch, import_matches};
use store::manual::CardCount;
use store::write::{MatchWrite, NewEvent, NewLogFile, WriteBatch, commit, insert_log_file};
use tracker::history::History;
use tracker::{Engine, TrackerConfig};

const UTC: Tz = chrono_tz::UTC;

// ---- Fixtures through the tracker ----------------------------------------------

/// The fixture log read by the tracker into a fresh database, as the tracker
/// tests do. Returns the folder (kept alive) and a connection to the database.
pub(crate) fn tracked(name: &str, label: &str) -> (TestDir, Connection) {
    let fixture = load_fixture(name);
    let dir = TestDir::new(label);
    dir.write("logs/mtgo.log", &fixture.bytes);
    let config = TrackerConfig {
        db_path: dir.join("data/mymtgo.sqlite"),
        timezone: fixture.meta.tz(),
        os_timezone: None,
        log_overrides: vec![dir.join("logs")],
        arena_log_overrides: vec![dir.join("arena/Player.log")],
        mtgo_data_folder: Some(dir.join("mtgo-data")),
        replay_day_anchor: Some(fixture.meta.day_anchor()),
        watching: true,
        arena_enabled: true,
        endstep_enabled: false,
    };
    // Hours later, so every match is finished.
    let mut now = fixture.meta.now_ms() + 6 * 3_600_000;
    let mut engine = Engine::open(
        config.clone(),
        Env::current(),
        Arc::default(),
        Vec::new(),
        now,
    )
    .expect("open engine");
    for _ in 0..10_000 {
        let tick = engine.tick(now).expect("tick");
        now += 1;
        if !tick.more {
            break;
        }
    }
    for _ in 0..3 {
        now += 1_000;
        engine.tick(now).expect("tick");
    }
    engine.flush().expect("flush");
    drop(engine);
    let conn = store::open_writer(&config.db_path).expect("writer");
    (dir, conn)
}

/// The token of the match that played Game ID `game_id`.
fn token_of_game(conn: &Connection, game_id: u64) -> String {
    conn.query_row(
        "SELECT m.token FROM matches m JOIN games g ON g.match_id = m.id WHERE g.mtgo_game_id = ?1",
        [game_id as i64],
        |row| row.get(0),
    )
    .expect("a match for the game")
}

fn number_of_game(conn: &Connection, game_id: u64) -> u32 {
    conn.query_row(
        "SELECT game_number FROM games WHERE mtgo_game_id = ?1",
        [game_id as i64],
        |row| row.get(0),
    )
    .expect("the game")
}

fn shown(conn: &Connection, token: &str) -> MatchDetail {
    detail(conn, token, &[])
        .expect("detail")
        .expect("the match is listed")
}

fn game(detail: &MatchDetail, number: u32) -> &GameDetailView {
    detail
        .games
        .iter()
        .find(|game| game.number == number)
        .expect("the game")
}

fn reveal_ids(game: &GameDetailView) -> Vec<(u32, u32)> {
    game.opponent_cards_seen
        .iter()
        .map(|card| (card.catalog_id, card.quantity))
        .collect()
}

#[test]
fn a_league_match_shows_its_hero_hands_and_reveals_from_the_frames() {
    let (_dir, conn) = tracked("league_join_drop", "matches-detail-league");
    conn.execute_batch(
        "INSERT OR REPLACE INTO cards (catalog_id, name, type, color_identity, image) VALUES
             (136111, 'Qbeast', 'Creature - Beast', 'G', 'https://cards.scryfall.io/q/1.jpg');",
    )
    .expect("cards");
    let token = token_of_game(&conn, 945_500_844);
    let detail = shown(&conn, &token);
    assert_eq!(detail.token, token);
    assert!(!detail.imported);
    assert!(!detail.manual);
    assert_eq!(detail.format.as_deref(), Some("Modern"));
    assert!(detail.opponent.is_some());
    assert_eq!(detail.deck, None, "no deck files in this test");
    assert_eq!(detail.notes, None);

    let first = game(&detail, number_of_game(&conn, 945_500_844));
    assert_eq!(first.opening_hand.len(), 7);
    assert!(first.opening_hand.iter().all(|card| !card.bottomed));
    assert!(first.mulliganed_hands.is_empty());
    assert_eq!((first.local_mulligans, first.opponent_mulligans), (0, 0));
    // Per-card rows: the reveals read in catalog order.
    assert_eq!(reveal_ids(first), vec![(34_832, 1), (136_111, 1)]);
    let beast = &first.opponent_cards_seen[1];
    assert_eq!(beast.name, "Qbeast");
    assert_eq!(beast.type_line.as_deref(), Some("Creature - Beast"));
    assert_eq!(beast.colors, vec!["G".to_string()]);
    assert_eq!(
        beast.image.as_deref(),
        Some("https://cards.scryfall.io/q/1.jpg")
    );
    // No card data: the name the game log gave its stub (Plan S task 1).
    let unknown = &first.opponent_cards_seen[0];
    assert_eq!(unknown.name, "Expedition Map");
    assert_eq!(unknown.type_line, None);
    assert_eq!(unknown.image, None);
    assert!(
        first
            .opening_hand
            .iter()
            .all(|card| card.name.starts_with("Card "))
    );
    assert!(first.sideboard_changes.is_empty(), "game 1 has none");
    assert!(first.has_game_log, "the MetaMessages rebuild the log");

    let ended_early = game(&detail, number_of_game(&conn, 945_500_946));
    assert!(ended_early.opening_hand.is_empty());
    assert!(ended_early.opponent_cards_seen.is_empty());
}

#[test]
fn each_game_says_whether_it_has_frames_to_replay() {
    // A tracked match: its games are rows, with frames.
    let (_dir, conn) = tracked("league_join_drop", "matches-detail-has-frames");
    let token = token_of_game(&conn, 945_500_844);
    let detail = shown(&conn, &token);
    assert_eq!(detail.source, "log");
    let match_id: i64 = conn
        .query_row("SELECT id FROM matches WHERE token = ?1", [&token], |row| {
            row.get(0)
        })
        .expect("match id");
    let with_frames = store::replay::games_with_frames(&conn, match_id).expect("read");
    assert!(!with_frames.is_empty());
    for game in &detail.games {
        let id: i64 = conn
            .query_row(
                "SELECT id FROM games WHERE match_id = ?1 AND game_number = ?2",
                store::params![match_id, game.number],
                |row| row.get(0),
            )
            .expect("game id");
        assert_eq!(i64::from(game.id), id);
        assert_eq!(game.has_frames, with_frames.contains(&id), "game {id}");
    }

    // A manual match never has any.
    let (_manual_dir, manual) = manual_match("matches-detail-has-frames-manual");
    let detail = shown(&manual, "q-manual");
    assert_eq!(detail.source, "manual");
    assert!(!detail.games.is_empty());
    assert!(
        detail
            .games
            .iter()
            .all(|game| !game.has_frames && game.id > 0)
    );
}

#[test]
fn a_recorded_game_has_frames() {
    let (_dir, conn) = manual_match("matches-detail-recorded-frames");
    conn.execute_batch(
        "INSERT INTO recorded_replays (token, text, file_hash, imported_at)
         VALUES ('q-manual', '[Game]', 'h', 0);
         INSERT INTO recorded_replay_games (token, position, game_number)
         VALUES ('q-manual', 0, 2);",
    )
    .expect("a recording of game 2");
    let detail = shown(&conn, "q-manual");
    let by_number: Vec<(u32, bool)> = detail
        .games
        .iter()
        .map(|game| (game.number, game.has_frames))
        .collect();
    assert_eq!(by_number, vec![(1, false), (2, true)]);
}

#[test]
fn a_draft_game_shows_its_mulligan_bottomed_card_and_the_draw() {
    let (_dir, conn) = tracked("draft_hobbit", "matches-detail-draft");
    let mulligan_token = token_of_game(&conn, 959_763_634);
    let detail = shown(&conn, &mulligan_token);
    let mulligan = game(&detail, number_of_game(&conn, 959_763_634));
    assert_eq!(mulligan.local_mulligans, 1);
    assert_eq!(mulligan.mulliganed_hands.len(), 1);
    assert_eq!(
        mulligan
            .opening_hand
            .iter()
            .filter(|card| card.bottomed)
            .count(),
        1
    );
    assert_eq!(mulligan.opponent_cards_seen.len(), 11);

    let draw_token = token_of_game(&conn, 959_762_288);
    let detail = shown(&conn, &draw_token);
    let on_the_draw = game(&detail, number_of_game(&conn, 959_762_288));
    assert_eq!(on_the_draw.on_the_play, Some(false));
    assert_eq!(
        on_the_draw.opening_hand.len(),
        8,
        "the first draw is in the hand, as 0.x"
    );
}

#[test]
fn only_each_game_s_first_deck_used_list_counts() {
    let (_dir, conn) = tracked("draft_hobbit", "matches-detail-deck-used");
    // MTGO logged this game's list twice.
    let token = token_of_game(&conn, 959_762_636);
    let events = History::default()
        .match_events(&conn, &token)
        .expect("events");
    let lists = first_deck_lists(&events);
    let first_raw = events
        .iter()
        .find(|event| {
            event.kind == EventKind::DeckUsed && event.subjects.game_id == Some(959_762_636)
        })
        .expect("a deck_used line");
    let (_, first) = decks::parse_deck_used(&first_raw.text).expect("parses");
    assert_eq!(lists.get(&959_762_636), Some(&first));
    let doubled = events
        .iter()
        .filter(|event| {
            event.kind == EventKind::DeckUsed && event.subjects.game_id == Some(959_762_636)
        })
        .count();
    assert_eq!(doubled, 2, "the fixture logs it twice");

    // Register exactly this game's deck: a doubled list would show every
    // card as boarded in.
    let frames: Vec<&str> = events
        .iter()
        .filter(|event| {
            event.kind == EventKind::GameStateUpdate && event.subjects.game_id == Some(959_762_636)
        })
        .map(|event| event.text.as_str())
        .collect();
    let frames = parse_frames(&frames);
    let local = frames[0]
        .players
        .iter()
        .find(|player| player.name == "Qtixxxxxxx")
        .expect("local player")
        .id;
    let registered = local_deck(&first, &frames[0], local);
    link_version(&conn, &token, &registered);
    let detail = shown(&conn, &token);
    let number = number_of_game(&conn, 959_762_636);
    assert!(number >= 2, "a sideboarded game");
    assert_eq!(game(&detail, number).sideboard_changes, Vec::new());
}

/// Link `token` to a new deck version holding `cards`.
fn link_version(conn: &Connection, token: &str, cards: &[DeckCard]) {
    let signature = decks::signature(cards);
    conn.execute_batch(&format!(
        "INSERT INTO decks (id, mtgo_id, name, format, color_identity, created_at, updated_at, source)
             VALUES (50, 'q-deck', 'Qdeck', 'CMODERN', 'U,R', 0, 0, 'file');
         INSERT INTO deck_versions (id, deck_id, signature, modified_at) VALUES (60, 50, '{signature}', 0);
         UPDATE matches SET deck_version_id = 60 WHERE token = '{token}';"
    ))
    .expect("link");
}

#[test]
fn a_linked_deck_names_the_hero_and_later_games_show_their_sideboard_changes() {
    let (_dir, conn) = tracked("league_join_drop", "matches-detail-sideboard");
    let token = token_of_game(&conn, 945_500_844);
    // A registered list unlike anything played: game 2 boards every card.
    link_version(
        &conn,
        &token,
        &[DeckCard {
            catalog_id: 999_001,
            quantity: 4,
            sideboard: false,
        }],
    );
    conn.execute_batch(
        "INSERT INTO cards (catalog_id, name, image) VALUES (999001, 'Qcut', 'https://cards.scryfall.io/q/c.jpg');",
    )
    .expect("cards");
    let detail = shown(&conn, &token);
    assert_eq!(
        detail.deck,
        Some(MatchDeckView {
            id: 50,
            name: "Qdeck".to_string(),
            colors: vec!["U".to_string(), "R".to_string()],
            version_id: 60,
        })
    );
    assert!(
        game(&detail, 1).sideboard_changes.is_empty(),
        "never for game 1"
    );
    // Game 2 ended before any hand, but its deck was logged.
    let second = game(&detail, number_of_game(&conn, 945_500_946));
    let cut = second
        .sideboard_changes
        .iter()
        .find(|change| change.catalog_id == 999_001)
        .expect("the registered card is boarded out");
    assert_eq!(
        (cut.kind, cut.quantity, cut.name.as_str()),
        (SideboardKind::Out, 4, "Qcut")
    );
    assert!(
        second
            .sideboard_changes
            .iter()
            .any(|change| change.kind == SideboardKind::In)
    );
}

#[test]
fn a_compacted_match_shows_the_same_detail() {
    let (_dir, mut conn) = tracked("draft_hobbit", "matches-detail-compacted");
    let token = token_of_game(&conn, 959_763_634);
    let before = shown(&conn, &token);
    let lines_before = game_log_lines(&conn, &token, 1, &[], UTC).expect("log");
    assert!(!lines_before.is_empty());
    commit(
        &mut conn,
        &WriteBatch {
            compact: vec![token.clone()],
            now_ms: 1,
            ..WriteBatch::default()
        },
    )
    .expect("compact");
    assert!(
        store::read::live_events_for_match(&conn, &token)
            .expect("live")
            .is_empty(),
        "every event moved to the archive"
    );
    assert_eq!(shown(&conn, &token), before);
    assert_eq!(
        game_log_lines(&conn, &token, 1, &[], UTC).expect("log"),
        lines_before
    );
}

#[test]
fn a_hidden_or_unknown_match_has_no_detail() {
    let (_dir, conn) = tracked("league_join_drop", "matches-detail-hidden");
    let token = token_of_game(&conn, 945_500_844);
    conn.execute(
        "UPDATE matches SET hidden_at = 1 WHERE token = ?1",
        [&token],
    )
    .expect("hide");
    assert_eq!(detail(&conn, &token, &[]).expect("detail"), None);
    assert_eq!(detail(&conn, "q-none", &[]).expect("detail"), None);
    assert!(
        game_log_lines(&conn, "q-none", 1, &[], UTC)
            .expect("log")
            .is_empty()
    );
}

// ---- A manual match -------------------------------------------------------------

/// A manual match (Plan L) of Qlocal's deck 5, whose version 10 is 4 Qbolt
/// (4001), 4 Qgoyf (4002) and 20 Qmountain (4004) main and 3 Qrest (4003)
/// in the sideboard. Game 1 has a recorded hand after one mulligan and two
/// reveals; game 2 sided 2 Qrest in for 2 Qgoyf.
fn manual_match(label: &str) -> (TestDir, Connection) {
    let dir = TestDir::new(label);
    let mut conn = store::open_writer(&dir.join("mymtgo.sqlite")).expect("open");
    let card = |catalog_id, quantity, sideboard| DeckCard {
        catalog_id,
        quantity,
        sideboard,
    };
    let registered = decks::signature(&[
        card(4001, 4, false),
        card(4002, 4, false),
        card(4004, 20, false),
        card(4003, 3, true),
    ]);
    conn.execute_batch(&format!(
        "INSERT INTO accounts (id, username, first_seen_at) VALUES (1, 'Qlocal', 0);
         INSERT INTO opponents (id, username, first_seen_at) VALUES (1, 'Qkai', 0);
         INSERT INTO decks (id, mtgo_id, account_id, name, format, color_identity, created_at,
             updated_at, source)
         VALUES (5, 'q-deck', 1, 'Qburn', 'CMODERN', 'R', 0, 0, 'file');
         INSERT INTO deck_versions (id, deck_id, signature, modified_at)
         VALUES (10, 5, '{registered}', 0);
         INSERT INTO cards (catalog_id, oracle_id, name, type, color_identity, image) VALUES
             (4001, 'o-bolt', 'Qbolt', 'Instant', 'R', 'https://cards.scryfall.io/q/1.jpg'),
             (4002, 'o-goyf', 'Qgoyf', 'Creature', 'G', NULL),
             (4003, 'o-rest', 'Qrest', 'Enchantment', 'W', NULL),
             (4004, 'o-mountain', 'Qmountain', 'Basic Land', NULL, NULL),
             (5001, 'o-seize', 'Qseize', 'Sorcery', 'B', 'https://cards.scryfall.io/q/5.jpg');
         INSERT INTO matches (id, token, account_id, opponent_id, source, format, match_type,
             state, outcome, started_at, ended_at, last_activity_at, revision, derived_version)
         VALUES (1, 'q-manual', 1, 1, 'manual', 'CMODERN', 'Constructed', 'complete', 'win',
             1000, 7000, 7000, 3, 0);
         INSERT INTO games (match_id, game_number, result, end_reason, on_play, local_mulligans,
             opp_mulligans, turn_count, started_at, ended_at)
         VALUES (1, 1, 'win', 'unknown', 1, 1, 0, 7, 1000, 4000),
                (1, 2, 'win', 'unknown', 0, 0, 0, NULL, 4000, 7000);
         UPDATE matches SET deck_version_id = 10 WHERE token = 'q-manual';"
    ))
    .expect("a manual match");
    let count = |catalog_id, quantity| CardCount {
        catalog_id,
        quantity,
    };
    let recorded = [
        ManualGameDetail {
            game_number: 1,
            starting_hand: Some(vec![4001, 4001, 4002, 4004, 4004, 4004]),
            bottomed: vec![4002],
            mulligans: vec![vec![4004; 7]],
            postboard_main: None,
            reveals: vec![count(5001, 2), count(5002, 1)],
        },
        ManualGameDetail {
            game_number: 2,
            starting_hand: None,
            bottomed: Vec::new(),
            mulligans: Vec::new(),
            postboard_main: Some(vec![
                count(4001, 4),
                count(4002, 2),
                count(4003, 2),
                count(4004, 20),
            ]),
            reveals: Vec::new(),
        },
    ];
    // As the manual edits store them: each game's rows.
    let tx = conn.transaction().expect("tx");
    for detail in &recorded {
        let game_id: i64 = tx
            .query_row(
                "SELECT id FROM games WHERE match_id = 1 AND game_number = ?1",
                [detail.game_number],
                |row| row.get(0),
            )
            .expect("game");
        store::game_sync::write_game_cards(&tx, game_id, &detail.game_cards()).expect("rows");
    }
    tx.commit().expect("commit");
    (dir, conn)
}

fn option(catalog_id: u32, name: &str, quantity: u32) -> DeckCardOptionView {
    DeckCardOptionView {
        catalog_id,
        name: name.to_string(),
        image: (catalog_id == 4001).then(|| "https://cards.scryfall.io/q/1.jpg".to_string()),
        local_image: None,
        quantity,
    }
}

fn hand_names(hand: &[HandCardView]) -> Vec<(&str, bool)> {
    hand.iter()
        .map(|card| (card.name.as_str(), card.bottomed))
        .collect()
}

/// The list marks a manual match (0.x `ManualMatchBadge`), and only it.
#[test]
fn a_manual_match_is_marked_manual_in_the_list() {
    let (_dir, conn) = manual_match("matches-list-manual");
    conn.execute_batch(
        "INSERT INTO matches (id, token, account_id, opponent_id, source, format, match_type,
             state, outcome, started_at, ended_at, last_activity_at, revision, derived_version)
         VALUES (2, 'q-log', 1, 1, 'log', 'CMODERN', 'Constructed', 'complete', 'loss',
             9000, 9500, 9500, 1, 4);",
    )
    .expect("a log match");
    let filter = store::match_list::MatchListFilter::newest_first(decks::stats::Range::all_time());
    let found = store::match_list::match_list(
        &conn,
        store::match_list::Scope::All(store::Platform::Mtgo),
        &filter,
        1,
        25,
    )
    .expect("list");
    let page = page_view(found);
    let marks: Vec<(&str, bool)> = page
        .rows
        .iter()
        .map(|row| (row.token.as_str(), row.manual))
        .collect();
    assert_eq!(marks, vec![("q-log", false), ("q-manual", true)]);
}

/// An Endstep ranked result shows on the list row and the match page, and
/// only when the before, after and change are all stored.
#[test]
fn an_endstep_match_row_has_its_rating() {
    let (_dir, conn) = manual_match("matches-list-rating");
    let rating_of = |conn: &Connection| {
        let filter =
            store::match_list::MatchListFilter::newest_first(decks::stats::Range::all_time());
        let found = store::match_list::match_list(
            conn,
            store::match_list::Scope::All(store::Platform::Mtgo),
            &filter,
            1,
            25,
        )
        .expect("list");
        page_view(found).rows[0].rating
    };
    assert_eq!(rating_of(&conn), None);
    assert_eq!(shown(&conn, "q-manual").rating, None);

    conn.execute(
        "INSERT INTO endstep_ranked_results (match_id, rating_before, rating_after, delta)
         VALUES (1, 1371, 1336, -18)",
        [],
    )
    .expect("a ranked result");
    let expected = crate::summary::RatingView {
        before: 1371,
        after: 1336,
        delta: -35,
    };
    // The change is the rating's, after minus before; the stored delta (-18)
    // is Endstep's score change, another scale (final review I3).
    assert_eq!(rating_of(&conn), Some(expected));
    assert_eq!(shown(&conn, "q-manual").rating, Some(expected));

    conn.execute("UPDATE endstep_ranked_results SET delta = NULL", [])
        .expect("no score change");
    assert_eq!(rating_of(&conn), Some(expected));
    assert_eq!(shown(&conn, "q-manual").rating, Some(expected));

    conn.execute("UPDATE endstep_ranked_results SET rating_after = NULL", [])
        .expect("an unknown rating");
    assert_eq!(rating_of(&conn), None);
    assert_eq!(shown(&conn, "q-manual").rating, None);
}

/// 0.x `BuildMatchShowProps` for a manual match: the recorded hand, the
/// sideboard changes from the postboard list, the reveals, and the options
/// of the edit dialogs (`BuildManualEditingData`).
#[test]
fn a_manual_match_shows_its_recorded_hands_sideboard_and_reveals() {
    let (_dir, conn) = manual_match("matches-detail-manual");
    let detail = shown(&conn, "q-manual");
    assert!(detail.manual);
    assert!(!detail.imported);
    assert_eq!((detail.wins, detail.losses), (2, 0));
    assert_eq!(detail.opponent.as_deref(), Some("Qkai"));

    let first = game(&detail, 1);
    // The opening hand in hand order (spec section 17): the bottomed copy
    // follows its card's kept copies.
    assert_eq!(
        hand_names(&first.opening_hand),
        vec![
            ("Qbolt", false),
            ("Qbolt", false),
            ("Qgoyf", false),
            ("Qgoyf", true),
            ("Qmountain", false),
            ("Qmountain", false),
            ("Qmountain", false),
        ]
    );
    assert_eq!(first.mulliganed_hands.len(), 1);
    assert_eq!(
        hand_names(&first.mulliganed_hands[0]),
        vec![("Qmountain", false); 7]
    );
    assert_eq!((first.local_mulligans, first.opponent_mulligans), (1, 0));
    assert_eq!(first.turns, Some(7));
    assert_eq!(first.on_the_play, Some(true));
    assert!(first.sideboard_changes.is_empty());
    assert!(!first.has_game_log);
    assert_eq!(reveal_ids(first), vec![(5001, 2), (5002, 1)]);
    let seize = &first.opponent_cards_seen[0];
    assert_eq!(
        (
            seize.name.as_str(),
            seize.type_line.as_deref(),
            seize.colors.clone(),
            seize.image.as_deref()
        ),
        (
            "Qseize",
            Some("Sorcery"),
            vec!["B".to_string()],
            Some("https://cards.scryfall.io/q/5.jpg")
        )
    );
    assert_eq!(first.opponent_cards_seen[1].name, "Card 5002");

    let second = game(&detail, 2);
    assert!(second.opening_hand.is_empty(), "no hand recorded");
    assert!(second.mulliganed_hands.is_empty());
    assert_eq!(second.turns, None);
    assert!(second.opponent_cards_seen.is_empty());
    let changes: Vec<(u32, u32, SideboardKind, &str)> = second
        .sideboard_changes
        .iter()
        .map(|change| {
            (
                change.catalog_id,
                change.quantity,
                change.kind,
                change.name.as_str(),
            )
        })
        .collect();
    assert_eq!(
        changes,
        vec![
            (4003, 2, SideboardKind::In, "Qrest"),
            (4002, 2, SideboardKind::Out, "Qgoyf"),
        ]
    );

    let editing = detail.manual_editing.expect("the dialogs' options");
    let registered_mains = vec![
        option(4001, "Qbolt", 4),
        option(4002, "Qgoyf", 4),
        option(4004, "Qmountain", 20),
    ];
    assert_eq!(editing.mains, registered_mains);
    assert_eq!(editing.sideboard, vec![option(4003, "Qrest", 3)]);
    assert_eq!(
        editing.games,
        vec![
            ManualGameEditingView {
                number: 1,
                mains: registered_mains.clone(),
            },
            ManualGameEditingView {
                number: 2,
                mains: vec![
                    option(4001, "Qbolt", 4),
                    option(4002, "Qgoyf", 2),
                    option(4004, "Qmountain", 20),
                    option(4003, "Qrest", 2),
                ],
            },
        ]
    );
    assert!(
        game_log_lines(&conn, "q-manual", 1, &[], UTC)
            .expect("log")
            .is_empty(),
        "no game log"
    );
}

#[test]
fn a_tracked_match_has_no_manual_editing() {
    let (_dir, conn) = tracked("league_join_drop", "matches-detail-not-manual");
    let detail = shown(&conn, &token_of_game(&conn, 945_500_844));
    assert!(!detail.manual);
    assert_eq!(detail.manual_editing, None);
}

// ---- An imported match ---------------------------------------------------------

#[test]
fn an_imported_match_shows_only_its_stored_games() {
    let dir = TestDir::new("matches-detail-imported");
    let mut conn = store::open_writer(&dir.join("mymtgo.sqlite")).expect("open");
    let game = |number: u32, result: &str| ImportedGame {
        mtgo_game_id: Some(i64::from(number) + 700),
        game_number: number,
        result: result.to_string(),
        on_play: Some(number == 1),
        local_mulligans: Some(1),
        opp_mulligans: Some(0),
        local_die_roll: None,
        opp_die_roll: None,
        turn_count: Some(6 + number as i64),
        started_at_ms: Some(10_000 + i64::from(number) * 1_000),
        ended_at_ms: Some(10_500 + i64::from(number) * 1_000),
    };
    import_matches(
        &mut conn,
        &[ImportedMatch {
            token: "q-imported".to_string(),
            mtgo_match_id: Some(5),
            local_username: "Qlocal".to_string(),
            opponent: Some("Qopponent".to_string()),
            format: Some("CPAUPER".to_string()),
            match_type: Some("Constructed".to_string()),
            tournament_event_id: None,
            tournament_round: None,
            state: "complete".to_string(),
            outcome: "win".to_string(),
            started_at_ms: 10_000,
            ended_at_ms: Some(14_000),
            note: Some("Q imported note".to_string()),
            games: vec![game(1, "win"), game(2, "loss"), game(3, "win")],
            manual: None,
            games_won_imported: None,
            games_lost_imported: None,
            deck_linked: false,
            replay_share: None,
            legacy_history: false,
        }],
        1,
    )
    .expect("import");
    let detail = shown(&conn, "q-imported");
    assert!(detail.imported);
    assert_eq!(detail.format.as_deref(), Some("Pauper"));
    assert_eq!((detail.wins, detail.losses), (2, 1));
    assert_eq!(detail.outcome, SummaryOutcome::Win);
    assert_eq!(detail.opponent.as_deref(), Some("Qopponent"));
    assert_eq!(detail.notes.as_deref(), Some("Q imported note"));
    assert_eq!(detail.league_name, None);
    let numbers: Vec<(u32, SummaryOutcome, Option<bool>, Option<u32>)> = detail
        .games
        .iter()
        .map(|game| (game.number, game.result, game.on_the_play, game.turns))
        .collect();
    assert_eq!(
        numbers,
        vec![
            (1, SummaryOutcome::Win, Some(true), Some(7)),
            (2, SummaryOutcome::Loss, Some(false), Some(8)),
            (3, SummaryOutcome::Win, Some(false), Some(9)),
        ]
    );
    for game in &detail.games {
        assert!(game.opening_hand.is_empty());
        assert!(game.opponent_cards_seen.is_empty());
        assert!(game.sideboard_changes.is_empty());
        assert!(!game.has_game_log);
        // No frames: the stored counts.
        assert_eq!((game.local_mulligans, game.opponent_mulligans), (1, 0));
    }
}

// ---- A match with a game log file -----------------------------------------------

const DAT_TOKEN: &str = "6a0f564b-f27a-42c0-acea-464f7929342b";

/// A stored match for the `clean_2_0_win.dat` fixture (no fixture log has
/// its token): its games span the file's games, the local player is the
/// log's first player.
fn dat_match(label: &str) -> (TestDir, Connection, Vec<GameLogEntry>, Vec<String>) {
    let dir = TestDir::new(label);
    let bytes = load_game_log("clean_2_0_win");
    dir.write(
        &format!("mtgo-data/Q/Some/Folder/Match_GameLog_{DAT_TOKEN}.dat"),
        &bytes,
    );
    let entries = parse_game_log(&bytes, UTC).expect("parses").entries;
    let players = detect_players(&entries);
    let mut games = Vec::new();
    for (index, slice) in split_into_games(&entries).iter().enumerate() {
        games.push(GameView {
            mtgo_game_id: Some(800 + index as u64),
            game_number: index as u32 + 1,
            result: GameResult::Win,
            end_reason: EndReason::Win,
            on_play: Some(index == 0),
            local_mulligans: None,
            opp_mulligans: None,
            local_die_roll: None,
            opp_die_roll: None,
            turn_count: None,
            started_at_ms: slice.first().map(|entry| entry.timestamp_ms),
            ended_at_ms: slice.last().map(|entry| entry.timestamp_ms),
        });
    }
    let started = games[0].started_at_ms.expect("start");
    let view = MatchView {
        token: DAT_TOKEN.to_string(),
        mtgo_match_id: Some(1),
        local_username: players[0].clone(),
        opponent: Some(players[1].clone()),
        format: Some("CMODERN".to_string()),
        match_type: Some("Constructed".to_string()),
        tournament_event_id: None,
        tournament_round: None,
        league_token: None,
        game_structure: None,
        state: MatchState::Complete,
        resolution: None,
        outcome: Outcome::Win,
        tentative_outcome: None,
        wins: games.len() as u32,
        losses: 0,
        started_at_ms: started,
        ended_at_ms: games.last().and_then(|game| game.ended_at_ms),
        last_activity_ms: started,
        games,
    };
    let mut conn = store::open_writer(&dir.join("mymtgo.sqlite")).expect("open");
    commit(
        &mut conn,
        &WriteBatch {
            matches: vec![MatchWrite::Upsert {
                view: Box::new(view),
                revision: 1,
            }],
            ..WriteBatch::default()
        },
    )
    .expect("match");
    // The tracker's game sync once the file is found: decoded into
    // `game_logs`, the games' rows read from it.
    store::game_sync::sync_games(
        &mut conn,
        &[DAT_TOKEN.to_string()],
        &[(DAT_TOKEN.to_string(), bytes)],
        UTC,
    )
    .expect("game sync");
    (dir, conn, entries, players)
}

/// The tracker's game sync of `token` (a test that stores a log match
/// without running the tracker).
fn sync(conn: &mut Connection, token: &str) {
    store::game_sync::sync_games(conn, &[token.to_string()], &[], UTC).expect("game sync");
}

#[test]
fn the_game_log_file_is_found_by_token_in_the_data_folder() {
    let (dir, _conn, entries, _players) = dat_match("matches-dat-find");
    let cache = std::sync::Mutex::new(GameLogCache::new());
    let files = GameLogFiles {
        env: Env::current(),
        data_folder: Some(dir.join("mtgo-data")),
    };
    let now = 1_800_000_000_000;
    assert_eq!(dat_entries(&cache, &files, DAT_TOKEN, UTC, now), entries);
    assert!(dat_entries(&cache, &files, "q-other-token", UTC, now).is_empty());
    // Found again from the remembered path, parsed once.
    assert_eq!(dat_entries(&cache, &files, DAT_TOKEN, UTC, now), entries);
    assert_eq!(lock(&cache).parsed_count(), 1);
    let nowhere = GameLogFiles {
        env: Env::current(),
        data_folder: Some(dir.join("missing")),
    };
    assert!(dat_entries(&cache, &nowhere, DAT_TOKEN, UTC, now).is_empty());
}

#[test]
fn an_arena_match_without_stored_messages_skips_the_data_folder_search() {
    let (_dir, conn, _entries, _players) = dat_match("matches-dat-arena-skip");
    assert!(!skips_dat_search(&conn, "q-unknown").expect("unknown"));
    // The fixture's game log is stored, so the MTGO match skips already; clear
    // that to show the platform alone decides.
    conn.execute("DELETE FROM game_logs", []).expect("clear");
    assert!(!skips_dat_search(&conn, DAT_TOKEN).expect("mtgo"));
    conn.execute(
        "INSERT INTO accounts (username, first_seen_at, platform, arena_user_id)
         VALUES ('Qarena#12345', 0, 'arena', 'Qarena-id')",
        [],
    )
    .expect("arena account");
    conn.execute(
        "UPDATE matches SET account_id = (SELECT id FROM accounts WHERE platform = 'arena')
         WHERE token = ?1",
        [DAT_TOKEN],
    )
    .expect("move");
    assert!(skips_dat_search(&conn, DAT_TOKEN).expect("arena"));
}

#[test]
fn an_endstep_match_skips_the_data_folder_search() {
    let (_dir, mut conn, _entries, _players) = dat_match("matches-dat-endstep-skip");
    conn.execute("DELETE FROM game_logs", []).expect("clear");
    crate::endstep_seed::seed_endstep_match(&mut conn, "q-endstep");
    assert!(!skips_dat_search(&conn, DAT_TOKEN).expect("mtgo"));
    assert!(skips_dat_search(&conn, "q-endstep").expect("endstep"));
}

// Endstep plan 3: an Endstep game's log is read from the match's stored
// frames, one game at a time.
#[test]
fn endstep_game_log_lines_come_from_the_frames() {
    let dir = TestDir::new("matches-endstep-game-log");
    let path = dir.join("mymtgo.sqlite");
    crate::endstep_seed::sync_endstep_fixture(&path, &dir, 1_794_400_000_000);
    let conn = store::open_reader(&path).expect("reader");
    let token = crate::endstep_seed::ENDSTEP_FIXTURE_TOKEN;

    let lines = game_log_lines(&conn, token, 1, &[], UTC).expect("log");
    let messages: Vec<&str> = lines.iter().map(|line| line.message.as_str()).collect();
    assert!(
        messages.contains(&"Player cast Expedition Map"),
        "{messages:?}"
    );
    let game_1 = store::endstep_sync::game_log_lines(&conn, token, 1).expect("game 1");
    let game_2 = store::endstep_sync::game_log_lines(&conn, token, 2).expect("game 2");
    assert_eq!(lines.len(), game_1.len());
    assert!(!game_2.is_empty());
    let last_of_game_1 = game_1.iter().map(|line| line.at_ms).max().expect("lines");
    for line in &game_2 {
        assert!(line.at_ms > last_of_game_1, "{} is game 2's", line.message);
    }
    assert!(
        game_log_lines(&conn, token, 9, &[], UTC)
            .expect("no game 9")
            .is_empty()
    );
    // The page offers each game's log.
    let page = detail(&conn, token, &[])
        .expect("detail")
        .expect("the match");
    assert_eq!(page.games.len(), 3);
    assert!(page.games.iter().all(|game| game.has_game_log));
}

// Endstep plan 4: an Endstep game's replay is built from the stored frames
// when it opens, so the page offers it on every tracked game without
// walking the match; a placeholder game (never seen) has no start.
#[test]
fn endstep_games_with_frames_show_replay() {
    let dir = TestDir::new("matches-endstep-replay");
    let path = dir.join("mymtgo.sqlite");
    crate::endstep_seed::sync_endstep_fixture(&path, &dir, 1_794_400_000_000);
    let token = crate::endstep_seed::ENDSTEP_FIXTURE_TOKEN;
    let replayable = |conn: &Connection| -> Vec<(u32, bool)> {
        detail(conn, token, &[])
            .expect("detail")
            .expect("the match")
            .games
            .iter()
            .map(|game| (game.number, game.has_frames))
            .collect()
    };
    let conn = store::open_reader(&path).expect("reader");
    assert_eq!(replayable(&conn), vec![(1, true), (2, true), (3, true)]);

    store::open_writer(&path)
        .expect("writer")
        .execute(
            "UPDATE games SET started_at = NULL WHERE game_number = 2",
            [],
        )
        .expect("game 2 as a placeholder");
    assert_eq!(replayable(&conn), vec![(1, true), (2, false), (3, true)]);
}

// #707: the page and Game Stats (`games.local_mulligans`) show the same
// count. A stored count above the hands the frames kept wins; the hands shown
// stay the ones kept.
#[test]
fn the_mulligan_count_is_the_larger_of_the_hands_and_the_stored_count() {
    let dir = TestDir::new("matches-endstep-mulligan-count");
    let path = dir.join("mymtgo.sqlite");
    crate::endstep_seed::sync_endstep_fixture(&path, &dir, 1_794_400_000_000);
    let token = crate::endstep_seed::ENDSTEP_FIXTURE_TOKEN;
    let conn = store::open_reader(&path).expect("reader");
    let third = |conn: &Connection| -> (u32, usize) {
        let detail = shown(conn, token);
        let third = game(&detail, 3);
        (third.local_mulligans, third.mulliganed_hands.len())
    };
    assert_eq!(third(&conn), (1, 1));

    store::open_writer(&path)
        .expect("writer")
        .execute(
            "UPDATE games SET local_mulligans = 2 WHERE game_number = 3",
            [],
        )
        .expect("a mulligan the frames missed");
    assert_eq!(third(&conn), (2, 1));

    store::open_writer(&path)
        .expect("writer")
        .execute(
            "UPDATE games SET local_mulligans = 0 WHERE game_number = 3",
            [],
        )
        .expect("a stored count below the hands");
    assert_eq!(third(&conn), (1, 1));
}

// Endstep plan 4: the Game log button needs frames to read, so a placeholder
// game (never seen, no start) has none.
#[test]
fn an_endstep_placeholder_game_has_no_game_log() {
    let dir = TestDir::new("matches-endstep-placeholder-log");
    let path = dir.join("mymtgo.sqlite");
    crate::endstep_seed::sync_endstep_fixture(&path, &dir, 1_794_400_000_000);
    let token = crate::endstep_seed::ENDSTEP_FIXTURE_TOKEN;
    let logs = |conn: &Connection| -> Vec<(u32, bool)> {
        detail(conn, token, &[])
            .expect("detail")
            .expect("the match")
            .games
            .iter()
            .map(|game| (game.number, game.has_game_log))
            .collect()
    };
    let conn = store::open_reader(&path).expect("reader");
    assert_eq!(logs(&conn), vec![(1, true), (2, true), (3, true)]);

    store::open_writer(&path)
        .expect("writer")
        .execute(
            "UPDATE games SET started_at = NULL WHERE game_number = 2",
            [],
        )
        .expect("game 2 as a placeholder");
    assert_eq!(logs(&conn), vec![(1, true), (2, false), (3, true)]);
}

#[test]
fn the_game_log_file_gives_turns_log_only_reveals_and_log_lines() {
    let (_dir, conn, entries, players) = dat_match("matches-dat-detail");
    let detail = detail(&conn, DAT_TOKEN, &entries)
        .expect("detail")
        .expect("listed");
    let cards = extract_log_cards(&entries);
    for (index, game) in detail.games.iter().enumerate() {
        assert_eq!(
            game.turns, cards.games[index].meta.turn_count,
            "game {}",
            game.number
        );
        assert!(game.has_game_log);
        // No frames: every reveal comes from the log, under the log's name.
        let logged: Vec<(u32, String)> = cards
            .game_cards(index, &players[1])
            .iter()
            .map(|card| (card.mtgo_id, card.name.clone()))
            .collect();
        let shown: Vec<(u32, String)> = game
            .opponent_cards_seen
            .iter()
            .map(|card| (card.catalog_id, card.name.clone()))
            .collect();
        assert!(!logged.is_empty());
        let mut unique: Vec<(u32, String)> = Vec::new();
        for card in logged {
            let face = card.1.to_lowercase();
            if !unique.iter().any(|kept| kept.1.to_lowercase() == face) {
                unique.push(card);
            }
        }
        // Per-card rows: the reveals read in catalog order.
        unique.sort();
        assert_eq!(shown, unique, "game {}", game.number);
    }

    let lines = game_log_lines(&conn, DAT_TOKEN, 1, &entries, UTC).expect("log");
    assert!(!lines.is_empty());
    assert!(lines.iter().all(|line| {
        line.time.len() == 8 && line.time.as_bytes()[2] == b':' && line.time.as_bytes()[5] == b':'
    }));
    assert!(lines.iter().all(|line| !line.message.contains("@P")));
    let first = &entries[0];
    let expected = UTC
        .timestamp_millis_opt(first.timestamp_ms)
        .single()
        .expect("time")
        .format("%H:%M:%S")
        .to_string();
    assert_eq!(lines[0].time, expected);
    // The OS timezone shows local wall-clock times.
    let london =
        game_log_lines(&conn, DAT_TOKEN, 1, &entries, chrono_tz::Europe::London).expect("log");
    assert_eq!(london.len(), lines.len());
    assert!(
        game_log_lines(&conn, DAT_TOKEN, 9, &entries, UTC)
            .expect("log")
            .is_empty()
    );
}

// ---- A long synthetic game -------------------------------------------------------

/// One state frame: both players, `lands` lands on the local battlefield,
/// seven cards in hand.
fn frame_text(game_id: u64, lands: u32, turn: u32) -> String {
    let mut cards = Vec::new();
    for hand in 0..7 {
        cards.push(format!(
            "{{\"Id\":{},\"CatalogID\":{},\"Zone\":\"Hand\",\"ActualZone\":\"Hand\",\"Owner\":1,\"Controller\":1}}",
            100 + hand,
            5_000 + hand
        ));
    }
    for land in 0..lands {
        cards.push(format!(
            "{{\"Id\":{},\"CatalogID\":4000,\"Zone\":\"Battlefield\",\"ActualZone\":\"Battlefield\",\"Owner\":1,\"Controller\":1}}",
            200 + land
        ));
    }
    for filler in 0..40 {
        cards.push(format!(
            "{{\"Id\":{},\"CatalogID\":6000,\"Zone\":\"Library\",\"ActualZone\":\"Library\",\"Owner\":2,\"Controller\":2,\"Turn\":{turn}}}",
            1_000 + filler
        ));
    }
    format!(
        "12:00:00 [INF] (Twitch Info|Game Play Status Update for Game ID: {game_id}, Match ID: 99, Event ID: 99) {{\"Players\":[{{\"Id\":1,\"Name\":\"Qlocal\",\"LibraryCount\":53,\"HandCount\":7,\"Life\":20}},{{\"Id\":2,\"Name\":\"Qopponent\",\"LibraryCount\":53,\"HandCount\":7,\"Life\":20}}],\"Cards\":[{}]}}",
        cards.join(",")
    )
}

/// A one-game match whose game has `frames` state frames (every other one a
/// repeat), ending with `lands` lands in play. No turn count is stored.
fn long_match(label: &str, frames: u32, lands: u32) -> (TestDir, Connection) {
    let dir = TestDir::new(label);
    let mut conn = store::open_writer(&dir.join("mymtgo.sqlite")).expect("open");
    let file = insert_log_file(
        &conn,
        &NewLogFile {
            path: "Q/mtgo.log".to_string(),
            identity_hash: 1,
            head_hash: 2,
            head_len: 10,
            created_at_fs: None,
            day_anchor: NaiveDate::from_ymd_opt(2026, 10, 7).expect("date"),
            timezone: "UTC".to_string(),
            inherited_username: Some("Qlocal".to_string()),
            seen_at_ms: 1,
        },
    )
    .expect("log file");
    let mut events = Vec::new();
    for index in 0..frames {
        let progress = index / 2;
        let built = (progress * lands) / (frames / 2).max(1);
        events.push(NewEvent {
            log_file_id: file,
            byte_offset: u64::from(index) * 10_000,
            kind: Some(EventKind::GameStateUpdate),
            subjects: Subjects {
                match_id: Some(99),
                game_id: Some(901),
                ..Subjects::default()
            },
            logged_at_ms: 1_000_000 + i64::from(index),
            raw: frame_text(901, built.min(lands), progress),
            quarantined: false,
        });
    }
    // The last frame has every land.
    if let Some(last) = events.last_mut() {
        last.raw = frame_text(901, lands, frames);
    }
    let view = MatchView {
        token: "q-long".to_string(),
        mtgo_match_id: Some(99),
        local_username: "Qlocal".to_string(),
        opponent: Some("Qopponent".to_string()),
        format: Some("CMODERN".to_string()),
        match_type: Some("Constructed".to_string()),
        tournament_event_id: None,
        tournament_round: None,
        league_token: None,
        game_structure: None,
        state: MatchState::Complete,
        resolution: None,
        outcome: Outcome::Win,
        tentative_outcome: None,
        wins: 1,
        losses: 0,
        started_at_ms: 1_000_000,
        ended_at_ms: Some(1_000_000 + i64::from(frames)),
        last_activity_ms: 1_000_000,
        games: vec![GameView {
            mtgo_game_id: Some(901),
            game_number: 1,
            result: GameResult::Win,
            end_reason: EndReason::Win,
            on_play: Some(true),
            local_mulligans: Some(0),
            opp_mulligans: Some(0),
            local_die_roll: None,
            opp_die_roll: None,
            turn_count: None,
            started_at_ms: Some(1_000_000),
            ended_at_ms: Some(1_000_000 + i64::from(frames)),
        }],
    };
    commit(
        &mut conn,
        &WriteBatch {
            events,
            matches: vec![MatchWrite::Upsert {
                view: Box::new(view),
                revision: 1,
            }],
            ..WriteBatch::default()
        },
    )
    .expect("commit");
    conn.execute_batch(
        "INSERT INTO cards (catalog_id, name, type) VALUES (4000, 'Qisland', 'Basic Land - Island');",
    )
    .expect("land");
    sync(&mut conn, "q-long");
    (dir, conn)
}

#[test]
fn without_a_turn_count_the_lands_in_play_estimate_the_turns() {
    let (_dir, conn) = long_match("matches-detail-estimate", 20, 6);
    let detail = shown(&conn, "q-long");
    assert_eq!(game(&detail, 1).turns, Some(6));
    assert_eq!(game(&detail, 1).opening_hand.len(), 7);
}

/// Run with `cargo test -p app --release -- --ignored --nocapture`.
#[test]
#[ignore = "timing check, run in release"]
fn a_thousand_frame_game_is_read_within_the_page_budget() {
    let (_dir, conn) = long_match("matches-detail-timing", 1_000, 12);
    let mut best = std::time::Duration::MAX;
    for _ in 0..5 {
        let started = std::time::Instant::now();
        let detail = shown(&conn, "q-long");
        best = best.min(started.elapsed());
        assert_eq!(game(&detail, 1).turns, Some(12));
    }
    println!("match_detail over 1,000 frames: best of 5 = {best:?}");
    assert!(best < std::time::Duration::from_millis(100), "{best:?}");
}

#[test]
fn a_long_game_is_read_correctly_in_debug_builds_too() {
    let (_dir, conn) = long_match("matches-detail-long", 400, 9);
    let detail = shown(&conn, "q-long");
    assert_eq!(game(&detail, 1).turns, Some(9));
}

// ---- Lists ---------------------------------------------------------------------

#[test]
fn the_ui_sends_list_queries_in_camel_case_with_0x_sort_names() {
    let json = serde_json::json!({
        "from": "2026-09-07",
        "to": null,
        "result": "loss",
        "kind": "league",
        "sort": "game_2",
        "sortDir": "asc",
        "page": 3,
    });
    let query: MatchListQuery = serde_json::from_value(json).expect("parses");
    assert_eq!(query.sort, MatchSortKey::Game2);
    assert_eq!(query.sort_dir, MatchSortDir::Asc);
    assert_eq!(query.result, Some(MatchResultFilter::Loss));
    assert_eq!(query.kind, Some(MatchKindFilter::League));
    assert_eq!(query.page, 3);
    assert!(!query.is_unfiltered());
    assert!(MatchListQuery::newest_first(2).is_unfiltered());
    let blank = MatchListQuery {
        from: Some(" ".to_string()),
        ..MatchListQuery::newest_first(1)
    };
    assert!(blank.is_unfiltered());
}

#[test]
fn list_filters_map_to_the_store_and_days_are_local() {
    let la = chrono_tz::America::Los_Angeles;
    let query = MatchListQuery {
        from: Some("2026-09-07".to_string()),
        to: Some("2026-09-07".to_string()),
        result: Some(MatchResultFilter::Win),
        kind: Some(MatchKindFilter::Casual),
        best_of: Some(BestOfView::Bo1),
        archetype: None,
        sort: MatchSortKey::Duration,
        sort_dir: MatchSortDir::Asc,
        page: 1,
    };
    let filter = match_filter(&query, Range::all_time(), la).expect("filter");
    // Local midnight in Los Angeles (PDT, UTC-7) to the day's last millisecond.
    let from = chrono::Utc
        .with_ymd_and_hms(2026, 9, 7, 7, 0, 0)
        .single()
        .expect("time")
        .timestamp_millis();
    assert_eq!(
        filter.range,
        Range {
            from_ms: from,
            to_ms: from + 24 * 3_600_000 - 1,
        }
    );
    assert_eq!(filter.result, Some(ResultFilter::Win));
    assert_eq!(filter.kind, Some(KindFilter::Casual));
    assert_eq!(filter.best_of, Some(store::BestOf::One));
    assert_eq!((filter.sort, filter.dir), (SortKey::Duration, SortDir::Asc));

    // Inside a deck page's timeframe: the narrower bound wins on each side.
    let base = Range {
        from_ms: from + 3_600_000,
        to_ms: i64::MAX,
    };
    let narrowed = match_filter(&query, base, la).expect("filter");
    assert_eq!(narrowed.range.from_ms, from + 3_600_000);
    assert_eq!(narrowed.range.to_ms, from + 24 * 3_600_000 - 1);

    let open =
        match_filter(&MatchListQuery::newest_first(1), Range::all_time(), la).expect("filter");
    assert_eq!(open, MatchListFilter::newest_first(Range::all_time()));

    let bad = MatchListQuery {
        to: Some("07/09/2026".to_string()),
        ..MatchListQuery::newest_first(1)
    };
    assert_eq!(
        match_filter(&bad, Range::all_time(), la),
        Err(CommandError::Refused(
            "The to date must be a date like 2026-09-07.".to_string()
        ))
    );
}

#[test]
fn every_kind_filter_maps_to_the_store() {
    let kinds = [
        (MatchKindFilter::League, KindFilter::League),
        (MatchKindFilter::Casual, KindFilter::Casual),
        (MatchKindFilter::Manual, KindFilter::Manual),
        (MatchKindFilter::Ranked, KindFilter::Ranked),
        (MatchKindFilter::Unranked, KindFilter::Unranked),
    ];
    for (asked, stored) in kinds {
        let query = MatchListQuery {
            kind: Some(asked),
            ..MatchListQuery::newest_first(1)
        };
        let filter = match_filter(&query, Range::all_time(), chrono_tz::UTC).expect("filter");
        assert_eq!(filter.kind, Some(stored), "{asked:?}");
    }
}

/// 0.x `MatchesDateFilterTest`: `filter_from` and `filter_to` are local
/// calendar days in the system timezone.
#[test]
fn date_filters_are_local_calendar_days_in_the_system_timezone() {
    let dir = TestDir::new("matches-date-filter");
    let conn = store::open_writer(&dir.join("mymtgo.sqlite")).expect("open");
    conn.execute_batch(
        "INSERT INTO accounts (id, username, first_seen_at) VALUES (1, 'Qlocal', 0);
         INSERT INTO decks (id, mtgo_id, account_id, name, format, created_at, updated_at, source)
             VALUES (5, 'q-deck', 1, 'Qburn', 'CMODERN', 0, 0, 'file');
         INSERT INTO deck_versions (id, deck_id, signature, modified_at) VALUES (10, 5, 'SIG', 0);",
    )
    .expect("deck");
    let utc = |text: &str| {
        chrono::NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M:%S")
            .expect("time")
            .and_utc()
            .timestamp_millis()
    };
    let starts = [
        // 22:00 PDT on 6 Sep: outside a local 7 Sep filter.
        ("q-1", utc("2026-09-07 05:00:00"), "win"),
        // 01:00 PDT on 7 Sep: inside.
        ("q-2", utc("2026-09-07 08:00:00"), "win"),
        // 22:00 PDT on 7 Sep: inside.
        ("q-3", utc("2026-09-08 05:00:00"), "loss"),
        // 23:30 PDT on 7 Sep: inside.
        ("q-4", utc("2026-09-08 06:30:00"), "loss"),
    ];
    for (token, started, outcome) in starts {
        conn.execute(
            "INSERT INTO matches (token, account_id, source, state, outcome, started_at,
                 last_activity_at, revision, derived_version)
             VALUES (?1, 1, 'log', 'complete', ?2, ?3, ?3, 1, 1)",
            rusqlite_params(token, outcome, started),
        )
        .expect("match");
        conn.execute(
            "UPDATE matches SET deck_version_id = 10 WHERE token = ?1",
            [token],
        )
        .expect("link");
    }
    let query = MatchListQuery {
        from: Some("2026-09-07".to_string()),
        to: Some("2026-09-07".to_string()),
        ..MatchListQuery::newest_first(1)
    };
    let now = utc("2026-09-10 12:00:00");
    let la = chrono_tz::America::Los_Angeles;
    let page =
        crate::decks::matches(&conn, 5, None, "alltime", &query, now, la, None).expect("page");
    let tokens: Vec<&str> = page.rows.iter().map(|row| row.token.as_str()).collect();
    assert_eq!(tokens, ["q-4", "q-3", "q-2"]);
    assert_eq!((page.total, page.per_page), (3, 50));

    // The main list reads the same filter.
    let filter = match_filter(&query, Range::all_time(), la).expect("filter");
    let (rows, total) =
        crate::live::read_page(&conn, store::Platform::Mtgo, &filter, 1).expect("main list");
    assert_eq!(total, 3);
    assert_eq!(rows.len(), 3);

    // Sorted by outcome, ascending: losses first, newest first within.
    let sorted = MatchListQuery {
        sort: MatchSortKey::Outcome,
        sort_dir: MatchSortDir::Asc,
        ..MatchListQuery::newest_first(1)
    };
    let page =
        crate::decks::matches(&conn, 5, None, "alltime", &sorted, now, la, None).expect("page");
    let tokens: Vec<&str> = page.rows.iter().map(|row| row.token.as_str()).collect();
    assert_eq!(tokens, ["q-4", "q-3", "q-2", "q-1"]);
}

fn rusqlite_params<'a>(token: &'a str, outcome: &'a str, started: i64) -> (&'a str, &'a str, i64) {
    (token, outcome, started)
}

// ---- Edits through the tracker ----------------------------------------------------

#[test]
fn notes_and_hiding_go_through_the_tracker_and_refusals_reach_the_user() {
    let dir = TestDir::new("matches-edits");
    let (state, _events) = crate::commands::tests::ready_state(&dir);
    assert_eq!(
        hide(&state, "q-none", 5),
        Err(CommandError::Refused(
            "This match is not stored yet, or it no longer exists.".to_string()
        ))
    );
    {
        let mut writer = store::open_writer(&state.db_path).expect("writer");
        commit(
            &mut writer,
            &WriteBatch {
                matches: vec![MatchWrite::Upsert {
                    view: Box::new(crate::summary::testing::view(
                        "q-edit",
                        1_000,
                        MatchState::Complete,
                    )),
                    revision: 1,
                }],
                ..WriteBatch::default()
            },
        )
        .expect("match");
    }
    let too_long = "Q".repeat(5_001);
    assert_eq!(
        set_notes(&state, "q-edit", &too_long),
        Err(CommandError::Refused(
            "The notes field must not be greater than 5000 characters.".to_string()
        ))
    );
    set_notes(&state, "q-edit", "  Q sideboard plan  ").expect("notes");
    let conn = store::open_reader(&state.db_path).expect("reader");
    assert_eq!(
        shown(&conn, "q-edit").notes.as_deref(),
        Some("Q sideboard plan")
    );
    hide(&state, "q-edit", 6).expect("hidden");
    assert_eq!(detail(&conn, "q-edit", &[]).expect("detail"), None);
    lock(&state.tracker).take().expect("tracker").stop();
}

#[test]
fn a_deleted_match_is_listed_and_restored_through_the_tracker() {
    let dir = TestDir::new("matches-restore");
    let (state, _events) = crate::commands::tests::ready_state(&dir);
    assert_eq!(
        restore(&state, "q-none", 5),
        Err(CommandError::Refused(
            "This match is not stored yet, or it no longer exists.".to_string()
        ))
    );
    {
        let mut writer = store::open_writer(&state.db_path).expect("writer");
        commit(
            &mut writer,
            &WriteBatch {
                matches: vec![MatchWrite::Upsert {
                    view: Box::new(crate::summary::testing::view(
                        "q-restore",
                        1_000,
                        MatchState::Complete,
                    )),
                    revision: 1,
                }],
                ..WriteBatch::default()
            },
        )
        .expect("match");
    }
    let conn = store::open_reader(&state.db_path).expect("reader");
    assert!(deleted(&conn).expect("deleted").matches.is_empty());

    hide(&state, "q-restore", 6).expect("deleted");
    let listed = deleted(&conn).expect("deleted");
    assert_eq!(listed.matches.len(), 1);
    assert_eq!(listed.matches[0].token, "q-restore");
    assert_eq!(listed.matches[0].hidden_at_ms, 6);
    let json = serde_json::to_value(&listed).expect("json");
    assert_eq!(json["matches"][0]["hiddenAtMs"], 6);
    assert_eq!(json["matches"][0]["token"], "q-restore");
    assert_eq!(detail(&conn, "q-restore", &[]).expect("detail"), None);

    restore(&state, "q-restore", 7).expect("restored");
    assert!(detail(&conn, "q-restore", &[]).expect("detail").is_some());
    assert!(deleted(&conn).expect("deleted").matches.is_empty());
    lock(&state.tracker).take().expect("tracker").stop();
}

#[test]
fn setting_a_matchs_deck_goes_through_the_tracker_and_refusals_reach_the_user() {
    let dir = TestDir::new("matches-set-deck");
    let (state, _events) = crate::commands::tests::ready_state(&dir);
    assert_eq!(
        set_deck(&state, &[], 1),
        Err(CommandError::Refused(
            "Choose at least one match.".to_string()
        ))
    );
    let version: i64 = {
        let mut writer = store::open_writer(&state.db_path).expect("writer");
        commit(
            &mut writer,
            &WriteBatch {
                matches: vec![MatchWrite::Upsert {
                    view: Box::new(crate::summary::testing::view(
                        "q-deck-edit",
                        1_000,
                        MatchState::Complete,
                    )),
                    revision: 1,
                }],
                ..WriteBatch::default()
            },
        )
        .expect("match");
        writer
            .execute(
                "INSERT INTO decks (id, mtgo_id, name, format, created_at, updated_at, source)
                 VALUES (5, 'q-deck', 'Qdeck', 'CMODERN', 0, 0, 'file')",
                [],
            )
            .expect("deck");
        writer
            .query_row(
                "INSERT INTO deck_versions (deck_id, signature, modified_at)
                 VALUES (5, 'Q', 0) RETURNING id",
                [],
                |row| row.get(0),
            )
            .expect("version")
    };
    let version = u32::try_from(version).expect("small id");
    assert_eq!(
        set_deck(&state, &["q-deck-edit".to_string()], version + 100),
        Err(CommandError::Refused(
            "That deck version no longer exists. Choose another one.".to_string()
        ))
    );
    set_deck(&state, &["q-deck-edit".to_string()], version).expect("set");
    let conn = store::open_reader(&state.db_path).expect("reader");
    assert_eq!(
        store::decks::link_of(&conn, "q-deck-edit").expect("link"),
        Some(i64::from(version))
    );
    lock(&state.tracker).take().expect("tracker").stop();
}

// ---- An imported match with its 0.x detail (Plan O) ---------------------------

use store::imported_detail::{
    DetailCard, DetailLogEntry, ImportedDetail, ImportedGameDetail, import_detail,
};

/// An imported game numbered `number`: Game ID `700 + number` unless
/// `mtgo_game_id` says otherwise, played from second `number * 100`.
fn imported_game(number: u32, mtgo_game_id: Option<i64>, turn_count: Option<i64>) -> ImportedGame {
    ImportedGame {
        mtgo_game_id,
        game_number: number,
        result: if number == 2 { "loss" } else { "win" }.to_string(),
        on_play: Some(number == 1),
        local_mulligans: Some(1),
        opp_mulligans: Some(1),
        local_die_roll: None,
        opp_die_roll: None,
        turn_count,
        started_at_ms: Some(i64::from(number) * 100_000),
        ended_at_ms: Some(i64::from(number) * 100_000 + 50_000),
    }
}

/// A 0.x import of `token` between Qlocal and Qopponent with `games`.
fn import_0x(conn: &mut Connection, token: &str, games: Vec<ImportedGame>) {
    import_matches(
        conn,
        &[ImportedMatch {
            token: token.to_string(),
            mtgo_match_id: Some(5),
            local_username: "Qlocal".to_string(),
            opponent: Some("Qopponent".to_string()),
            format: Some("CMODERN".to_string()),
            match_type: Some("Constructed".to_string()),
            tournament_event_id: None,
            tournament_round: None,
            state: "complete".to_string(),
            outcome: "win".to_string(),
            started_at_ms: 100_000,
            ended_at_ms: Some(400_000),
            note: None,
            games,
            manual: None,
            games_won_imported: None,
            games_lost_imported: None,
            deck_linked: false,
            replay_share: None,
            legacy_history: false,
        }],
        1,
    )
    .expect("import");
}

fn detail_card(mtgo_id: u32, quantity: u32, sideboard: bool) -> DetailCard {
    DetailCard {
        mtgo_id,
        quantity,
        sideboard,
    }
}

/// A frame as the 0.x import stores it: the JSON without the log header.
fn bare_frame(lands: u32, turn: u32) -> FrameText {
    let text = frame_text(0, lands, turn);
    let open = text.find('{').expect("json");
    FrameText {
        time: format!("10:00:{turn:02}"),
        text: text[open..].to_string(),
    }
}

/// Three games: game 1 with frames (hand, lands), a deck list and a card
/// 0.x saw; game 2 with only deck lists (no Game ID in the detail, matched
/// by number); game 3 with nothing. One log line in game 1.
fn imported_with_detail(label: &str) -> (TestDir, Connection) {
    let dir = TestDir::new(label);
    let mut conn = store::open_writer(&dir.join("mymtgo.sqlite")).expect("open");
    import_0x(
        &mut conn,
        "q-imported-detail",
        vec![
            imported_game(1, Some(701), None),
            imported_game(2, Some(702), Some(11)),
            imported_game(3, Some(703), Some(12)),
        ],
    );
    let main = vec![detail_card(4000, 20, false), detail_card(4100, 40, false)];
    let boarded = vec![
        detail_card(4000, 20, false),
        detail_card(4100, 37, false),
        detail_card(4200, 3, false),
        detail_card(4100, 3, true),
    ];
    let detail = ImportedDetail {
        games: vec![
            ImportedGameDetail {
                mtgo_game_id: Some(701),
                game_number: 1,
                local_deck: main.clone(),
                opponent_cards: vec![detail_card(7001, 1, false), detail_card(6000, 2, false)],
                frames: vec![bare_frame(0, 0), bare_frame(3, 3), bare_frame(5, 5)],
                ..ImportedGameDetail::default()
            },
            ImportedGameDetail {
                mtgo_game_id: None,
                game_number: 2,
                local_deck: boarded,
                opponent_cards: vec![
                    detail_card(7002, 1, false),
                    detail_card(7002, 1, true),
                    detail_card(7003, 1, false),
                ],
                frames: Vec::new(),
                ..ImportedGameDetail::default()
            },
            ImportedGameDetail {
                mtgo_game_id: Some(703),
                game_number: 3,
                ..ImportedGameDetail::default()
            },
        ],
        log: vec![
            DetailLogEntry {
                at_ms: 101_000,
                message: "@P@PQlocal plays @[Qisland@:4000,1:@].".to_string(),
            },
            DetailLogEntry {
                at_ms: 900_000,
                message: "Long after every game.".to_string(),
            },
        ],
    };
    import_detail(&mut conn, "q-imported-detail", &detail).expect("detail");
    link_version(
        &conn,
        "q-imported-detail",
        &[
            DeckCard {
                catalog_id: 4000,
                quantity: 20,
                sideboard: false,
            },
            DeckCard {
                catalog_id: 4100,
                quantity: 40,
                sideboard: false,
            },
        ],
    );
    conn.execute_batch(
        "INSERT OR REPLACE INTO cards (catalog_id, name, type) VALUES
             (4000, 'Qisland', 'Basic Land - Island'),
             (4100, 'Qbolt', 'Instant'),
             (4200, 'Qblocker', 'Creature - Wall'),
             (7001, 'Qdragon', 'Creature - Dragon');",
    )
    .expect("cards");
    (dir, conn)
}

#[test]
fn an_imported_match_with_frames_shows_its_opening_hand_turns_and_reveals() {
    let (_dir, conn) = imported_with_detail("matches-imported-frames");
    let detail = shown(&conn, "q-imported-detail");
    assert!(detail.imported);
    let first = game(&detail, 1);
    assert_eq!(first.opening_hand.len(), 7, "the hand from the 0.x frames");
    assert!(first.mulliganed_hands.is_empty());
    assert_eq!(
        (first.local_mulligans, first.opponent_mulligans),
        (0, 0),
        "the frames' mulligans, not the stored counts"
    );
    assert_eq!(first.turns, Some(5), "estimated from the lands in play");
    // 0.x's list merged with the frame's own, each with the larger count
    // (per-card rows: catalog order).
    assert_eq!(reveal_ids(first), vec![(6000, 40), (7001, 1)]);
    assert_eq!(first.opponent_cards_seen[1].name, "Qdragon");
    assert!(first.has_deck_list);
    assert!(first.sideboard_changes.is_empty(), "never for game 1");
}

#[test]
fn an_imported_game_with_only_deck_lists_shows_sideboard_changes_and_reveals() {
    let (_dir, conn) = imported_with_detail("matches-imported-decks");
    let detail = shown(&conn, "q-imported-detail");
    let second = game(&detail, 2);
    assert!(second.opening_hand.is_empty(), "no frames, no hand");
    assert_eq!(
        (second.local_mulligans, second.opponent_mulligans),
        (1, 1),
        "the stored counts"
    );
    assert_eq!(second.turns, Some(11));
    assert!(second.has_deck_list);
    let changes: Vec<(u32, SideboardKind, u32)> = second
        .sideboard_changes
        .iter()
        .map(|change| (change.catalog_id, change.kind, change.quantity))
        .collect();
    assert_eq!(
        changes,
        vec![(4200, SideboardKind::In, 3), (4100, SideboardKind::Out, 3)]
    );
    // 0.x summed each card's rows, sideboard or not.
    assert_eq!(reveal_ids(second), vec![(7002, 2), (7003, 1)]);
    assert_eq!(second.opponent_cards_seen[1].name, "Card 7003");

    let third = game(&detail, 3);
    assert!(third.opening_hand.is_empty());
    assert!(third.opponent_cards_seen.is_empty());
    assert!(third.sideboard_changes.is_empty());
    assert!(!third.has_deck_list, "nothing for this game");
    assert_eq!(third.turns, Some(12));
}

#[test]
fn an_imported_match_s_game_log_comes_from_its_0x_detail() {
    let (_dir, conn) = imported_with_detail("matches-imported-log");
    let detail = shown(&conn, "q-imported-detail");
    assert!(game(&detail, 1).has_game_log);
    assert!(!game(&detail, 2).has_game_log, "no line in its window");
    let lines = game_log_lines(&conn, "q-imported-detail", 1, &[], UTC).expect("log");
    assert_eq!(
        lines,
        vec![GameLogLine {
            time: "00:01:41".to_string(),
            message: "Qlocal plays Qisland.".to_string(),
        }]
    );
    assert!(
        game_log_lines(&conn, "q-imported-detail", 2, &[], UTC)
            .expect("log")
            .is_empty()
    );

    // The stored log (0.x's decoded file) is read before any `.dat` file
    // still on disk; the file stands in only without one.
    let dat = vec![GameLogEntry {
        timestamp_ms: 102_000,
        message: "Q from the file.".to_string(),
    }];
    let stored = game_log_lines(&conn, "q-imported-detail", 1, &dat, UTC).expect("log");
    assert_eq!(stored[0].message, "Qlocal plays Qisland.");
    conn.execute("DELETE FROM game_logs", [])
        .expect("no stored log");
    let from_file = game_log_lines(&conn, "q-imported-detail", 1, &dat, UTC).expect("log");
    assert_eq!(from_file.len(), 1);
    assert_eq!(from_file[0].message, "Q from the file.");
}

#[test]
fn an_imported_match_s_0x_log_gives_turns_and_log_only_reveals_as_its_file_did() {
    let bytes = load_game_log("clean_2_0_win");
    let entries = parse_game_log(&bytes, UTC).expect("parses").entries;
    let players = detect_players(&entries);
    let dir = TestDir::new("matches-imported-log-cards");
    let mut conn = store::open_writer(&dir.join("mymtgo.sqlite")).expect("open");
    let mut games = Vec::new();
    for (index, slice) in split_into_games(&entries).iter().enumerate() {
        let number = index as u32 + 1;
        games.push(ImportedGame {
            started_at_ms: slice.first().map(|entry| entry.timestamp_ms),
            ended_at_ms: slice.last().map(|entry| entry.timestamp_ms),
            ..imported_game(number, Some(800 + i64::from(number)), None)
        });
    }
    import_matches(
        &mut conn,
        &[ImportedMatch {
            token: "q-imported-log".to_string(),
            mtgo_match_id: Some(6),
            local_username: players[0].clone(),
            opponent: Some(players[1].clone()),
            format: Some("CMODERN".to_string()),
            match_type: Some("Constructed".to_string()),
            tournament_event_id: None,
            tournament_round: None,
            state: "complete".to_string(),
            outcome: "win".to_string(),
            started_at_ms: entries[0].timestamp_ms,
            ended_at_ms: entries.last().map(|entry| entry.timestamp_ms),
            note: None,
            games,
            manual: None,
            games_won_imported: None,
            games_lost_imported: None,
            deck_linked: false,
            replay_share: None,
            legacy_history: false,
        }],
        1,
    )
    .expect("import");
    let mut log = Vec::new();
    for entry in &entries {
        log.push(DetailLogEntry {
            at_ms: entry.timestamp_ms,
            message: entry.message.clone(),
        });
    }
    import_detail(
        &mut conn,
        "q-imported-log",
        &ImportedDetail {
            games: Vec::new(),
            log,
        },
    )
    .expect("detail");

    let detail = shown(&conn, "q-imported-log");
    let cards = extract_log_cards(&entries);
    for (index, game) in detail.games.iter().enumerate() {
        assert!(game.has_game_log);
        assert_eq!(game.turns, cards.games[index].meta.turn_count);
        assert!(!game.opponent_cards_seen.is_empty(), "log-only reveals");
        assert!(game.opening_hand.is_empty());
        assert!(!game.has_deck_list);
    }
}

#[test]
fn an_unreadable_0x_detail_shows_the_match_without_it() {
    let (_dir, conn) = imported_with_detail("matches-imported-corrupt");
    conn.execute_batch(
        "UPDATE game_timelines SET raw = X'00';
         UPDATE game_logs SET decoded_entries = X'00';",
    )
    .expect("corrupt");
    let detail = shown(&conn, "q-imported-detail");
    assert_eq!(detail.games.len(), 3);
    for game in &detail.games {
        assert!(!game.has_game_log);
    }
    // The rows still show; the frames' turn estimate is gone.
    assert_eq!(game(&detail, 1).opening_hand.len(), 7);
    assert_eq!(game(&detail, 1).turns, None);
    assert!(
        game_log_lines(&conn, "q-imported-detail", 1, &[], UTC)
            .expect("log")
            .is_empty()
    );
}

// ---- Rows equal Plan K's views ----------------------------------------------------

/// What Plan K's page computed for one game from its events, before the
/// game rows existed: the shown hand `(catalog id, bottomed)` with the turn
/// 1 draw, the hands sent back, the mulligan counts, the deck as played and
/// the opponent's cards seen.
struct PlanKGame {
    hand: Vec<(u32, bool)>,
    drawn: Option<u32>,
    mulliganed: Vec<Vec<u32>>,
    mulligans: (u32, u32),
    deck: Vec<(u32, u32, bool)>,
    seen: Vec<(u32, u32)>,
}

fn plan_k_game(
    events: &[mtgo_protocol::PreparedEvent],
    lists: &HashMap<u64, Vec<DeckCard>>,
    account: &str,
    row: &GameRow,
) -> PlanKGame {
    let texts: Vec<&str> = events
        .iter()
        .filter(|event| {
            event.kind == EventKind::GameStateUpdate && event.subjects.game_id == row.mtgo_game_id
        })
        .map(|event| event.text.as_str())
        .collect();
    let frames = parse_frames(&texts);
    let mut found = PlanKGame {
        hand: Vec::new(),
        drawn: None,
        mulliganed: Vec::new(),
        mulligans: (
            row.local_mulligans.unwrap_or(0),
            row.opp_mulligans.unwrap_or(0),
        ),
        deck: Vec::new(),
        seen: Vec::new(),
    };
    let Some((local, opponent)) = frames
        .first()
        .and_then(|first| game_detail::seats(first, account))
    else {
        return found;
    };
    let hand = game_detail::opening_hand(&frames, local, opponent);
    for card in hand.shown_hand() {
        found.hand.push((card.catalog_id, card.bottomed));
    }
    // `drawnIndex` (MatchGame.vue): on the draw, a hand above 7 minus the
    // mulligans ends with the turn 1 draw.
    let expected = 7usize.saturating_sub(hand.local_mulligans as usize);
    if row.on_play != Some(true)
        && found.hand.len() > expected
        && found.hand.last().is_some_and(|(_, bottomed)| !bottomed)
    {
        found.drawn = found.hand.last().map(|(id, _)| *id);
    }
    for sent_back in &hand.mulliganed_hands {
        let mut ids: Vec<u32> = sent_back.iter().map(|card| card.catalog_id).collect();
        ids.sort_unstable();
        found.mulliganed.push(ids);
    }
    found.mulligans = (hand.local_mulligans, hand.opponent_mulligans);
    if let (Some(first), Some(list)) = (
        frames.first(),
        row.mtgo_game_id.and_then(|id| lists.get(&id)),
    ) {
        for card in local_deck(list, first, local) {
            found
                .deck
                .push((card.catalog_id, card.quantity, card.sideboard));
        }
    }
    if let Some(last) = frames.last() {
        found.seen = game_detail::opponent_cards(last, opponent);
    }
    found.hand.sort_unstable();
    found.deck.sort_unstable();
    found.seen.sort_unstable();
    found
}

#[test]
fn game_card_rows_match_plan_k_views_on_every_fixture() {
    let mut checked = 0;
    for fixture in ["draft_hobbit", "draft_incomplete", "league_join_drop"] {
        let (_dir, conn) = tracked(fixture, &format!("matches-plan-k-{fixture}"));
        let tokens: Vec<String> = {
            let mut stmt = conn
                .prepare("SELECT token FROM listed_matches WHERE source = 'log' ORDER BY id")
                .expect("prepare");
            let rows = stmt.query_map([], |row| row.get(0)).expect("query");
            rows.map(|row| row.expect("token")).collect()
        };
        for token in tokens {
            let header = match_header(&conn, &token)
                .expect("header")
                .expect("listed");
            let events = History::default()
                .match_events(&conn, &token)
                .expect("events");
            let lists = first_deck_lists(&events);
            let Some(page) = detail(&conn, &token, &[]).expect("detail") else {
                continue;
            };
            let rows = match_games(&conn, header.id).expect("games");
            for row in &rows {
                let old = plan_k_game(&events, &lists, &header.account, row);
                let shown = game(&page, row.game_number);
                let what = format!("{fixture} {token} game {}", row.game_number);

                let mut hand: Vec<(u32, bool)> = shown
                    .opening_hand
                    .iter()
                    .map(|card| (card.catalog_id, card.bottomed))
                    .collect();
                hand.sort_unstable();
                assert_eq!(hand, old.hand, "{what}: the hand, except its order");
                let drawn: Vec<u32> = shown
                    .opening_hand
                    .iter()
                    .filter(|card| card.first_draw)
                    .map(|card| card.catalog_id)
                    .collect();
                assert_eq!(
                    drawn,
                    old.drawn.into_iter().collect::<Vec<_>>(),
                    "{what}: the draw"
                );
                if let Some(last) = shown.opening_hand.last()
                    && old.drawn.is_some()
                {
                    assert!(last.first_draw, "{what}: the draw shows last");
                }
                let mulliganed: Vec<Vec<u32>> = shown
                    .mulliganed_hands
                    .iter()
                    .map(|hand| {
                        let mut ids: Vec<u32> = hand.iter().map(|card| card.catalog_id).collect();
                        ids.sort_unstable();
                        ids
                    })
                    .collect();
                assert_eq!(mulliganed, old.mulliganed, "{what}: the hands sent back");
                assert_eq!(
                    (shown.local_mulligans, shown.opponent_mulligans),
                    old.mulligans,
                    "{what}: the mulligans"
                );
                let cards = store::game_sync::read_game_cards(&conn, row.id).expect("rows");
                let mut deck = Vec::new();
                for card in &cards.cards {
                    if card.side != Side::Local {
                        continue;
                    }
                    if card.main_qty > 0 {
                        deck.push((card.catalog_id, card.main_qty, false));
                    }
                    if card.side_qty > 0 {
                        deck.push((card.catalog_id, card.side_qty, true));
                    }
                }
                deck.sort_unstable();
                assert_eq!(deck, old.deck, "{what}: the deck as played");
                assert_eq!(shown.has_deck_list, !old.deck.is_empty(), "{what}");
                let mut seen = reveal_ids(shown);
                seen.sort_unstable();
                assert_eq!(seen, old.seen, "{what}: the reveals, except their order");
                assert!(
                    shown.sideboard_changes.is_empty(),
                    "no deck files: no registered list"
                );
                checked += 1;
            }
        }
    }
    // Every game of every listed log match of the three fixtures.
    assert_eq!(checked, 9);
}

// ---- Archetypes (archetypes spec sections 8 and 12) ----------------------------

fn insert_archetype(
    conn: &Connection,
    uuid: &str,
    name: &str,
    format: Option<&str>,
    source: &str,
) -> i64 {
    conn.execute(
        "INSERT INTO archetypes (uuid, name, format, color_identity, source, created_at, updated_at)
         VALUES (?1, ?2, ?3, 'B,R', ?4, 0, 0)",
        (uuid, name, format, source),
    )
    .expect("archetype");
    conn.last_insert_rowid()
}

fn store_matches(state: &AppState, tokens: &[&str]) {
    let mut writer = store::open_writer(&state.db_path).expect("writer");
    let mut matches = Vec::new();
    for (index, token) in tokens.iter().enumerate() {
        matches.push(MatchWrite::Upsert {
            view: Box::new(crate::summary::testing::view(
                token,
                1_000 + index as i64 * 10_000,
                MatchState::Complete,
            )),
            revision: 1,
        });
    }
    commit(
        &mut writer,
        &WriteBatch {
            matches,
            ..WriteBatch::default()
        },
    )
    .expect("matches");
}

/// `(token, archetype id, confidence, manual)` of every opponent seat.
fn opponent_seats(state: &AppState) -> Vec<(String, i64, f64, i64)> {
    let conn = store::open_reader(&state.db_path).expect("reader");
    let mut stmt = conn
        .prepare(
            "SELECT m.token, x.archetype_id, x.confidence, x.manual
             FROM match_archetypes x JOIN matches m ON m.id = x.match_id
             WHERE x.is_opponent = 1 ORDER BY m.token",
        )
        .expect("prepare");
    stmt.query_map([], |row| {
        Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
    })
    .expect("query")
    .map(|row| row.expect("row"))
    .collect()
}

/// 0.x `BulkUpdateArchetypeTest` (six), through the tracker.
#[test]
fn setting_the_opponent_archetype_ports_bulk_update_archetype() {
    let dir = TestDir::new("matches-set-archetype");
    let (state, _events) = crate::commands::tests::ready_state(&dir);
    store_matches(&state, &["q-1", "q-2", "q-3"]);
    let (old, new) = {
        let writer = store::open_writer(&state.db_path).expect("writer");
        let old = insert_archetype(&writer, "q-old", "Qold", Some("modern"), "downloaded");
        let new = insert_archetype(&writer, "q-new", "Qnew", Some("modern"), "downloaded");
        writer
            .execute(
                "INSERT INTO match_archetypes (match_id, is_opponent, archetype_id, confidence)
                 SELECT id, 1, ?1, 0.5 FROM matches WHERE token = 'q-3'",
                [old],
            )
            .expect("old row");
        (old, new)
    };
    let id = |value: i64| u32::try_from(value).expect("id");
    let tokens = |list: &[&str]| {
        list.iter()
            .map(|token| token.to_string())
            .collect::<Vec<_>>()
    };

    // "validates match_ids and archetype_id are required".
    assert_eq!(
        set_opponent(&state, &[], Some(id(new)), 5),
        Err(CommandError::Refused(
            "The match ids field is required.".to_string()
        ))
    );
    // "validates archetype exists".
    assert_eq!(
        set_opponent(&state, &tokens(&["q-1"]), Some(99_999), 5),
        Err(CommandError::Refused(
            "The selected archetype id is invalid.".to_string()
        ))
    );
    assert!(opponent_seats(&state).iter().all(|seat| seat.1 == old));

    // "sets archetype for multiple matches", "updates existing archetypes
    // when bulk setting" (one row per seat), "marks a bulk-assigned
    // archetype manual so a re-detect cannot wipe it".
    set_opponent(&state, &tokens(&["q-1", "q-2", "q-3"]), Some(id(new)), 5).expect("set");
    assert_eq!(
        opponent_seats(&state),
        vec![
            ("q-1".to_string(), new, 1.0, 1),
            ("q-2".to_string(), new, 1.0, 1),
            ("q-3".to_string(), new, 1.0, 1),
        ]
    );

    // "skips matches with no opponent".
    {
        let writer = store::open_writer(&state.db_path).expect("writer");
        writer
            .execute(
                "UPDATE matches SET opponent_id = NULL WHERE token = 'q-1'",
                [],
            )
            .expect("no opponent");
        writer
            .execute(
                "DELETE FROM match_archetypes WHERE match_id = (SELECT id FROM matches WHERE token = 'q-1')",
                [],
            )
            .expect("no row");
    }
    set_opponent(&state, &tokens(&["q-1"]), Some(id(old)), 6).expect("skipped");
    assert_eq!(opponent_seats(&state).len(), 2);

    // The match page pencil's "none" clears the seat.
    set_opponent(&state, &tokens(&["q-2"]), None, 7).expect("cleared");
    assert_eq!(
        opponent_seats(&state),
        vec![("q-3".to_string(), new, 1.0, 1)]
    );
    let conn = store::open_reader(&state.db_path).expect("reader");
    let row = shown(&conn, "q-3");
    assert_eq!(
        row.opponent_archetype,
        Some(OpponentArchetypeView {
            id: new,
            name: "Qnew".to_string(),
            colors: vec!["B".to_string(), "R".to_string()],
            commander_cards: Vec::new(),
        })
    );
    assert_eq!(shown(&conn, "q-2").opponent_archetype, None);
    lock(&state.tracker).take().expect("tracker").stop();
}

#[test]
fn the_picker_offers_the_formats_archetypes_and_the_fallbacks() {
    let dir = TestDir::new("matches-archetype-options");
    let conn = store::open_writer(&dir.join("mymtgo.sqlite")).expect("open");
    insert_archetype(&conn, "q-burn", "Qburn", Some("modern"), "downloaded");
    insert_archetype(&conn, "q-belcher", "Qbelcher", Some("legacy"), "downloaded");
    let names = |format: Option<&str>| -> Vec<(String, bool)> {
        archetype_option_views(&conn, format)
            .expect("options")
            .into_iter()
            .map(|option| (option.name, option.is_fallback))
            .collect()
    };
    let modern = vec![
        ("Qburn".to_string(), false),
        ("Homebrew".to_string(), true),
        ("Rogue".to_string(), true),
    ];
    assert_eq!(names(Some("CMODERN")), modern);
    assert_eq!(names(Some("Modern")), modern, "a label works too");
    // No format (a bulk set over mixed formats) offers every archetype, as
    // 0.x's picker did with a null format; a limited code only the fallbacks.
    assert_eq!(
        names(None),
        vec![
            ("Qbelcher".to_string(), false),
            ("Qburn".to_string(), false),
            ("Homebrew".to_string(), true),
            ("Rogue".to_string(), true),
        ]
    );
    assert_eq!(names(Some("DMH3")).len(), 2, "only the fallbacks");
    let option = &archetype_option_views(&conn, Some("CMODERN")).expect("options")[0];
    assert_eq!(option.format.as_deref(), Some("modern"));
    assert_eq!(option.colors, ["B", "R"]);
}

#[test]
fn the_picker_marks_the_players_own_archetypes_as_custom() {
    let dir = TestDir::new("matches-archetype-options-custom");
    let conn = store::open_writer(&dir.join("mymtgo.sqlite")).expect("open");
    // The player's own Mono-Blue Tron beside MyMTGO's: same name, one custom.
    let own = insert_archetype(&conn, "q-own", "Qtron", Some("modern"), "manual");
    let server = insert_archetype(&conn, "q-server", "Qtron", Some("modern"), "downloaded");
    // Held as Custom by the offline download: the website's, so not custom.
    let held = insert_archetype(&conn, "q-held", "Qheld", Some("modern"), "manual");
    conn.execute(
        "UPDATE archetypes SET offline_held = 1 WHERE id = ?1",
        [held],
    )
    .expect("held");
    for format in [Some("CMODERN"), None] {
        let custom: Vec<(i64, bool)> = archetype_option_views(&conn, format)
            .expect("options")
            .into_iter()
            .filter(|option| !option.is_fallback)
            .map(|option| (option.id, option.is_custom))
            .collect();
        assert_eq!(custom, vec![(held, false), (own, true), (server, false)]);
    }
}

#[test]
fn a_manual_match_shows_its_opponent_archetype_and_quick_picks() {
    let (_dir, conn) = manual_match("matches-manual-archetype");
    assert_eq!(
        shown(&conn, "q-manual")
            .manual_editing
            .expect("editing")
            .archetype_decklist,
        None
    );
    let archetype = insert_archetype(&conn, "q-opp", "Qopp", Some("modern"), "manual");
    conn.execute_batch(&format!(
        "INSERT INTO match_archetypes (match_id, is_opponent, archetype_id, confidence, manual)
             VALUES (1, 1, {archetype}, 1.0, 1);
         INSERT INTO archetype_cards (archetype_id, list, catalog_id, quantity, sideboard)
             VALUES ({archetype}, 1, 4002, 4, 0), ({archetype}, 1, 4003, 2, 1);"
    ))
    .expect("seat and list");
    let detail = shown(&conn, "q-manual");
    assert_eq!(
        detail.opponent_archetype.as_ref().map(|a| a.name.as_str()),
        Some("Qopp")
    );
    let picks = detail
        .manual_editing
        .expect("editing")
        .archetype_decklist
        .expect("a list");
    let shown_picks: Vec<(u32, &str, u32, bool)> = picks
        .iter()
        .map(|card| {
            (
                card.catalog_id,
                card.name.as_str(),
                card.quantity,
                card.sideboard,
            )
        })
        .collect();
    assert_eq!(
        shown_picks,
        vec![(4002, "Qgoyf", 4, false), (4003, "Qrest", 2, true)]
    );
}

#[test]
fn the_list_filters_sorts_and_counts_by_archetype() {
    let json = serde_json::json!({
        "from": null, "to": null, "result": null, "kind": null,
        "archetype": "none", "sort": "archetype", "sortDir": "asc", "page": 1,
    });
    let query: MatchListQuery = serde_json::from_value(json).expect("parses");
    assert_eq!(query.sort, MatchSortKey::Archetype);
    assert!(!query.is_unfiltered());
    let filter = match_filter(&query, Range::all_time(), chrono_tz::UTC).expect("filter");
    assert_eq!(filter.archetype, Some(ArchetypeFilter::Unknown));
    assert_eq!(filter.sort, SortKey::Archetype);
    let by_id = MatchListQuery {
        archetype: Some("42".to_string()),
        ..MatchListQuery::newest_first(1)
    };
    assert_eq!(
        match_filter(&by_id, Range::all_time(), chrono_tz::UTC)
            .expect("filter")
            .archetype,
        Some(ArchetypeFilter::Is(42))
    );

    let (_dir, conn) = manual_match("matches-list-archetypes");
    let archetype = insert_archetype(&conn, "q-opp", "Qopp", Some("modern"), "manual");
    let page = crate::decks::matches(
        &conn,
        5,
        None,
        "alltime",
        &MatchListQuery::newest_first(1),
        10_000,
        chrono_tz::UTC,
        None,
    )
    .expect("page");
    assert_eq!(page.unknown_archetype_count, 1);
    let options: Vec<(&str, u32)> = page
        .archetypes
        .iter()
        .map(|option| (option.name.as_str(), option.match_count))
        .collect();
    assert_eq!(options, [("Homebrew", 0), ("Qopp", 0), ("Rogue", 0)]);
    conn.execute(
        "INSERT INTO match_archetypes (match_id, is_opponent, archetype_id, confidence)
         VALUES (1, 1, ?1, 1.0)",
        [archetype],
    )
    .expect("seat");
    let page = crate::decks::matches(
        &conn,
        5,
        None,
        "alltime",
        &MatchListQuery::newest_first(1),
        10_000,
        chrono_tz::UTC,
        None,
    )
    .expect("page");
    assert_eq!(page.unknown_archetype_count, 0);
    assert_eq!(page.archetypes[0].match_count, 1);
    assert_eq!(
        page.rows[0]
            .opponent_archetype
            .as_ref()
            .map(|a| a.name.as_str()),
        Some("Qopp")
    );
}

#[test]
fn an_arena_match_page_has_games_and_no_detail() {
    use crate::commands::tests::ready_state;
    let dir = TestDir::new("arena-match-page");
    let (state, _events) = ready_state(&dir);
    crate::platform::tests::seed_both_platforms(&state);
    let conn = store::open_writer(&state.db_path).expect("writer");
    conn.execute(
        "UPDATE matches SET match_type = 'ranked' WHERE token = 'q-match-arena'",
        [],
    )
    .expect("match type");
    conn.execute(
        "UPDATE opponents SET username = 'Opponent' WHERE username = 'Qarenaopp'",
        [],
    )
    .expect("opponent");
    let match_id: i64 = conn
        .query_row(
            "SELECT id FROM matches WHERE token = 'q-match-arena'",
            [],
            |row| row.get(0),
        )
        .expect("id");
    for (number, result) in [(1, "win"), (2, "loss"), (3, "win")] {
        conn.execute(
            "INSERT INTO games (match_id, game_number, result, started_at, ended_at, end_reason)
             VALUES (?1, ?2, ?3, 1000, 2000, 'concede')",
            store::params![match_id, number, result],
        )
        .expect("game");
    }
    let page = detail(&conn, "q-match-arena", &[])
        .expect("no error")
        .expect("the match is listed");
    assert_eq!(page.platform, crate::platform::PlatformView::Arena);
    assert_eq!(page.opponent.as_deref(), Some("Opponent"));
    assert_eq!(page.format.as_deref(), Some("Pioneer"));
    assert_eq!(page.match_type.as_deref(), Some("Ranked"));
    assert_eq!(page.games.len(), 3);
    for game in &page.games {
        assert!(!game.has_frames);
        assert!(!game.has_game_log);
    }
    let mtgo = detail(&conn, "q-match-mtgo", &[])
        .expect("no error")
        .expect("listed");
    assert_eq!(mtgo.platform, crate::platform::PlatformView::Mtgo);
    drop(conn);
    crate::platform::tests::stop(&state);
}

// ---- Arena game detail -------------------------------------------------------

/// The Arena fixture log `name` read by the tracker into a fresh database
/// (its Bo3 match synced into game rows). Returns the folder (kept alive), a
/// connection and the match's token.
pub(crate) fn tracked_arena(name: &str, label: &str) -> (TestDir, Connection, String) {
    let dir = TestDir::new(label);
    dir.write(
        "arena/Player.log",
        &fixture_tools::arena_fixture(name).bytes,
    );
    let config = TrackerConfig {
        db_path: dir.join("data/mymtgo.sqlite"),
        timezone: UTC,
        os_timezone: None,
        log_overrides: vec![dir.join("logs")],
        arena_log_overrides: vec![dir.join("arena/Player.log")],
        mtgo_data_folder: Some(dir.join("mtgo-data")),
        replay_day_anchor: None,
        watching: true,
        arena_enabled: true,
        endstep_enabled: false,
    };
    let mut now = 1_793_720_000_000;
    let mut engine = Engine::open(
        config.clone(),
        Env::current(),
        Arc::default(),
        Vec::new(),
        now,
    )
    .expect("open engine");
    for _ in 0..1_000 {
        let tick = engine.tick(now).expect("tick");
        now += 1;
        if !tick.more {
            break;
        }
    }
    for _ in 0..3 {
        now += 2_000;
        engine.tick(now).expect("tick");
    }
    engine.flush().expect("flush");
    drop(engine);
    let conn = store::open_writer(&config.db_path).expect("writer");
    let token = conn
        .query_row("SELECT token FROM matches", [], |row| row.get(0))
        .expect("the arena match");
    (dir, conn, token)
}

#[test]
fn arena_detail_has_hands_and_opponent_cards() {
    let (_dir, conn, token) = tracked_arena("bo3-three-games", "arena-detail-hands");
    let page = detail(&conn, &token, &[])
        .expect("no error")
        .expect("the match is listed");
    assert_eq!(page.platform, crate::platform::PlatformView::Arena);
    assert_eq!(page.games.len(), 3);
    let first = &page.games[0];
    assert!(!first.opening_hand.is_empty(), "game 1 has an opening hand");
    assert!(
        !first.opponent_cards_seen.is_empty(),
        "game 1 shows the opponent's cards"
    );
    assert!(first.has_deck_list);
    assert!(first.turns.is_some(), "turns come from games.turn_count");
    for game in &page.games {
        assert!(game.has_game_log, "game {} has a game log", game.number);
    }
}

/// Keep games 1 and 2's stored messages only, as live rows.
fn keep_only_games_one_and_two(conn: &mut Connection, token: &str) {
    let kept: Vec<store::arena_games::NewGameMessage> =
        store::arena_games::game_messages(conn, token)
            .expect("messages")
            .into_iter()
            .filter(|message| message.game_number != Some(3))
            .map(|message| store::arena_games::NewGameMessage {
                file_id: message.file_id,
                byte_offset: message.byte_offset,
                seq: message.seq,
                match_token: token.to_string(),
                game_number: message.game_number,
                source: message.source,
                logged_at: message.logged_at,
                json: message.json,
            })
            .collect();
    conn.execute("DELETE FROM arena_game_archives", [])
        .expect("archive");
    store::write::commit(
        conn,
        &store::write::WriteBatch {
            arena_game_messages: kept,
            ..store::write::WriteBatch::default()
        },
    )
    .expect("messages");
}

#[test]
fn an_arena_game_without_messages_has_no_game_log() {
    let (_dir, mut conn, token) = tracked_arena("bo3-three-games", "arena-detail-game-log");
    keep_only_games_one_and_two(&mut conn, &token);
    let page = detail(&conn, &token, &[])
        .expect("no error")
        .expect("listed");
    let with_log: Vec<u32> = page
        .games
        .iter()
        .filter(|game| game.has_game_log)
        .map(|game| game.number)
        .collect();
    assert_eq!(with_log, vec![1, 2]);
}

#[test]
fn arena_games_with_messages_are_replayable() {
    let (_dir, mut conn, token) = tracked_arena("bo3-three-games", "arena-detail-replay");
    let page = detail(&conn, &token, &[])
        .expect("no error")
        .expect("listed");
    assert_eq!(page.games.len(), 3);
    assert!(page.games.iter().all(|game| game.has_frames));

    // A game without stored messages has no replay.
    keep_only_games_one_and_two(&mut conn, &token);
    let page = detail(&conn, &token, &[])
        .expect("no error")
        .expect("listed");
    let replayable: Vec<u32> = page
        .games
        .iter()
        .filter(|game| game.has_frames)
        .map(|game| game.number)
        .collect();
    assert_eq!(replayable, vec![1, 2]);
}

#[test]
fn arena_game_log_lines() {
    let (_dir, conn, token) = tracked_arena("bo3-three-games", "arena-game-log");
    let tz: Tz = chrono_tz::Europe::London;
    let unnamed = game_log_lines(&conn, &token, 1, &[], tz).expect("lines");
    assert!(!unnamed.is_empty(), "game 1 has log lines");
    // Without card data the line names the card as the rest of the app does.
    // The opponent was on the play in game 1: the first local line is later.
    let index = unnamed
        .iter()
        .position(|line| line.message.starts_with("You "))
        .expect("a local line");
    let first = &unnamed[index];
    let id: u32 = first
        .message
        .rsplit("Card ")
        .next()
        .and_then(|id| id.parse().ok())
        .unwrap_or_else(|| panic!("an unnamed card: {}", first.message));
    assert!(
        first.message.starts_with("You drew Card ")
            || first.message.starts_with("You played Card "),
        "{}",
        first.message
    );
    conn.execute(
        "INSERT INTO cards (catalog_id, name) VALUES (?1, 'Qfirst')
         ON CONFLICT (catalog_id) DO UPDATE SET name = excluded.name",
        [id],
    )
    .expect("card");
    let named = game_log_lines(&conn, &token, 1, &[], tz).expect("lines");
    let line = &named[index];
    assert!(line.message.ends_with(" Qfirst"), "{}", line.message);
    assert_eq!(line.time.len(), 8);
    assert_eq!(line.time.as_bytes()[2], b':');
    assert_eq!(line.time.as_bytes()[5], b':');
    // Every line is one of the five shapes.
    for line in &named {
        let known = [
            "You drew ",
            "You played ",
            "You cast ",
            "Opponent played ",
            "Opponent cast ",
        ];
        assert!(
            known.iter().any(|start| line.message.starts_with(start)),
            "{}",
            line.message
        );
    }
    assert!(
        named
            .iter()
            .any(|line| line.message.starts_with("Opponent "))
    );
    assert!(
        game_log_lines(&conn, &token, 9, &[], tz)
            .expect("lines")
            .is_empty()
    );
}

/// A commander format match shows the opponent's commanders (partners both,
/// in seat order) as full cards; every other format has none.
#[test]
fn a_brawl_match_names_the_opponents_commanders() {
    let (_dir, conn, token) = tracked_arena("bo3-three-games", "arena-detail-commanders");
    conn.execute_batch(
        "INSERT INTO cards (catalog_id, name, type, color_identity, image, local_image)
         VALUES (900001, 'Qomnath', 'Legendary Creature', 'G,R,U,W',
                 'https://cards/900001.jpg', '900001.jpg'),
                (900003, 'Qglissa', 'Legendary Creature', 'B,G', NULL, NULL);
         INSERT INTO match_commanders (match_id, seat, position, catalog_id)
         SELECT id, 'opponent', 1, 900002 FROM matches;
         INSERT INTO match_commanders (match_id, seat, position, catalog_id)
         SELECT id, 'opponent', 0, 900001 FROM matches;
         INSERT INTO match_commanders (match_id, seat, position, catalog_id)
         SELECT id, 'player', 0, 900003 FROM matches;",
    )
    .expect("commanders");
    let page = detail(&conn, &token, &[])
        .expect("no error")
        .expect("listed");
    assert!(
        page.opponent_commanders.is_empty(),
        "not a commander format"
    );

    conn.execute("UPDATE matches SET format = 'ARENA_HISTORIC_BRAWL'", [])
        .expect("brawl");
    let page = detail(&conn, &token, &[])
        .expect("no error")
        .expect("listed");
    assert_eq!(
        page.opponent_commanders,
        vec![
            CommanderCardView {
                catalog_id: 900001,
                name: "Qomnath".to_string(),
                image: Some("https://cards/900001.jpg".to_string()),
                local_image: Some("900001.jpg".to_string()),
                colors: vec![
                    "W".to_string(),
                    "U".to_string(),
                    "R".to_string(),
                    "G".to_string()
                ],
            },
            CommanderCardView {
                catalog_id: 900002,
                name: "Card 900002".to_string(),
                image: None,
                local_image: None,
                colors: Vec::new(),
            },
        ]
    );
}
